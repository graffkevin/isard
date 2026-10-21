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

/// The vertex shader of the passes over the whole screen: one triangle covering it, drawn with 3 vertices and no
/// attribute (an empty vertex array); `v_ndc` the normalized device coordinates of the pixel
pub const FULL_SCREEN_VERTEX: &str = r"#version 300 es
out vec2 v_ndc;
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)) * 2.0 - 1.0;
  v_ndc = p;
  gl_Position = vec4(p, 0.0, 1.0);
}";

/// A depth (and stencil) texture the size of the canvas and its framebuffer, made again when the canvas changes size:
/// the scene's depth drawn again for a pass over the screen (the viewshed, panoramas, the surface under a point)
#[derive(Default)]
pub struct DepthTarget(Option<(glow::Texture, glow::Framebuffer, (i32, i32))>);

impl DepthTarget {
    /// The framebuffer and its texture at `size`, made (or made again) if needed
    pub fn at(&mut self, gl: &glow::Context, size: (i32, i32)) -> Result<(glow::Framebuffer, glow::Texture), String> {
        if let Some((texture, framebuffer, at)) = self.0
            && at == size
        {
            return Ok((framebuffer, texture));
        }
        self.delete(gl);

        let mut objects = Objects::new(gl);
        let texture = objects.texture()?;
        let framebuffer = objects.framebuffer()?;

        objects.keep();
        // SAFETY: storage and attachment of objects just created on the live context
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::DEPTH24_STENCIL8, size.0, size.1);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_STENCIL_ATTACHMENT,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.0 = Some((texture, framebuffer, size));

