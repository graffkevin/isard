//! What the feature layers (lines, circles, fills) share: the style of each feature in textures, read by their
//! shaders, so that restyling, recoloring or filtering thousands of features is one texture upload and never touches
//! the geometry.
//!
//! - The style texture holds `TEXELS` RGBA32F texels per feature: its color, its second color (a circle's stroke),
//!   then two sizes given as zoom stops (up to four zoom-value pairs each, linear in between, as `MapLibre`'s
//!   `interpolate linear zoom`): a line's width, a circle's radius and stroke width.
//! - A value per feature (R32F), turned into a color by the layer's ramp when it has one: water depth, a gauge's
//!   level… Values from 1e29 up (or NaN) keep the feature's own color.
//! - A visibility mask (R8): the filter.
//!
//! Layer-wide: an opacity given as zoom stops (a layer fading in past a zoom), and a zoom range.

use crate::engine::Engine;
use crate::render::circles::CircleLayer;
use crate::render::extrusions::ExtrusionLayer;
use crate::render::fills::FillLayer;
use crate::render::gl::{self, Objects};
use crate::render::lines::LineLayer;
use glow::HasContext;
use wasm_bindgen::prelude::*;

/// Texels per feature in the style texture
pub const TEXELS: usize = 8;
/// Floats per feature in the style given to `Features::set_style`
pub const FLOATS: usize = TEXELS * 4;
/// Values from here up keep the feature's own color
pub const NO_VALUE: f32 = 1e29;
/// Texels of the ramp texture
const RAMP: usize = 256;

/// GLSL of the feature styles: `feature_visible`, `feature_color`, `feature_size` (sizes 0 and 1, CSS pixels at the
/// current zoom) and `layer_opacity`
pub const FEATURES: &str = r"
uniform highp sampler2D u_style;
uniform highp sampler2D u_values;
uniform highp sampler2D u_mask;
uniform sampler2D u_ramp;
// Ramp: lowest and highest value, 1 when the layer has one
uniform vec3 u_ramp_range;
uniform float u_zoom;
uniform float u_pixel_ratio;
uniform vec4 u_opacity_zooms;
uniform vec4 u_opacity_values;

ivec2 feature_texel(uint i) { return ivec2(int(i & 4095u), int(i >> 12)); }
vec4 style_texel(uint f, uint k) { return texelFetch(u_style, feature_texel(f * 8u + k), 0); }

// Linear between up to four (zoom, value) stops, constant outside
float stops(vec4 z, vec4 v) {
  if (u_zoom <= z.x) return v.x;
  if (u_zoom <= z.y) return mix(v.x, v.y, (u_zoom - z.x) / max(z.y - z.x, 1e-6));
  if (u_zoom <= z.z) return mix(v.y, v.z, (u_zoom - z.y) / max(z.z - z.y, 1e-6));
  if (u_zoom <= z.w) return mix(v.z, v.w, (u_zoom - z.z) / max(z.w - z.z, 1e-6));
  return v.w;
}

bool feature_visible(uint f) { return texelFetch(u_mask, feature_texel(f), 0).r > 0.5; }

vec4 feature_color(uint f, uint k) {
  vec4 c = style_texel(f, k);
  if (k == 0u && u_ramp_range.z > 0.5) {
    float v = texelFetch(u_values, feature_texel(f), 0).r;
    if (v < 1e29 && v == v) {
      float t = clamp((v - u_ramp_range.x) / max(u_ramp_range.y - u_ramp_range.x, 1e-30), 0.0, 1.0);
      vec4 r = texture(u_ramp, vec2((t * 255.0 + 0.5) / 256.0, 0.5));
      c = vec4(r.rgb, r.a * c.a);
    }
  }
  return c;
}

float feature_size(uint f, uint k) { return stops(style_texel(f, 2u + 2u * k), style_texel(f, 3u + 2u * k)); }

float layer_opacity() { return stops(u_opacity_zooms, u_opacity_values); }
";

/// Texture units of the feature textures (the drape takes 6 and 7)
pub const STYLE_UNIT: u32 = 8;
pub const VALUES_UNIT: u32 = 9;
pub const MASK_UNIT: u32 = 10;
pub const RAMP_UNIT: u32 = 11;

