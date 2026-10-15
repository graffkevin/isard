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
//! An elevation source (raw altitudes, `image/x-bil;bits=32`, in an R32F pool) gives the relief: the vertex shader
//! lifts the grid of every tile by the heights read in it, three levels coarser than the imagery (a 16 × 16 grid
//! needs far fewer samples than 256 px), and a skirt under the tile edges hides the cracks between levels.

use crate::geo::camera::{View, lon_lat};
use crate::geo::geoid;
use crate::geo::tiles::{self, TileId};
use crate::render::gl::{self, Grid, Projection};
use glow::HasContext;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

const TILE: i32 = 256;
/// Texture slots per layer (256 KB each): 256 slots = 64 MB of GPU memory
const SLOTS: usize = 256;
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
/// Below this, an altitude is the no-data of the server (-99999 on the Géoplateforme)
const NO_DATA: f32 = -1000.0;

const VERTEX: &str = r"
uniform mat4 u_matrix;
layout(location = 0) in vec2 a_grid;
layout(location = 1) in vec4 a_rect;
layout(location = 2) in vec4 a_merc;
// Per layer: uv rect of the tile, uv rect of its stand-in, (slot of the tile, slot of the stand-in, fade)
layout(location = 3) in vec4 a_uv0;
layout(location = 4) in vec4 a_back0;
layout(location = 5) in vec3 a_slot0;
layout(location = 6) in vec4 a_uv1;
layout(location = 7) in vec4 a_back1;
layout(location = 8) in vec3 a_slot1;
layout(location = 9) in vec4 a_uv2;
layout(location = 10) in vec4 a_back2;
layout(location = 11) in vec3 a_slot2;
layout(location = 12) in vec4 a_uv3;
layout(location = 13) in vec4 a_back3;
layout(location = 14) in vec3 a_slot3;
// Heights: offset and scale of this tile in its elevation tile, slot of that tile (< 0: none)
layout(location = 15) in vec4 a_height;
uniform highp sampler2DArray u_heights;
// Vertical exaggeration; 0: flat
uniform float u_relief;
out vec4 v_uv[4];
flat out vec3 v_slot[4];
vec2 g;
vec4 uv(vec4 front, vec4 back) { return vec4(mix(front.xy, front.zw, g), mix(back.xy, back.zw, g)); }

// Altitude (metres) at a point of the tile, bilinear between the samples (R32F cannot be filtered)
float height() {
  if (u_relief <= 0.0 || a_height.w < 0.0) return 0.0;
  vec2 p = clamp((a_height.xy + g * a_height.z) * 256.0 - 0.5, 0.0, 255.0);
  ivec2 i = ivec2(floor(p));
  ivec2 j = min(i + 1, ivec2(255));
  vec2 f = p - vec2(i);
  int s = int(a_height.w);
  float h00 = texelFetch(u_heights, ivec3(i.x, i.y, s), 0).r;
  float h10 = texelFetch(u_heights, ivec3(j.x, i.y, s), 0).r;
  float h01 = texelFetch(u_heights, ivec3(i.x, j.y, s), 0).r;
  float h11 = texelFetch(u_heights, ivec3(j.x, j.y, s), 0).r;
  return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y) * u_relief;
}

void main() {
  // Skirt vertices come shifted by +2: they hang under the edge, by 5 % of the tile width
  g = a_grid;
  float skirt = 0.0;
  if (g.x > 1.5) {
    g -= 2.0;
    skirt = 1.0;
  }
  v_uv[0] = uv(a_uv0, a_back0); v_slot[0] = a_slot0;
  v_uv[1] = uv(a_uv1, a_back1); v_slot[1] = a_slot1;
  v_uv[2] = uv(a_uv2, a_back2); v_slot[2] = a_slot2;
  v_uv[3] = uv(a_uv3, a_back3); v_slot[3] = a_slot3;
  vec2 merc = mix(a_merc.xy, a_merc.zw, g);
  float lat = atan(sinh(3.1415927 * (1.0 - 2.0 * merc.y)));
  float width = (a_merc.z - a_merc.x) * 40075016.686 * cos(lat);
  vec3 p = project_up(mix(a_rect.xy, a_rect.zw, g), merc, height() - skirt * 0.05 * width);
  gl_Position = u_matrix * vec4(p, 1.0);
}";

const FRAGMENT: &str = r"#version 300 es
precision highp float;
uniform highp sampler2DArray u_tiles0;
uniform highp sampler2DArray u_tiles1;
uniform highp sampler2DArray u_tiles2;
uniform highp sampler2DArray u_tiles3;
uniform int u_count;
uniform float u_opacity[4];
uniform float u_saturation[4];
in vec4 v_uv[4];
flat in vec3 v_slot[4];
out vec4 color;

