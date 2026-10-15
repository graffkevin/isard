//! Network layer: line segments between nodes, colored by a value per node (travel time, water depth…).
//!
//! The geometry is uploaded once: node positions in a float texture, edges as one instance each (two node
//! indices). A new set of values is a single texture upload of 4 bytes per node, then one draw call: millions of
//! segments recolor at every mouse move. Lines are expanded to quads on the GPU, with a constant width in pixels
//! and antialiased edges. In relief, the nodes stand on the ground (`drape`) and the hills hide the lines behind them.

use crate::geo::camera::{self, Camera, View};
use crate::render::drape::{self, Drape, DrapeUniforms};
use crate::render::gl::{self, Projection, bytes as as_bytes};
use crate::render::raster::RasterSource;
use glam::DVec2;
use glow::HasContext;

/// Width of the node textures; 4096 is supported everywhere WebGL2 is
const WIDTH: usize = 4096;

const VERTEX: &str = r"
uniform mat4 u_matrix;
// Node offsets are Mercator, relative to the network origin: on the plane, in pixels from the camera center
// (u_world, pixels per world, comes with PROJECT)
uniform vec2 u_plane_origin;
uniform vec2 u_origin;
uniform vec2 u_viewport;
uniform float u_width;
// Pulls the lines towards the camera in relief (clip space), so that they win over the ground they lie on
uniform float u_depth_bias;
uniform int u_segments;
uniform highp sampler2D u_positions;
uniform highp sampler2D u_values;
layout(location = 0) in uvec2 a_edge;
#ifdef PICK
flat out uint v_id;
#endif
out float v_value;
out float v_across;
flat out float v_half;

ivec2 texel(uint i) { return ivec2(int(i & 4095u), int(i >> 12)); }

vec2 offset_a;
vec2 offset_b;

// Clip position of the point at `t` along the edge, standing on the ground in relief
vec4 clip(float t) {
  vec2 offset = mix(offset_a, offset_b, t);
  vec2 merc = u_origin + offset;
  vec4 c = u_matrix * vec4(project_up(u_plane_origin + offset * u_world, merc, altitude(merc)), 1.0);
  c.z -= u_depth_bias;
  return c;
}

void main() {
  // The edge is a strip of `u_segments` pieces (1 on flat ground; in relief each piece follows the ground)
  int k = gl_VertexID >> 1;
  float side = float(gl_VertexID & 1) * 2.0 - 1.0;
  float step = 1.0 / float(u_segments);
  float t = float(k) * step;
  offset_a = texelFetch(u_positions, texel(a_edge.x), 0).xy;
  offset_b = texelFetch(u_positions, texel(a_edge.y), 0).xy;
  vec4 c = clip(t);
  vec4 prev = k > 0 ? clip(t - step) : c;
  vec4 next = k < u_segments ? clip(t + step) : c;
  if (c.w <= 0.0 || prev.w <= 0.0 || next.w <= 0.0) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
  vec2 s = c.xy / c.w * u_viewport;
  vec2 dir = next.xy / next.w * u_viewport - prev.xy / prev.w * u_viewport;
  dir = length(dir) > 1e-6 ? normalize(dir) : vec2(1.0, 0.0);
  vec2 normal = vec2(-dir.y, dir.x);
  // Half width plus one pixel for the antialiasing ramp; caps extended by the same amount
  float half_width = u_width * 0.5 + 1.0;
  float cap = k == 0 ? -1.0 : k == u_segments ? 1.0 : 0.0;
  s += normal * side * half_width + dir * cap * half_width * 0.5;
  gl_Position = vec4(s / u_viewport * c.w, c.z, c.w);
  float va = texelFetch(u_values, texel(a_edge.x), 0).r;
  float vb = texelFetch(u_values, texel(a_edge.y), 0).r;
  // Unreached (negative) ends stay unreached along the edge
  v_value = k == 0 ? va : k == u_segments ? vb : (va < 0.0 || vb < 0.0 ? -1.0 : mix(va, vb, t));
  v_across = side * half_width;
  v_half = half_width;
#ifdef PICK
  v_id = uint(gl_InstanceID) + 1u;
#endif
}";

const FRAGMENT: &str = r"#version 300 es
precision highp float;
uniform float u_max;
uniform float u_contour;
uniform float u_opacity;
in float v_value;
in float v_across;
flat in float v_half;
out vec4 color;

