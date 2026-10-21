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

mod shaders;

use crate::engine::Engine;
use crate::js;
use crate::layer::{Frame, Layer};
use crate::render::drape::Drape;
use crate::render::features::{stops, stops_of};
use crate::render::gl::{self, DepthState, Objects, RAMP_TEXELS, bytes as as_bytes, unit};
use crate::render::overlay::{
    self, LINE, LINE_DEPTH_BIAS, Overlay, PIECE_ATTRIBUTES, PIECE_STRIDE as STRIDE, RELIEF_SEGMENTS,
};
use geo::pieces::{self, Grid, Lines, Visible};
use glam::DVec2;
use glow::HasContext;
use shaders::{FRAGMENT, PICK_FRAGMENT, VALUE, VERTEX};
use std::cell::Cell;
use wasm_bindgen::prelude::*;

/// Stored for a closed way: Infinity does not survive every driver's float textures (some read it back as 0); the
/// shader takes values and costs from 1e29 up as unreachable
const CLOSED: f32 = 1e30;

/// Extra width of the lines in the picking pass, CSS pixels: thin lines stay easy to hover
const PICK_SLOP: f64 = 6.0;

/// Uniforms of a pass, by name (besides `Overlay`'s)
const UNIFORMS: [&str; 15] = [
    "u_width",
    "u_values",
    "u_max",
    "u_contour",
    "u_opacity",
    "u_layer",
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
/// Index of `u_layer` in `UNIFORMS`
const LAYER_UNIFORM: usize = 5;

/// A program with its uniforms
struct Pass {
    overlay: Overlay,
    uniforms: [Option<glow::UniformLocation>; 15],
}

impl Pass {
    fn new(objects: &mut Objects, gl: &glow::Context, defines: &str, fragment: &str) -> Result<Self, String> {
        let overlay = Overlay::new(objects, gl, &[defines, LINE, VERTEX], &[defines, VALUE, fragment])?;
        let program = overlay.program;
        let uniforms = gl::uniforms(gl, program, UNIFORMS);

        gl::sampler_units(
            gl,
            program,
            &[
                (uniforms[1].as_ref(), VALUES_UNIT),
                (uniforms[8].as_ref(), EDGES_UNIT),
                (uniforms[9].as_ref(), COSTS_UNIT),
                (uniforms[11].as_ref(), SCALES_UNIT),
                (uniforms[12].as_ref(), RAMP_UNIT),
            ],
        );

        Ok(Self { overlay, uniforms })
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

/// Texture units of the layer's own textures
const VALUES_UNIT: u32 = unit::OWN[0];
const EDGES_UNIT: u32 = unit::OWN[1];
const COSTS_UNIT: u32 = unit::OWN[2];
const SCALES_UNIT: u32 = unit::OWN[3];
const RAMP_UNIT: u32 = unit::OWN[4];

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
    /// Width factor of each edge (R32F), colors of the values (`RAMP_TEXELS` RGBA)
    scales: glow::Texture,
    ramp: glow::Texture,
    with_costs: bool,
    /// Mercator origin of the piece positions
    origin: DVec2,
    /// Squares of the network and the pieces of each
    grid: Grid,
    start: Vec<u32>,
    /// The squares in view and their pieces, kept from frame to frame
    visible: Visible,
    drape: Drape,
    nodes: usize,
    edge_count: usize,
    piece_count: usize,
    /// Pieces drawn in the last frame
    drawn: Cell<u32>,
}

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

        check_edges(edges, nodes)?;

        let edge_count = edges.len() / 2;

        overlay::pick_ids_fit(edge_count, "network layer");

        // The points as they came, no copy (millions for a region): pairs of f64 are a DVec2
        let merc: &[DVec2] = bytemuck::cast_slice(&mercator[..nodes * 2]);
        let lines = lines.map(|(start, line)| Lines {
            start,
            points: bytemuck::cast_slice(&line[..line.len() / 2 * 2]),
        });
        let origin = overlay::bounds(merc).map_or(DVec2::ZERO, |(min, max)| (min + max) / 2.0);
        let built = pieces::build(merc, edges, lines.as_ref(), origin);
        // Everything created is deleted again if a step fails
        let mut objects = Objects::new(gl);
        let shaded = Pass::new(&mut objects, gl, "", FRAGMENT)?;
        let pick = Pass::new(&mut objects, gl, "#define PICK\n", PICK_FRAGMENT)?;
        let values = objects.adopt_texture(gl::table_texture(gl, glow::R32F, nodes)?);
        let edge_texture = objects.adopt_texture(gl::table_texture(gl, glow::RG32UI, edge_count)?);

        gl::upload_table(gl, as_bytes(edges), edge_count, glow::RG_INTEGER, glow::UNSIGNED_INT, 8);

        let cost_texture = objects.adopt_texture(gl::table_texture(gl, glow::RG32F, edge_count)?);
        let scales = objects.adopt_texture(gl::table_texture(gl, glow::R32F, edge_count)?);
        let ramp = objects.texture()?;
        let vao = objects.vertex_array()?;
        let buffer = objects.buffer()?;

        // SAFETY: storage, parameters and uploads of objects just created on a live WebGL2 context
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(ramp));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, RAMP_TEXELS as i32, 1);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, key, value as i32);
            }
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&built.pieces), glow::STATIC_DRAW);
            for location in 0..3 {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_divisor(location, 1);
            }
            gl.bind_vertex_array(None);
        }
        objects.keep();

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
            visible: Visible::default(),
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

    /// One value per node, in node order; negative (or 1e30 and above) = unreached
    pub fn set_values(&mut self, gl: &glow::Context, values: &[f32]) {
        let n = values.len().min(self.nodes);

        // SAFETY: binding a texture owned by this layer, for the upload inside its storage
        unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(self.values)) };
        gl::upload_table(gl, as_bytes(&values[..n]), n, glow::RED, glow::FLOAT, 4);
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

        // SAFETY: binding a texture owned by this layer, for the upload inside its storage
        unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(self.costs)) };
        gl::upload_table(gl, as_bytes(&clamped), clamped.len() / 2, glow::RG, glow::FLOAT, 8);
    }

    /// Width factor of each edge (1 for all without)
    pub fn set_scales(&mut self, gl: &glow::Context, scales: Option<&[f32]>) {
        let mut all = vec![1.0f32; self.edge_count.max(1)];

        if let Some(s) = scales {
            let n = s.len().min(all.len());

            all[..n].copy_from_slice(&s[..n]);
        }
        // SAFETY: binding a texture owned by this layer, for the upload inside its storage
        unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(self.scales)) };
        gl::upload_table(gl, as_bytes(&all), self.edge_count, glow::RED, glow::FLOAT, 4);
    }

    /// Colors of the values from 0 to the maximum: `RAMP_TEXELS` RGBA texels; None: the default ramp, green to red
    pub fn set_ramp(&mut self, gl: &glow::Context, rgba: Option<&[u8]>) {
        let default = default_ramp();
        let data = rgba.filter(|r| r.len() >= RAMP_TEXELS * 4).unwrap_or(&default);

        // SAFETY: upload of the whole ramp texture
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.ramp));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                RAMP_TEXELS as i32,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&data[..RAMP_TEXELS * 4])),
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

    /// Sets up a pass for this layer: its program, the view, the style but the width, the textures, the pieces
    fn bind(&self, frame: &Frame, pass: &Pass) {
        let gl = frame.gl;
        let [
            _,
            _,
            max,
            contour,
            _,
            _,
            segments,
            with_costs,
            _,
            _,
            _,
            _,
            _,
            unreached,
            fade,
        ] = &pass.uniforms;
        let style = &self.style;

        pass.overlay.begin(frame, self.origin, LINE_DEPTH_BIAS, &self.drape);

        // SAFETY: state setup with objects owned by this layer (the sampler units set once, by `Pass::new`)
        unsafe {
            let parts = if frame.relief.is_some() { RELIEF_SEGMENTS } else { 1 };

            gl.uniform_1_i32(segments.as_ref(), parts);
            gl.uniform_1_f32(max.as_ref(), style.max);
            gl.uniform_1_f32(contour.as_ref(), style.contour);
            gl.uniform_1_i32(with_costs.as_ref(), i32::from(self.with_costs));
            gl.uniform_4_f32_slice(unreached.as_ref(), &style.unreached);
            gl.uniform_1_f32(fade.as_ref(), style.fade);
            for (unit, texture) in [
                (VALUES_UNIT, self.values),
                (EDGES_UNIT, self.edges),
                (COSTS_UNIT, self.costs),
                (SCALES_UNIT, self.scales),
                (RAMP_UNIT, self.ramp),
            ] {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.pieces));
        }
    }

    /// Draws the pieces of `ranges` at a width, the pass set up by `bind` (the halo then the lines share it: only
    /// their width and opacity change)
    fn draw_ranges(&self, frame: &Frame, pass: &Pass, width: Width, ranges: &[(u32, u32)]) {
        let [width_u, _, _, _, opacity, _, _, _, _, _, halo, ..] = &pass.uniforms;
        let (gl, pixel_ratio) = (frame.gl, frame.pixel_ratio);
        let style = &self.style;
        let parts = if frame.relief.is_some() { RELIEF_SEGMENTS } else { 1 };

        // SAFETY: draws with the objects bound by `bind`
        unsafe {
            gl.uniform_1_f32(width_u.as_ref(), (width.pixels * pixel_ratio) as f32);

            let alpha = if width.halo { style.halo_opacity } else { style.opacity };

            gl.uniform_1_f32(opacity.as_ref(), alpha);
            gl.uniform_1_f32(halo.as_ref(), if width.halo { 1.0 } else { 0.0 });
        }
        gl::draw_instances(gl, &PIECE_ATTRIBUTES, STRIDE, 2 * (parts + 1), ranges);
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

        let look = overlay::look(self.origin, view, relief, 0.0);
        let mut visible = std::mem::take(&mut self.visible);
        let ranges = visible.update(&self.grid, &self.start, view, look);

        self.drawn.set(ranges.iter().map(|(a, b)| b - a).sum());

        // Line quads are built on screen, either way round
        let depth = DepthState::over_ground(gl, relief.is_some());
        let style = self.style;

        if !ranges.is_empty() {
            self.bind(frame, &self.shaded);

            let halo_width = stops(&style.halo, camera.zoom);

            if halo_width > 0.0 && style.halo_opacity > 0.0 {
                let halo = Width {
                    pixels: halo_width,
                    halo: true,
                };

                self.draw_ranges(frame, &self.shaded, halo, ranges);
            }

            let lines = Width {
                pixels: stops(&style.width, camera.zoom),
                halo: false,
            };

            self.draw_ranges(frame, &self.shaded, lines, ranges);
            // SAFETY: unbinding on the live context
            unsafe { gl.bind_vertex_array(None) };
        }
        self.visible = visible;
        drop(depth);

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
            relief,
            ..
        } = frame;

        // SAFETY: uniform on the picking program
        unsafe {
            gl.use_program(Some(self.pick.overlay.program));
            gl.uniform_1_f32(self.pick.uniforms[LAYER_UNIFORM].as_ref(), f32::from(layer));
        }

        let look = overlay::look(self.origin, view, relief, 0.0);
        let ranges = overlay::pick_ranges(&self.visible, &self.grid, &self.start, view, look);

        if ranges.is_empty() {
            return;
        }

        let width = Width {
            pixels: stops(&self.style.width, camera.zoom) + PICK_SLOP,
            halo: false,
        };
        // Hidden by the hills as on screen (the picking pass drew the ground's depth)
        let _depth = DepthState::over_ground(gl, relief.is_some());

        self.bind(frame, &self.pick);
        self.draw_ranges(frame, &self.pick, width, &ranges);
        // SAFETY: unbinding on the live context
        unsafe { gl.bind_vertex_array(None) };
    }

    fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.shaded.overlay.program);
            gl.delete_program(self.pick.overlay.program);
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