/// Value of four zoom stops (zooms, then values) at a zoom, as the shaders' `stops`
#[must_use]
pub fn stops([z, v]: &[[f32; 4]; 2], zoom: f64) -> f64 {
    let zoom = zoom as f32;
    if zoom <= z[0] {
        return f64::from(v[0]);
    }
    for k in 1..4 {
        if zoom <= z[k] {
            let t = (zoom - z[k - 1]) / (z[k] - z[k - 1]).max(1e-6);
            return f64::from(v[k - 1] + (v[k] - v[k - 1]) * t);
        }
    }
    f64::from(v[3])
}

/// Four zoom stops from eight floats (zooms, then values)
#[must_use]
pub fn stops_of(s: &[f32]) -> Option<[[f32; 4]; 2]> {
    let s: &[f32; 8] = s.get(..8)?.try_into().ok()?;
    Some([[s[0], s[1], s[2], s[3]], [s[4], s[5], s[6], s[7]]])
}

/// Layer-wide style
#[derive(Clone, Copy, Debug)]
pub struct LayerStyle {
    /// Opacity as zoom stops: zooms, then values
    pub opacity: [[f32; 4]; 2],
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub visible: bool,
}

impl Default for LayerStyle {
    fn default() -> Self {
        Self {
            opacity: [[0.0; 4], [1.0; 4]],
            min_zoom: 0.0,
            max_zoom: 24.0,
            visible: true,
        }
    }
}

impl LayerStyle {
    /// Whether the layer shows at this zoom
    #[must_use]
    pub fn shows(&self, zoom: f64) -> bool {
        self.visible && zoom >= self.min_zoom && zoom < self.max_zoom
    }

    /// Whether its opacity is under 1 at some zoom
    #[must_use]
    pub fn translucent(&self) -> bool {
        self.opacity[1].iter().any(|&o| o < 1.0)
    }
}

/// The feature textures of a layer
pub struct Features {
    pub count: usize,
    style: glow::Texture,
    values: glow::Texture,
    mask: glow::Texture,
    ramp: glow::Texture,
    ramp_range: Option<(f32, f32)>,
    /// A feature's color or the ramp has an alpha under 1 (`colors_translucent`)
    translucent_style: bool,
    translucent_ramp: bool,
    pub layer: LayerStyle,
}

/// Uniform locations of `FEATURES`
pub struct FeatureUniforms([Option<glow::UniformLocation>; 9]);

impl FeatureUniforms {
    pub fn new(gl: &glow::Context, program: glow::Program) -> Self {
        let uniforms = crate::render::gl::uniforms(
            gl,
            program,
            [
                "u_style",
                "u_values",
                "u_mask",
                "u_ramp",
                "u_ramp_range",
                "u_zoom",
                "u_opacity_zooms",
                "u_opacity_values",
                "u_pixel_ratio",
            ],
        );
        gl::sampler_units(
            gl,
            program,
            &[
                (uniforms[0].as_ref(), STYLE_UNIT),
                (uniforms[1].as_ref(), VALUES_UNIT),
                (uniforms[2].as_ref(), MASK_UNIT),
                (uniforms[3].as_ref(), RAMP_UNIT),
            ],
        );
        Self(uniforms)
    }
}

