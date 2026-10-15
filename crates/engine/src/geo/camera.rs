//! Perspective camera, with the same conventions as `MapLibre` (center, zoom, pitch, bearing), so camera settings move
//! from one engine to the other unchanged.
//!
//! Two projections: Web Mercator (a plane) and the globe. The globe is a sphere at low zoom that blends into the
//! Mercator plane between zooms 5 and 7 (`GLOBE_FULL`, `GLOBE_NONE`); above, the globe and Mercator views are the
//! same. The sphere radius makes one zoom level mean the same scale at the center in both projections.
//!
//! Everything is computed in f64. The GPU only receives coordinates relative to the camera center, in pixels at the
//! current zoom, which keeps f32 exact to the centimetre at any zoom.
//!
//! Local frame of a view: pixels relative to the center, x east, y south, z up. The sphere touches the plane at the
//! center, its own center at (0, 0, -radius).

use crate::geo::geodesy;
use glam::{DMat4, DVec2, DVec3, DVec4};
use std::f64::consts::PI;

/// Size of the world in CSS pixels at zoom 0 (`MapLibre` convention)
pub const TILE_SIZE: f64 = 512.0;
/// Vertical field of view (`MapLibre` default: 0.6435 rad, about 36.87°)
pub const FOV: f64 = 0.643_501_108_793_284_4;
pub const MAX_PITCH: f64 = 70.0;
pub const MIN_ZOOM: f64 = 0.0;
pub const MAX_ZOOM: f64 = 22.0;
/// Globe projection: full sphere up to this zoom…
pub const GLOBE_FULL: f64 = 5.0;
/// …blending into the Mercator plane up to this one
pub const GLOBE_NONE: f64 = 7.0;
/// Mercator latitude limit
const MAX_LAT: f64 = 85.051_128_779_806_59;

/// Longitude and latitude (degrees) to Mercator coordinates in [0, 1], y towards the south
#[must_use]
pub fn mercator(lon: f64, lat: f64) -> DVec2 {
    let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    DVec2::new(
        (lon + 180.0) / 360.0,
        0.5 - (PI / 4.0 + lat / 2.0).tan().ln() / (2.0 * PI),
    )
}

/// Mercator coordinates in [0, 1] to longitude and latitude (degrees)
#[must_use]
pub fn lon_lat(m: DVec2) -> DVec2 {
    let lat = (PI * (1.0 - 2.0 * m.y)).sinh().atan().to_degrees();
    DVec2::new(m.x * 360.0 - 180.0, lat)
}

/// Unit vector from the Earth center (x: lon 0 lat 0, y: lon 90° E, z: north pole) of a Mercator point
#[must_use]
pub fn unit(m: DVec2) -> DVec3 {
    let lon = m.x * 2.0 * PI - PI;
    let lat = (PI * (1.0 - 2.0 * m.y)).sinh().atan();
    DVec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin())
}

#[derive(Clone, Debug)]
pub struct Camera {
    /// Center in Mercator [0, 1]
    pub center: DVec2,
    pub zoom: f64,
    /// Degrees from the vertical
    pub pitch: f64,
    /// Degrees clockwise from the north
    pub bearing: f64,
    /// Viewport in CSS pixels
    pub width: f64,
    pub height: f64,
    pub min_zoom: f64,
    pub max_zoom: f64,
    /// Globe projection (else Web Mercator)
    pub globe: bool,
    /// Altitude of the center (metres): the camera turns around the ground of the relief, not sea level
    pub elevation: f64,
}

/// Sphere of a view, in the local frame
#[derive(Clone, Copy, Debug)]
pub struct Globe {
    /// 1 = sphere, 0 = Mercator plane
    pub factor: f64,
    /// Pixels
    pub radius: f64,
    /// Local axes at the center, in Earth coordinates (`unit`)
    pub east: DVec3,
    pub north: DVec3,
    pub up: DVec3,
}

