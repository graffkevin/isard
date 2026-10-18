//! Line layer: polylines styled per feature (color, or a value through the layer's ramp; width as zoom stops), with
//! round caps and joins, dashes and a layer opacity: rivers, outlines, routes, casings (a wider line under another).
//!
//! The geometry is that of the network layer (`geo::pieces`): each polyline is an edge whose line is its points, cut
//! into pieces sorted by square, so only the squares in view are drawn, and in relief the pieces stand on the ground
//! and the hills hide them. Each piece is a quad (a strip of parts in relief) built on screen, wide enough for its
//! round caps: two pieces meeting at a point overlap in a round join. A translucent layer draws through the stencil,
//! so that the overlaps of its joins are not blended twice.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::drape::{self, Drape, DrapeUniforms};
use crate::render::features::{self, FEATURES, FeatureUniforms, Features};
use crate::render::gl::{self, Objects, Projection, bytes};
use crate::render::overlay::{self, ANCHOR, Anchor, LINE, LINE_DEPTH_BIAS, PIECE_STRIDE as STRIDE, RELIEF_SEGMENTS};
use geo::pieces::{self, Grid, Lines, Visible};
use glam::DVec2;
use glow::HasContext;
use wasm_bindgen::prelude::*;

const VERTEX: &str = r"
uniform float u_slop;
// Per line: its feature and its length (Mercator)
uniform highp sampler2D u_lines;
layout(location = 2) in uint a_line;
flat out vec4 v_color;
flat out float v_half;
flat out float v_length;
flat out uint v_id;
out float v_across;
out float v_along;
// Distance along the line, in line widths (the dashes)
out float v_dash;

void main() {
  vec2 line = texelFetch(u_lines, feature_texel(a_line), 0).xy;
  uint f = uint(line.x);
  float width = feature_size(f, 0u) * u_pixel_ratio + u_slop;
  v_color = feature_color(f, 0u);
  v_color.a *= layer_opacity();
  if (!feature_visible(f) || width <= 0.0 || v_color.a <= 0.0) { hide(); return; }
  vec4 c0 = piece_clip(0.0);
  vec4 c1 = piece_clip(1.0);
  StripVertex v;
  // Both ends pushed out by a half width: room for the round caps
  if (c0.w <= 0.0 || c1.w <= 0.0 || !strip_vertex(width, 1.0, v)) { hide(); return; }
  // Device pixels, as the strip
  float len = length(c1.xy / c1.w * u_viewport - c0.xy / c0.w * u_viewport);
  v_length = len;
  v_along = v.cap < 0.0 ? -v.half_width : v.cap > 0.0 ? len + v.half_width : v.t * len;
  v_across = v.side * v.half_width;
  v_half = width;
  v_dash = mix(a_fractions.x, a_fractions.y, v.t) * line.y * u_world * u_pixel_ratio / max(width, 1e-3);
  v_id = f + 1u;
}";

const FRAGMENT: &str = r"
uniform vec2 u_dash;
uniform float u_layer;
uniform bool u_pick;
flat in vec4 v_color;
flat in float v_half;
flat in float v_length;
flat in uint v_id;
in float v_across;
in float v_along;
in float v_dash;
out vec4 color;

void main() {
  // Distance to the piece (pixels), round beyond its ends
  float along = v_along < 0.0 ? v_along : v_along > v_length ? v_along - v_length : 0.0;
  float d = length(vec2(along, v_across));
  float alpha = clamp(v_half * 0.5 - d + 0.5, 0.0, 1.0);
  if (u_dash.x > 0.0) {
    float period = u_dash.x + u_dash.y;
    float p = mod(v_dash, period);
    float w = max(fwidth(v_dash), 1e-4);
    alpha *= smoothstep(-w, w, u_dash.x - p) * smoothstep(-w, w, p);
  }
  if (alpha <= 0.0) discard;
  if (u_pick) {
    if (alpha < 0.5) discard;
    color = vec4(float(v_id & 255u), float((v_id >> 8) & 255u), float((v_id >> 16) & 255u), u_layer) / 255.0;
    return;
  }
  color = vec4(v_color.rgb, v_color.a * alpha);
}";

/// Uniforms of the pass, by name (besides `Anchor`'s)
const UNIFORMS: [&str; 6] = ["u_segments", "u_lines", "u_dash", "u_layer", "u_pick", "u_slop"];

