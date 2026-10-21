//! Models: a glTF / GLB mesh drawn at many positions, in one instanced draw per primitive. The file is fetched and
//! decoded like a 3D Tiles tile (`tiles3d::decode`: meshopt, quantization, Draco and KTX2 in the decoding workers),
//! uploaded once; each instance gets its matrix every frame, made in f64 in the local frame (relative to the camera)
//! from its position, heading, pitch, roll and scale, handed over in f32: exact at any zoom, on the plane and the
//! globe. Textured surfaces are drawn unlit as the 3D Tiles are; surfaces without texture take their material's base
//! color, lit from above so that their shape reads. Instances are picked by their index.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::gl::{self, DepthState, Objects};
use crate::render::tiles3d::fetch::{decode_images, fetch_bytes, get_array, js_get, load_in_worker};
use crate::render::tiles3d::gpu::{GpuMesh, delete_mesh, upload, upload_decoded};
use crate::render::tiles3d::next_request;
use geo::camera::View;
use glam::{DMat4, DVec3};
use glow::HasContext;
use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;
use tiles3d::decode::{self, Mesh};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

const VERTEX: &str = "uniform mat4 u_view_proj;
layout(location = 0) in vec3 a_position;
layout(location = 1) in vec2 a_uv;
layout(location = 2) in mat4 a_model;
out vec2 v_uv;
out vec3 v_local;
flat out highp uint v_id;
void main() {
  vec4 p = a_model * vec4(a_position, 1.0);
  v_uv = a_uv;
  v_local = p.xyz;
  v_id = uint(gl_InstanceID) + 1u;
  gl_Position = u_view_proj * p;
}";

const FRAGMENT: &str = "uniform sampler2D u_texture;
uniform float u_textured;
uniform vec4 u_color;
uniform vec3 u_eye;
in vec2 v_uv;
in vec3 v_local;
flat in highp uint v_id;
out vec4 color;
void main() {
  if (u_textured > 0.5) {
    color = texture(u_texture, v_uv) * u_color;
    return;
  }
  // The face's normal from the screen derivatives, turned towards the eye; light from above, the south-east
  vec3 n = normalize(cross(dFdx(v_local), dFdy(v_local)));
  if (dot(n, u_eye - v_local) < 0.0) n = -n;
  float light = 0.55 + 0.45 * max(dot(n, normalize(vec3(0.4, 0.5, 1.0))), 0.0);
  color = vec4(u_color.rgb * light, u_color.a);
}";

const PICK_FRAGMENT: &str = "uniform float u_layer;
flat in highp uint v_id;
out vec4 color;
void main() {
  color = pick_color(v_id, u_layer);
}";

/// Numbers per instance handed over by the page: longitude, latitude, altitude, heading, pitch, roll, scale
pub const INSTANCE_NUMBERS: usize = 7;

/// Where and how an instance stands
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    /// Degrees
    pub lon: f64,
    pub lat: f64,
    /// Metres, above the ground or above the ellipsoid (`ModelLayer::ground`)
    pub altitude: f64,
    /// Degrees: heading clockwise from the north, pitch nose up, roll right side down
    pub heading: f64,
    pub pitch: f64,
    pub roll: f64,
    pub scale: f64,
}

impl Instance {
    /// The instances of the page's numbers (`INSTANCE_NUMBERS` each); one that is not a number is left out
    #[must_use]
    pub fn parse(numbers: &[f64]) -> Vec<Self> {
        numbers
            .as_chunks::<INSTANCE_NUMBERS>()
            .0
            .iter()
            .map(|&[lon, lat, altitude, heading, pitch, roll, scale]| Self {
                lon,
                lat,
                altitude,
                heading,
                pitch,
                roll,
                scale,
            })
            .collect()
    }

    fn valid(&self) -> bool {
        [
            self.lon,
            self.lat,
            self.altitude,
            self.heading,
            self.pitch,
            self.roll,
            self.scale,
        ]
        .iter()
        .all(|v| v.is_finite())
            && self.lat.abs() <= 90.0
    }

    /// Model coordinates (the decoded mesh: glTF's y up turned to z up, around `anchor`) to the instance's east-north-up
    /// frame in metres: the model's front (glTF +Z) to the north turned by the heading, its up (glTF +Y) to the
    /// vertical, then pitch, roll and scale
    #[must_use]
    pub fn matrix(&self, anchor: DVec3) -> DMat4 {
        DMat4::from_rotation_z(-self.heading.to_radians())
            * DMat4::from_rotation_x(self.pitch.to_radians())
            * DMat4::from_rotation_y(self.roll.to_radians())
            * DMat4::from_scale(DVec3::splat(self.scale))
            // The decoder turns glTF's front (+Z) to the south: half a turn faces it north
            * DMat4::from_rotation_z(PI)
            * DMat4::from_translation(anchor)
    }
}

