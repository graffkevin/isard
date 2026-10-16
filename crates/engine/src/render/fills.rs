//! Fill layer: polygons (holes included) filled per feature (color, or a value through the layer's ramp), with a layer
//! opacity: basins, veils, flood areas, a mask with a hole over the rest of the world.
//!
//! On flat ground the triangles (`geo::shapes`) are drawn as they are. Over the relief a fill must lie on the ground,
//! hills and valleys alike, at any size: it is drawn as a shadow volume. Each polygon becomes a prism from below the
//! lowest ground to above the highest; the faces of the prisms that lie behind the ground are counted in the stencil
//! (z-fail: back faces add, front faces subtract), which leaves a count on exactly the ground inside a prism; the
//! prisms are then drawn once more, coloring the counted pixels and clearing the count. The cost is a few full-screen
//! passes at worst, whatever the relief, and the edges are exact to the pixel.
//!
//! Picking is done on the CPU: the ground position under the cursor tested against the polygons.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::features::{FEATURES, FeatureUniforms, Features};
use crate::render::gl::{self, Objects, Projection, bytes};
use crate::render::overlay;
use geo::shapes::{self, Mesh, Polygons};
use glam::DVec2;
use glow::HasContext;
use wasm_bindgen::prelude::*;

const VERTEX: &str = r"
uniform mat4 u_matrix;
// Camera center, Mercator, in two parts (as the positions)
uniform vec2 u_center_hi;
uniform vec2 u_center_lo;
// Bottom and top of the prisms (metres); 0, 0: flat
uniform vec2 u_heights;
uniform bool u_volume;
layout(location = 0) in vec2 a_hi;
layout(location = 1) in vec2 a_lo;
layout(location = 2) in uint a_feature;
layout(location = 3) in float a_level;
flat out vec4 v_color;

void main() {
  uint f = a_feature;
  v_color = feature_color(f, 0u);
  v_color.a *= layer_opacity();
  if (!feature_visible(f)) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
  // The offset from the camera center, part by part: exact even for a polygon as large as the world
  vec2 d = (a_hi - u_center_hi) + (a_lo - u_center_lo);
  vec2 merc = a_hi + a_lo;
  float h = u_volume ? mix(u_heights.x, u_heights.y, a_level) : 0.0;
  vec4 c = u_matrix * vec4(project_up(d * u_world, merc, h), 1.0);
  // The prisms reach past the far plane: kept on it rather than clipped (depth clamp), so that their far faces
  // still count behind the ground
  if (u_volume && c.w > 0.0) c.z = min(c.z, c.w * 0.999999);
  gl_Position = c;
}";

const FRAGMENT: &str = r"
flat in vec4 v_color;
out vec4 color;
void main() { color = v_color; }";

const UNIFORMS: [&str; 5] = ["u_matrix", "u_center_hi", "u_center_lo", "u_heights", "u_volume"];

/// Prisms from this far below the sea (metres): deep valleys, and the chords of the globe sagging under the sphere
const BOTTOM: f32 = -3000.0;
/// …to this high above it, times the exaggeration (above Mont Blanc)
const TOP: f32 = 5500.0;

/// One polygon kept on the CPU for picking: its rings and bounds
struct Shape {
    feature: u32,
    rings: Vec<Vec<DVec2>>,
    min: DVec2,
    max: DVec2,
}

impl Shape {
    /// Even-odd rule over all rings (the holes cut out)
    fn contains(&self, p: DVec2) -> bool {
        if p.x < self.min.x || p.y < self.min.y || p.x > self.max.x || p.y > self.max.y {
            return false;
        }
        let mut inside = false;
        for ring in &self.rings {
            let n = ring.len();
            for i in 0..n {
                let (a, b) = (ring[i], ring[(i + 1) % n]);
                if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                    inside = !inside;
                }
            }
        }
        inside
    }
}

pub struct FillLayer {
    pub features: Features,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 5],
    projection: Projection,
    feature_uniforms: FeatureUniforms,
    vao: glow::VertexArray,
    vertices: glow::Buffer,
    indices: glow::Buffer,
    /// Index counts: top cap, all
    top: i32,
    all: i32,
    shapes: Vec<Shape>,
    /// Bounds of the polygons (Mercator), to skip the layer out of view
    min: DVec2,
    max: DVec2,
}

