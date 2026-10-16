//! Network layer: edges between nodes, colored by a value per node (travel time, water depth…).
//!
//! The geometry is uploaded once: each edge, straight or along its line, is cut into pieces (`geo::pieces`), one
//! instance each, sorted by square of a grid over the network; each frame draws only the squares in view. The nodes
//! of each edge and its costs live in textures, the values of the nodes too: a new set of values is a single texture
//! upload of 4 bytes per node, and millions of pieces recolor at every mouse move.
//!
//! The value at a point of an edge is worked out per pixel: with costs, the quickest way there, min(t(A) + cost
//! from A, t(B) + cost from B), so an edge reached from both ends shows where the two fronts meet; without, the two
//! values interpolated. An unreached node (negative or 1e30 and above) reaches nothing. Lines are expanded to quads
//! on the GPU with a constant width in pixels and antialiased edges; an optional halo pass draws them wide and faint
//! first, so that the streets add up into a heat map. In relief, the pieces stand on the ground (`drape`) and the
//! hills hide the lines behind them.

use crate::engine::Engine;
use crate::js;
use crate::layer::{Frame, Layer};
use crate::render::drape::{self, Drape, DrapeUniforms};
use crate::render::features::{stops, stops_of};
use crate::render::gl::{self, Projection, bytes as as_bytes};
use crate::render::raster::RasterSource;
use geo::camera::{Camera, View};
use geo::pieces::{self, Grid, Lines};
use glam::DVec2;
use glow::HasContext;
use std::cell::Cell;
use wasm_bindgen::prelude::*;

/// Width of the node and edge textures; 4096 is supported everywhere WebGL2 is
const WIDTH: usize = 4096;

/// Stored for a closed way: Infinity does not survive every driver's float textures (some read it back as 0); the
/// shader takes values and costs from 1e29 up as unreachable
const CLOSED: f32 = 1e30;

const VERTEX: &str = r"
uniform mat4 u_matrix;
// Piece ends are Mercator, relative to the network origin: on the plane, in pixels from the camera center
// (u_world, pixels per world, comes with PROJECT)
uniform vec2 u_plane_origin;
uniform vec2 u_origin;
uniform vec2 u_viewport;
uniform float u_width;
// Pulls the lines towards the camera in relief (clip space), so that they win over the ground they lie on
uniform float u_depth_bias;
uniform int u_segments;
uniform bool u_with_costs;
uniform highp sampler2D u_values;
uniform highp usampler2D u_edges;
uniform highp sampler2D u_costs;
// Width factor of each edge (rail lines wider than streets)
uniform highp sampler2D u_scales;
uniform float u_max;
uniform float u_halo;
// From, to (Mercator offsets); fractions of the edge's length at both ends; the edge
layout(location = 0) in vec4 a_piece;
layout(location = 1) in vec2 a_fractions;
layout(location = 2) in uint a_edge;
#ifdef PICK
flat out uint v_id;
#endif
out float v_fraction;
flat out vec4 v_ends;
out float v_across;
flat out float v_half;

ivec2 texel(uint i) { return ivec2(int(i & 4095u), int(i >> 12)); }

// Clip position of the point at `t` along the piece, standing on the ground in relief
vec4 clip(float t) {
  vec2 offset = mix(a_piece.xy, a_piece.zw, t);
  vec2 merc = u_origin + offset;
  vec4 c = u_matrix * vec4(project_up(u_plane_origin + offset * u_world, merc, altitude(merc)), 1.0);
  c.z -= u_depth_bias;
  return c;
}

void hide() { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); }

