//! What the layers drawn over the ground share: their coordinates relative to the camera (exact at any zoom), the
//! strip a piece of line is drawn as, the squares and boxes in view, the depth bias in relief.
//!
//! Precision: the layers keep their positions in f32 as Mercator offsets from an origin of their own (the center of
//! their extent). On the GPU, the camera's offset from that origin comes in two parts (hi + lo, as the fills' camera
//! center): the difference with a vertex is taken first, exact near the camera, and only then scaled to pixels. A
//! layer as wide as France stays still to the pixel at zoom 22, where `origin + offset × world` in f32 jittered by
//! several pixels.

use crate::layer::Frame;
use crate::render::drape::{self, Drape, DrapeUniforms};
use crate::render::gl::{self, Objects, Projection};
use crate::render::raster::RasterSource;
use geo::camera::View;
use geo::pieces::{Grid, Look, Visible};
use glam::DVec2;
use glow::HasContext;

/// GLSL after `PROJECT`: the uniforms of `Anchor` and `plane_of(offset)`, the position on the plane (pixels from the
/// camera center) of a Mercator offset from the layer's origin
pub const ANCHOR: &str = r"
uniform mat4 u_matrix;
// The layer's origin (Mercator, f32: for the altitudes and the sphere, which need no more)
uniform vec2 u_origin;
// The camera center's offset from the origin (Mercator, on the copy of the world nearest to it), in two parts
uniform vec2 u_camera_hi;
uniform vec2 u_camera_lo;
// Half the viewport, device pixels
uniform vec2 u_viewport;
// Pulls what is drawn towards the camera in relief (clip space), so that it wins over the ground it lies on
uniform float u_depth_bias;
// The eye from the sphere's center (local frame) and the horizon's bound, as `View::faces_camera`; w < 0 off the globe
uniform vec4 u_horizon;

vec2 plane_of(vec2 offset) { return ((offset - u_camera_hi) - u_camera_lo) * u_world; }
// A ground position (`project`) past the horizon of the globe, hidden by the body
bool behind_globe(vec3 ground) { return dot(ground - vec3(0.0, 0.0, -u_radius), u_horizon.xyz) <= u_horizon.w; }
";

/// GLSL after `ANCHOR` and `drape::ALTITUDE`: a piece of line (`geo::pieces::Piece`, attributes 0 and 1) drawn as a
/// strip of `u_segments` parts facing the screen, `strip_vertex` placing vertex `gl_VertexID` of it
pub const LINE: &str = r"
uniform int u_segments;
// From, to (Mercator offsets); fractions of the edge's length at both ends
layout(location = 0) in vec4 a_piece;
layout(location = 1) in vec2 a_fractions;

// Clip position of the point at `t` along the piece, standing on the ground in relief
vec4 piece_clip(float t) {
  vec2 offset = mix(a_piece.xy, a_piece.zw, t);
  vec2 merc = u_origin + offset;
  vec4 c = u_matrix * vec4(project_up(plane_of(offset), merc, altitude(merc)), 1.0);
  c.z -= u_depth_bias;
  return c;
}

void hide() { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); }

// A vertex of the strip: where along the piece (0 to 1), which side (-1, 1), which end (-1 start, 1 end, else 0), and
// the half width (device pixels, one more for the antialiasing)
struct StripVertex {
  float t;
  float side;
  float cap;
  float half_width;
};

// Places vertex `gl_VertexID` of the strip of a piece `width` device pixels wide, both ends pushed out by
// `cap_length` half widths; false if it is behind the camera (the caller hides it)
bool strip_vertex(float width, float cap_length, out StripVertex v) {
  int k = gl_VertexID >> 1;
  v.side = float(gl_VertexID & 1) * 2.0 - 1.0;
  float step = 1.0 / float(u_segments);
  v.t = float(k) * step;
  v.cap = k == 0 ? -1.0 : k == u_segments ? 1.0 : 0.0;
  v.half_width = width * 0.5 + 1.0;
  vec4 c = piece_clip(v.t);
  vec4 prev = k > 0 ? piece_clip(v.t - step) : c;
  vec4 next = k < u_segments ? piece_clip(v.t + step) : c;
  if (c.w <= 0.0 || prev.w <= 0.0 || next.w <= 0.0) return false;
  vec2 s = c.xy / c.w * u_viewport;
  vec2 dir = next.xy / next.w * u_viewport - prev.xy / prev.w * u_viewport;
  dir = length(dir) > 1e-6 ? normalize(dir) : vec2(1.0, 0.0);
  vec2 normal = vec2(-dir.y, dir.x);
  s += normal * v.side * v.half_width + dir * v.cap * v.half_width * cap_length;
  gl_Position = vec4(s / u_viewport * c.w, c.z, c.w);
  return true;
}
";

