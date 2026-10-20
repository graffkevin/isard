//! Viewshed: what can be seen from a point. The solid scene (the relief, extrusions, 3D Tiles, models) is drawn from
//! the observer into the six faces of a depth cube, as for the shadows of a point light; then once more from the
//! camera into a depth texture. A pass over the screen puts each pixel back in the local frame from that depth and
//! compares its distance to the observer with the cube's: nearer or as near, it is seen; farther, something hides
//! it. Within the range, seen pixels are tinted with one color, hidden ones with another.
//!
//! Several observers share the cube in turn: each one's comparison goes into an accumulation target (how much of a
//! pixel lies within one's reach, how much is seen by one), and one last pass tints the screen from it. A pixel seen
//! by any observer is seen; hidden from all that reach it, it is hidden.
//!
//! Looking one way only (a camera, a lookout), the viewshed is a cone: a heading, a tilt and two angles of view; what
//! lies outside it is left untinted (the whole cube is still drawn).
//!
//! The scene drawn from the observer is the one the frame selected: tiles out of the camera's view are not there to
//! hide anything.

mod outline;

use super::{Engine, Slot};
use crate::render::gl::{self, Objects};
use geo::camera::View;
use glam::{DVec2, DVec3};
use glow::HasContext;
use wasm_bindgen::prelude::*;

/// Pixels of a face of the cube
const CUBE: i32 = 1024;

const VERTEX: &str = r"#version 300 es
out vec2 v_ndc;
void main() {
  // One triangle covering the screen
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)) * 2.0 - 1.0;
  v_ndc = p;
  gl_Position = vec4(p, 0.0, 1.0);
}";

/// Observers at most
const OBSERVERS: usize = 16;

/// The comparing pass, after the cube's size
fn fragment() -> String {
    format!("#version 300 es\nprecision highp float;\nconst float CUBE = {CUBE:?}.0;\n{FRAGMENT}")
}

const FRAGMENT: &str = r"precision highp float;
precision highp sampler2D;
precision highp samplerCubeShadow;
uniform sampler2D u_depth;
uniform samplerCubeShadow u_cube;
uniform mat4 u_inverse;
uniform vec3 u_observer;
// Range, the cube's near and far planes (pixels of the local frame); pixels per metre at the observer
uniform vec4 u_range;
// The ground taken as the plane z = 0 where nothing was drawn (no relief)
uniform float u_flat;
// The cone: its axes (right, up, forward), the tangents of its half angles across and up, 1 if there is one
uniform mat3 u_axes;
uniform vec3 u_cone;
in vec2 v_ndc;
out vec4 color;

vec3 unproject(float z) {
  vec4 p = u_inverse * vec4(v_ndc, z, 1.0);
  return p.xyz / p.w;
}

void main() {
  float depth = texture(u_depth, v_ndc * 0.5 + 0.5).r;
  vec3 p;
  if (depth < 1.0) {
    p = unproject(depth * 2.0 - 1.0);
  } else {
    vec3 a = unproject(-1.0);
    vec3 b = unproject(1.0);
    if (u_flat < 0.5 || a.z <= b.z || a.z * b.z > 0.0) discard;
    p = mix(a, b, a.z / (a.z - b.z));
  }
  vec3 v = p - u_observer;
  float distance = length(v);
  float range = u_range.x;
  if (distance > range) discard;
  if (u_cone.z > 0.5) {
    vec3 c = v * u_axes;
    if (c.z <= 0.0 || abs(c.x) > c.z * u_cone.x || abs(c.y) > c.z * u_cone.y) discard;
  }
  // Depth the cube holds along v, against this point's: its major axis, less a margin. A texel of the cube covers
  // more of a surface seen at a grazing angle (the far ground from an eye a few metres up): the margin grows with
  // the slope, from the surface's normal on screen
  vec3 normal = normalize(cross(dFdx(p), dFdy(p)));
  float facing = abs(dot(normal, v / max(distance, 1e-6)));
  float texel = distance * 2.0 / CUBE;
  float slope = min(sqrt(max(1.0 - facing * facing, 0.0)) / max(facing, 0.02), 24.0);
  vec3 a = abs(v);
  float n = u_range.y;
  float f = u_range.z;
  float major = max(max(a.x, a.y), a.z);
  major = max(major - (0.5 * u_range.w + texel * (1.5 + slope) + 0.002 * distance), n);
  float reference = 0.5 * ((f + n) / (f - n) - 2.0 * f * n / ((f - n) * major)) + 0.5;
  float seen = texture(u_cube, vec4(v, reference));
  // Fading out at the edge of the range; blended by maximum with the other observers'
  float edge = 1.0 - smoothstep(0.92 * range, range, distance);
  color = vec4(seen * edge, edge, 0.0, 0.0);
}";

