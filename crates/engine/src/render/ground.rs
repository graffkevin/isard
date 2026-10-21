//! Ground layer: a fragment shader of the application's, run over the ground itself: the tiles the raster pass just
//! drew are drawn once more with it, so it lies on the relief exactly as the basemap does, at any tilt, and costs one
//! fragment per pixel of ground where it shows. For fields the application computes and the GPU draws: water depth
//! from a terrain grid and levels per section, a heat map…
//!
//! The application gives a GLSL function `vec4 ground(vec2 offset)`: the color (premultiplied alpha) of the ground at
//! `offset`, the Mercator offset of the pixel from the layer's anchor, exact to the millimetre (worked out from the
//! position relative to the camera, never from absolute f32 coordinates). It declares its own uniforms and samplers;
//! the textures it gets are set by name (integer formats included, 2D or 2D arrays), as are its uniforms. Only the
//! ground tiles that meet the layer's bounds are drawn.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::gl::{self, DepthState, Projection};
use crate::render::raster::{self, STACK};
use glam::DVec2;
use glow::HasContext;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

/// What comes before the application's code in the fragment shader
const PRELUDE: &str = r"#version 300 es
precision highp float;
precision highp int;
precision highp usampler2D;
precision highp isampler2D;
precision highp sampler2DArray;
precision highp usampler2DArray;
uniform vec2 u_anchor_offset;
uniform float u_world;
uniform vec4 u_bounds;
uniform float u_zoom;
uniform float u_pixel_ratio;
uniform vec3 u_fog;
uniform vec2 u_fog_range;
in vec2 v_plane;
in float v_distance;
out vec4 isard_color;

// Mercator offset of this pixel from the layer's anchor
vec2 ground_offset() { return u_anchor_offset + v_plane / u_world; }
";

/// What comes after it
const MAIN: &str = r"
void main() {
  vec2 offset = ground_offset();
  if (any(lessThan(offset, u_bounds.xy)) || any(greaterThan(offset, u_bounds.zw))) discard;
  vec4 c = ground(offset);
  if (c.a <= 0.0) discard;
  if (u_fog_range.y > 0.0) c.rgb = mix(c.rgb, u_fog * c.a, smoothstep(u_fog_range.x, u_fog_range.y, v_distance));
  isard_color = c;
}";

/// Uniforms the layer sets itself
const UNIFORMS: [&str; 11] = [
    "u_matrix",
    "u_heights",
    "u_relief",
    "u_eye",
    "u_anchor_offset",
    "u_world",
    "u_bounds",
    "u_zoom",
    "u_pixel_ratio",
    "u_fog",
    "u_fog_range",
];

/// First texture unit of the application's textures (the tiles and heights of the vertex shader take the first ones)
const FIRST_UNIT: u32 = STACK as u32 + 1;
/// Units WebGL2 guarantees to a fragment shader
const MAX_UNITS: u32 = 16;

/// A texture format: internal format, format, type, bytes per texel, whether it is read with integer samplers
fn format(name: &str) -> Option<(u32, u32, u32, usize, bool)> {
    Some(match name {
        "r8" => (glow::R8, glow::RED, glow::UNSIGNED_BYTE, 1, false),
        "r8ui" => (glow::R8UI, glow::RED_INTEGER, glow::UNSIGNED_BYTE, 1, true),
        "r16ui" => (glow::R16UI, glow::RED_INTEGER, glow::UNSIGNED_SHORT, 2, true),
        "r16i" => (glow::R16I, glow::RED_INTEGER, glow::SHORT, 2, true),
        "r32ui" => (glow::R32UI, glow::RED_INTEGER, glow::UNSIGNED_INT, 4, true),
        "r32f" => (glow::R32F, glow::RED, glow::FLOAT, 4, false),
        "rg16ui" => (glow::RG16UI, glow::RG_INTEGER, glow::UNSIGNED_SHORT, 4, true),
        "rg32f" => (glow::RG32F, glow::RG, glow::FLOAT, 8, false),
        "rgba8" => (glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE, 4, false),
        "rgba8ui" => (glow::RGBA8UI, glow::RGBA_INTEGER, glow::UNSIGNED_BYTE, 4, true),
        "rgba16ui" => (glow::RGBA16UI, glow::RGBA_INTEGER, glow::UNSIGNED_SHORT, 8, true),
        "rgba32f" => (glow::RGBA32F, glow::RGBA, glow::FLOAT, 16, false),
        _ => return None,
    })
}