impl Features {
    /// Textures for `count` features: black, size 1, visible, no value
    pub fn new(gl: &glow::Context, count: usize) -> Result<Self, String> {
        // Everything created is deleted again if a step fails
        let mut objects = Objects::new(gl);
        let style = objects.adopt_texture(gl::table_texture(gl, glow::RGBA32F, count.saturating_mul(TEXELS))?);
        let values = objects.adopt_texture(gl::table_texture(gl, glow::R32F, count)?);
        let mask = objects.adopt_texture(gl::table_texture(gl, glow::R8, count)?);
        let ramp = objects.texture()?;
        // SAFETY: storage and parameters of a texture just created on a live context
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(ramp));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, RAMP as i32, 1);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, key, value as i32);
            }
        }
        objects.keep();
        let mut features = Self {
            count,
            style,
            values,
            mask,
            ramp,
            ramp_range: None,
            translucent_style: false,
            translucent_ramp: false,
            layer: LayerStyle::default(),
        };
        features.set_style(gl, &default_style(count));
        features.set_values(gl, &vec![NO_VALUE; count]);
        features.set_mask(gl, &vec![255u8; count]);
        Ok(features)
    }

    /// `FLOATS` per feature: color (RGBA 0-1), second color, then two sizes as stops: four zooms, four values each
    pub fn set_style(&mut self, gl: &glow::Context, style: &[f32]) {
        let n = (style.len() / FLOATS).min(self.count) * FLOATS;
        // The features past the style given keep theirs, and whether one of them was translucent
        let translucent = style_translucent(&style[..n]);
        self.translucent_style = translucent || (n < self.count * FLOATS && self.translucent_style);
        // SAFETY: binding a texture owned by this layer, for the upload inside its storage
        unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(self.style)) };
        gl::upload_table(gl, gl::bytes(&style[..n]), n / 4, glow::RGBA, glow::FLOAT, 16);
    }

    /// One value per feature; from `NO_VALUE` up (or NaN), the feature keeps its own color
    pub fn set_values(&self, gl: &glow::Context, values: &[f32]) {
        let n = values.len().min(self.count);
        let clean: Vec<f32> = values[..n]
            .iter()
            .map(|&v| if v.is_nan() { NO_VALUE } else { v.min(NO_VALUE) })
            .collect();
        // SAFETY: binding a texture owned by this layer, for the upload inside its storage
        unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(self.values)) };
        gl::upload_table(gl, gl::bytes(&clean), n, glow::RED, glow::FLOAT, 4);
    }

    /// One byte per feature: 0 hides it
    pub fn set_mask(&self, gl: &glow::Context, mask: &[u8]) {
        let n = mask.len().min(self.count);
        // SAFETY: binding a texture owned by this layer, for the upload inside its storage
        unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(self.mask)) };
        gl::upload_table(gl, &mask[..n], n, glow::RED, glow::UNSIGNED_BYTE, 1);
    }

    /// Colors of the values from `min` to `max`: 256 RGBA texels; None: features keep their own colors
    pub fn set_ramp(&mut self, gl: &glow::Context, ramp: Option<(&[u8], f32, f32)>) {
        self.ramp_range = ramp.map(|(_, min, max)| (min, max));
        let Some((rgba, _, _)) = ramp else {
            return;
        };
        if rgba.len() < RAMP * 4 {
            self.ramp_range = None;
            return;
        }
        self.translucent_ramp = ramp_translucent(&rgba[..RAMP * 4]);
        // SAFETY: upload of the whole ramp texture
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.ramp));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                RAMP as i32,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&rgba[..RAMP * 4])),
            );
        }
    }

    /// Whether a feature's color, or the ramp in use, has an alpha under 1: what is drawn may then show through
    /// itself where it overlaps (the joins of a line), as with a layer opacity under 1
    #[must_use]
    pub fn colors_translucent(&self) -> bool {
        self.translucent_style || (self.ramp_range.is_some() && self.translucent_ramp)
    }

    /// Whether the layer draws translucent: its opacity or its colors (`colors_translucent`)
    #[must_use]
    pub fn translucent(&self) -> bool {
        self.layer.translucent() || self.colors_translucent()
    }

    /// Binds the textures and sets the uniforms of `FEATURES` (program in use)
    pub fn bind(&self, gl: &glow::Context, uniforms: &FeatureUniforms, zoom: f64, pixel_ratio: f64) {
        let [_, _, _, _, range, zoom_u, opacity_zooms, opacity_values, ratio] = &uniforms.0;
        let (lo, hi, on) = self.ramp_range.map_or((0.0, 1.0, 0.0), |(lo, hi)| (lo, hi, 1.0));
        // SAFETY: uniforms and bindings on the program in use (its sampler units set once, by `FeatureUniforms::new`)
        unsafe {
            for (unit, texture) in [
                (STYLE_UNIT, self.style),
                (VALUES_UNIT, self.values),
                (MASK_UNIT, self.mask),
                (RAMP_UNIT, self.ramp),
            ] {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            }
            gl.active_texture(glow::TEXTURE0);
            gl.uniform_3_f32(range.as_ref(), lo, hi, on);
            gl.uniform_1_f32(zoom_u.as_ref(), zoom as f32);
            gl.uniform_4_f32_slice(opacity_zooms.as_ref(), &self.layer.opacity[0]);
            gl.uniform_4_f32_slice(opacity_values.as_ref(), &self.layer.opacity[1]);
            gl.uniform_1_f32(ratio.as_ref(), pixel_ratio as f32);
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: textures owned by this layer, not used afterwards
        unsafe {
            for texture in [self.style, self.values, self.mask, self.ramp] {
                gl.delete_texture(texture);
            }
        }
    }
}