        Ok((framebuffer, texture))
    }

    pub fn delete(&mut self, gl: &glow::Context) {
        if let Some((texture, framebuffer, _)) = self.0.take() {
            // SAFETY: objects owned by the target, not used afterwards
            unsafe {
                gl.delete_texture(texture);
                gl.delete_framebuffer(framebuffer);
            }
        }
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

/// Sets the texture units of samplers of a program, once after it is linked (they never change): the program is left
/// in use
pub fn sampler_units(gl: &glow::Context, program: glow::Program, units: &[(Option<&glow::UniformLocation>, u32)]) {
    // SAFETY: uniforms of a linked program, put in use
    unsafe {
        gl.use_program(Some(program));

        for &(location, unit) in units {
            gl.uniform_1_i32(location, unit as i32);
        }
    }
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
        let mut vertices: Vec<f32> = Vec::new();

        for j in 0..n {
            for i in 0..n {
                vertices.extend([f32::from(i) / f32::from(GRID), f32::from(j) / f32::from(GRID)]);
            }
        }

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

/// Allocates and fills level 0 of the bound `TEXTURE_2D` from a decoded image (RGBA8): the textures of the 3D Tiles
#[cfg(feature = "tiles3d")]
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
/// WebGL2 is. GLSL: `table_texel(i)` (`COMMON`)
pub const TABLE_WIDTH: usize = 4096;
const _: () = assert!(TABLE_WIDTH.is_power_of_two());

/// Texels of the ramps (RGBA8, one row) that turn values into colors: the feature layers' and the networks'
pub const RAMP_TEXELS: usize = 256;

/// Texture units, set once per program (`sampler_units`). The raster pass takes 0 to `STACK - 1` for its tile pools and
/// `STACK` for the relief, the ground layers their own textures from `STACK + 1` up (`render::ground`); the layers
/// drawn over the ground share these
pub mod unit {
    /// A layer's own textures (a line layer's table, a network's values…)
    pub const OWN: [u32; 5] = [1, 2, 3, 4, 5];
    /// The drape: the page table and the elevation pool (`render::drape`)
    pub const PAGE: u32 = 6;
    pub const HEIGHTS: u32 = 7;
    /// The feature textures (`render::features`)
    pub const STYLE: u32 = 8;
    pub const VALUES: u32 = 9;
    pub const MASK: u32 = 10;
    pub const RAMP: u32 = 11;
}

/// GLSL shared by the programs of the layers, generated from the constants above: `table_texel(i)`, the texel of entry
/// `i` of a table; `ramp_uv(t)`, where a ramp holds the color at `t` (0 to 1, clamped); `pick_color(id, layer)`, the
/// color of the picking pass for an id (a feature's number + 1, 24 bits, little end in red) and a layer
#[must_use]
pub fn common() -> String {
    let last = RAMP_TEXELS - 1;

    format!(
        "ivec2 table_texel(highp uint i) {{ return ivec2(int(i & {mask}u), int(i >> {shift}u)); }}
vec2 ramp_uv(float t) {{ return vec2((clamp(t, 0.0, 1.0) * {last}.0 + 0.5) / {RAMP_TEXELS}.0, 0.5); }}
vec4 pick_color(highp uint id, float layer) {{
  return vec4(float(id & 255u), float((id >> 8) & 255u), float((id >> 16) & 255u), layer) / 255.0;
}}
",
        mask = TABLE_WIDTH - 1,
        shift = TABLE_WIDTH.trailing_zeros(),
    )
}

/// A vertex shader of the layers: `common()` then `parts`
#[must_use]
pub fn vertex_shader(parts: &[&str]) -> String {
    shader("#version 300 es\n", parts)
}

/// A fragment shader of the layers: high precision, `common()`, then `parts`
#[must_use]
pub fn fragment_shader(parts: &[&str]) -> String {
    shader("#version 300 es\nprecision highp float;\nprecision highp int;\n", parts)
}

fn shader(head: &str, parts: &[&str]) -> String {
    let mut source = head.to_owned() + &common();

    for part in parts {
        source.push_str(part);
    }

    source
}

/// Sets the state every layer starts from (`Layer`): blending on (straight alpha), back faces culled (clockwise front
/// faces), no depth or stencil test, depth and stencil written (`LEQUAL` when tested), every color channel written, no
/// scissor.
/// The frame and the picking pass start from it whatever was left before
pub fn reset_state(gl: &glow::Context) {
    // SAFETY: state changes on a live context
    unsafe {
        gl.enable(glow::BLEND);
        gl.blend_func_separate(
            glow::SRC_ALPHA,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );
        gl.enable(glow::CULL_FACE);
        gl.front_face(glow::CW);
        gl.disable(glow::DEPTH_TEST);
        gl.depth_func(glow::LEQUAL);
        gl.depth_mask(true);
        gl.disable(glow::STENCIL_TEST);
        gl.stencil_mask(0xff);
        gl.disable(glow::SCISSOR_TEST);
        gl.color_mask(true, true, true, true);
    }
}

/// Sets back what the page's own drawing (custom layers) may have changed besides `reset_state`'s state: the canvas
/// bound with its viewport, no program, vertex array nor buffers bound, texture unit 0, the default blending equation,
/// culling and stencil operations, no polygon offset nor coverage, the full depth range, the default pixel storage of
/// uploads; then `reset_state`. The engine binds its own programs, arrays and textures before each draw
pub fn reset_after_page(gl: &glow::Context, width: i32, height: i32) {
    // SAFETY: state changes on a live context
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.viewport(0, 0, width, height);
        gl.use_program(None);
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
        gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
        gl.active_texture(glow::TEXTURE0);
        gl.blend_equation(glow::FUNC_ADD);
        gl.cull_face(glow::BACK);
        gl.stencil_func(glow::ALWAYS, 0, 0xff);
        gl.stencil_op(glow::KEEP, glow::KEEP, glow::KEEP);
        for capability in [
            glow::POLYGON_OFFSET_FILL,
            glow::SAMPLE_ALPHA_TO_COVERAGE,
            glow::RASTERIZER_DISCARD,
        ] {
            gl.disable(capability);
        }
        gl.depth_range(0.0, 1.0);

        for (parameter, value) in [
            (glow::UNPACK_ALIGNMENT, 4),
            (glow::PACK_ALIGNMENT, 4),
            // UNPACK_FLIP_Y_WEBGL, UNPACK_PREMULTIPLY_ALPHA_WEBGL (WebGL's own, not in glow)
            (0x9240, 0),
            (0x9241, 0),
            (glow::UNPACK_ROW_LENGTH, 0),
            (glow::UNPACK_SKIP_ROWS, 0),
            (glow::UNPACK_SKIP_PIXELS, 0),
        ] {
            gl.pixel_store_i32(parameter, value);
        }
    }
    reset_state(gl);
}

/// The depth state of a layer while it draws, set back to `reset_state`'s when dropped (culling on, no depth test,
/// depth written). Culling is off but for the flat ground: quads built on screen face either way, and blocks are seen
/// from inside
#[must_use = "the state is set back when the guard is dropped"]
pub struct DepthState<'a>(&'a glow::Context);

