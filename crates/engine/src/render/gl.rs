//! Small WebGL2 helpers on top of glow

use glow::HasContext;

/// Compiles and links a program; the error carries the info log. Nothing is left behind on a failure
pub fn program(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    // SAFETY: plain GL calls on a live context, every object created here is attached and deleted, or deleted on
    // the way out of an error
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        let mut error = None;
        for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let shader = match gl.create_shader(kind) {
                Ok(shader) => shader,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            };
            shaders.push(shader);
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                error = Some(gl.get_shader_info_log(shader));
                break;
            }
            gl.attach_shader(program, shader);
        }
        if error.is_none() {
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                error = Some(gl.get_program_info_log(program));
            }
        }
        for shader in shaders {
            // Detaching a shader never attached is a harmless GL error
            if error.is_none() {
                gl.detach_shader(program, shader);
            }
            gl.delete_shader(shader);
        }
        match error {
            Some(e) => {
                gl.delete_program(program);
                Err(e)
            }
            None => Ok(program),
        }
    }
}

/// The GL objects a constructor creates, deleted together when it fails half-way (an early `?`): no leak on a partial
/// failure. `keep` hands them over once everything succeeded
pub struct Objects<'a> {
    gl: &'a glow::Context,
    programs: Vec<glow::Program>,
    vertex_arrays: Vec<glow::VertexArray>,
    buffers: Vec<glow::Buffer>,
    textures: Vec<glow::Texture>,
    framebuffers: Vec<glow::Framebuffer>,
    renderbuffers: Vec<glow::Renderbuffer>,
}

impl<'a> Objects<'a> {
    pub fn new(gl: &'a glow::Context) -> Self {
        Self {
            gl,
            programs: Vec::new(),
            vertex_arrays: Vec::new(),
            buffers: Vec::new(),
            textures: Vec::new(),
            framebuffers: Vec::new(),
            renderbuffers: Vec::new(),
        }
    }

    pub fn program(&mut self, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
        let program = program(self.gl, vertex, fragment)?;
        self.programs.push(program);
        Ok(program)
    }

    pub fn vertex_array(&mut self) -> Result<glow::VertexArray, String> {
        // SAFETY: creation on a live context, tracked for deletion
        let vao = unsafe { self.gl.create_vertex_array()? };
        self.vertex_arrays.push(vao);
        Ok(vao)
    }

    pub fn buffer(&mut self) -> Result<glow::Buffer, String> {
        // SAFETY: creation on a live context, tracked for deletion
        let buffer = unsafe { self.gl.create_buffer()? };
        self.buffers.push(buffer);
        Ok(buffer)
    }

    pub fn texture(&mut self) -> Result<glow::Texture, String> {
        // SAFETY: creation on a live context, tracked for deletion
        let texture = unsafe { self.gl.create_texture()? };
        self.textures.push(texture);
        Ok(texture)
    }

    /// A texture already made (`table_texture`…), deleted with the others on a failure
    pub fn adopt_texture(&mut self, texture: glow::Texture) -> glow::Texture {
        self.textures.push(texture);
        texture
    }

    pub fn framebuffer(&mut self) -> Result<glow::Framebuffer, String> {
        // SAFETY: creation on a live context, tracked for deletion
        let framebuffer = unsafe { self.gl.create_framebuffer()? };
        self.framebuffers.push(framebuffer);
        Ok(framebuffer)
    }

    pub fn renderbuffer(&mut self) -> Result<glow::Renderbuffer, String> {
        // SAFETY: creation on a live context, tracked for deletion
        let renderbuffer = unsafe { self.gl.create_renderbuffer()? };
        self.renderbuffers.push(renderbuffer);
        Ok(renderbuffer)
    }

    /// Everything succeeded: the objects belong to the caller from now on
    pub fn keep(mut self) {
        self.programs.clear();
        self.vertex_arrays.clear();
        self.buffers.clear();
        self.textures.clear();
        self.framebuffers.clear();
        self.renderbuffers.clear();
    }
}

impl Drop for Objects<'_> {
    fn drop(&mut self) {
        let gl = self.gl;
        // SAFETY: objects created by this guard and never handed over (`keep` empties the lists)
        unsafe {
            for &p in &self.programs {
                gl.delete_program(p);
            }
            for &v in &self.vertex_arrays {
                gl.delete_vertex_array(v);
            }
            for &b in &self.buffers {
                gl.delete_buffer(b);
            }
            for &t in &self.textures {
                gl.delete_texture(t);
            }
            for &f in &self.framebuffers {
                gl.delete_framebuffer(f);
            }
            for &r in &self.renderbuffers {
                gl.delete_renderbuffer(r);
            }
        }
    }
}

