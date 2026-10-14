//! Raster tile layer (WMTS / XYZ in Web Mercator, 256 px tiles).
//!
//! All the tiles of a layer live in one texture array (a fixed pool of slots, recycled least recently used first),
//! so the whole layer is drawn in a single instanced call, whatever the number of tiles. Images are decoded by the
//! browser (`createImageBitmap`, off the main thread) and uploaded straight into their slot, a few per frame so a
//! burst of arrivals never stalls a frame. While a tile loads, its nearest loaded ancestor (or its loaded
//! descendants, when zooming out) stands in, so panning and zooming never show holes. Requests for tiles that left
//! the view are aborted, so the download slots always serve the current view.

use crate::camera::View;
use crate::gl::{self, Grid, Projection};
use crate::tiles::{self, TileId};
use glow::HasContext;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

const TILE: i32 = 256;
/// Texture slots per layer (256 KB each): 256 slots = 64 MB of GPU memory
const SLOTS: usize = 256;
/// Requests in flight per layer
const MAX_REQUESTS: usize = 16;
/// Texture uploads per frame and layer; the rest waits for the next frames
const UPLOADS_PER_FRAME: usize = 6;
/// Target size of a tile on screen, CSS pixels: tiles are shown between 181 and 362 px
const TILE_PX: f64 = 362.0;
/// Levels searched below a missing tile for stand-ins
const DESCENDANT_LEVELS: u8 = 3;
/// After its last retry, a tile that kept failing is asked again after this delay (ms)
const COOLDOWN_MS: f64 = 15_000.0;
/// Pauses before the retries of a tile after a transient error (milliseconds; the last attempt has none)
const RETRY_PAUSES_MS: [i32; 4] = [500, 1000, 2000, 0];
/// Delay after the camera stops before the deferred (off-center) tiles load (ms)
const REST_MS: i32 = 120;
/// Tiles of a flight destination loaded in advance, per layer
const PRELOAD_TILES: usize = 48;
/// New tiles fade in over their stand-in during this time (ms)
const FADE_MS: f64 = 200.0;
/// When nothing covers a tile yet, a tile this many levels coarser is loaded first: it arrives fast and covers the
/// area while the fine tiles load (shallow tiles first, like 3DTilesRendererJS)
const COVER_LEVELS: u8 = 2;
/// Floats per instance: plane rect (4), Mercator rect (4), uv (4), slot (1), fade (1)
const INSTANCE: usize = 14;

const VERTEX: &str = r"
uniform mat4 u_matrix;
layout(location = 0) in vec2 a_grid;
layout(location = 1) in vec4 a_rect;
layout(location = 2) in vec4 a_merc;
layout(location = 3) in vec4 a_uv;
layout(location = 4) in float a_slot;
layout(location = 5) in float a_fade;
out vec3 v_uv;
out float v_fade;
void main() {
  v_uv = vec3(mix(a_uv.xy, a_uv.zw, a_grid), a_slot);
  v_fade = a_fade;
  vec3 p = project(mix(a_rect.xy, a_rect.zw, a_grid), mix(a_merc.xy, a_merc.zw, a_grid));
  gl_Position = u_matrix * vec4(p, 1.0);
}";

const FRAGMENT: &str = r"#version 300 es
precision highp float;
uniform highp sampler2DArray u_tiles;
uniform float u_opacity;
uniform float u_saturation;
in vec3 v_uv;
in float v_fade;
out vec4 color;
void main() {
  vec4 c = texture(u_tiles, v_uv);
  float grey = dot(c.rgb, vec3(0.299, 0.587, 0.114));
  color = vec4(mix(vec3(grey), c.rgb, u_saturation), c.a * u_opacity * v_fade);
}";

pub struct RasterOptions {
    pub url: String,
    pub min_z: u8,
    pub max_z: u8,
    pub opacity: f32,
    pub saturation: f32,
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
}

/// A tile held in the pool
struct Loaded {
    slot: u16,
    /// Last frame it was drawn
    used: u64,
    /// Upload time (ms), for the fade-in
    at: f64,
}

type Inbox = Rc<RefCell<Vec<(TileId, Result<web_sys::ImageBitmap, Fetch>)>>>;

