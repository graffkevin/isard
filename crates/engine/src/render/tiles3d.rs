//! 3D Tiles layer: loads a tileset (b3dm / glb meshes, external tilesets) and draws its textured meshes.
//!
//! The tree and its traversal live in `tiles3d::tileset`, the decoding in `tiles3d::decode`; this module fetches, uploads and draws.
//! Tiles are fetched and decoded asynchronously (images by the browser, off the main thread), uploaded a few per
//! frame, kept within a GPU memory budget (least recently drawn unloaded first), and their requests aborted when they
//! leave the view. Each mesh is drawn with a matrix computed in f64 (`View::ecef_matrix`), its vertices in f32 around
//! the tile's center: exact at any zoom, on the plane and on the globe.

use crate::engine::Engine;
use crate::js;
use crate::layer::{Frame, Layer, Loading as Profile};
use crate::render::gl::{self, bytes};
use geo::camera::View;
use glow::HasContext;
use std::cell::RefCell;
use std::rc::Rc;
use tiles3d::decode::{self, Mesh};
use tiles3d::tileset::{self, Content, Readiness, Tree};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// After an error, a tile is asked again after this delay (ms)
const COOLDOWN_MS: f64 = 5_000.0;
/// Downloads of siblings out of view at a time
const PREFETCH_REQUESTS: usize = 4;

const VERTEX: &str = r"#version 300 es
uniform mat4 u_matrix;
layout(location = 0) in vec3 a_position;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;
void main() {
  v_uv = a_uv;
  gl_Position = u_matrix * vec4(a_position, 1.0);
}";

const FRAGMENT: &str = r"#version 300 es
precision highp float;
uniform sampler2D u_texture;
uniform float u_textured;
uniform float u_opacity;
in vec2 v_uv;
out vec4 color;
void main() {
  vec4 c = u_textured > 0.5 ? texture(u_texture, v_uv) : vec4(0.8, 0.8, 0.8, 1.0);
  color = vec4(c.rgb, c.a * u_opacity);
}";

struct GpuPrimitive {
    vao: glow::VertexArray,
    buffers: [glow::Buffer; 2],
    count: i32,
    texture: Option<glow::Texture>,
}

struct GpuMesh {
    anchor: glam::DVec3,
    primitives: Vec<GpuPrimitive>,
    bytes: usize,
    used: u64,
}

enum State {
    Unloaded,
    Loading(Loading),
    Ready(GpuMesh),
    /// No drawable content, for good (none, unsupported, grafted external tileset)
    Empty,
    /// Errored: asked again after this time (ms)
    Failed(f64),
}

/// A request in flight: its number (arrivals of an older request are ignored) and how to cancel it
struct Loading {
    id: u32,
    /// Fetched on this thread; None when a worker of the loader has it
    controller: Option<web_sys::AbortController>,
}

enum Arrival {
    /// Decoded on this thread (no loader)
    Mesh(Mesh, Vec<Option<web_sys::ImageBitmap>>),
    /// Decoded by a worker: `{ anchor, primitives: [{ vertices, indices, bitmap }] }`
    Decoded(JsValue),
    Tileset(Vec<u8>, String),
    Failed,
}

/// (tile, request number, arrival)
type Inbox = Rc<RefCell<Vec<(usize, u32, Arrival)>>>;

thread_local! {
    /// Request numbers, unique across layers (they share the worker pool)
    static NEXT_REQUEST: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
}

fn next_request() -> u32 {
    NEXT_REQUEST.with(|n| {
        let id = n.get();
        n.set(id.wrapping_add(1).max(1));
        id
    })
}

#[derive(Default, Clone, Copy)]
pub struct Tiles3dStats {
    pub drawn: usize,
    pub loaded: usize,
    /// Downloads of tiles in view
    pub in_flight: usize,
    /// Downloads of siblings out of view, in the background
    pub prefetching: usize,
    pub gpu_bytes: usize,
    pub tiles: usize,
}

