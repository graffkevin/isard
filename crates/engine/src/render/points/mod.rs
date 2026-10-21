//! Points on the GPU: the vertices of `pointcloud::points` (position around an anchor, color and class, intensity,
//! returns; `STRIDE` bytes each) drawn as `GL_POINTS`, colored by their RGB, their class (a palette, which also hides
//! classes), their altitude or their intensity (a ramp), sized in pixels or by the spacing of their level (Potree's
//! adaptive size: the spacing of the deepest level drawn there, projected at their distance), square or round. Eye-Dome
//! Lighting (EDL) shades them by the depth of their neighbours on screen: the points are drawn into a framebuffer of
//! their own, then laid over the frame with their depth, still hidden by the relief and the buildings.
//!
//! The point clouds of 3D Tiles (`.pnts`, glTF points) use the same vertices and program: their RGB, lit by their
//! normal when they have one (oct-encoded in the intensity, flagged in the returns).
//!
//! - `copc`: the COPC layer (its octree, its requests, its nodes on the GPU)

pub(crate) mod copc;

use crate::render::gl::{self, Objects, RAMP_TEXELS, unit};
use glow::HasContext;
use pointcloud::points::{Attributes, COLOR, INTENSITY, RETURNS, STRIDE, push};
use wasm_bindgen::JsValue;

const VERTEX: &str = "uniform mat4 u_matrix;
uniform float u_size;
uniform float u_max_size;
uniform float u_spacing;
uniform int u_mode;
uniform bool u_classes;
uniform bool u_lit;
uniform sampler2D u_palette;
uniform sampler2D u_ramp;
uniform vec2 u_range;
uniform vec3 u_up;
uniform float u_base;
uniform highp uint u_first;
layout(location = 0) in vec3 a_position;
layout(location = 1) in vec4 a_color;
layout(location = 2) in float a_intensity;
layout(location = 3) in vec2 a_returns;
out vec3 v_color;
flat out highp uint v_id;
void main() {
  int cls = int(a_color.a * 255.0 + 0.5);
  vec4 palette = texelFetch(u_palette, ivec2(cls, 0), 0);
  if (u_classes && palette.a < 0.5) {
    // A hidden class: out of the clip volume
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    gl_PointSize = 0.0;
    return;
  }
  gl_Position = u_matrix * vec4(a_position, 1.0);
  vec3 c = a_color.rgb;
  if (u_mode == 1) {
    c = palette.rgb;
  } else if (u_mode == 2) {
    float h = u_base + dot(a_position, u_up);
    c = texture(u_ramp, ramp_uv((h - u_range.x) / (u_range.y - u_range.x))).rgb;
  } else if (u_mode == 3) {
    float t = clamp((a_intensity - u_range.x) / (u_range.y - u_range.x), 0.0, 1.0);
    c = vec3(pow(t, 0.7));
  } else if (u_lit && a_returns.x > 0.5) {
    // The normal of a 3D Tiles point, oct-encoded on two bytes: lit from above
    vec2 o = vec2(mod(a_intensity, 256.0), floor(a_intensity / 256.0)) / 255.0 * 2.0 - 1.0;
    vec3 n = vec3(o, 1.0 - abs(o.x) - abs(o.y));
    if (n.z < 0.0) n.xy = (1.0 - abs(n.yx)) * sign(n.xy);
    c *= 0.55 + 0.45 * max(dot(normalize(n), u_up), 0.0);
  }
  v_color = c;
  v_id = u_first + uint(gl_VertexID) + 1u;
  gl_PointSize = u_spacing > 0.0 ? clamp(u_spacing / gl_Position.w, u_size, u_max_size) : u_size;
}";

const FRAGMENT: &str = "uniform bool u_round;
in vec3 v_color;
flat in highp uint v_id;
out vec4 color;
void main() {
  vec2 d = gl_PointCoord * 2.0 - 1.0;
  if (u_round && dot(d, d) > 1.0) discard;
  color = vec4(v_color, 1.0);
}";

const PICK_FRAGMENT: &str = "uniform bool u_round;
uniform float u_layer;
in vec3 v_color;
flat in highp uint v_id;
out vec4 color;
void main() {
  vec2 d = gl_PointCoord * 2.0 - 1.0;
  if (u_round && dot(d, d) > 1.0) discard;
  color = pick_color(v_id, u_layer);
}";

/// The full screen in one triangle
const SCREEN: &str = "void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}";