/// The tint over the screen, from the accumulation (its red channel: seen by some observer; green: within the reach
/// of one). Multiplied into the frame: the scene keeps its textures and its shading under the colors, brightened to
/// their strongest channel so that they color rather than darken
const TINT: &str = r"#version 300 es
precision highp float;
uniform sampler2D u_accumulation;
uniform vec3 u_visible;
uniform vec3 u_hidden;
uniform float u_opacity;
in vec2 v_ndc;
out vec4 color;

vec3 bright(vec3 c) {
  return c / max(max(c.r, c.g), max(c.b, 1e-3));
}

void main() {
  vec2 a = texture(u_accumulation, v_ndc * 0.5 + 0.5).rg;
  if (a.g <= 0.0) discard;
  vec3 tint = mix(bright(u_hidden), bright(u_visible), clamp(a.r / a.g, 0.0, 1.0));
  color = vec4(mix(vec3(1.0), tint, u_opacity * a.g), 1.0);
}";

/// What the page set: the observers and the look of the result
#[derive(Clone, Debug)]
pub(super) struct Settings {
    observers: Vec<Observer>,
    visible: [f32; 3],
    hidden: [f32; 3],
    opacity: f32,
    /// The color of the cones' edges drawn in 3D, if they are
    outline: Option<[f32; 3]>,
}

/// A point the view is taken from
#[derive(Clone, Copy, Debug)]
struct Observer {
    /// Longitude, latitude (degrees)
    position: DVec2,
    /// Metres above the ground
    height: f64,
    /// Metres around the observer
    range: f64,
    cone: Option<Cone>,
}

/// One way only: heading (degrees clockwise from north), tilt (degrees above the horizon), angles of view across and
/// up (degrees)
#[derive(Clone, Copy, Debug)]
struct Cone {
    heading: f64,
    tilt: f64,
    across: f64,
    up: f64,
}

impl Cone {
    /// Its axes in the local frame (x east, y south, z up): right, up, forward
    fn axes(&self) -> [DVec3; 3] {
        let (b, t) = (self.heading.to_radians(), self.tilt.to_radians());
        let forward = DVec3::new(b.sin() * t.cos(), -b.cos() * t.cos(), t.sin());
        let right = DVec3::new(b.cos(), b.sin(), 0.0);
        [right, forward.cross(right), forward]
    }

    /// Tangents of its half angles, across and up
    fn tangents(&self) -> (f64, f64) {
        (
            (self.across / 2.0).to_radians().tan(),
            (self.up / 2.0).to_radians().tan(),
        )
    }
}

/// The screen-sized targets: the scene's depth from the camera, the observers' accumulation
#[derive(Clone, Copy)]
struct Screen {
    depth: glow::Texture,
    depth_target: glow::Framebuffer,
    accumulation: glow::Texture,
    accumulation_target: glow::Framebuffer,
    size: (i32, i32),
}

/// The GPU side: the cube and the screen's targets, the pass that compares them and the one that tints
pub(super) struct Viewshed {
    settings: Settings,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 8],
    tint: glow::Program,
    tint_uniforms: [Option<glow::UniformLocation>; 4],
    vao: glow::VertexArray,
    cube: glow::Texture,
    cube_target: glow::Framebuffer,
    screen: Option<Screen>,
    outline: outline::Outline,
}