pub struct Tiles3dLayer {
    url: String,
    tree: Tree,
    root: Option<usize>,
    states: Vec<State>,
    inbox: Inbox,
    /// Arrived, waiting for their upload
    queue: Vec<(usize, Arrival)>,
    /// Pool of decoding workers (`load(id, url, transform, yUp) → Promise`, `abort(id)`); None: decoding here
    loader: Option<JsValue>,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 4],
    pub max_error: f64,
    pub opacity: f32,
    /// GPU memory budget for the meshes (bytes)
    pub budget: usize,
    gpu_bytes: usize,
    pub stats: Tiles3dStats,
}

impl Tiles3dLayer {
    pub fn new(
        gl: &glow::Context,
        url: String,
        max_error: f64,
        loader: Option<JsValue>,
        redraw: &Rc<dyn Fn()>,
    ) -> Result<Self, String> {
        let program = gl::program(gl, VERTEX, FRAGMENT)?;
        let uniforms = gl::uniforms(gl, program, ["u_matrix", "u_texture", "u_textured", "u_opacity"]);
        let layer = Self {
            url: url.clone(),
            tree: Tree::default(),
            root: None,
            states: Vec::new(),
            inbox: Rc::default(),
            queue: Vec::new(),
            loader,
            program,
            uniforms,
            max_error,
            opacity: 1.0,
            budget: 512 << 20,
            gpu_bytes: 0,
            stats: Tiles3dStats::default(),
        };
        // The root tileset, delivered as the arrival of a virtual tile `usize::MAX`
        let (inbox, redraw) = (layer.inbox.clone(), redraw.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let arrival = match fetch_bytes(&url, None, false).await {
                Ok(bytes) => Arrival::Tileset(bytes, url),
                Err(_) => Arrival::Failed,
            };
            inbox.borrow_mut().push((usize::MAX, 0, arrival));
            redraw();
        });
        Ok(layer)
    }

    fn receive(&mut self, gl: &glow::Context, raw: &web_sys::WebGl2RenderingContext, frame: u64, budget_ms: f64) {
        let arrivals: Vec<_> = self.inbox.borrow_mut().drain(..).collect();
        for (tile, id, arrival) in arrivals {
            // Only the answer to the current request of a tile counts (not one that was aborted, then asked again)
            let current = tile < self.states.len() && matches!(&self.states[tile], State::Loading(l) if l.id == id);
            match arrival {
                Arrival::Tileset(json, url) if tile == usize::MAX || current => self.graft(&json, &url, tile),
                Arrival::Mesh(_, _) | Arrival::Decoded(_) if current => self.queue.push((tile, arrival)),
                Arrival::Failed if current => {
                    self.states[tile] = State::Failed(js_sys::Date::now() + COOLDOWN_MS);
                }
                stale => discard(stale),
            }
        }
        // Uploads within the frame's time budget (at least one), the rest waits for the next frames
        let start = js_sys::Date::now();
        let mut uploaded = 0;
        while !self.queue.is_empty() && (uploaded == 0 || js_sys::Date::now() - start < budget_ms) {
            let (tile, arrival) = self.queue.remove(0);
            uploaded += 1;
            if !matches!(self.states[tile], State::Loading(_)) {
                discard(arrival);
                continue;
            }
            let uploaded = match arrival {
                Arrival::Mesh(mesh, images) => upload(gl, &mesh, images, frame),
                Arrival::Decoded(tile) => upload_decoded(gl, raw, &tile, frame).map_err(|e| format!("{e:?}")),
                other => {
                    discard(other);
                    continue;
                }
            };
            match uploaded {
                Ok(gpu) => {
                    self.gpu_bytes += gpu.bytes;
                    self.states[tile] = State::Ready(gpu);
                }
                Err(e) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!("3D Tiles upload: {e}")));
                    self.states[tile] = State::Failed(js_sys::Date::now() + COOLDOWN_MS);
                }
            }
        }
    }

    /// Adds a tileset: the root one (`tile == usize::MAX`) or an external one under `tile`
    fn graft(&mut self, json: &[u8], url: &str, tile: usize) {
        let parent = (tile != usize::MAX).then_some(tile);
        match self.tree.add(json, url, parent) {
            Ok(root) => {
                if parent.is_none() {
                    self.root = Some(root);
                }
                if let Some(p) = parent {
                    self.states[p] = State::Empty;
                }
                self.states.resize_with(self.tree.nodes.len(), || State::Unloaded);
                for (i, node) in self.tree.nodes.iter().enumerate() {
                    if matches!(self.states[i], State::Unloaded)
                        && matches!(node.content, Content::None | Content::Unsupported)
                    {
                        self.states[i] = State::Empty;
                    }
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("3D Tiles {}: {e}", self.url)));
                if let Some(p) = parent {
                    self.states[p] = State::Empty;
                }
            }
        }
    }

    fn abort(&self, loading: &Loading) {
        match &loading.controller {
            Some(controller) => controller.abort(),
            None => {
                if let Some(loader) = &self.loader
                    && let Ok(abort) = js_sys::Reflect::get(loader, &JsValue::from_str("abort"))
                    && let Ok(abort) = abort.dyn_into::<js_sys::Function>()
                {
                    let _ = abort.call1(loader, &JsValue::from(loading.id));
                }
            }
        }
    }

    fn request(&mut self, tile: usize, redraw: &Rc<dyn Fn()>) {
        let node = &self.tree.nodes[tile];
        let (url, is_tileset) = match &node.content {
            Content::Mesh(url) => (url.clone(), false),
            Content::Tileset(url) => (url.clone(), true),
            _ => return,
        };
        let (transform, y_up) = (node.transform, node.y_up);
        // A tile that failed before is fetched past the HTTP cache (it may hold the error)
        let reload = matches!(self.states[tile], State::Failed(_));
        let id = next_request();
        let (inbox, redraw) = (self.inbox.clone(), redraw.clone());
        // Meshes go to the worker pool when there is one: fetched and decoded off the main thread
        if !is_tileset
            && let Some(promise) = self
                .loader
                .as_ref()
                .and_then(|l| load_in_worker(l, id, &url, &transform, y_up, reload))
        {
            self.states[tile] = State::Loading(Loading { id, controller: None });
            wasm_bindgen_futures::spawn_local(async move {
                let arrival = match JsFuture::from(promise).await {
                    Ok(decoded) => Arrival::Decoded(decoded),
                    // Aborted downloads land here too: no warning (the tile is no longer waited for)
                    Err(_) => Arrival::Failed,
                };
                inbox.borrow_mut().push((tile, id, arrival));
                redraw();
            });
            return;
        }
        let Ok(controller) = web_sys::AbortController::new() else {
            return;
        };
        let signal = controller.signal();
        self.states[tile] = State::Loading(Loading {
            id,
            controller: Some(controller),
        });
        wasm_bindgen_futures::spawn_local(async move {
            let arrival = match fetch_bytes(&url, Some(&signal), reload).await {
                Ok(bytes) if is_tileset => Arrival::Tileset(bytes, url),
                Ok(bytes) => match decode::decode(&bytes, &transform, y_up, &[]) {
                    Ok(mesh) => {
                        let images = decode_images(&mesh).await;
                        Arrival::Mesh(mesh, images)
                    }
                    Err(e) => {
                        web_sys::console::warn_1(&JsValue::from_str(&format!("{url}: {e}")));
                        Arrival::Failed
                    }
                },
                Err(_) => Arrival::Failed,
            };
            if signal.aborted() {
                discard(arrival);
                return;
            }
            inbox.borrow_mut().push((tile, id, arrival));
            redraw();
        });
    }

    fn readiness(&self, tile: usize, now: f64) -> Readiness {
        match &self.states[tile] {
            State::Ready(_) => Readiness::Ready,
            State::Empty => Readiness::Empty,
            State::Failed(until) if now < *until => Readiness::Unavailable,
            _ => Readiness::Pending,
        }
    }

    /// Selects, loads and draws; returns true while tiles are loading
    fn draw_tiles(
        &mut self,
        gl: &glow::Context,
        raw: &web_sys::WebGl2RenderingContext,
        view: &View,
        frame: u64,
        redraw: &Rc<dyn Fn()>,
        loading: Profile,
    ) -> bool {
        self.receive(gl, raw, frame, loading.upload_budget_ms);
        let Some(root) = self.root else { return true };
        let now = js_sys::Date::now();
        let selection = tileset::select(&self.tree, root, view, self.max_error, &|i| self.readiness(i, now));
        // Downloads of tiles that left the view are aborted
        let mut visible = vec![false; self.states.len()];
        for &i in &selection.visible {
            visible[i] = true;
        }
        let stale: Vec<usize> = (0..self.states.len())
            .filter(|&i| !visible[i] && matches!(self.states[i], State::Loading(_)))
            .collect();
        for i in stale {
            if let State::Loading(loading) = std::mem::replace(&mut self.states[i], State::Unloaded) {
                self.abort(&loading);
            }
        }
        let mut in_flight = self.states.iter().filter(|s| matches!(s, State::Loading(_))).count();
        for &i in &selection.request {
            if in_flight >= loading.mesh_requests {
                break;
            }
            if matches!(self.states[i], State::Unloaded | State::Failed(_)) {
                self.request(i, redraw);
                in_flight += 1;
            }
        }
        // Siblings out of view: only with the visible tiles all requested, a few at a time (bandwidth stays for what
        // is on screen)
        if selection
            .request
            .iter()
            .all(|&i| !matches!(self.states[i], State::Unloaded | State::Failed(_)))
        {
            let prefetching = selection
                .prefetch
                .iter()
                .filter(|&&i| matches!(self.states[i], State::Loading(_)))
                .count();
            let free = PREFETCH_REQUESTS
                .saturating_sub(prefetching)
                .min(loading.mesh_requests.saturating_sub(in_flight));
            let next: Vec<usize> = selection
                .prefetch
                .iter()
                .copied()
                .filter(|&i| matches!(self.states[i], State::Unloaded))
                .take(free)
                .collect();
            for i in next {
                self.request(i, redraw);
                in_flight += 1;
            }
        }
        for &i in selection.draw.iter().chain(&selection.backfill) {
            if let State::Ready(mesh) = &mut self.states[i] {
                mesh.used = frame;
            }
        }
        self.evict(gl, frame);
        self.render(gl, view, &selection.draw, &selection.backfill);
        let prefetching = selection
            .prefetch
            .iter()
            .filter(|&&i| matches!(self.states[i], State::Loading(_)))
            .count();
        self.stats = Tiles3dStats {
            drawn: selection.draw.len() + selection.backfill.len(),
            loaded: self.states.iter().filter(|s| matches!(s, State::Ready(_))).count(),
            in_flight: in_flight - prefetching,
            prefetching,
            gpu_bytes: self.gpu_bytes,
            tiles: self.tree.nodes.len(),
        };
        if !self.queue.is_empty() {
            redraw();
        }
        in_flight > prefetching || !self.queue.is_empty()
    }

    /// Frees the least recently drawn meshes beyond the budget
    fn evict(&mut self, gl: &glow::Context, frame: u64) {
        if self.gpu_bytes <= self.budget {
            return;
        }
        let mut candidates: Vec<(u64, usize)> = self
            .states
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s {
                State::Ready(mesh) if mesh.used < frame => Some((mesh.used, i)),
                _ => None,
            })
            .collect();
        candidates.sort_unstable();
        for (_, i) in candidates {
            if self.gpu_bytes <= self.budget * 9 / 10 {
                break;
            }
            if let State::Ready(mesh) = std::mem::replace(&mut self.states[i], State::Unloaded) {
                self.gpu_bytes -= mesh.bytes;
                delete_mesh(gl, &mesh);
            }
        }
    }

    /// Draws the selected tiles, then the stand-in parents where no finer tile was drawn. The stencil holds the finest
    /// level drawn at each pixel: 255 for the selected tiles, depth + 1 for a parent, which passes only over coarser
    /// values (its own fragments included, so its surface still sorts by depth)
    fn render(&self, gl: &glow::Context, view: &View, tiles: &[usize], backfill: &[usize]) {
        let [matrix, texture, textured, opacity] = &self.uniforms;
        // SAFETY: draws with objects owned by this layer
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_1_i32(texture.as_ref(), 0);
            gl.uniform_1_f32(opacity.as_ref(), self.opacity);
            gl.active_texture(glow::TEXTURE0);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.disable(glow::CULL_FACE);
            // Layers drawn before may have left counts in the stencil
            gl.clear_stencil(0);
            gl.clear(glow::STENCIL_BUFFER_BIT);
            gl.enable(glow::STENCIL_TEST);
            gl.stencil_op(glow::KEEP, glow::KEEP, glow::REPLACE);
            gl.stencil_func(glow::ALWAYS, 255, 0xff);
            let levels = backfill
                .iter()
                .map(|&i| (i, Some((self.tree.nodes[i].depth + 1).min(254) as i32)));
            for (i, level) in tiles.iter().map(|&i| (i, None)).chain(levels) {
                let State::Ready(mesh) = &self.states[i] else { continue };
                if let Some(level) = level {
                    gl.stencil_func(glow::GEQUAL, level, 0xff);
                }
                let m = view.view_proj * view.ecef_matrix(mesh.anchor);
                gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&m));
                for p in &mesh.primitives {
                    gl.uniform_1_f32(textured.as_ref(), if p.texture.is_some() { 1.0 } else { 0.0 });
                    gl.bind_texture(glow::TEXTURE_2D, p.texture);
                    gl.bind_vertex_array(Some(p.vao));
                    gl.draw_elements(glow::TRIANGLES, p.count, glow::UNSIGNED_INT, 0);
                }
            }
            gl.bind_vertex_array(None);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.enable(glow::CULL_FACE);
        }
    }
}

