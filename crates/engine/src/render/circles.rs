//! Circle layer: points drawn as discs with a stroke, styled per feature (fill and stroke colors, radius and stroke
//! width as zoom stops, a value through the layer's ramp): gauges, stations, marks. Each point is an instanced quad,
//! the disc worked out per pixel. Facing the screen (`MapLibre`'s `circle-pitch-alignment: viewport`) or lying on the
//! ground (`map`); in relief, standing on the ground, the hills hiding them; on the globe, hidden past its horizon.
//!
//! The points are sorted by square of a grid over the layer (as the pieces of the line layers), in their order within a
//! square: only the squares in view are drawn. Circles of different squares overlapping may stack in another order
//! than the points'.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::drape::{self, Drape, DrapeUniforms};
use crate::render::features::{FEATURES, FeatureUniforms, Features};
use crate::render::gl::{self, Objects, Projection, bytes};
use crate::render::overlay::{self, ANCHOR, Anchor};
use geo::pieces::{self, Grid, Visible};
use glam::DVec2;
use glow::HasContext;
use wasm_bindgen::prelude::*;

const VERTEX: &str = r"
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
  // Past the horizon of the globe, hidden by the body
  if (behind_globe(project(plane_of(a_point), merc))) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
  float h = altitude(merc);
  vec4 c;
  if (u_on_ground > 0.5) {
    // Pixels at the camera's scale, laid on the ground around the point
    vec2 plane = plane_of(a_point) + corner * extent / u_pixel_ratio;
    c = u_matrix * vec4(project_up(plane, merc, h), 1.0);
  } else {
    c = u_matrix * vec4(project_up(plane_of(a_point), merc, h), 1.0);
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

const UNIFORMS: [&str; 4] = ["u_on_ground", "u_layer", "u_pick", "u_slop"];

/// Circles in relief are pulled towards the camera by this fraction of their distance
const DEPTH_BIAS: f64 = 0.003;
/// Extra radius in the picking pass, device pixels
const PICK_SLOP: f32 = 4.0;
/// Margin around the points when culling the layer, CSS pixels: the circles reach that far out of their centers
const CULL_MARGIN: f64 = 128.0;
/// Bytes per point: offset (2 × f32), feature
const STRIDE: i32 = 12;
/// Squares of the culling grid along the longer side of the layer
const SQUARES_ACROSS: f64 = 32.0;
/// Smallest square of the culling grid, Mercator (about 100 m at the equator)
const MIN_SQUARE: f64 = 100.0 / 40_075_016.686;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Point {
    offset: [f32; 2],
    feature: u32,
}

/// Points sorted by square of their culling grid, offsets from the middle of their bounds
struct Sorted {
    min: DVec2,
    max: DVec2,
    grid: Grid,
    start: Vec<u32>,
    data: Vec<Point>,
}

/// Points (Mercator, interleaved) and their features sorted by square of a grid over them, in their order within a
/// square; the points that are not numbers left out (one at infinity would stretch the grid to nothing)
fn sort(points: &[f64], feature: &[u32]) -> Sorted {
    let (merc, feature): (Vec<DVec2>, Vec<u32>) = overlay::points(points)
        .into_iter()
        .zip(feature.iter().copied())
        .filter(|(p, _)| p.is_finite())
        .unzip();
    let (min, max) = overlay::bounds(&merc).unwrap_or((DVec2::ZERO, DVec2::ZERO));
    let origin = (min + max) / 2.0;
    let extent = (max - min).max_element();
    let grid = Grid::over(min, max, (extent / SQUARES_ACROSS).max(MIN_SQUARE));
    let squares: Vec<u32> = merc.iter().map(|&p| grid.square(p)).collect();
    let (order, start) = pieces::sort_by_square(&squares, grid.len());
    let data = order
        .iter()
        .map(|&k| {
            let o = merc[k as usize] - origin;
            Point {
                offset: [o.x as f32, o.y as f32],
                feature: feature[k as usize],
            }
        })
        .collect();
    Sorted {
        min,
        max,
        grid,
        start,
        data,
    }
}

pub struct CircleLayer {
    pub features: Features,
    /// Lying on the ground rather than facing the screen
    pub on_ground: bool,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 4],
    anchor: Anchor,
    projection: Projection,
    drape_uniforms: DrapeUniforms,
    feature_uniforms: FeatureUniforms,
    vao: glow::VertexArray,
    buffer: glow::Buffer,
    origin: DVec2,
    min: DVec2,
    max: DVec2,
    /// The culling grid and the first point of each of its squares (and the end)
    grid: Grid,
    start: Vec<u32>,
    /// The squares in view and their points, kept from frame to frame
    visible: Visible,
    drape: Drape,
    count: usize,
}

