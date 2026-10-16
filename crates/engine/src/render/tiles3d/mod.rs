//! 3D Tiles layer: loads a tileset (b3dm / glb meshes, external tilesets) and draws its textured meshes.
//!
//! The tree and its traversal live in `tiles3d::tileset`, the decoding in `tiles3d::decode`; this module fetches, uploads and draws.
//! Tiles are fetched and decoded asynchronously (images by the browser, off the main thread), uploaded a few per
//! frame, kept within a GPU memory budget (least recently drawn unloaded first), and their requests aborted when they
//! leave the view. Each mesh is drawn with a matrix computed in f64 (`View::ecef_matrix`), its vertices in f32 around
//! the tile's center: exact at any zoom, on the plane and on the globe.
//!
//! - `state`: the loading state of each tile and the requests of a frame (pure)
//! - `gpu`: the uploads; `fetch`: the downloads
//!
//! A root tileset that cannot be loaded is retried a few times, with a growing pause; meanwhile the layer does not
//! count as loading (the map can go idle), and an error goes to the console.

mod fetch;
mod gpu;
mod state;

use crate::engine::Engine;
use crate::js;
use crate::layer::{Frame, Layer, Loading as Profile};
use crate::render::gl;
use fetch::{close, decode_images, fetch_bytes, get_array, js_get, load_in_worker};
use geo::camera::View;
use glow::HasContext;
use gpu::{GpuMesh, delete_mesh, upload, upload_decoded};
use state::{COOLDOWN_MS, Loading, State, States, root_retry_delay};
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;
use tiles3d::decode::{self, Mesh};
use tiles3d::tileset::{self, Content, Tree};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

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

enum Arrival {
    /// Decoded on this thread (no loader)
    Mesh(Mesh, Vec<Option<web_sys::ImageBitmap>>),
    /// Decoded by a worker: `{ anchor, primitives: [{ vertices, indices, bitmap }] }`
    Decoded(JsValue),
    Tileset(Vec<u8>, String),
    /// A worker found a JSON document rather than a mesh (an external tileset behind an extension-less URI): asked
    /// again as a tileset
    NotMesh,
    Failed,
}

/// The virtual tile of the root tileset's arrival
const ROOT: usize = usize::MAX;

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