impl Layer for Tiles3dLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        self.draw_tiles(
            frame.gl,
            frame.raw,
            frame.view,
            frame.number,
            frame.redraw,
            frame.loading,
        )
    }

    fn delete(&mut self, gl: &glow::Context) {
        for i in 0..self.states.len() {
            match std::mem::replace(&mut self.states[i], State::Empty) {
                State::Loading(loading) => self.abort(&loading),
                State::Ready(mesh) => delete_mesh(gl, &mesh),
                _ => {}
            }
        }
        for (_, arrival) in self.queue.drain(..) {
            discard(arrival);
        }
        // SAFETY: the program is owned by this layer, not used afterwards
        unsafe { gl.delete_program(self.program) };
    }
}

fn upload(
    gl: &glow::Context,
    mesh: &Mesh,
    images: Vec<Option<web_sys::ImageBitmap>>,
    frame: u64,
) -> Result<GpuMesh, String> {
    let mut primitives = Vec::new();
    let mut total = 0;
    for (p, image) in mesh.primitives.iter().zip(images) {
        let vertices: Vec<f32> = p
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .zip(p.uvs.as_chunks::<2>().0)
            .flat_map(|(&[x, y, z], &[u, v])| [x, y, z, u, v])
            .collect();
        // SAFETY: object creation and uploads on the live context
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&vertices), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 20, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 20, 12);
            let ibo = gl.create_buffer()?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes(&p.indices), glow::STATIC_DRAW);
            gl.bind_vertex_array(None);
            total += vertices.len() * 4 + p.indices.len() * 4;
            let texture = match image {
                Some(bitmap) => {
                    let (texture, bytes) = texture(gl, &bitmap)?;
                    total += bytes;
                    Some(texture)
                }
                None => None,
            };
            primitives.push(GpuPrimitive {
                vao,
                buffers: [vbo, ibo],
                count: p.indices.len() as i32,
                texture,
            });
        }
    }
    Ok(GpuMesh {
        anchor: mesh.anchor,
        primitives,
        bytes: total,
        used: frame,
    })
}