/// Extra width in the picking pass, device pixels
const PICK_SLOP: f32 = 8.0;
/// Texture unit of the line table
const LINES_UNIT: u32 = 1;

/// Geometry of a line layer: polylines in Mercator, each with its feature
pub struct Polylines<'a> {
    /// Interleaved x, y
    pub points: &'a [f64],
    /// Points of line `l`: `start[l] .. start[l + 1]` (counted in points)
    pub start: &'a [u32],
    /// Feature of each line
    pub feature: &'a [u32],
}

/// The pieces of polylines (`geo::pieces`), the table of their lines (feature and length of each) and their origin.
/// Each line is an edge from its first point to its last, along its points; `start` and `feature` as they come from
/// the page, whatever their lengths and values
fn build(geometry: &Polylines) -> (pieces::Pieces, Vec<f32>, DVec2) {
    let points = overlay::points(geometry.points);
    let count = geometry.start.len().saturating_sub(1).min(geometry.feature.len());
    let mut nodes = Vec::with_capacity(count * 2);
    let mut edges = Vec::with_capacity(count * 2);
    let mut table = Vec::with_capacity(count * 2);
    for l in 0..count {
        let (s, e) = (
            geometry.start[l] as usize,
            (geometry.start[l + 1] as usize).min(points.len()),
        );
        let line = &points[s.min(e)..e];
        let (a, b) = match line {
            [a, .., b] => (*a, *b),
            [a] => (*a, *a),
            [] => (DVec2::ZERO, DVec2::ZERO),
        };
        edges.extend([nodes.len() as u32, nodes.len() as u32 + 1]);
        nodes.extend([a, b]);
        let length: f64 = line.windows(2).map(|w| w[0].distance(w[1])).sum();
        table.extend([geometry.feature[l] as f32, length as f32]);
    }
    let origin = overlay::bounds(&points).map_or(DVec2::ZERO, |(min, max)| (min + max) / 2.0);
    let lines = Lines {
        // No line: no entry either (`start` may be empty)
        start: geometry.start.get(..=count).unwrap_or_default(),
        points: &points,
    };
    (pieces::build(&nodes, &edges, Some(&lines), origin), table, origin)
}

pub struct LineLayer {
    pub features: Features,
    /// Dash and gap lengths, in line widths; 0: solid
    pub dash: [f32; 2],
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 6],
    anchor: Anchor,
    projection: Projection,
    drape_uniforms: DrapeUniforms,
    feature_uniforms: FeatureUniforms,
    vao: glow::VertexArray,
    buffer: glow::Buffer,
    lines: glow::Texture,
    origin: DVec2,
    grid: Grid,
    start: Vec<u32>,
    /// The squares in view and their pieces, kept from frame to frame
    visible: Visible,
    drape: Drape,
    pieces: usize,
}