void main() {
  uvec2 nodes = texelFetch(u_edges, texel(a_edge), 0).xy;
  vec2 costs = u_with_costs ? texelFetch(u_costs, texel(a_edge), 0).xy : vec2(0.0);
  // An edge closed both ways is not drawn
  if (u_with_costs && costs.x >= 1e29 && costs.y >= 1e29) { hide(); return; }
  v_ends = vec4(texelFetch(u_values, texel(nodes.x), 0).r, texelFetch(u_values, texel(nodes.y), 0).r, costs);
  // The halo of an edge reached at neither end within the maximum draws nothing: out of the clip space, it costs no
  // pixel (a million wide quads zoomed out would saturate the graphics card)
  bool near_a = v_ends.x >= 0.0 && v_ends.x <= u_max;
  bool near_b = v_ends.y >= 0.0 && v_ends.y <= u_max;
  if (u_halo > 0.0 && !near_a && !near_b) { hide(); return; }
  float width = u_width * texelFetch(u_scales, texel(a_edge), 0).r;
  // The piece is a strip of `u_segments` parts (1 on flat ground; in relief each part follows the ground)
  int k = gl_VertexID >> 1;
  float side = float(gl_VertexID & 1) * 2.0 - 1.0;
  float step = 1.0 / float(u_segments);
  float t = float(k) * step;
  vec4 c = clip(t);
  vec4 prev = k > 0 ? clip(t - step) : c;
  vec4 next = k < u_segments ? clip(t + step) : c;
  if (c.w <= 0.0 || prev.w <= 0.0 || next.w <= 0.0) { hide(); return; }
  vec2 s = c.xy / c.w * u_viewport;
  vec2 dir = next.xy / next.w * u_viewport - prev.xy / prev.w * u_viewport;
  dir = length(dir) > 1e-6 ? normalize(dir) : vec2(1.0, 0.0);
  vec2 normal = vec2(-dir.y, dir.x);
  // Half width plus one pixel for the antialiasing ramp; caps extended by the same amount
  float half_width = width * 0.5 + 1.0;
  float cap = k == 0 ? -1.0 : k == u_segments ? 1.0 : 0.0;
  s += normal * side * half_width + dir * cap * half_width * 0.5;
  gl_Position = vec4(s / u_viewport * c.w, c.z, c.w);
  v_fraction = mix(a_fractions.x, a_fractions.y, t);
  v_across = side * half_width;
  v_half = half_width;
#ifdef PICK
  v_id = a_edge + 1u;
#endif
}";

/// Value at a pixel of an edge; negative: unreached
const VALUE: &str = r"
uniform bool u_with_costs;
in float v_fraction;
flat in vec4 v_ends;

float value() {
  float ta = v_ends.x, tb = v_ends.y;
  bool ra = ta >= 0.0 && ta < 1e29;
  bool rb = tb >= 0.0 && tb < 1e29;
  if (!u_with_costs) return ra && rb ? mix(ta, tb, v_fraction) : -1.0;
  // The quickest way here: from A forwards or from B backwards
  float t = 1e30;
  if (ra && v_ends.z < 1e29) t = ta + v_fraction * v_ends.z;
  if (rb && v_ends.w < 1e29) t = min(t, tb + (1.0 - v_fraction) * v_ends.w);
  return t >= 1e29 ? -1.0 : t;
}
";

const FRAGMENT: &str = r"
uniform float u_max;
uniform float u_contour;
uniform float u_opacity;
// 0: the lines; 1: the halo (wide, faint, reached parts only)
uniform float u_halo;
uniform sampler2D u_ramp;
// Color of the parts beyond the maximum (or unreached), straight alpha
uniform vec4 u_unreached;
// Share of the maximum over which the colors fade out before it, 0 for none
uniform float u_fade;
in float v_across;
flat in float v_half;
out vec4 color;

vec3 ramp(float t) { return texture(u_ramp, vec2((clamp(t, 0.0, 1.0) * 255.0 + 0.5) / 256.0, 0.5)).rgb; }