fn delete_mesh(gl: &glow::Context, mesh: &GpuMesh) {
    // SAFETY: objects owned by the mesh, not used afterwards
    unsafe {
        for p in &mesh.primitives {
            gl.delete_vertex_array(p.vao);
            for b in p.buffers {
                gl.delete_buffer(b);
            }
            if let Some(t) = p.texture {
                gl.delete_texture(t);
            }
        }
    }
}

/// Frees what an arrival holds outside the wasm heap (decoded images)
fn discard(arrival: Arrival) {
    match arrival {
        Arrival::Mesh(_, images) => close(images),
        Arrival::Decoded(tile) => {
            for p in get_array(&tile, "primitives").iter() {
                if let Ok(bitmap) = js_get(&p, "bitmap").dyn_into::<web_sys::ImageBitmap>() {
                    bitmap.close();
                }
            }
        }
        Arrival::Tileset(..) | Arrival::Failed => {}
    }
}

fn js_get(target: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(target, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

fn get_array(target: &JsValue, key: &str) -> js_sys::Array {
    js_get(target, key).dyn_into().unwrap_or_default()
}

/// Asks the worker pool for a tile; None if the loader cannot take it
fn load_in_worker(
    loader: &JsValue,
    id: u32,
    url: &str,
    transform: &glam::DMat4,
    y_up: bool,
    reload: bool,
) -> Option<js_sys::Promise> {
    let load: js_sys::Function = js_get(loader, "load").dyn_into().ok()?;
    let args = js_sys::Array::of5(
        &JsValue::from(id),
        &JsValue::from_str(url),
        &js_sys::Float64Array::from(&transform.to_cols_array()[..]),
        &JsValue::from_bool(y_up),
        &JsValue::from_bool(reload),
    );
    load.apply(loader, &args).ok()?.dyn_into().ok()
}

/// Uploads a tile decoded by a worker, straight from its typed arrays (no copy into the wasm heap)
fn upload_decoded(
    gl: &glow::Context,
    raw: &web_sys::WebGl2RenderingContext,
    tile: &JsValue,
    frame: u64,
) -> Result<GpuMesh, JsValue> {
    let anchor: js_sys::Float64Array = js_get(tile, "anchor").dyn_into()?;
    let anchor = anchor.to_vec();
    let anchor = glam::DVec3::new(anchor[0], anchor[1], anchor[2]);
    let mut primitives = Vec::new();
    let mut total = 0;
    for p in get_array(tile, "primitives").iter() {
        let vertices: js_sys::Float32Array = js_get(&p, "vertices").dyn_into()?;
        let indices: js_sys::Uint32Array = js_get(&p, "indices").dyn_into()?;
        // SAFETY: object creation and uploads on the live context; glow binds, the raw context uploads into the
        // bound buffers (the same WebGL state)
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            raw.buffer_data_with_array_buffer_view(glow::ARRAY_BUFFER, &vertices, glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 20, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 20, 12);
            let ibo = gl.create_buffer()?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
            raw.buffer_data_with_array_buffer_view(glow::ELEMENT_ARRAY_BUFFER, &indices, glow::STATIC_DRAW);
            gl.bind_vertex_array(None);
            total += (vertices.byte_length() + indices.byte_length()) as usize;
            let texture = if let Ok(bitmap) = js_get(&p, "bitmap").dyn_into::<web_sys::ImageBitmap>() {
                let (texture, bytes) = texture(gl, &bitmap)?;
                total += bytes;
                Some(texture)
            } else if js_get(&p, "texture").is_object() {
                let (texture, bytes) = compressed_texture(gl, raw, &js_get(&p, "texture"))?;
                total += bytes;
                Some(texture)
            } else {
                None
            };
            primitives.push(GpuPrimitive {
                vao,
                buffers: [vbo, ibo],
                count: indices.length() as i32,
                texture,
            });
        }
    }
    Ok(GpuMesh {
        anchor,
        primitives,
        bytes: total,
        used: frame,
    })
}

/// Texture transcoded by a worker (KTX2): its mip levels uploaded as they are, compressed or RGBA8
fn compressed_texture(
    gl: &glow::Context,
    raw: &web_sys::WebGl2RenderingContext,
    texture: &JsValue,
) -> Result<(glow::Texture, usize), JsValue> {
    let format = js_get(texture, "format")
        .as_f64()
        .ok_or("compressed texture without format")? as u32;
    let levels = get_array(texture, "levels");
    let mut bytes = 0;
    // SAFETY: texture creation on the live context; glow binds, the raw context uploads into the bound texture
    unsafe {
        let handle = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(handle));
        for (level, info) in levels.iter().enumerate() {
            let width = js_get(&info, "width").as_f64().unwrap_or(0.0) as i32;
            let height = js_get(&info, "height").as_f64().unwrap_or(0.0) as i32;
            let data: js_sys::Uint8Array = js_get(&info, "data").dyn_into()?;
            bytes += data.byte_length() as usize;
            if format == glow::RGBA8 {
                raw.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_array_buffer_view(
                    glow::TEXTURE_2D,
                    level as i32,
                    glow::RGBA8 as i32,
                    width,
                    height,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    Some(&data),
                )?;
            } else {
                raw.compressed_tex_image_2d_with_array_buffer_view(
                    glow::TEXTURE_2D,
                    level as i32,
                    format,
                    width,
                    height,
                    0,
                    &data,
                );
            }
        }
        let count = levels.length() as i32;
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, (count - 1).max(0));
        let min = if count > 1 {
            glow::LINEAR_MIPMAP_LINEAR
        } else {
            glow::LINEAR
        };
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        Ok((handle, bytes))
    }
}