/// A texture of two floats per entry (RG32F), read with `feature_texel`: the feature of each line and its length,
/// exact up to 2^24 features
pub fn index_texture(gl: &glow::Context, entries: &[f32]) -> Result<glow::Texture, String> {
    let texture = gl::table_texture(gl, glow::RG32F, entries.len() / 2)?;
    gl::upload_table(gl, gl::bytes(entries), entries.len() / 2, glow::RG, glow::FLOAT, 8);
    Ok(texture)
}

/// Whether a style (`FLOATS` per feature) has a color with an alpha under 1 (the first color: the second, a circle's
/// stroke, never overlaps itself)
fn style_translucent(style: &[f32]) -> bool {
    style.as_chunks::<FLOATS>().0.iter().any(|s| s[3] < 1.0)
}

/// Whether a ramp (RGBA bytes) has a color with an alpha under 1
fn ramp_translucent(rgba: &[u8]) -> bool {
    rgba.as_chunks::<4>().0.iter().any(|c| c[3] < 255)
}

/// The style of `count` features before any is given: opaque black, sizes 1 at every zoom
fn default_style(count: usize) -> Vec<f32> {
    let mut style = vec![0f32; count * FLOATS];
    for s in style.as_chunks_mut::<FLOATS>().0 {
        s[3] = 1.0;
        s[7] = 1.0;
        s[12..16].fill(1.0);
        s[20..24].fill(1.0);
    }
    style
}

impl Engine {
    /// Runs `f` on the feature textures of a line, circle or fill layer, or of the batch `key` of an extrusion layer
    /// (all its batches without a key)
    fn with_features(&mut self, id: u32, key: Option<u32>, mut f: impl FnMut(&mut Features, &glow::Context)) {
        if let Some((l, gl)) = self.layer_mut::<LineLayer>(id) {
            f(&mut l.features, gl);
        } else if let Some((l, gl)) = self.layer_mut::<CircleLayer>(id) {
            f(&mut l.features, gl);
        } else if let Some((l, gl)) = self.layer_mut::<FillLayer>(id) {
            f(&mut l.features, gl);
        } else if let Some((l, gl)) = self.layer_mut::<ExtrusionLayer>(id) {
            for features in l.features(key) {
                f(features, gl);
            }
        }
    }
}

#[wasm_bindgen]
impl Engine {
    /// Style of the features of a line, circle, fill or extrusion layer (batch `key` of the latter): 32 floats per
    /// feature, its color (RGBA 0-1), its second color (a circle's stroke), then two sizes as zoom stops (four zooms,
    /// four values each): a line's width; a circle's radius and stroke width, CSS pixels; a block's height and base
    /// height, metres
    #[wasm_bindgen(js_name = setFeatureStyle)]
    pub fn set_feature_style(&mut self, id: u32, style: &[f32], key: Option<u32>) {
        self.with_features(id, key, |features, gl| features.set_style(gl, style));
    }

    /// One value per feature, colored through the layer's ramp (`setFeatureRamp`); 1e30 or NaN: its own color
    #[wasm_bindgen(js_name = setFeatureValues)]
    pub fn set_feature_values(&mut self, id: u32, values: &[f32], key: Option<u32>) {
        self.with_features(id, key, |features, gl| features.set_values(gl, values));
    }

    /// One byte per feature, 0 hiding it (the filter)
    #[wasm_bindgen(js_name = setFeatureMask)]
    pub fn set_feature_mask(&mut self, id: u32, mask: &[u8], key: Option<u32>) {
        self.with_features(id, key, |features, gl| features.set_mask(gl, mask));
    }

