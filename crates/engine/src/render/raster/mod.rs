//! Raster tiles (WMTS / XYZ in Web Mercator, 256 px tiles): `RasterSource` loads and keeps the tiles of one layer,
//! `RasterPass` draws a stack of up to four layers in a single pass.
//!
//! All the tiles of a source live in one texture array (a fixed pool of slots, recycled least recently used first).
//! Images are decoded by the browser (`createImageBitmap`, off the main thread) and uploaded straight into their slot,
//! a few per frame so a burst of arrivals never stalls a frame. While a tile loads, its nearest loaded ancestor (or
//! its loaded descendants, when zooming out) stands in, so panning and zooming never show holes. Requests for tiles
//! that left the view are aborted, so the download slots always serve the current view.
//!
//! Consecutive raster layers are composited in the fragment shader, as iTowns does: one instanced draw call for the
//! whole stack, each pixel shaded once, whatever the number of tiles and layers. A tile fading in is mixed with its
//! stand-in in the same shader.
//!
//! An elevation source (raw altitudes, `image/x-bil;bits=32`, an `ArcGIS` raw grid or the floats of the page's loader,
//! in an R32F pool) gives the relief: the vertex shader lifts the grid of every tile by the heights read in it, three
//! levels coarser than the imagery (a 16 × 16 grid needs far fewer samples than 256 px), and a skirt under the tile
//! edges hides the cracks between levels.
//!
//! - `cache`: which tile is in which slot, least recently used order, failures, what to draw and load (pure)
//! - `data`: the altitudes made ready for the pool, the URLs of the tiles (pure)
//! - `fetch`: the downloads
//! - `pass`: the GPU pass; `shaders`: its shaders

mod cache;
mod data;
mod fetch;
mod pass;
mod shaders;

pub use cache::simulate_plans;
pub use data::{terrain_geoid, terrain_levels, unpack_depth};
pub use pass::RasterPass;
pub use shaders::VERTEX;

use crate::layer::Loading;
use crate::render::gl;
use cache::{Cache, Install, Motion, Plan, Sample};
use fetch::{Fetch, Pixels};
use geo::camera::View;
use geo::tiles::{self, Selected, TileId};
use glow::HasContext;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

const TILE: i32 = 256;
/// Finest level a raster source accepts (tile coordinates stay within `u32`, `1 << z` too)
pub const MAX_Z: u8 = 30;
/// Texture slots per layer (256 KB each): 256 slots = 64 MB of GPU memory
const SLOTS: usize = 256;
/// GPU memory of the pool of a layer with larger tiles (bytes): its slot count follows, from 96 to `SLOTS`
const POOL_BYTES: usize = 96 << 20;
/// Target size of a tile on screen, CSS pixels: tiles are shown between 181 and 362 px
const TILE_PX: f64 = 362.0;
/// Levels searched below a missing tile for stand-ins
const DESCENDANT_LEVELS: u8 = 3;
/// After its last retry, a tile that kept failing is asked again after this delay (ms), doubled at each new failure
/// up to `COOLDOWN_MAX_MS`
const COOLDOWN_MS: f64 = 2_000.0;
const COOLDOWN_MAX_MS: f64 = 30_000.0;
/// Pauses before the retries of a tile after a transient error (milliseconds; the last attempt has none)
const RETRY_PAUSES_MS: [i32; 3] = [400, 800, 0];
/// Delay after the camera stops before the deferred (off-center) tiles load (ms)
const REST_MS: i32 = 120;
/// Age under which a request no longer needed is let finish rather than aborted (ms)
const STALE_GRACE_MS: f64 = 300.0;
/// Tiles of a flight destination loaded in advance, per layer
const PRELOAD_TILES: usize = 48;
/// When nothing covers a tile yet, a tile this many levels coarser is loaded first: it arrives fast and covers the
/// area while the fine tiles load (shallow tiles first, like 3DTilesRendererJS)
const COVER_LEVELS: u8 = 2;
/// Layers composited in one pass (vertex attributes: 3 per layer + 4, the 16 of WebGL2)
pub const STACK: usize = 4;
/// Floats per instance: plane rect (4), Mercator rect (4), then per layer: uv of the tile (4), uv of its stand-in
/// (4), slots of both and fade (3), and the heights: offset (2), scale (1) and slot (1) in the elevation pool
const INSTANCE: usize = 8 + STACK * 11 + 4;
/// Elevation tiles are chosen this many times larger on screen than imagery tiles: three levels coarser
const ELEVATION_SCALE: f64 = 8.0;

