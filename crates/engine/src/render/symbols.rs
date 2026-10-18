//! Symbol layer: text labels at points, drawn from signed distance field glyphs (in the format of `MapLibre`'s
//! glyph ranges), with a halo. The page shapes the text (`glyphs.ts`: lines, wrapping, anchors)
//! and keeps the glyph atlas; the engine places the labels each frame, every symbol layer sharing one collision grid
//! from the top layer down (a label showing only where no label placed before it does), and draws each glyph as an
//! instanced quad facing the screen. In relief a label stands on the ground, and the hills hide it; on the globe, a
//! label past its horizon is not placed.
//!
//! The atlas has pages of one size (the layers of a texture array), so it never runs out: the atlas coordinates of a
//! glyph count its page's rows first (`v + page × height`, as if the pages were stacked), and the shader takes the
//! page back from them. Glyphs already placed keep their coordinates when a page is added.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::drape::Drape;
use crate::render::features::{stops, stops_of};
use crate::render::gl::{self, DepthState, Objects, TABLE_WIDTH as WIDTH, Table, bytes, unit};
use crate::render::overlay::{self, Overlay};
use geo::collision;
use glam::{DVec2, DVec4};
use glow::HasContext;
use wasm_bindgen::prelude::*;

const VERTEX: &str = r"
uniform float u_pixel_ratio;
// Size of a page of the atlas, texels
uniform vec2 u_atlas_size;
uniform highp sampler2D u_labels;
uniform highp sampler2D u_visible;
layout(location = 0) in uint a_label;
layout(location = 1) in vec4 a_rect;
layout(location = 2) in vec4 a_uv;
layout(location = 3) in float a_scale;
out vec2 v_uv;
// Page of the atlas
flat out float v_page;
flat out vec4 v_fill;
flat out vec4 v_halo;
// Halo edge in distance units, and the font scale
flat out vec2 v_edges;

void main() {
  // Placed this frame: shown, its angle on screen and its scale (see `place`)
  vec4 state = texelFetch(u_visible, table_texel(a_label), 0);
  if (state.x < 0.5) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
  float scale = state.z;
  vec2 offset = texelFetch(u_labels, table_texel(a_label * 4u), 0).xy;
  v_fill = texelFetch(u_labels, table_texel(a_label * 4u + 1u), 0);
  v_halo = texelFetch(u_labels, table_texel(a_label * 4u + 2u), 0);
  float halo_width = texelFetch(u_labels, table_texel(a_label * 4u + 3u), 0).x;
  vec2 merc = u_origin + offset;
  vec4 c = u_matrix * vec4(project_up(plane_of(offset), merc, altitude(merc)), 1.0);
  c.z -= u_depth_bias;
  vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
  vec2 q = (a_rect.xy + corner * a_rect.zw) * scale;
  float cs = cos(state.y), sn = sin(state.y);
  vec2 px = vec2(q.x * cs - q.y * sn, q.x * sn + q.y * cs) * u_pixel_ratio;
  // Screen y runs down, clip y up
  c.xy += vec2(px.x, -px.y) / u_viewport * c.w;
  gl_Position = c;
  // The pages are stacked in the atlas coordinates: the page from the top of the glyph (half a texel in, safe from
  // rounding), then the place in it
  float page = floor((a_uv.y + 0.5) / u_atlas_size.y);
  v_page = page;
  v_uv = (mix(a_uv.xy, a_uv.zw, corner) - vec2(0.0, page * u_atlas_size.y)) / u_atlas_size;
  float font = a_scale * scale;
  v_edges = vec2(0.75 - halo_width / (font * 8.0), font);
}";

const FRAGMENT: &str = r"
uniform mediump sampler2DArray u_atlas;
uniform float u_opacity;
in vec2 v_uv;
flat in float v_page;
flat in vec4 v_fill;
flat in vec4 v_halo;
flat in vec2 v_edges;
out vec4 color;