/// Matrices of one frame, derived from the camera
#[derive(Clone, Debug)]
pub struct View {
    /// Local frame to clip space
    pub view_proj: DMat4,
    pub inverse: DMat4,
    /// Camera position in the local frame
    pub eye: DVec3,
    /// Distance from the camera to the center, in pixels
    pub center_distance: f64,
    /// Near plane distance, in pixels
    pub near: f64,
    /// Altitude of the center in the local frame (pixels): the plane (or sphere) the camera turns around
    pub lift: f64,
    /// Pixels of the local frame per metre of altitude
    pub vertical: f64,
    pub world_size: f64,
    pub center: DVec2,
    pub globe: Globe,
}

impl View {
    /// Position in the local frame of a Mercator point. `shift`: whole worlds added to x on the plane, to draw a
    /// tile on the copy of the world nearest to the center.
    #[must_use]
    pub fn position(&self, m: DVec2, shift: f64) -> DVec3 {
        let plane = DVec3::new(
            (m.x + shift - self.center.x) * self.world_size,
            (m.y - self.center.y) * self.world_size,
            0.0,
        );
        if self.globe.factor <= 0.0 {
            return plane;
        }
        plane.lerp(self.sphere(m), self.globe.factor)
    }

    #[must_use]
    pub fn sphere(&self, m: DVec2) -> DVec3 {
        let (g, n) = (&self.globe, unit(m));
        g.radius * DVec3::new(n.dot(g.east), -n.dot(g.north), n.dot(g.up) - 1.0)
    }

    /// Center of the sphere in the local frame
    #[must_use]
    pub fn sphere_center(&self) -> DVec3 {
        DVec3::new(0.0, 0.0, -self.globe.radius)
    }

    /// Mercator point of a position on the sphere, or above or under it (local frame)
    #[must_use]
    pub fn sphere_to_mercator(&self, p: DVec3) -> DVec2 {
        let g = &self.globe;
        let local = (p - self.sphere_center()).normalize_or_zero();
        let n = g.east * local.x - g.north * local.y + g.up * local.z;
        mercator(n.y.atan2(n.x).to_degrees(), n.z.clamp(-1.0, 1.0).asin().to_degrees())
    }

    /// Pixels per metre at a latitude (degrees): on the plane, as Web Mercator scales it; on the sphere, its radius
    /// over the Earth's
    #[must_use]
    pub fn pixels_per_metre(&self, lat: f64) -> f64 {
        let plane = self.world_size / (2.0 * PI * geodesy::A * lat.to_radians().cos().max(1e-6));
        let sphere = self.globe.radius / geodesy::A;
        plane + (sphere - plane) * self.globe.factor
    }

    /// Affine map from ECEF offsets around `anchor` (metres) to the local frame (pixels), linearized at the anchor:
    /// the anchor is placed like a ground point at its height, the offsets go through its east-north-up axes. Exact
    /// at the anchor, a tile-sized neighbourhood stays well under a pixel.
    #[must_use]
    pub fn ecef_matrix(&self, anchor: DVec3) -> DMat4 {
        let g = geodesy::from_ecef(anchor);
        let m = mercator(g.x, g.y);
        let [east, north, up] = geodesy::enu(g.x, g.y);
        let k = self.pixels_per_metre(g.y);
        let shift = -(m.x - self.center.x).round();
        // Where the anchor goes, and where its east, north and up axes point, blended like `position`
        let mut origin = self.position(m, shift);
        let (mut e, mut n, mut u) = (DVec3::X, DVec3::NEG_Y, DVec3::Z);
        if self.globe.factor > 0.0 {
            let local = |v: DVec3| DVec3::new(v.dot(self.globe.east), -v.dot(self.globe.north), v.dot(self.globe.up));
            let f = self.globe.factor;
            e = e.lerp(local(east), f);
            n = n.lerp(local(north), f);
            u = u.lerp(local(up), f);
        }
        origin += u * (g.z * k);
        // Columns: images of the ECEF axes (through east, north, up)
        let to_local = |axis: DVec3| (e * axis.dot(east) + n * axis.dot(north) + u * axis.dot(up)) * k;
        DMat4::from_cols(
            to_local(DVec3::X).extend(0.0),
            to_local(DVec3::Y).extend(0.0),
            to_local(DVec3::Z).extend(0.0),
            origin.extend(1.0),
        )
    }