/// Eye-Dome Lighting (Boucheny 2009, as Potree): the log of the depth compared with eight neighbours at `u_radius`
/// pixels, a point in front of them lit, one behind them darkened; the depth written as the points'
const EDL: &str = "uniform sampler2D u_color;
uniform highp sampler2D u_depth;
uniform vec2 u_near_far;
uniform float u_strength;
uniform float u_radius;
out vec4 color;
float log_depth(float d) {
  float z = d * 2.0 - 1.0;
  float n = u_near_far.x, f = u_near_far.y;
  return log2(2.0 * n * f / (f + n - z * (f - n)));
}
void main() {
  ivec2 p = ivec2(gl_FragCoord.xy);
  float d = texelFetch(u_depth, p, 0).r;
  if (d >= 1.0) discard;
  float depth = log_depth(d);
  float sum = 0.0;
  ivec2 size = textureSize(u_depth, 0) - 1;
  for (int i = 0; i < 8; i++) {
    float a = float(i) * 0.7853982;
    ivec2 q = clamp(p + ivec2(round(vec2(cos(a), sin(a)) * u_radius)), ivec2(0), size);
    float n = texelFetch(u_depth, q, 0).r;
    // A neighbour without a point counts for nothing
    if (n < 1.0) sum += max(0.0, depth - log_depth(n));
  }
  float shade = exp(-sum / 8.0 * 300.0 * u_strength);
  color = vec4(texelFetch(u_color, p, 0).rgb * shade, 1.0);
  gl_FragDepth = d;
}";

/// What a point cloud is colored by
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorBy {
    Rgb = 0,
    Classification = 1,
    Elevation = 2,
    Intensity = 3,
}

impl ColorBy {
    #[must_use]
    pub fn from_number(n: f64) -> Self {
        match n as i32 {
            1 => Self::Classification,
            2 => Self::Elevation,
            3 => Self::Intensity,
            _ => Self::Rgb,
        }
    }
}

/// How the points of a draw look
#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools, reason = "switches of the shader, set together")]
pub struct Look {
    pub color_by: ColorBy,
    /// CSS pixels: the size, or the smallest with `adaptive`
    pub size: f64,
    pub max_size: f64,
    pub adaptive: bool,
    pub round: bool,
    /// The range of the altitudes (elevation) or intensities mapped onto the ramp
    pub range: [f64; 2],
    /// Classes hidden by the palette's alpha
    pub classes: bool,
    /// 3D Tiles points with normals lit
    pub lit: bool,
}

/// One node or tile of points to draw: its points, the matrix of its anchor, the vertical there (ECEF axes), the
/// altitude of the anchor, and the spacing of its points in pixels at a unit of distance (0: fixed size)
pub struct Draw<'a> {
    pub points: &'a GpuPoints,
    pub matrix: glam::DMat4,
    pub up: glam::DVec3,
    pub base: f64,
    pub spacing: f64,
}

/// The programs of the points (drawn, picked), their palette and ramp textures
pub struct PointProgram {
    program: glow::Program,
    pick: glow::Program,
    /// `UNIFORMS` of each program (`u_layer` for the picking only)
    uniforms: [[Option<glow::UniformLocation>; 13]; 2],
    palette: glow::Texture,
    ramp: glow::Texture,
}

const UNIFORMS: [&str; 13] = [
    "u_matrix",
    "u_size",
    "u_max_size",
    "u_spacing",
    "u_mode",
    "u_classes",
    "u_lit",
    "u_range",
    "u_up",
    "u_base",
    "u_first",
    "u_round",
    "u_layer",
];

/// Texture unit of the palette (the ramp has the shared one)
const PALETTE: u32 = unit::OWN[0];

impl PointProgram {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        let mut objects = Objects::new(gl);
        let vertex = gl::vertex_shader(&[VERTEX]);
        let program = objects.program(&vertex, &gl::fragment_shader(&[FRAGMENT]))?;
        let pick = objects.program(&vertex, &gl::fragment_shader(&[PICK_FRAGMENT]))?;
        let palette = objects.texture()?;
        let ramp = objects.texture()?;

        for (texture, width, filter) in [(palette, 256, glow::NEAREST), (ramp, RAMP_TEXELS, glow::LINEAR)] {
            // SAFETY: storage and parameters of a texture just created on a live context
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, width as i32, 1);

