//! Symbol layer: text labels at points, drawn from signed distance field glyphs (the Géoplateforme's fonts, in the
//! format of `MapLibre`'s glyph ranges), with a halo. The page shapes the text (`glyphs.ts`: lines, wrapping, anchors)
//! and keeps the glyph atlas; the engine places the labels each frame, every symbol layer sharing one collision grid
//! from the top layer down (a label showing only where no label placed before it does), and draws each glyph as an
//! instanced quad facing the screen. In relief a label stands on the ground, and the hills hide it.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::drape::{self, Drape, DrapeUniforms};
use crate::render::features::{stops, stops_of};
use crate::render::gl::{self, Projection, bytes};
use geo::collision;
use glam::{DVec2, DVec4};
use glow::HasContext;
use wasm_bindgen::prelude::*;

const VERTEX: &str = r"
uniform mat4 u_matrix;
uniform vec2 u_plane_origin;
uniform vec2 u_origin;
uniform vec2 u_viewport;
uniform float u_depth_bias;
uniform float u_pixel_ratio;
uniform vec2 u_atlas_size;
uniform highp sampler2D u_labels;
uniform highp sampler2D u_visible;
layout(location = 0) in uint a_label;
layout(location = 1) in vec4 a_rect;
layout(location = 2) in vec4 a_uv;
layout(location = 3) in float a_scale;
out vec2 v_uv;
flat out vec4 v_fill;
flat out vec4 v_halo;
// Halo edge in distance units, and the font scale
flat out vec2 v_edges;

ivec2 texel(uint i) { return ivec2(int(i & 4095u), int(i >> 12)); }

void main() {
  // Placed this frame: shown, its angle on screen and its scale (see `place`)
  vec4 state = texelFetch(u_visible, texel(a_label), 0);
  if (state.x < 0.5) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
  float scale = state.z;
  vec2 offset = texelFetch(u_labels, texel(a_label * 4u), 0).xy;
  v_fill = texelFetch(u_labels, texel(a_label * 4u + 1u), 0);
  v_halo = texelFetch(u_labels, texel(a_label * 4u + 2u), 0);
  float halo_width = texelFetch(u_labels, texel(a_label * 4u + 3u), 0).x;
  vec2 merc = u_origin + offset;
  vec4 c = u_matrix * vec4(project_up(u_plane_origin + offset * u_world, merc, altitude(merc)), 1.0);
  c.z -= u_depth_bias;
  vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
  vec2 q = (a_rect.xy + corner * a_rect.zw) * scale;
  float cs = cos(state.y), sn = sin(state.y);
  vec2 px = vec2(q.x * cs - q.y * sn, q.x * sn + q.y * cs) * u_pixel_ratio;
  // Screen y runs down, clip y up
  c.xy += vec2(px.x, -px.y) / u_viewport * c.w;
  gl_Position = c;
  v_uv = mix(a_uv.xy, a_uv.zw, corner) / u_atlas_size;
  float font = a_scale * scale;
  v_edges = vec2(0.75 - halo_width / (font * 8.0), font);
}";

const FRAGMENT: &str = r"
uniform sampler2D u_atlas;
uniform float u_opacity;
in vec2 v_uv;
flat in vec4 v_fill;
flat in vec4 v_halo;
flat in vec2 v_edges;
out vec4 color;

void main() {
  float d = texture(u_atlas, v_uv).r;
  float g = max(fwidth(d) * 0.7, 1e-3);
  float fill = smoothstep(0.75 - g, 0.75 + g, d);
  float halo = v_edges.x < 0.75 ? smoothstep(v_edges.x - g, v_edges.x + g, d) : 0.0;
  float a_fill = v_fill.a * fill;
  float a_halo = v_halo.a * halo * (1.0 - a_fill);
  float a = a_fill + a_halo;
  if (a <= 0.0) discard;
  color = vec4((v_fill.rgb * a_fill + v_halo.rgb * a_halo) / a, a * u_opacity);
}";

const UNIFORMS: [&str; 11] = [
    "u_matrix",
    "u_plane_origin",
    "u_origin",
    "u_viewport",
    "u_depth_bias",
    "u_pixel_ratio",
    "u_atlas_size",
    "u_labels",
    "u_visible",
    "u_atlas",
    "u_opacity",
];