impl LineLayer {
    pub fn new(gl: &glow::Context, geometry: &Polylines, feature_count: usize) -> Result<Self, String> {
        overlay::pick_ids_fit(feature_count, "line layer");
        // Everything created is deleted again if a step fails
        let mut objects = Objects::new(gl);
        let program = objects.program(
            &format!(
                "#version 300 es\n{}{}{ANCHOR}{LINE}{FEATURES}{VERTEX}",
                gl::PROJECT,
                drape::ALTITUDE
            ),
            &format!("#version 300 es\nprecision highp float;\n{FRAGMENT}"),
        )?;
        let vao = objects.vertex_array()?;
        let buffer = objects.buffer()?;
        let lines = objects.texture()?;
        // SAFETY: state setup of objects just created on a live context
        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            for location in 0..3 {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_divisor(location, 1);
            }
            gl.bind_vertex_array(None);
        }
        let features = Features::new(gl, feature_count)?;
        objects.keep();
        let uniforms = gl::uniforms(gl, program, UNIFORMS);
        gl::sampler_units(gl, program, &[(uniforms[1].as_ref(), LINES_UNIT)]);
        let mut layer = Self {
            features,
            dash: [0.0, 0.0],
            program,
            uniforms,
            anchor: Anchor::new(gl, program),
            projection: Projection::new(gl, program),
            drape_uniforms: DrapeUniforms::new(gl, program),
            feature_uniforms: FeatureUniforms::new(gl, program),
            vao,
            buffer,
            lines,
            origin: DVec2::ZERO,
            grid: Grid {
                min: DVec2::ZERO,
                size: 1.0,
                cols: 1,
                rows: 1,
            },
            start: vec![0, 0],
            visible: Visible::default(),
            drape: Drape::new(),
            pieces: 0,
        };
        if let Err(e) = layer.set_geometry(gl, geometry) {
            layer.delete(gl);
            return Err(e);
        }
        Ok(layer)
    }

    /// Replaces the lines (the styles stay, by feature)
    pub fn set_geometry(&mut self, gl: &glow::Context, geometry: &Polylines) -> Result<(), String> {
        let (built, table, origin) = build(geometry);
        // The new table first: if it cannot be made, the layer keeps its lines
        let lines = features::index_texture(gl, &table)?;
        // SAFETY: uploads into objects owned by this layer; the old table, replaced, is not used afterwards
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&built.pieces), glow::STATIC_DRAW);
            gl.delete_texture(self.lines);
        }
        self.lines = lines;
        self.origin = origin;
        self.grid = built.grid;
        self.start = built.start;
        self.pieces = built.pieces.len();
        self.visible.invalidate();
        // The extent changed: a new page table
        self.drape.delete(gl);
        self.drape = Drape::new();
        Ok(())
    }

    /// Draws the pieces of `ranges`; `pick`: the picking pass, with this layer number
    fn render(&self, frame: &Frame, ranges: &[(u32, u32)], pick: Option<u8>) {
        let &Frame {
            gl,
            camera,
            view,
            pixel_ratio,
            relief,
            ..
        } = frame;
        let [segments, _, dash, layer, pick_u, slop] = &self.uniforms;
        let parts = if relief.is_some() { RELIEF_SEGMENTS } else { 1 };
        // SAFETY: draw with objects owned by this layer
        unsafe {
            gl.use_program(Some(self.program));
            let viewport = (camera.width * pixel_ratio, camera.height * pixel_ratio);
            self.anchor
                .set_view(gl, view, viewport, relief.is_some(), LINE_DEPTH_BIAS);
            self.anchor.set_origin(gl, view, self.origin);
            gl.uniform_1_i32(segments.as_ref(), parts);
            gl.uniform_2_f32(dash.as_ref(), self.dash[0], self.dash[1]);
            gl.uniform_1_f32(layer.as_ref(), f32::from(pick.unwrap_or(0)));
            gl.uniform_1_i32(pick_u.as_ref(), i32::from(pick.is_some()));
            gl.uniform_1_f32(slop.as_ref(), if pick.is_some() { PICK_SLOP } else { 0.0 });
            self.projection.set(gl, view);
            self.drape.bind(gl, &self.drape_uniforms, relief);
            self.features.bind(gl, &self.feature_uniforms, camera.zoom, pixel_ratio);
            gl.active_texture(glow::TEXTURE0 + LINES_UNIT);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.lines));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            for &(first, end) in ranges {
                let offset = first as i32 * STRIDE;
                gl.vertex_attrib_pointer_f32(0, 4, glow::FLOAT, false, STRIDE, offset);
                gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, STRIDE, offset + 16);
                gl.vertex_attrib_pointer_i32(2, 1, glow::UNSIGNED_INT, STRIDE, offset + 24);
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 2 * (parts + 1), (end - first) as i32);
            }
            gl.bind_vertex_array(None);
        }
    }

    fn update_drape(&mut self, frame: &Frame) {
        if let Some((source, _)) = frame.relief
            && self.pieces > 0
        {
            let size = DVec2::new(f64::from(self.grid.cols), f64::from(self.grid.rows)) * self.grid.size;
            self.drape.update(frame.gl, self.grid.min, self.grid.min + size, source);
        }
    }

    /// Whether the layer has anything to draw at this zoom
    fn shows(&self, frame: &Frame) -> bool {
        self.pieces > 0 && self.features.layer.shows(frame.camera.zoom)
    }
}