/// Fills `out` with the matrix of each instance in the local frame of a view (16 floats each, column-major), zeros
/// for an instance left out (not a number, or behind the globe): an instance keeps its index in the buffer, which the
/// picking gives back. `ground`: the altitude of the relief under an instance (metres, as drawn), None without relief
pub fn instance_matrices(
    view: &View,
    instances: &[Instance],
    anchor: DVec3,
    ground: impl Fn(f64, f64) -> Option<f64>,
    out: &mut Vec<f32>,
) {
    out.clear();

    for instance in instances {
        if !instance.valid() {
            out.extend([0.0; 16]);
            continue;
        }

        let altitude = instance.altitude + ground(instance.lon, instance.lat).unwrap_or(0.0);
        let local = view.enu_matrix(DVec3::new(instance.lon, instance.lat, altitude)) * instance.matrix(anchor);

        if view.globe.factor > 0.0 && !view.faces_camera(local.w_axis.truncate()) {
            out.extend([0.0; 16]);
            continue;
        }
        out.extend(gl::mat4(&local));
    }
}

enum Arrival {
    /// Decoded on this thread (no loader)
    Mesh(Mesh, Vec<Option<web_sys::ImageBitmap>>),
    /// Decoded by a worker: `{ anchor, primitives }`
    Decoded(JsValue),
    Failed(String),
}

enum Model {
    /// The request number, and the controller of a download on this thread
    Loading(u32, Option<web_sys::AbortController>),
    Ready(GpuMesh),
    Failed,
}

pub struct ModelLayer {
    url: String,
    model: Model,
    inbox: Rc<RefCell<Option<Arrival>>>,
    /// The decoding workers' pool (`load`, `abort`), if any
    loader: Option<JsValue>,
    pub instances: Vec<Instance>,
    /// Altitudes above the relief (else above the ellipsoid)
    pub ground: bool,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 5],
    pick_program: glow::Program,
    pick_uniforms: [Option<glow::UniformLocation>; 2],
    /// The instances' matrices
    buffer: glow::Buffer,
    matrices: Vec<f32>,
    redraw: Rc<dyn Fn()>,
}

impl ModelLayer {
    pub fn new(
        gl: &glow::Context,
        url: String,
        instances: Vec<Instance>,
        ground: bool,
        loader: Option<JsValue>,
        redraw: &Rc<dyn Fn()>,
    ) -> Result<Self, String> {
        let mut objects = Objects::new(gl);
        let vertex = gl::vertex_shader(&[VERTEX]);
        let program = objects.program(&vertex, &gl::fragment_shader(&[FRAGMENT]))?;
        let pick_program = objects.program(&vertex, &gl::fragment_shader(&[PICK_FRAGMENT]))?;
        let buffer = objects.buffer()?;

        objects.keep();

        let mut layer = Self {
            uniforms: gl::uniforms(
                gl,
                program,
                ["u_view_proj", "u_texture", "u_textured", "u_color", "u_eye"],
            ),
            pick_uniforms: gl::uniforms(gl, pick_program, ["u_view_proj", "u_layer"]),
            url,
            model: Model::Failed,
            inbox: Rc::default(),
            loader,
            instances,
            ground,
            program,
            pick_program,
            buffer,
            matrices: Vec::new(),
            redraw: redraw.clone(),
        };

        layer.model = layer.load();

        Ok(layer)
    }

    /// Starts loading the file: in a worker when there is a pool (Draco and KTX2 decoded there), else here
    fn load(&self) -> Model {
        let (inbox, redraw, url) = (self.inbox.clone(), self.redraw.clone(), self.url.clone());
        let id = next_request();
        let deliver = move |arrival| {
            *inbox.borrow_mut() = Some(arrival);
            redraw();
        };

        if let Some(promise) = self
            .loader
            .as_ref()
            .and_then(|l| load_in_worker(l, id, &url, &DMat4::IDENTITY, (true, false), "high"))
        {
            wasm_bindgen_futures::spawn_local(async move {
                deliver(match JsFuture::from(promise).await {
                    Ok(decoded) => Arrival::Decoded(decoded),
                    Err(e) => Arrival::Failed(format!("{e:?}")),
                });
            });

            return Model::Loading(id, None);
        }

        let Ok(controller) = web_sys::AbortController::new() else {
            return Model::Failed;
        };
        let signal = controller.signal();

        wasm_bindgen_futures::spawn_local(async move {
            let arrival = match fetch_bytes(&url, Some(&signal), false, "high").await {
                Ok(bytes) => match decode::decode(&bytes, &DMat4::IDENTITY, true, &[]) {
                    Ok(mesh) => {
                        let images = decode_images(&mesh).await;

                        Arrival::Mesh(mesh, images)
                    }
                    Err(e) => Arrival::Failed(e),
                },
                Err(e) => Arrival::Failed(format!("{e:?}")),
            };

            if signal.aborted() {
                discard(arrival);
            } else {
                deliver(arrival);
            }
        });

        Model::Loading(id, Some(controller))
    }