vec3 ramp(float t) {
  const vec3 c0 = vec3(0.000, 0.659, 0.420);
  const vec3 c1 = vec3(0.608, 0.827, 0.294);
  const vec3 c2 = vec3(1.000, 0.824, 0.247);
  const vec3 c3 = vec3(1.000, 0.549, 0.196);
  const vec3 c4 = vec3(0.910, 0.220, 0.310);
  t = clamp(t, 0.0, 1.0) * 4.0;
  if (t < 1.0) return mix(c0, c1, t);
  if (t < 2.0) return mix(c1, c2, t - 1.0);
  if (t < 3.0) return mix(c2, c3, t - 2.0);
  return mix(c3, c4, t - 3.0);
}

void main() {
  float edge = clamp(v_half - abs(v_across), 0.0, 1.0);
  bool reached = v_value >= 0.0 && v_value <= u_max;
  vec3 rgb = reached ? ramp(v_value / u_max) : vec3(0.55);
  float alpha = reached ? 1.0 : 0.25;
  if (reached && u_contour > 0.0) {
    float d = abs(fract(v_value / u_contour + 0.5) - 0.5) * u_contour;
    float w = fwidth(v_value);
    rgb *= mix(0.45, 1.0, smoothstep(w * 0.5, w * 1.5, d));
  }
  color = vec4(rgb, alpha * edge * u_opacity);
}";

/// Picking pass: the edge number (+1) in RGB, the layer in alpha; no blending, no antialiasing
const PICK_FRAGMENT: &str = r"#version 300 es
precision highp float;
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

/// Pieces of each edge in relief: a straight edge would cut through the bumps of the ground between its nodes
const RELIEF_SEGMENTS: i32 = 8;

/// A program with its uniforms
struct Pass {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 14],
    projection: Projection,
    drape: DrapeUniforms,
}

impl Pass {
    fn new(gl: &glow::Context, defines: &str, fragment: &str) -> Result<Self, String> {
        let program = gl::program(
            gl,
            &format!("#version 300 es\n{defines}{}{}{VERTEX}", gl::PROJECT, drape::ALTITUDE),
            fragment,
        )?;
        let uniforms = gl::uniforms(
            gl,
            program,
            [
                "u_matrix",
                "u_viewport",
                "u_width",
                "u_positions",
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
            ],
        );
        Ok(Self {
            program,
            uniforms,
            projection: Projection::new(gl, program),
            drape: DrapeUniforms::new(gl, program),
        })
    }
}

pub struct NetworkStyle {
    /// CSS pixels
    pub width: f32,
    /// Values above are drawn as unreached
    pub max: f32,
    /// Darker line every `contour` value units, 0 for none
    pub contour: f32,
    pub opacity: f32,
}

pub struct NetworkLayer {
    pub style: NetworkStyle,
    shaded: Pass,
    pick: Pass,
    vao: glow::VertexArray,
    edges: glow::Buffer,
    positions: glow::Texture,
    values: glow::Texture,
    /// Mercator origin of the node positions
    origin: DVec2,
    /// Mercator extent of the nodes
    extent: (DVec2, DVec2),
    drape: Drape,
    nodes: usize,
    edge_count: usize,
}