void main() {
  float v = value();
  bool reached = v >= 0.0 && v <= u_max;
  float fade = u_fade > 0.0 && reached ? 1.0 - smoothstep(u_max * (1.0 - u_fade), u_max, v) : 1.0;
  if (u_halo > 0.0) {
    if (!reached) discard;
    // Quadratic falloff from the middle of the line: overlapping streets add up and fill the blocks
    float d = 1.0 - clamp(abs(v_across) / v_half, 0.0, 1.0);
    color = vec4(ramp(v / u_max), d * d * u_opacity * fade);
    return;
  }
  float edge = clamp(v_half - abs(v_across), 0.0, 1.0);
  vec3 rgb = reached ? ramp(v / u_max) : u_unreached.rgb;
  float alpha = reached ? fade : u_unreached.a;
  if (reached && u_contour > 0.0) {
    float d = abs(fract(v / u_contour + 0.5) - 0.5) * u_contour;
    float w = fwidth(v);
    rgb *= mix(0.45, 1.0, smoothstep(w * 0.5, w * 1.5, d));
  }
  color = vec4(rgb, alpha * edge * u_opacity);
}";

/// Picking pass: the edge number (+1) in RGB, the layer in alpha; no blending, no antialiasing
const PICK_FRAGMENT: &str = r"
uniform float u_layer;
in float v_across;
flat in float v_half;
flat in uint v_id;
out vec4 color;
void main() {
  if (abs(v_across) > v_half) discard;
  color = vec4(float(v_id & 255u), float((v_id >> 8) & 255u), float((v_id >> 16) & 255u), u_layer) / 255.0;
}";

/// Extra width of the lines in the picking pass, CSS pixels: thin lines stay easy to hover
const PICK_SLOP: f64 = 6.0;

/// Lines in relief are pulled towards the camera by this fraction of their distance: the ground under them is
/// interpolated between the vertices of its mesh and can rise a little above the line
const DEPTH_BIAS: f64 = 0.002;

/// Parts of each piece in relief: a straight piece would cut through the bumps of the ground between its ends
const RELIEF_SEGMENTS: i32 = 8;

/// Altitudes the network may stand at in relief (metres, before exaggeration), to bound the squares in view
const RELIEF_RANGE: (f64, f64) = (-200.0, 5000.0);

/// Uniforms of a pass, by name
const UNIFORMS: [&str; 21] = [
    "u_matrix",
    "u_viewport",
    "u_width",
    "u_values",
    "u_max",
    "u_contour",
    "u_opacity",
    "u_plane_origin",
    "u_world",
    "u_origin",
    "u_layer",
    "u_depth_bias",
    "u_segments",
    "u_with_costs",
    "u_edges",
    "u_costs",
    "u_halo",
    "u_scales",
    "u_ramp",
    "u_unreached",
    "u_fade",
];

/// A program with its uniforms
struct Pass {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 21],
    projection: Projection,
    drape: DrapeUniforms,
}

impl Pass {
    fn new(gl: &glow::Context, defines: &str, fragment: &str) -> Result<Self, String> {
        let program = gl::program(
            gl,
            &format!("#version 300 es\n{defines}{}{}{VERTEX}", gl::PROJECT, drape::ALTITUDE),
            &format!("#version 300 es\nprecision highp float;\n{defines}{VALUE}{fragment}"),
        )?;
        Ok(Self {
            program,
            uniforms: gl::uniforms(gl, program, UNIFORMS),
            projection: Projection::new(gl, program),
            drape: DrapeUniforms::new(gl, program),
        })
    }
}

#[derive(Clone, Copy)]
pub struct NetworkStyle {
    /// CSS pixels, as zoom stops (zooms, then values)
    pub width: [[f32; 4]; 2],
    /// Width of the halo, CSS pixels as zoom stops, 0 for none
    pub halo: [[f32; 4]; 2],
    /// Values above are drawn as unreached
    pub max: f32,
    /// Darker line every `contour` value units, 0 for none
    pub contour: f32,
    pub opacity: f32,
    pub halo_opacity: f32,
    /// Color of the unreached parts, RGBA 0-1
    pub unreached: [f32; 4],
    /// Share of the maximum over which the colors fade out before it
    pub fade: f32,
}

impl NetworkStyle {
    /// From the floats of `setNetworkStyle`
    fn from_floats(s: &[f32]) -> Option<Self> {
        let s: &[f32; FLOATS] = s.get(..FLOATS)?.try_into().ok()?;
        Some(Self {
            width: stops_of(&s[0..8])?,
            halo: stops_of(&s[8..16])?,
            max: s[16],
            contour: s[17],
            opacity: s[18],
            halo_opacity: s[19],
            unreached: [s[20], s[21], s[22], s[23]],
            fade: s[24],
        })
    }
}