    /// Position in the local frame of an ECEF point
    #[must_use]
    pub fn ecef(&self, p: DVec3) -> DVec3 {
        self.ecef_matrix(p).w_axis.truncate()
    }

    /// Whether a position on the sphere faces the camera (in front of the horizon)
    #[must_use]
    pub fn faces_camera(&self, p: DVec3) -> bool {
        let c = self.sphere_center();
        (p - c).dot(self.eye - c) > 0.98 * self.globe.radius * self.globe.radius
    }
}

impl Camera {
    #[must_use]
    pub fn new(lon: f64, lat: f64, zoom: f64) -> Self {
        Self {
            center: mercator(lon, lat),
            zoom,
            pitch: 0.0,
            bearing: 0.0,
            width: 1.0,
            height: 1.0,
            min_zoom: MIN_ZOOM,
            max_zoom: MAX_ZOOM,
            globe: false,
            elevation: 0.0,
        }
    }

    #[must_use]
    pub fn world_size(&self) -> f64 {
        TILE_SIZE * self.zoom.exp2()
    }

    pub fn clamp(&mut self) {
        // On the globe, the zoom follows the latitude to keep the globe's size (`shift_center`): the floor is the
        // equator's, lowered by the same amount, so the whole globe can be seen from any latitude
        let floor = if self.globe {
            self.min_zoom + lon_lat(self.center).y.to_radians().cos().max(1e-3).log2()
        } else {
            self.min_zoom
        };
        self.zoom = self.zoom.clamp(floor, self.max_zoom);
        self.pitch = self.pitch.clamp(0.0, MAX_PITCH);
        self.bearing = (self.bearing + 540.0).rem_euclid(360.0) - 180.0;
        self.center.x = self.center.x.rem_euclid(1.0);
        let lat_limit = mercator(0.0, MAX_LAT).y;
        self.center.y = self.center.y.clamp(lat_limit, 1.0 - lat_limit);
    }