/// Lines in relief are pulled towards the camera by this fraction of their distance: the ground under them is
/// interpolated between the vertices of its mesh and can rise a little above the line
pub const LINE_DEPTH_BIAS: f64 = 0.002;
/// Parts of each piece in relief: a straight piece would cut through the bumps of the ground between its ends
pub const RELIEF_SEGMENTS: i32 = 8;
/// Bytes per piece (`geo::pieces::Piece`)
pub const PIECE_STRIDE: i32 = 28;
/// The attributes of a piece (`LINE`, the edge at location 2): from and to, fractions, edge
pub const PIECE_ATTRIBUTES: [gl::Attribute; 3] = [(0, 4, true, 0), (1, 2, true, 16), (2, 1, false, 24)];
const _: () = assert!(PIECE_STRIDE as usize == size_of::<geo::pieces::Piece>());

/// A program of a layer drawn over the ground (lines, circles, networks, symbols), its vertex shader made of
/// `PROJECT`, `drape::ALTITUDE`, `ANCHOR` then `vertex`, and the uniforms they share
pub struct Overlay {
    pub program: glow::Program,
    anchor: Anchor,
    projection: Projection,
    drape: DrapeUniforms,
}

impl Overlay {
    /// The program of `vertex` (with `gl::common` before the parts above) and `fragment` (`gl::fragment_shader`)
    pub fn new(objects: &mut Objects, gl: &glow::Context, vertex: &[&str], fragment: &[&str]) -> Result<Self, String> {
        let mut source = gl::vertex_shader(&[gl::PROJECT, drape::ALTITUDE, ANCHOR]);

        for part in vertex {
            source.push_str(part);
        }

        let program = objects.program(&source, &gl::fragment_shader(fragment))?;

        Ok(Self {
            program,
            anchor: Anchor::new(gl, program),
            projection: Projection::new(gl, program),
            drape: DrapeUniforms::new(gl, program),
        })
    }

    /// Puts the program in use and sets what they share: the view (with `depth_bias` in relief), the origin of the
    /// offsets drawn, the projection and the relief under them (`drape`)
    pub fn begin(&self, frame: &Frame, origin: DVec2, depth_bias: f64, drape: &Drape) {
        let &Frame {
            gl,
            camera,
            view,
            pixel_ratio,
            relief,
            ..
        } = frame;

        // SAFETY: a program of this layer, on the live context
        unsafe { gl.use_program(Some(self.program)) };

        let viewport = (camera.width * pixel_ratio, camera.height * pixel_ratio);

        self.anchor.set_view(gl, view, viewport, relief.is_some(), depth_bias);
        self.anchor.set_origin(gl, view, origin);
        self.projection.set(gl, view);
        drape.bind(gl, &self.drape, relief);
    }
}

/// Mercator points of interleaved coordinates
#[must_use]
pub fn points(coordinates: &[f64]) -> Vec<DVec2> {
    coordinates
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[x, y]| DVec2::new(x, y))
        .collect()
}

/// Bounds of the points that are numbers, None for none
#[must_use]
pub fn bounds(points: &[DVec2]) -> Option<(DVec2, DVec2)> {
    let (min, max) = points
        .iter()
        .filter(|p| p.is_finite())
        .fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), &p| {
            (a.min(p), b.max(p))
        });

    min.is_finite().then_some((min, max))
}

/// Whole worlds to add to x so that a layer at `origin` is drawn on the copy of the world nearest to the center
#[must_use]
pub fn shift(origin: DVec2, view: &View) -> f64 {
    -(origin.x - view.center.x).round()
}

/// Lowest and highest altitudes (pixels of the local frame) a layer may stand at: 0 on flat ground, the range of the
/// body's relief in relief
#[must_use]
pub fn heights(view: &View, relief: Option<(&RasterSource, f32)>) -> (f64, f64) {
    relief.map_or((0.0, 0.0), |(_, e)| {
        let k = view.vertical * f64::from(e);
        let (low, high) = view.body.relief;

        (low * k, high * k)
    })
}