impl Viewshed {
    fn new(gl: &glow::Context, settings: Settings) -> Result<Self, String> {
        let mut objects = Objects::new(gl);
        let program = objects.program(VERTEX, &fragment())?;
        let tint = objects.program(VERTEX, TINT)?;
        let vao = objects.vertex_array()?;
        let cube = objects.texture()?;
        let cube_target = objects.framebuffer()?;
        // On a failure, `objects` deletes the others
        let outline = outline::Outline::new(gl)?;
        objects.keep();
        let uniforms = gl::uniforms(
            gl,
            program,
            [
                "u_inverse",
                "u_observer",
                "u_range",
                "u_flat",
                "u_depth",
                "u_cube",
                "u_axes",
                "u_cone",
            ],
        );
        let tint_uniforms = gl::uniforms(gl, tint, ["u_accumulation", "u_visible", "u_hidden", "u_opacity"]);
        // SAFETY: storage of a texture just created on the live context
        unsafe {
            gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(cube));
            gl.tex_storage_2d(glow::TEXTURE_CUBE_MAP, 1, glow::DEPTH24_STENCIL8, CUBE, CUBE);
            gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(
                glow::TEXTURE_CUBE_MAP,
                glow::TEXTURE_COMPARE_MODE,
                glow::COMPARE_REF_TO_TEXTURE as i32,
            );
            gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, glow::TEXTURE_COMPARE_FUNC, glow::LEQUAL as i32);
            gl.bind_texture(glow::TEXTURE_CUBE_MAP, None);
        }
        Ok(Self {
            settings,
            program,
            uniforms,
            tint,
            tint_uniforms,
            vao,
            cube,
            cube_target,
            screen: None,
            outline,
        })
    }

    pub(super) fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by the viewshed, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_program(self.tint);
            gl.delete_vertex_array(self.vao);
            gl.delete_texture(self.cube);
            gl.delete_framebuffer(self.cube_target);
        }
        self.outline.delete(gl);
        self.delete_screen(gl);
    }

    fn delete_screen(&mut self, gl: &glow::Context) {
        if let Some(screen) = self.screen.take() {
            // SAFETY: the screen's targets, not used afterwards
            unsafe {
                gl.delete_texture(screen.depth);
                gl.delete_framebuffer(screen.depth_target);
                gl.delete_texture(screen.accumulation);
                gl.delete_framebuffer(screen.accumulation_target);
            }
        }
    }

    /// The screen's targets, (re)created at the canvas size
    fn screen_target(&mut self, gl: &glow::Context, size: (i32, i32)) -> Result<Screen, String> {
        if let Some(screen) = self.screen
            && screen.size == size
        {
            return Ok(screen);
        }
        self.delete_screen(gl);
        let mut objects = Objects::new(gl);
        let depth = objects.texture()?;
        let depth_target = objects.framebuffer()?;
        let accumulation = objects.texture()?;
        let accumulation_target = objects.framebuffer()?;
        objects.keep();
        // SAFETY: storage and attachment of objects just created on the live context
        unsafe {
            for (texture, target, format, attachment) in [
                (
                    depth,
                    depth_target,
                    glow::DEPTH24_STENCIL8,
                    glow::DEPTH_STENCIL_ATTACHMENT,
                ),
                (accumulation, accumulation_target, glow::RGBA8, glow::COLOR_ATTACHMENT0),
            ] {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_storage_2d(glow::TEXTURE_2D, 1, format, size.0, size.1);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, attachment, glow::TEXTURE_2D, Some(texture), 0);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        let screen = Screen {
            depth,
            depth_target,
            accumulation,
            accumulation_target,
            size,
        };
        self.screen = Some(screen);
        Ok(screen)
    }
}

/// The view from `eye` along one face of a cube (the faces in the order of `TEXTURE_CUBE_MAP_POSITIVE_X` + i, their
/// up vectors those of the cube map's layout), sharing the local frame of `view`
pub(super) fn face_view(view: &View, eye: DVec3, face: u32, near: f64, far: f64) -> View {
    let (look, up) = face_axes(face);
    let projection = glam::dcamera::rh::proj::opengl::perspective(std::f64::consts::FRAC_PI_2, 1.0, near, far);
    let mut face_view = view.clone();
    face_view.view_proj = projection * glam::dcamera::rh::view::look_to_mat4(eye, look, up);
    face_view.inverse = face_view.view_proj.inverse();
    face_view.eye = eye;
    face_view.near = near;
    face_view.far = far;
    face_view
}

/// The axis a face of the cube looks along and its up vector
fn face_axes(face: u32) -> (DVec3, DVec3) {
    match face {
        0 => (DVec3::X, DVec3::NEG_Y),
        1 => (DVec3::NEG_X, DVec3::NEG_Y),
        2 => (DVec3::Y, DVec3::Z),
        3 => (DVec3::NEG_Y, DVec3::NEG_Z),
        4 => (DVec3::Z, DVec3::NEG_Y),
        _ => (DVec3::NEG_Z, DVec3::NEG_Y),
    }
}