    /// Uploads the file once it has arrived; its primitives' arrays then read the instances' matrices
    fn receive(&mut self, gl: &glow::Context, raw: &web_sys::WebGl2RenderingContext, frame: u64) {
        let Some(arrival) = self.inbox.borrow_mut().take() else {
            return;
        };
        let uploaded = match arrival {
            Arrival::Mesh(mesh, images) => upload(gl, &mesh, images, frame),
            Arrival::Decoded(tile) => upload_decoded(gl, raw, &tile, frame).map_err(|e| {
                discard(Arrival::Decoded(tile));

                format!("{e:?}")
            }),
            Arrival::Failed(e) => Err(e),
        };

        self.model = match uploaded {
            Ok(mesh) => {
                // SAFETY: setup of the arrays just uploaded: four columns of a matrix per instance
                unsafe {
                    for p in &mesh.primitives {
                        gl.bind_vertex_array(Some(p.vao));
                        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));

                        for column in 0..4 {
                            gl.enable_vertex_attrib_array(2 + column);
                            gl.vertex_attrib_pointer_f32(2 + column, 4, glow::FLOAT, false, 64, 16 * column as i32);
                            gl.vertex_attrib_divisor(2 + column, 1);
                        }
                    }
                    gl.bind_vertex_array(None);
                }

                Model::Ready(mesh)
            }
            Err(e) => {
                web_sys::console::error_1(&JsValue::from_str(&format!("Model {}: {e}", self.url)));

                Model::Failed
            }
        };
    }

    /// The instances' matrices for this frame's view, uploaded
    fn upload_matrices(&self, frame: &Frame, anchor: DVec3, out: &mut Vec<f32>) {
        let relief = frame.relief.filter(|_| self.ground);
        let ground = |lon, lat| {
            let (source, exaggeration) = relief?;

            source
                .altitude(geo::camera::mercator(lon, lat))
                .map(|h| h * f64::from(exaggeration))
        };

        instance_matrices(frame.view, &self.instances, anchor, ground, out);

        // SAFETY: upload into a buffer owned by this layer
        unsafe {
            frame.gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            frame
                .gl
                .buffer_data_u8_slice(glow::ARRAY_BUFFER, gl::bytes(out), glow::DYNAMIC_DRAW);
        }
    }

    /// Draws the instances (or their picking colors, `pick`: the layer's ordinal), the matrices uploaded
    fn render(&self, frame: &Frame, mesh: &GpuMesh, pick: Option<u8>) {
        let gl = frame.gl;
        let view = frame.view;
        let count = self.instances.len() as i32;
        // The models hide one another and what is drawn after them, and are hidden by the hills
        let _depth = DepthState::solid(gl);

        // SAFETY: draws with objects owned by this layer and its mesh
        unsafe {
            if let Some(layer) = pick {
                let [matrix, ordinal] = &self.pick_uniforms;

                gl.use_program(Some(self.pick_program));
                gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
                gl.uniform_1_f32(ordinal.as_ref(), f32::from(layer));
            } else {
                let [matrix, texture, _, _, eye] = &self.uniforms;

                gl.use_program(Some(self.program));
                gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
                gl.uniform_1_i32(texture.as_ref(), 0);

                let e = view.eye.as_vec3();

                gl.uniform_3_f32(eye.as_ref(), e.x, e.y, e.z);
                gl.active_texture(glow::TEXTURE0);
            }

            let [_, _, textured, color, _] = &self.uniforms;

            for p in &mesh.primitives {
                if pick.is_none() {
                    gl.uniform_1_f32(textured.as_ref(), if p.texture.is_some() { 1.0 } else { 0.0 });
                    gl.uniform_4_f32_slice(color.as_ref(), &p.color);
                    gl.bind_texture(glow::TEXTURE_2D, p.texture);
                }
                gl.bind_vertex_array(Some(p.vao));
                gl.draw_elements_instanced(glow::TRIANGLES, p.count, glow::UNSIGNED_INT, 0, count);
            }
            gl.bind_vertex_array(None);
        }
    }
}