/// How a layer at `origin` looks for its squares in view (`geo::pieces::Look`): the heights it may stand at, the copy
/// of the world it is drawn on, `margin` (Mercator) around what its squares hold, squares past the far plane left out
#[must_use]
pub fn look(origin: DVec2, view: &View, relief: Option<(&RasterSource, f32)>, margin: f64) -> Look {
    (heights(view, relief), shift(origin, view), margin, true)
}

/// The ranges of items in view for the picking pass: those of the last frame if it had the same view, else worked out
/// anew (`visible` is the layer's, updated by its frames only)
#[must_use]
pub fn pick_ranges<'a>(
    visible: &'a Visible,
    grid: &Grid,
    start: &[u32],
    view: &View,
    look: Look,
) -> std::borrow::Cow<'a, [(u32, u32)]> {
    visible.cached(view, look).map_or_else(
        || {
            let mut fresh = Visible::default();

            std::borrow::Cow::Owned(fresh.update(grid, start, view, look).to_vec())
        },
        std::borrow::Cow::Borrowed,
    )
}

/// An f64 as two f32 whose sum is it to about 1e-14
#[must_use]
pub fn split(x: f64) -> (f32, f32) {
    let hi = x as f32;

    (hi, (x - f64::from(hi)) as f32)
}

/// Uniforms of `ANCHOR`
pub struct Anchor([Option<glow::UniformLocation>; 7]);

impl Anchor {
    pub fn new(gl: &glow::Context, program: glow::Program) -> Self {
        Self(gl::uniforms(
            gl,
            program,
            [
                "u_matrix",
                "u_origin",
                "u_camera_hi",
                "u_camera_lo",
                "u_viewport",
                "u_depth_bias",
                "u_horizon",
            ],
        ))
    }

    /// The view: its matrix, half the viewport (device pixels), the horizon of the globe and, in relief, the depth
    /// bias (a fraction of the distance: 2 × near × fraction in clip space). Program in use
    pub fn set_view(&self, gl: &glow::Context, view: &View, viewport: (f64, f64), relief: bool, depth_bias: f64) {
        let [matrix, _, _, _, viewport_u, bias_u, horizon_u] = &self.0;
        let bias = if relief { 2.0 * view.near * depth_bias } else { 0.0 };
        let horizon = horizon(view);

        // SAFETY: uniform uploads on the program in use
        unsafe {
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_2_f32(
                viewport_u.as_ref(),
                (viewport.0 / 2.0) as f32,
                (viewport.1 / 2.0) as f32,
            );
            gl.uniform_1_f32(bias_u.as_ref(), bias as f32);
            gl.uniform_4_f32_slice(horizon_u.as_ref(), &horizon);
        }
    }

    /// The origin of the offsets drawn next, and the camera relative to it. Program in use
    pub fn set_origin(&self, gl: &glow::Context, view: &View, origin: DVec2) {
        let [_, origin_u, hi_u, lo_u, _, _, _] = &self.0;
        let camera = camera_offset(view, origin);
        let ((xh, xl), (yh, yl)) = (split(camera.x), split(camera.y));

        // SAFETY: uniform uploads on the program in use
        unsafe {
            gl.uniform_2_f32(origin_u.as_ref(), origin.x as f32, origin.y as f32);
            gl.uniform_2_f32(hi_u.as_ref(), xh, yh);
            gl.uniform_2_f32(lo_u.as_ref(), xl, yl);
        }
    }
}

/// `u_horizon` of a view: the eye from the sphere's center and `View::faces_camera`'s bound, nothing hidden off the
/// globe
#[must_use]
pub fn horizon(view: &View) -> [f32; 4] {
    if view.globe.factor <= 0.0 {
        return [0.0, 0.0, 0.0, -1.0];
    }

    let eye = view.eye - view.sphere_center();
    let r = view.globe.radius;

    [eye.x as f32, eye.y as f32, eye.z as f32, (0.98 * r * r) as f32]
}

/// The camera center's offset from a layer's origin, on the copy of the world nearest to the center (Mercator)
#[must_use]
pub fn camera_offset(view: &View, origin: DVec2) -> DVec2 {
    view.center - DVec2::new(origin.x + shift(origin, view), origin.y)
}

