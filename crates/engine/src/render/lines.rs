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
use crate::render::gl::{self, Projection, bytes};
use crate::render::raster::RasterSource;
use geo::camera::View;
use geo::pieces::{self, Grid, Lines};
use glam::DVec2;
use glow::HasContext;
use wasm_bindgen::prelude::*;

const VERTEX: &str = r"
uniform mat4 u_matrix;
uniform vec2 u_plane_origin;
uniform vec2 u_origin;
uniform vec2 u_viewport;
uniform float u_depth_bias;
uniform int u_segments;
uniform float u_slop;
// Per line: its feature and its length (Mercator)
uniform highp sampler2D u_lines;
layout(location = 0) in vec4 a_piece;
layout(location = 1) in vec2 a_fractions;
layout(location = 2) in uint a_line;
flat out vec4 v_color;
flat out float v_half;
flat out float v_length;
flat out uint v_id;
out float v_across;
out float v_along;
// Distance along the line, in line widths (the dashes)
out float v_dash;

vec4 clip(float t) {
  vec2 offset = mix(a_piece.xy, a_piece.zw, t);
  vec2 merc = u_origin + offset;
  vec4 c = u_matrix * vec4(project_up(u_plane_origin + offset * u_world, merc, altitude(merc)), 1.0);
  c.z -= u_depth_bias;
  return c;
}

void hide() { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); }

void main() {
  vec2 line = texelFetch(u_lines, feature_texel(a_line), 0).xy;
  uint f = uint(line.x);
  float width = feature_size(f, 0u) * u_pixel_ratio + u_slop;
  v_color = feature_color(f, 0u);
  v_color.a *= layer_opacity();
  if (!feature_visible(f) || width <= 0.0 || v_color.a <= 0.0) { hide(); return; }
  int k = gl_VertexID >> 1;
  float side = float(gl_VertexID & 1) * 2.0 - 1.0;
  float step = 1.0 / float(u_segments);
  float t = float(k) * step;
  vec4 c = clip(t);
  vec4 prev = k > 0 ? clip(t - step) : c;
  vec4 next = k < u_segments ? clip(t + step) : c;
  vec4 c0 = clip(0.0);
  vec4 c1 = clip(1.0);
  if (c.w <= 0.0 || prev.w <= 0.0 || next.w <= 0.0 || c0.w <= 0.0 || c1.w <= 0.0) { hide(); return; }
  vec2 s = c.xy / c.w * u_viewport;
  vec2 dir = next.xy / next.w * u_viewport - prev.xy / prev.w * u_viewport;
  dir = length(dir) > 1e-6 ? normalize(dir) : vec2(1.0, 0.0);
  vec2 normal = vec2(-dir.y, dir.x);
  // Half width plus one pixel of antialiasing; both ends pushed out by as much, room for the round caps
  float half_width = width * 0.5 + 1.0;
  float cap = k == 0 ? -1.0 : k == u_segments ? 1.0 : 0.0;
  s += normal * side * half_width + dir * cap * half_width;
  gl_Position = vec4(s / u_viewport * c.w, c.z, c.w);
  // Device pixels, as `s`
  float len = length(c1.xy / c1.w * u_viewport - c0.xy / c0.w * u_viewport);
  v_length = len;
  v_along = cap < 0.0 ? -half_width : cap > 0.0 ? len + half_width : t * len;
  v_across = side * half_width;
  v_half = width;
  v_dash = mix(a_fractions.x, a_fractions.y, t) * line.y * u_world * u_pixel_ratio / max(width, 1e-3);
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

/// Uniforms of the pass, by name
const UNIFORMS: [&str; 12] = [
    "u_matrix",
    "u_plane_origin",
    "u_origin",
    "u_viewport",
    "u_depth_bias",
    "u_segments",
    "u_lines",
    "u_dash",
    "u_layer",
    "u_pick",
    "u_slop",
    "u_world",
];

/// Lines in relief are pulled towards the camera by this fraction of their distance (as the network's)
const DEPTH_BIAS: f64 = 0.002;
/// Parts of each piece in relief
const RELIEF_SEGMENTS: i32 = 8;
/// Altitudes the lines may stand at in relief (metres, before exaggeration), to bound the squares in view
const RELIEF_RANGE: (f64, f64) = (-200.0, 5000.0);
/// Extra width in the picking pass, device pixels
const PICK_SLOP: f32 = 8.0;
/// Bytes per piece
const STRIDE: i32 = 28;
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

pub struct LineLayer {
    pub features: Features,
    /// Dash and gap lengths, in line widths; 0: solid
    pub dash: [f32; 2],
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 12],
    projection: Projection,
    drape_uniforms: DrapeUniforms,
    feature_uniforms: FeatureUniforms,
    vao: glow::VertexArray,
    buffer: glow::Buffer,
    lines: glow::Texture,
    origin: DVec2,
    grid: Grid,
    start: Vec<u32>,
    drape: Drape,
    pieces: usize,
}