/// Where the root tileset is
enum Root {
    /// Loading: the attempt number (0: the first)
    Loading(u32),
    Ready(usize),
    /// Given up after `state::ROOT_ATTEMPTS` attempts, or not a tileset
    Failed,
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
    root: Root,
    states: States<GpuMesh>,
    inbox: Inbox,
    /// Arrived, waiting for their upload
    queue: VecDeque<(usize, Arrival)>,
    /// Tiles whose content turned out to be a tileset (by its first bytes, the URI not telling)
    json: HashSet<usize>,
    /// Pool of decoding workers (`load(id, url, transform, yUp) → Promise`, `abort(id)`); None: decoding here
    loader: Option<JsValue>,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 4],
    pub max_error: f64,
    pub opacity: f32,
    /// GPU memory budget for the meshes (bytes)
    pub budget: usize,
    pub stats: Tiles3dStats,
    /// Buffers of `draw_tiles`, kept from frame to frame
    stale: Vec<Loading>,
    requests: Vec<usize>,
    redraw: Rc<dyn Fn()>,
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
            url,
            tree: Tree::default(),
            root: Root::Loading(0),
            states: States::default(),
            inbox: Rc::default(),
            queue: VecDeque::new(),
            json: HashSet::new(),
            loader,
            program,
            uniforms,
            max_error,
            opacity: 1.0,
            budget: 512 << 20,
            stats: Tiles3dStats::default(),
            stale: Vec::new(),
            requests: Vec::new(),
            redraw: redraw.clone(),
        };
        layer.fetch_root(0.0);
        Ok(layer)
    }

    /// Fetches the root tileset after `delay_ms`: it arrives as the virtual tile `ROOT`
    fn fetch_root(&self, delay_ms: f64) {
        let (inbox, redraw, url) = (self.inbox.clone(), self.redraw.clone(), self.url.clone());
        wasm_bindgen_futures::spawn_local(async move {
            if delay_ms > 0.0 {
                js::sleep(delay_ms as i32).await;
            }
            // After a failure, past the HTTP cache (it may hold the error)
            let arrival = match fetch_bytes(&url, None, delay_ms > 0.0).await {
                Ok(bytes) => Arrival::Tileset(bytes, url),
                Err(_) => Arrival::Failed,
            };
            inbox.borrow_mut().push((ROOT, 0, arrival));
            redraw();
        });
    }

    /// The root tileset failed to load (`retry`: worth another attempt)
    fn root_failed(&mut self, error: &str, retry: bool) {
        let Root::Loading(attempt) = self.root else {
            return;
        };
        let delay = if retry { root_retry_delay(attempt + 1) } else { None };
        let next = match delay {
            Some(ms) => format!("retried in {} s", ms / 1000.0),
            None => "given up".to_owned(),
        };
        web_sys::console::error_1(&JsValue::from_str(&format!("3D Tiles {}: {error}; {next}", self.url)));
        match delay {
            Some(ms) => {
                self.root = Root::Loading(attempt + 1);
                self.fetch_root(ms);
            }
            None => self.root = Root::Failed,
        }
    }

    fn receive(&mut self, gl: &glow::Context, raw: &web_sys::WebGl2RenderingContext, frame: u64, budget_ms: f64) {
        let now = js_sys::Date::now();
        let arrivals: Vec<_> = self.inbox.borrow_mut().drain(..).collect();
        for (tile, id, arrival) in arrivals {
            if tile == ROOT {
                match arrival {
                    Arrival::Tileset(json, url) => {
                        if let Err(e) = self.graft(&json, &url, None) {
                            self.root_failed(&e, false);
                        }
                    }
                    other => {
                        discard(other);
                        self.root_failed("the tileset could not be loaded", true);
                    }
                }
                continue;
            }
            // Only the answer to the current request of a tile counts (not one that was aborted, then asked again)
            let current = self.states.is_current(tile, id);
            match arrival {
                Arrival::Tileset(json, url) if current => {
                    if let Err(e) = self.graft(&json, &url, Some(tile)) {
                        web_sys::console::warn_1(&JsValue::from_str(&format!("3D Tiles {}: {e}", self.url)));
                    }
                }
                Arrival::Mesh(_, _) | Arrival::Decoded(_) if current => self.queue.push_back((tile, arrival)),
                Arrival::NotMesh if current => {
                    self.json.insert(tile);
                    self.states.replace(tile, State::Unloaded);
                }
                Arrival::Failed if current => {
                    self.states.replace(tile, State::Failed(now + COOLDOWN_MS));
                }
                stale => discard(stale),
            }
        }
        // Uploads within the frame's time budget (at least one), the rest waits for the next frames
        let start = js_sys::Date::now();
        let mut uploaded = 0;
        while uploaded == 0 || js_sys::Date::now() - start < budget_ms {
            let Some((tile, arrival)) = self.queue.pop_front() else {
                break;
            };
            uploaded += 1;
            if !matches!(self.states.get(tile), Some(State::Loading(_))) {
                discard(arrival);
                continue;
            }
            let uploaded = match arrival {
                Arrival::Mesh(mesh, images) => upload(gl, &mesh, images, frame),
                Arrival::Decoded(tile) => upload_decoded(gl, raw, &tile, frame).map_err(|e| {
                    // The images not uploaded are closed
                    discard(Arrival::Decoded(tile));
                    format!("{e:?}")
                }),
                other => {
                    discard(other);
                    continue;
                }
            };
            match uploaded {
                Ok(gpu) => {
                    self.states.replace(tile, State::Ready(gpu));
                }
                Err(e) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!("3D Tiles upload: {e}")));
                    self.states
                        .replace(tile, State::Failed(js_sys::Date::now() + COOLDOWN_MS));
                }
            }
        }
    }

    /// Adds a tileset: the root one (`parent` None) or an external one (or implicit subtree) under `parent`
    fn graft(&mut self, json: &[u8], url: &str, parent: Option<usize>) -> Result<(), String> {
        let added = self.tree.add(json, url, parent);
        if let Some(p) = parent {
            self.states.replace(p, State::Empty);
        }
        let root = added?;
        if parent.is_none() {
            self.root = Root::Ready(root);
        }
        let nodes = &self.tree.nodes;
        self.states.grow(nodes.len(), |i| {
            matches!(nodes[i].content, Content::None | Content::Unsupported)
        });
        Ok(())
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

    fn request(&mut self, tile: usize) {
        // Implicit tiling: the levels of a subtree already in memory are grafted without a download
        if self.tree.expandable(tile) {
            if let Err(e) = self.graft(&[], "", Some(tile)) {
                web_sys::console::warn_1(&JsValue::from_str(&format!("3D Tiles {}: {e}", self.url)));
            }
            return;
        }
        let Some(node) = self.tree.nodes.get(tile) else {
            return;
        };
        let (url, is_tileset) = match &node.content {
            Content::Mesh(url) => (url.clone(), self.json.contains(&tile)),
            Content::Tileset(url) => (url.clone(), true),
            _ => return,
        };
        let (transform, y_up) = (node.transform, node.y_up);
        // A tile that failed before is fetched past the HTTP cache (it may hold the error)
        let reload = self.states.failed_before(tile);
        let id = next_request();
        let (inbox, redraw) = (self.inbox.clone(), self.redraw.clone());
        // Meshes go to the worker pool when there is one: fetched and decoded off the main thread
        if !is_tileset
            && let Some(promise) = self
                .loader
                .as_ref()
                .and_then(|l| load_in_worker(l, id, &url, &transform, y_up, reload))
        {
            self.states
                .replace(tile, State::Loading(Loading { id, controller: None }));
            wasm_bindgen_futures::spawn_local(async move {
                let arrival = match JsFuture::from(promise).await {
                    Ok(decoded) => Arrival::Decoded(decoded),
                    // A JSON document where a mesh was expected: an external tileset
                    Err(e) if format!("{e:?}").contains("JSON document") => Arrival::NotMesh,
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
        self.states.replace(
            tile,
            State::Loading(Loading {
                id,
                controller: Some(controller),
            }),
        );
        wasm_bindgen_futures::spawn_local(async move {
            let arrival = match fetch_bytes(&url, Some(&signal), reload).await {
                // The content tells what it is when the URI does not
                Ok(bytes) if is_tileset || tileset::sniff(&bytes) == tileset::Kind::Json => {
                    Arrival::Tileset(bytes, url)
                }
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

    /// Selects, loads and draws; returns true while tiles are loading
    fn draw_tiles(
        &mut self,
        gl: &glow::Context,
        raw: &web_sys::WebGl2RenderingContext,
        view: &View,
        frame: u64,
        loading: Profile,
    ) -> bool {
        self.receive(gl, raw, frame, loading.upload_budget_ms);
        let root = match self.root {
            Root::Ready(root) => root,
            // The first attempt counts as loading; while retrying, or given up, the layer does not hold the map back
            Root::Loading(attempt) => return attempt == 0,
            Root::Failed => return false,
        };
        let now = js_sys::Date::now();
        let states = &self.states;
        let selection = tileset::select(&self.tree, root, view, self.max_error, &|i| states.readiness(i, now));
        // Downloads of tiles that left the view are aborted
        let mut stale = std::mem::take(&mut self.stale);
        self.states.take_stale(&selection.visible, &mut stale);
        for loading in stale.drain(..) {
            self.abort(&loading);
        }
        self.stale = stale;
        let mut requests = std::mem::take(&mut self.requests);
        self.states.plan_requests(
            &selection.request,
            &selection.prefetch,
            loading.mesh_requests,
            &mut requests,
        );
        for &i in &requests {
            self.request(i);
        }
        self.requests = requests;
        self.states.touch(&selection.draw, frame);
        self.states.touch(&selection.backfill, frame);
        self.evict(gl, frame);
        self.render(gl, view, &selection.draw, &selection.backfill);
        let prefetching = selection
            .prefetch
            .iter()
            .filter(|&&i| self.states.is_loading(i))
            .count();
        let (in_flight, loaded, gpu_bytes) = self.states.counts();
        self.stats = Tiles3dStats {
            drawn: selection.draw.len() + selection.backfill.len(),
            loaded,
            in_flight: in_flight.saturating_sub(prefetching),
            prefetching,
            gpu_bytes,
            tiles: self.tree.nodes.len(),
        };
        if !self.queue.is_empty() {
            (self.redraw)();
        }
        in_flight > prefetching || !self.queue.is_empty()
    }

    /// Frees the least recently drawn meshes beyond the budget
    fn evict(&mut self, gl: &glow::Context, frame: u64) {
        for i in self.states.over_budget(self.budget, frame) {
            if let Some(State::Ready(mesh)) = self.states.replace(i, State::Unloaded) {
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
                let Some(State::Ready(mesh)) = self.states.get(i) else {
                    continue;
                };
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
        self.draw_tiles(frame.gl, frame.raw, frame.view, frame.number, frame.loading)
    }

    fn delete(&mut self, gl: &glow::Context) {
        for state in self.states.drain() {
            match state {
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
        Arrival::Tileset(..) | Arrival::NotMesh | Arrival::Failed => {}
    }
}

impl Drop for Tiles3dLayer {
    /// Stops the downloads and frees the decoded images still waiting
    fn drop(&mut self) {
        for i in 0..self.states.len() {
            if let Some(State::Loading(loading)) = self.states.get(i) {
                self.abort(loading);
            }
        }
        for (_, arrival) in self.queue.drain(..) {
            discard(arrival);
        }
        for (_, _, arrival) in self.inbox.borrow_mut().drain(..) {
            discard(arrival);
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