/// The numbers of a uniform, in its type
enum Values {
    Float(Vec<f32>),
    Int(Vec<i32>),
    Uint(Vec<u32>),
}

impl Values {
    /// `values` for a uniform of GL type `kind` (booleans as integers)
    fn of(kind: u32, values: &[f64]) -> Self {
        match kind {
            glow::INT
            | glow::BOOL
            | glow::INT_VEC2
            | glow::BOOL_VEC2
            | glow::INT_VEC3
            | glow::BOOL_VEC3
            | glow::INT_VEC4
            | glow::BOOL_VEC4 => Self::Int(values.iter().map(|&v| v as i32).collect()),
            glow::UNSIGNED_INT | glow::UNSIGNED_INT_VEC2 | glow::UNSIGNED_INT_VEC3 | glow::UNSIGNED_INT_VEC4 => {
                Self::Uint(values.iter().map(|&v| v as u32).collect())
            }
            _ => Self::Float(values.iter().map(|&v| v as f32).collect()),
        }
    }
}

struct Texture {
    handle: glow::Texture,
    unit: u32,
    /// `TEXTURE_2D` or `TEXTURE_2D_ARRAY`
    target: u32,
    format: (u32, u32, u32, usize, bool),
    size: (i32, i32, i32),
}

/// A texture of a ground layer as the page describes it (`setGroundTexture`)
pub struct TextureSpec<'a> {
    /// The sampler it is bound to
    pub name: &'a str,
    /// Its format by name (`format`: r8, rgba8, r32f…)
    pub format: &'a str,
    /// Texels
    pub width: i32,
    /// Texels
    pub height: i32,
    /// 0 for a 2D texture, else the layers of a 2D array
    pub depth: i32,
    /// Linear filtering (the non-integer formats), else nearest
    pub linear: bool,
}

/// A region of a ground layer's texture, texels: `width` × `height` at (`x`, `y`) of layer `layer` (0 for a 2D
/// texture)
#[derive(Clone, Copy)]
pub struct Region {
    /// Its first column
    pub x: i32,
    /// Its first row
    pub y: i32,
    /// The layer of a 2D array, 0 for a 2D texture
    pub layer: i32,
    /// Texels
    pub width: i32,
    /// Texels
    pub height: i32,
}

/// Makes a texture of `target` (2D or 2D array) with its storage, clamped, filtered as `spec` says, and uploads
/// `data` (exactly its bytes) if any. Left bound
fn create_texture(
    gl: &glow::Context,
    target: u32,
    (internal, pixel_format, kind, _, integer): (u32, u32, u32, usize, bool),
    spec: &TextureSpec,
    data: Option<&[u8]>,
) -> Result<glow::Texture, String> {
    let &TextureSpec {
        width,
        height,
        depth,
        linear,
        ..
    } = spec;

    // SAFETY: creation and upload on the live context, the data checked against the storage by the caller
    unsafe {
        let texture = gl.create_texture()?;

        gl.bind_texture(target, Some(texture));

        let filter = if linear && !integer {
            glow::LINEAR
        } else {
            glow::NEAREST
        };

        for (key, value) in [
            (glow::TEXTURE_MIN_FILTER, filter),
            (glow::TEXTURE_MAG_FILTER, filter),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(target, key, value as i32);
        }
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        if depth > 0 {
            gl.tex_storage_3d(target, 1, internal, width, height, depth);

            if let Some(data) = data {
                gl.tex_sub_image_3d(
                    target,
                    0,
                    0,
                    0,
                    0,
                    width,
                    height,
                    depth,
                    pixel_format,
                    kind,
                    glow::PixelUnpackData::Slice(Some(data)),
                );
            }
        } else {
            gl.tex_storage_2d(target, 1, internal, width, height);

            if let Some(data) = data {
                gl.tex_sub_image_2d(
                    target,
                    0,
                    0,
                    0,
                    width,
                    height,
                    pixel_format,
                    kind,
                    glow::PixelUnpackData::Slice(Some(data)),
                );
            }
        }
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);

        Ok(texture)
    }
}