/// Whether a tile is a descendant of a wanted tile close enough to stand in for it (`DESCENDANT_LEVELS`)
fn stands_in_for(tile: TileId, wanted: &HashSet<TileId>) -> bool {
    let mut t = tile.parent();

    for _ in 0..DESCENDANT_LEVELS {
        let Some(a) = t else {
            return false;
        };

        if wanted.contains(&a) {
            return true;
        }
        t = a.parent();
    }

    false
}

pub struct RasterOptions {
    /// Template: `{z}`, `{x}`, `{y}` (WMTS, XYZ) or `{bbox}` (WMS `GetMap` in EPSG:3857, `ArcGIS` `exportImage`: west,
    /// south, east, north in Mercator metres of the body)
    pub url: String,
    pub min_z: u8,
    pub max_z: u8,
    pub opacity: f32,
    pub saturation: f32,
    /// Raw altitudes (`image/x-bil;bits=32`) rather than images: the relief
    pub elevation: bool,
    /// The body the tiles cover: its size for `{bbox}`, its relief for the no-data of the altitudes
    pub body: geo::geodesy::Body,
    /// Altitudes raised by the RAF20 geoid height, to heights above the ellipsoid (those of 3D Tiles)
    pub geoid: bool,
    /// Side of the textures of the tiles, pixels (256; more for tiles drawn by the page, sharper)
    pub size: i32,
    /// Draws the tiles instead of fetching `url`: `(z, x, y, signal) => Promise<ImageBitmap | null>`, or
    /// `Promise<Float32Array | null>` of 256 × 256 altitudes for the relief (null: no tile here; a rejection: an error
    /// worth a retry)
    pub loader: Option<js_sys::Function>,
}

impl RasterOptions {
    /// Slots of the pool of this source
    fn slots(&self) -> usize {
        if self.size <= TILE {
            return SLOTS;
        }

        (POOL_BYTES / (self.size as usize * self.size as usize * 4)).clamp(96, SLOTS)
    }
}

/// What a raster layer needs to load a frame's tiles
pub struct DrawContext<'a> {
    pub view: &'a View,
    pub frame: u64,
    pub redraw: &'a Rc<dyn Fn()>,
    /// The camera moved during the last frames
    pub moving: bool,
    /// Destination of a camera flight (generation, view), to load in advance
    pub preload: Option<(u64, &'a View)>,
    pub loading: Loading,
    /// The time (ms)
    pub now: f64,
}

/// Side of the copy of an elevation tile kept on the CPU, to know the altitude under a point
const CPU_HEIGHTS: usize = 32;

type Inbox = Rc<RefCell<Vec<(TileId, Result<Pixels, Fetch>)>>>;

/// Counters of the last frame, for diagnostics
#[derive(Default, Clone, Copy)]
pub struct RasterStats {
    pub drawn: usize,
    pub loaded: usize,
    pub in_flight: usize,
    pub queued: usize,
    /// Drawn with an ancestor's texture (the tile itself is not loaded yet)
    pub stand_ins: usize,
    /// Waiting before a new attempt after errors
    pub cooling: usize,
    /// Not served (404): covered by an ancestor
    pub missing: usize,
}

/// Columns of the canvas a raster layer shows in when it is not cut by a swipe (device pixels)
pub const UNCLIPPED: [f32; 2] = [-1e9, 1e9];