void main() {
  float d = texture(u_atlas, vec3(v_uv, v_page)).r;
  float g = max(fwidth(d) * 0.7, 1e-3);
  float fill = smoothstep(0.75 - g, 0.75 + g, d);
  float halo = v_edges.x < 0.75 ? smoothstep(v_edges.x - g, v_edges.x + g, d) : 0.0;
  float a_fill = v_fill.a * fill;
  float a_halo = v_halo.a * halo * (1.0 - a_fill);
  float a = a_fill + a_halo;
  if (a <= 0.0) discard;
  color = vec4((v_fill.rgb * a_fill + v_halo.rgb * a_halo) / a, a * u_opacity);
}";

const UNIFORMS: [&str; 6] = [
    "u_pixel_ratio",
    "u_atlas_size",
    "u_labels",
    "u_visible",
    "u_atlas",
    "u_opacity",
];

/// Floats per glyph quad from the page: label, rect (x, y, w, h, CSS pixels from the anchor at the shaped size), atlas
/// rect (u0, v0, u1, v1, texels; v counts the rows of the pages before the glyph's), font scale (shaped size / 24)
pub const QUAD_FLOATS: usize = 10;
/// Floats per label from the page: fill RGBA, halo RGBA, halo width (CSS pixels), flags (1: always shown, never
/// hiding others), direction (Mercator; 0, 0: none, the text stays horizontal; else it runs along it, kept upright),
/// then its scale as zoom stops (four zooms, four values: displayed size over shaped size)
pub const LABEL_FLOATS: usize = 20;
const LABELS_UNIT: u32 = unit::OWN[0];
const VISIBLE_UNIT: u32 = unit::OWN[1];
const ATLAS_UNIT: u32 = unit::OWN[2];
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

/// Labels as `SymbolLayer::set_labels` takes them, ready to upload
struct Built {
    labels: Vec<Label>,
    /// Per label: offset, fill, halo, halo width (`gl::table_rows` rows of RGBA32F texels)
    table: Vec<f32>,
    quads: Vec<Quad>,
    min: DVec2,
    max: DVec2,
    origin: DVec2,
}

