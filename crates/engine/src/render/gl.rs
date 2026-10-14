//! Small WebGL2 helpers on top of glow

use glow::HasContext;

/// Compiles and links a program; the error carries the info log
pub fn program(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    // SAFETY: plain GL calls on a live context, every object created here is checked or deleted
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(gl.get_shader_info_log(shader));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            return Err(gl.get_program_info_log(program));
        }
        Ok(program)
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
/// center) and in Mercator, blended between the plane and the sphere like `View::position`
pub const PROJECT: &str = r"
uniform float u_globe;
uniform float u_radius;
uniform vec3 u_east;
uniform vec3 u_north;
uniform vec3 u_up;

vec3 project(vec2 plane, vec2 merc) {
  if (u_globe <= 0.0) return vec3(plane, 0.0);
  float lon = merc.x * 6.2831853 - 3.1415927;
  float lat = atan(sinh(3.1415927 * (1.0 - 2.0 * merc.y)));
  vec3 n = vec3(cos(lat) * cos(lon), cos(lat) * sin(lon), sin(lat));
  vec3 sphere = u_radius * vec3(dot(n, u_east), -dot(n, u_north), dot(n, u_up) - 1.0);
  return mix(vec3(plane, 0.0), sphere, u_globe);
}
";

/// Uniforms of `PROJECT`
pub struct Projection([Option<glow::UniformLocation>; 5]);

impl Projection {
    pub fn new(gl: &glow::Context, program: glow::Program) -> Self {
        Self(uniforms(
            gl,
            program,
            ["u_globe", "u_radius", "u_east", "u_north", "u_up"],
        ))
    }

    /// Call with the program in use
    pub fn set(&self, gl: &glow::Context, view: &crate::geo::camera::View) {
        let [factor, radius, east, north, up] = &self.0;
        let g = &view.globe;
        // SAFETY: uniform uploads on the program in use
        unsafe {
            gl.uniform_1_f32(factor.as_ref(), g.factor as f32);
            gl.uniform_1_f32(radius.as_ref(), g.radius as f32);
            for (location, axis) in [(east, g.east), (north, g.north), (up, g.up)] {
                gl.uniform_3_f32(location.as_ref(), axis.x as f32, axis.y as f32, axis.z as f32);
            }
        }
    }
}

/// Subdivisions of the tile mesh on the globe
pub const GRID: u16 = 16;

/// Static mesh of a unit square: (GRID + 1)² vertices (location 0, vec2 in [0, 1]); indices of the full grid
/// (`GRID² × 6`, for the globe) followed by the 6 indices of the corners only (for the plane). Clockwise on screen,
/// the front faces (`front_face(CW)`).
pub struct Grid {
    pub vertices: glow::Buffer,
    pub indices: glow::Buffer,
}

impl Grid {
    pub const FULL: i32 = (GRID as i32) * (GRID as i32) * 6;
    /// Byte offset and count of the corner-only quad
    pub const CORNERS: (i32, i32) = (Self::FULL * 2, 6);

    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        let n = GRID + 1;
        let vertices: Vec<f32> = (0..n)
            .flat_map(|j| (0..n).flat_map(move |i| [f32::from(i) / f32::from(GRID), f32::from(j) / f32::from(GRID)]))
            .collect();
        let mut indices: Vec<u16> = Vec::new();
        for j in 0..GRID {
            for i in 0..GRID {
                let a = j * n + i;
                indices.extend_from_slice(&[a, a + 1, a + n, a + 1, a + n + 1, a + n]);
            }
        }
        let (last, row) = (n * n - 1, GRID);
        indices.extend_from_slice(&[0, row, n * GRID, row, last, n * GRID]);
        // SAFETY: buffer creation and uploads on a live context
        unsafe {
            let vertex_buffer = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&vertices), glow::STATIC_DRAW);
            let index_buffer = gl.create_buffer()?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes(&indices), glow::STATIC_DRAW);
            Ok(Self {
                vertices: vertex_buffer,
                indices: index_buffer,
            })
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

    /// Instanced draw of the mesh: full grid on the globe, two triangles on the plane
    pub fn draw(gl: &glow::Context, view: &crate::geo::camera::View, instances: i32) {
        let (offset, count) = if view.globe.factor > 0.0 {
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

pub fn bytes<T: Copy>(data: &[T]) -> &[u8] {
    // SAFETY: plain numeric types without padding, viewed as bytes for an upload
    unsafe { std::slice::from_raw_parts(data.as_ptr().cast(), std::mem::size_of_val(data)) }
}