/// A ground layer: the application's GLSL `ground(offset)` compiled into a raster-pass program, its own uniforms and
/// textures, drawn over the ground inside its Mercator bounds
pub struct GroundLayer {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 11],
    projection: Projection,
    /// The application's uniforms: location, type and array size
    custom: HashMap<String, (glow::UniformLocation, u32, i32)>,
    /// Values set before a draw, converted once to the uniform's type, replayed each draw (the program is shared by
    /// nothing else)
    values: HashMap<String, Values>,
    textures: HashMap<String, Texture>,
    /// Mercator anchor of the offsets, and bounds (Mercator)
    anchor: DVec2,
    min: DVec2,
    max: DVec2,
    /// Drawn at all (the application's toggle)
    pub visible: bool,
}

impl GroundLayer {
    /// `circumference`: the body's, in metres (`ISARD_CIRCUMFERENCE`)
    pub fn new(
        gl: &glow::Context,
        code: &str,
        circumference: f64,
        anchor: DVec2,
        min: DVec2,
        max: DVec2,
    ) -> Result<Self, String> {
        // Length of the Mercator world in metres of the body (offsets times this: metres of EPSG:3857 on the Earth);
        // `ISARD_EARTH`, its first name (when the Earth was the only body), kept for the code written with it
        let earth = format!(
            "const float ISARD_CIRCUMFERENCE = {circumference:.3};\nconst float ISARD_EARTH = ISARD_CIRCUMFERENCE;\n"
        );
        let program = gl::program(
            gl,
            &format!("#version 300 es\n{}{}", gl::PROJECT, raster::VERTEX),
            &format!("{PRELUDE}{earth}#line 1\n{code}\n{MAIN}"),
        )?;
        let mut custom = HashMap::new();

        // SAFETY: queries on a linked program
        unsafe {
            for i in 0..gl.get_program_parameter_i32(program, glow::ACTIVE_UNIFORMS) as u32 {
                let Some(active) = gl.get_active_uniform(program, i) else {
                    continue;
                };
                let name = active.name.trim_end_matches("[0]").to_owned();

                if name.starts_with("u_") && (UNIFORMS.contains(&name.as_str()) || is_projection(&name)) {
                    continue;
                }

                if let Some(location) = gl.get_uniform_location(program, &name) {
                    custom.insert(name, (location, active.utype, active.size));
                }
            }
        }

        Ok(Self {
            program,
            uniforms: gl::uniforms(gl, program, UNIFORMS),
            projection: Projection::new(gl, program),
            custom,
            values: HashMap::new(),
            textures: HashMap::new(),
            anchor,
            min,
            max,
            visible: true,
        })
    }

    /// The Mercator box (0-1) it shows in
    pub fn set_bounds(&mut self, min: DVec2, max: DVec2) {
        self.min = min;
        self.max = max;
    }

    /// A uniform of the application's code; numbers for any type (booleans 0 or 1, matrices column major)
    pub fn set_uniform(&mut self, name: &str, values: &[f64]) -> Result<(), String> {
        let Some(&(_, kind, _)) = self.custom.get(name) else {
            return Err(format!(
                "no uniform {name} in the ground shader (unused uniforms are removed by the compiler)"
            ));
        };

        self.values.insert(name.to_owned(), Values::of(kind, values));

        Ok(())
    }

    /// Creates (or replaces) the texture bound to sampler `name`: `depth` 0 for a 2D texture, else the layers of a 2D
    /// array; `data` (optional): its texels, row after row, layer after layer
    pub fn set_texture(&mut self, gl: &glow::Context, spec: &TextureSpec, data: Option<&[u8]>) -> Result<(), String> {
        let &TextureSpec {
            name,
            format: format_name,
            width,
            height,
            depth,
            ..
        } = spec;
        let format = format(format_name).ok_or_else(|| format!("unknown texture format {format_name}"))?;
        let unit = self.texture_unit(name)?;
        let expected = gl::area(width, height)
            .and_then(|texels| texels.checked_mul(usize::try_from(depth.max(1)).ok()?))
            .and_then(|texels| texels.checked_mul(format.3))
            .filter(|_| width > 0 && height > 0)
            .ok_or_else(|| format!("texture {name}: bad size {width} × {height} × {depth}"))?;

        if data.is_some_and(|d| d.len() < expected) {
            return Err(format!(
                "texture {name}: {} bytes, {expected} expected",
                data.map_or(0, <[u8]>::len)
            ));
        }
        if let Some(old) = self.textures.remove(name) {
            // SAFETY: texture owned by this layer, replaced
            unsafe { gl.delete_texture(old.handle) };
        }

        let target = if depth > 0 {
            glow::TEXTURE_2D_ARRAY
        } else {
            glow::TEXTURE_2D
        };
        let handle = create_texture(gl, target, format, spec, data.map(|d| &d[..expected]))?;

        self.textures.insert(
            name.to_owned(),
            Texture {
                handle,
                unit,
                target,
                format,
                size: (width, height, depth),
            },
        );

        Ok(())
    }