// One layer, straight alpha: the tile mixed with its stand-in while it fades in; slot < 0 = none
vec4 layer(highp sampler2DArray tiles, vec4 uv, vec3 slot, float opacity, float saturation) {
  vec4 c = slot.x >= 0.0 ? texture(tiles, vec3(uv.xy, slot.x)) : vec4(0.0);
  if (slot.z < 1.0) {
    c = slot.y >= 0.0 ? mix(texture(tiles, vec3(uv.zw, slot.y)), c, slot.z) : vec4(c.rgb, c.a * slot.z);
  }
  float grey = dot(c.rgb, vec3(0.299, 0.587, 0.114));
  return vec4(mix(vec3(grey), c.rgb, saturation), c.a * opacity);
}

// Over operator, premultiplied result
void over(inout vec4 acc, vec4 c) {
  acc = vec4(c.rgb * c.a + acc.rgb * (1.0 - c.a), c.a + acc.a * (1.0 - c.a));
}

void main() {
  vec4 acc = vec4(0.0);
  over(acc, layer(u_tiles0, v_uv[0], v_slot[0], u_opacity[0], u_saturation[0]));
  if (u_count > 1) over(acc, layer(u_tiles1, v_uv[1], v_slot[1], u_opacity[1], u_saturation[1]));
  if (u_count > 2) over(acc, layer(u_tiles2, v_uv[2], v_slot[2], u_opacity[2], u_saturation[2]));
  if (u_count > 3) over(acc, layer(u_tiles3, v_uv[3], v_slot[3], u_opacity[3], u_saturation[3]));
  color = acc;
}";

pub struct RasterOptions {
    /// Template: `{z}`, `{x}`, `{y}` (WMTS, XYZ) or `{bbox}` (WMS `GetMap` in EPSG:3857: west, south, east, north in metres)
    pub url: String,
    pub min_z: u8,
    pub max_z: u8,
    pub opacity: f32,
    pub saturation: f32,
    /// Raw altitudes (`image/x-bil;bits=32`) rather than images: the relief
    pub elevation: bool,
}

/// How tiles load and show: the engine's two profiles
#[derive(Clone, Copy, Debug)]
pub struct Loading {
    /// New tiles fade in over their stand-in during this time (ms); 0: they show at once
    pub fade_ms: f64,
    /// During a camera flight, the levels flown through are skipped (Cesium's skip LOD)
    pub skip_levels_in_flight: bool,
    /// While the camera moves, covered tiles off the screen center wait (foveated loading)
    pub defer_off_center: bool,
    /// Time spent uploading arrived tiles to the GPU per frame and layer (ms; at least one tile)
    pub upload_budget_ms: f64,
    /// Requests in flight per raster layer, and per 3D Tiles layer
    pub raster_requests: usize,
    pub mesh_requests: usize,
}

impl Loading {
    /// Everything visible loads and shows as soon as it arrives (the default)
    pub const IMMEDIATE: Self = Self {
        fade_ms: 0.0,
        skip_levels_in_flight: false,
        defer_off_center: false,
        upload_budget_ms: 8.0,
        raster_requests: 24,
        mesh_requests: 20,
    };
    /// Fewer downloads while moving, tiles fading in: smoother on slow networks and GPUs
    pub const SMOOTH: Self = Self {
        fade_ms: 200.0,
        skip_levels_in_flight: true,
        defer_off_center: true,
        upload_budget_ms: 3.0,
        raster_requests: 16,
        mesh_requests: 12,
    };
}

/// What a layer needs to draw a frame
pub struct DrawContext<'a> {
    pub view: &'a View,
    pub frame: u64,
    pub redraw: &'a Rc<dyn Fn()>,
    /// The camera moved during the last frames
    pub moving: bool,
    /// Destination of a camera flight (generation, view), to load in advance
    pub preload: Option<(u64, &'a View)>,
    pub loading: Loading,
}

/// Side of the copy of an elevation tile kept on the CPU, to know the altitude under a point
const CPU_HEIGHTS: usize = 32;

/// A tile held in the pool
struct Loaded {
    slot: u16,
    /// Elevation tiles: altitudes subsampled to `CPU_HEIGHTS`², rows from the north
    heights: Option<Box<[f32]>>,
    /// Last frame it was drawn
    used: u64,
    /// Upload time (ms), for the fade-in
    at: f64,
}

/// The content of an arrived tile
enum Pixels {
    Image(web_sys::ImageBitmap),
    /// 256 × 256 altitudes, rows from the north
    Heights(Vec<f32>),
}

impl Pixels {
    fn close(&self) {
        if let Self::Image(bitmap) = self {
            bitmap.close();
        }
    }
}

type Inbox = Rc<RefCell<Vec<(TileId, Result<Pixels, Fetch>)>>>;

/// What a source gives to one tile of the pass: texture slots and uv rects of the tile and of its stand-in
#[derive(Clone, Copy)]
pub struct Sample {
    front: Option<(u16, [f32; 4])>,
    back: Option<(u16, [f32; 4])>,
    fade: f32,
}

impl Sample {
    const NONE: Self = Self {
        front: None,
        back: None,
        fade: 1.0,
    };

