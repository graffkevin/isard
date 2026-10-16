//! Circle layer: points drawn as discs with a stroke, styled per feature (fill and stroke colors, radius and stroke
//! width as zoom stops, a value through the layer's ramp): gauges, stations, marks. Each point is an instanced quad,
//! the disc worked out per pixel. Facing the screen (`MapLibre`'s `circle-pitch-alignment: viewport`) or lying on the
//! ground (`map`); in relief, standing on the ground, the hills hiding them. Drawn in the order of the points.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::drape::{self, Drape, DrapeUniforms};
use crate::render::features::{FEATURES, FeatureUniforms, Features};
use crate::render::gl::{self, Projection, bytes};
use glam::DVec2;
use glow::HasContext;
use wasm_bindgen::prelude::*;

const VERTEX: &str = r"
uniform mat4 u_matrix;
uniform vec2 u_plane_origin;
uniform vec2 u_origin;
uniform vec2 u_viewport;
uniform float u_depth_bias;
// 1: lying on the ground; 0: facing the screen
uniform float u_on_ground;
uniform float u_slop;
layout(location = 0) in vec2 a_point;
layout(location = 1) in uint a_feature;
flat out vec4 v_fill;
flat out vec4 v_stroke;
// Radius and outer radius (with the stroke), device pixels
flat out vec2 v_radii;
flat out uint v_id;
out vec2 v_offset;

void main() {
  uint f = a_feature;
  float radius = feature_size(f, 0u) * u_pixel_ratio;
  float stroke = max(feature_size(f, 1u), 0.0) * u_pixel_ratio;
  float outer = radius + stroke + u_slop;
  float opacity = layer_opacity();
  v_fill = feature_color(f, 0u);
  v_stroke = feature_color(f, 1u);
  v_fill.a *= opacity;
  v_stroke.a *= opacity;
  if (!feature_visible(f) || outer <= 0.0 || opacity <= 0.0) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
  // Corner of the quad, one pixel of antialiasing around
  vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1)) * 2.0 - 1.0;
  float extent = outer + 1.0;
  vec2 merc = u_origin + a_point;
  float h = altitude(merc);
  vec4 c;
  if (u_on_ground > 0.5) {
    // Pixels at the camera's scale, laid on the ground around the point
    vec2 offset = a_point * u_world + corner * extent / u_pixel_ratio;
    c = u_matrix * vec4(project_up(u_plane_origin + offset, merc, h), 1.0);
  } else {
    c = u_matrix * vec4(project_up(u_plane_origin + a_point * u_world, merc, h), 1.0);
    c.xy += corner * extent / u_viewport * c.w;
  }
  c.z -= u_depth_bias;
  gl_Position = c;
  v_offset = corner * extent;
  v_radii = vec2(radius, radius + stroke);
  v_id = f + 1u;
}";

const FRAGMENT: &str = r"
uniform float u_layer;
uniform bool u_pick;
flat in vec4 v_fill;
flat in vec4 v_stroke;
flat in vec2 v_radii;
flat in uint v_id;
in vec2 v_offset;
out vec4 color;

void main() {
  float d = length(v_offset);
  float w = max(fwidth(d), 1e-3);
  float outside = smoothstep(v_radii.y - w * 0.5, v_radii.y + w * 0.5, d);
  if (outside >= 1.0) discard;
  if (u_pick) {
    color = vec4(float(v_id & 255u), float((v_id >> 8) & 255u), float((v_id >> 16) & 255u), u_layer) / 255.0;
    return;
  }
  // Fill inside the radius, stroke beyond (none if the stroke is thin to nothing)
  float stroked = v_radii.y > v_radii.x ? smoothstep(v_radii.x - w * 0.5, v_radii.x + w * 0.5, d) : 0.0;
  vec4 c = mix(v_fill, v_stroke, stroked);
  color = vec4(c.rgb, c.a * (1.0 - outside));
}";

const UNIFORMS: [&str; 9] = [
    "u_matrix",
    "u_plane_origin",
    "u_origin",
    "u_viewport",
    "u_depth_bias",
    "u_on_ground",
    "u_layer",
    "u_pick",
    "u_slop",
];