impl NetworkLayer {
    /// `lon_lat`: node coordinates in degrees, interleaved; `edges`: node index pairs, interleaved
    pub fn new(gl: &glow::Context, lon_lat: &[f64], edges: &[u32], style: NetworkStyle) -> Result<Self, String> {
        let nodes = lon_lat.len() / 2;
        if let Some(bad) = edges.iter().find(|&&i| i as usize >= nodes) {
            return Err(format!("edge to node {bad}, only {nodes} nodes"));
        }
        let shaded = Pass::new(gl, "", FRAGMENT)?;
        let pick = Pass::new(gl, "#define PICK\n", PICK_FRAGMENT)?;
        let merc: Vec<DVec2> = lon_lat
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[lon, lat]| camera::mercator(lon, lat))
            .collect();
        let (min, max) = merc.iter().fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), &p| {
            (a.min(p), b.max(p))
        });
        let origin = if nodes == 0 { DVec2::ZERO } else { (min + max) / 2.0 };
        let rows = nodes.div_ceil(WIDTH).max(1);
        let mut positions = vec![0f32; rows * WIDTH * 2];
        for (i, p) in merc.iter().enumerate() {
            let d = *p - origin;
            positions[i * 2] = d.x as f32;
            positions[i * 2 + 1] = d.y as f32;
        }
        // SAFETY: object creation and uploads on a live WebGL2 context
        unsafe {
            let positions_texture = float_texture(gl, glow::RG32F, rows)?;
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                WIDTH as i32,
                rows as i32,
                glow::RG,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(as_bytes(&positions))),
            );
            let values = float_texture(gl, glow::R32F, rows)?;
            let vao = gl.create_vertex_array()?;
            let edge_buffer = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(edge_buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(edges), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_i32(0, 2, glow::UNSIGNED_INT, 8, 0);
            gl.vertex_attrib_divisor(0, 1);
            gl.bind_vertex_array(None);
            let mut layer = Self {
                style,
                shaded,
                pick,
                vao,
                edges: edge_buffer,
                positions: positions_texture,
                values,
                origin,
                extent: (min, max),
                drape: Drape::new(),
                nodes,
                edge_count: edges.len() / 2,
            };
            // Unreached until the first values
            layer.set_values(gl, &vec![-1.0; nodes]);
            Ok(layer)
        }
    }

    /// One value per node, in node order; negative = unreached
    pub fn set_values(&mut self, gl: &glow::Context, values: &[f32]) {
        let n = values.len().min(self.nodes);
        let (full, rest) = (n / WIDTH, n % WIDTH);
        // SAFETY: uploads inside the texture allocated for `nodes` values
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.values));
            if full > 0 {
                upload_rows(gl, 0, WIDTH, full, &values[..full * WIDTH]);
            }
            if rest > 0 {
                upload_rows(gl, full, rest, 1, &values[full * WIDTH..n]);
            }
        }
    }

    /// Draws the lines; `relief`: the elevation source and its exaggeration, to stand the nodes on the ground
    pub fn draw(
        &mut self,
        gl: &glow::Context,
        camera: &Camera,
        view: &View,
        pixel_ratio: f64,
        relief: Option<(&RasterSource, f32)>,
    ) {
        if let Some((source, _)) = relief
            && self.nodes > 0
        {
            self.drape.update(gl, self.extent.0, self.extent.1, source);
        }
        self.render(gl, &self.shaded, camera, view, pixel_ratio, 0.0, relief);
    }

    /// Picking pass into the bound framebuffer: edge number + 1 in RGB, `layer` in alpha
    pub fn draw_pick(
        &self,
        gl: &glow::Context,
        camera: &Camera,
        view: &View,
        pixel_ratio: f64,
        layer: u8,
        relief: Option<(&RasterSource, f32)>,
    ) {
        // SAFETY: uniform on the picking program
        unsafe {
            gl.use_program(Some(self.pick.program));
            gl.uniform_1_f32(self.pick.uniforms[11].as_ref(), f32::from(layer));
        }
        self.render(gl, &self.pick, camera, view, pixel_ratio, PICK_SLOP, relief);
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        gl: &glow::Context,
        pass: &Pass,
        camera: &Camera,
        view: &View,
        pixel_ratio: f64,
        slop: f64,
        relief: Option<(&RasterSource, f32)>,
    ) {
        let [
            matrix,
            viewport,
            width,
            positions,
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
        ] = &pass.uniforms;
        let shift = -(self.origin.x - camera.center.x).round();
        let plane = (DVec2::new(self.origin.x + shift, self.origin.y) - camera.center) * view.world_size;
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
            let bias = if relief.is_some() { 2.0 * view.near * DEPTH_BIAS } else { 0.0 };
            gl.uniform_1_f32(depth_bias.as_ref(), bias as f32);
            let pieces = if relief.is_some() { RELIEF_SEGMENTS } else { 1 };
            gl.uniform_1_i32(segments.as_ref(), pieces);
            let (w, h) = (camera.width * pixel_ratio / 2.0, camera.height * pixel_ratio / 2.0);
            gl.uniform_2_f32(viewport.as_ref(), w as f32, h as f32);
            let line = (f64::from(self.style.width) + slop) * pixel_ratio;
            gl.uniform_1_f32(width.as_ref(), line as f32);
            gl.uniform_1_f32(max.as_ref(), self.style.max);
            gl.uniform_1_f32(contour.as_ref(), self.style.contour);
            gl.uniform_1_f32(opacity.as_ref(), self.style.opacity);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.positions));
            gl.uniform_1_i32(positions.as_ref(), 1);
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.values));
            gl.uniform_1_i32(values.as_ref(), 2);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 2 * (pieces + 1), self.edge_count as i32);
            gl.bind_vertex_array(None);
            gl.active_texture(glow::TEXTURE0);
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.shaded.program);
            gl.delete_program(self.pick.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.edges);
            gl.delete_texture(self.positions);
            gl.delete_texture(self.values);
        }
        self.drape.delete(gl);
    }
}

/// Float texture of `rows` × 4096 texels, nearest filtering (read with `texelFetch`)
unsafe fn float_texture(gl: &glow::Context, format: u32, rows: usize) -> Result<glow::Texture, String> {
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

unsafe fn upload_rows(gl: &glow::Context, row: usize, width: usize, rows: usize, values: &[f32]) {
    // SAFETY: the caller bound the values texture and sized the slice
    unsafe {
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            0,
            row as i32,
            width as i32,
            rows as i32,
            glow::RED,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(Some(as_bytes(values))),
        );
    }
}