/// Edges as node index pairs: an even count, every node among the `nodes`
fn check_edges(edges: &[u32], nodes: usize) -> Result<(), String> {
    if !edges.len().is_multiple_of(2) {
        return Err(format!("edges: node pairs expected, {} numbers", edges.len()));
    }
    if let Some(bad) = edges.iter().find(|&&i| i as usize >= nodes) {
        return Err(format!("edge to node {bad}, only {nodes} nodes"));
    }

    Ok(())
}

/// The ramp used without one given: `DEFAULT_RAMP` over `RAMP_TEXELS` RGBA texels
fn default_ramp() -> Vec<u8> {
    let mut rgba = Vec::with_capacity(RAMP_TEXELS * 4);

    for i in 0..RAMP_TEXELS {
        let t = i as f32 / (RAMP_TEXELS - 1) as f32 * 4.0;
        let k = (t.floor() as usize).min(3);
        let f = t - k as f32;
        let (a, b) = (DEFAULT_RAMP[k], DEFAULT_RAMP[k + 1]);

        for c in 0..3 {
            rgba.push(((a[c] + (b[c] - a[c]) * f) * 255.0).round() as u8);
        }
        rgba.push(255);
    }

    rgba
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_read_25_floats() {
        let mut f: Vec<f32> = (0..25).map(|i| i as f32).collect();
        let s = NetworkStyle::from_floats(&f).unwrap();

        assert_eq!(s.width, [[0.0, 1.0, 2.0, 3.0], [4.0, 5.0, 6.0, 7.0]]);
        assert_eq!(s.halo[1], [12.0, 13.0, 14.0, 15.0]);
        assert_eq!((s.max, s.contour, s.opacity, s.halo_opacity), (16.0, 17.0, 18.0, 19.0));
        assert_eq!(s.unreached, [20.0, 21.0, 22.0, 23.0]);
        assert_eq!(s.fade, 24.0);
        f.pop();
        assert!(NetworkStyle::from_floats(&f).is_none());
    }

    #[test]
    fn edges_are_checked_before_any_upload() {
        assert!(check_edges(&[0, 1, 1, 2], 3).is_ok());
        assert!(check_edges(&[0, 1, 2], 3).is_err());
        assert!(check_edges(&[0, 3], 3).is_err());
        // An odd count past a full row of the texture (it used to panic)
        assert!(check_edges(&vec![0; 2 * 4096 + 1], 1).is_err());
    }

    #[test]
    fn the_default_ramp_runs_green_to_red() {
        let r = default_ramp();

        assert_eq!(r.len(), 1024);
        assert_eq!(&r[..4], &[0, 168, 107, 255]);
        assert_eq!(&r[1020..], &[232, 56, 79, 255]);
    }
}
