//! 3D Tiles layer: loads a tileset (b3dm / glb meshes, external tilesets) and draws its textured meshes.
//!
//! The tree and its traversal live in `tileset`, the decoding in `b3dm`; this module fetches, uploads and draws.
//! Tiles are fetched and decoded asynchronously (images by the browser, off the main thread), uploaded a few per
//! frame, kept within a GPU memory budget (least recently drawn unloaded first), and their requests aborted when they
//! leave the view. Each mesh is drawn with a matrix computed in f64 (`View::ecef_matrix`), its vertices in f32 around
//! the tile's center: exact at any zoom, on the plane and on the globe.

use crate::b3dm::{self, Mesh};
use crate::camera::View;
use crate::gl::{self, bytes};
use crate::tileset::{self, Content, Readiness, Tree};
use glow::HasContext;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Requests in flight
const MAX_REQUESTS: usize = 12;
/// Meshes uploaded per frame
const UPLOADS_PER_FRAME: usize = 4;
/// After an error, a tile is asked again after this delay (ms)
const COOLDOWN_MS: f64 = 5_000.0;

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
    Loading(web_sys::AbortController),
    Ready(GpuMesh),
    /// No drawable content, for good (none, unsupported, grafted external tileset)
    Empty,
    /// Errored: asked again after this time (ms)
    Failed(f64),
}

enum Arrival {
    Mesh(Mesh, Vec<Option<web_sys::ImageBitmap>>),
    Tileset(Vec<u8>, String),
    Failed,
}

type Inbox = Rc<RefCell<Vec<(usize, Arrival)>>>;

#[derive(Default, Clone, Copy)]
pub struct Tiles3dStats {
    pub drawn: usize,
    pub loaded: usize,
    pub in_flight: usize,
    pub gpu_bytes: usize,
    pub tiles: usize,
}