/// Floats of a style for `setNetworkStyle`
const FLOATS: usize = 25;

/// Texture units of the layer's own textures (the drape takes 6 and 7)
const VALUES_UNIT: u32 = 1;
const EDGES_UNIT: u32 = 2;
const COSTS_UNIT: u32 = 3;
const SCALES_UNIT: u32 = 4;
const RAMP_UNIT: u32 = 5;

/// The ramp used without one given: green to red, five stops
const DEFAULT_RAMP: [[f32; 3]; 5] = [
    [0.000, 0.659, 0.420],
    [0.608, 0.827, 0.294],
    [1.000, 0.824, 0.247],
    [1.000, 0.549, 0.196],
    [0.910, 0.220, 0.310],
];

pub struct NetworkLayer {
    pub style: NetworkStyle,
    shaded: Pass,
    pick: Pass,
    vao: glow::VertexArray,
    pieces: glow::Buffer,
    /// Node values (R32F), nodes of each edge (RG32UI), costs of each edge (RG32F)
    values: glow::Texture,
    edges: glow::Texture,
    costs: glow::Texture,
    /// Width factor of each edge (R32F), colors of the values (256 RGBA)
    scales: glow::Texture,
    ramp: glow::Texture,
    with_costs: bool,
    /// Mercator origin of the piece positions
    origin: DVec2,
    /// Squares of the network and the pieces of each
    grid: Grid,
    start: Vec<u32>,
    drape: Drape,
    nodes: usize,
    edge_count: usize,
    piece_count: usize,
    /// Pieces drawn in the last frame
    drawn: Cell<u32>,
}

/// Bytes per piece: from, to (4 × f32), fractions (2 × f32), edge (u32)
const STRIDE: i32 = 28;