                for (key, value) in [
                    (glow::TEXTURE_MIN_FILTER, filter),
                    (glow::TEXTURE_MAG_FILTER, filter),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, key, value as i32);
                }
            }
        }
        objects.keep();

        let uniforms = [gl::uniforms(gl, program, UNIFORMS), gl::uniforms(gl, pick, UNIFORMS)];

        for p in [program, pick] {
            let [palette, ramp] = gl::uniforms(gl, p, ["u_palette", "u_ramp"]);

            gl::sampler_units(gl, p, &[(palette.as_ref(), PALETTE), (ramp.as_ref(), unit::RAMP)]);
        }

        let made = Self {
            program,
            pick,
            uniforms,
            palette,
            ramp,
        };

        // Every class shown in grey, a grey ramp, until the layer's style is set
        made.set_palette(gl, &[[200, 200, 200, 255]; 256].concat());

        let ramp: Vec<u8> = (0..RAMP_TEXELS)
            .flat_map(|i| [i as u8, i as u8, i as u8, 255])
            .collect();

        made.set_ramp(gl, &ramp);

        Ok(made)
    }

    /// The colors of the 256 classes (RGBA, alpha 0 hides the class)
    pub fn set_palette(&self, gl: &glow::Context, rgba: &[u8]) {
        upload_row(gl, self.palette, rgba, 256);
    }

    /// The ramp of the altitudes (`RAMP_TEXELS` RGBA texels)
    pub fn set_ramp(&self, gl: &glow::Context, rgba: &[u8]) {
        upload_row(gl, self.ramp, rgba, RAMP_TEXELS);
    }

    /// Draws points (or their picking colors, `pick`: the layer's ordinal and the id of the first point, each point
    /// then an id of its own) with depth: they hide one another and what is drawn after them, the relief and the
    /// buildings hide them
    pub fn draw(&self, gl: &glow::Context, pixel_ratio: f64, look: &Look, draws: &[Draw], pick: Option<(u8, &[u32])>) {
        let [
            m,
            size,
            max_size,
            spacing,
            mode,
            classes,
            lit,
            range,
            up,
            base,
            first,
            round,
            layer,
        ] = &self.uniforms[usize::from(pick.is_some())];
        let ratio = pixel_ratio as f32;
        let _depth = gl::DepthState::solid(gl);

        // SAFETY: draws with objects owned by this program and the points given
        unsafe {
            gl.use_program(Some(if pick.is_some() { self.pick } else { self.program }));
            gl.active_texture(glow::TEXTURE0 + PALETTE);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.palette));
            gl.active_texture(glow::TEXTURE0 + unit::RAMP);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.ramp));
            gl.active_texture(glow::TEXTURE0);
            gl.uniform_1_f32(size.as_ref(), look.size as f32 * ratio);
            gl.uniform_1_f32(max_size.as_ref(), look.max_size.max(look.size) as f32 * ratio);
            gl.uniform_1_i32(mode.as_ref(), look.color_by as i32);
            gl.uniform_1_i32(classes.as_ref(), i32::from(look.classes));
            gl.uniform_1_i32(lit.as_ref(), i32::from(look.lit));

            let [lo, hi] = look.range;
            let hi = if hi > lo { hi } else { lo + 1.0 };

            gl.uniform_2_f32(range.as_ref(), lo as f32, hi as f32);
            gl.uniform_1_i32(round.as_ref(), i32::from(look.round));
            if let Some((ordinal, _)) = pick {
                gl.uniform_1_f32(layer.as_ref(), f32::from(ordinal));
            }
            for (k, d) in draws.iter().enumerate() {
                gl.uniform_matrix_4_f32_slice(m.as_ref(), false, &gl::mat4(&d.matrix));

                let u = d.up.as_vec3();

                gl.uniform_3_f32(up.as_ref(), u.x, u.y, u.z);
                gl.uniform_1_f32(base.as_ref(), d.base as f32);

                let s = if look.adaptive { d.spacing * pixel_ratio } else { 0.0 };

                gl.uniform_1_f32(spacing.as_ref(), s as f32);
                if let Some((_, firsts)) = pick {
                    gl.uniform_1_u32(first.as_ref(), firsts.get(k).copied().unwrap_or(0));
                }
                gl.bind_vertex_array(Some(d.points.vao));
                gl.draw_arrays(glow::POINTS, 0, d.points.count);
            }
            gl.bind_vertex_array(None);
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by this program, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_program(self.pick);
            gl.delete_texture(self.palette);
            gl.delete_texture(self.ramp);
        }
    }
}

fn upload_row(gl: &glow::Context, texture: glow::Texture, rgba: &[u8], width: usize) {
    let Some(rgba) = rgba.get(..width * 4) else {
        return;
    };

    // SAFETY: upload into a texture of this width, owned by the program
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            0,
            0,
            width as i32,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(rgba)),
        );
    }
}