pub struct RasterSource {
    pub options: RasterOptions,
    /// The columns of the canvas it shows in (device pixels, from the left): `UNCLIPPED`, or one side of a swipe
    pub clip: [f32; 2],
    pool: glow::Texture,
    cache: Cache,
    /// Requests in flight: their abort handle and start time (ms)
    in_flight: HashMap<TileId, (web_sys::AbortController, f64)>,
    inbox: Inbox,
    /// Arrived, waiting for their upload, and the same tiles as a set
    queue: VecDeque<(TileId, Pixels)>,
    queued: HashSet<TileId>,
    /// This frame's plan: tiles to draw when this source is the base of a pass (stand-ins included), tiles to load
    plan: Plan,
    /// Buffers of `prepare`, kept from frame to frame: the tiles wanted, those kept loading (and their ancestors)
    wanted: Vec<Selected>,
    wanted_set: HashSet<TileId>,
    keep: HashSet<TileId>,
    fading: bool,
    /// A redraw is scheduled for the deferred tiles
    wake: Rc<std::cell::Cell<bool>>,
    /// Flight destination: generation and its tiles
    preload: (u64, Vec<TileId>),
    /// Changes whenever a tile enters the pool (unique across sources): what reads the pool knows when to look again
    generation: u64,
    pub stats: RasterStats,
}

/// Source of `RasterSource::generation`
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