    fn write(&self, data: &mut Vec<f32>) {
        let uv = |s: Option<(u16, [f32; 4])>| s.map_or([0.0; 4], |(_, uv)| uv);
        let slot = |s: Option<(u16, [f32; 4])>| s.map_or(-1.0, |(slot, _)| f32::from(slot));
        data.extend_from_slice(&uv(self.front));
        data.extend_from_slice(&uv(self.back));
        data.extend_from_slice(&[slot(self.front), slot(self.back), self.fade]);
    }
}

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

pub struct RasterSource {
    pub options: RasterOptions,
    pool: glow::Texture,
    slots: HashMap<TileId, Loaded>,
    free: Vec<u16>,
    /// Requests in flight: their abort handle and start time (ms)
    in_flight: HashMap<TileId, (web_sys::AbortController, f64)>,
    /// Tiles that could not load: time (ms) they may be asked again (never for a missing tile), failures so far
    failed: HashMap<TileId, (f64, u32)>,
    inbox: Inbox,
    /// Arrived, waiting for their upload
    queue: Vec<(TileId, Pixels)>,
    /// Tiles to draw this frame, when this source is the base of a pass (stand-ins included)
    tiles: Vec<TileId>,
    fading: bool,
    /// A redraw is scheduled for the deferred tiles
    wake: Rc<std::cell::Cell<bool>>,
    /// Flight destination: generation and its tiles
    preload: (u64, Vec<TileId>),
    pub stats: RasterStats,
}