impl FillLayer {
    pub fn new(gl: &glow::Context, polygons: &Polygons, feature: &[u32], feature_count: usize) -> Result<Self, String> {
        // Everything created is deleted again if a step fails
        let mut objects = Objects::new(gl);
        let program = objects.program(
            &format!("#version 300 es\n{}{FEATURES}{VERTEX}", gl::PROJECT),
            &format!("#version 300 es\nprecision highp float;\n{FRAGMENT}"),
        )?;
        let vao = objects.vertex_array()?;
        let vertices = objects.buffer()?;
        let indices = objects.buffer()?;
        // SAFETY: state setup of objects just created on a live context
        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertices));
            let stride = size_of::<shapes::Vertex>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_i32(2, 1, glow::UNSIGNED_INT, stride, 16);
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_f32(3, 1, glow::FLOAT, false, stride, 20);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(indices));
            gl.bind_vertex_array(None);
        }
        let features = Features::new(gl, feature_count)?;
        objects.keep();
        let mut layer = Self {
            features,
            program,
            uniforms: gl::uniforms(gl, program, UNIFORMS),
            projection: Projection::new(gl, program),
            feature_uniforms: FeatureUniforms::new(gl, program),
            vao,
            vertices,
            indices,
            top: 0,
            all: 0,
            shapes: Vec::new(),
            min: DVec2::ZERO,
            max: DVec2::ZERO,
        };
        layer.set_geometry(gl, polygons, feature);
        Ok(layer)
    }

    /// Replaces the polygons; `feature`: the feature of each polygon
    pub fn set_geometry(&mut self, gl: &glow::Context, polygons: &Polygons, feature: &[u32]) {
        let mut mesh: Mesh = shapes::triangulate(polygons);
        // Polygon numbers to features
        for v in &mut mesh.vertices {
            v.polygon = feature.get(v.polygon as usize).copied().unwrap_or(u32::MAX);
        }
        self.shapes = (0..polygons.polygon_start.len().saturating_sub(1))
            .map(|p| {
                let rings: Vec<Vec<DVec2>> = (polygons.polygon_start[p]..polygons.polygon_start[p + 1])
                    .filter_map(|r| {
                        let (s, e) = (
                            *polygons.ring_start.get(r as usize)? as usize,
                            *polygons.ring_start.get(r as usize + 1)? as usize,
                        );
                        polygons.points.get(s..e).map(<[DVec2]>::to_vec)
                    })
                    .collect();
                let (min, max) = rings
                    .iter()
                    .flatten()
                    .fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), &q| {
                        (a.min(q), b.max(q))
                    });
                Shape {
                    feature: feature.get(p).copied().unwrap_or(u32::MAX),
                    rings,
                    min,
                    max,
                }
            })
            .collect();
        (self.min, self.max) = overlay::bounds(polygons.points).unwrap_or((DVec2::ZERO, DVec2::ZERO));
        // SAFETY: uploads into buffers owned by this layer
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vertices));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&mesh.vertices), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.indices));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes(&mesh.indices), glow::STATIC_DRAW);
            gl.bind_vertex_array(None);
        }
        self.top = mesh.top as i32;
        self.all = mesh.indices.len() as i32;
    }

    fn set_uniforms(&self, frame: &Frame, volume: bool) {
        let &Frame {
            gl,
            camera,
            view,
            pixel_ratio,
            relief,
            ..
        } = frame;
        let [matrix, center_hi, center_lo, heights, volume_u] = &self.uniforms;
        let ((xh, xl), (yh, yl)) = (overlay::split(camera.center.x), overlay::split(camera.center.y));
        let exaggeration = relief.map_or(1.0, |(_, e)| e);
        // SAFETY: uniforms on the program in use
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_2_f32(center_hi.as_ref(), xh, yh);
            gl.uniform_2_f32(center_lo.as_ref(), xl, yl);
            gl.uniform_2_f32(
                heights.as_ref(),
                BOTTOM * exaggeration.max(1.0),
                TOP * exaggeration.max(1.0),
            );
            gl.uniform_1_i32(volume_u.as_ref(), i32::from(volume));
            self.projection.set(gl, view);
            self.features.bind(gl, &self.feature_uniforms, camera.zoom, pixel_ratio);
            gl.bind_vertex_array(Some(self.vao));
        }
    }
}

