//! What is drawn under a screen point: the relief, or a surface standing on it (extrusions, 3D Tiles, models). The
//! solid scene of the last frame is drawn again into a depth texture, as for the viewshed; a pass reads that depth at
//! the pixel and packs it in the color of the picking target, read back. Its position is then on the camera's ray at
//! that depth.

use super::Engine;
use crate::js;
use crate::render::gl::{self, Objects};
use crate::render::raster::unpack_depth;
use geo::camera::{self, Camera};
use glow::HasContext;
use wasm_bindgen::prelude::*;

const VERTEX: &str = r"#version 300 es
void main() {
  // One triangle covering the screen
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)) * 2.0 - 1.0;
  gl_Position = vec4(p, 0.0, 1.0);
}";

/// The depth at the pixel, packed as `DEPTH_FRAGMENT` packs the ground's; alpha 0 where nothing was drawn
const FRAGMENT: &str = r"#version 300 es
precision highp float;
uniform highp sampler2D u_depth;
out vec4 color;
void main() {
  float z = texelFetch(u_depth, ivec2(gl_FragCoord.xy), 0).r;
  if (z >= 1.0) {
    color = vec4(0.0);
    return;
  }
  vec3 e = fract(z * vec3(1.0, 255.0, 65025.0));
  e -= e.yzz * vec3(1.0 / 255.0, 1.0 / 255.0, 0.0);
  color = vec4(e, 1.0);
}";

/// The depth texture at the canvas size and the pass reading it
pub(super) struct Probe {
    program: glow::Program,
    depth_u: Option<glow::UniformLocation>,
    vao: glow::VertexArray,
    target: Option<(glow::Texture, glow::Framebuffer, (i32, i32))>,
}

impl Probe {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        let mut objects = Objects::new(gl);
        let program = objects.program(VERTEX, FRAGMENT)?;
        let vao = objects.vertex_array()?;
        objects.keep();
        let [depth_u] = gl::uniforms(gl, program, ["u_depth"]);
        Ok(Self {
            program,
            depth_u,
            vao,
            target: None,
        })
    }

    pub(super) fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by the probe, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            if let Some((texture, framebuffer, _)) = self.target.take() {
                gl.delete_texture(texture);
                gl.delete_framebuffer(framebuffer);
            }
        }
    }

    /// The depth target and its texture, (re)created at the canvas size
    fn target(&mut self, gl: &glow::Context, size: (i32, i32)) -> Result<(glow::Framebuffer, glow::Texture), String> {
        if let Some((texture, framebuffer, at)) = self.target
            && at == size
        {
            return Ok((framebuffer, texture));
        }
        if let Some((texture, framebuffer, _)) = self.target.take() {
            // SAFETY: the previous target, replaced
            unsafe {
                gl.delete_texture(texture);
                gl.delete_framebuffer(framebuffer);
            }
        }
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
        self.target = Some((texture, framebuffer, size));
        Ok((framebuffer, texture))
    }
}

impl Engine {
    /// Depth (0-1) of the solid scene of the last frame at a device pixel, None where nothing was drawn
    fn surface_depth(&mut self, cursor: (i32, i32), size: (i32, i32)) -> Option<f64> {
        let view = self.drawn.clone()?;
        let mut probe = match self.surface.take() {
            Some(probe) => probe,
            None => Probe::new(&self.gl).ok()?,
        };
        let read = probe.target(&self.gl, size).ok().and_then(|(framebuffer, texture)| {
            let pick = self.pick_target(size).ok()?;
            let gl = &self.gl;
            let mut pixel = [0u8; 4];
            // SAFETY: offscreen passes on the live context, the default framebuffer bound again after
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                gl.viewport(0, 0, size.0, size.1);
                self.draw_solid_depth(&view);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(pick));
                gl.viewport(0, 0, size.0, size.1);
                gl.disable(glow::BLEND);
                gl.disable(glow::CULL_FACE);
                gl.enable(glow::SCISSOR_TEST);
                gl.scissor(cursor.0, cursor.1, 1, 1);
                gl.use_program(Some(probe.program));
                gl.uniform_1_i32(probe.depth_u.as_ref(), 0);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.bind_vertex_array(Some(probe.vao));
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                gl.bind_vertex_array(None);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.read_pixels(
                    cursor.0,
                    cursor.1,
                    1,
                    1,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut pixel)),
                );
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
            gl::reset_state(gl);
            (pixel[3] != 0).then(|| unpack_depth(pixel))
        });
        self.surface = Some(probe);
        read
    }
}

#[wasm_bindgen]
impl Engine {
    /// What is drawn under a screen point (CSS pixels): longitude, latitude and its height above the relief (metres),
    /// on a roof, a wall or a mesh as on the ground; undefined where nothing was drawn. The scene is drawn again and
    /// read back (a stall): for clicks and drags
    #[wasm_bindgen(unchecked_return_type = "[lon: number, lat: number, height: number] | undefined")]
    pub fn surface(&mut self, x: f64, y: f64) -> JsValue {
        let size = self.canvas_size();
        let (Some(cursor), Some(view)) = (self.device_point(x, y, size), self.drawn.clone()) else {
            return JsValue::UNDEFINED;
        };
        let Some(depth) = self.surface_depth(cursor, size) else {
            return JsValue::UNDEFINED;
        };
        let ndc = glam::DVec4::new(
            2.0 * x / self.camera.width - 1.0,
            1.0 - 2.0 * y / self.camera.height,
            2.0 * depth - 1.0,
            1.0,
        );
        let p = view.inverse * ndc;
        let altitude = Camera::altitude_of(&view, p.truncate() / p.w);
        let Some(m) = self.camera.ground_at(&view, x, y, altitude) else {
            return JsValue::UNDEFINED;
        };
        let ground = super::ground_elevation(self.terrain.as_ref(), m).unwrap_or(0.0);
        let at = camera::lon_lat(m);
        js::triple(at.x, at.y, altitude - ground)
    }
}