/// The labels of `SymbolLayer::set_labels`, from arrays of any lengths (as many labels as all three describe) and
/// quads of any label numbers (those of no label left out)
fn build(anchors: &[f64], boxes: &[f32], styles: &[f32], quads: &[f32]) -> Built {
    let n = (anchors.len() / 2)
        .min(boxes.len() / 4)
        .min(styles.len() / LABEL_FLOATS);
    let mut merc = overlay::points(anchors);
    merc.truncate(n);
    let (min, max) = overlay::bounds(&merc).unwrap_or((DVec2::ZERO, DVec2::ZERO));
    let origin = (min + max) / 2.0;
    let labels = merc
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
    // Per label: offset, fill, halo, halo width
    let rows = gl::table_rows(n * 4);
    let mut table = vec![0f32; rows * WIDTH * 4];
    for (i, m) in merc.iter().enumerate() {
        let o = (*m - origin).as_vec2();
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
    Built {
        labels,
        table,
        quads: data,
        min,
        max,
        origin,
    }
}

pub struct SymbolLayer {
    pub style: SymbolStyle,
    overlay: Overlay,
    uniforms: [Option<glow::UniformLocation>; 6],
    vao: glow::VertexArray,
    quads: glow::Buffer,
    quad_count: i32,
    /// Per label: offset, fill, halo, halo width (4 texels); its state this frame (`visible`). Kept from one set of
    /// labels to the next while they fit (`gl::Table`): new labels come several times a second while the map moves
    labels_table: Table,
    visible_table: Table,
    labels: Vec<Label>,
    /// Per label, this frame: shown (1 or 0), angle on screen (radians, clockwise), scale, unused
    visible: Vec<f32>,
    /// The states of the placement before, to tell whether they changed (a buffer kept between placements)
    previous: Vec<f32>,
    /// What the last placement depended on (`PlaceKey`), and the boxes it took in the collision grid: an unchanged
    /// placement takes them again without working anything out
    placed: Option<(PlaceKey, Vec<collision::Rect>)>,
    /// The state texture holds `visible` (no upload needed)
    uploaded: bool,
    origin: DVec2,
    min: DVec2,
    max: DVec2,
    drape: Drape,
}

impl SymbolLayer {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        // Everything created is deleted again if a step fails
        let mut objects = Objects::new(gl);
        let overlay = Overlay::new(&mut objects, gl, &[VERTEX], &[FRAGMENT])?;
        let program = overlay.program;
        let vao = objects.vertex_array()?;
        let quads = objects.buffer()?;
        let labels_table = Table::new(gl, glow::RGBA32F, 0)?;
        objects.adopt_texture(labels_table.texture);
        let visible_table = Table::new(gl, glow::RGBA32F, 0)?;
        objects.adopt_texture(visible_table.texture);
        // SAFETY: state setup of objects just created on a live context
        unsafe {
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
        }
        objects.keep();
        let uniforms = gl::uniforms(gl, program, UNIFORMS);
        gl::sampler_units(
            gl,
            program,
            &[
                (uniforms[2].as_ref(), LABELS_UNIT),
                (uniforms[3].as_ref(), VISIBLE_UNIT),
                (uniforms[4].as_ref(), ATLAS_UNIT),
            ],
        );
        Ok(Self {
            style: SymbolStyle {
                size: [[0.0; 4], [1.0; 4]],
                opacity: [[0.0; 4], [1.0; 4]],
                min_zoom: 0.0,
                max_zoom: 24.0,
                visible: true,
            },
            overlay,
            uniforms,
            vao,
            quads,
            quad_count: 0,
            labels_table,
            visible_table,
            labels: Vec::new(),
            visible: Vec::new(),
            previous: Vec::new(),
            placed: None,
            uploaded: false,
            origin: DVec2::ZERO,
            min: DVec2::ZERO,
            max: DVec2::ZERO,
            drape: Drape::new(),
        })
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
        let Built {
            labels,
            table,
            quads: data,
            min,
            max,
            origin,
        } = build(anchors, boxes, styles, quads);
        let n = labels.len();
        let rows = gl::table_rows(n * 4);
        // Larger tables first if they do not fit: if one cannot be made, the layer keeps its labels
        let labels_table = self.labels_table.prepare(gl, n * 4)?;
        let visible_table = match self.visible_table.prepare(gl, n) {
            Ok(made) => made,
            Err(e) => {
                if let Some((texture, _)) = labels_table {
                    // SAFETY: a texture just made, not used
                    unsafe { gl.delete_texture(texture) };
                }
                return Err(e);
            }
        };
        self.labels_table.replace(gl, labels_table);
        self.visible_table.replace(gl, visible_table);
        // SAFETY: uploads into objects owned by this layer
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.labels_table.texture));
            gl::upload_table(gl, bytes(&table), rows * WIDTH, glow::RGBA, glow::FLOAT, 16);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.quads));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&data), glow::STATIC_DRAW);
        }
        (self.min, self.max, self.origin) = (min, max, origin);
        self.labels = labels;
        self.visible = vec![0.0; n * 4];
        self.placed = None;
        self.uploaded = false;
        self.quad_count = data.len() as i32;
        self.drape.delete(gl);
        self.drape = Drape::new();
        Ok(())
    }

    fn shows(&self, zoom: f64) -> bool {
        self.style.visible && zoom >= self.style.min_zoom && zoom < self.style.max_zoom && self.quad_count > 0
    }
}

/// What a placement depends on: the camera (and the screen), the labels, the relief under them, the style, and the
/// collision grid as the layers above left it
#[derive(Clone, Copy, PartialEq, Eq)]
struct PlaceKey {
    camera: [u64; 9],
    relief: (u64, u32),
    style: [u32; 8],
    grid: u64,
}

impl PlaceKey {
    fn new(frame: &Frame, style: &SymbolStyle, grid: &collision::Grid) -> Self {
        let c = frame.camera;
        Self {
            camera: [
                c.center.x,
                c.center.y,
                c.zoom,
                c.pitch,
                c.bearing,
                c.width,
                c.height,
                c.elevation,
                f64::from(u8::from(c.globe)),
            ]
            .map(f64::to_bits),
            relief: frame
                .relief
                .map_or((0, 0), |(source, e)| (source.generation(), e.to_bits())),
            style: [
                style.size[0][0],
                style.size[0][1],
                style.size[0][2],
                style.size[0][3],
                style.size[1][0],
                style.size[1][1],
                style.size[1][2],
                style.size[1][3],
            ]
            .map(f32::to_bits),
            grid: grid.fingerprint(),
        }
    }
}