/// Mipmapped texture from a decoded image (closed after the upload); returns it with its GPU size
fn texture(gl: &glow::Context, bitmap: &web_sys::ImageBitmap) -> Result<(glow::Texture, usize), String> {
    // SAFETY: texture creation and upload on the live context
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d_with_image_bitmap(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            bitmap,
        );
        // Mipmaps: no shimmering when the mesh is seen from afar
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR_MIPMAP_LINEAR as i32,
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        let bytes = (bitmap.width() * bitmap.height()) as usize * 4 * 4 / 3;
        bitmap.close();
        Ok((texture, bytes))
    }
}

fn close(images: Vec<Option<web_sys::ImageBitmap>>) {
    for image in images.into_iter().flatten() {
        image.close();
    }
}

/// Decodes the images of a mesh off the main thread (`createImageBitmap`)
async fn decode_images(mesh: &Mesh) -> Vec<Option<web_sys::ImageBitmap>> {
    let mut out = Vec::new();
    for p in &mesh.primitives {
        out.push(match &p.image {
            Some((bytes, mime)) => decode_image(bytes, mime).await.ok(),
            None => None,
        });
    }
    out
}

async fn decode_image(bytes: &[u8], mime: &str) -> Result<web_sys::ImageBitmap, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)?;
    let bitmap_options = web_sys::ImageBitmapOptions::new();
    bitmap_options.set_premultiply_alpha(web_sys::PremultiplyAlpha::None);
    let bitmap =
        JsFuture::from(window.create_image_bitmap_with_blob_and_image_bitmap_options(&blob, &bitmap_options)?).await?;
    bitmap.dyn_into()
}