/// Floats per glyph quad from the page: label, rect (x, y, w, h, CSS pixels from the anchor at the shaped size), atlas
/// rect (u0, v0, u1, v1, texels), font scale (shaped size / 24)
pub const QUAD_FLOATS: usize = 10;
/// Floats per label from the page: fill RGBA, halo RGBA, halo width (CSS pixels), flags (1: always shown, never
/// hiding others), direction (Mercator; 0, 0: none, the text stays horizontal; else it runs along it, kept upright),
/// then its scale as zoom stops (four zooms, four values: displayed size over shaped size)
pub const LABEL_FLOATS: usize = 20;
const WIDTH: usize = 4096;
const LABELS_UNIT: u32 = 1;
const VISIBLE_UNIT: u32 = 2;
const ATLAS_UNIT: u32 = 3;
/// Labels in relief are pulled towards the camera by this fraction of their distance
const DEPTH_BIAS: f64 = 0.01;
/// Labels whose anchor is further than this out of the screen are not placed (CSS pixels)
const MARGIN: f64 = 100.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Quad {
    label: u32,
    rect: [f32; 4],
    uv: [f32; 4],
    scale: f32,
}

struct Label {
    merc: DVec2,
    /// CSS pixels from the anchor at the shaped size
    rect: [f32; 4],
    always: bool,
    /// Direction of the line it runs along (Mercator), zero for none
    direction: DVec2,
    /// Scale as zoom stops
    size: [[f32; 4]; 2],
}

/// Layer-wide style
#[derive(Clone, Copy)]
pub struct SymbolStyle {
    /// Size scale as zoom stops (zooms, then values: displayed size over shaped size)
    pub size: [[f32; 4]; 2],
    pub opacity: [[f32; 4]; 2],
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub visible: bool,
}

pub struct SymbolLayer {
    pub style: SymbolStyle,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 11],
    projection: Projection,
    drape_uniforms: DrapeUniforms,
    vao: glow::VertexArray,
    quads: glow::Buffer,
    quad_count: i32,
    labels_texture: glow::Texture,
    visible_texture: glow::Texture,
    labels: Vec<Label>,
    /// Per label, this frame: shown (1 or 0), angle on screen (radians, clockwise), scale, unused
    visible: Vec<f32>,
    origin: DVec2,
    min: DVec2,
    max: DVec2,
    drape: Drape,
}