impl RasterSource {
    pub fn new(gl: &glow::Context, mut options: RasterOptions) -> Result<Self, String> {
        options.max_z = options.max_z.min(MAX_Z);
        options.min_z = options.min_z.min(options.max_z);

        let slots = options.slots();
        // SAFETY: object creation and state setup on a live WebGL2 context
        let pool = unsafe {
            let pool = gl.create_texture()?;

            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(pool));

            // Altitudes are read with texelFetch (R32F cannot be filtered in WebGL2)
            let (format, filter) = if options.elevation {
                (glow::R32F, glow::NEAREST)
            } else {
                (glow::RGBA8, glow::LINEAR)
            };
            let size = if options.elevation { TILE } else { options.size.max(1) };

            gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY, 1, format, size, size, slots as i32);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, filter),
                (glow::TEXTURE_MAG_FILTER, filter),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, key, value as i32);
            }

            pool
        };

        Ok(Self {
            clip: UNCLIPPED,
            cache: Cache::new(options.min_z, options.max_z, slots),
            options,
            pool,
            in_flight: HashMap::new(),
            inbox: Rc::default(),
            queue: VecDeque::new(),
            queued: HashSet::new(),
            plan: Plan::default(),
            wanted: Vec::new(),
            wanted_set: HashSet::new(),
            keep: HashSet::new(),
            fading: false,
            wake: Rc::default(),
            preload: (0, Vec::new()),
            generation: next_generation(),
            stats: RasterStats::default(),
        })
    }

    /// Takes the tiles that arrived, uploads a few of them (those of `wanted_set` first)
    fn receive(&mut self, gl: &glow::Context, frame: u64, now: f64, budget_ms: f64) {
        for (tile, pixels) in self.inbox.borrow_mut().drain(..) {
            self.in_flight.remove(&tile);

            match pixels {
                Ok(pixels) => {
                    self.cache.succeeded(tile);

                    if self.queued.insert(tile) {
                        self.queue.push_back((tile, pixels));
                    } else {
                        pixels.close();
                    }
                }
                Err(Fetch::Missing) => self.cache.missing(tile),
                Err(Fetch::Transient) => self.cache.failed(tile, now),
            }
        }

        // Tiles no longer needed (neither wanted nor kept by `abort_stale`, nor a descendant standing in for a wanted
        // tile while zooming out) are dropped: a decoded image may hold 4 MB outside the heap
        let (wanted, keep, queued) = (&self.wanted_set, &self.keep, &mut self.queued);

        self.queue.retain(|(tile, pixels)| {
            let needed = keep.contains(tile) || stands_in_for(*tile, wanted);

            if !needed {
                queued.remove(tile);
                pixels.close();
            }

            needed
        });
        // Tiles of the current view first, the others (stand-ins, tiles that left the view) after
        self.queue
            .make_contiguous()
            .sort_by_key(|(tile, _)| !wanted.contains(tile));

        // Uploads within the frame's time budget (at least one), the rest waits for the next frames
        let start = crate::js::now();
        let mut uploaded = 0;

        while uploaded == 0 || crate::js::now() - start < budget_ms {
            let Some((tile, pixels)) = self.queue.pop_front() else {
                break;
            };

            uploaded += 1;

            let heights = match &pixels {
                Pixels::Heights(heights) => Some(data::subsample(heights)),
                Pixels::Image(_) => None,
            };
            let slot = match self.cache.install(tile, heights, frame, now) {
                Install::Slot(slot) => slot,
                Install::Duplicate => {
                    self.queued.remove(&tile);
                    pixels.close();
                    continue;
                }
                Install::Full => {
                    // Every slot is held by this frame: the tile is let go (the plan asks for it again once the
                    // pool has room; kept waiting, it would ask for a frame after another)
                    self.queued.remove(&tile);
                    pixels.close();
                    continue;
                }
            };

            self.queued.remove(&tile);

            let size = self.options.size;

            // SAFETY: upload into an allocated slot of the pool
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.pool));

                match &pixels {
                    Pixels::Image(bitmap) => gl::upload_bitmap_layer(gl, i32::from(slot), size, bitmap),
                    Pixels::Heights(heights) => gl.tex_sub_image_3d(
                        glow::TEXTURE_2D_ARRAY,
                        0,
                        0,
                        0,
                        i32::from(slot),
                        TILE,
                        TILE,
                        1,
                        glow::RED,
                        glow::FLOAT,
                        glow::PixelUnpackData::Slice(Some(gl::bytes(heights))),
                    ),
                }
            }
            pixels.close();
            self.generation = next_generation();
        }
    }

    /// The texture array of the pool
    pub fn pool(&self) -> glow::Texture {
        self.pool
    }

    /// Changes whenever the pool content changes (a tile entered it, maybe in the slot of an evicted one)
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Tiles in the pool and their slots
    pub fn loaded(&self) -> impl Iterator<Item = (TileId, u16)> + '_ {
        self.cache.loaded()
    }

    /// Tiles the last frame's view wanted, by priority (the nearest to the eye first)
    pub fn wanted(&self) -> impl Iterator<Item = TileId> + '_ {
        self.wanted.iter().map(|s| s.tile)
    }

    /// Tiles of the flight destination, computed once per destination
    fn update_preload(&mut self, ctx: &DrawContext) {
        match ctx.preload {
            Some((generation, view)) if generation != self.preload.0 => {
                let tiles = tiles::select(view, self.options.min_z, self.options.max_z, self.tile_px());

                self.preload = (
                    generation,
                    tiles.into_iter().take(PRELOAD_TILES).map(|s| s.tile).collect(),
                );
            }
            Some(_) => {}
            None => {
                self.preload.0 = 0;
                self.preload.1.clear();
            }
        }
    }

    /// Target size of a tile on screen: elevation tiles larger (coarser levels)
    fn tile_px(&self) -> f64 {
        if self.options.elevation {
            TILE_PX * ELEVATION_SCALE
        } else {
            TILE_PX
        }
    }

    /// Starts the download of a tile; `urgent`: with a high fetch priority (else low)
    fn request(&mut self, tile: TileId, urgent: bool, redraw: &Rc<dyn Fn()>, now: f64) {
        let Ok(controller) = web_sys::AbortController::new() else {
            return;
        };
        let url = data::tile_url(&self.options.url, tile, &self.options.body);
        let (elevation, geoid, size) = (self.options.elevation, self.options.geoid, self.options.size);
        let no_data = data::no_data(&self.options.body);
        let loader = self.options.loader.clone();
        let signal = controller.signal();
        // A tile that failed before is fetched past the HTTP cache (it may hold the error)
        let retried = self.cache.failed_before(tile);
        let priority = if urgent { "high" } else { "low" };

        self.in_flight.insert(tile, (controller, now));

        let inbox = self.inbox.clone();
        let redraw = redraw.clone();

        wasm_bindgen_futures::spawn_local(async move {
            // Transient errors (network, rate limit, server) are retried; "no tile here" is final
            let mut pixels = Err(Fetch::Transient);

            for (attempt, pause) in RETRY_PAUSES_MS.iter().enumerate() {
                let reload = retried || attempt > 0;
                let fetched = if let (Some(loader), true) = (&loader, elevation) {
                    fetch::draw_heights(loader, tile, geoid, no_data, &signal)
                        .await
                        .map(Pixels::Heights)
                } else if let Some(loader) = &loader {
                    fetch::draw_tile(loader, tile, size, &signal).await.map(Pixels::Image)
                } else if elevation {
                    fetch::fetch_heights(&url, tile, geoid, no_data, &signal, reload, priority)
                        .await
                        .map(Pixels::Heights)
                } else {
                    fetch::fetch_bitmap(&url, size, &signal, reload, priority)
                        .await
                        .map(Pixels::Image)
                };

                match fetched {
                    Ok(p) => {
                        pixels = Ok(p);
                        break;
                    }
                    Err(Fetch::Missing) => {
                        pixels = Err(Fetch::Missing);
                        break;
                    }
                    Err(Fetch::Transient) if attempt + 1 < RETRY_PAUSES_MS.len() && !signal.aborted() => {
                        crate::js::sleep(*pause).await;
                    }
                    Err(Fetch::Transient) => {}
                }

                if signal.aborted() {
                    break;
                }
            }

            // An aborted tile was already forgotten (or its source dropped); it is not a failure
            if !signal.aborted() {
                inbox.borrow_mut().push((tile, pixels));
                redraw();
            } else if let Ok(pixels) = pixels {
                pixels.close();
            }
        });
    }

    /// Aborts the requests of tiles no longer needed: neither wanted nor an ancestor of a wanted tile (ancestors
    /// stand in while zooming in). A request younger than `STALE_GRACE_MS` is let finish: while the camera moves, the
    /// tiles of a frame are often gone by the next one, and aborting them at once fired hundreds of requests a second
    /// that never completed (a burden for the server, and Firefox queued the next ones behind them)
    fn abort_stale(&mut self, now: f64) {
        self.keep.clear();
        for &tile in &self.wanted_set {
            let mut t = Some(tile);

            while let Some(a) = t {
                if !self.keep.insert(a) {
                    break;
                }
                t = a.parent();
            }
        }

        let keep = &self.keep;

        self.in_flight.retain(|tile, (controller, started)| {
            let kept = keep.contains(tile) || now - *started < STALE_GRACE_MS;

            if !kept {
                controller.abort();
            }

            kept
        });
    }

    /// Loads what the view needs and lists the tiles to draw (`plan.tiles`); returns true while tiles are loading
    /// or waiting for their upload
    pub fn prepare(&mut self, gl: &glow::Context, ctx: &DrawContext) -> bool {
        let (view, frame, redraw, now) = (ctx.view, ctx.frame, ctx.redraw, ctx.now);

        self.wanted = tiles::select(view, self.options.min_z, self.options.max_z, self.tile_px());
        self.update_preload(ctx);
        self.wanted_set.clear();
        self.wanted_set.extend(self.wanted.iter().map(|s| s.tile));
        self.wanted_set.extend(self.preload.1.iter().copied());
        // The tiles kept first (`keep`), which the arrivals not needed any more are checked against
        self.abort_stale(now);
        self.fading = false;

        let motion = Motion {
            flying: ctx.loading.skip_levels_in_flight && ctx.moving && ctx.preload.is_some(),
            defer_off_center: ctx.loading.defer_off_center && ctx.moving,
        };

        // The plan before the uploads: the tiles it relies on are held, the uploads take the other slots
        self.cache
            .plan(&mut self.plan, &self.wanted, &self.preload.1, frame, now, motion);
        self.receive(gl, frame, now, ctx.loading.upload_budget_ms);
        // The deferred tiles load once the camera rests
        if self.plan.deferred && !self.wake.get() {
            self.wake.set(true);

            let (wake, redraw) = (self.wake.clone(), redraw.clone());

            wasm_bindgen_futures::spawn_local(async move {
                crate::js::sleep(REST_MS).await;
                wake.set(false);
                redraw();
            });
        }

        // Coverage first, then the tiles by priority, then the flight destination
        let (missing, urgent) = (
            std::mem::take(&mut self.plan.missing),
            std::mem::take(&mut self.plan.urgent),
        );

        self.request_all(&missing, &urgent, redraw, ctx.loading.raster_requests, now);
        (self.plan.missing, self.plan.urgent) = (missing, urgent);

        let (cooling, missing) = self.cache.failure_counts(now);

        self.stats = RasterStats {
            drawn: self.plan.tiles.len(),
            loaded: self.cache.len(),
            in_flight: self.in_flight.len(),
            queued: self.queue.len(),
            stand_ins: self.plan.tiles.iter().filter(|&&t| !self.cache.contains(t)).count(),
            cooling,
            missing,
        };

        !self.in_flight.is_empty() || !self.queue.is_empty()
    }

    /// After the pass: asks for the next frame while uploads wait or tiles fade in
    pub fn finish(&self, redraw: &Rc<dyn Fn()>) {
        if !self.queue.is_empty() || self.fading {
            redraw();
        }
    }

    /// What this source draws on the area of `tile` (see `Cache::sample`)
    fn sample(&mut self, tile: TileId, frame: u64, now: f64, fade_ms: f64) -> Sample {
        let (sample, fading) = self.cache.sample(tile, frame, now, fade_ms);

        self.fading |= fading;

        sample
    }

    /// Where the altitudes of `tile` are in the pool (see `Cache::heights`)
    pub fn heights(&mut self, tile: TileId, frame: u64) -> [f32; 4] {
        self.cache.heights(tile, frame)
    }

    /// Altitude (metres) at a Mercator point, from the finest elevation tile loaded there; None if none is
    pub fn altitude(&self, m: glam::DVec2) -> Option<f64> {
        self.cache.altitude(m)
    }

    /// Starts the requests, in order, within the in-flight limit; `urgent`: which ones get a high fetch priority
    fn request_all(&mut self, tiles: &[TileId], urgent: &[bool], redraw: &Rc<dyn Fn()>, max_requests: usize, now: f64) {
        for (k, &tile) in tiles.iter().enumerate() {
            if self.in_flight.len() >= max_requests {
                break;
            }

            // Neither loading, nor arrived and waiting for its upload, nor uploaded since the plan
            if !self.in_flight.contains_key(&tile) && !self.queued.contains(&tile) && !self.cache.contains(tile) {
                self.request(tile, urgent.get(k).copied().unwrap_or(false), redraw, now);
            }
        }
    }

    /// Frees the pool (the downloads stop when the source is dropped)
    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: the pool is owned by this source, not used afterwards
        unsafe { gl.delete_texture(self.pool) };
    }
}

impl Drop for RasterSource {
    /// Stops the downloads and frees the decoded images waiting outside the wasm heap
    fn drop(&mut self) {
        for (controller, _) in self.in_flight.values() {
            controller.abort();
        }
        for (_, pixels) in self.queue.drain(..) {
            pixels.close();
        }

        for (_, pixels) in self.inbox.borrow_mut().drain(..) {
            if let Ok(pixels) = pixels {
                pixels.close();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_descendants_stand_in_for_wanted_tiles() {
        let wanted: HashSet<TileId> = [TileId::new(5, 10, 10)].into_iter().collect();
        let child = TileId::new(6, 21, 20);

        assert!(stands_in_for(child, &wanted));
        assert!(stands_in_for(TileId::new(8, 84, 80), &wanted));
        // Too deep, elsewhere, the tile itself or its parent: no
        assert!(!stands_in_for(TileId::new(9, 168, 160), &wanted));
        assert!(!stands_in_for(TileId::new(6, 0, 0), &wanted));
        assert!(!stands_in_for(TileId::new(5, 10, 10), &wanted));
        assert!(!stands_in_for(TileId::new(4, 5, 5), &wanted));
        assert!(!stands_in_for(TileId::new(0, 0, 0), &wanted));
    }
}
