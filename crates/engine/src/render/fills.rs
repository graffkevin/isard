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
//! The triangles are cut along a grid fine enough for the layer (`geo::shapes::square_for`) and sorted by square,
//! their order kept within a square (triangles of different squares never overlap): only the squares in view are
//! drawn, flat or in relief. On flat ground only the top cap is built and uploaded; the prism is, from the polygons
//! the layer keeps, on its first frame in relief.
//!
//! Picking is done on the CPU: the ground position under the cursor tested against the polygons.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::features::{FeatureUniforms, Features, Layout};
use crate::render::gl::{self, Objects, Projection, bytes};
use crate::render::overlay;
use geo::pieces::{self, Grid, Visible};
use geo::shapes::{self, Mesh, OwnedPolygons, Polygons};
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

/// Prisms from this far below the body's lowest ground (metres): the chords of the globe sagging under the sphere
/// (-3250 m on the Earth)
const BOTTOM_MARGIN: f64 = 2800.0;
/// …to this high above its highest, times the exaggeration (9400 m on the Earth, above Everest)
const TOP_MARGIN: f64 = 500.0;

/// Altitudes the prisms reach from and to (metres, before exaggeration)
fn prism_range(body: &geo::geodesy::Body) -> (f64, f64) {
    (body.relief.0 - BOTTOM_MARGIN, body.relief.1 + TOP_MARGIN)
}

/// Whether a point lies inside a polygon's rings, even-odd (the holes cut out)
fn contains<'a>(rings: impl Iterator<Item = &'a [DVec2]>, p: DVec2) -> bool {
    let mut inside = false;
    for ring in rings {
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

/// A polygon as the picking tests it: its feature, its bounds (empty without a point), its rings (`polygon_rings`)
struct Pick {
    feature: u32,
    min: DVec2,
    max: DVec2,
    rings: std::ops::Range<usize>,
}

/// The polygons as the picking tests them
fn picks_of(polygons: &Polygons, feature: &[u32]) -> Vec<Pick> {
    shapes::polygon_rings(polygons)
        .enumerate()
        .map(|(p, rings)| {
            let (min, max) = polygons
                .rings(rings.clone())
                .flatten()
                .fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), &q| {
                    (a.min(q), b.max(q))
                });
            Pick {
                feature: feature.get(p).copied().unwrap_or(u32::MAX),
                min,
                max,
                rings,
            }
        })
        .collect()
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
    /// Index count of the top cap
    all: i32,
    /// The polygons: tested by the picking, and the prism built from them in relief
    polygons: OwnedPolygons,
    picks: Vec<Pick>,
    /// The side of the squares the triangles are cut along (`shapes::square_for`)
    square: f64,
    /// The prism (the bottom cap and the walls) is uploaded with the top cap: since the first frame in relief
    prism: bool,
    /// The grid the triangles are sorted by, and the first index of each square in the top cap and in the rest (the
    /// bottom cap and the walls, empty without the prism), with the ends
    grid: Grid,
    top_start: Vec<u32>,
    rest_start: Vec<u32>,
    /// The squares in view and their ranges of the top cap, and of the rest, kept from frame to frame
    visible: Visible,
    rest: Vec<(u32, u32)>,
}

impl FillLayer {
    pub fn new(gl: &glow::Context, polygons: &Polygons, feature: &[u32], feature_count: usize) -> Result<Self, String> {
        // Everything created is deleted again if a step fails
        let mut objects = Objects::new(gl);
        let program = objects.program(
            &gl::vertex_shader(&[gl::PROJECT, &Layout::FILL.glsl(), VERTEX]),
            &gl::fragment_shader(&[FRAGMENT]),
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
        let features = Features::new(gl, feature_count, Layout::FILL)?;
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
            all: 0,
            polygons: OwnedPolygons::default(),
            picks: Vec::new(),
            square: shapes::SQUARE,
            prism: false,
            grid: Grid::over(DVec2::ZERO, DVec2::ZERO, 1.0),
            top_start: vec![0, 0],
            rest_start: vec![0, 0],
            visible: Visible::default(),
            rest: Vec::new(),
        };
        layer.set_geometry(gl, polygons, feature);
        Ok(layer)
    }

    /// Replaces the polygons; `feature`: the feature of each polygon
    pub fn set_geometry(&mut self, gl: &glow::Context, polygons: &Polygons, feature: &[u32]) {
        let (min, max) = overlay::bounds(polygons.points).unwrap_or((DVec2::ZERO, DVec2::ZERO));
        self.square = shapes::square_for(min, max);
        self.polygons = OwnedPolygons::new(polygons);
        self.picks = picks_of(polygons, feature);
        self.upload(gl);
    }

