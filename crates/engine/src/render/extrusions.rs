//! Extrusion layer: polygons raised into blocks (buildings), with depth, lit by the side they face and darker towards
//! their foot (`MapLibre`'s vertical gradient). Each block takes its feature's color, or its value through the layer's
//! ramp: water at a building, or the travel time to it read from a network layer's node values (two nodes and the
//! cost from each, per building), so that the buildings recolor with the network in the same frame, at no cost.
//!
//! The geometry comes in batches, added and removed by key: the tiles of a tiled source (one batch per tile, the
//! application loading them), or one batch for all. Each batch has its own feature textures; the ramp and the
//! layer-wide style are shared. In relief, a wall's foot stands on the ground under it and the roof at the given height
//! over the ground at the block's centroid. A translucent layer is drawn in two passes, depth then color, so that each
//! pixel shows its nearest face only.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::drape::{self, Drape, DrapeUniforms};
use crate::render::features::{FEATURES, FeatureUniforms, Features, LayerStyle};
use crate::render::gl::{self, Projection, bytes};
use crate::render::network::NetworkLayer;
use geo::shapes::{self, Polygons};
use glam::DVec2;
use glow::HasContext;
use std::collections::BTreeMap;
use wasm_bindgen::prelude::*;

const VERTEX: &str = r"
uniform mat4 u_matrix;
uniform vec2 u_plane_origin;
uniform vec2 u_origin;
uniform uint u_pick_base;
// Colored by a network's node values: two nodes and the costs to reach the block from them (RGBA32F per feature)
uniform bool u_linked;
uniform highp sampler2D u_anchors;
uniform highp sampler2D u_network;
uniform float u_linked_max;
layout(location = 0) in vec2 a_offset;
layout(location = 1) in vec2 a_centroid;
layout(location = 2) in uint a_feature;
layout(location = 3) in float a_level;
layout(location = 4) in float a_shade;
flat out vec4 v_color;
flat out uint v_id;
out float v_shade;
out float v_level;

float node(float i) {
  uint k = uint(i);
  float t = texelFetch(u_network, ivec2(int(k & 4095u), int(k >> 12)), 0).r;
  return t >= 0.0 && t < 1e29 ? t : 1e30;
}

void main() {
  uint f = a_feature;
  float height = feature_size(f, 0u);
  float base = feature_size(f, 1u);
  vec4 color = style_texel(f, 0u);
  float value = 1e30;
  if (u_linked) {
    vec4 a = texelFetch(u_anchors, feature_texel(f), 0);
    value = min(node(a.x) + a.z, node(a.y) + a.w);
    if (value > u_linked_max) value = 1e30;
  } else {
    value = texelFetch(u_values, feature_texel(f), 0).r;
  }
  if (u_ramp_range.z > 0.5 && value < 1e29 && value == value) {
    float t = clamp((value - u_ramp_range.x) / max(u_ramp_range.y - u_ramp_range.x, 1e-30), 0.0, 1.0);
    vec4 r = texture(u_ramp, vec2((t * 255.0 + 0.5) / 256.0, 0.5));
    color = vec4(r.rgb, r.a * color.a);
  }
  v_color = color;
  v_color.a *= layer_opacity();
  if (!feature_visible(f) || height <= 0.0) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
  vec2 merc = u_origin + a_offset;
  // The foot on the ground under it, the roof over the ground at the centroid
  float h = a_level > 0.5 ? altitude(u_origin + a_centroid) + height : altitude(merc) + base;
  gl_Position = u_matrix * vec4(project_up(u_plane_origin + a_offset * u_world, merc, h), 1.0);
  v_shade = a_shade;
  v_level = a_level;
  v_id = u_pick_base + f + 1u;
}";

const FRAGMENT: &str = r"
uniform float u_layer;
uniform bool u_pick;
flat in vec4 v_color;
flat in uint v_id;
in float v_shade;
in float v_level;
out vec4 color;

void main() {
  if (u_pick) {
    color = vec4(float(v_id & 255u), float((v_id >> 8) & 255u), float((v_id >> 16) & 255u), u_layer) / 255.0;
    return;
  }
  // Darker towards the foot of the walls (the roofs, at level 1, are not)
  float gradient = mix(0.82, 1.0, clamp(v_level, 0.0, 1.0));
  color = vec4(v_color.rgb * v_shade * gradient, v_color.a);
}";

const UNIFORMS: [&str; 10] = [
    "u_matrix",
    "u_plane_origin",
    "u_origin",
    "u_pick_base",
    "u_linked",
    "u_anchors",
    "u_network",
    "u_linked_max",
    "u_layer",
    "u_pick",
];