impl RasterSource {
    pub fn new(gl: &glow::Context, options: RasterOptions) -> Result<Self, String> {
        // SAFETY: object creation and state setup on a live WebGL2 context
        unsafe {
            let pool = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(pool));
            // Altitudes are read with texelFetch (R32F cannot be filtered in WebGL2)
            let (format, filter) = if options.elevation {
                (glow::R32F, glow::NEAREST)
            } else {
                (glow::RGBA8, glow::LINEAR)
            };
            gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY, 1, format, TILE, TILE, SLOTS as i32);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, filter),
                (glow::TEXTURE_MAG_FILTER, filter),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, key, value as i32);
            }
            Ok(Self {
                options,
                pool,
                slots: HashMap::new(),
                free: (0..SLOTS as u16).rev().collect(),
                in_flight: HashMap::new(),
                failed: HashMap::new(),
                inbox: Rc::default(),
                queue: Vec::new(),
                tiles: Vec::new(),
                fading: false,
                wake: Rc::default(),
                preload: (0, Vec::new()),
                stats: RasterStats::default(),
            })
        }
    }

    /// Takes the tiles that arrived, uploads a few of them (nearest to `wanted` first)
    fn receive(&mut self, gl: &glow::Context, frame: u64, wanted: &HashSet<TileId>, budget_ms: f64) {
        for (tile, pixels) in self.inbox.borrow_mut().drain(..) {
            self.in_flight.remove(&tile);
            match pixels {
                Ok(pixels) => {
                    self.failed.remove(&tile);
                    self.queue.push((tile, pixels));
                }
                Err(Fetch::Missing) => {
                    self.failed.insert(tile, (f64::INFINITY, 1));
                }
                Err(Fetch::Transient) => {
                    let strikes = self.failed.get(&tile).map_or(0, |f| f.1) + 1;
                    let delay = (COOLDOWN_MS * f64::from(1u32 << (strikes - 1).min(8))).min(COOLDOWN_MAX_MS);
                    self.failed.insert(tile, (js_sys::Date::now() + delay, strikes));
                }
            }
        }
        // Tiles of the current view first, the others (stand-ins, tiles that left the view) after
        self.queue.sort_by_key(|(tile, _)| !wanted.contains(tile));
        // Uploads within the frame's time budget (at least one), the rest waits for the next frames
        let start = js_sys::Date::now();
        let mut uploaded = 0;
        while !self.queue.is_empty() && (uploaded == 0 || js_sys::Date::now() - start < budget_ms) {
            let (tile, pixels) = self.queue.remove(0);
            uploaded += 1;
            // Already in the pool (a duplicate arrival): installing it again would leak a slot
            if self.slots.contains_key(&tile) {
                pixels.close();
                continue;
            }
            let Some(slot) = self.free.pop().or_else(|| self.evict(frame)) else {
                // Every slot is on screen: the tile waits for one (the next frame is asked by `finish`)
                self.queue.insert(0, (tile, pixels));
                break;
            };
            // SAFETY: upload into an allocated slot of the pool
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.pool));
                match &pixels {
                    Pixels::Image(bitmap) => gl.tex_sub_image_3d_with_image_bitmap(
                        glow::TEXTURE_2D_ARRAY,
                        0,
                        0,
                        0,
                        i32::from(slot),
                        TILE,
                        TILE,
                        1,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        bitmap,
                    ),
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
            let heights = match &pixels {
                Pixels::Heights(heights) => Some(subsample(heights)),
                Pixels::Image(_) => None,
            };
            self.slots.insert(
                tile,
                Loaded {
                    slot,
                    heights,
                    used: frame,
                    at: js_sys::Date::now(),
                },
            );
        }
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
            None => self.preload = (0, Vec::new()),
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

    fn is_failed(&self, tile: TileId, now: f64) -> bool {
        self.failed.get(&tile).is_some_and(|&(until, _)| now < until)
    }

    /// Frees the least recently used slot not drawn in the current frame
    fn evict(&mut self, frame: u64) -> Option<u16> {
        let (&tile, _) = self
            .slots
            .iter()
            .filter(|(_, l)| l.used < frame)
            .min_by_key(|(_, l)| l.used)?;
        self.slots.remove(&tile).map(|l| l.slot)
    }

    fn request(&mut self, tile: TileId, redraw: &Rc<dyn Fn()>) {
        let Ok(controller) = web_sys::AbortController::new() else {
            return;
        };
        let url = self
            .options
            .url
            .replace("{z}", &tile.z.to_string())
            .replace("{x}", &tile.x.to_string())
            .replace("{y}", &tile.y.to_string())
            .replace("{bbox}", &bbox(tile));
        let elevation = self.options.elevation;
        let signal = controller.signal();
        // A tile that failed before is fetched past the HTTP cache (it may hold the error)
        let retried = self.failed.contains_key(&tile);
        self.in_flight.insert(tile, (controller, js_sys::Date::now()));
        let inbox = self.inbox.clone();
        let redraw = redraw.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // Transient errors (network, rate limit, server) are retried; "no tile here" is final
            let mut pixels = Err(Fetch::Transient);
            for (attempt, pause) in RETRY_PAUSES_MS.iter().enumerate() {
                let reload = retried || attempt > 0;
                let fetched = if elevation {
                    fetch_heights(&url, tile, &signal, reload).await.map(Pixels::Heights)
                } else {
                    fetch_bitmap(&url, &signal, reload).await.map(Pixels::Image)
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
                        sleep(*pause).await;
                    }
                    Err(Fetch::Transient) => {}
                }
                if signal.aborted() {
                    break;
                }
            }
            // An aborted tile was already forgotten; it is not a failure
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
    fn abort_stale(&mut self, wanted: &HashSet<TileId>) {
        let mut keep: HashSet<TileId> = HashSet::with_capacity(wanted.len() * 2);
        for &tile in wanted {
            let mut t = Some(tile);
            while let Some(a) = t {
                if !keep.insert(a) {
                    break;
                }
                t = a.parent();
            }
        }
        let now = js_sys::Date::now();
        self.in_flight.retain(|tile, (controller, started)| {
            let kept = keep.contains(tile) || now - *started < STALE_GRACE_MS;
            if !kept {
                controller.abort();
            }
            kept
        });
    }

    /// Loads what the view needs and lists the tiles to draw (`tiles`); returns true while tiles are loading or
    /// waiting for their upload
    pub fn prepare(&mut self, gl: &glow::Context, ctx: &DrawContext) -> bool {
        let (view, frame, redraw) = (ctx.view, ctx.frame, ctx.redraw);
        let wanted = tiles::select(view, self.options.min_z, self.options.max_z, self.tile_px());
        let now = js_sys::Date::now();
        self.update_preload(ctx);
        let mut wanted_set: HashSet<TileId> = wanted.iter().map(|s| s.tile).collect();
        wanted_set.extend(self.preload.1.iter().copied());
        self.receive(gl, frame, &wanted_set, ctx.loading.upload_budget_ms);
        self.abort_stale(&wanted_set);
        self.tiles.clear();
        self.fading = false;
        let mut missing = Vec::new();
        let mut cover = Vec::new();
        let mut deferred = false;
        let flying = ctx.loading.skip_levels_in_flight && ctx.moving && ctx.preload.is_some();
        for selected in &wanted {
            let tile = selected.tile;
            if self.slots.contains_key(&tile) {
                self.tiles.push(tile);
                continue;
            }
            // Stand-in: nearest loaded ancestor (cropped to this tile by `sample`), or its loaded descendants
            // (zooming out)
            let covered = self.loaded_ancestor(tile).is_some();
            if covered {
                self.tiles.push(tile);
            } else {
                let before = self.tiles.len();
                self.push_descendants(tile, DESCENDANT_LEVELS);
                if self.tiles.len() == before {
                    // Nothing yet: drawn anyway, the other layers of the pass may cover it
                    self.tiles.push(tile);
                }
                // Nothing loaded above: a coarse tile first, to cover the area quickly
                let level = tile.z.saturating_sub(COVER_LEVELS).max(self.options.min_z);
                let mut coarse = tile;
                while coarse.z > level {
                    coarse = coarse.parent().unwrap_or(coarse);
                }
                if coarse != tile && !self.is_failed(coarse, now) && !cover.contains(&coarse) {
                    cover.push(coarse);
                }
            }
            // During a flight, the levels flown through are skipped (Cesium's skip LOD): only the coarse coverage
            // and the destination load
            if flying {
                if covered {
                    deferred = true;
                } else if tile.z <= self.options.min_z.saturating_add(COVER_LEVELS) && !self.is_failed(tile, now) {
                    // Too coarse to have a coarser cover: the tile itself
                    missing.push(tile);
                }
                continue;
            }
            // While the camera moves, covered tiles off the screen center wait (foveated loading, as in Cesium):
            // the downloads go where the eye is
            if ctx.loading.defer_off_center && ctx.moving && covered && !selected.central {
                deferred = true;
                continue;
            }
            if self.is_failed(tile, now) {
                // No tile here (or it kept failing): its nearest ancestor that can load stands in
                let mut ancestor = tile.parent();
                while let Some(a) = ancestor {
                    if a.z < self.options.min_z || self.slots.contains_key(&a) {
                        break;
                    }
                    if !self.is_failed(a, now) {
                        missing.push(a);
                        break;
                    }
                    ancestor = a.parent();
                }
            } else {
                missing.push(tile);
            }
        }
        // Destination of a camera flight: loaded after the current view, kept in the pool
        for i in 0..self.preload.1.len() {
            let tile = self.preload.1[i];
            if let Some(loaded) = self.slots.get_mut(&tile) {
                loaded.used = frame;
            } else if !self.is_failed(tile, now) {
                missing.push(tile);
            }
        }
        // The deferred tiles load once the camera rests
        if deferred && !self.wake.get() {
            self.wake.set(true);
            let (wake, redraw) = (self.wake.clone(), redraw.clone());
            wasm_bindgen_futures::spawn_local(async move {
                sleep(REST_MS).await;
                wake.set(false);
                redraw();
            });
        }
        // Coverage first, then the tiles by priority, then the flight destination
        missing.splice(0..0, cover);
        self.request_all(&missing, redraw, ctx.loading.raster_requests);
        self.stats = RasterStats {
            drawn: self.tiles.len(),
            loaded: self.slots.len(),
            in_flight: self.in_flight.len(),
            queued: self.queue.len(),
            stand_ins: self.tiles.iter().filter(|t| !self.slots.contains_key(t)).count(),
            cooling: self
                .failed
                .values()
                .filter(|(until, _)| until.is_finite() && *until > now)
                .count(),
            missing: self.failed.values().filter(|(until, _)| until.is_infinite()).count(),
        };
        !self.in_flight.is_empty() || !self.queue.is_empty()
    }

    /// After the pass: asks for the next frame while uploads wait or tiles fade in
    pub fn finish(&self, redraw: &Rc<dyn Fn()>) {
        if !self.queue.is_empty() || self.fading {
            redraw();
        }
    }

    /// What this source draws on the area of `tile`: its texture (or the deepest level it has, enlarged), with the
    /// stand-in to mix while it fades in; or its nearest loaded ancestor alone
    pub fn sample(&mut self, tile: TileId, frame: u64, now: f64, fade_ms: f64) -> Sample {
        let mut target = tile;
        while target.z > self.options.max_z {
            target = target.parent().unwrap_or(target);
        }
        if target.z < self.options.min_z {
            return Sample::NONE;
        }
        let front = self.slots.get_mut(&target).map(|loaded| {
            loaded.used = frame;
            let fade = if fade_ms > 0.0 {
                ((now - loaded.at) / fade_ms).clamp(0.0, 1.0)
            } else {
                1.0
            };
            (loaded.slot, fade as f32)
        });
        if let Some((slot, fade)) = front
            && fade >= 1.0
        {
            return Sample {
                front: Some((slot, crop(tile, target))),
                back: None,
                fade,
            };
        }
        let back = self.loaded_ancestor(target).map(|ancestor| {
            let loaded = self.slots.get_mut(&ancestor).expect("loaded ancestor");
            loaded.used = frame;
            (loaded.slot, crop(tile, ancestor))
        });
        match front {
            Some((slot, fade)) => {
                self.fading = true;
                Sample {
                    front: Some((slot, crop(tile, target))),
                    back,
                    fade,
                }
            }
            None => Sample {
                front: back,
                back: None,
                fade: 1.0,
            },
        }
    }

    /// Where the altitudes of `tile` are in the pool: offset (2) and scale (1) of the tile in the deepest loaded
    /// elevation tile covering it, and its slot (-1: none yet, flat)
    pub fn heights(&mut self, tile: TileId, frame: u64) -> [f32; 4] {
        let mut target = tile;
        while target.z > self.options.max_z {
            target = target.parent().unwrap_or(target);
        }
        let source = if self.slots.contains_key(&target) {
            Some(target)
        } else {
            self.loaded_ancestor(target)
        };
        let Some(source) = source else {
            return [0.0, 0.0, 1.0, -1.0];
        };
        let loaded = self.slots.get_mut(&source).expect("loaded tile");
        loaded.used = frame;
        let [u0, v0, u1, _] = crop(tile, source);
        [u0, v0, u1 - u0, f32::from(loaded.slot)]
    }

    /// Altitude (metres) at a Mercator point, from the finest elevation tile loaded there; None if none is
    pub fn altitude(&self, m: glam::DVec2) -> Option<f64> {
        let m = m.clamp(glam::DVec2::ZERO, glam::DVec2::splat(1.0 - 1e-9));
        for z in (self.options.min_z..=self.options.max_z).rev() {
            let n = f64::from(1u32 << z);
            let tile = TileId::new(z, (m.x * n) as u32, (m.y * n) as u32);
            let Some(heights) = self.slots.get(&tile).and_then(|l| l.heights.as_deref()) else {
                continue;
            };
            // Bilinear, samples at the cell centers
            let side = CPU_HEIGHTS as f64;
            let p = ((m * n - glam::DVec2::new(f64::from(tile.x), f64::from(tile.y))) * side - 0.5)
                .clamp(glam::DVec2::ZERO, glam::DVec2::splat(side - 1.0));
            let (i, f) = (p.floor(), p - p.floor());
            let at = |x: f64, y: f64| {
                let (x, y) = ((x as usize).min(CPU_HEIGHTS - 1), (y as usize).min(CPU_HEIGHTS - 1));
                f64::from(heights[y * CPU_HEIGHTS + x])
            };
            let top = at(i.x, i.y) * (1.0 - f.x) + at(i.x + 1.0, i.y) * f.x;
            let bottom = at(i.x, i.y + 1.0) * (1.0 - f.x) + at(i.x + 1.0, i.y + 1.0) * f.x;
            return Some(top * (1.0 - f.y) + bottom * f.y);
        }
        None
    }

    fn loaded_ancestor(&self, tile: TileId) -> Option<TileId> {
        let mut ancestor = tile.parent();
        while let Some(a) = ancestor {
            if self.slots.contains_key(&a) {
                return Some(a);
            }
            ancestor = a.parent();
        }
        None
    }

    /// Starts the requests, in order, within the in-flight limit
    fn request_all(&mut self, tiles: &[TileId], redraw: &Rc<dyn Fn()>, max_requests: usize) {
        for &tile in tiles {
            if self.in_flight.len() >= max_requests {
                break;
            }
            // Neither loading nor arrived and waiting for its upload
            if !self.in_flight.contains_key(&tile) && !self.queue.iter().any(|(queued, _)| *queued == tile) {
                self.request(tile, redraw);
            }
        }
    }

    /// Lists the loaded descendants of a tile, down to `levels` levels
    fn push_descendants(&mut self, tile: TileId, levels: u8) {
        if levels == 0 {
            return;
        }
        for child in tile.children() {
            if self.slots.contains_key(&child) {
                self.tiles.push(child);
            } else {
                self.push_descendants(child, levels - 1);
            }
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        for (controller, _) in self.in_flight.values() {
            controller.abort();
        }
        // SAFETY: the pool is owned by this source, not used afterwards
        unsafe { gl.delete_texture(self.pool) };
    }
}