impl Layer for ModelLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        self.receive(frame.gl, frame.raw, frame.number);

        let mesh = match &self.model {
            Model::Ready(mesh) => mesh,
            Model::Loading(..) => return true,
            Model::Failed => return false,
        };

        if self.instances.is_empty() {
            return false;
        }

        let mut matrices = std::mem::take(&mut self.matrices);

        self.upload_matrices(frame, mesh.anchor, &mut matrices);
        self.render(frame, mesh, None);
        self.matrices = matrices;

        false
    }

    fn pickable(&self) -> bool {
        matches!(self.model, Model::Ready(_)) && !self.instances.is_empty()
    }

    fn draw_depth(&self, frame: &Frame) {
        self.draw_pick(frame, 0);
    }

    fn draw_pick(&self, frame: &Frame, layer: u8) {
        if let Model::Ready(mesh) = &self.model {
            // The picking pass's view may not be the last frame's
            self.upload_matrices(frame, mesh.anchor, &mut Vec::new());
            self.render(frame, mesh, Some(layer));
        }
    }

    fn delete(&mut self, gl: &glow::Context) {
        self.abort();
        if let Model::Ready(mesh) = std::mem::replace(&mut self.model, Model::Failed) {
            delete_mesh(gl, &mesh);
        }

        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_program(self.pick_program);
            gl.delete_buffer(self.buffer);
        }
    }
}

impl ModelLayer {
    /// Stops the download under way
    fn abort(&self) {
        match &self.model {
            Model::Loading(_, Some(controller)) => controller.abort(),
            Model::Loading(id, None) => {
                if let Some(loader) = &self.loader
                    && let Ok(abort) = js_get(loader, "abort").dyn_into::<js_sys::Function>()
                {
                    let _ = abort.call1(loader, &JsValue::from(*id));
                }
            }
            Model::Ready(_) | Model::Failed => {}
        }
    }
}

impl Drop for ModelLayer {
    /// Stops the download and frees the decoded images still waiting
    fn drop(&mut self) {
        self.abort();

        if let Some(arrival) = self.inbox.borrow_mut().take() {
            discard(arrival);
        }
    }
}

/// Frees what an arrival holds outside the wasm heap (decoded images)
fn discard(arrival: Arrival) {
    match arrival {
        Arrival::Mesh(_, images) => images.into_iter().flatten().for_each(|i| i.close()),
        Arrival::Decoded(tile) => {
            for p in get_array(&tile, "primitives").iter() {
                if let Ok(bitmap) = js_get(&p, "bitmap").dyn_into::<web_sys::ImageBitmap>() {
                    bitmap.close();
                }
            }
        }
        Arrival::Failed(_) => {}
    }
}

#[wasm_bindgen]
impl Engine {
    /// A glTF / GLB model (its URL, a `blob:` one for data of the page) drawn at each instance of `instances`
    /// (`INSTANCE_NUMBERS` numbers each: longitude, latitude, altitude, heading, pitch, roll in degrees, scale);
    /// `ground`: altitudes above the relief, else above the ellipsoid
    #[wasm_bindgen(js_name = addModels)]
    pub fn add_models(&mut self, url: String, instances: &[f64], ground: bool) -> Result<u32, JsValue> {
        let layer = ModelLayer::new(
            &self.gl,
            url,
            Instance::parse(instances),
            ground,
            self.loader.clone(),
            &self.redraw,
        )?;

        Ok(self.add_layer(Box::new(layer)))
    }