/// The box of a label on screen around its anchor `at`: its rect (CSS pixels at the shaped size) scaled and turned by
/// `angle` (radians, clockwise on screen), as an axis-aligned box
fn label_box(at: DVec2, rect: [f32; 4], scale: f64, angle: f64) -> collision::Rect {
    let r = rect.map(|v| f64::from(v) * scale);
    let (cs, sn) = (angle.cos(), angle.sin());
    let (mut lo, mut hi) = (DVec2::INFINITY, DVec2::NEG_INFINITY);
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let p = DVec2::new(x * cs - y * sn, x * sn + y * cs);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    [at.x + lo.x, at.y + lo.y, at.x + hi.x, at.y + hi.y]
}

/// Angle on screen of a line from `at` to `ahead`, kept upright (turned half a turn if upside down)
fn upright(at: DVec2, ahead: DVec2) -> f64 {
    let d = ahead - at;
    let mut angle = d.y.atan2(d.x);
    if angle > std::f64::consts::FRAC_PI_2 {
        angle -= std::f64::consts::PI;
    } else if angle < -std::f64::consts::FRAC_PI_2 {
        angle += std::f64::consts::PI;
    }
    angle
}

impl Layer for SymbolLayer {
    fn place(&mut self, frame: &Frame, grid: &mut collision::Grid) {
        if !self.shows(frame.camera.zoom) {
            self.placed = None;
            if self.visible.iter().any(|&v| v != 0.0) {
                self.visible.fill(0.0);
                self.uploaded = false;
            }
            return;
        }
        // Nothing changed since the last placement: the same boxes again, the same states
        let key = PlaceKey::new(frame, &self.style, grid);
        if let Some((last, boxes)) = &self.placed
            && *last == key
        {
            for &r in boxes {
                grid.insert(r);
            }
            return;
        }
        // The states of the last placement kept aside, its buffer reused for the new ones
        std::mem::swap(&mut self.visible, &mut self.previous);
        self.visible.clear();
        self.visible.resize(self.previous.len(), 0.0);
        let mut boxes = self.placed.take().map(|(_, b)| b).unwrap_or_default();
        boxes.clear();
        let (camera, view) = (frame.camera, frame.view);
        let layer_scale = stops(&self.style.size, camera.zoom);
        // Screen point and distance to the eye (pixels of the local frame); none past the horizon of the globe, hidden
        // by the body (as `Engine::project`)
        let screen = |m: DVec2, altitude: f64| -> Option<(DVec2, f64)> {
            let shift = -(m.x - camera.center.x).round();
            let ground = view.position(m, shift);
            if view.globe.factor > 0.0 && !view.faces_camera(ground) {
                return None;
            }
            let p = ground + glam::DVec3::Z * altitude * view.vertical;
            let clip = view.view_proj * DVec4::new(p.x, p.y, p.z, 1.0);
            (clip.w > 0.0)
                .then(|| {
                    let at = DVec2::new(
                        f64::midpoint(clip.x / clip.w, 1.0) * camera.width,
                        f64::midpoint(-clip.y / clip.w, 1.0) * camera.height,
                    );
                    (at, clip.w)
                })
                // An anchor that is not a number is nowhere
                .filter(|(at, _)| at.is_finite())
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
            // Along its line, kept upright: the angle of the line on screen
            let mut angle = 0.0;
            if label.direction != DVec2::ZERO {
                // A step of one pixel along the line
                let step = label.direction.normalize() / view.world_size;
                if let Some((ahead, _)) = screen(label.merc + step, altitude) {
                    angle = upright(at, ahead);
                }
            }
            let rect = label_box(at, label.rect, scale, angle);
            if label.always || grid.place(rect) {
                if !label.always {
                    boxes.push(rect);
                }
                *state = [1.0, angle as f32, scale as f32, 0.0];
            }
        }
        self.uploaded &= self.visible == self.previous;
        self.placed = Some((key, boxes));
    }