/// Points on the GPU: their vertex array and buffer
pub struct GpuPoints {
    pub vao: glow::VertexArray,
    buffer: glow::Buffer,
    pub count: i32,
    pub bytes: usize,
}

impl GpuPoints {
    /// Uploads vertices (`STRIDE` bytes each) by `fill` into the bound buffer: from the wasm heap or straight from a
    /// JavaScript array. Nothing is left on the GPU if it fails
    fn new(gl: &glow::Context, bytes: usize, fill: impl FnOnce(u32) -> Result<(), JsValue>) -> Result<Self, JsValue> {
        let mut objects = Objects::new(gl);
        let vao = objects.vertex_array()?;
        let buffer = objects.buffer()?;
        let stride = STRIDE as i32;

        // SAFETY: state setup of objects just created on the live context
        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            fill(glow::ARRAY_BUFFER)?;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 4, glow::UNSIGNED_BYTE, true, stride, COLOR as i32);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 1, glow::UNSIGNED_SHORT, false, stride, INTENSITY as i32);
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_f32(3, 2, glow::UNSIGNED_BYTE, false, stride, RETURNS as i32);
            gl.bind_vertex_array(None);
        }
        objects.keep();

        Ok(Self {
            vao,
            buffer,
            count: (bytes / STRIDE) as i32,
            bytes,
        })
    }

    /// From vertices in the wasm heap
    pub fn upload(gl: &glow::Context, vertices: &[u8]) -> Result<Self, JsValue> {
        Self::new(gl, vertices.len(), |target| {
            // SAFETY: upload into the buffer bound by `new`
            unsafe { gl.buffer_data_u8_slice(target, vertices, glow::STATIC_DRAW) };

            Ok(())
        })
    }

    /// From vertices a worker decoded, without a copy into the wasm heap
    pub fn upload_js(
        gl: &glow::Context,
        raw: &web_sys::WebGl2RenderingContext,
        vertices: &js_sys::Uint8Array,
    ) -> Result<Self, JsValue> {
        Self::new(gl, vertices.byte_length() as usize, |target| {
            raw.buffer_data_with_array_buffer_view(target, vertices, glow::STATIC_DRAW);

            Ok(())
        })
    }

    /// Reads the vertex `index` back from the GPU (the picked point's attributes)
    pub fn read(&self, gl: &glow::Context, index: u32) -> Option<[u8; STRIDE]> {
        if index >= self.count as u32 {
            return None;
        }

        let mut out = [0u8; STRIDE];

        // SAFETY: a read within the buffer (index checked against its count)
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.get_buffer_sub_data(glow::ARRAY_BUFFER, (index as usize * STRIDE) as i32, &mut out);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }

        Some(out)
    }

    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by these points, not used afterwards
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.buffer);
        }
    }
}