impl SymbolLayer {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        let program = gl::program(
            gl,
            &format!("#version 300 es\n{}{}{VERTEX}", gl::PROJECT, drape::ALTITUDE),
            &format!("#version 300 es\nprecision highp float;\n{FRAGMENT}"),
        )?;
        // SAFETY: object creation on a live context
        unsafe {
            let vao = gl.create_vertex_array()?;
            let quads = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(quads));
            let stride = size_of::<Quad>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_i32(0, 1, glow::UNSIGNED_INT, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, stride, 4);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, stride, 20);
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_f32(3, 1, glow::FLOAT, false, stride, 36);
            for location in 0..4 {
                gl.vertex_attrib_divisor(location, 1);
            }
            gl.bind_vertex_array(None);
            Ok(Self {
                style: SymbolStyle {
                    size: [[0.0; 4], [1.0; 4]],
                    opacity: [[0.0; 4], [1.0; 4]],
                    min_zoom: 0.0,
                    max_zoom: 24.0,
                    visible: true,
                },
                program,
                uniforms: gl::uniforms(gl, program, UNIFORMS),
                projection: Projection::new(gl, program),
                drape_uniforms: DrapeUniforms::new(gl, program),
                vao,
                quads,
                quad_count: 0,
                labels_texture: gl.create_texture()?,
                visible_texture: gl.create_texture()?,
                labels: Vec::new(),
                visible: Vec::new(),
                origin: DVec2::ZERO,
                min: DVec2::ZERO,
                max: DVec2::ZERO,
                drape: Drape::new(),
            })
        }
    }

    /// Replaces the labels: anchors (Mercator, interleaved), their boxes (4 floats each), styles (`LABEL_FLOATS` each)
    /// and glyph quads (`QUAD_FLOATS` each)
    pub fn set_labels(
        &mut self,
        gl: &glow::Context,
        anchors: &[f64],
        boxes: &[f32],
        styles: &[f32],
        quads: &[f32],
    ) -> Result<(), String> {
        let n = (anchors.len() / 2)
            .min(boxes.len() / 4)
            .min(styles.len() / LABEL_FLOATS);
        let merc: Vec<DVec2> = anchors
            .as_chunks::<2>()
            .0
            .iter()
            .take(n)
            .map(|&[x, y]| DVec2::new(x, y))
            .collect();
        let (min, max) = merc.iter().fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), &p| {
            (a.min(p), b.max(p))
        });
        (self.min, self.max) = if n == 0 { (DVec2::ZERO, DVec2::ZERO) } else { (min, max) };
        self.origin = (self.min + self.max) / 2.0;
        self.labels = merc
            .iter()
            .enumerate()
            .map(|(i, &m)| Label {
                merc: m,
                rect: [boxes[i * 4], boxes[i * 4 + 1], boxes[i * 4 + 2], boxes[i * 4 + 3]],
                always: styles[i * LABEL_FLOATS + 9] > 0.5,
                direction: DVec2::new(
                    f64::from(styles[i * LABEL_FLOATS + 10]),
                    f64::from(styles[i * LABEL_FLOATS + 11]),
                ),
                size: stops_of(&styles[i * LABEL_FLOATS + 12..i * LABEL_FLOATS + 20]).unwrap_or([[0.0; 4], [1.0; 4]]),
            })
            .collect();
        self.visible = vec![0.0; n * 4];
        // Per label: offset, fill, halo, halo width
        let rows = (n * 4).div_ceil(WIDTH).max(1);
        let mut table = vec![0f32; rows * WIDTH * 4];
        for (i, m) in merc.iter().enumerate() {
            let o = (*m - self.origin).as_vec2();
            let s = &styles[i * LABEL_FLOATS..(i + 1) * LABEL_FLOATS];
            let t = i * 16;
            table[t..t + 2].copy_from_slice(&[o.x, o.y]);
            table[t + 4..t + 12].copy_from_slice(&s[..8]);
            table[t + 12] = s[8];
        }
        let data: Vec<Quad> = quads
            .as_chunks::<QUAD_FLOATS>()
            .0
            .iter()
            .filter(|q| (q[0] as usize) < n)
            .map(|q| Quad {
                label: q[0] as u32,
                rect: [q[1], q[2], q[3], q[4]],
                uv: [q[5], q[6], q[7], q[8]],
                scale: q[9],
            })
            .collect();
        // SAFETY: (re)allocation and uploads into objects owned by this layer
        unsafe {
            gl.delete_texture(self.labels_texture);
            gl.delete_texture(self.visible_texture);
            self.labels_texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(self.labels_texture));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA32F, WIDTH as i32, rows as i32);
            nearest(gl);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                WIDTH as i32,
                rows as i32,
                glow::RGBA,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(bytes(&table))),
            );
            self.visible_texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(self.visible_texture));
            gl.tex_storage_2d(
                glow::TEXTURE_2D,
                1,
                glow::RGBA32F,
                WIDTH as i32,
                n.div_ceil(WIDTH).max(1) as i32,
            );
            nearest(gl);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.quads));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&data), glow::STATIC_DRAW);
        }
        self.quad_count = data.len() as i32;
        self.drape.delete(gl);
        self.drape = Drape::new();
        Ok(())
    }

    fn shows(&self, zoom: f64) -> bool {
        self.style.visible && zoom >= self.style.min_zoom && zoom < self.style.max_zoom && self.quad_count > 0
    }
}

/// Nearest filtering on the bound texture
unsafe fn nearest(gl: &glow::Context) {
    // SAFETY: parameters of the bound texture
    unsafe {
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
    }
}