impl NetworkLayer {
    /// `mercator`: node positions in Mercator [0, 1] (y towards the south), interleaved; `edges`: node index pairs,
    /// interleaved; `lines`: the line of each edge from A to B (offsets of edge `e`'s points: `start[e] .. start[e +
    /// 1]`, points in Mercator, interleaved); `costs`: per edge, the cost of travelling it from A to B then from B to A
    pub fn new(
        gl: &glow::Context,
        mercator: &[f64],
        edges: &[u32],
        lines: Option<(&[u32], &[f64])>,
        costs: Option<&[f32]>,
        style: NetworkStyle,
    ) -> Result<Self, String> {
        let nodes = mercator.len() / 2;
        if let Some(bad) = edges.iter().find(|&&i| i as usize >= nodes) {
            return Err(format!("edge to node {bad}, only {nodes} nodes"));
        }
        let edge_count = edges.len() / 2;
        let shaded = Pass::new(gl, "", FRAGMENT)?;
        let pick = Pass::new(gl, "#define PICK\n", PICK_FRAGMENT)?;
        let points = |pairs: &[f64]| -> Vec<DVec2> {
            pairs
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[x, y]| DVec2::new(x, y))
                .collect()
        };
        let merc = points(mercator);
        let line_points = lines.map(|(_, line)| points(line));
        let lines = lines
            .zip(line_points.as_deref())
            .map(|((start, _), points)| Lines { start, points });
        let (min, max) = merc.iter().fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), &p| {
            (a.min(p), b.max(p))
        });
        let origin = if nodes == 0 { DVec2::ZERO } else { (min + max) / 2.0 };
        let built = pieces::build(&merc, edges, lines.as_ref(), origin);
        // SAFETY: object creation and uploads on a live WebGL2 context
        unsafe {
            let node_rows = nodes.div_ceil(WIDTH).max(1);
            let edge_rows = edge_count.div_ceil(WIDTH).max(1);
            let values = texture(gl, glow::R32F, node_rows)?;
            let edge_texture = texture(gl, glow::RG32UI, edge_rows)?;
            let mut padded = vec![0u32; edge_rows * WIDTH * 2];
            padded[..edges.len()].copy_from_slice(edges);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                WIDTH as i32,
                edge_rows as i32,
                glow::RG_INTEGER,
                glow::UNSIGNED_INT,
                glow::PixelUnpackData::Slice(Some(as_bytes(&padded))),
            );
            let cost_texture = texture(gl, glow::RG32F, edge_rows)?;
            let scales = texture(gl, glow::R32F, edge_rows)?;
            let ramp = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(ramp));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, 256, 1);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, key, value as i32);
            }
            let vao = gl.create_vertex_array()?;
            let buffer = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&built.pieces), glow::STATIC_DRAW);
            for location in 0..3 {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_divisor(location, 1);
            }
            gl.bind_vertex_array(None);
            let mut layer = Self {
                style,
                shaded,
                pick,
                vao,
                pieces: buffer,
                values,
                edges: edge_texture,
                costs: cost_texture,
                scales,
                ramp,
                with_costs: false,
                origin,
                grid: built.grid,
                start: built.start,
                drape: Drape::new(),
                nodes,
                edge_count,
                piece_count: built.pieces.len(),
                drawn: Cell::new(0),
            };
            // Unreached until the first values
            layer.set_values(gl, &vec![-1.0; nodes]);
            layer.set_costs(gl, costs);
            layer.set_scales(gl, None);
            layer.set_ramp(gl, None);
            Ok(layer)
        }
    }

    /// One value per node, in node order; negative (or 1e30 and above) = unreached
    pub fn set_values(&mut self, gl: &glow::Context, values: &[f32]) {
        let n = values.len().min(self.nodes);
        // SAFETY: uploads inside the texture allocated for `nodes` values
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.values));
            upload(gl, &values[..n], 1, glow::RED);
        }
    }

    /// Per edge, the cost of travelling it from A to B then from B to A (Infinity: closed that way); None: values
    /// interpolated along the edges
    pub fn set_costs(&mut self, gl: &glow::Context, costs: Option<&[f32]>) {
        self.with_costs = costs.is_some();
        let Some(costs) = costs else {
            return;
        };
        let clamped: Vec<f32> = costs
            .iter()
            .take(self.edge_count * 2)
            .map(|&c| if c.is_nan() { CLOSED } else { c.min(CLOSED) })
            .collect();
        // SAFETY: uploads inside the texture allocated for `edge_count` pairs
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.costs));
            upload(gl, &clamped, 2, glow::RG);
        }
    }

    /// Width factor of each edge (1 for all without)
    pub fn set_scales(&mut self, gl: &glow::Context, scales: Option<&[f32]>) {
        let mut all = vec![1.0f32; self.edge_count.max(1)];
        if let Some(s) = scales {
            let n = s.len().min(all.len());
            all[..n].copy_from_slice(&s[..n]);
        }
        // SAFETY: upload inside the texture allocated for `edge_count` edges
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.scales));
            upload(gl, &all[..self.edge_count], 1, glow::RED);
        }
    }

    /// Colors of the values from 0 to the maximum: 256 RGBA texels; None: the default ramp, green to red
    pub fn set_ramp(&mut self, gl: &glow::Context, rgba: Option<&[u8]>) {
        let default: Vec<u8> = (0..256)
            .flat_map(|i| {
                let t = i as f32 / 255.0 * 4.0;
                let k = (t.floor() as usize).min(3);
                let f = t - k as f32;
                let (a, b) = (DEFAULT_RAMP[k], DEFAULT_RAMP[k + 1]);
                [0, 1, 2]
                    .map(|c| ((a[c] + (b[c] - a[c]) * f) * 255.0).round() as u8)
                    .into_iter()
                    .chain([255])
            })
            .collect();
        let data = rgba.filter(|r| r.len() >= 1024).unwrap_or(&default);
        // SAFETY: upload of the whole ramp texture
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.ramp));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                256,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&data[..1024])),
            );
        }
    }

    /// The node values (R32F, 4096 wide, node `i` at (i % 4096, i / 4096)), read by layers colored by them
    pub fn values_texture(&self) -> glow::Texture {
        self.values
    }

    /// Pieces in all, and drawn in the last frame
    pub fn counts(&self) -> (usize, u32) {
        (self.piece_count, self.drawn.get())
    }

    /// Ranges of pieces in the squares in view
    fn visible(&self, view: &View, relief: Option<(&RasterSource, f32)>) -> Vec<(u32, u32)> {
        let heights = relief.map_or((0.0, 0.0), |(_, e)| {
            let k = view.vertical * f64::from(e);
            (RELIEF_RANGE.0 * k, RELIEF_RANGE.1 * k)
        });
        let shift = -(self.origin.x - view.center.x).round();
        pieces::visible(&self.grid, &self.start, view, heights, shift)
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        gl: &glow::Context,
        pass: &Pass,
        camera: &Camera,
        view: &View,
        pixel_ratio: f64,
        width: Width,
        relief: Option<(&RasterSource, f32)>,
        ranges: &[(u32, u32)],
    ) {
        if ranges.is_empty() {
            return;
        }
        let [
            matrix,
            viewport,
            width_u,
            values,
            max,
            contour,
            opacity,
            plane_origin,
            world,
            origin,
            _,
            depth_bias,
            segments,
            with_costs,
            edges,
            costs,
            halo,
            scales,
            ramp,
            unreached,
            fade,
        ] = &pass.uniforms;
        let shift = -(self.origin.x - camera.center.x).round();
        let plane = (DVec2::new(self.origin.x + shift, self.origin.y) - camera.center) * view.world_size;
        let style = &self.style;
        // SAFETY: draw with objects owned by this layer
        unsafe {
            gl.use_program(Some(pass.program));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_2_f32(plane_origin.as_ref(), plane.x as f32, plane.y as f32);
            gl.uniform_1_f32(world.as_ref(), view.world_size as f32);
            gl.uniform_2_f32(origin.as_ref(), self.origin.x as f32, self.origin.y as f32);
            pass.projection.set(gl, view);
            self.drape.bind(gl, &pass.drape, relief);
            // About 0.2 % of the distance: in clip space a constant (2 × near × fraction)
            let bias = if relief.is_some() {
                2.0 * view.near * DEPTH_BIAS
            } else {
                0.0
            };
            gl.uniform_1_f32(depth_bias.as_ref(), bias as f32);
            let parts = if relief.is_some() { RELIEF_SEGMENTS } else { 1 };
            gl.uniform_1_i32(segments.as_ref(), parts);
            let (w, h) = (camera.width * pixel_ratio / 2.0, camera.height * pixel_ratio / 2.0);
            gl.uniform_2_f32(viewport.as_ref(), w as f32, h as f32);
            gl.uniform_1_f32(width_u.as_ref(), (width.pixels * pixel_ratio) as f32);
            gl.uniform_1_f32(max.as_ref(), style.max);
            gl.uniform_1_f32(contour.as_ref(), style.contour);
            let alpha = if width.halo { style.halo_opacity } else { style.opacity };
            gl.uniform_1_f32(opacity.as_ref(), alpha);
            gl.uniform_1_f32(halo.as_ref(), if width.halo { 1.0 } else { 0.0 });
            gl.uniform_1_i32(with_costs.as_ref(), i32::from(self.with_costs));
            gl.uniform_4_f32_slice(unreached.as_ref(), &style.unreached);
            gl.uniform_1_f32(fade.as_ref(), style.fade);
            for (unit, location, texture) in [
                (VALUES_UNIT, values, self.values),
                (EDGES_UNIT, edges, self.edges),
                (COSTS_UNIT, costs, self.costs),
                (SCALES_UNIT, scales, self.scales),
                (RAMP_UNIT, ramp, self.ramp),
            ] {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.uniform_1_i32(location.as_ref(), unit as i32);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.pieces));
            // No base instance in WebGL2: the instanced attributes are pointed at the first piece of each range
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
}