/// Counters of the last frame, for diagnostics
#[derive(Default, Clone, Copy)]
pub struct RasterStats {
    pub drawn: usize,
    pub loaded: usize,
    pub in_flight: usize,
    pub queued: usize,
}

pub struct RasterLayer {
    pub options: RasterOptions,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 4],
    projection: Projection,
    vao: glow::VertexArray,
    instances: glow::Buffer,
    pool: glow::Texture,
    slots: HashMap<TileId, Loaded>,
    free: Vec<u16>,
    in_flight: HashMap<TileId, web_sys::AbortController>,
    /// Tiles that could not load, with the time (ms) they may be asked again: never for a missing tile
    failed: HashMap<TileId, f64>,
    inbox: Inbox,
    /// Arrived, waiting for their upload
    queue: Vec<(TileId, web_sys::ImageBitmap)>,
    data: Vec<f32>,
    /// Instances drawn before `data`: stand-ins under fading tiles
    under: Vec<f32>,
    fading: bool,
    /// A redraw is scheduled for the deferred tiles
    wake: Rc<std::cell::Cell<bool>>,
    /// Flight destination: generation and its tiles
    preload: (u64, Vec<TileId>),
    pub stats: RasterStats,
}

impl RasterLayer {
    pub fn new(gl: &glow::Context, grid: &Grid, options: RasterOptions) -> Result<Self, String> {
        let program = gl::program(gl, &format!("#version 300 es\n{}{VERTEX}", gl::PROJECT), FRAGMENT)?;
        let uniforms = gl::uniforms(gl, program, ["u_matrix", "u_tiles", "u_opacity", "u_saturation"]);
        let projection = Projection::new(gl, program);
        // SAFETY: object creation and state setup on a live WebGL2 context
        unsafe {
            let pool = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(pool));
            gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY, 1, glow::RGBA8, TILE, TILE, SLOTS as i32);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, key, value as i32);
            }
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            grid.bind(gl);
            let instances = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(instances));
            let stride = (INSTANCE * 4) as i32;
            for (location, size, offset) in [(1, 4, 0), (2, 4, 16), (3, 4, 32), (4, 1, 48), (5, 1, 52)] {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
                gl.vertex_attrib_divisor(location, 1);
            }
            gl.bind_vertex_array(None);
            Ok(Self {
                options,
                program,
                uniforms,
                projection,
                vao,
                instances,
                pool,
                slots: HashMap::new(),
                free: (0..SLOTS as u16).rev().collect(),
                in_flight: HashMap::new(),
                failed: HashMap::new(),
                inbox: Rc::default(),
                queue: Vec::new(),
                data: Vec::new(),
                under: Vec::new(),
                fading: false,
                wake: Rc::default(),
                preload: (0, Vec::new()),
                stats: RasterStats::default(),
            })
        }
    }

    /// Takes the tiles that arrived, uploads a few of them (nearest to `wanted` first)
    fn receive(&mut self, gl: &glow::Context, frame: u64, wanted: &HashSet<TileId>) {
        for (tile, bitmap) in self.inbox.borrow_mut().drain(..) {
            self.in_flight.remove(&tile);
            match bitmap {
                Ok(bitmap) => {
                    self.failed.remove(&tile);
                    self.queue.push((tile, bitmap));
                }
                Err(Fetch::Missing) => {
                    self.failed.insert(tile, f64::INFINITY);
                }
                Err(Fetch::Transient) => {
                    self.failed.insert(tile, js_sys::Date::now() + COOLDOWN_MS);
                }
            }
        }
        // Tiles of the current view first, the others (stand-ins, tiles that left the view) after
        self.queue.sort_by_key(|(tile, _)| !wanted.contains(tile));
        let take = self.queue.len().min(UPLOADS_PER_FRAME);
        for (tile, bitmap) in self.queue.drain(..take).collect::<Vec<_>>() {
            let Some(slot) = self.free.pop().or_else(|| self.evict(frame)) else {
                bitmap.close();
                continue;
            };
            // SAFETY: upload into an allocated slot of the pool
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.pool));
                gl.tex_sub_image_3d_with_image_bitmap(
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
                    &bitmap,
                );
            }
            bitmap.close();
            self.slots.insert(
                tile,
                Loaded {
                    slot,
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
                let tiles = tiles::select(view, self.options.min_z, self.options.max_z, TILE_PX);
                self.preload = (
                    generation,
                    tiles.into_iter().take(PRELOAD_TILES).map(|s| s.tile).collect(),
                );
            }
            Some(_) => {}
            None => self.preload = (0, Vec::new()),
        }
    }

    fn is_failed(&self, tile: TileId, now: f64) -> bool {
        self.failed.get(&tile).is_some_and(|&until| now < until)
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
            .replace("{y}", &tile.y.to_string());
        let signal = controller.signal();
        self.in_flight.insert(tile, controller);
        let inbox = self.inbox.clone();
        let redraw = redraw.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // Transient errors (network, rate limit, server) are retried; "no tile here" is final
            let mut bitmap = Err(Fetch::Transient);
            for (attempt, pause) in RETRY_PAUSES_MS.iter().enumerate() {
                match fetch_bitmap(&url, &signal).await {
                    Ok(b) => {
                        bitmap = Ok(b);
                        break;
                    }
                    Err(Fetch::Missing) => {
                        bitmap = Err(Fetch::Missing);
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
                inbox.borrow_mut().push((tile, bitmap));
                redraw();
            }
        });
    }

    /// Aborts the requests of tiles no longer needed: neither wanted nor an ancestor of a wanted tile (ancestors
    /// stand in while zooming in)
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
        self.in_flight.retain(|tile, controller| {
            let needed = keep.contains(tile);
            if !needed {
                controller.abort();
            }
            needed
        });
    }

    /// Draws the layer; returns true while tiles are loading or waiting for their upload
    pub fn draw(&mut self, gl: &glow::Context, ctx: &DrawContext) -> bool {
        let (view, frame, redraw) = (ctx.view, ctx.frame, ctx.redraw);
        let wanted = tiles::select(view, self.options.min_z, self.options.max_z, TILE_PX);
        let now = js_sys::Date::now();
        self.update_preload(ctx);
        let mut wanted_set: HashSet<TileId> = wanted.iter().map(|s| s.tile).collect();
        wanted_set.extend(self.preload.1.iter().copied());
        self.receive(gl, frame, &wanted_set);
        self.abort_stale(&wanted_set);
        self.data.clear();
        self.under.clear();
        self.fading = false;
        let mut missing = Vec::new();
        let mut cover = Vec::new();
        let mut deferred = false;
        let flying = ctx.moving && ctx.preload.is_some();
        for selected in &wanted {
            let tile = selected.tile;
            if let Some(fade) = self.push_loaded(view, tile, tile, frame, now, false) {
                // Fading in: its stand-in stays visible under it
                if fade < 1.0 {
                    self.push_ancestor(view, tile, frame, now, true);
                }
                continue;
            }
            // Stand-in: nearest loaded ancestor, cropped to this tile, or its loaded descendants (zooming out)
            let covered = self.push_ancestor(view, tile, frame, now, false);
            if !covered {
                self.push_descendants(view, tile, DESCENDANT_LEVELS, frame, now);
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
            if ctx.moving && covered && !selected.central {
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
        // Stand-ins under the fading tiles, then the tiles
        let mut data = std::mem::take(&mut self.under);
        data.extend_from_slice(&self.data);
        std::mem::swap(&mut self.data, &mut data);
        self.under = data;
        // Coverage first, then the tiles by priority, then the flight destination
        missing.splice(0..0, cover);
        self.request_all(&missing, redraw);
        self.submit(gl, view);
        // Uploads left for the next frames, fades running
        if !self.queue.is_empty() || self.fading {
            redraw();
        }
        !self.in_flight.is_empty() || !self.queue.is_empty()
    }

    /// Starts the requests, in order, within the in-flight limit
    fn request_all(&mut self, tiles: &[TileId], redraw: &Rc<dyn Fn()>) {
        for &tile in tiles {
            if self.in_flight.len() >= MAX_REQUESTS {
                break;
            }
            if !self.in_flight.contains_key(&tile) {
                self.request(tile, redraw);
            }
        }
    }

    /// Uploads the instances and draws the whole layer in one call
    fn submit(&mut self, gl: &glow::Context, view: &View) {
        let count = self.data.len() / INSTANCE;
        self.stats = RasterStats {
            drawn: count,
            loaded: self.slots.len(),
            in_flight: self.in_flight.len(),
            queued: self.queue.len(),
        };
        // SAFETY: draw with objects owned by this layer
        unsafe {
            gl.use_program(Some(self.program));
            let [matrix, sampler, opacity, saturation] = &self.uniforms;
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_1_i32(sampler.as_ref(), 0);
            gl.uniform_1_f32(opacity.as_ref(), self.options.opacity);
            gl.uniform_1_f32(saturation.as_ref(), self.options.saturation);
            self.projection.set(gl, view);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.pool));
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.instances));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, gl::bytes(&self.data), glow::STREAM_DRAW);
            Grid::draw(gl, view, count as i32);
            gl.bind_vertex_array(None);
        }
    }

    /// Draws `tile`'s area with the texture of `source` (the tile itself or one of its ancestors), if loaded;
    /// returns its fade-in progress (0 to 1). `under`: in the layer drawn first (stand-ins of fading tiles).
    fn push_loaded(
        &mut self,
        view: &View,
        tile: TileId,
        source: TileId,
        frame: u64,
        now: f64,
        under: bool,
    ) -> Option<f32> {
        let loaded = self.slots.get_mut(&source)?;
        loaded.used = frame;
        let slot = loaded.slot;
        let fade = ((now - loaded.at) / FADE_MS).clamp(0.0, 1.0) as f32;
        self.fading |= fade < 1.0;
        let scale = f64::from(1u32 << (tile.z - source.z));
        let u0 = (f64::from(tile.x) / scale - f64::from(source.x)) as f32;
        let v0 = (f64::from(tile.y) / scale - f64::from(source.y)) as f32;
        let step = (1.0 / scale) as f32;
        let target = if under { &mut self.under } else { &mut self.data };
        push(
            target,
            view,
            tile,
            [u0, v0, u0 + step, v0 + step],
            slot,
            if under { 1.0 } else { fade },
        );
        Some(fade)
    }

    /// Draws `tile`'s area with its nearest loaded ancestor; false if none is loaded
    fn push_ancestor(&mut self, view: &View, tile: TileId, frame: u64, now: f64, under: bool) -> bool {
        let mut ancestor = tile.parent();
        while let Some(a) = ancestor {
            if self.push_loaded(view, tile, a, frame, now, under).is_some() {
                return true;
            }
            ancestor = a.parent();
        }
        false
    }

    fn push_descendants(&mut self, view: &View, tile: TileId, levels: u8, frame: u64, now: f64) {
        if levels == 0 {
            return;
        }
        for child in tile.children() {
            if self.push_loaded(view, child, child, frame, now, false).is_none() {
                self.push_descendants(view, child, levels - 1, frame, now);
            }
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        for controller in self.in_flight.values() {
            controller.abort();
        }
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.instances);
            gl.delete_texture(self.pool);
        }
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
async fn fetch_bitmap(url: &str, signal: &web_sys::AbortSignal) -> Result<web_sys::ImageBitmap, Fetch> {
    let response = fetch_response(url, signal).await.map_err(|_| Fetch::Transient)?;
    match response.status() {
        204 | 404 | 410 => return Err(Fetch::Missing),
        200..=299 => {}
        _ => return Err(Fetch::Transient),
    }
    decode(&response).await.map_err(|_| Fetch::Transient)
}

async fn fetch_response(url: &str, signal: &web_sys::AbortSignal) -> Result<web_sys::Response, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_signal(Some(signal));
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

/// One instance: `tile`'s area, drawn with `uv` of the pool `slot`
fn push(data: &mut Vec<f32>, view: &View, tile: TileId, uv: [f32; 4], slot: u16, fade: f32) {
    let (min, max) = tile.bounds();
    let shift = tile.shift(view.center);
    // Plane positions here; the sphere ones are computed on the GPU from the Mercator rect
    let plane = |m: glam::DVec2| (glam::DVec2::new(m.x + shift, m.y) - view.center) * view.world_size;
    let (a, b) = (plane(min), plane(max));
    data.extend_from_slice(&[a.x as f32, a.y as f32, b.x as f32, b.y as f32]);
    data.extend_from_slice(&[min.x as f32, min.y as f32, max.x as f32, max.y as f32]);
    data.extend_from_slice(&uv);
    data.push(f32::from(slot));
    data.push(fade);
}