impl Layer for SymbolLayer {
    fn place(&mut self, frame: &Frame, grid: &mut collision::Grid) {
        self.visible.fill(0.0);
        if !self.shows(frame.camera.zoom) {
            return;
        }
        let (camera, view) = (frame.camera, frame.view);
        let layer_scale = stops(&self.style.size, camera.zoom);
        // Screen point and distance to the eye (pixels of the local frame)
        let screen = |m: DVec2, altitude: f64| -> Option<(DVec2, f64)> {
            let shift = -(m.x - camera.center.x).round();
            let p = view.position(m, shift) + glam::DVec3::Z * altitude * view.vertical;
            let clip = view.view_proj * DVec4::new(p.x, p.y, p.z, 1.0);
            (clip.w > 0.0).then(|| {
                let at = DVec2::new(
                    f64::midpoint(clip.x / clip.w, 1.0) * camera.width,
                    f64::midpoint(-clip.y / clip.w, 1.0) * camera.height,
                );
                (at, clip.w)
            })
        };
        for (label, state) in self.labels.iter().zip(self.visible.as_chunks_mut::<4>().0) {
            let altitude = frame
                .relief
                .and_then(|(source, e)| source.altitude(label.merc).map(|h| h * f64::from(e)))
                .unwrap_or(0.0);
            let Some((at, distance)) = screen(label.merc, altitude) else {
                continue;
            };
            if at.x < -MARGIN || at.y < -MARGIN || at.x > camera.width + MARGIN || at.y > camera.height + MARGIN {
                continue;
            }
            // Smaller further away, as MapLibre's perspective ratio: a tilted view keeps its far labels sparse
            let perspective = (0.5 + 0.5 * view.center_distance / distance).clamp(0.5, 1.5);
            let scale = layer_scale * stops(&label.size, camera.zoom) * perspective;
            if scale <= 0.0 {
                continue;
            }
            // Along its line, kept upright: the angle of the line on screen, turned half a turn if upside down
            let mut angle = 0.0;
            if label.direction != DVec2::ZERO {
                // A step of one pixel along the line
                let step = label.direction.normalize() / view.world_size;
                if let Some((ahead, _)) = screen(label.merc + step, altitude) {
                    let d = ahead - at;
                    angle = d.y.atan2(d.x);
                    if angle > std::f64::consts::FRAC_PI_2 {
                        angle -= std::f64::consts::PI;
                    } else if angle < -std::f64::consts::FRAC_PI_2 {
                        angle += std::f64::consts::PI;
                    }
                }
            }
            // The box turned with the text, as an axis-aligned box
            let r = label.rect.map(|v| f64::from(v) * scale);
            let (cs, sn) = (angle.cos(), angle.sin());
            let (mut lo, mut hi) = (DVec2::INFINITY, DVec2::NEG_INFINITY);
            for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
                let p = DVec2::new(x * cs - y * sn, x * sn + y * cs);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            let rect = [at.x + lo.x, at.y + lo.y, at.x + hi.x, at.y + hi.y];
            if label.always || grid.place(rect) {
                *state = [1.0, angle as f32, scale as f32, 0.0];
            }
        }
        let texels = self.labels.len();
        if texels == 0 {
            return;
        }
        let (full, rest) = (texels / WIDTH, texels % WIDTH);
        let data: &[u8] = bytes(&self.visible);
        // SAFETY: upload into the state texture allocated for these labels
        unsafe {
            let gl = frame.gl;
            gl.bind_texture(glow::TEXTURE_2D, Some(self.visible_texture));
            if full > 0 {
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    WIDTH as i32,
                    full as i32,
                    glow::RGBA,
                    glow::FLOAT,
                    glow::PixelUnpackData::Slice(Some(&data[..full * WIDTH * 16])),
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
                    glow::RGBA,
                    glow::FLOAT,
                    glow::PixelUnpackData::Slice(Some(&data[full * WIDTH * 16..texels * 16])),
                );
            }
        }
    }

    fn draw(&mut self, frame: &Frame) -> bool {
        let &Frame {
            gl,
            camera,
            view,
            pixel_ratio,
            relief,
            glyphs,
            ..
        } = frame;
        let Some((atlas, atlas_size)) = glyphs else {
            return false;
        };
        if !self.shows(camera.zoom) {
            return false;
        }
        if let Some((source, _)) = relief {
            let pad = DVec2::splat(1e-6);
            self.drape.update(gl, self.min - pad, self.max + pad, source);
        }
        let [
            matrix,
            plane_origin,
            origin,
            viewport,
            depth_bias,
            ratio,
            atlas_size_u,
            labels,
            visible,
            atlas_u,
            opacity,
        ] = &self.uniforms;
        let shift = -(self.origin.x - camera.center.x).round();
        let plane = (DVec2::new(self.origin.x + shift, self.origin.y) - camera.center) * view.world_size;
        // SAFETY: draw with objects owned by this layer and the engine's glyph atlas
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
            gl.uniform_1_f32(ratio.as_ref(), pixel_ratio as f32);
            gl.uniform_2_f32(atlas_size_u.as_ref(), atlas_size.0 as f32, atlas_size.1 as f32);
            gl.uniform_1_f32(opacity.as_ref(), stops(&self.style.opacity, camera.zoom) as f32);
            self.projection.set(gl, view);
            self.drape.bind(gl, &self.drape_uniforms, relief);
            for (unit, location, texture) in [
                (LABELS_UNIT, labels, self.labels_texture),
                (VISIBLE_UNIT, visible, self.visible_texture),
                (ATLAS_UNIT, atlas_u, atlas),
            ] {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.uniform_1_i32(location.as_ref(), unit as i32);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.disable(glow::CULL_FACE);
            if relief.is_some() {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LEQUAL);
                gl.depth_mask(false);
            }
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, self.quad_count);
            gl.bind_vertex_array(None);
            gl.enable(glow::CULL_FACE);
            gl.disable(glow::DEPTH_TEST);
            gl.depth_mask(true);
        }
        false
    }

    fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.quads);
            gl.delete_texture(self.labels_texture);
            gl.delete_texture(self.visible_texture);
        }
        self.drape.delete(gl);
    }
}