enum Fetch {
    /// No tile at this place (404, 204): final
    Missing,
    /// Network error, rate limit, server error, decoding error: worth a retry
    Transient,
}

/// Fetches an image and decodes it off the main thread, without premultiplied alpha (the shader blends straight
/// alpha)
/// `reload`: bypass the HTTP cache. The Géoplateforme sends its error answers with a 21-day cache lifetime: a retry
/// served from the cache would read the same error for weeks; reloading also replaces the cached error.
async fn fetch_bitmap(url: &str, signal: &web_sys::AbortSignal, reload: bool) -> Result<web_sys::ImageBitmap, Fetch> {
    let response = fetch_response(url, signal, reload)
        .await
        .map_err(|_| Fetch::Transient)?;
    match response.status() {
        204 | 404 | 410 => return Err(Fetch::Missing),
        200..=299 => {}
        _ => return Err(Fetch::Transient),
    }
    decode(&response).await.map_err(|_| Fetch::Transient)
}

/// Altitudes of a tile: a full grid of 256 × 256 little-endian f32. A server error may come as a 200 with an XML
/// exception: only a grid counts. No-data (outside the coverage) takes the lowest altitude of the tile: neither pit
/// nor wall at the coast
async fn fetch_heights(
    url: &str,
    tile: TileId,
    signal: &web_sys::AbortSignal,
    reload: bool,
) -> Result<Vec<f32>, Fetch> {
    let response = fetch_response(url, signal, reload)
        .await
        .map_err(|_| Fetch::Transient)?;
    match response.status() {
        204 | 404 | 410 => return Err(Fetch::Missing),
        200..=299 => {}
        _ => return Err(Fetch::Transient),
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|_| Fetch::Transient)?)
        .await
        .map_err(|_| Fetch::Transient)?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    if bytes.len() != (TILE * TILE * 4) as usize {
        return Err(Fetch::Transient);
    }
    let mut heights: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| f32::from_le_bytes(b))
        .collect();
    let lowest = heights
        .iter()
        .copied()
        .filter(|&h| h > NO_DATA)
        .fold(f32::INFINITY, f32::min);
    let fill = if lowest.is_finite() { lowest } else { 0.0 };
    for h in &mut heights {
        if *h <= NO_DATA || !h.is_finite() {
            *h = fill;
        }
    }
    to_ellipsoid(tile, &mut heights);
    Ok(heights)
}