/// Texture units of the anchors and the network values (the features take 8 to 11, the drape 6 and 7)
const ANCHORS_UNIT: u32 = 1;
const NETWORK_UNIT: u32 = 2;

struct Batch {
    features: Features,
    vao: glow::VertexArray,
    vertices: glow::Buffer,
    indices: glow::Buffer,
    count: i32,
    origin: DVec2,
    min: DVec2,
    max: DVec2,
    anchors: Option<glow::Texture>,
    drape: Drape,
}

impl Batch {
    fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by this batch, not used afterwards
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vertices);
            gl.delete_buffer(self.indices);
            if let Some(anchors) = self.anchors {
                gl.delete_texture(anchors);
            }
        }
        self.features.delete(gl);
        self.drape.delete(gl);
    }
}

pub struct ExtrusionLayer {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 10],
    projection: Projection,
    drape_uniforms: DrapeUniforms,
    feature_uniforms: FeatureUniforms,
    batches: BTreeMap<u32, Batch>,
    /// Shared by the batches
    pub layer: LayerStyle,
    ramp: Option<(Vec<u8>, f32, f32)>,
    /// The network whose node values color the blocks: its id, its values texture, the highest value shown
    link: Option<(u32, glow::Texture, f32)>,
}

impl ExtrusionLayer {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        let program = gl::program(
            gl,
            &format!("#version 300 es\n{}{}{FEATURES}{VERTEX}", gl::PROJECT, drape::ALTITUDE),
            &format!("#version 300 es\nprecision highp float;\n{FRAGMENT}"),
        )?;
        Ok(Self {
            program,
            uniforms: gl::uniforms(gl, program, UNIFORMS),
            projection: Projection::new(gl, program),
            drape_uniforms: DrapeUniforms::new(gl, program),
            feature_uniforms: FeatureUniforms::new(gl, program),
            batches: BTreeMap::new(),
            layer: LayerStyle::default(),
            ramp: None,
            link: None,
        })
    }

    /// Adds or replaces the batch `key`: polygons and the feature of each, `feature_count` features
    pub fn set_batch(
        &mut self,
        gl: &glow::Context,
        key: u32,
        polygons: &Polygons,
        feature: &[u32],
        feature_count: usize,
    ) -> Result<(), String> {
        let (min, max) = polygons
            .points
            .iter()
            .fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), &p| {
                (a.min(p), b.max(p))
            });
        let (min, max) = if polygons.points.is_empty() {
            (DVec2::ZERO, DVec2::ZERO)
        } else {
            (min, max)
        };
        let origin = (min + max) / 2.0;
        let mut blocks = shapes::extrude(polygons, origin);
        for v in &mut blocks.vertices {
            v.polygon = feature.get(v.polygon as usize).copied().unwrap_or(u32::MAX);
        }
        let mut features = Features::new(gl, feature_count)?;
        features.set_ramp(gl, self.ramp.as_ref().map(|(rgba, lo, hi)| (rgba.as_slice(), *lo, *hi)));
        // SAFETY: object creation and uploads on the live context
        let batch = unsafe {
            let vao = gl.create_vertex_array()?;
            let vertices = gl.create_buffer()?;
            let indices = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertices));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&blocks.vertices), glow::STATIC_DRAW);
            let stride = size_of::<shapes::Block>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_i32(2, 1, glow::UNSIGNED_INT, stride, 16);
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_f32(3, 1, glow::FLOAT, false, stride, 20);
            gl.enable_vertex_attrib_array(4);
            gl.vertex_attrib_pointer_f32(4, 1, glow::FLOAT, false, stride, 24);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(indices));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes(&blocks.indices), glow::STATIC_DRAW);
            gl.bind_vertex_array(None);
            Batch {
                features,
                vao,
                vertices,
                indices,
                count: blocks.indices.len() as i32,
                origin,
                min,
                max,
                anchors: None,
                drape: Drape::new(),
            }
        };
        if let Some(old) = self.batches.insert(key, batch) {
            old.delete(gl);
        }
        Ok(())
    }

    pub fn remove_batch(&mut self, gl: &glow::Context, key: u32) {
        if let Some(old) = self.batches.remove(&key) {
            old.delete(gl);
        }
    }

    /// The feature textures of one batch, or of all
    pub fn features(&mut self, key: Option<u32>) -> Vec<&mut Features> {
        match key {
            Some(key) => self
                .batches
                .get_mut(&key)
                .map(|b| &mut b.features)
                .into_iter()
                .collect(),
            None => self.batches.values_mut().map(|b| &mut b.features).collect(),
        }
    }

    /// Ramp of every batch, present and future
    pub fn set_ramp(&mut self, gl: &glow::Context, ramp: Option<(&[u8], f32, f32)>) {
        self.ramp = ramp.map(|(rgba, lo, hi)| (rgba.to_vec(), lo, hi));
        for b in self.batches.values_mut() {
            b.features.set_ramp(gl, ramp);
        }
    }

    /// Per feature of batch `key`: the two network nodes and the costs from them (4 floats per feature)
    pub fn set_anchors(&mut self, gl: &glow::Context, key: u32, anchors: &[f32]) -> Result<(), String> {
        let Some(batch) = self.batches.get_mut(&key) else {
            return Ok(());
        };
        let n = batch.features.count;
        let mut padded = vec![0f32; n.div_ceil(4096).max(1) * 4096 * 4];
        for (i, a) in anchors.as_chunks::<4>().0.iter().take(n).enumerate() {
            padded[i * 4..i * 4 + 4].copy_from_slice(a);
        }
        // SAFETY: texture creation and upload on the live context
        unsafe {
            let texture = if let Some(texture) = batch.anchors {
                texture
            } else {
                let texture = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA32F, 4096, n.div_ceil(4096).max(1) as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
                batch.anchors = Some(texture);
                texture
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                4096,
                n.div_ceil(4096).max(1) as i32,
                glow::RGBA,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(bytes(&padded))),
            );
        }
        Ok(())
    }

    fn render(&self, frame: &Frame, pick: Option<u8>) {
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
            pick_base,
            linked,
            anchors,
            network,
            linked_max,
            layer,
            pick_u,
        ] = &self.uniforms;
        // SAFETY: draws with objects owned by this layer (and the linked network's values texture)
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_1_f32(layer.as_ref(), f32::from(pick.unwrap_or(0)));
            gl.uniform_1_i32(pick_u.as_ref(), i32::from(pick.is_some()));
            gl.uniform_1_i32(anchors.as_ref(), ANCHORS_UNIT as i32);
            gl.uniform_1_i32(network.as_ref(), NETWORK_UNIT as i32);
            gl.uniform_1_f32(linked_max.as_ref(), self.link.map_or(0.0, |(_, _, max)| max));
            gl.active_texture(glow::TEXTURE0 + NETWORK_UNIT);
            gl.bind_texture(glow::TEXTURE_2D, self.link.map(|(_, t, _)| t));
            self.projection.set(gl, view);
            let mut base = 0u32;
            for batch in self.batches.values() {
                let shift = -(batch.origin.x - camera.center.x).round();
                let plane = (DVec2::new(batch.origin.x + shift, batch.origin.y) - camera.center) * view.world_size;
                gl.uniform_2_f32(plane_origin.as_ref(), plane.x as f32, plane.y as f32);
                gl.uniform_2_f32(origin.as_ref(), batch.origin.x as f32, batch.origin.y as f32);
                gl.uniform_1_u32(pick_base.as_ref(), base);
                let is_linked = self.link.is_some() && batch.anchors.is_some();
                gl.uniform_1_i32(linked.as_ref(), i32::from(is_linked));
                gl.active_texture(glow::TEXTURE0 + ANCHORS_UNIT);
                gl.bind_texture(glow::TEXTURE_2D, batch.anchors);
                gl.active_texture(glow::TEXTURE0);
                batch.drape.bind(gl, &self.drape_uniforms, relief);
                batch
                    .features
                    .bind(gl, &self.feature_uniforms, camera.zoom, pixel_ratio);
                gl.bind_vertex_array(Some(batch.vao));
                gl.draw_elements(glow::TRIANGLES, batch.count, glow::UNSIGNED_INT, 0);
                base += batch.features.count as u32;
            }
            gl.bind_vertex_array(None);
        }
    }

    fn shows(&self, zoom: f64) -> bool {
        !self.batches.is_empty() && self.layer.shows(zoom)
    }
}