impl Engine {
    /// The solid scene's depth into the bound target, seen with `view`. Call with the target bound and the viewport
    /// set; the state is set back but for the framebuffer
    pub(super) unsafe fn draw_solid_depth(&self, view: &View) {
        let gl = &self.gl;
        gl::reset_state(gl);
        // SAFETY: offscreen pass on the live context, the state set back after
        unsafe {
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.color_mask(false, false, false, false);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            if let Some((terrain, _)) = &self.terrain {
                self.raster_pass.draw_depth(gl, view, terrain);
            }
            let frame = self.frame_state(view);
            for (_, slot) in &self.layers {
                if let Slot::Layer(layer) = slot {
                    gl.enable(glow::DEPTH_TEST);
                    gl.disable(glow::CULL_FACE);
                    layer.draw_depth(&frame);
                }
            }
        }
        gl::reset_state(gl);
    }

    /// Draws the viewshed over the frame, if one is set: the depth from the camera, then for each observer the cube
    /// and its comparison, then the tint
    pub(super) fn draw_viewshed(&mut self, view: &View) {
        let Some(mut viewshed) = self.viewshed.take() else {
            return;
        };
        let size = (
            (self.camera.width * self.pixel_ratio).round() as i32,
            (self.camera.height * self.pixel_ratio).round() as i32,
        );
        if !viewshed.settings.observers.is_empty()
            && let Ok(screen) = viewshed.screen_target(&self.gl, size)
        {
            self.draw_viewshed_passes(&viewshed, view, screen);
        }
        self.viewshed = Some(viewshed);
    }

    fn draw_viewshed_passes(&self, viewshed: &Viewshed, view: &View, screen: Screen) {
        let gl = &self.gl;
        let size = screen.size;
        // SAFETY: offscreen passes on the live context, the accumulation cleared for the observers
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(screen.depth_target));
            gl.viewport(0, 0, size.0, size.1);
            self.draw_solid_depth(view);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(screen.accumulation_target));
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        for observer in &viewshed.settings.observers {
            self.draw_observer(viewshed, view, observer, screen);
        }
        let s = &viewshed.settings;
        let [accumulation, visible, hidden, opacity] = &viewshed.tint_uniforms;
        // SAFETY: the tint over the default framebuffer, the state set back after
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, size.0, size.1);
            gl.use_program(Some(viewshed.tint));
            gl.uniform_1_i32(accumulation.as_ref(), 0);
            gl.uniform_3_f32_slice(visible.as_ref(), &s.visible);
            gl.uniform_3_f32_slice(hidden.as_ref(), &s.hidden);
            gl.uniform_1_f32(opacity.as_ref(), s.opacity);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(screen.accumulation));
            gl.bind_vertex_array(Some(viewshed.vao));
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(glow::DST_COLOR, glow::ZERO, glow::ZERO, glow::ONE);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl::reset_state(gl);
        if let Some(color) = s.outline {
            let mut segments = Vec::new();
            for observer in &s.observers {
                if let Some(cone) = &observer.cone {
                    let (eye, k) = self.eye(view, observer);
                    outline::segments(eye, cone, observer.range * k, &mut segments);
                }
            }
            viewshed
                .outline
                .draw(gl, view, &segments, color, (2.5 * self.pixel_ratio) as f32, size);
        }
    }

    /// An observer's eye in the local frame of `view`, and the pixels of that frame per metre there
    fn eye(&self, view: &View, s: &Observer) -> (DVec3, f64) {
        let m = geo::camera::mercator(s.position.x, s.position.y);
        let ground = super::ground_elevation(self.terrain.as_ref(), m).unwrap_or(0.0);
        let eye = view.geodetic(DVec3::new(s.position.x, s.position.y, ground + s.height));
        (eye, view.pixels_per_metre(s.position.y))
    }

    /// One observer: its cube, then its comparison into the accumulation, by maximum with the other observers'
    fn draw_observer(&self, viewshed: &Viewshed, view: &View, s: &Observer, screen: Screen) {
        let (eye, k) = self.eye(view, s);
        let range = s.range * k;
        let near = (0.2 * k).max(range / 20_000.0);
        let far = range * 1.05;
        let gl = &self.gl;
        // SAFETY: offscreen passes on the live context, the state set back after
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(viewshed.cube_target));
            gl.viewport(0, 0, CUBE, CUBE);
            for face in 0..6 {
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::DEPTH_STENCIL_ATTACHMENT,
                    glow::TEXTURE_CUBE_MAP_POSITIVE_X + face,
                    Some(viewshed.cube),
                    0,
                );
                self.draw_solid_depth(&face_view(view, eye, face, near, far));
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(screen.accumulation_target));
            gl.viewport(0, 0, screen.size.0, screen.size.1);

            let [inverse, observer, range_u, flat, depth, cube, axes_u, cone_u] = &viewshed.uniforms;
            gl.use_program(Some(viewshed.program));
            gl.uniform_matrix_4_f32_slice(inverse.as_ref(), false, &gl::mat4(&view.inverse));
            gl.uniform_3_f32(observer.as_ref(), eye.x as f32, eye.y as f32, eye.z as f32);
            gl.uniform_4_f32(range_u.as_ref(), range as f32, near as f32, far as f32, k as f32);
            let flat_ground = self.terrain.is_none() && view.globe.factor <= 0.0;
            gl.uniform_1_f32(flat.as_ref(), if flat_ground { 1.0 } else { 0.0 });
            gl.uniform_1_i32(depth.as_ref(), 0);
            gl.uniform_1_i32(cube.as_ref(), 1);
            let (axes, cone) = s.cone.map_or(([DVec3::ZERO; 3], [0.0, 0.0, 0.0]), |cone| {
                let (x, y) = cone.tangents();
                (cone.axes(), [x as f32, y as f32, 1.0])
            });
            let columns = axes.map(|a| [a.x as f32, a.y as f32, a.z as f32]).concat();
            gl.uniform_matrix_3_f32_slice(axes_u.as_ref(), false, &columns);
            gl.uniform_3_f32_slice(cone_u.as_ref(), &cone);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(screen.depth));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(viewshed.cube));
            gl.bind_vertex_array(Some(viewshed.vao));
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::MAX);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.blend_equation(glow::FUNC_ADD);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_CUBE_MAP, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl::reset_state(gl);
    }
}