impl Layer for NetworkLayer {
    /// Draws the halo then the lines; in relief, the network stands on the ground and the hills hide the lines behind
    /// them (tested against the ground's depth, not written)
    fn draw(&mut self, frame: &Frame) -> bool {
        let &Frame {
            gl,
            camera,
            view,
            pixel_ratio,
            relief,
            ..
        } = frame;
        if let Some((source, _)) = relief
            && self.nodes > 0
        {
            let (lo, hi) = (
                self.grid.min,
                self.grid.min + DVec2::new(f64::from(self.grid.cols), f64::from(self.grid.rows)) * self.grid.size,
            );
            self.drape.update(gl, lo, hi, source);
        }
        let ranges = self.visible(view, relief);
        self.drawn.set(ranges.iter().map(|(a, b)| b - a).sum());
        // Line quads are built on screen, either way round
        // SAFETY: state changes on the live context, set back below
        unsafe {
            gl.disable(glow::CULL_FACE);
            if relief.is_some() {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LEQUAL);
                gl.depth_mask(false);
            }
        }
        let style = self.style;
        let halo_width = stops(&style.halo, camera.zoom);
        if halo_width > 0.0 && style.halo_opacity > 0.0 {
            let halo = Width {
                pixels: halo_width,
                halo: true,
            };
            self.render(gl, &self.shaded, camera, view, pixel_ratio, halo, relief, &ranges);
        }
        let lines = Width {
            pixels: stops(&style.width, camera.zoom),
            halo: false,
        };
        self.render(gl, &self.shaded, camera, view, pixel_ratio, lines, relief, &ranges);
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

