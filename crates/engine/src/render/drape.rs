//! Layers draped on the relief: where their vertices find the altitudes. The elevation tiles sit in the slots of a
//! texture array at various levels; a page table over the layer's extent (one texel per tile of a fixed level) gives
//! the slot and level of the deepest loaded tile at each place, so a vertex finds its altitude with two fetches and a
//! bilinear read, as the ground does. The table is rebuilt only when the pool changes.

use crate::render::raster::RasterSource;
use geo::page::table;
use glam::DVec2;
use glow::HasContext;

/// GLSL: `altitude(merc)` in metres (0 off the table or before any tile), times `u_relief`
pub const ALTITUDE: &str = r"
uniform highp usampler2D u_page;
uniform highp sampler2DArray u_heights;
// First cell (x, y) and cells per world (2^level) of the table
uniform vec3 u_page_rect;
// Vertical exaggeration; 0: flat
uniform float u_relief;

float altitude(vec2 merc) {
  if (u_relief <= 0.0) return 0.0;
  ivec2 cell = ivec2(floor(merc * u_page_rect.z - u_page_rect.xy));
  ivec2 size = textureSize(u_page, 0);
  if (any(lessThan(cell, ivec2(0))) || any(greaterThanEqual(cell, size))) return 0.0;
  uvec4 entry = texelFetch(u_page, cell, 0);
  if (entry.b == 0u) return 0.0;
  // Position in the elevation tile, samples at the texel centers
  vec2 uv = fract(merc * exp2(float(entry.g)));
  vec2 p = clamp(uv * 256.0 - 0.5, 0.0, 255.0);
  ivec2 i = ivec2(floor(p));
  ivec2 j = min(i + 1, ivec2(255));
  vec2 f = p - vec2(i);
  int s = int(entry.r);
  float h00 = texelFetch(u_heights, ivec3(i.x, i.y, s), 0).r;
  float h10 = texelFetch(u_heights, ivec3(j.x, i.y, s), 0).r;
  float h01 = texelFetch(u_heights, ivec3(i.x, j.y, s), 0).r;
  float h11 = texelFetch(u_heights, ivec3(j.x, j.y, s), 0).r;
  return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y) * u_relief;
}
";

/// Texture units of the table and the elevation pool, kept apart from the layers' own
pub const PAGE_UNIT: u32 = 6;
pub const HEIGHTS_UNIT: u32 = 7;

/// Page table of one layer
pub struct Drape {
    texture: Option<glow::Texture>,
    /// Pool generation the table was built from (0: never)
    generation: u64,
    /// First cell and level of the table
    first: (u32, u32),
    level: u8,
}

impl Drape {
    pub fn new() -> Self {
        Self {
            texture: None,
            generation: 0,
            first: (0, 0),
            level: 0,
        }
    }

    /// Rebuilds the table of the extent `min`..`max` if the pool changed since
    pub fn update(&mut self, gl: &glow::Context, min: DVec2, max: DVec2, source: &RasterSource) {
        if source.generation() == self.generation {
            return;
        }
        self.generation = source.generation();
        let t = table(min, max, source.options.max_z, source.loaded());
        self.first = t.first;
        self.level = t.level;
        // SAFETY: (re)allocation and upload of the table on the live context
        unsafe {
            let texture = match self.texture {
                Some(texture) => texture,
                None => match gl.create_texture() {
                    Ok(texture) => {
                        self.texture = Some(texture);
                        texture
                    }
                    Err(_) => return,
                },
            };
            gl.active_texture(glow::TEXTURE0 + PAGE_UNIT);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8UI as i32,
                t.size.0 as i32,
                t.size.1 as i32,
                0,
                glow::RGBA_INTEGER,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(t.cells.as_flattened())),
            );
            // Back to the default, which the other uploads expect
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.active_texture(glow::TEXTURE0);
        }
    }

    /// Binds the table and the pool, sets the uniforms of `ALTITUDE` (program in use); `relief`: the source and its
    /// exaggeration, None for flat
    pub fn bind(&self, gl: &glow::Context, uniforms: &DrapeUniforms, relief: Option<(&RasterSource, f32)>) {
        let [_, _, rect, exaggeration] = &uniforms.0;
        let relief = relief.filter(|_| self.texture.is_some());
        // SAFETY: uniforms and bindings on the program in use (its sampler units set once, by `DrapeUniforms::new`)
        unsafe {
            gl.uniform_1_f32(exaggeration.as_ref(), relief.map_or(0.0, |(_, e)| e));
            let Some((source, _)) = relief else {
                return;
            };
            let n = f64::from(1u32 << self.level);
            gl.uniform_3_f32(rect.as_ref(), self.first.0 as f32, self.first.1 as f32, n as f32);
            gl.active_texture(glow::TEXTURE0 + PAGE_UNIT);
            gl.bind_texture(glow::TEXTURE_2D, self.texture);
            gl.active_texture(glow::TEXTURE0 + HEIGHTS_UNIT);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(source.pool()));
            gl.active_texture(glow::TEXTURE0);
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        if let Some(texture) = self.texture {
            // SAFETY: texture owned by this table, not used afterwards
            unsafe { gl.delete_texture(texture) };
        }
    }
}

/// Uniform locations of `ALTITUDE` in a program
pub struct DrapeUniforms([Option<glow::UniformLocation>; 4]);

impl DrapeUniforms {
    pub fn new(gl: &glow::Context, program: glow::Program) -> Self {
        let uniforms = crate::render::gl::uniforms(gl, program, ["u_page", "u_heights", "u_page_rect", "u_relief"]);
        crate::render::gl::sampler_units(
            gl,
            program,
            &[(uniforms[0].as_ref(), PAGE_UNIT), (uniforms[1].as_ref(), HEIGHTS_UNIT)],
        );
        Self(uniforms)
    }
}