async fn fetch_bytes(url: &str, signal: Option<&web_sys::AbortSignal>, reload: bool) -> Result<Vec<u8>, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_signal(signal);
    // A tileset (or tile) server can be slow (~15 s per cold file in Lille): low priority, so that it never holds the
    // basemap back
    crate::js::fetch_priority(&init, "low");
    if reload {
        init.set_cache(web_sys::RequestCache::Reload);
    }
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await?
        .dyn_into()?;
    if !response.ok() {
        return Err(JsValue::from(response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer()?).await?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

impl Drop for Tiles3dLayer {
    fn drop(&mut self) {
        for state in &self.states {
            if let State::Loading(loading) = state {
                self.abort(loading);
            }
        }
    }
}

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r"
/** Counters of a 3D Tiles layer */
export interface Tiles3dStats {
  /** Tiles drawn in the last frame */
  drawn: number;
  /** Meshes held on the GPU */
  loaded: number;
  /** Downloads of tiles in view */
  inFlight: number;
  /** Downloads of tiles just out of view (siblings of refined tiles), in the background */
  prefetching: number;
  /** GPU memory of the meshes, MB */
  gpuMb: number;
  /** Tiles known in the tree (external tilesets included once loaded) */
  tiles: number;
}
";

#[wasm_bindgen]
impl Engine {
    /// Pool of workers that fetch and decode 3D Tiles content off the main thread: an object with
    /// `load(id, url, transform, yUp) → Promise<{ anchor, primitives }>` and `abort(id)`. Without it, tiles are
    /// decoded on the main thread. Set it before adding 3D Tiles layers.
    #[wasm_bindgen(js_name = setTileLoader)]
    pub fn set_tile_loader(&mut self, #[wasm_bindgen(unchecked_param_type = "TileLoaderLike | null")] loader: JsValue) {
        self.loader = (!loader.is_null() && !loader.is_undefined()).then_some(loader);
    }

    /// Compressed texture formats of the GPU (their WebGL extensions get enabled), for the KTX2 transcoding
    #[wasm_bindgen(js_name = textureFormats, unchecked_return_type = "TextureFormats")]
    #[must_use]
    pub fn texture_formats(&self) -> JsValue {
        let has = |names: &[&str]| names.iter().any(|n| self.raw.get_extension(n).ok().flatten().is_some());
        js::object(&[
            ("astc", has(&["WEBGL_compressed_texture_astc"]).into()),
            ("bptc", has(&["EXT_texture_compression_bptc"]).into()),
            ("etc", has(&["WEBGL_compressed_texture_etc"]).into()),
            ("s3tc", has(&["WEBGL_compressed_texture_s3tc"]).into()),
        ])
    }

    /// 3D Tiles (b3dm / glb textured meshes, external tilesets) from the URL of a `tileset.json`. `max_error`:
    /// screen-space error (CSS pixels) above which a tile is refined; 16 is the usual value, lower is finer.
    #[wasm_bindgen(js_name = addTiles3d)]
    pub fn add_tiles3d(&mut self, url: String, max_error: f64) -> Result<u32, JsValue> {
        let layer = Tiles3dLayer::new(&self.gl, url, max_error, self.loader.clone(), &self.redraw)?;
        Ok(self.add_layer(Box::new(layer)))
    }

    /// Screen-space error threshold and opacity of a 3D Tiles layer
    #[wasm_bindgen(js_name = setTiles3dStyle)]
    pub fn set_tiles3d_style(&mut self, id: u32, max_error: f64, opacity: f32) {
        if let Some((layer, _)) = self.layer_mut::<Tiles3dLayer>(id) {
            layer.max_error = max_error;
            layer.opacity = opacity;
        }
    }

    /// Counters of a 3D Tiles layer, undefined if there is none with this id
    #[wasm_bindgen(js_name = tiles3dStats, unchecked_return_type = "Tiles3dStats | undefined")]
    #[must_use]
    pub fn tiles3d_stats(&self, id: u32) -> JsValue {
        self.layer::<Tiles3dLayer>(id).map_or(JsValue::UNDEFINED, |layer| {
            let s = layer.stats;
            js::object(&[
                ("drawn", s.drawn.into()),
                ("loaded", s.loaded.into()),
                ("inFlight", s.in_flight.into()),
                ("prefetching", s.prefetching.into()),
                ("gpuMb", (s.gpu_bytes as f64 / 1_048_576.0).into()),
                ("tiles", s.tiles.into()),
            ])
        })
    }
}