    /// Edge number + 1 in RGB, `layer` in alpha, lines widened a little to be easier to hit
    fn draw_pick(&self, frame: &Frame, layer: u8) {
        let &Frame {
            gl,
            camera,
            view,
            pixel_ratio,
            relief,
            ..
        } = frame;
        // SAFETY: uniform on the picking program
        unsafe {
            gl.use_program(Some(self.pick.program));
            gl.uniform_1_f32(self.pick.uniforms[10].as_ref(), f32::from(layer));
        }
        let ranges = self.visible(view, relief);
        let width = Width {
            pixels: stops(&self.style.width, camera.zoom) + PICK_SLOP,
            halo: false,
        };
        self.render(gl, &self.pick, camera, view, pixel_ratio, width, relief, &ranges);
    }

    fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.shaded.program);
            gl.delete_program(self.pick.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.pieces);
            gl.delete_texture(self.values);
            gl.delete_texture(self.edges);
            gl.delete_texture(self.costs);
            gl.delete_texture(self.scales);
            gl.delete_texture(self.ramp);
        }
        self.drape.delete(gl);
    }
}

/// Width of a pass and whether it is the halo
#[derive(Clone, Copy)]
struct Width {
    pixels: f64,
    halo: bool,
}

/// Texture of `rows` × 4096 texels, nearest filtering (read with `texelFetch`), left bound
unsafe fn texture(gl: &glow::Context, format: u32, rows: usize) -> Result<glow::Texture, String> {
    // SAFETY: called with a live context
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_storage_2d(glow::TEXTURE_2D, 1, format, WIDTH as i32, rows as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        Ok(texture)
    }
}

/// Uploads `data` (`channels` floats per texel) into the bound texture from its first texel, row after row
unsafe fn upload(gl: &glow::Context, data: &[f32], channels: usize, format: u32) {
    let texels = data.len() / channels;
    let (full, rest) = (texels / WIDTH, texels % WIDTH);
    // SAFETY: the caller bound a texture large enough for `data`
    unsafe {
        if full > 0 {
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                WIDTH as i32,
                full as i32,
                format,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(as_bytes(&data[..full * WIDTH * channels]))),
            );
        }
        if rest > 0 {
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                full as i32,
                rest as i32,
                1,
                format,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(as_bytes(&data[full * WIDTH * channels..texels * channels]))),
            );
        }
    }
}

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r"
/** Pieces of a network layer: its edges cut along their lines, sorted by square; only the squares in view are drawn */
export interface NetworkStats {
  pieces: number;
  /** Drawn in the last frame */
  drawn: number;
}
";