/// The glyph atlas of an engine: signed distances, one byte per texel
pub struct GlyphAtlas {
    pub texture: glow::Texture,
    pub size: (i32, i32),
}

#[wasm_bindgen]
impl Engine {
    /// An empty symbol layer
    #[wasm_bindgen(js_name = addSymbols)]
    pub fn add_symbols(&mut self) -> Result<u32, JsValue> {
        let layer = SymbolLayer::new(&self.gl)?;
        Ok(self.add_layer(Box::new(layer)))
    }

    /// Labels of a symbol layer, shaped by the page: anchors (Mercator, interleaved), boxes (min x, min y, max x, max y,
    /// CSS pixels from the anchor), styles (20 floats each, see `LABEL_FLOATS`) and glyph quads (10 floats each: label, x, y, width, height from the anchor, atlas u0, v0, u1, v1 in
    /// texels, font scale)
    #[wasm_bindgen(js_name = setSymbols)]
    pub fn set_symbols(
        &mut self,
        id: u32,
        anchors: &[f64],
        boxes: &[f32],
        styles: &[f32],
        quads: &[f32],
    ) -> Result<(), JsValue> {
        if let Some((layer, gl)) = self.layer_mut::<SymbolLayer>(id) {
            layer.set_labels(gl, anchors, boxes, styles, quads)?;
        }
        Ok(())
    }

    /// Layer-wide style of a symbol layer: size scale and opacity as zoom stops (four zooms then four values each),
    /// zoom range, visibility
    #[wasm_bindgen(js_name = setSymbolLayer)]
    pub fn set_symbol_layer(
        &mut self,
        id: u32,
        size: &[f32],
        opacity: &[f32],
        min_zoom: f64,
        max_zoom: f64,
        visible: bool,
    ) {
        let pair = |s: &[f32]| -> Option<[[f32; 4]; 2]> {
            let s: &[f32; 8] = s.get(..8)?.try_into().ok()?;
            Some([[s[0], s[1], s[2], s[3]], [s[4], s[5], s[6], s[7]]])
        };
        if let Some((layer, _)) = self.layer_mut::<SymbolLayer>(id) {
            if let Some(size) = pair(size) {
                layer.style.size = size;
            }
            if let Some(opacity) = pair(opacity) {
                layer.style.opacity = opacity;
            }
            layer.style.min_zoom = min_zoom;
            layer.style.max_zoom = max_zoom;
            layer.style.visible = visible;
        }
    }

    /// The glyph atlas shared by the symbol layers: `width` × `height` bytes of signed distances (allocated again,
    /// filled with `data` if given)
    #[wasm_bindgen(js_name = setGlyphAtlas)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_glyph_atlas(&mut self, width: i32, height: i32, data: Option<Box<[u8]>>) -> Result<(), JsValue> {
        // SAFETY: (re)allocation and upload of the atlas on the live context
        unsafe {
            let gl = &self.gl;
            if let Some(old) = self.glyphs.take() {
                gl.delete_texture(old.texture);
            }
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::R8, width, height);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, key, value as i32);
            }
            if let Some(data) = data.as_deref().filter(|d| d.len() >= (width * height) as usize) {
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    width,
                    height,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&data[..(width * height) as usize])),
                );
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            }
            self.glyphs = Some(GlyphAtlas {
                texture,
                size: (width, height),
            });
        }
        Ok(())
    }

    /// A region of the glyph atlas: `width` × `height` bytes at (`x`, `y`)
    #[wasm_bindgen(js_name = updateGlyphAtlas)]
    pub fn update_glyph_atlas(&mut self, x: i32, y: i32, width: i32, height: i32, data: &[u8]) {
        let Some(atlas) = &self.glyphs else {
            return;
        };
        if data.len() < (width * height) as usize || x + width > atlas.size.0 || y + height > atlas.size.1 {
            return;
        }
        // SAFETY: upload inside the atlas
        unsafe {
            let gl = &self.gl;
            gl.bind_texture(glow::TEXTURE_2D, Some(atlas.texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                x,
                y,
                width,
                height,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&data[..(width * height) as usize])),
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        }
    }
}
