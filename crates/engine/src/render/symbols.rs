//! Symbol layer: text labels at points, drawn from signed distance field glyphs (in the format of `MapLibre`'s
//! glyph ranges), with a halo, and their icons (a style's sprite, the application's images). The page shapes the text
//! (`glyphs.ts`: lines, wrapping, anchors), lays the icons out (`icons.ts`: anchors, stretched to the text) and keeps
//! both atlases; the engine places the labels each frame, every symbol layer sharing one collision grid from the top
//! layer down (a label's text and icon showing only where nothing placed before them does, as `MapLibre`'s
//! `*-allow-overlap`, `*-ignore-placement` and `*-optional` say), and draws each glyph and icon as an instanced quad
//! facing the screen. In relief a label stands on the ground, and the hills hide it; on the globe, a label past its
//! horizon is not placed.
//!
//! The atlas has pages of one size (the layers of a texture array), so it never runs out: the atlas coordinates of a
//! glyph count its page's rows first (`v + page × height`, as if the pages were stacked), and the shader takes the
//! page back from them. Glyphs already placed keep their coordinates when a page is added.
//!
//! Submodules:
//! - `shaders`: the program, a glyph or an icon per instanced quad;
//! - `build`: the page's arrays turned into labels, their table and their quads;
//! - `placement`: where each label lands on screen and what of it fits in the collision grid;
//! - `atlas`: the glyph and icon atlases shared by the symbol layers.

pub(crate) mod atlas;
mod build;
mod placement;
mod shaders;

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::drape::Drape;
use crate::render::features::{stops, stops_of};
use crate::render::gl::{self, DepthState, Objects, TABLE_WIDTH as WIDTH, Table, bytes, unit};
use crate::render::overlay::Overlay;
use build::{Built, build};
use geo::collision;
use glam::DVec2;
use glow::HasContext;
use placement::PlaceKey;
use shaders::{FRAGMENT, UNIFORMS, VERTEX};
use wasm_bindgen::prelude::*;

/// Floats per quad from the page: label, rect (x, y, w, h, CSS pixels from the anchor at the shaped size), atlas rect
/// (u0, v0, u1, v1, texels; for a glyph, v counts the rows of the pages before the glyph's), font scale (shaped size /
/// 24; an icon: -1, or -2 for a signed distance field), turn about the anchor (radians, clockwise; icons)
pub const QUAD_FLOATS: usize = 11;
/// Floats per label from the page: fill RGBA, halo RGBA, halo width (CSS pixels), flags (`flag`), direction (Mercator;
/// 0, 0: none), the text's scale as zoom stops (four zooms, four values: displayed size over shaped size), the icon's
/// box (CSS pixels from the anchor at its shaped size), its color (RGBA, its opacity in alpha) and scale as zoom stops
pub const LABEL_FLOATS: usize = 36;
const LABELS_UNIT: u32 = unit::OWN[0];
const VISIBLE_UNIT: u32 = unit::OWN[1];
const ATLAS_UNIT: u32 = unit::OWN[2];
const ICONS_UNIT: u32 = unit::OWN[3];
/// Texels per label in the table: offset, fill, halo, halo width, icon color
const TEXELS: usize = 5;

/// Flags of a label (`MapLibre`'s layout properties of the same names)
pub mod flag {
    /// `text-allow-overlap`: the text shows even where it collides
    pub const TEXT_ALLOW_OVERLAP: u32 = 1;
    /// `text-ignore-placement`: the text takes no room, others may overlap it
    pub const TEXT_IGNORE_PLACEMENT: u32 = 2;
    /// `icon-allow-overlap`: the icon shows even where it collides
    pub const ICON_ALLOW_OVERLAP: u32 = 4;
    /// `icon-ignore-placement`: the icon takes no room, others may overlap it
    pub const ICON_IGNORE_PLACEMENT: u32 = 8;
    /// `text-optional`: the icon shows alone when the text collides
    pub const TEXT_OPTIONAL: u32 = 16;
    /// `icon-optional`: the text shows alone when the icon collides
    pub const ICON_OPTIONAL: u32 = 32;
    /// It has a text, an icon
    pub const TEXT: u32 = 64;
    /// It has an icon
    pub const ICON: u32 = 128;
    /// The text runs along the direction, kept upright
    pub const TEXT_ALONG: u32 = 256;
    /// The icon turns with the direction (`icon-rotation-alignment: map`)
    pub const ICON_ALONG: u32 = 512;
    /// The icon turns as its text does, kept upright with it (stretched around it, `icon-text-fit`)
    pub const ICON_UPRIGHT: u32 = 1024;
}

/// Labels in relief are pulled towards the camera by this fraction of their distance
const DEPTH_BIAS: f64 = 0.01;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Quad {
    label: u32,
    rect: [f32; 4],
    uv: [f32; 4],
    scale: f32,
    angle: f32,
}

struct Label {
    merc: DVec2,
    /// Its text's box, CSS pixels from the anchor at the shaped size
    rect: [f32; 4],
    /// Its icon's box, CSS pixels from the anchor at its shaped size
    icon: [f32; 4],
    /// `flag`
    flags: u32,
    /// Direction of the line it runs along (Mercator), zero for none
    direction: DVec2,
    /// Scales of its text and its icon as zoom stops
    size: [[f32; 4]; 2],
    icon_size: [[f32; 4]; 2],
}