/// Altitudes of a tile to heights above the ellipsoid, those of 3D Tiles: plus the geoid height at each pixel center
fn to_ellipsoid(tile: TileId, heights: &mut [f32]) {
    let (min, max) = tile.bounds();
    let size = TILE as usize;
    let at = |k: usize, lo: f64, hi: f64| lo + (hi - lo) * (k as f64 + 0.5) / size as f64;
    let lons: Vec<f64> = (0..size).map(|i| lon_lat(glam::DVec2::new(at(i, min.x, max.x), 0.5)).x).collect();
    for (row, line) in heights.chunks_exact_mut(size).enumerate() {
        let lat = lon_lat(glam::DVec2::new(0.5, at(row, min.y, max.y))).y;
        for (h, &lon) in line.iter_mut().zip(&lons) {
            *h += geoid::height(lon, lat) as f32;
        }
    }
}

/// `CPU_HEIGHTS`² altitudes out of a full 256 × 256 grid: the mean of each block
fn subsample(heights: &[f32]) -> Box<[f32]> {
    let block = TILE as usize / CPU_HEIGHTS;
    let mut out = vec![0f32; CPU_HEIGHTS * CPU_HEIGHTS];
    for (k, value) in out.iter_mut().enumerate() {
        let (bx, by) = (k % CPU_HEIGHTS * block, k / CPU_HEIGHTS * block);
        let sum: f32 = (0..block)
            .flat_map(|y| (0..block).map(move |x| heights[(by + y) * TILE as usize + bx + x]))
            .sum();
        *value = sum / (block * block) as f32;
    }
    out.into_boxed_slice()
}