    /// The texture unit of sampler `name`: the one it has, else the next free one; an error past `MAX_UNITS`
    fn texture_unit(&self, name: &str) -> Result<u32, String> {
        let unit = match self.textures.get(name) {
            Some(t) => t.unit,
            None => FIRST_UNIT + self.textures.len() as u32,
        };

        if unit >= MAX_UNITS {
            return Err(format!(
                "too many textures in the ground layer ({} at most)",
                MAX_UNITS - FIRST_UNIT
            ));
        }

        Ok(unit)
    }

    /// Replaces a region of texture `name` with `data` (its texels, row after row)
    pub fn update_texture(&self, gl: &glow::Context, name: &str, region: Region, data: &[u8]) -> Result<(), String> {
        let t = self.textures.get(name).ok_or_else(|| format!("no texture {name}"))?;
        let (_, pixel_format, kind, bytes, _) = t.format;
        let Region {
            x,
            y,
            layer,
            width,
            height,
        } = region;
        let expected = gl::area(width, height)
            .and_then(|texels| texels.checked_mul(bytes))
            .unwrap_or(usize::MAX);

        if data.len() < expected
            || !gl::region_fits(x, y, width, height, (t.size.0, t.size.1))
            || layer < 0
            || layer >= t.size.2.max(1)
        {
            return Err(format!("texture {name}: region out of its bounds or too few bytes"));
        }
        // SAFETY: upload inside the texture's storage
        unsafe {
            gl.bind_texture(t.target, Some(t.handle));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);

            let pixels = glow::PixelUnpackData::Slice(Some(&data[..expected]));

            if t.target == glow::TEXTURE_2D_ARRAY {
                gl.tex_sub_image_3d(t.target, 0, x, y, layer, width, height, 1, pixel_format, kind, pixels);
            } else {
                gl.tex_sub_image_2d(t.target, 0, x, y, width, height, pixel_format, kind, pixels);
            }
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        }

        Ok(())
    }

    /// Sets the application's uniforms (program in use)
    fn apply_values(&self, gl: &glow::Context) {
        for (name, values) in &self.values {
            let Some((location, kind, _)) = self.custom.get(name) else {
                continue;
            };
            let l = Some(location);

            // SAFETY: uniform uploads on the program in use, sizes from the slices
            unsafe {
                match (values, *kind) {
                    (Values::Float(f), glow::FLOAT) => gl.uniform_1_f32_slice(l, f),
                    (Values::Float(f), glow::FLOAT_VEC2) => gl.uniform_2_f32_slice(l, f),
                    (Values::Float(f), glow::FLOAT_VEC3) => gl.uniform_3_f32_slice(l, f),
                    (Values::Float(f), glow::FLOAT_VEC4) => gl.uniform_4_f32_slice(l, f),
                    (Values::Float(f), glow::FLOAT_MAT3) => gl.uniform_matrix_3_f32_slice(l, false, f),
                    (Values::Float(f), glow::FLOAT_MAT4) => gl.uniform_matrix_4_f32_slice(l, false, f),
                    (Values::Int(i), glow::INT | glow::BOOL) => gl.uniform_1_i32_slice(l, i),
                    (Values::Int(i), glow::INT_VEC2 | glow::BOOL_VEC2) => gl.uniform_2_i32_slice(l, i),
                    (Values::Int(i), glow::INT_VEC3 | glow::BOOL_VEC3) => gl.uniform_3_i32_slice(l, i),
                    (Values::Int(i), glow::INT_VEC4 | glow::BOOL_VEC4) => gl.uniform_4_i32_slice(l, i),
                    (Values::Uint(u), glow::UNSIGNED_INT) => gl.uniform_1_u32_slice(l, u),
                    (Values::Uint(u), glow::UNSIGNED_INT_VEC2) => gl.uniform_2_u32_slice(l, u),
                    (Values::Uint(u), glow::UNSIGNED_INT_VEC3) => gl.uniform_3_u32_slice(l, u),
                    (Values::Uint(u), glow::UNSIGNED_INT_VEC4) => gl.uniform_4_u32_slice(l, u),
                    _ => {}
                }
            }
        }
    }
}