    /// Replaces the instances of a model layer (as for `addModels`) and the reference of their altitudes
    #[wasm_bindgen(js_name = setModelInstances)]
    pub fn set_model_instances(&mut self, id: u32, instances: &[f64], ground: bool) {
        if let Some((layer, _)) = self.layer_mut::<ModelLayer>(id) {
            layer.instances = Instance::parse(instances);
            layer.ground = ground;
            (self.redraw)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::camera::{Camera, mercator};

    fn instance(heading: f64, pitch: f64, roll: f64, scale: f64) -> Instance {
        Instance {
            lon: 2.424_573,
            lat: 48.845_726,
            altitude: 0.0,
            heading,
            pitch,
            roll,
            scale,
        }
    }

    /// glTF axes as the decoder leaves them (y up turned to z up: +Z front to -y, +Y up to +z, +X left stays +x)
    const FRONT: DVec3 = DVec3::NEG_Y;
    const UP: DVec3 = DVec3::Z;
    const LEFT: DVec3 = DVec3::X;

    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < 1e-9
    }

    #[test]
    fn the_front_faces_the_heading() {
        // Heading 0: front north, up up, left west; 90: front east, left north
        let m = instance(0.0, 0.0, 0.0, 1.0).matrix(DVec3::ZERO);

        assert!(close(m.transform_vector3(FRONT), DVec3::Y));
        assert!(close(m.transform_vector3(UP), DVec3::Z));
        assert!(close(m.transform_vector3(LEFT), DVec3::NEG_X));

        let m = instance(90.0, 0.0, 0.0, 1.0).matrix(DVec3::ZERO);

        assert!(close(m.transform_vector3(FRONT), DVec3::X));
        assert!(close(m.transform_vector3(LEFT), DVec3::Y));

        // Pitch 90 points the nose up; roll 90 lays the right side down (up to the east when facing north)
        let m = instance(0.0, 90.0, 0.0, 1.0).matrix(DVec3::ZERO);

        assert!(close(m.transform_vector3(FRONT), DVec3::Z));

        let m = instance(0.0, 0.0, 90.0, 1.0).matrix(DVec3::ZERO);

        assert!(close(m.transform_vector3(UP), DVec3::X));
    }

    #[test]
    fn the_anchor_and_the_scale_put_the_model_s_origin_at_the_position() {
        // Decoded positions are relative to the anchor: the model's origin is at -anchor, where the instance stands
        let anchor = DVec3::new(1.0, -2.0, 3.0);
        let m = instance(30.0, 10.0, -5.0, 2.5).matrix(anchor);

        assert!(close(m.transform_point3(-anchor), DVec3::ZERO));

        // A point 1 m above the origin: 2.5 m up, whatever the heading (pitch and roll off)
        let m = instance(123.0, 0.0, 0.0, 2.5).matrix(anchor);

        assert!(close(m.transform_point3(-anchor + UP), DVec3::new(0.0, 0.0, 2.5)));
    }

    #[test]
    fn instances_land_where_the_ground_is() {
        let mut camera = Camera::new(2.424_573, 48.845_726, 16.0);

        camera.width = 800.0;
        camera.height = 600.0;

        let view = camera.view();
        let instances = Instance::parse(&[
            2.424_573,
            48.845_726,
            10.0,
            0.0,
            0.0,
            0.0,
            1.0,
            // Not a number: left out, its index kept
            f64::NAN,
            48.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            2.425,
            48.846,
            0.0,
            45.0,
            0.0,
            0.0,
            3.0,
        ]);

        assert_eq!(instances.len(), 3);

        let mut out = Vec::new();

        // The relief 100 m high everywhere
        instance_matrices(&view, &instances, DVec3::ZERO, |_, _| Some(100.0), &mut out);
        assert_eq!(out.len(), 48);

        let k = view.pixels_per_metre(48.845_726);

        // The first stands at the center, 110 m up; the second is all zeros
        assert!((f64::from(out[12])).abs() < 1e-3 && (f64::from(out[13])).abs() < 1e-3);
        assert!((f64::from(out[14]) - 110.0 * k).abs() < 1e-3);
        assert!(out[16..32].iter().all(|&v| v == 0.0));

        // The third where the plane puts its position, 3 m to a metre of its frame
        let at = view.position(mercator(2.425, 48.846), 0.0);

        assert!((f64::from(out[44]) - at.x).abs() < 1e-2 && (f64::from(out[45]) - at.y).abs() < 1e-2);

        let column = DVec3::new(f64::from(out[32]), f64::from(out[33]), f64::from(out[34]));

        assert!((column.length() / (3.0 * view.pixels_per_metre(48.846)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn instances_behind_the_globe_are_left_out() {
        let mut camera = Camera::new(2.4, 48.8, 1.5);

        camera.globe = true;
        camera.width = 800.0;
        camera.height = 600.0;

        let view = camera.view();
        let near = instance(0.0, 0.0, 0.0, 1.0);
        let far = Instance {
            lon: -177.6,
            lat: -48.8,
            ..near
        };
        let mut out = Vec::new();

        instance_matrices(&view, &[near, far], DVec3::ZERO, |_, _| None, &mut out);
        assert!(out[..16].iter().any(|&v| v != 0.0));
        assert!(out[16..].iter().all(|&v| v == 0.0));
    }
}