/// Circles in relief are pulled towards the camera by this fraction of their distance
const DEPTH_BIAS: f64 = 0.003;
/// Extra radius in the picking pass, device pixels
const PICK_SLOP: f32 = 4.0;
/// Bytes per point: offset (2 × f32), feature
const STRIDE: i32 = 12;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Point {
    offset: [f32; 2],
    feature: u32,
}

pub struct CircleLayer {
    pub features: Features,
    /// Lying on the ground rather than facing the screen
    pub on_ground: bool,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 9],
    projection: Projection,
    drape_uniforms: DrapeUniforms,
    feature_uniforms: FeatureUniforms,
    vao: glow::VertexArray,
    buffer: glow::Buffer,
    origin: DVec2,
    min: DVec2,
    max: DVec2,
    drape: Drape,
    count: usize,
}

impl CircleLayer {
    pub fn new(gl: &glow::Context, points: &[f64], feature: &[u32], feature_count: usize) -> Result<Self, String> {
        let program = gl::program(
            gl,
            &format!("#version 300 es\n{}{}{FEATURES}{VERTEX}", gl::PROJECT, drape::ALTITUDE),
            &format!("#version 300 es\nprecision highp float;\n{FRAGMENT}"),
        )?;
        // SAFETY: object creation on a live context
        let (vao, buffer) = unsafe {
            let vao = gl.create_vertex_array()?;
            let buffer = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, STRIDE, 0);
            gl.vertex_attrib_divisor(0, 1);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_i32(1, 1, glow::UNSIGNED_INT, STRIDE, 8);
            gl.vertex_attrib_divisor(1, 1);
            gl.bind_vertex_array(None);
            (vao, buffer)
        };
        let mut layer = Self {
            features: Features::new(gl, feature_count)?,
            on_ground: false,
            program,
            uniforms: gl::uniforms(gl, program, UNIFORMS),
            projection: Projection::new(gl, program),
            drape_uniforms: DrapeUniforms::new(gl, program),
            feature_uniforms: FeatureUniforms::new(gl, program),
            vao,
            buffer,
            origin: DVec2::ZERO,
            min: DVec2::ZERO,
            max: DVec2::ZERO,
            drape: Drape::new(),
            count: 0,
        };
        layer.set_geometry(gl, points, feature);
        Ok(layer)
    }

    /// Replaces the points (Mercator, interleaved) and their features
    pub fn set_geometry(&mut self, gl: &glow::Context, points: &[f64], feature: &[u32]) {
        let merc: Vec<DVec2> = points
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[x, y]| DVec2::new(x, y))
            .collect();
        let n = merc.len().min(feature.len());
        let (min, max) = merc[..n]
            .iter()
            .fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), &p| {
                (a.min(p), b.max(p))
            });
        (self.min, self.max) = if n == 0 { (DVec2::ZERO, DVec2::ZERO) } else { (min, max) };
        self.origin = (self.min + self.max) / 2.0;
        let data: Vec<Point> = merc[..n]
            .iter()
            .zip(feature)
            .map(|(&p, &feature)| {
                let o = p - self.origin;
                Point {
                    offset: [o.x as f32, o.y as f32],
                    feature,
                }
            })
            .collect();
        // SAFETY: upload into the buffer owned by this layer
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&data), glow::STATIC_DRAW);
        }
        self.count = n;
        self.drape.delete(gl);
        self.drape = Drape::new();
    }

    fn update_drape(&mut self, frame: &Frame) {
        if let Some((source, _)) = frame.relief {
            // A little margin around the points: the page table covers at least a cell
            let pad = DVec2::splat(1e-6);
            self.drape.update(frame.gl, self.min - pad, self.max + pad, source);
        }
    }

    fn render(&self, frame: &Frame, pick: Option<u8>) {
        let &Frame {
            gl,
            camera,
            view,
            pixel_ratio,
            relief,
            ..
        } = frame;
        let [
            matrix,
            plane_origin,
            origin,
            viewport,
            depth_bias,
            on_ground,
            layer,
            pick_u,
            slop,
        ] = &self.uniforms;
        let shift = -(self.origin.x - camera.center.x).round();
        let plane = (DVec2::new(self.origin.x + shift, self.origin.y) - camera.center) * view.world_size;
        // SAFETY: draw with objects owned by this layer
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_2_f32(plane_origin.as_ref(), plane.x as f32, plane.y as f32);
            gl.uniform_2_f32(origin.as_ref(), self.origin.x as f32, self.origin.y as f32);
            gl.uniform_2_f32(
                viewport.as_ref(),
                (camera.width * pixel_ratio / 2.0) as f32,
                (camera.height * pixel_ratio / 2.0) as f32,
            );
            let bias = if relief.is_some() {
                2.0 * view.near * DEPTH_BIAS
            } else {
                0.0
            };
            gl.uniform_1_f32(depth_bias.as_ref(), bias as f32);
            gl.uniform_1_f32(on_ground.as_ref(), if self.on_ground { 1.0 } else { 0.0 });
            gl.uniform_1_f32(layer.as_ref(), f32::from(pick.unwrap_or(0)));
            gl.uniform_1_i32(pick_u.as_ref(), i32::from(pick.is_some()));
            gl.uniform_1_f32(slop.as_ref(), if pick.is_some() { PICK_SLOP } else { 0.0 });
            self.projection.set(gl, view);
            self.drape.bind(gl, &self.drape_uniforms, relief);
            self.features.bind(gl, &self.feature_uniforms, camera.zoom, pixel_ratio);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, self.count as i32);
            gl.bind_vertex_array(None);
        }
    }

    fn shows(&self, frame: &Frame) -> bool {
        self.count > 0 && self.features.layer.shows(frame.camera.zoom)
    }
}