/// Box of a tile in EPSG:3857 metres, `west,south,east,north`, for a WMS `GetMap`
fn bbox(tile: TileId) -> String {
    const HALF: f64 = 20_037_508.342_789_244;
    let (min, max) = tile.bounds();
    let x = |m: f64| -HALF + m * 2.0 * HALF;
    let y = |m: f64| HALF - m * 2.0 * HALF;
    format!("{},{},{},{}", x(min.x), y(max.y), x(max.x), y(min.y))
}

async fn fetch_response(url: &str, signal: &web_sys::AbortSignal, reload: bool) -> Result<web_sys::Response, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_signal(Some(signal));
    // The basemap first: it covers the whole screen
    crate::js::fetch_priority(&init, "high");
    if reload {
        init.set_cache(web_sys::RequestCache::Reload);
    }
    JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await?
        .dyn_into()
}

async fn decode(response: &web_sys::Response) -> Result<web_sys::ImageBitmap, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let blob: web_sys::Blob = JsFuture::from(response.blob()?).await?.dyn_into()?;
    let options = web_sys::ImageBitmapOptions::new();
    options.set_premultiply_alpha(web_sys::PremultiplyAlpha::None);
    options.set_color_space_conversion(web_sys::ColorSpaceConversion::None);
    // Every slot is 256 × 256: any other size (512 px tiles, an error image) is resized, never half-copied
    options.set_resize_width(TILE as u32);
    options.set_resize_height(TILE as u32);
    let bitmap =
        JsFuture::from(window.create_image_bitmap_with_blob_and_image_bitmap_options(&blob, &options)?).await?;
    bitmap.dyn_into()
}

/// Resolves after `ms` milliseconds
async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = JsFuture::from(promise).await;
}

/// Uv rect of `tile` inside `source` (the tile itself or one of its ancestors)
fn crop(tile: TileId, source: TileId) -> [f32; 4] {
    let scale = f64::from(1u32 << (tile.z - source.z));
    let u0 = (f64::from(tile.x) / scale - f64::from(source.x)) as f32;
    let v0 = (f64::from(tile.y) / scale - f64::from(source.y)) as f32;
    let step = (1.0 / scale) as f32;
    [u0, v0, u0 + step, v0 + step]
}

/// Draws a stack of raster sources in one pass: the tiles of the first one (the base), each composited with what
/// the others have on its area
pub struct RasterPass {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 10],
    projection: Projection,
    vao: glow::VertexArray,
    instances: glow::Buffer,
    data: Vec<f32>,
}