    fn draw(&mut self, frame: &Frame) -> bool {
        let &Frame {
            gl,
            camera,
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
        if !self.uploaded {
            // The states changed: one upload
            // SAFETY: binding the state texture owned by this layer, for the upload inside its storage
            unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(self.visible_table.texture)) };
            gl::upload_table(gl, bytes(&self.visible), self.labels.len(), glow::RGBA, glow::FLOAT, 16);
            self.uploaded = true;
        }
        let [ratio, atlas_size_u, _, _, _, opacity] = &self.uniforms;
        self.overlay.begin(frame, self.origin, DEPTH_BIAS, &self.drape);
        // SAFETY: draw with objects owned by this layer and the engine's glyph atlas
        unsafe {
            gl.uniform_1_f32(ratio.as_ref(), pixel_ratio as f32);
            gl.uniform_2_f32(atlas_size_u.as_ref(), atlas_size.0 as f32, atlas_size.1 as f32);
            gl.uniform_1_f32(opacity.as_ref(), stops(&self.style.opacity, camera.zoom) as f32);
            // The sampler units are set once, by `SymbolLayer::new`
            for (unit, target, texture) in [
                (LABELS_UNIT, glow::TEXTURE_2D, self.labels_table.texture),
                (VISIBLE_UNIT, glow::TEXTURE_2D, self.visible_table.texture),
                (ATLAS_UNIT, glow::TEXTURE_2D_ARRAY, atlas),
            ] {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(target, Some(texture));
            }
            gl.active_texture(glow::TEXTURE0);
            let _depth = DepthState::over_ground(gl, relief.is_some());
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, self.quad_count);
            gl.bind_vertex_array(None);
        }
        false
    }

    fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.overlay.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.quads);
        }
        self.labels_table.delete(gl);
        self.visible_table.delete(gl);
        self.drape.delete(gl);
    }
}

/// The glyph atlas of an engine: signed distances, one byte per texel, in pages of one size (a texture array)
pub struct GlyphAtlas {
    pub texture: glow::Texture,
    /// Size of a page
    pub size: (i32, i32),
    pub pages: i32,
}