impl<'a> DepthState<'a> {
    /// Drawn over the ground: in relief, tested against the ground's depth (`LEQUAL`, the hills hide what lies behind
    /// them) and not written; flat, no test
    pub fn over_ground(gl: &'a glow::Context, relief: bool) -> Self {
        // SAFETY: state changes on a live context, set back by `drop`
        unsafe {
            gl.disable(glow::CULL_FACE);

            if relief {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LEQUAL);
                gl.depth_mask(false);
            }
        }

        Self(gl)
    }

    /// The ground drawn again (`RasterPass::redraw`): in relief as `over_ground`, both faces (the skirts hang either
    /// side); flat, as the frame starts (the far side of the globe culled)
    pub fn ground(gl: &'a glow::Context, relief: bool) -> Self {
        if relief {
            return Self::over_ground(gl, true);
        }

        Self(gl)
    }

    /// Solid: tested (`LEQUAL`) and written, the blocks of an extrusion layer hiding one another
    pub fn solid(gl: &'a glow::Context) -> Self {
        // SAFETY: state changes on a live context, set back by `drop`
        unsafe {
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
        }

        Self(gl)
    }
}

impl Drop for DepthState<'_> {
    fn drop(&mut self) {
        let gl = self.0;

        // SAFETY: state changes on a live context
        unsafe {
            gl.enable(glow::CULL_FACE);
            gl.disable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
        }
    }
}

/// An attribute of the instances of a buffer: location, components, whether they are floats (else one u32), byte
/// offset in the instance
pub type Attribute = (u32, i32, bool, i32);

/// Draws ranges of instances (`first..end`) of the bound vertex array, `vertices` each as a triangle strip: WebGL2 has
/// no base instance, so the instance attributes are pointed at the first instance of each range (buffer bound to
/// `ARRAY_BUFFER`, `stride` bytes per instance)
pub fn draw_instances(gl: &glow::Context, attributes: &[Attribute], stride: i32, vertices: i32, ranges: &[(u32, u32)]) {
    // SAFETY: the caller bound the vertex array and the instance buffer; the ranges are within it
    unsafe {
        for &(first, end) in ranges {
            let base = first as i32 * stride;

            for &(location, size, float, offset) in attributes {
                if float {
                    gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, base + offset);
                } else {
                    gl.vertex_attrib_pointer_i32(location, size, glow::UNSIGNED_INT, stride, base + offset);
                }
            }
            gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, vertices, (end - first) as i32);
        }
    }
}

/// A table texture kept from one content to the next (`table_texture`): reused while what comes fits, replaced by a
/// larger one (half as large again as needed, room for what follows) otherwise, or by a smaller one when it would
/// stay four times too large. A new texture is made before the old one is deleted: if it cannot be, nothing changes
pub struct Table {
    pub texture: glow::Texture,
    format: u32,
    /// Texels it holds
    capacity: usize,
}

impl Table {
    pub fn new(gl: &glow::Context, format: u32, texels: usize) -> Result<Self, String> {
        Ok(Self {
            texture: table_texture(gl, format, texels)?,
            format,
            capacity: table_rows(texels) * TABLE_WIDTH,
        })
    }