impl Layer for CircleLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        if !self.shows(frame) {
            return false;
        }
        self.update_drape(frame);
        let gl = frame.gl;
        // SAFETY: state changes on the live context, set back below
        unsafe {
            gl.disable(glow::CULL_FACE);
            if frame.relief.is_some() {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LEQUAL);
                gl.depth_mask(false);
            }
        }
        self.render(frame, None);
        // SAFETY: state changes on the live context
        unsafe {
            gl.enable(glow::CULL_FACE);
            gl.disable(glow::DEPTH_TEST);
            gl.depth_mask(true);
        }
        false
    }

    fn pickable(&self) -> bool {
        true
    }

    fn draw_pick(&self, frame: &Frame, layer: u8) {
        if !self.shows(frame) {
            return;
        }
        // The page table is the last frame's
        self.render(frame, Some(layer));
    }

    fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.buffer);
        }
        self.features.delete(gl);
        self.drape.delete(gl);
    }
}

#[wasm_bindgen]
impl Engine {
    /// Circle layer of `feature_count` features: points (Mercator [0, 1], interleaved) and the feature of each
    #[wasm_bindgen(js_name = addCircles)]
    pub fn add_circles(&mut self, points: &[f64], feature: &[u32], feature_count: usize) -> Result<u32, JsValue> {
        let layer = CircleLayer::new(&self.gl, points, feature, feature_count)?;
        Ok(self.add_layer(Box::new(layer)))
    }

    /// New points for a circle layer (same features)
    #[wasm_bindgen(js_name = setCircles)]
    pub fn set_circles(&mut self, id: u32, points: &[f64], feature: &[u32]) {
        if let Some((layer, gl)) = self.layer_mut::<CircleLayer>(id) {
            layer.set_geometry(gl, points, feature);
        }
    }

    /// Circles lying on the ground (true) or facing the screen
    #[wasm_bindgen(js_name = setCircleAlignment)]
    pub fn set_circle_alignment(&mut self, id: u32, on_ground: bool) {
        if let Some((layer, _)) = self.layer_mut::<CircleLayer>(id) {
            layer.on_ground = on_ground;
        }
    }
}