impl Layer for FillLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        if self.all == 0 || !self.features.layer.shows(frame.camera.zoom) {
            return false;
        }
        let volume = frame.relief.is_some();
        // Out of view: nothing to draw (the prisms of a volume reach from `BOTTOM` to `TOP`)
        let heights = frame.relief.map_or((0.0, 0.0), |(_, e)| {
            let k = frame.view.vertical * f64::from(e.max(1.0));
            (f64::from(BOTTOM) * k, f64::from(TOP) * k)
        });
        if !overlay::in_view(frame.view, self.min, self.max, heights) {
            return false;
        }
        let gl = frame.gl;
        self.set_uniforms(frame, volume);
        // SAFETY: state changes and draws on the live context, set back at the end
        unsafe {
            gl.disable(glow::CULL_FACE);
            if volume {
                // Count the faces behind the ground
                gl.clear_stencil(0);
                gl.clear(glow::STENCIL_BUFFER_BIT);
                gl.enable(glow::STENCIL_TEST);
                gl.stencil_func(glow::ALWAYS, 0, 0xff);
                gl.stencil_op_separate(glow::BACK, glow::KEEP, glow::INCR_WRAP, glow::KEEP);
                gl.stencil_op_separate(glow::FRONT, glow::KEEP, glow::DECR_WRAP, glow::KEEP);
                gl.color_mask(false, false, false, false);
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LESS);
                gl.depth_mask(false);
                gl.draw_elements(glow::TRIANGLES, self.all, glow::UNSIGNED_INT, 0);
                // Color the counted ground, once per pixel
                gl.color_mask(true, true, true, true);
                gl.disable(glow::DEPTH_TEST);
                gl.stencil_func(glow::NOTEQUAL, 0, 0xff);
                gl.stencil_op(glow::ZERO, glow::ZERO, glow::ZERO);
                gl.draw_elements(glow::TRIANGLES, self.all, glow::UNSIGNED_INT, 0);
                gl.disable(glow::STENCIL_TEST);
                gl.depth_mask(true);
            } else {
                gl.draw_elements(glow::TRIANGLES, self.top, glow::UNSIGNED_INT, 0);
            }
            gl.bind_vertex_array(None);
            gl.enable(glow::CULL_FACE);
        }
        false
    }

    fn picks_ground(&self) -> bool {
        self.all > 0
    }

    fn pick_ground(&self, m: DVec2, zoom: f64) -> Option<u32> {
        if !self.features.layer.shows(zoom) {
            return None;
        }
        // The last polygon drawn is on top
        self.shapes.iter().rev().find(|s| s.contains(m)).map(|s| s.feature)
    }

    fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vertices);
            gl.delete_buffer(self.indices);
        }
        self.features.delete(gl);
    }
}

#[wasm_bindgen]
impl Engine {
    /// Fill layer of `feature_count` features: polygons given by their points (Mercator [0, 1], interleaved), the first
    /// point of each ring (`ring_start`, one more entry than rings), the first ring of each polygon (`polygon_start`,
    /// one more entry than polygons; the first ring is the outer one, the others holes) and the feature of each polygon
    #[wasm_bindgen(js_name = addFills)]
    pub fn add_fills(
        &mut self,
        points: &[f64],
        ring_start: &[u32],
        polygon_start: &[u32],
        feature: &[u32],
        feature_count: usize,
    ) -> Result<u32, JsValue> {
        let points = overlay::points(points);
        let polygons = Polygons {
            points: &points,
            ring_start,
            polygon_start,
        };
        let layer = FillLayer::new(&self.gl, &polygons, feature, feature_count)?;
        Ok(self.add_layer(Box::new(layer)))
    }

    /// New polygons for a fill layer (same arguments as `addFills`, the same features)
    #[wasm_bindgen(js_name = setFills)]
    pub fn set_fills(&mut self, id: u32, points: &[f64], ring_start: &[u32], polygon_start: &[u32], feature: &[u32]) {
        let points = overlay::points(points);
        if let Some((layer, gl)) = self.layer_mut::<FillLayer>(id) {
            let polygons = Polygons {
                points: &points,
                ring_start,
                polygon_start,
            };
            layer.set_geometry(gl, &polygons, feature);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_hold_their_inside_holes_cut_out() {
        let square = |o: f64, s: f64| {
            vec![
                DVec2::new(o, o),
                DVec2::new(o + s, o),
                DVec2::new(o + s, o + s),
                DVec2::new(o, o + s),
            ]
        };
        let shape = Shape {
            feature: 7,
            rings: vec![square(0.0, 1.0), square(0.25, 0.5)],
            min: DVec2::ZERO,
            max: DVec2::ONE,
        };
        assert!(shape.contains(DVec2::new(0.1, 0.1)));
        assert!(!shape.contains(DVec2::new(0.5, 0.5)));
        assert!(!shape.contains(DVec2::new(1.5, 0.5)));
    }
}