impl CircleLayer {
    pub fn new(gl: &glow::Context, points: &[f64], feature: &[u32], feature_count: usize) -> Result<Self, String> {
        overlay::pick_ids_fit(feature_count, "circle layer");
        // Everything created is deleted again if a step fails
        let mut objects = Objects::new(gl);
        let program = objects.program(
            &format!(
                "#version 300 es\n{}{}{ANCHOR}{FEATURES}{VERTEX}",
                gl::PROJECT,
                drape::ALTITUDE
            ),
            &format!("#version 300 es\nprecision highp float;\n{FRAGMENT}"),
        )?;
        let vao = objects.vertex_array()?;
        let buffer = objects.buffer()?;
        // SAFETY: state setup of objects just created on a live context
        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, STRIDE, 0);
            gl.vertex_attrib_divisor(0, 1);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_i32(1, 1, glow::UNSIGNED_INT, STRIDE, 8);
            gl.vertex_attrib_divisor(1, 1);
            gl.bind_vertex_array(None);
        }
        let features = Features::new(gl, feature_count)?;
        objects.keep();
        let mut layer = Self {
            features,
            on_ground: false,
            program,
            uniforms: gl::uniforms(gl, program, UNIFORMS),
            anchor: Anchor::new(gl, program),
            projection: Projection::new(gl, program),
            drape_uniforms: DrapeUniforms::new(gl, program),
            feature_uniforms: FeatureUniforms::new(gl, program),
            vao,
            buffer,
            origin: DVec2::ZERO,
            min: DVec2::ZERO,
            max: DVec2::ZERO,
            grid: Grid::over(DVec2::ZERO, DVec2::ZERO, 1.0),
            start: vec![0, 0],
            visible: Visible::default(),
            drape: Drape::new(),
            count: 0,
        };
        layer.set_geometry(gl, points, feature);
        Ok(layer)
    }

    /// Replaces the points (Mercator, interleaved) and their features
    pub fn set_geometry(&mut self, gl: &glow::Context, points: &[f64], feature: &[u32]) {
        let Sorted {
            min,
            max,
            grid,
            start,
            data,
        } = sort(points, feature);
        (self.min, self.max, self.grid) = (min, max, grid);
        self.origin = (min + max) / 2.0;
        self.start = start;
        self.visible.invalidate();
        // SAFETY: upload into the buffer owned by this layer
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&data), glow::STATIC_DRAW);
        }
        self.count = data.len();
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

    /// Draws the points of `ranges`; `pick`: the picking pass, with this layer number
    fn render(&self, frame: &Frame, ranges: &[(u32, u32)], pick: Option<u8>) {
        let &Frame {
            gl,
            camera,
            view,
            pixel_ratio,
            relief,
            ..
        } = frame;
        let [on_ground, layer, pick_u, slop] = &self.uniforms;
        // SAFETY: draw with objects owned by this layer
        unsafe {
            gl.use_program(Some(self.program));
            let viewport = (camera.width * pixel_ratio, camera.height * pixel_ratio);
            self.anchor.set_view(gl, view, viewport, relief.is_some(), DEPTH_BIAS);
            self.anchor.set_origin(gl, view, self.origin);
            gl.uniform_1_f32(on_ground.as_ref(), if self.on_ground { 1.0 } else { 0.0 });
            gl.uniform_1_f32(layer.as_ref(), f32::from(pick.unwrap_or(0)));
            gl.uniform_1_i32(pick_u.as_ref(), i32::from(pick.is_some()));
            gl.uniform_1_f32(slop.as_ref(), if pick.is_some() { PICK_SLOP } else { 0.0 });
            self.projection.set(gl, view);
            self.drape.bind(gl, &self.drape_uniforms, relief);
            self.features.bind(gl, &self.feature_uniforms, camera.zoom, pixel_ratio);
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            for &(first, end) in ranges {
                let offset = first as i32 * STRIDE;
                gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, STRIDE, offset);
                gl.vertex_attrib_pointer_i32(1, 1, glow::UNSIGNED_INT, STRIDE, offset + 8);
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, (end - first) as i32);
            }
            gl.bind_vertex_array(None);
        }
    }

    /// Whether the layer has anything to draw at this zoom
    fn shows(&self, frame: &Frame) -> bool {
        self.count > 0 && self.features.layer.shows(frame.camera.zoom)
    }

    /// How the layer looks for its squares in view: the circles reach `CULL_MARGIN` out of their points
    fn look(&self, frame: &Frame) -> pieces::Look {
        overlay::look(
            self.origin,
            frame.view,
            frame.relief,
            CULL_MARGIN / frame.view.world_size,
        )
    }
}

impl Layer for CircleLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        if !self.shows(frame) {
            return false;
        }
        let look = self.look(frame);
        let mut visible = std::mem::take(&mut self.visible);
        let ranges = visible.update(&self.grid, &self.start, frame.view, look);
        if ranges.is_empty() {
            self.visible = visible;
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
        self.render(frame, ranges, None);
        self.visible = visible;
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
        let ranges = overlay::pick_ranges(&self.visible, &self.grid, &self.start, frame.view, self.look(frame));
        if ranges.is_empty() {
            return;
        }
        // The page table is the last frame's
        self.render(frame, &ranges, Some(layer));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_not_numbers_are_left_out() {
        let points = [
            0.5,
            0.5,
            f64::INFINITY,
            0.5,
            0.25,
            f64::NAN,
            0.5001,
            0.5002,
            -f64::INFINITY,
            1e308,
        ];
        let sorted = sort(&points, &[0, 1, 2, 3, 4]);
        assert_eq!(sorted.data.len(), 2);
        assert!(sorted.grid.len() <= 33 * 33, "{}", sorted.grid.len());
        assert_eq!(*sorted.start.last().unwrap(), 2);
        let mut features: Vec<u32> = sorted.data.iter().map(|p| p.feature).collect();
        features.sort_unstable();
        assert_eq!(features, [0, 3]);
        assert!(sorted.max.x - sorted.min.x < 1e-3);
        // Fewer features than points: the points without one are left out; none at all
        assert_eq!(sort(&points, &[0]).data.len(), 1);
        let none = sort(&[], &[]);
        assert!(none.data.is_empty() && none.grid.len() == 1 && none.start == [0, 0]);
    }
}
