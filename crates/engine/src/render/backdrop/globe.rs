//! Globe base: a plain sphere under the tiles, which fills the polar caps that Web Mercator tiles do not cover
//! (beyond 85°) and gives the globe its outline while tiles load. Drawn only in the globe projection, fading out with
//! it. Under relief it lies as deep as the body's lowest ground: at the datum, it would show its colour over a
//! canyon floor or a basin forming the horizon (Valles Marineris seen along its length).

use crate::render::gl::{self, Grid};
use geo::camera::View;
use glow::HasContext;

/// Patches of the sphere: longitude × latitude, each one a `Grid` mesh
const LON: usize = 8;
const LAT: usize = 4;

const VERTEX: &str = r"#version 300 es
uniform mat4 u_matrix;
uniform float u_radius;
// How far under the datum the base lies (pixels)
uniform float u_depth;
uniform vec3 u_east;
uniform vec3 u_north;
uniform vec3 u_up;
layout(location = 0) in vec2 a_grid;
// Longitude min, latitude max, longitude max, latitude min (radians)
layout(location = 1) in vec4 a_patch;
void main() {
  float lon = mix(a_patch.x, a_patch.z, a_grid.x);
  float lat = mix(a_patch.y, a_patch.w, a_grid.y);
  vec3 n = vec3(cos(lat) * cos(lon), cos(lat) * sin(lon), sin(lat));
  vec3 p = (u_radius - u_depth) * vec3(dot(n, u_east), -dot(n, u_north), dot(n, u_up)) - vec3(0.0, 0.0, u_radius);
  gl_Position = u_matrix * vec4(p, 1.0);
}";

const FRAGMENT: &str = r"#version 300 es
precision highp float;
uniform vec4 u_color;
out vec4 color;
void main() { color = u_color; }";

/// How far under the datum the body's lowest ground lies (pixels), under relief of this exaggeration (None: no
/// relief, 0)
pub fn depth(view: &View, exaggeration: Option<f32>) -> f64 {
    exaggeration.map_or(0.0, |e| {
        -view.body.relief.0.min(0.0) * f64::from(e.max(0.0)) * view.vertical
    })
}

/// The globe base: `LON × LAT` patches of the shared `Grid` mesh, drawn instanced as one sphere of a plain color
pub struct GlobeBase {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 7],
    vao: glow::VertexArray,
    patches: glow::Buffer,
    /// Its color, RGB 0-1 (the alpha follows the globe factor, fading with it)
    pub color: [f32; 3],
}

impl GlobeBase {
    /// Builds the program and the patch buffer; the mesh is the raster pass's `Grid`
    pub fn new(gl: &glow::Context, grid: &Grid) -> Result<Self, String> {
        // Everything created is deleted again if a step fails
        let mut objects = gl::Objects::new(gl);
        let program = objects.program(VERTEX, FRAGMENT)?;
        let uniforms = gl::uniforms(
            gl,
            program,
            [
                "u_matrix", "u_radius", "u_depth", "u_east", "u_north", "u_up", "u_color",
            ],
        );
        let pi = std::f32::consts::PI;
        let mut patches = Vec::with_capacity(LON * LAT * 4);

        for j in 0..LAT {
            for i in 0..LON {
                let lon = |i: usize| -pi + 2.0 * pi * i as f32 / LON as f32;
                let lat = |j: usize| pi / 2.0 - pi * j as f32 / LAT as f32;

                patches.extend_from_slice(&[lon(i), lat(j), lon(i + 1), lat(j + 1)]);
            }
        }

        let vao = objects.vertex_array()?;
        let buffer = objects.buffer()?;

        objects.keep();

        // SAFETY: state setup of objects just created on a live context
        unsafe {
            gl.bind_vertex_array(Some(vao));
            grid.bind(gl);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, gl::bytes(&patches), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, 16, 0);
            gl.vertex_attrib_divisor(1, 1);
            gl.bind_vertex_array(None);

            Ok(Self {
                program,
                uniforms,
                vao,
                patches: buffer,
                color: [0.80, 0.86, 0.90],
            })
        }
    }

    /// Deletes the program, vertex array and patch buffer (not the shared grid)
    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by the base, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.patches);
        }
    }

    /// `depth`: how far under the datum it lies (pixels, `depth`)
    pub fn draw(&self, gl: &glow::Context, view: &View, depth: f64) {
        let g = &view.globe;

        if g.factor <= 0.0 {
            return;
        }

        let [matrix, radius, depth_at, east, north, up, color] = &self.uniforms;

        // SAFETY: draw with objects owned by this base
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_1_f32(radius.as_ref(), g.radius as f32);
            gl.uniform_1_f32(depth_at.as_ref(), depth as f32);
            for (location, axis) in [(east, g.east), (north, g.north), (up, g.up)] {
                gl.uniform_3_f32(location.as_ref(), axis.x as f32, axis.y as f32, axis.z as f32);
            }

            let [r, gr, b] = self.color;

            gl.uniform_4_f32(color.as_ref(), r, gr, b, g.factor as f32);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_elements_instanced(glow::TRIANGLES, Grid::FULL, glow::UNSIGNED_SHORT, 0, (LON * LAT) as i32);
            gl.bind_vertex_array(None);
        }
    }
}