impl Layer for LineLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        if !self.shows(frame) {
            return false;
        }
        self.update_drape(frame);
        let look = overlay::look(self.origin, frame.view, frame.relief, 0.0);
        let mut visible = std::mem::take(&mut self.visible);
        let ranges = visible.update(&self.grid, &self.start, frame.view, look);
        let gl = frame.gl;
        // Translucent: each pixel blended once (the joins overlap)
        // A translucent color too (a feature's, or the ramp's): the joins would show as darker dots
        let translucent = self.features.translucent();
        // SAFETY: state changes on the live context, set back below
        unsafe {
            gl.disable(glow::CULL_FACE);
            if frame.relief.is_some() {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LEQUAL);
                gl.depth_mask(false);
            }
            if translucent {
                gl.clear_stencil(0);
                gl.clear(glow::STENCIL_BUFFER_BIT);
                gl.enable(glow::STENCIL_TEST);
                gl.stencil_func(glow::EQUAL, 0, 0xff);
                gl.stencil_op(glow::KEEP, glow::KEEP, glow::INCR);
            }
        }
        self.render(frame, ranges, None);
        self.visible = visible;
        // SAFETY: state changes on the live context
        unsafe {
            gl.disable(glow::STENCIL_TEST);
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
        let look = overlay::look(self.origin, frame.view, frame.relief, 0.0);
        let ranges = overlay::pick_ranges(&self.visible, &self.grid, &self.start, frame.view, look);
        self.render(frame, &ranges, Some(layer));
    }

    fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.buffer);
            gl.delete_texture(self.lines);
        }
        self.features.delete(gl);
        self.drape.delete(gl);
    }
}

#[wasm_bindgen]
impl Engine {
    /// Line layer of `feature_count` features: polylines given by their points (Mercator [0, 1], interleaved), the
    /// first point of each (`start`, one more entry than lines) and the feature each one belongs to
    #[wasm_bindgen(js_name = addLines)]
    pub fn add_lines(
        &mut self,
        points: &[f64],
        start: &[u32],
        feature: &[u32],
        feature_count: usize,
    ) -> Result<u32, JsValue> {
        let geometry = Polylines { points, start, feature };
        let layer = LineLayer::new(&self.gl, &geometry, feature_count)?;
        Ok(self.add_layer(Box::new(layer)))
    }

    /// New lines for a line layer (same arguments as `addLines`, the same features)
    #[wasm_bindgen(js_name = setLines)]
    pub fn set_lines(&mut self, id: u32, points: &[f64], start: &[u32], feature: &[u32]) -> Result<(), JsValue> {
        if let Some((layer, gl)) = self.layer_mut::<LineLayer>(id) {
            layer.set_geometry(gl, &Polylines { points, start, feature })?;
        }
        Ok(())
    }

    /// Dashes of a line layer, in line widths (`MapLibre`'s `line-dasharray`); 0, 0: solid
    #[wasm_bindgen(js_name = setLineDash)]
    pub fn set_line_dash(&mut self, id: u32, dash: f32, gap: f32) {
        if let Some((layer, _)) = self.layer_mut::<LineLayer>(id) {
            layer.dash = [dash.max(0.0), gap.max(0.0)];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_lines_do_not_panic() {
        let points = [0.5, 0.5, 0.5001, 0.5, 0.5001, 0.5001, f64::INFINITY, 0.5];
        for (start, feature) in [
            (&[][..], &[][..]),
            (&[], &[1, 2]),
            (&[0], &[0]),
            (&[0, 3], &[]),
            (&[0, u32::MAX], &[0]),
            (&[u32::MAX, 0, 3], &[0, 1]),
            (&[3, 1, 4], &[0, 1]),
        ] {
            let (built, table, origin) = build(&Polylines {
                points: &points,
                start,
                feature,
            });
            let lines = start.len().saturating_sub(1).min(feature.len());
            assert_eq!(table.len(), lines * 2, "{start:?}");
            assert!(origin.is_finite());
            assert!(
                built
                    .pieces
                    .iter()
                    .all(|q| q.from.iter().chain(&q.to).all(|v| v.is_finite()))
            );
            assert_eq!(built.start.len(), built.grid.len() + 1);
        }
        // A good line: its pieces, its feature and length
        let (built, table, _) = build(&Polylines {
            points: &points,
            start: &[0, 3],
            feature: &[7],
        });
        assert_ne!(built.pieces.len(), 0);
        assert_eq!(table[0], 7.0);
        assert!((f64::from(table[1]) - 0.0002).abs() < 1e-9);
    }
}
