//! What is drawn under a screen point, or at a position: the relief, or a surface standing on it (extrusions, 3D
//! Tiles, models). The solid scene of the last frame is drawn again into a depth texture, as for the viewshed; a pass
//! reads that depth at a pixel and packs it in the color of the picking target, read back. Under a screen point, the
//! position is then on the camera's ray at that depth; at a position, the scene is drawn from above into a single
//! pixel, looking straight down, and the depth there is the top of what stands at that position.

use super::{Engine, viewshed};
use crate::js;
use crate::render::gl::{self, Objects};
use crate::render::raster::unpack_depth;
use geo::camera::{self, Camera, View};
use glam::DVec3;
use glow::HasContext;
use wasm_bindgen::prelude::*;

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
    target: gl::DepthTarget,
}

impl Probe {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        let mut objects = Objects::new(gl);
        let program = objects.program(gl::FULL_SCREEN_VERTEX, FRAGMENT)?;
        let vao = objects.vertex_array()?;
        objects.keep();
        let [depth_u] = gl::uniforms(gl, program, ["u_depth"]);
        Ok(Self {
            program,
            depth_u,
            vao,
            target: gl::DepthTarget::default(),
        })
    }

    pub(super) fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by the probe, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
        }
        self.target.delete(gl);
    }
}

impl Engine {
    /// Depth (0-1) of the solid scene of the last frame seen with `view` at a device pixel of a `viewport` (from the
    /// corner), None where nothing was drawn
    fn surface_depth(&mut self, view: &View, cursor: (i32, i32), viewport: (i32, i32)) -> Option<f64> {
        let size = self.canvas_size();
        let mut probe = match self.surface.take() {
            Some(probe) => probe,
            None => Probe::new(&self.gl).ok()?,
        };
        let read = probe.target.at(&self.gl, size).ok().and_then(|(framebuffer, texture)| {
            let pick = self.pick_target(size).ok()?;
            let gl = &self.gl;
            let mut pixel = [0u8; 4];
            // SAFETY: offscreen passes on the live context, the default framebuffer bound again after
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                gl.viewport(0, 0, viewport.0, viewport.1);
                self.draw_solid_depth(view);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(pick));
                gl.viewport(0, 0, viewport.0, viewport.1);
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
        let Some(depth) = self.surface_depth(&view, cursor, size) else {
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

    /// The height above the relief (metres) of the top of what stands at a position (degrees): a roof, a mesh, 0 on
    /// the bare ground; undefined where nothing of the last frame is there. The scene is drawn from above into a single
    /// pixel and read back (a stall, cheap: one pixel to fill)
    #[wasm_bindgen(js_name = surfaceAt)]
    pub fn surface_at(&mut self, lon: f64, lat: f64) -> JsValue {
        const ABOVE: f64 = 2000.0;
        let Some(view) = self.drawn.clone() else {
            return JsValue::UNDEFINED;
        };
        if !lon.is_finite() || !lat.is_finite() {
            return JsValue::UNDEFINED;
        }
        let m = camera::mercator(lon, lat);
        let ground = super::ground_elevation(self.terrain.as_ref(), m).unwrap_or(0.0);
        let k = view.pixels_per_metre(lat);
        let top = view.geodetic(DVec3::new(lon, lat, ground + ABOVE));
        // The face of the cube looking down
        let down = viewshed::face_view(&view, top, 5, k, (ABOVE + 1000.0) * k);
        let Some(depth) = self.surface_depth(&down, (0, 0), (1, 1)) else {
            return JsValue::UNDEFINED;
        };
        let p = down.inverse * glam::DVec4::new(0.0, 0.0, 2.0 * depth - 1.0, 1.0);
        let altitude = Camera::altitude_of(&view, p.truncate() / p.w);
        JsValue::from_f64(altitude - ground)
    }
}