/// Uniform locations of a program, looked up once
pub fn uniforms<const N: usize>(
    gl: &glow::Context,
    program: glow::Program,
    names: [&str; N],
) -> [Option<glow::UniformLocation>; N] {
    // SAFETY: lookup on a linked program
    names.map(|name| unsafe { gl.get_uniform_location(program, name) })
}

/// Column-major f32 copy of an f64 matrix, for `uniform_matrix_4_f32_slice`
#[must_use]
pub fn mat4(m: &glam::DMat4) -> [f32; 16] {
    m.as_mat4().to_cols_array()
}

/// GLSL shared by the layers: position in the local frame of a point given on the plane (pixels relative to the
/// center) and in Mercator, blended between the plane and the sphere like `View::position`; `project_up` lifts it
/// `h` metres above the ellipsoid (along the vertical of the plane or the normal of the sphere)
pub const PROJECT: &str = r"
uniform float u_globe;
uniform float u_radius;
uniform vec3 u_east;
uniform vec3 u_north;
uniform vec3 u_up;
uniform float u_world;
// The body: its circumference and radius (metres)
uniform vec2 u_body;

vec3 project_up(vec2 plane, vec2 merc, float h) {
  float lat = atan(sinh(3.1415927 * (1.0 - 2.0 * merc.y)));
  // Pixels per metre on the plane at this latitude (Web Mercator scale), as View::pixels_per_metre
  vec3 flat_ = vec3(plane, h * u_world / (u_body.x * cos(lat)));
  if (u_globe <= 0.0) return flat_;
  float lon = merc.x * 6.2831853 - 3.1415927;
  vec3 n = vec3(cos(lat) * cos(lon), cos(lat) * sin(lon), sin(lat));
  vec3 local = vec3(dot(n, u_east), -dot(n, u_north), dot(n, u_up));
  vec3 sphere = u_radius * (local + vec3(0.0, 0.0, -1.0)) + local * (h * u_radius / u_body.y);
  return mix(flat_, sphere, u_globe);
}

vec3 project(vec2 plane, vec2 merc) { return project_up(plane, merc, 0.0); }
";

/// Uniforms of `PROJECT`
pub struct Projection([Option<glow::UniformLocation>; 7]);

impl Projection {
    pub fn new(gl: &glow::Context, program: glow::Program) -> Self {
        Self(uniforms(
            gl,
            program,
            ["u_globe", "u_radius", "u_east", "u_north", "u_up", "u_world", "u_body"],
        ))
    }

    /// Call with the program in use
    pub fn set(&self, gl: &glow::Context, view: &geo::camera::View) {
        let [factor, radius, east, north, up, world, body] = &self.0;
        let g = &view.globe;
        // SAFETY: uniform uploads on the program in use
        unsafe {
            gl.uniform_1_f32(factor.as_ref(), g.factor as f32);
            gl.uniform_1_f32(radius.as_ref(), g.radius as f32);
            gl.uniform_1_f32(world.as_ref(), view.world_size as f32);
            gl.uniform_2_f32(body.as_ref(), view.body.circumference() as f32, view.body.radius as f32);
            for (location, axis) in [(east, g.east), (north, g.north), (up, g.up)] {
                gl.uniform_3_f32(location.as_ref(), axis.x as f32, axis.y as f32, axis.z as f32);
            }
        }
    }
}

/// Subdivisions of the tile mesh on the globe
pub const GRID: u16 = 16;

/// Static mesh of a unit square: (GRID + 1)² vertices (location 0, vec2 in [0, 1]), then a copy of the border ones
/// shifted by +2 (the skirt, hanging under the tile edges in relief: it hides the cracks between tiles of different
/// levels). Indices: the full grid (`GRID² × 6`), the skirt (both windings), then the 6 indices of the corners only
/// (for the flat plane). Clockwise on screen, the front faces (`front_face(CW)`).
pub struct Grid {
    pub vertices: glow::Buffer,
    pub indices: glow::Buffer,
}