/// Picking ids: a feature's number + 1 in 24 bits of RGB. Whether `count` features fit (a warning in the console when
/// they do not: beyond, ids would overlap)
pub fn pick_ids_fit(count: usize, what: &str) -> bool {
    const MAX: usize = (1 << 24) - 1;
    if count <= MAX {
        return true;
    }
    #[cfg(target_arch = "wasm32")]
    web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
        "{what}: {count} features, picking tells apart only the first {MAX}"
    )));

    #[cfg(not(target_arch = "wasm32"))]
    let _ = what;

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::camera::{Camera, mercator};

    fn view(lon: f64, lat: f64, zoom: f64, pitch: f64) -> View {
        let mut camera = Camera::new(lon, lat, zoom);

        camera.width = 800.0;
        camera.height = 600.0;
        camera.pitch = pitch;

        camera.view()
    }

    #[test]
    fn the_horizon_hides_the_far_side_of_the_globe() {
        // The shader's test in f32, against View::faces_camera
        let behind = |v: &View, p: glam::DVec3| {
            let [x, y, z, w] = horizon(v);
            let q = (p - v.sphere_center()).as_vec3();

            q.dot(glam::Vec3::new(x, y, z)) <= w
        };
        let mut camera = Camera::new(-80.0, 0.0, 1.2);

        camera.globe = true;
        camera.width = 640.0;
        camera.height = 400.0;

        let v = camera.view();

        for lon in (-180..180).step_by(5) {
            for lat in (-80..=80).step_by(10) {
                let m = mercator(f64::from(lon), f64::from(lat));
                let p = v.position(m, -(m.x - v.center.x).round());

                assert_eq!(behind(&v, p), !v.faces_camera(p), "{lon} {lat}");
            }
        }

        // Gale crater, on the far side of Tharsis; Valles Marineris in front
        let at = |lon: f64, lat: f64| {
            let m = mercator(lon, lat);

            v.position(m, -(m.x - v.center.x).round())
        };

        assert!(behind(&v, at(137.44, -4.59)));
        assert!(!behind(&v, at(-59.2, -13.9)));

        // On the plane, nothing hidden
        let flat = view(-80.0, 0.0, 12.0, 60.0);

        assert!(!behind(&flat, flat.position(mercator(-79.0, 0.5), 0.0)));
    }

    #[test]
    fn split_keeps_the_low_bits() {
        let x = 0.507_123_456_789_012_3;
        let (hi, lo) = split(x);

        assert!((f64::from(hi) + f64::from(lo) - x).abs() < 1e-15);
        assert!((f64::from(hi) - x).abs() > 1e-10);
    }

    #[test]
    fn camera_relative_offsets_stay_exact_far_from_the_origin() {
        // A layer as wide as France, its origin in the middle; the camera at zoom 22 over Lille
        let origin = mercator(2.5, 46.5);
        let v = view(3.06, 50.63, 22.0, 0.0);
        let point = mercator(3.060_01, 50.630_01);
        let offset = ((point - origin).as_vec2(), (point - origin));
        let camera = camera_offset(&v, origin);
        let ((xh, xl), (yh, yl)) = (split(camera.x), split(camera.y));
        // The shader's arithmetic, in f32
        let plane = |o: f32, h: f32, l: f32| ((o - h) - l) * v.world_size as f32;
        let gpu = DVec2::new(
            f64::from(plane(offset.0.x, xh, xl)),
            f64::from(plane(offset.0.y, yh, yl)),
        );
        let exact = (point - v.center) * v.world_size;

        // Within a pixel; the old `origin_px + offset * world` was off by several
        assert!((gpu - exact).length() < 1.0, "{gpu} {exact}");

        let old = (origin - v.center).as_vec2() * v.world_size as f32 + offset.0 * v.world_size as f32;

        assert!((old.as_dvec2() - exact).length() > 1.0);
    }

    #[test]
    fn points_and_bounds() {
        let p = points(&[0.1, 0.2, 0.3, 0.05, 0.9]);

        assert_eq!(p, [DVec2::new(0.1, 0.2), DVec2::new(0.3, 0.05)]);
        assert_eq!(bounds(&p), Some((DVec2::new(0.1, 0.05), DVec2::new(0.3, 0.2))));
        assert_eq!(bounds(&[]), None);

        // Points not numbers are left out
        let q = [DVec2::new(f64::INFINITY, 0.0), DVec2::new(0.2, 0.4), DVec2::NAN];

        assert_eq!(bounds(&q), Some((DVec2::new(0.2, 0.4), DVec2::new(0.2, 0.4))));
        assert_eq!(bounds(&q[2..]), None);
    }

    #[test]
    fn pick_ids_are_24_bits() {
        assert!(pick_ids_fit((1 << 24) - 1, "test"));
        assert!(!pick_ids_fit(1 << 24, "test"));
    }
}