impl Layer for ExtrusionLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        if !self.shows(frame.camera.zoom) {
            return false;
        }
        for batch in self.batches.values_mut() {
            batch.features.layer = self.layer;
            if let Some((source, _)) = frame.relief {
                let pad = DVec2::splat(1e-6);
                batch.drape.update(frame.gl, batch.min - pad, batch.max + pad, source);
            }
        }
        let gl = frame.gl;
        let translucent = self.layer.opacity[1].iter().any(|&o| o < 1.0);
        // SAFETY: state changes on the live context, set back below
        unsafe {
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.disable(glow::CULL_FACE);
            if translucent {
                // Depth first: then each pixel is colored by its nearest face only
                gl.color_mask(false, false, false, false);
                self.render(frame, None);
                gl.color_mask(true, true, true, true);
                gl.depth_func(glow::EQUAL);
                gl.depth_mask(false);
            }
        }
        self.render(frame, None);
        // SAFETY: state changes on the live context
        unsafe {
            gl.disable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.enable(glow::CULL_FACE);
        }
        false
    }

    fn pickable(&self) -> bool {
        true
    }

    fn draw_pick(&self, frame: &Frame, layer: u8) {
        if !self.shows(frame.camera.zoom) {
            return;
        }
        // SAFETY: state changes on the live context, set back below
        unsafe {
            frame.gl.enable(glow::DEPTH_TEST);
            frame.gl.depth_func(glow::LEQUAL);
            frame.gl.depth_mask(true);
        }
        self.render(frame, Some(layer));
        // SAFETY: as above
        unsafe { frame.gl.disable(glow::DEPTH_TEST) };
    }

    fn forget(&mut self, id: u32) {
        if self.link.is_some_and(|(network, _, _)| network == id) {
            self.link = None;
        }
    }

    fn delete(&mut self, gl: &glow::Context) {
        for batch in self.batches.values() {
            batch.delete(gl);
        }
        // SAFETY: program owned by this layer, not used afterwards
        unsafe { gl.delete_program(self.program) };
    }
}