impl Grid {
    pub const FULL: i32 = (GRID as i32) * (GRID as i32) * 6;
    /// The skirt: 4 × GRID border segments, two triangles each, both windings
    pub const SKIRT: i32 = 4 * (GRID as i32) * 12;
    /// Byte offset and count of the corner-only quad
    pub const CORNERS: (i32, i32) = ((Self::FULL + Self::SKIRT) * 2, 6);

    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        let n = GRID + 1;
        let mut vertices: Vec<f32> = (0..n)
            .flat_map(|j| (0..n).flat_map(move |i| [f32::from(i) / f32::from(GRID), f32::from(j) / f32::from(GRID)]))
            .collect();
        let mut indices: Vec<u16> = Vec::new();
        for j in 0..GRID {
            for i in 0..GRID {
                let a = j * n + i;
                indices.extend_from_slice(&[a, a + 1, a + n, a + 1, a + n + 1, a + n]);
            }
        }
        // The border, once around: top, right, bottom, left
        let mut border: Vec<u16> = Vec::new();
        border.extend(0..GRID);
        border.extend((0..GRID).map(|j| j * n + GRID));
        border.extend((0..GRID).map(|i| GRID * n + GRID - i));
        border.extend((0..GRID).map(|j| (GRID - j) * n));
        let skirt = |k: usize| n * n + k as u16;
        for &b in &border {
            let (x, y) = (vertices[usize::from(b) * 2], vertices[usize::from(b) * 2 + 1]);
            vertices.extend_from_slice(&[x + 2.0, y + 2.0]);
        }
        for k in 0..border.len() {
            let next = (k + 1) % border.len();
            let (a, b, sa, sb) = (border[k], border[next], skirt(k), skirt(next));
            indices.extend_from_slice(&[a, b, sa, b, sb, sa, a, sa, b, b, sa, sb]);
        }
        let (last, row) = (n * n - 1, GRID);
        indices.extend_from_slice(&[0, row, n * GRID, row, last, n * GRID]);
        let mut objects = Objects::new(gl);
        let vertex_buffer = objects.buffer()?;
        let index_buffer = objects.buffer()?;
        objects.keep();
        // SAFETY: uploads into buffers just created on a live context
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&vertices), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes(&indices), glow::STATIC_DRAW);
            Ok(Self {
                vertices: vertex_buffer,
                indices: index_buffer,
            })
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: buffers owned by the mesh, not used afterwards
        unsafe {
            gl.delete_buffer(self.vertices);
            gl.delete_buffer(self.indices);
        }
    }

    /// Binds the mesh into the current vertex array: vertices at location 0, indices
    pub fn bind(&self, gl: &glow::Context) {
        // SAFETY: binding owned buffers into the current vertex array
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vertices));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.indices));
        }
    }

    /// Instanced draw of the mesh: in relief the full grid and its skirt, else the full grid on the globe and two
    /// triangles on the plane
    pub fn draw(gl: &glow::Context, view: &geo::camera::View, instances: i32, relief: bool) {
        let (offset, count) = if relief {
            (0, Self::FULL + Self::SKIRT)
        } else if view.globe.factor > 0.0 {
            (0, Self::FULL)
        } else {
            Self::CORNERS
        };
        // SAFETY: draw with the vertex array bound by the caller
        unsafe {
            gl.draw_elements_instanced(glow::TRIANGLES, count, glow::UNSIGNED_SHORT, offset, instances);
        }
    }
}

/// Numbers viewed as bytes for an upload (checked at compile time: plain types without padding)
pub fn bytes<T: bytemuck::NoUninit>(data: &[T]) -> &[u8] {
    bytemuck::cast_slice(data)
}

/// The glow context of a WebGL2 context (only in the browser: natively, the pure logic is compiled for its tests)
#[cfg_attr(target_arch = "wasm32", allow(clippy::unnecessary_wraps))]
pub fn context(webgl: web_sys::WebGl2RenderingContext) -> Result<glow::Context, String> {
    #[cfg(target_arch = "wasm32")]
    return Ok(glow::Context::from_webgl2_context(webgl));
    #[cfg(not(target_arch = "wasm32"))]
    {
        drop(webgl);
        Err("WebGL2 is only available in the browser".to_owned())
    }
}