    /// Builds the mesh of the polygons (with the prism once the layer was drawn in relief) and uploads it
    fn upload(&mut self, gl: &glow::Context) {
        let square = self.square;
        let mut mesh: Mesh = shapes::triangulate_with(&self.polygons.as_polygons(), square, self.prism);
        let chunks = shapes::chunk(&mut mesh, square);
        // Polygon numbers to features
        for v in &mut mesh.vertices {
            v.feature = self.picks.get(v.feature as usize).map_or(u32::MAX, |p| p.feature);
        }
        self.grid = chunks.grid;
        self.top_start = chunks.top;
        self.rest_start = chunks.rest;
        self.visible.invalidate();
        // SAFETY: uploads into buffers owned by this layer
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vertices));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&mesh.vertices), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.indices));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes(&mesh.indices), glow::STATIC_DRAW);
            gl.bind_vertex_array(None);
        }
        self.all = mesh.top as i32;
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
        let (bottom, top) = prism_range(&view.body);
        // SAFETY: uniforms on the program in use
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_2_f32(center_hi.as_ref(), xh, yh);
            gl.uniform_2_f32(center_lo.as_ref(), xl, yl);
            gl.uniform_2_f32(
                heights.as_ref(),
                bottom as f32 * exaggeration.max(1.0),
                top as f32 * exaggeration.max(1.0),
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
        if volume && !self.prism {
            self.prism = true;
            self.upload(frame.gl);
        }
        // The squares in view (the prisms of a volume reach through `prism_range`, and their faces past the far plane
        // still count: kept on it); the fills are drawn where they are, on no other copy of the world
        let heights = frame.relief.map_or((0.0, 0.0), |(_, e)| {
            let k = frame.view.vertical * f64::from(e.max(1.0));
            let (bottom, top) = prism_range(&frame.view.body);
            (bottom * k, top * k)
        });
        let mut visible = std::mem::take(&mut self.visible);
        let mut rest = std::mem::take(&mut self.rest);
        visible.update(&self.grid, &self.top_start, frame.view, (heights, 0.0, 0.0, !volume));
        let top = visible.ranges();
        if top.is_empty() {
            self.visible = visible;
            self.rest = rest;
            return false;
        }
        // The bottom caps and the walls too, in relief
        if volume {
            pieces::ranges_into(visible.squares(), &self.rest_start, &mut rest);
        } else {
            rest.clear();
        }
        let draw = |ranges: &[&[(u32, u32)]]| {
            for &(first, end) in ranges.iter().copied().flatten() {
                // SAFETY: a draw of indices inside the index buffer, the vertex array bound by `set_uniforms`
                unsafe {
                    frame.gl.draw_elements(
                        glow::TRIANGLES,
                        (end - first) as i32,
                        glow::UNSIGNED_INT,
                        first as i32 * 4,
                    );
                }
            }
        };
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
                draw(&[top, &rest]);
                // Color the counted ground, once per pixel
                gl.color_mask(true, true, true, true);
                gl.disable(glow::DEPTH_TEST);
                gl.stencil_func(glow::NOTEQUAL, 0, 0xff);
                gl.stencil_op(glow::ZERO, glow::ZERO, glow::ZERO);
                draw(&[top, &rest]);
                gl.disable(glow::STENCIL_TEST);
                gl.depth_mask(true);
            } else {
                draw(&[top]);
            }
            gl.bind_vertex_array(None);
            gl.enable(glow::CULL_FACE);
        }
        self.visible = visible;
        self.rest = rest;
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
        let polygons = self.polygons.as_polygons();
        let inside = |p: &Pick| m.x >= p.min.x && m.y >= p.min.y && m.x <= p.max.x && m.y <= p.max.y;
        self.picks
            .iter()
            .rev()
            .find(|p| inside(p) && contains(polygons.rings(p.rings.clone()), m))
            .map(|p| p.feature)
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
    use geo::geodesy::Body;

    #[test]
    fn prisms_reach_through_the_relief_of_the_body() {
        // The Earth's from under the Dead Sea to above Everest; Mars's from under Hellas to above Olympus Mons
        assert_eq!(prism_range(&Body::EARTH), (-3250.0, 9400.0));
        let (bottom, top) = prism_range(&Body::MARS);
        assert!(bottom < -8201.0 - 2000.0 && top > 21_241.0, "{bottom} {top}");
    }

    #[test]
    fn polygons_hold_their_inside_holes_cut_out() {
        let square = |o: f64, s: f64| {
            vec![
                DVec2::new(o, o),
                DVec2::new(o + s, o),
                DVec2::new(o + s, o + s),
                DVec2::new(o, o + s),
            ]
        };
        let rings = [square(0.0, 1.0), square(0.25, 0.5)];
        let inside = |p: DVec2| contains(rings.iter().map(Vec::as_slice), p);
        assert!(inside(DVec2::new(0.1, 0.1)));
        assert!(!inside(DVec2::new(0.5, 0.5)));
        assert!(!inside(DVec2::new(1.5, 0.5)));
    }

    #[test]
    fn malformed_polygons_keep_only_the_rings_there_are() {
        let points = [DVec2::ZERO, DVec2::X, DVec2::ONE, DVec2::Y];
        // Polygon 0 claims every ring there could be, polygon 1 starts past the rings, polygons 2 and 3 run backwards
        // or start inside polygon 0: no ring is shared
        let polygons = Polygons {
            points: &points,
            ring_start: &[0, 4, u32::MAX],
            polygon_start: &[0, u32::MAX, 1, 0, u32::MAX],
        };
        let picks = picks_of(&polygons, &[3]);
        assert_eq!(picks.len(), 4);
        assert_eq!(picks[0].rings, 0..2);
        let rings: Vec<&[DVec2]> = polygons.rings(0..2).collect();
        assert!(rings[1].is_empty(), "the ring past the points is empty");
        assert!(contains(rings.into_iter(), DVec2::splat(0.5)));
        assert_eq!((picks[0].feature, picks[1].feature), (3, u32::MAX));
        for pick in &picks[1..] {
            assert!(pick.rings.is_empty() && !pick.min.is_finite());
        }
        let empty = Polygons {
            points: &[],
            ring_start: &[],
            polygon_start: &[],
        };
        assert!(picks_of(&empty, &[]).is_empty());
    }
}