#[wasm_bindgen]
impl Engine {
    /// An empty extrusion layer: its blocks come in batches (`setExtrusionBatch`)
    #[wasm_bindgen(js_name = addExtrusions)]
    pub fn add_extrusions(&mut self) -> Result<u32, JsValue> {
        let layer = ExtrusionLayer::new(&self.gl)?;
        Ok(self.add_layer(Box::new(layer)))
    }

    /// Adds or replaces batch `key` of an extrusion layer: polygons as for `addFills`, `feature_count` features (the
    /// feature style gives each its height and base height as its two sizes, metres)
    #[wasm_bindgen(js_name = setExtrusionBatch)]
    #[allow(clippy::too_many_arguments)]
    pub fn set_extrusion_batch(
        &mut self,
        id: u32,
        key: u32,
        points: &[f64],
        ring_start: &[u32],
        polygon_start: &[u32],
        feature: &[u32],
        feature_count: usize,
    ) -> Result<(), JsValue> {
        let points: Vec<DVec2> = points
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[x, y]| DVec2::new(x, y))
            .collect();
        if let Some((layer, gl)) = self.layer_mut::<ExtrusionLayer>(id) {
            let polygons = Polygons {
                points: &points,
                ring_start,
                polygon_start,
            };
            layer.set_batch(gl, key, &polygons, feature, feature_count)?;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = removeExtrusionBatch)]
    pub fn remove_extrusion_batch(&mut self, id: u32, key: u32) {
        if let Some((layer, gl)) = self.layer_mut::<ExtrusionLayer>(id) {
            layer.remove_batch(gl, key);
        }
    }

    /// Colors an extrusion layer by the node values of network layer `network` (undefined: by its own values):
    /// each block takes min(value(A) + cost from A, value(B) + cost from B) from its anchors (`setExtrusionAnchors`),
    /// through the layer's ramp, up to `max` (above: its own color)
    #[wasm_bindgen(js_name = linkExtrusionNetwork)]
    pub fn link_extrusion_network(&mut self, id: u32, network: Option<u32>, max: f32) {
        let values = network.and_then(|n| self.layer::<NetworkLayer>(n).map(|l| (n, l.values_texture())));
        if let Some((layer, _)) = self.layer_mut::<ExtrusionLayer>(id) {
            layer.link = values.map(|(n, texture)| (n, texture, max));
        }
    }

    /// Anchors of batch `key` on the linked network: per feature, node A, node B, cost from A, cost from B
    #[wasm_bindgen(js_name = setExtrusionAnchors)]
    pub fn set_extrusion_anchors(&mut self, id: u32, key: u32, anchors: &[f32]) -> Result<(), JsValue> {
        if let Some((layer, gl)) = self.layer_mut::<ExtrusionLayer>(id) {
            layer.set_anchors(gl, key, anchors)?;
        }
        Ok(())
    }
}