/// Uniforms of `PROJECT`, set by `Projection`
fn is_projection(name: &str) -> bool {
    matches!(name, "u_globe" | "u_radius" | "u_east" | "u_north" | "u_up" | "u_body")
}

impl Layer for GroundLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        if !self.visible {
            return false;
        }

        let &Frame {
            gl,
            camera,
            view,
            pixel_ratio,
            relief,
            ground,
            fog,
            ..
        } = frame;
        let [
            matrix,
            heights,
            relief_u,
            eye,
            anchor_offset,
            world,
            bounds,
            zoom,
            ratio,
            fog_u,
            fog_range,
        ] = &self.uniforms;
        let offset = camera.center - self.anchor;
        let (lo, hi) = (self.min - self.anchor, self.max - self.anchor);
        let ([fr, fg, fb], [start, end]) = fog.unwrap_or(([0.0; 3], [0.0; 2]));

        // SAFETY: uniforms, bindings and draws on the live context; the state is set back at the end
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_1_f32(relief_u.as_ref(), relief.map_or(0.0, |(_, e)| e));
            gl.uniform_3_f32(eye.as_ref(), view.eye.x as f32, view.eye.y as f32, view.eye.z as f32);
            gl.uniform_2_f32(anchor_offset.as_ref(), offset.x as f32, offset.y as f32);
            gl.uniform_1_f32(world.as_ref(), view.world_size as f32);
            gl.uniform_4_f32(bounds.as_ref(), lo.x as f32, lo.y as f32, hi.x as f32, hi.y as f32);
            gl.uniform_1_f32(zoom.as_ref(), camera.zoom as f32);
            gl.uniform_1_f32(ratio.as_ref(), pixel_ratio as f32);
            gl.uniform_3_f32(fog_u.as_ref(), fr, fg, fb);
            gl.uniform_2_f32(fog_range.as_ref(), start, end);
            self.projection.set(gl, view);
            gl.uniform_1_i32(heights.as_ref(), STACK as i32);
            gl.active_texture(glow::TEXTURE0 + STACK as u32);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, relief.map(|(source, _)| source.pool()));
            for (name, t) in &self.textures {
                gl.active_texture(glow::TEXTURE0 + t.unit);
                gl.bind_texture(t.target, Some(t.handle));

                if let Some((location, _, _)) = self.custom.get(name) {
                    gl.uniform_1_i32(Some(location), t.unit as i32);
                }
            }
            gl.active_texture(glow::TEXTURE0);
            self.apply_values(gl);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);

            let depth = DepthState::ground(gl, relief.is_some());
            let (min, max) = (self.min, self.max);

            ground.redraw(gl, view, |a, b| {
                a.x <= max.x && a.y <= max.y && b.x >= min.x && b.y >= min.y
            });
            drop(depth);
            gl.blend_func_separate(
                glow::SRC_ALPHA,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }

        false
    }

    fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by this layer, not used afterwards
        unsafe {
            gl.delete_program(self.program);

            for t in self.textures.values() {
                gl.delete_texture(t.handle);
            }
        }
    }
}