/// Most pages of the glyph atlas (the least `MAX_ARRAY_TEXTURE_LAYERS` of WebGL2)
const MAX_PAGES: i32 = 256;

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
        if let Some((layer, _)) = self.layer_mut::<SymbolLayer>(id) {
            if let Some(size) = stops_of(size) {
                layer.style.size = size;
            }
            if let Some(opacity) = stops_of(opacity) {
                layer.style.opacity = opacity;
            }
            layer.style.min_zoom = min_zoom;
            layer.style.max_zoom = max_zoom;
            layer.style.visible = visible;
        }
    }

    /// The glyph atlas shared by the symbol layers, one page: `width` × `height` bytes of signed distances
    /// (allocated again, filled with `data` if given)
    #[wasm_bindgen(js_name = setGlyphAtlas)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_glyph_atlas(&mut self, width: i32, height: i32, data: Option<Box<[u8]>>) -> Result<(), JsValue> {
        self.set_glyph_pages(width, height, 1)?;
        if let Some(data) = data {
            self.update_glyph_page(0, 0, 0, width, height, &data);
        }
        Ok(())
    }

    /// The glyph atlas shared by the symbol layers, `pages` pages of `width` × `height` bytes of signed distances,
    /// allocated again and empty (`updateGlyphPage` fills them). A glyph on page `p` at row `v` has the atlas
    /// coordinate `v + p × height`
    #[wasm_bindgen(js_name = setGlyphPages)]
    pub fn set_glyph_pages(&mut self, width: i32, height: i32, pages: i32) -> Result<(), JsValue> {
        gl::area(width, height)
            .filter(|_| width > 0 && height > 0 && (1..=MAX_PAGES).contains(&pages))
            .ok_or("glyph atlas: a positive size and 1 to 256 pages expected")?;
        // SAFETY: (re)allocation of the atlas on the live context
        unsafe {
            let gl = &self.gl;
            if let Some(old) = self.glyphs.take() {
                gl.delete_texture(old.texture);
            }
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(texture));
            gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY, 1, glow::R8, width, height, pages);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, key, value as i32);
            }
            self.glyphs = Some(GlyphAtlas {
                texture,
                size: (width, height),
                pages,
            });
        }
        Ok(())
    }

    /// A region of the first page of the glyph atlas: `width` × `height` bytes at (`x`, `y`)
    #[wasm_bindgen(js_name = updateGlyphAtlas)]
    pub fn update_glyph_atlas(&mut self, x: i32, y: i32, width: i32, height: i32, data: &[u8]) {
        self.update_glyph_page(0, x, y, width, height, data);
    }

    /// A region of a page of the glyph atlas: `width` × `height` bytes at (`x`, `y`) of the page
    #[wasm_bindgen(js_name = updateGlyphPage)]
    pub fn update_glyph_page(&mut self, page: i32, x: i32, y: i32, width: i32, height: i32, data: &[u8]) {
        let Some(atlas) = &self.glyphs else {
            return;
        };
        let Some(len) = gl::area(width, height) else {
            return;
        };
        if data.len() < len || !(0..atlas.pages).contains(&page) || !gl::region_fits(x, y, width, height, atlas.size) {
            return;
        }
        // SAFETY: upload inside a page of the atlas
        unsafe {
            let gl = &self.gl;
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(atlas.texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_3d(
                glow::TEXTURE_2D_ARRAY,
                0,
                x,
                y,
                page,
                width,
                height,
                1,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&data[..len])),
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    #[test]
    fn label_boxes_turn_with_the_text() {
        let at = DVec2::new(100.0, 50.0);
        let rect = [-20.0, -5.0, 20.0, 5.0];
        let flat = label_box(at, rect, 1.0, 0.0);
        assert_eq!(flat, [80.0, 45.0, 120.0, 55.0]);
        // Twice as large
        assert_eq!(label_box(at, rect, 2.0, 0.0), [60.0, 40.0, 140.0, 60.0]);
        // A quarter turn: the box stands up
        let turned = label_box(at, rect, 1.0, FRAC_PI_2);
        for (a, b) in turned.iter().zip([95.0, 30.0, 105.0, 70.0]) {
            assert!((a - b).abs() < 1e-9, "{turned:?}");
        }
    }

    #[test]
    fn text_along_a_line_stays_upright() {
        let o = DVec2::ZERO;
        assert_eq!(upright(o, DVec2::new(1.0, 0.0)), 0.0);
        // Running right to left: turned half a turn, read left to right
        assert!(upright(o, DVec2::new(-1.0, 0.0)).abs() < 1e-12);
        assert!((upright(o, DVec2::new(1.0, 1.0)) - PI / 4.0).abs() < 1e-12);
        assert!((upright(o, DVec2::new(-1.0, -1.0)) - PI / 4.0).abs() < 1e-12);
        assert!(upright(o, DVec2::new(0.0, -1.0)).abs() <= FRAC_PI_2);
    }

    #[test]
    fn malformed_labels_do_not_panic() {
        let style = [1.0f32; LABEL_FLOATS];
        // Two anchors (one not a number), one box, two styles: one label
        let built = build(
            &[0.5, 0.5, f64::NAN, 0.2],
            &[0.0, 0.0, 10.0, 5.0, 1.0],
            &[style, style].concat(),
            &[],
        );
        assert_eq!(built.labels.len(), 1);
        assert_eq!(built.table.len(), gl::table_rows(4) * WIDTH * 4);
        assert!(built.origin.is_finite());
        // Quads of no label (past it, negative, not a number: 0 as usize, the first) and a partial one
        let mut quads = vec![0f32; QUAD_FLOATS * 4 + 3];
        quads[QUAD_FLOATS] = 1.0;
        quads[QUAD_FLOATS * 2] = -5.0;
        quads[QUAD_FLOATS * 3] = f32::NAN;
        let built = build(&[0.5, 0.5], &[0.0; 4], &style, &quads);
        assert_eq!(built.quads.len(), 3);
        assert!(built.quads.iter().all(|q| q.label == 0));
        let none = build(&[], &[], &[], &quads);
        assert!(none.labels.is_empty() && none.quads.is_empty());
    }
}