/// Uploads a decoded image into layer `layer` of the bound `TEXTURE_2D_ARRAY` (`size` × `size`, RGBA8)
pub fn upload_bitmap_layer(gl: &glow::Context, layer: i32, size: i32, bitmap: &web_sys::ImageBitmap) {
    // SAFETY: upload into the bound texture array, inside its storage (checked by the caller)
    #[cfg(target_arch = "wasm32")]
    unsafe {
        gl.tex_sub_image_3d_with_image_bitmap(
            glow::TEXTURE_2D_ARRAY,
            0,
            0,
            0,
            layer,
            size,
            size,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            bitmap,
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (gl, layer, size, bitmap);
}

/// Allocates and fills level 0 of the bound `TEXTURE_2D` from a decoded image (RGBA8)
pub fn image_bitmap_2d(gl: &glow::Context, bitmap: &web_sys::ImageBitmap) {
    // SAFETY: (re)allocation of the bound texture from the image
    #[cfg(target_arch = "wasm32")]
    unsafe {
        gl.tex_image_2d_with_image_bitmap(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            bitmap,
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (gl, bitmap);
}

/// Width of the tables read with `texelFetch` (an entry per texel, row after row): 4096 is supported everywhere
/// WebGL2 is. GLSL: `ivec2(int(i & 4095u), int(i >> 12))`
pub const TABLE_WIDTH: usize = 4096;

/// Rows of a table of `texels` texels (at least one)
#[must_use]
pub fn table_rows(texels: usize) -> usize {
    texels.div_ceil(TABLE_WIDTH).max(1)
}

/// A table texture of `texels` texels in `format`, `TABLE_WIDTH` wide, nearest filtering, left bound
pub fn table_texture(gl: &glow::Context, format: u32, texels: usize) -> Result<glow::Texture, String> {
    // SAFETY: creation and storage on a live context
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_storage_2d(
            glow::TEXTURE_2D,
            1,
            format,
            TABLE_WIDTH as i32,
            table_rows(texels) as i32,
        );
        for (key, value) in [
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, key, value as i32);
        }
        Ok(texture)
    }
}

/// Uploads `texels` texels of `data` (`size` bytes each) into the bound table texture from its first texel: the full
/// rows in one call, the rest of the last row in another. Rows of 1-byte texels need `UNPACK_ALIGNMENT` 1 only for
/// the rest, which this sets and resets
pub fn upload_table(gl: &glow::Context, data: &[u8], texels: usize, format: u32, kind: u32, size: usize) {
    let texels = texels.min(data.len() / size.max(1));
    let (full, rest) = (texels / TABLE_WIDTH, texels % TABLE_WIDTH);
    // SAFETY: the caller bound a table texture with room for `texels` texels; the slices are within `data`
    unsafe {
        if full > 0 {
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                TABLE_WIDTH as i32,
                full as i32,
                format,
                kind,
                glow::PixelUnpackData::Slice(Some(&data[..full * TABLE_WIDTH * size])),
            );
        }
        if rest > 0 {
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                full as i32,
                rest as i32,
                1,
                format,
                kind,
                glow::PixelUnpackData::Slice(Some(&data[full * TABLE_WIDTH * size..texels * size])),
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        }
    }
}

/// Texels of a `width` × `height` region, None if a side is negative or the count overflows
#[must_use]
pub fn area(width: i32, height: i32) -> Option<usize> {
    usize::try_from(width).ok()?.checked_mul(usize::try_from(height).ok()?)
}

/// Whether the region `x`, `y`, `width` × `height` lies within a texture of `size` (no overflow)
#[must_use]
pub fn region_fits(x: i32, y: i32, width: i32, height: i32, size: (i32, i32)) -> bool {
    let fits =
        |at: i32, len: i32, total: i32| at >= 0 && len >= 0 && i64::from(at) + i64::from(len) <= i64::from(total);
    fits(x, width, size.0) && fits(y, height, size.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_and_regions_never_overflow() {
        assert_eq!(area(4096, 4096), Some(1 << 24));
        assert_eq!(area(-1, 10), None);
        assert_eq!(area(i32::MAX, i32::MAX), Some(i32::MAX as usize * i32::MAX as usize));
        assert!(region_fits(0, 0, 10, 10, (10, 10)));
        assert!(!region_fits(1, 0, 10, 10, (10, 10)));
        assert!(!region_fits(i32::MAX, 0, 10, 1, (10, 10)));
        assert!(!region_fits(-1, 0, 1, 1, (10, 10)));
    }

    #[test]
    fn tables_have_at_least_a_row() {
        assert_eq!(table_rows(0), 1);
        assert_eq!(table_rows(TABLE_WIDTH), 1);
        assert_eq!(table_rows(TABLE_WIDTH + 1), 2);
    }
}