/// Layer-wide style
#[derive(Clone, Copy)]
pub struct SymbolStyle {
    /// Size scale as zoom stops (zooms, then values: displayed size over shaped size)
    pub size: [[f32; 4]; 2],
    /// Opacity as zoom stops: zooms, then values
    pub opacity: [[f32; 4]; 2],
    /// Shown from this zoom on (inclusive)
    pub min_zoom: f64,
    /// Hidden from this zoom on (exclusive)
    pub max_zoom: f64,
    /// Shown at all (the application's toggle)
    pub visible: bool,
}

/// A symbol layer: labels (text and icon quads from the glyph and icon atlases), placed each frame against the
/// collision grid shared by the symbol layers, their state uploaded to a texture the shader reads
pub struct SymbolLayer {
    /// Layer-wide style, set by `setSymbolLayer`
    pub style: SymbolStyle,
    overlay: Overlay,
    uniforms: [Option<glow::UniformLocation>; 8],
    vao: glow::VertexArray,
    quads: glow::Buffer,
    quad_count: i32,
    /// Per label: offset, fill, halo, halo width, icon color (5 texels); its state this frame (`visible`). Kept from one set of
    /// labels to the next while they fit (`gl::Table`): new labels come several times a second while the map moves
    labels_table: Table,
    visible_table: Table,
    labels: Vec<Label>,
    /// Per label, this frame: what shows and turns (`shown`), angle on screen (radians, clockwise), text scale, icon
    /// scale
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
    /// An empty layer: no label until `set_labels`
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
            gl.enable_vertex_attrib_array(4);
            gl.vertex_attrib_pointer_f32(4, 1, glow::FLOAT, false, stride, 40);
            for location in 0..5 {
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
                (uniforms[7].as_ref(), ICONS_UNIT),
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
        let rows = gl::table_rows(n * TEXELS);
        // Larger tables first if they do not fit: if one cannot be made, the layer keeps its labels
        let labels_table = self.labels_table.prepare(gl, n * TEXELS)?;
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

impl Layer for SymbolLayer {
    fn place(&mut self, frame: &Frame, grid: &mut collision::Grid) {
        if !self.shows(frame.camera.zoom) {
            self.hide_all();

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
        self.place_labels(frame, grid, &mut boxes);
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
            icons,
            ..
        } = frame;

        if (glyphs.is_none() && icons.is_none()) || !self.shows(camera.zoom) {
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

        let [ratio, atlas_size_u, _, _, _, opacity, icon_size_u, _] = &self.uniforms;
        let atlas_size = glyphs.map_or((1, 1), |g| g.1);
        let icon_size = icons.map_or((1, 1), |i| i.1);

        self.overlay.begin(frame, self.origin, DEPTH_BIAS, &self.drape);
        // SAFETY: draw with objects owned by this layer and the engine's glyph atlas
        unsafe {
            gl.uniform_1_f32(ratio.as_ref(), pixel_ratio as f32);
            gl.uniform_2_f32(atlas_size_u.as_ref(), atlas_size.0 as f32, atlas_size.1 as f32);
            gl.uniform_1_f32(opacity.as_ref(), stops(&self.style.opacity, camera.zoom) as f32);
            gl.uniform_2_f32(icon_size_u.as_ref(), icon_size.0 as f32, icon_size.1 as f32);
            // The sampler units are set once, by `SymbolLayer::new`
            for (unit, target, texture) in [
                (LABELS_UNIT, glow::TEXTURE_2D, Some(self.labels_table.texture)),
                (VISIBLE_UNIT, glow::TEXTURE_2D, Some(self.visible_table.texture)),
                (ATLAS_UNIT, glow::TEXTURE_2D_ARRAY, glyphs.map(|g| g.0)),
                (ICONS_UNIT, glow::TEXTURE_2D, icons.map(|i| i.0)),
            ] {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(target, texture);
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
        self.overlay.delete(gl, self.vao, self.quads);
        self.labels_table.delete(gl);
        self.visible_table.delete(gl);
        self.drape.delete(gl);
    }
}

#[wasm_bindgen]
impl Engine {
    /// An empty symbol layer
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) when a WebGL object cannot be created or a shader fails to compile (the error carries its
    /// info log)
    #[wasm_bindgen(js_name = addSymbols)]
    pub fn add_symbols(&mut self) -> Result<u32, JsValue> {
        let layer = SymbolLayer::new(&self.gl)?;

        Ok(self.add_layer(Box::new(layer)))
    }

    /// Labels of a symbol layer, shaped by the page: anchors (Mercator, interleaved), their texts' boxes (min x, min y,
    /// max x, max y, CSS pixels from the anchor), styles (36 floats each, see `LABEL_FLOATS`) and glyph and icon quads
    /// (11 floats each, see `QUAD_FLOATS`)
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) when the labels need a table texture past the GPU's `MAX_TEXTURE_SIZE`, or
    /// when it cannot be created; the layer then keeps its labels
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
}