    /// Colors of the values `min` to `max`, 256 RGBA texels (1024 bytes); undefined: none
    #[wasm_bindgen(js_name = setFeatureRamp)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_feature_ramp(&mut self, id: u32, rgba: Option<Box<[u8]>>, min: f32, max: f32) {
        let ramp = rgba.as_deref().map(|r| (r, min, max));
        if let Some((layer, gl)) = self.layer_mut::<ExtrusionLayer>(id) {
            layer.set_ramp(gl, ramp);
            return;
        }
        self.with_features(id, None, |features, gl| features.set_ramp(gl, ramp));
    }

    /// Layer-wide style of a feature layer: opacity as zoom stops (four zooms then four values), the zoom range it
    /// shows in (`min_zoom` included, `max_zoom` excluded), whether it shows at all
    #[wasm_bindgen(js_name = setFeatureLayer)]
    pub fn set_feature_layer(&mut self, id: u32, opacity: &[f32], min_zoom: f64, max_zoom: f64, visible: bool) {
        let mut style = LayerStyle {
            min_zoom,
            max_zoom,
            visible,
            ..LayerStyle::default()
        };
        if let Some(stops) = stops_of(opacity) {
            style.opacity = stops;
        }
        if let Some((layer, _)) = self.layer_mut::<ExtrusionLayer>(id) {
            layer.layer = style;
            return;
        }
        self.with_features(id, None, |features, _| features.layer = style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_interpolate_linearly_and_hold_outside() {
        let s = [[10.0, 12.0, 14.0, 16.0], [1.0, 3.0, 3.0, 0.0]];
        assert_eq!(stops(&s, 5.0), 1.0);
        assert_eq!(stops(&s, 11.0), 2.0);
        assert_eq!(stops(&s, 13.0), 3.0);
        assert_eq!(stops(&s, 15.0), 1.5);
        assert_eq!(stops(&s, 20.0), 0.0);
        // Equal zooms: a step, no division by zero
        let step = [[10.0, 10.0, 10.0, 10.0], [0.0, 1.0, 1.0, 1.0]];
        assert_eq!(stops(&step, 9.0), 0.0);
        assert_eq!(stops(&step, 11.0), 1.0);
    }

    #[test]
    fn stops_of_reads_eight_floats() {
        let f = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        assert_eq!(stops_of(&f), Some([[1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]]));
        assert_eq!(stops_of(&f[..7]), None);
    }

    #[test]
    fn the_default_style_is_black_and_sizes_one() {
        let s = default_style(2);
        assert_eq!(s.len(), 2 * FLOATS);
        assert_eq!(&s[FLOATS..FLOATS + 8], &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        // Zooms 0, sizes 1
        assert_eq!(&s[8..16], &[0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(&s[16..24], &[0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn layers_show_in_their_zoom_range() {
        let style = LayerStyle {
            min_zoom: 10.0,
            max_zoom: 14.0,
            ..LayerStyle::default()
        };
        assert!(!style.shows(9.99) && style.shows(10.0) && style.shows(13.9) && !style.shows(14.0));
        assert!(
            !LayerStyle {
                visible: false,
                ..LayerStyle::default()
            }
            .shows(12.0)
        );
    }

    #[test]
    fn translucent_colors_are_noticed() {
        let mut style = default_style(3);
        assert!(!style_translucent(&style));
        // The second color (a stroke) does not count, the first does
        style[FLOATS + 7] = 0.5;
        assert!(!style_translucent(&style));
        style[2 * FLOATS + 3] = 0.99;
        assert!(style_translucent(&style));
        let mut ramp = vec![255u8; RAMP * 4];
        assert!(!ramp_translucent(&ramp));
        ramp[RAMP * 4 - 1] = 128;
        assert!(ramp_translucent(&ramp));
        let opaque = LayerStyle::default();
        assert!(!opaque.translucent());
        let fading = LayerStyle {
            opacity: [[10.0, 12.0, 12.0, 12.0], [0.0, 1.0, 1.0, 1.0]],
            ..opaque
        };
        assert!(fading.translucent());
    }
}