impl LineLayer {
    pub fn new(gl: &glow::Context, geometry: &Polylines, feature_count: usize) -> Result<Self, String> {
        let program = gl::program(
            gl,
            &format!("#version 300 es\n{}{}{FEATURES}{VERTEX}", gl::PROJECT, drape::ALTITUDE),
            &format!("#version 300 es\nprecision highp float;\n{FRAGMENT}"),
        )?;
        // SAFETY: object creation on a live context
        let (vao, buffer, lines) = unsafe {
            let vao = gl.create_vertex_array()?;
            let buffer = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            for location in 0..3 {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_divisor(location, 1);
            }
            gl.bind_vertex_array(None);
            (vao, buffer, gl.create_texture()?)
        };
        let mut layer = Self {
            features: Features::new(gl, feature_count)?,
            dash: [0.0, 0.0],
            program,
            uniforms: gl::uniforms(gl, program, UNIFORMS),
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
            drape: Drape::new(),
            pieces: 0,
        };
        layer.set_geometry(gl, geometry)?;
        Ok(layer)
    }

    /// Replaces the lines (the styles stay, by feature)
    pub fn set_geometry(&mut self, gl: &glow::Context, geometry: &Polylines) -> Result<(), String> {
        let points: Vec<DVec2> = geometry
            .points
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[x, y]| DVec2::new(x, y))
            .collect();
        let count = geometry.start.len().saturating_sub(1).min(geometry.feature.len());
        // Each line is an edge from its first point to its last, along its points
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
        let (min, max) = points
            .iter()
            .fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), &p| {
                (a.min(p), b.max(p))
            });
        self.origin = if points.is_empty() {
            DVec2::ZERO
        } else {
            (min + max) / 2.0
        };
        let lines = Lines {
            start: &geometry.start[..=count],
            points: &points,
        };
        let built = pieces::build(&nodes, &edges, Some(&lines), self.origin);
        // SAFETY: uploads into objects owned by this layer
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&built.pieces), glow::STATIC_DRAW);
            gl.delete_texture(self.lines);
        }
        self.lines = features::index_texture(gl, &table)?;
        self.grid = built.grid;
        self.start = built.start;
        self.pieces = built.pieces.len();
        // The extent changed: a new page table
        self.drape.delete(gl);
        self.drape = Drape::new();
        Ok(())
    }

    fn visible(&self, view: &View, relief: Option<(&RasterSource, f32)>) -> Vec<(u32, u32)> {
        let heights = relief.map_or((0.0, 0.0), |(_, e)| {
            let k = view.vertical * f64::from(e);
            (RELIEF_RANGE.0 * k, RELIEF_RANGE.1 * k)
        });
        let shift = -(self.origin.x - view.center.x).round();
        pieces::visible(&self.grid, &self.start, view, heights, shift)
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
        let [
            matrix,
            plane_origin,
            origin,
            viewport,
            depth_bias,
            segments,
            lines,
            dash,
            layer,
            pick_u,
            slop,
            world,
        ] = &self.uniforms;
        let shift = -(self.origin.x - camera.center.x).round();
        let plane = (DVec2::new(self.origin.x + shift, self.origin.y) - camera.center) * view.world_size;
        let parts = if relief.is_some() { RELIEF_SEGMENTS } else { 1 };
        // SAFETY: draw with objects owned by this layer
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_2_f32(plane_origin.as_ref(), plane.x as f32, plane.y as f32);
            gl.uniform_1_f32(world.as_ref(), view.world_size as f32);
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
            gl.uniform_1_i32(lines.as_ref(), LINES_UNIT as i32);
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
        let ranges = self.visible(frame.view, frame.relief);
        let gl = frame.gl;
        // Translucent: each pixel blended once (the joins overlap)
        let translucent = self.features.layer.opacity[1].iter().any(|&o| o < 1.0);
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
        self.render(frame, &ranges, None);
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
        let ranges = self.visible(frame.view, frame.relief);
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