pub struct Tiles3dLayer {
    url: String,
    tree: Tree,
    root: Option<usize>,
    states: Vec<State>,
    inbox: Inbox,
    queue: Vec<(usize, Mesh, Vec<Option<web_sys::ImageBitmap>>)>,
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
    pub fn new(gl: &glow::Context, url: String, max_error: f64, redraw: &Rc<dyn Fn()>) -> Result<Self, String> {
        let program = gl::program(gl, VERTEX, FRAGMENT)?;
        let uniforms = gl::uniforms(gl, program, ["u_matrix", "u_texture", "u_textured", "u_opacity"]);
        let layer = Self {
            url: url.clone(),
            tree: Tree::default(),
            root: None,
            states: Vec::new(),
            inbox: Rc::default(),
            queue: Vec::new(),
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
            let arrival = match fetch_bytes(&url, None).await {
                Ok(bytes) => Arrival::Tileset(bytes, url),
                Err(_) => Arrival::Failed,
            };
            inbox.borrow_mut().push((usize::MAX, arrival));
            redraw();
        });
        Ok(layer)
    }

    fn receive(&mut self, gl: &glow::Context, frame: u64) {
        let arrivals: Vec<_> = self.inbox.borrow_mut().drain(..).collect();
        for (tile, arrival) in arrivals {
            match arrival {
                Arrival::Tileset(json, url) => {
                    let parent = (tile != usize::MAX).then_some(tile);
                    match self.tree.add(&json, &url, parent) {
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
                Arrival::Mesh(mesh, images) => {
                    if tile < self.states.len() && matches!(self.states[tile], State::Loading(_)) {
                        self.queue.push((tile, mesh, images));
                    } else {
                        close(images);
                    }
                }
                Arrival::Failed => {
                    if tile < self.states.len() {
                        self.states[tile] = State::Failed(js_sys::Date::now() + COOLDOWN_MS);
                    }
                }
            }
        }
        let take = self.queue.len().min(UPLOADS_PER_FRAME);
        for (tile, mesh, images) in self.queue.drain(..take).collect::<Vec<_>>() {
            match upload(gl, &mesh, images, frame) {
                Ok(gpu) => {
                    self.gpu_bytes += gpu.bytes;
                    self.states[tile] = State::Ready(gpu);
                }
                Err(_) => self.states[tile] = State::Failed(js_sys::Date::now() + COOLDOWN_MS),
            }
        }
    }

    fn request(&mut self, tile: usize, redraw: &Rc<dyn Fn()>) {
        let Ok(controller) = web_sys::AbortController::new() else {
            return;
        };
        let node = &self.tree.nodes[tile];
        let (url, is_tileset) = match &node.content {
            Content::Mesh(url) => (url.clone(), false),
            Content::Tileset(url) => (url.clone(), true),
            _ => return,
        };
        let (transform, y_up) = (node.transform, node.y_up);
        let signal = controller.signal();
        self.states[tile] = State::Loading(controller);
        let (inbox, redraw) = (self.inbox.clone(), redraw.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let arrival = match fetch_bytes(&url, Some(&signal)).await {
                Ok(bytes) if is_tileset => Arrival::Tileset(bytes, url),
                Ok(bytes) => match b3dm::decode(&bytes, &transform, y_up) {
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
                if let Arrival::Mesh(_, images) = arrival {
                    close(images);
                }
                return;
            }
            inbox.borrow_mut().push((tile, arrival));
            redraw();
        });
    }

    fn readiness(&self, tile: usize, now: f64) -> Readiness {
        match &self.states[tile] {
            State::Ready(_) => Readiness::Ready,
            State::Empty => Readiness::Empty,
            State::Failed(until) if now < *until => Readiness::Empty,
            _ => Readiness::Pending,
        }
    }

    /// Selects, loads and draws; returns true while tiles are loading
    pub fn draw(&mut self, gl: &glow::Context, view: &View, frame: u64, redraw: &Rc<dyn Fn()>) -> bool {
        self.receive(gl, frame);
        let Some(root) = self.root else { return true };
        let now = js_sys::Date::now();
        let selection = tileset::select(&self.tree, root, view, self.max_error, &|i| self.readiness(i, now));
        // Downloads of tiles that left the view are aborted
        let mut visible = vec![false; self.states.len()];
        for &i in &selection.visible {
            visible[i] = true;
        }
        for (i, state) in self.states.iter_mut().enumerate() {
            if let State::Loading(controller) = state
                && !visible[i]
            {
                controller.abort();
                *state = State::Unloaded;
            }
        }
        let mut in_flight = self.states.iter().filter(|s| matches!(s, State::Loading(_))).count();
        for &i in &selection.request {
            if in_flight >= MAX_REQUESTS {
                break;
            }
            if matches!(self.states[i], State::Unloaded | State::Failed(_)) {
                self.request(i, redraw);
                in_flight += 1;
            }
        }
        for &i in &selection.draw {
            if let State::Ready(mesh) = &mut self.states[i] {
                mesh.used = frame;
            }
        }
        self.evict(gl, frame);
        self.render(gl, view, &selection.draw);
        self.stats = Tiles3dStats {
            drawn: selection.draw.len(),
            loaded: self.states.iter().filter(|s| matches!(s, State::Ready(_))).count(),
            in_flight,
            gpu_bytes: self.gpu_bytes,
            tiles: self.tree.nodes.len(),
        };
        if !self.queue.is_empty() {
            redraw();
        }
        in_flight > 0 || !self.queue.is_empty()
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

    fn render(&self, gl: &glow::Context, view: &View, tiles: &[usize]) {
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
            for &i in tiles {
                let State::Ready(mesh) = &self.states[i] else { continue };
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
            gl.disable(glow::DEPTH_TEST);
            gl.enable(glow::CULL_FACE);
        }
    }

    pub fn delete(&mut self, gl: &glow::Context) {
        for state in &mut self.states {
            match std::mem::replace(state, State::Empty) {
                State::Loading(controller) => controller.abort(),
                State::Ready(mesh) => delete_mesh(gl, &mesh),
                _ => {}
            }
        }
        for (_, _, images) in self.queue.drain(..) {
            close(images);
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
                    let texture = gl.create_texture()?;
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    gl.tex_image_2d_with_image_bitmap(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA8 as i32,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        &bitmap,
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
                    total += (bitmap.width() * bitmap.height()) as usize * 4 * 4 / 3;
                    bitmap.close();
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

async fn fetch_bytes(url: &str, signal: Option<&web_sys::AbortSignal>) -> Result<Vec<u8>, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_signal(signal);
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
            if let State::Loading(controller) = state {
                controller.abort();
            }
        }
    }
}