impl RasterPass {
    pub fn new(gl: &glow::Context, grid: &Grid) -> Result<Self, String> {
        let program = gl::program(gl, &format!("#version 300 es\n{}{VERTEX}", gl::PROJECT), FRAGMENT)?;
        let uniforms = gl::uniforms(
            gl,
            program,
            [
                "u_matrix",
                "u_count",
                "u_opacity",
                "u_saturation",
                "u_tiles0",
                "u_tiles1",
                "u_tiles2",
                "u_tiles3",
                "u_heights",
                "u_relief",
            ],
        );
        let projection = Projection::new(gl, program);
        // SAFETY: object creation and state setup on a live WebGL2 context
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            grid.bind(gl);
            let instances = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(instances));
            let stride = (INSTANCE * 4) as i32;
            let mut attributes = vec![(1, 4, 0), (2, 4, 16)];
            for k in 0..STACK as u32 {
                let base = 32 + k as i32 * 44;
                attributes.extend([
                    (3 + 3 * k, 4, base),
                    (4 + 3 * k, 4, base + 16),
                    (5 + 3 * k, 3, base + 32),
                ]);
            }
            attributes.push((15, 4, 32 + STACK as i32 * 44));
            for (location, size, offset) in attributes {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
                gl.vertex_attrib_divisor(location, 1);
            }
            gl.bind_vertex_array(None);
            Ok(Self {
                program,
                uniforms,
                projection,
                vao,
                instances,
                data: Vec::new(),
            })
        }
    }

    /// One draw call for the whole stack (1 to `STACK` sources, prepared for this frame), in relief over `terrain`
    /// (prepared too) when given: `relief` is its vertical exaggeration
    pub fn draw(
        &mut self,
        gl: &glow::Context,
        view: &View,
        sources: &mut [&mut RasterSource],
        mut terrain: Option<(&mut RasterSource, f32)>,
        frame: u64,
        fade_ms: f64,
    ) {
        let n = sources.len().min(STACK);
        if n == 0 {
            return;
        }
        let now = js_sys::Date::now();
        self.data.clear();
        let tiles = std::mem::take(&mut sources[0].tiles);
        for &tile in &tiles {
            let (min, max) = tile.bounds();
            let shift = tile.shift(view.center);
            // Plane positions here; the sphere ones are computed on the GPU from the Mercator rect
            let plane = |m: glam::DVec2| (glam::DVec2::new(m.x + shift, m.y) - view.center) * view.world_size;
            let (a, b) = (plane(min), plane(max));
            self.data
                .extend_from_slice(&[a.x as f32, a.y as f32, b.x as f32, b.y as f32]);
            self.data
                .extend_from_slice(&[min.x as f32, min.y as f32, max.x as f32, max.y as f32]);
            for source in sources.iter_mut().take(n) {
                source.sample(tile, frame, now, fade_ms).write(&mut self.data);
            }
            for _ in n..STACK {
                Sample::NONE.write(&mut self.data);
            }
            let heights = terrain
                .as_mut()
                .map_or([0.0, 0.0, 1.0, -1.0], |(source, _)| source.heights(tile, frame));
            self.data.extend_from_slice(&heights);
        }
        sources[0].tiles = tiles;
        let mut opacity = [0f32; STACK];
        let mut saturation = [1f32; STACK];
        for (k, source) in sources.iter().take(n).enumerate() {
            opacity[k] = source.options.opacity;
            saturation[k] = source.options.saturation;
        }
        let count = (self.data.len() / INSTANCE) as i32;
        let relief = terrain.as_ref().map_or(0.0, |(_, exaggeration)| *exaggeration);
        // SAFETY: draw with objects owned by this pass and the sources' pools
        unsafe {
            gl.use_program(Some(self.program));
            let [
                matrix,
                count_u,
                opacity_u,
                saturation_u,
                t0,
                t1,
                t2,
                t3,
                heights_u,
                relief_u,
            ] = &self.uniforms;
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_1_i32(count_u.as_ref(), n as i32);
            gl.uniform_1_f32_slice(opacity_u.as_ref(), &opacity);
            gl.uniform_1_f32_slice(saturation_u.as_ref(), &saturation);
            gl.uniform_1_f32(relief_u.as_ref(), relief);
            self.projection.set(gl, view);
            for (k, unit) in [t0, t1, t2, t3].into_iter().enumerate() {
                gl.uniform_1_i32(unit.as_ref(), k as i32);
                gl.active_texture(glow::TEXTURE0 + k as u32);
                // Unused units get the base pool, so every sampler has a valid texture
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(sources[if k < n { k } else { 0 }].pool));
            }
            // Without relief the elevation unit stays empty: the shader never reads it
            gl.uniform_1_i32(heights_u.as_ref(), STACK as i32);
            gl.active_texture(glow::TEXTURE0 + STACK as u32);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, terrain.as_ref().map(|(source, _)| source.pool));
            gl.active_texture(glow::TEXTURE0);
            // The shader composites the stack and outputs premultiplied alpha
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            let lifted = relief > 0.0;
            if lifted {
                // In relief the ground hides itself (hills) and the 3D layers drawn after; the skirts hang either side
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LEQUAL);
                gl.depth_mask(true);
                gl.disable(glow::CULL_FACE);
            }
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.instances));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, gl::bytes(&self.data), glow::STREAM_DRAW);
            Grid::draw(gl, view, count, lifted);
            if lifted {
                gl.disable(glow::DEPTH_TEST);
                gl.enable(glow::CULL_FACE);
            }
            gl.bind_vertex_array(None);
            gl.blend_func_separate(
                glow::SRC_ALPHA,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }
    }
}