/// Two bytes of a unit normal, oct-encoded (the inverse of `tiles3d::decode::oct_decode`)
#[must_use]
pub fn oct_encode(n: [f32; 3]) -> [u8; 2] {
    let l1 = n[0].abs() + n[1].abs() + n[2].abs();

    if l1 <= 0.0 {
        return [128, 128];
    }

    let (mut x, mut y) = (n[0] / l1, n[1] / l1);

    if n[2] < 0.0 {
        (x, y) = ((1.0 - y.abs()) * x.signum(), (1.0 - x.abs()) * y.signum());
    }

    let byte = |v: f32| ((v * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;

    [byte(x), byte(y)]
}

/// The vertices of 3D Tiles points: their RGB, their normal oct-encoded in the intensity (flagged in the returns)
#[must_use]
pub fn tile_vertices(points: &tiles3d::decode::Points) -> Vec<u8> {
    let mut out = Vec::with_capacity(points.colors.len() * STRIDE);

    for (i, (p, c)) in points
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(&points.colors)
        .enumerate()
    {
        let normal = points.normals.get(i).map(|&n| oct_encode(n));
        let attributes = Attributes {
            rgb: [c[0], c[1], c[2]],
            class: 0,
            intensity: normal.map_or(0, u16::from_le_bytes),
            returns: [u8::from(normal.is_some()), 0],
        };

        push(&mut out, *p, &attributes);
    }

    out
}

/// The framebuffer of the Eye-Dome Lighting, at the size of the canvas: the points drawn there, then laid over the
/// frame shaded, with their depth
pub struct Edl {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 3],
    framebuffer: glow::Framebuffer,
    color: glow::Texture,
    depth: glow::Texture,
    size: (i32, i32),
}

impl Edl {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        let mut objects = Objects::new(gl);
        let program = objects.program(&gl::vertex_shader(&[SCREEN]), &gl::fragment_shader(&[EDL]))?;
        let framebuffer = objects.framebuffer()?;
        let color = objects.texture()?;
        let depth = objects.texture()?;

        objects.keep();

        let [color_unit, depth_unit] = gl::uniforms(gl, program, ["u_color", "u_depth"]);

        gl::sampler_units(
            gl,
            program,
            &[(color_unit.as_ref(), unit::OWN[0]), (depth_unit.as_ref(), unit::OWN[1])],
        );

        Ok(Self {
            uniforms: gl::uniforms(gl, program, ["u_near_far", "u_strength", "u_radius"]),
            program,
            framebuffer,
            color,
            depth,
            size: (0, 0),
        })
    }

    /// Binds the framebuffer, cleared, at the size of the canvas (its textures made again when it changed)
    pub fn begin(&mut self, gl: &glow::Context, size: (i32, i32)) {
        // SAFETY: state changes and storage of the textures owned by this pass on the live context
        unsafe {
            if self.size != size {
                for (texture, internal, format, kind) in [
                    (self.color, glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE),
                    (
                        self.depth,
                        glow::DEPTH_COMPONENT24,
                        glow::DEPTH_COMPONENT,
                        glow::UNSIGNED_INT,
                    ),
                ] {
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        internal as i32,
                        size.0,
                        size.1,
                        0,
                        format,
                        kind,
                        glow::PixelUnpackData::Slice(None),
                    );

                    for key in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
                        gl.tex_parameter_i32(glow::TEXTURE_2D, key, glow::NEAREST as i32);
                    }
                }
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(self.color),
                    0,
                );
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::DEPTH_ATTACHMENT,
                    glow::TEXTURE_2D,
                    Some(self.depth),
                    0,
                );
                self.size = size;
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        }
    }

    /// Back to the canvas: the points laid over it, shaded by their neighbours' depth (`strength`, at `radius`
    /// device pixels), their depth tested against the frame's and written
    pub fn finish(&self, gl: &glow::Context, view: &geo::camera::View, strength: f64, radius: f64) {
        let [near_far, strength_u, radius_u] = &self.uniforms;
        let _depth = gl::DepthState::solid(gl);

        // SAFETY: draws with objects owned by this pass on the live context
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.disable(glow::BLEND);
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0 + unit::OWN[0]);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.color));
            gl.active_texture(glow::TEXTURE0 + unit::OWN[1]);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.depth));
            gl.active_texture(glow::TEXTURE0);
            gl.uniform_2_f32(near_far.as_ref(), view.near as f32, view.far as f32);
            gl.uniform_1_f32(strength_u.as_ref(), strength as f32);
            gl.uniform_1_f32(radius_u.as_ref(), radius as f32);
            gl.bind_vertex_array(None);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.enable(glow::BLEND);
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by this pass, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_texture(self.color);
            gl.delete_texture(self.depth);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiles3d::decode::oct_decode;

    #[test]
    fn oct_encoding_goes_back_and_forth() {
        for n in [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            [0.6, -0.48, 0.64],
            [-0.36, 0.48, -0.8],
        ] {
            let [x, y] = oct_encode(n);
            let back = oct_decode(x, y);
            let dot: f32 = n.iter().zip(back).map(|(a, b)| a * b).sum();

            assert!(dot > 0.999, "{n:?} → {back:?}");
        }
    }

    #[test]
    fn tile_points_carry_their_normals() {
        let points = tiles3d::decode::Points {
            positions: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
            colors: vec![[10, 20, 30, 255], [40, 50, 60, 128]],
            normals: vec![[0.0, 0.0, 1.0], [1.0, 0.0, 0.0]],
        };
        let v = tile_vertices(&points);

        assert_eq!(v.len(), 2 * STRIDE);
        assert_eq!(&v[COLOR..COLOR + 4], &[10, 20, 30, 0]);
        assert_eq!(v[RETURNS], 1);
        assert_eq!(
            oct_decode(v[INTENSITY], v[INTENSITY + 1]).map(f32::round),
            [0.0, 0.0, 1.0]
        );

        let plain = tile_vertices(&tiles3d::decode::Points {
            normals: Vec::new(),
            ..points
        });

        assert_eq!(plain[RETURNS], 0);
    }
}
