//! A viewshed's cones in 3D: the fan of each cone through its axis, at the eye's height and tilted with it (its left
//! and right edges and its axis from the eye to the range, the arc at the range between them). The whole pyramid of a
//! wide vertical field of view would mostly stand in the sky and under the ground; the fan shows the direction, the
//! width, the range, the height and the tilt. Each segment is a band of a few pixels, widened on screen; drawn twice
//! over the frame's depth, solid where nothing hides it and faint behind the scene, so that the cone reads in depth.

use super::Cone;
use crate::render::gl::{self, Objects};
use geo::camera::View;
use glam::DVec3;
use glow::HasContext;

const VERTEX: &str = r"#version 300 es
layout(location = 0) in vec3 a_from;
layout(location = 1) in vec3 a_to;
// Along the segment (0, 1) and across it (-1, 1)
layout(location = 2) in vec2 a_corner;
uniform mat4 u_matrix;
uniform vec2 u_viewport;
uniform float u_width;

void main() {
  vec4 a = u_matrix * vec4(a_from, 1.0);
  vec4 b = u_matrix * vec4(a_to, 1.0);
  // Clipped to just before the camera: an end behind it would turn the band around
  float near = 1e-4;
  if (a.w < near && b.w < near) {
    gl_Position = vec4(0.0, 0.0, 2.0, 1.0);
    return;
  }
  if (a.w < near) a = mix(a, b, (near - a.w) / (b.w - a.w));
  if (b.w < near) b = mix(b, a, (near - b.w) / (a.w - b.w));
  vec2 half_size = 0.5 * u_viewport;
  vec2 along = b.xy / b.w * half_size - a.xy / a.w * half_size;
  vec2 across = length(along) > 1e-6 ? normalize(vec2(-along.y, along.x)) : vec2(0.0, 1.0);
  vec4 p = mix(a, b, a_corner.x);
  p.xy += across * a_corner.y * 0.5 * u_width / half_size * p.w;
  gl_Position = p;
}";

const FRAGMENT: &str = r"#version 300 es
precision highp float;
uniform vec4 u_color;
out vec4 color;
void main() {
  color = u_color;
}";

/// Steps of the arc at the range
const STEPS: usize = 24;
/// Floats per vertex: from, to, corner
const STRIDE: usize = 8;

/// The program and the vertices of the segments of a frame
pub(super) struct Outline {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 4],
    vao: glow::VertexArray,
    buffer: glow::Buffer,
}

impl Outline {
    pub(super) fn new(gl: &glow::Context) -> Result<Self, String> {
        let mut objects = Objects::new(gl);
        let program = objects.program(VERTEX, FRAGMENT)?;
        let vao = objects.vertex_array()?;
        let buffer = objects.buffer()?;

        objects.keep();

        let uniforms = gl::uniforms(gl, program, ["u_matrix", "u_viewport", "u_width", "u_color"]);

        // SAFETY: layout of a vertex array just created on the live context
        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));

            let stride = (STRIDE * 4) as i32;

            for (location, size, offset) in [(0, 3, 0), (1, 3, 12), (2, 2, 24)] {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }

        Ok(Self {
            program,
            uniforms,
            vao,
            buffer,
        })
    }

    pub(super) fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by the outline, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.buffer);
        }
    }

    /// Draws the segments over the bound framebuffer and its depth, in `color` (RGB 0-1), `width` device pixels wide
    pub(super) fn draw(
        &self,
        gl: &glow::Context,
        view: &View,
        segments: &[[DVec3; 2]],
        color: [f32; 3],
        width: f32,
        size: (i32, i32),
    ) {
        if segments.is_empty() {
            return;
        }

        let mut vertices = Vec::with_capacity(segments.len() * 6 * STRIDE);

        for [from, to] in segments {
            for (along, across) in [
                (0.0, -1.0),
                (1.0, -1.0),
                (1.0, 1.0),
                (0.0, -1.0),
                (1.0, 1.0),
                (0.0, 1.0),
            ] {
                vertices.extend([from.x, from.y, from.z, to.x, to.y, to.z].map(|v| v as f32));
                vertices.extend([along, across]);
            }
        }

        let [matrix, viewport, width_u, color_u] = &self.uniforms;
        let [r, g, b] = color;

        // SAFETY: draws with objects owned by the outline; the state set back after
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_2_f32(viewport.as_ref(), size.0 as f32, size.1 as f32);
            gl.uniform_1_f32(width_u.as_ref(), width);
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, gl::bytes(&vertices), glow::STREAM_DRAW);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::BLEND);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_mask(false);
            // Behind the scene, faint; then where nothing hides it, solid
            for (func, alpha) in [(glow::GREATER, 0.3), (glow::LEQUAL, 1.0)] {
                gl.depth_func(func);
                gl.uniform_4_f32(color_u.as_ref(), r, g, b, alpha);
                gl.draw_arrays(glow::TRIANGLES, 0, (segments.len() * 6) as i32);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        gl::reset_state(gl);
    }
}

/// The segments of a cone's fan from `eye`, `range` long (units of the local frame): its left and right edges and its
/// axis, then the arc at the range between the edges
pub(super) fn segments(eye: DVec3, cone: &Cone, range: f64, out: &mut Vec<[DVec3; 2]>) {
    let [right, _, forward] = cone.axes();
    let (tx, _) = cone.tangents();
    let at = |x: f64| eye + (right * x * tx + forward).normalize() * range;

    for x in [-1.0, 0.0, 1.0] {
        out.push([eye, at(x)]);
    }

    for step in 0..STEPS {
        let (s, t) = (step as f64 / STEPS as f64, (step + 1) as f64 / STEPS as f64);

        out.push([at(2.0 * s - 1.0), at(2.0 * t - 1.0)]);
    }
}