#[wasm_bindgen]
impl Engine {
    /// Ground layer running `code` (GLSL ES 3.0 defining `vec4 ground(vec2 offset)`, see `render::ground`) over the
    /// ground; `anchor`: Mercator point the offsets are taken from; bounds: Mercator box it shows in. Compilation errors
    /// come back with the shader's info log (line numbers of `code`)
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) when `code` does not compile or link, with the shader's info log
    #[wasm_bindgen(js_name = addGround)]
    #[expect(
        clippy::too_many_arguments,
        reason = "a wasm-bindgen export: its arguments are the JavaScript API"
    )]
    pub fn add_ground(
        &mut self,
        code: &str,
        anchor_x: f64,
        anchor_y: f64,
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
    ) -> Result<u32, JsValue> {
        let layer = GroundLayer::new(
            &self.gl,
            code,
            self.body().circumference(),
            DVec2::new(anchor_x, anchor_y),
            DVec2::new(min_x, min_y),
            DVec2::new(max_x, max_y),
        )?;

        Ok(self.add_layer(Box::new(layer)))
    }

    /// Moves the Mercator box (0-1) a ground layer shows in
    #[wasm_bindgen(js_name = setGroundBounds)]
    pub fn set_ground_bounds(&mut self, id: u32, min_x: f64, min_y: f64, max_x: f64, max_y: f64) {
        if let Some((layer, _)) = self.layer_mut::<GroundLayer>(id) {
            layer.set_bounds(DVec2::new(min_x, min_y), DVec2::new(max_x, max_y));
        }
    }

    /// Shows or hides a ground layer (kept, with its uniforms and textures)
    #[wasm_bindgen(js_name = setGroundVisible)]
    pub fn set_ground_visible(&mut self, id: u32, visible: bool) {
        if let Some((layer, _)) = self.layer_mut::<GroundLayer>(id) {
            layer.visible = visible;
        }
    }

    /// A uniform of a ground layer's code, as numbers
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) when the shader has no uniform `name` (one its code does not use is removed
    /// by the compiler)
    #[wasm_bindgen(js_name = setGroundUniform)]
    pub fn set_ground_uniform(&mut self, id: u32, name: &str, values: &[f64]) -> Result<(), JsValue> {
        if let Some((layer, _)) = self.layer_mut::<GroundLayer>(id) {
            layer.set_uniform(name, values)?;
        }

        Ok(())
    }

    /// Creates the texture of sampler `name` (`format`: r8, r8ui, r16ui, r16i, r32ui, r32f, rg16ui, rg32f, rgba8,
    /// rgba8ui, rgba16ui, rgba32f; `depth` 0 for 2D, else a 2D array of that many layers; `linear` filtering for the
    /// non-integer formats); `data`: its texels as bytes, or undefined for an empty texture
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) on an unknown `format`, a size not positive (or overflowing), `data` shorter
    /// than the texels, more textures than the layer takes, or a texture WebGL cannot create
    #[wasm_bindgen(js_name = setGroundTexture)]
    #[expect(
        clippy::too_many_arguments,
        clippy::needless_pass_by_value,
        reason = "a wasm-bindgen export: its arguments are the JavaScript API, handed over"
    )]
    pub fn set_ground_texture(
        &mut self,
        id: u32,
        name: &str,
        format: &str,
        width: i32,
        height: i32,
        depth: i32,
        linear: bool,
        data: Option<Box<[u8]>>,
    ) -> Result<(), JsValue> {
        if let Some((layer, gl)) = self.layer_mut::<GroundLayer>(id) {
            let spec = TextureSpec {
                name,
                format,
                width,
                height,
                depth,
                linear,
            };

            layer.set_texture(gl, &spec, data.as_deref())?;
        }

        Ok(())
    }

    /// Replaces a region of a ground layer's texture (one layer of a 2D array)
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) when there is no texture `name`, or when the region is out of its bounds or
    /// `data` holds too few bytes
    #[wasm_bindgen(js_name = updateGroundTexture)]
    #[expect(
        clippy::too_many_arguments,
        reason = "a wasm-bindgen export: its arguments are the JavaScript API"
    )]
    pub fn update_ground_texture(
        &mut self,
        id: u32,
        name: &str,
        x: i32,
        y: i32,
        layer: i32,
        width: i32,
        height: i32,
        data: &[u8],
    ) -> Result<(), JsValue> {
        if let Some((ground, gl)) = self.layer_mut::<GroundLayer>(id) {
            let region = Region {
                x,
                y,
                layer,
                width,
                height,
            };

            ground.update_texture(gl, name, region, data)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_values_take_the_uniform_type() {
        assert!(matches!(Values::of(glow::FLOAT_VEC2, &[0.5, 1.5]), Values::Float(f) if f == [0.5, 1.5]));
        assert!(matches!(Values::of(glow::BOOL, &[1.0]), Values::Int(i) if i == [1]));
        assert!(matches!(Values::of(glow::UNSIGNED_INT_VEC2, &[3.0, 4.0]), Values::Uint(u) if u == [3, 4]));
    }

    #[test]
    fn texture_formats_are_known_by_name() {
        assert_eq!(format("r32f").map(|f| f.3), Some(4));
        assert_eq!(format("rgba8ui").map(|f| f.4), Some(true));
        assert!(format("rgb565").is_none());
    }
}