    fn globe(&self) -> Globe {
        let factor = if self.globe {
            ((GLOBE_NONE - self.zoom) / (GLOBE_NONE - GLOBE_FULL)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let lon_lat = lon_lat(self.center);
        let (lon, lat) = (lon_lat.x.to_radians(), lon_lat.y.to_radians());
        Globe {
            factor,
            // Same scale at the center as the Mercator plane
            radius: self.world_size() / (2.0 * PI * lat.cos()),
            east: DVec3::new(-lon.sin(), lon.cos(), 0.0),
            north: DVec3::new(-lat.sin() * lon.cos(), -lat.sin() * lon.sin(), lat.cos()),
            up: DVec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()),
        }
    }

    #[must_use]
    pub fn view(&self) -> View {
        let pitch = self.pitch.to_radians();
        let center_distance = 0.5 * self.height / (FOV / 2.0).tan();
        let globe = self.globe();
        // Far plane: just past the furthest ground point visible at the top of the screen…
        let top_angle = (PI / 2.0 - pitch - FOV / 2.0).max(0.01);
        let top_half = (FOV / 2.0).sin() * center_distance / top_angle.sin();
        let mut far = ((PI / 2.0 - pitch).cos() * top_half + center_distance) * 1.01;
        // …or past the whole sphere
        if globe.factor > 0.0 {
            far = far.max(center_distance + 2.0 * globe.radius);
        }
        // Valleys of the relief lie below the center: some margin
        if self.elevation != 0.0 {
            far *= 1.25;
        }
        let near = self.height / 50.0;
        let proj = DMat4::perspective_rh_gl(FOV, self.width / self.height, near, far);
        // The center stands at its altitude: the local frame goes down under the camera
        let lat = (PI * (1.0 - 2.0 * self.center.y)).sinh().atan();
        let plane = self.world_size() / (2.0 * PI * geodesy::A * lat.cos().max(1e-6));
        let vertical = plane + (globe.radius / geodesy::A - plane) * globe.factor;
        let lift = self.elevation * vertical;
        // Local frame (y south, z up) to eye space (y up, looking down -z)
        let view = DMat4::from_translation(DVec3::new(0.0, 0.0, -center_distance))
            * DMat4::from_rotation_x(-pitch)
            * DMat4::from_rotation_z(self.bearing.to_radians())
            * DMat4::from_scale(DVec3::new(1.0, -1.0, 1.0))
            * DMat4::from_translation(DVec3::new(0.0, 0.0, -lift));
        let view_proj = proj * view;
        let eye = view.inverse().transform_point3(DVec3::ZERO);
        View {
            view_proj,
            inverse: view_proj.inverse(),
            eye,
            center_distance,
            near,
            lift,
            vertical,
            world_size: self.world_size(),
            center: self.center,
            globe,
        }
    }

    /// Ground point (Mercator) under a screen point (CSS pixels from the top left), None off the ground; the ground is
    /// taken at the altitude of the center
    #[must_use]
    pub fn ground(&self, view: &View, x: f64, y: f64) -> Option<DVec2> {
        self.ground_at(view, x, y, self.elevation)
    }

    /// Ray from the camera through a screen point (CSS pixels): its points on the near and far planes, local frame
    #[must_use]
    pub fn ray(&self, view: &View, x: f64, y: f64) -> (DVec3, DVec3) {
        let ndc = DVec2::new(2.0 * x / self.width - 1.0, 1.0 - 2.0 * y / self.height);
        let unproject = |z: f64| {
            let p = view.inverse * DVec4::new(ndc.x, ndc.y, z, 1.0);
            p.truncate() / p.w
        };
        (unproject(-1.0), unproject(1.0))
    }

    /// Point (Mercator) where the ray through a screen point meets the surface at `altitude` (metres, exaggeration
    /// included), None if it misses it
    #[must_use]
    pub fn ground_at(&self, view: &View, x: f64, y: f64, altitude: f64) -> Option<DVec2> {
        let (a, b) = self.ray(view, x, y);
        let h = altitude * view.vertical;
        // While blending, the surface is closer to the sphere or to the plane
        if view.globe.factor >= 0.5 {
            let (d, c, r) = (b - a, view.sphere_center(), view.globe.radius + h);
            let (qa, qb, qc) = (d.dot(d), 2.0 * d.dot(a - c), (a - c).length_squared() - r * r);
            let disc = qb * qb - 4.0 * qa * qc;
            if disc < 0.0 {
                return None;
            }
            let t = (-qb - disc.sqrt()) / (2.0 * qa);
            return (t >= 0.0).then(|| view.sphere_to_mercator(a + d * t));
        }
        let t = (a.z - h) / (a.z - b.z);
        if !(0.0..=1.0).contains(&t) || b.z >= a.z {
            return None;
        }
        let p = a + (b - a) * t;
        Some(self.center + DVec2::new(p.x, p.y) / view.world_size)
    }

    /// Altitude (metres, exaggeration included) of a point of the local frame
    #[must_use]
    pub fn altitude_of(view: &View, p: DVec3) -> f64 {
        if view.globe.factor >= 0.5 {
            (p.distance(view.sphere_center()) - view.globe.radius) / view.vertical
        } else {
            p.z / view.vertical
        }
    }

    /// Moves the center so that the ground under `from` comes under `to` (screen points, CSS pixels)
    pub fn pan(&mut self, from: DVec2, to: DVec2) {
        self.pan_at(from, to, self.elevation);
    }

    /// `pan`, the grabbed point being at `altitude` (metres, exaggeration included): it stays under the cursor. The
    /// eye moves at a constant height: the point it orbits keeps its altitude, and `settle_elevation` brings it down
    /// to the ground afterwards without moving the eye (like Google Earth)
    pub fn pan_at(&mut self, from: DVec2, to: DVec2, altitude: f64) {
        let Some(grabbed) = self.ground_at(&self.view(), from.x, from.y, altitude) else {
            return;
        };
        // On the globe, turning it also turns the local frame: a few corrections keep the grabbed point under the
        // cursor
        for _ in 0..if self.globe { 4 } else { 1 } {
            let Some(under) = self.ground_at(&self.view(), to.x, to.y, altitude) else {
                return;
            };
            self.shift_center(grabbed, under);
        }
    }

    /// Zooms by `delta` levels, keeping the ground under the screen point `at` in place
    pub fn zoom_at(&mut self, delta: f64, at: DVec2) {
        self.zoom_at_altitude(delta, at, self.elevation);
    }

    /// `zoom_at`, the ground under `at` being at `altitude` (metres, exaggeration included); as for `pan_at`, the
    /// orbited point keeps its altitude
    pub fn zoom_at_altitude(&mut self, delta: f64, at: DVec2, altitude: f64) {
        let Some(before) = self.ground_at(&self.view(), at.x, at.y, altitude) else {
            self.zoom += delta;
            self.clamp();
            return;
        };
        self.zoom += delta;
        self.clamp();
        // On the globe, moving the center also turns the local frame: a few corrections converge
        for _ in 0..if self.globe { 4 } else { 1 } {
            let Some(after) = self.ground_at(&self.view(), at.x, at.y, altitude) else {
                return;
            };
            self.shift_center(before, after);
        }
    }

    /// Sets the altitude of the ground under the center without moving the eye: when finer relief arrives under a
    /// still camera, the point it orbits slides along the view axis to the new ground and the zoom follows the new
    /// distance, so nothing moves on screen. On the globe (where relief barely shows), or if the new ground would
    /// come too close to the eye, the altitude is just set.
    pub fn settle_elevation(&mut self, elevation: f64) {
        let delta = elevation - self.elevation;
        let view = self.view();
        let pitch = self.pitch.to_radians();
        if delta == 0.0 || view.globe.factor > 0.0 {
            self.elevation = elevation;
            return;
        }
        // Along the view axis, by `along` metres towards the eye for a rise of `delta`
        let metres = view.vertical; // pixels per metre
        let distance = view.center_distance / metres;
        let along = delta / pitch.cos();
        if distance - along < distance * 0.1 {
            self.elevation = elevation;
            return;
        }
        // The eye stands back from the center along the horizontal direction of `eye`
        let back = DVec2::new(view.eye.x, view.eye.y);
        if back.length() > 1e-9 {
            let shift = back.normalize() * delta * pitch.tan() * metres / view.world_size;
            self.center += shift;
        }
        self.zoom += (distance / (distance - along)).log2();
        self.elevation = elevation;
        self.clamp();
    }

    /// Moves the center by the ground offset from `b` to `a` (Mercator points).
    ///
    /// On the globe, the move is made in longitude and latitude (a turn of the globe: Mercator offsets stretch towards
    /// the poles), and the zoom follows the latitude so that the globe keeps its size on screen: its radius is
    /// 2^zoom / cos(latitude), to match the Mercator scale at the center. Without this, dragging the globe north or
    /// south would look like zooming.
    fn shift_center(&mut self, a: DVec2, b: DVec2) {
        let factor = self.globe().factor;
        let before = lon_lat(self.center);
        if factor >= 0.5 {
            let mut d = lon_lat(a) - lon_lat(b);
            d.x = (d.x + 540.0).rem_euclid(360.0) - 180.0;
            self.center = mercator(before.x + d.x, (before.y + d.y).clamp(-MAX_LAT, MAX_LAT));
        } else {
            let mut d = a - b;
            // Across the antimeridian, the short way
            d.x -= d.x.round();
            self.center += d;
        }
        self.clamp();
        if factor > 0.0 {
            let after = lon_lat(self.center);
            self.zoom += factor * (after.y.to_radians().cos() / before.y.to_radians().cos()).log2();
            self.clamp();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        let mut c = Camera::new(2.424_573, 48.845_726, 14.0);
        c.width = 1200.0;
        c.height = 800.0;
        c
    }

    fn globe(zoom: f64) -> Camera {
        let mut c = camera();
        c.globe = true;
        c.zoom = zoom;
        c
    }

    #[test]
    fn mercator_round_trip() {
        let m = mercator(2.424_573, 48.845_726);
        let back = lon_lat(m);
        assert!((back.x - 2.424_573).abs() < 1e-9 && (back.y - 48.845_726).abs() < 1e-9);
    }

    #[test]
    fn screen_center_is_camera_center() {
        for (pitch, bearing) in [(0.0, 0.0), (60.0, 0.0), (45.0, -17.0), (70.0, 120.0)] {
            for mut c in [camera(), globe(3.0)] {
                c.pitch = pitch;
                c.bearing = bearing;
                let g = c.ground(&c.view(), 600.0, 400.0).unwrap();
                assert!(
                    (g - c.center).length() < 1e-12,
                    "pitch {pitch} bearing {bearing} globe {}",
                    c.globe
                );
            }
        }
    }

    #[test]
    fn north_is_up_and_east_is_right() {
        let c = camera();
        let v = c.view();
        let top = c.ground(&v, 600.0, 0.0).unwrap();
        let right = c.ground(&v, 1200.0, 400.0).unwrap();
        assert!(top.y < c.center.y && (top.x - c.center.x).abs() < 1e-12);
        assert!(right.x > c.center.x && (right.y - c.center.y).abs() < 1e-12);
    }

    #[test]
    fn bearing_turns_the_map_clockwise() {
        // Bearing 90: the east is at the top of the screen
        let mut c = camera();
        c.bearing = 90.0;
        let top = c.ground(&c.view(), 600.0, 0.0).unwrap();
        assert!(top.x > c.center.x && (top.y - c.center.y).abs() < 1e-12);
    }

    #[test]
    fn pitch_pushes_the_top_of_the_screen_further() {
        let mut c = camera();
        let flat = c.center.y - c.ground(&c.view(), 600.0, 0.0).unwrap().y;
        c.pitch = 60.0;
        let tilted = c.center.y - c.ground(&c.view(), 600.0, 0.0).unwrap().y;
        assert!(tilted > 2.0 * flat);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        for mut c in [camera(), globe(3.0)] {
            c.pitch = 30.0;
            let at = DVec2::new(800.0, 300.0);
            let before = c.ground(&c.view(), at.x, at.y).unwrap();
            c.zoom_at(0.3, at);
            let after = c.ground(&c.view(), at.x, at.y).unwrap();
            assert!(
                (before - after).length() < 1e-4,
                "globe {}: {}",
                c.globe,
                (before - after).length()
            );
        }
    }

    #[test]
    fn pan_moves_the_grabbed_point() {
        let mut c = camera();
        c.pitch = 45.0;
        let (from, to) = (DVec2::new(300.0, 500.0), DVec2::new(700.0, 350.0));
        let grabbed = c.ground(&c.view(), from.x, from.y).unwrap();
        c.pan(from, to);
        let now = c.ground(&c.view(), to.x, to.y).unwrap();
        assert!((grabbed - now).length() < 1e-11);
    }

    #[test]
    fn sphere_touches_the_plane_at_the_center() {
        let v = globe(3.0).view();
        assert!(v.sphere(v.center).length() < 1e-6);
        // One pixel east on the plane is one pixel east on the sphere
        let m = v.center + DVec2::new(1.0 / v.world_size, 0.0);
        let s = v.sphere(m);
        assert!((s.x - 1.0).abs() < 1e-3 && s.y.abs() < 1e-3, "{s}");
        assert!((v.sphere_to_mercator(s) - m).length() < 1e-12);
    }

    #[test]
    fn ground_at_an_altitude() {
        // A point 900 m up seen from a tilted camera standing over a 1000 m relief: back to it through its screen point
        for globe_on in [false, true] {
            let mut c = camera();
            c.globe = globe_on;
            c.zoom = if globe_on { 4.0 } else { 14.0 };
            c.pitch = 55.0;
            c.elevation = 1000.0;
            let view = c.view();
            let m = c.center + DVec2::new(0.6, -0.4) / view.world_size * 300.0;
            let base = view.position(m, 0.0);
            let up = if view.globe.factor >= 0.5 {
                (base - view.sphere_center()).normalize()
            } else {
                DVec3::Z
            };
            let p = base + up * 900.0 * view.vertical;
            let clip = view.view_proj * p.extend(1.0);
            let (x, y) = (
                f64::midpoint(clip.x / clip.w, 1.0) * c.width,
                (1.0 - clip.y / clip.w) / 2.0 * c.height,
            );
            let found = c.ground_at(&view, x, y, 900.0).expect("on the ground");
            assert!((found - m).length() * view.world_size < 0.01, "{globe_on}: {found} {m}");
            assert!((Camera::altitude_of(&view, p) - 900.0).abs() < 0.01);
            // At the center's altitude instead, the ray meets the ground elsewhere (100 m off: visible at zoom 14)
            if !globe_on {
                let wrong = c.ground(&view, x, y).expect("on the ground");
                assert!((wrong - m).length() * view.world_size > 1.0);
            }
        }
    }

    #[test]
    fn grabbed_relief_stays_under_the_cursor() {
        // A slope rising 2000 m per 0.001 of Mercator y towards the north, the camera tilted over it
        let slope = |m: DVec2| Some(1000.0 + (0.3384 - m.y) * 2e6);
        let mut c = camera();
        c.pitch = 65.0;
        c.zoom = 13.5;
        c.elevation = slope(c.center).unwrap();
        let (from, to) = (DVec2::new(900.0, 250.0), DVec2::new(700.0, 330.0));
        // The relief under `from`: search the altitude where the ray meets the slope
        let view = c.view();
        let mut altitude = c.elevation;
        for _ in 0..50 {
            altitude = slope(c.ground_at(&view, from.x, from.y, altitude).unwrap()).unwrap();
        }
        let grabbed = c.ground_at(&view, from.x, from.y, altitude).unwrap();
        for k in 1..=10 {
            let a = from + (to - from) * f64::from(k - 1) / 10.0;
            let b = from + (to - from) * f64::from(k) / 10.0;
            c.pan_at(a, b, altitude);
            // Each frame brings the orbited point down to the ground under the new center, the eye staying
            c.settle_elevation(slope(c.center).unwrap());
        }
        let under = c.ground_at(&c.view(), to.x, to.y, altitude).unwrap();
        let error = (under - grabbed).length() * c.view().world_size;
        assert!(error < 0.05, "{error} px");
        // The settled center moves too: the next frames converge on the ground, still without moving the grabbed point
        for _ in 0..20 {
            c.settle_elevation(slope(c.center).unwrap());
        }
        assert!((c.elevation - slope(c.center).unwrap()).abs() < 0.01);
        let under = c.ground_at(&c.view(), to.x, to.y, altitude).unwrap();
        assert!((under - grabbed).length() * c.view().world_size < 0.05);
    }

    #[test]
    fn finer_relief_does_not_move_the_view() {
        let mut c = camera();
        c.pitch = 60.0;
        c.bearing = 30.0;
        c.zoom = 14.0;
        c.elevation = 800.0;
        let before = c.view();
        // A ground point 1000 m up, off the center: where it shows before and after the center's altitude changes
        let p = before.position(c.center + DVec2::new(150.0, -90.0) / before.world_size, 0.0)
            + DVec3::Z * 1000.0 * before.vertical;
        let screen = |c: &Camera, view: &View| {
            let m = c.center + DVec2::new(p.x, p.y) / before.world_size + before.center - c.center;
            let q = view.position(m, 0.0) + DVec3::Z * 1000.0 * view.vertical;
            let clip = view.view_proj * q.extend(1.0);
            DVec2::new(clip.x / clip.w, clip.y / clip.w)
        };
        let a = screen(&c, &before);
        c.settle_elevation(950.0);
        let after = c.view();
        let b = screen(&c, &after);
        assert!((a - b).length() * c.width < 0.05, "{a} {b}");
        assert!((c.elevation - 950.0).abs() < 1e-9);
    }

    #[test]
    fn far_side_of_the_globe_is_hidden() {
        let v = globe(1.0).view();
        let antipode = DVec2::new((v.center.x + 0.5).rem_euclid(1.0), 1.0 - v.center.y);
        assert!(v.faces_camera(v.sphere(v.center)));
        assert!(!v.faces_camera(v.sphere(antipode)));
    }

    #[test]
    fn globe_blends_into_the_plane() {
        assert!((globe(4.0).view().globe.factor - 1.0).abs() < 1e-12);
        assert!(globe(7.5).view().globe.factor.abs() < 1e-12);
        assert!(camera().view().globe.factor.abs() < 1e-12);
    }

    #[test]
    fn dragging_the_globe_keeps_its_size() {
        // From France towards the north pole and back: the globe turns, its radius on screen stays the same
        let mut c = globe(2.5);
        let radius = c.view().globe.radius;
        for _ in 0..10 {
            c.pan(DVec2::new(600.0, 300.0), DVec2::new(600.0, 420.0));
        }
        assert!(lon_lat(c.center).y > 60.0, "{}", lon_lat(c.center).y);
        assert!((c.view().globe.radius / radius - 1.0).abs() < 1e-9);
        for _ in 0..20 {
            c.pan(DVec2::new(600.0, 420.0), DVec2::new(600.0, 300.0));
        }
        assert!(lon_lat(c.center).y < 30.0, "{}", lon_lat(c.center).y);
        assert!((c.view().globe.radius / radius - 1.0).abs() < 1e-9);
    }

    #[test]
    fn dragging_the_globe_follows_the_cursor() {
        let mut c = globe(2.5);
        let (from, to) = (DVec2::new(600.0, 300.0), DVec2::new(640.0, 360.0));
        let grabbed = lon_lat(c.ground(&c.view(), from.x, from.y).unwrap());
        c.pan(from, to);
        let now = lon_lat(c.ground(&c.view(), to.x, to.y).unwrap());
        assert!((grabbed - now).length() < 1e-6, "{grabbed} → {now}");
    }

    #[test]
    fn ecef_points_land_where_the_basemap_puts_them() {
        // A ground point 300 m east of the center, through the ECEF map, lands where the Mercator plane has it
        for globe_zoom in [None, Some(3.0), Some(6.0)] {
            let mut c = camera();
            if let Some(z) = globe_zoom {
                c.globe = true;
                c.zoom = z;
            }
            let v = c.view();
            let center = lon_lat(c.center);
            let anchor = geodesy::to_ecef(center.x, center.y, 0.0);
            assert!(v.ecef(anchor).length() < 1e-6, "anchor at the center");
            let east = geodesy::to_ecef(
                center.x + 300.0 / (111_320.0 * center.y.to_radians().cos()),
                center.y,
                0.0,
            );
            let through_matrix = v.ecef_matrix(anchor).transform_point3(east - anchor);
            let direct = v.position(
                mercator(center.x + 300.0 / (111_320.0 * center.y.to_radians().cos()), center.y),
                0.0,
            );
            let scale = v.pixels_per_metre(center.y) * 300.0;
            assert!(
                (through_matrix - direct).length() < 0.02 * scale + 0.01,
                "{globe_zoom:?}: {through_matrix} vs {direct}"
            );
            // Up is up, scaled like the ground
            let high = v.ecef(geodesy::to_ecef(center.x, center.y, 100.0));
            assert!(
                (high.z - 100.0 * v.pixels_per_metre(center.y)).abs() < 1e-3 * high.z.abs().max(1.0),
                "{high}"
            );
        }
    }

    #[test]
    fn whole_globe_fits_at_zoom_zero() {
        // Above the globe, the ground is missing
        let c = globe(0.0);
        let v = c.view();
        assert!(c.ground(&v, 600.0, 400.0).is_some());
        assert!(c.ground(&v, 5.0, 5.0).is_none());
    }
}