#[wasm_bindgen]
impl Engine {
    /// Shows what can be seen from points, each within its range: `observers` holds 8 numbers per observer
    /// (16 at most): longitude, latitude (degrees), metres above the ground, range (metres), then to look one way
    /// only heading, tilt, angles of view across and up (degrees), the heading NaN to look all around. What some
    /// observer sees is tinted with `visible`, what is hidden from all those that reach it with `hidden` (RGB 0-1),
    /// at `opacity`; `outline` (RGB 0-1, or empty) the color of the cones' edges drawn in 3D
    #[wasm_bindgen(js_name = setViewshed)]
    pub fn set_viewshed(
        &mut self,
        observers: &[f64],
        visible: &[f32],
        hidden: &[f32],
        opacity: f32,
        outline: &[f32],
    ) -> Result<(), JsValue> {
        let rgb = |c: &[f32]| [0, 1, 2].map(|i| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0));
        let finite = |v: f64, default: f64| if v.is_finite() { v } else { default };
        let observers = observers
            .as_chunks::<8>()
            .0
            .iter()
            .take(OBSERVERS)
            .map(|o| Observer {
                position: DVec2::new(finite(o[0], 0.0), finite(o[1], 0.0).clamp(-85.0, 85.0)),
                height: finite(o[2], 1.7).clamp(0.0, 10_000.0),
                range: finite(o[3], 1000.0).clamp(1.0, 100_000.0),
                cone: o[4].is_finite().then(|| Cone {
                    heading: o[4],
                    tilt: finite(o[5], 0.0).clamp(-89.0, 89.0),
                    across: finite(o[6], 60.0).clamp(1.0, 179.0),
                    up: finite(o[7], 40.0).clamp(1.0, 179.0),
                }),
            })
            .collect();
        let settings = Settings {
            observers,
            visible: rgb(visible),
            hidden: rgb(hidden),
            opacity: if opacity.is_finite() {
                opacity.clamp(0.0, 1.0)
            } else {
                0.5
            },
            outline: (outline.len() >= 3).then(|| rgb(outline)),
        };
        match &mut self.viewshed {
            Some(viewshed) => viewshed.settings = settings,
            None => self.viewshed = Some(Viewshed::new(&self.gl, settings).map_err(|e| JsValue::from_str(&e))?),
        }
        (self.redraw)();
        Ok(())
    }

    /// Removes the viewshed
    #[wasm_bindgen(js_name = clearViewshed)]
    pub fn clear_viewshed(&mut self) {
        if let Some(mut viewshed) = self.viewshed.take() {
            viewshed.delete(&self.gl);
            (self.redraw)();
        }
    }
}