#[wasm_bindgen]
impl Engine {
    /// Network of `mercator.length / 2` nodes (Mercator [0, 1], y towards the south, interleaved: converted by the
    /// JavaScript side, where `Math.tan` and `Math.log` are much faster than in WebAssembly) and `edges.length / 2`
    /// edges (node index pairs, interleaved). `line_start` and `line_points`: the line of each edge from A to B, points
    /// `line_start[e] .. line_start[e + 1]` of `line_points` (Mercator, interleaved; straight if fewer than two). `costs`: per edge, the
    /// cost of travelling it from A to B then from B to A; without, values are interpolated along the edges.
    #[wasm_bindgen(js_name = addNetwork)]
    // Optional arrays only cross the boundary owned
    #[allow(clippy::needless_pass_by_value)]
    pub fn add_network(
        &mut self,
        mercator: &[f64],
        edges: &[u32],
        line_start: Option<Box<[u32]>>,
        line_points: Option<Box<[f64]>>,
        costs: Option<Box<[f32]>>,
        style: &[f32],
    ) -> Result<u32, JsValue> {
        let style = NetworkStyle::from_floats(style).ok_or("a network style is 25 numbers")?;
        let lines = line_start.as_deref().zip(line_points.as_deref());
        let layer = NetworkLayer::new(&self.gl, mercator, edges, lines, costs.as_deref(), style)?;
        Ok(self.add_layer(Box::new(layer)))
    }

    /// One value per node; negative = unreached
    #[wasm_bindgen(js_name = setNetworkValues)]
    pub fn set_network_values(&mut self, id: u32, values: &[f32]) {
        if let Some((layer, gl)) = self.layer_mut::<NetworkLayer>(id) {
            layer.set_values(gl, values);
        }
    }

    /// Costs of a network's edges (see `addNetwork`), when the way it is travelled changes; null: interpolated values
    #[wasm_bindgen(js_name = setNetworkCosts)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_network_costs(&mut self, id: u32, costs: Option<Box<[f32]>>) {
        if let Some((layer, gl)) = self.layer_mut::<NetworkLayer>(id) {
            layer.set_costs(gl, costs.as_deref());
        }
    }

    /// Style of a network: width (CSS pixels) and halo width as zoom stops (8 numbers each: four zooms, four values),
    /// then the maximum, the contour interval, the opacity, the halo opacity, the color of the unreached parts (RGBA
    /// 0-1) and the share of the maximum over which the colors fade out: 25 numbers
    #[wasm_bindgen(js_name = setNetworkStyle)]
    pub fn set_network_style(&mut self, id: u32, style: &[f32]) {
        if let (Some((layer, _)), Some(style)) = (self.layer_mut::<NetworkLayer>(id), NetworkStyle::from_floats(style))
        {
            layer.style = style;
        }
    }

    /// Width factor of each edge (1 without): rail lines wider than streets
    #[wasm_bindgen(js_name = setNetworkScales)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_network_scales(&mut self, id: u32, scales: Option<Box<[f32]>>) {
        if let Some((layer, gl)) = self.layer_mut::<NetworkLayer>(id) {
            layer.set_scales(gl, scales.as_deref());
        }
    }

    /// Colors of the values from 0 to the maximum, 256 RGBA texels (1024 bytes); undefined: green to red
    #[wasm_bindgen(js_name = setNetworkRamp)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_network_ramp(&mut self, id: u32, rgba: Option<Box<[u8]>>) {
        if let Some((layer, gl)) = self.layer_mut::<NetworkLayer>(id) {
            layer.set_ramp(gl, rgba.as_deref());
        }
    }

    /// Pieces of a network in all and drawn in the last frame (only the squares in view are)
    #[wasm_bindgen(js_name = networkStats, unchecked_return_type = "NetworkStats | undefined")]
    #[must_use]
    pub fn network_stats(&self, id: u32) -> JsValue {
        self.layer::<NetworkLayer>(id).map_or(JsValue::UNDEFINED, |layer| {
            let (pieces, drawn) = layer.counts();
            js::object(&[("pieces", (pieces as f64).into()), ("drawn", drawn.into())])
        })
    }
}