    /// A texture with room for `texels` if this one has not (or is far too large), for `replace`: half as large again
    /// as needed, or just enough if the GPU takes no more. Without one, this table's texture is left bound
    pub fn prepare(&self, gl: &glow::Context, texels: usize) -> Result<Option<(glow::Texture, usize)>, String> {
        let rows = table_rows(texels);

        if keeps(self.capacity / TABLE_WIDTH, rows) {
            // SAFETY: binding a texture owned by this table
            unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(self.texture)) };

            return Ok(None);
        }

        let roomy = rows.saturating_add(rows / 2).saturating_mul(TABLE_WIDTH);
        let made = table_texture(gl, self.format, roomy)
            .map(|t| (t, roomy))
            .or_else(|_| table_texture(gl, self.format, texels).map(|t| (t, rows * TABLE_WIDTH)))?;

        Ok(Some(made))
    }

    /// Takes the texture `prepare` made (if any), the old one deleted
    pub fn replace(&mut self, gl: &glow::Context, made: Option<(glow::Texture, usize)>) {
        if let Some((texture, capacity)) = made {
            // SAFETY: the old texture, replaced, is not used afterwards
            unsafe { gl.delete_texture(self.texture) };
            (self.texture, self.capacity) = (texture, capacity);
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: texture owned by this table, not used afterwards
        unsafe { gl.delete_texture(self.texture) };
    }
}

/// Whether a table of `have` rows is kept for `rows`: they fit, and it is not four times too large
fn keeps(have: usize, rows: usize) -> bool {
    rows <= have && rows.saturating_mul(4) >= have
}

/// Rows of a table of `texels` texels (at least one)
#[must_use]
pub fn table_rows(texels: usize) -> usize {
    texels.div_ceil(TABLE_WIDTH).max(1)
}

/// Rows of a table of `texels` texels if the GPU takes them (`max`: its `MAX_TEXTURE_SIZE`), else an error saying how
/// many entries fit: past it the texture would be missing and the layer drawn black, without a word
pub fn table_rows_within(texels: usize, max: i32) -> Result<i32, String> {
    let rows = table_rows(texels);

    match i32::try_from(rows) {
        Ok(rows) if rows <= max => Ok(rows),
        _ => Err(format!(
            "{texels} table entries: more than this GPU takes ({} at most)",
            usize::try_from(max).unwrap_or(0) * TABLE_WIDTH
        )),
    }
}

/// A table texture of `texels` texels in `format`, `TABLE_WIDTH` wide, nearest filtering, left bound; an error if the
/// GPU cannot hold that many (`table_rows_within`)
pub fn table_texture(gl: &glow::Context, format: u32, texels: usize) -> Result<glow::Texture, String> {
    // SAFETY: creation and storage on a live context
    unsafe {
        let rows = table_rows_within(texels, gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE))?;
        let texture = gl.create_texture()?;

        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_storage_2d(glow::TEXTURE_2D, 1, format, TABLE_WIDTH as i32, rows);
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

    #[test]
    fn tables_are_kept_while_they_fit() {
        assert!(keeps(1, 1) && keeps(6, 4) && keeps(4, 1));
        // Too small, or four times too large: replaced
        assert!(!keeps(4, 5) && !keeps(5, 1));

        let shader = common();

        assert!(shader.contains("i & 4095u") && shader.contains("i >> 12u"), "{shader}");
        assert!(shader.contains("* 255.0 + 0.5) / 256.0"), "{shader}");
    }

    #[test]
    fn tables_past_the_gpu_are_errors() {
        // 2.1 M features of 8 texels on a GPU of 4096: past it
        assert_eq!(table_rows_within(4096 * 4096, 4096), Ok(4096));
        assert!(table_rows_within(4096 * 4096 + 1, 4096).is_err());
        assert!(
            table_rows_within(2_100_000 * 8, 4096)
                .unwrap_err()
                .contains("16777216 at most")
        );
        assert!(table_rows_within(usize::MAX, 16_384).is_err());
        assert!(table_rows_within(10, 0).is_err());
    }
}
