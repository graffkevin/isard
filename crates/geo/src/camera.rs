//! Perspective camera, with the same conventions as `MapLibre` (center, zoom, pitch, bearing), so camera settings move
//! from one engine to the other unchanged.
//!
//! Two projections: Web Mercator (a plane) and the globe. The globe is a sphere at low zoom that blends into the
//! Mercator plane between zooms 5 and 7 (`GLOBE_FULL`, `GLOBE_NONE`), later under a tilted camera, which sees much
//! farther than its center (`globe_tilt`, up to `GLOBE_TILT_LEVELS`); above, the globe and Mercator views are the
//! same. The sphere radius makes one zoom level mean the same scale at the center in both projections.
//!
//! Everything is computed in f64. The GPU only receives coordinates relative to the camera center, in pixels at the
//! current zoom, which keeps f32 exact to the centimetre at any zoom.
//!
//! Local frame of a view: pixels relative to the center, x east, y south, z up. The sphere touches the plane at the
//! center, its own center at (0, 0, -radius).

use crate::geodesy::{self, Body};
use glam::{DMat3, DMat4, DVec2, DVec3, DVec4};
use std::f64::consts::PI;

/// Size of the world in CSS pixels at zoom 0 (`MapLibre` convention)
pub const TILE_SIZE: f64 = 512.0;
/// Vertical field of view (`MapLibre` default: 0.6435 rad, about 36.87°)
pub const FOV: f64 = 0.643_501_108_793_284_4;
/// Default pitch limit; `Camera::max_pitch` can raise it up to `PITCH_LIMIT`
pub const MAX_PITCH: f64 = 70.0;
/// Highest pitch a camera accepts: above, the view is almost all horizon
pub const PITCH_LIMIT: f64 = 85.0;
/// Under a camera tilted past the horizon, the ground is drawn up to this many center distances away (where the
/// fog of `Atmosphere` hides the edge)
pub const FAR_LIMIT: f64 = 100.0;
pub const MIN_ZOOM: f64 = 0.0;
pub const MAX_ZOOM: f64 = 22.0;
/// Globe projection: full sphere up to this zoom…
pub const GLOBE_FULL: f64 = 5.0;
/// …blending into the Mercator plane up to this one
pub const GLOBE_NONE: f64 = 7.0;
/// Levels the blend into the plane is put off by, at most, under a tilted camera: the sphere stays up to zoom 10 for a
/// view along the horizon (the f32 positions of the sphere are still exact to a tenth of a pixel there)
pub const GLOBE_TILT_LEVELS: f64 = 3.0;
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

/// Unit vector from the body's center (x: lon 0 lat 0, y: lon 90° E, z: north pole) of a Mercator point
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
    /// Degrees, up to `PITCH_LIMIT`
    pub max_pitch: f64,
    /// Globe projection (else Web Mercator)
    pub globe: bool,
    /// Altitude of the center (metres): the camera turns around the ground of the relief, not sea level
    pub elevation: f64,
    /// The body the map is drawn on: its size turns zoom levels into metres
    pub body: Body,
}

/// Sphere of a view, in the local frame
#[derive(Clone, Copy, Debug)]
pub struct Globe {
    /// 1 = sphere, 0 = Mercator plane
    pub factor: f64,
    /// Pixels
    pub radius: f64,
    /// Local axes at the center, in body coordinates (`unit`)
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
    /// Near and far plane distances, in pixels
    pub near: f64,
    pub far: f64,
    /// Altitude of the center in the local frame (pixels): the plane (or sphere) the camera turns around
    pub lift: f64,
    /// Pixels of the local frame per metre of altitude
    pub vertical: f64,
    pub world_size: f64,
    pub center: DVec2,
    pub globe: Globe,
    pub body: Body,
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
    /// over the body's
    #[must_use]
    pub fn pixels_per_metre(&self, lat: f64) -> f64 {
        let plane = self.world_size / (2.0 * PI * self.body.radius * lat.to_radians().cos().max(1e-6));
        let sphere = self.globe.radius / self.body.radius;
        plane + (sphere - plane) * self.globe.factor
    }

    /// Affine map from ECEF offsets around `anchor` (metres) to the local frame (pixels), linearized at the anchor:
    /// the anchor is placed like a ground point at its height, the offsets go through its east-north-up axes. Exact
    /// at the anchor, a tile-sized neighbourhood stays well under a pixel.
    #[must_use]
    pub fn ecef_matrix(&self, anchor: DVec3) -> DMat4 {
        let g = self.body.from_ecef(anchor);
        // ECEF offsets to east, north, up (rows), then `enu_matrix`
        let [east, north, up] = geodesy::enu(g.x, g.y);
        self.enu_matrix(g) * DMat4::from_mat3(DMat3::from_cols(east, north, up).transpose())
    }

    /// Affine map from a local east-north-up frame in metres at a longitude, latitude (degrees) and height (metres)
    /// to the local frame (pixels): the origin placed like a ground point at its height, its axes blended between the
    /// plane's and the sphere's like `position`. The frame of the models and of the page's own drawings (custom
    /// layers): exact at the origin, a few kilometres around it stay well under a pixel.
    #[must_use]
    pub fn enu_matrix(&self, g: DVec3) -> DMat4 {
        let m = mercator(g.x, g.y);
        let k = self.pixels_per_metre(g.y);
        let shift = -(m.x - self.center.x).round();
        // Where the origin goes, and where its east, north and up axes point
        let mut origin = self.position(m, shift);
        let (mut e, mut n, mut u) = (DVec3::X, DVec3::NEG_Y, DVec3::Z);
        if self.globe.factor > 0.0 {
            let [east, north, up] = geodesy::enu(g.x, g.y);
            let local = |v: DVec3| DVec3::new(v.dot(self.globe.east), -v.dot(self.globe.north), v.dot(self.globe.up));
            let f = self.globe.factor;
            e = e.lerp(local(east), f);
            n = n.lerp(local(north), f);
            u = u.lerp(local(up), f);
        }
        origin += u * (g.z * k);
        DMat4::from_cols(
            (e * k).extend(0.0),
            (n * k).extend(0.0),
            (u * k).extend(0.0),
            origin.extend(1.0),
        )
    }

    /// Position in the local frame of an ECEF point
    #[must_use]
    pub fn ecef(&self, p: DVec3) -> DVec3 {
        self.geodetic(self.body.from_ecef(p))
    }

    /// Position in the local frame of a longitude, latitude (degrees) and ellipsoidal height (m): the origin of
    /// `ecef_matrix`, without the rest of the matrix
    #[must_use]
    pub fn geodetic(&self, g: DVec3) -> DVec3 {
        let m = mercator(g.x, g.y);
        let k = self.pixels_per_metre(g.y);
        let shift = -(m.x - self.center.x).round();
        let mut u = DVec3::Z;
        if self.globe.factor > 0.0 {
            let up = geodesy::enu(g.x, g.y)[2];
            let local = DVec3::new(
                up.dot(self.globe.east),
                -up.dot(self.globe.north),
                up.dot(self.globe.up),
            );
            u = u.lerp(local, self.globe.factor);
        }
        self.position(m, shift) + u * (g.z * k)
    }

    /// Whether a sphere (local frame, radius in pixels) lies wholly behind the globe as the camera sees it: inside the
    /// cone of the globe's shadow, beyond its horizon. Only on the sphere (`globe.factor` near 1), false otherwise.
    /// The occluder is the globe shrunk by 1 km, or by the depth of the body's lowest ground, for ground below the
    /// ellipsoid.
    #[must_use]
    pub fn behind_horizon(&self, center: DVec3, radius: f64) -> bool {
        if self.globe.factor < 0.999 {
            return false;
        }
        let r = self.globe.radius * (1.0 - (-self.body.relief.0).max(1e3) / self.body.radius);
        let to_globe = self.sphere_center() - self.eye;
        let d = to_globe.length();
        let to_tile = center - self.eye;
        let l = to_tile.length();
        if !(d > r && l > radius && radius.is_finite()) {
            return false;
        }
        // Beyond the plane of the horizon circle
        let beyond = (center - self.sphere_center()).dot(-to_globe / d) + radius < r * r / d;
        // Inside the tangent cone
        let cone = (r / d).asin();
        let angle = (to_tile.dot(to_globe) / (l * d)).clamp(-1.0, 1.0).acos();
        beyond && angle + (radius / l).asin() <= cone
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
            max_pitch: MAX_PITCH,
            globe: false,
            elevation: 0.0,
            body: Body::EARTH,
        }
    }

    #[must_use]
    pub fn world_size(&self) -> f64 {
        TILE_SIZE * self.zoom.exp2()
    }

    /// Center, zoom, pitch and bearing from the page: a value that is not a number leaves the camera's own (the
    /// center needs both coordinates), so a bad value never poisons the camera
    pub fn set_view(&mut self, lon: f64, lat: f64, zoom: f64, pitch: f64, bearing: f64) {
        if lon.is_finite() && lat.is_finite() {
            self.center = mercator(lon, lat);
        }
        for (field, value) in [
            (&mut self.zoom, zoom),
            (&mut self.pitch, pitch),
            (&mut self.bearing, bearing),
        ] {
            if value.is_finite() {
                *field = value;
            }
        }
        self.clamp();
    }

    /// Zoom range from the page: a bound that is not a number is left as it was, and the two are put in order
    pub fn set_zoom_range(&mut self, min: f64, max: f64) {
        let min = if min.is_finite() { min } else { self.min_zoom };
        let max = if max.is_finite() { max } else { self.max_zoom };
        (self.min_zoom, self.max_zoom) = (min.min(max), min.max(max));
        self.clamp();
    }

    /// Pitch limit from the page, degrees: up to `PITCH_LIMIT`, ignored if not a number
    pub fn set_max_pitch(&mut self, max: f64) {
        if max.is_finite() {
            self.max_pitch = max.clamp(0.0, PITCH_LIMIT);
        }
        self.clamp();
    }

    /// Brings the camera back into its limits. Never panics, whatever the fields hold: bounds out of order give way
    /// to the lower one, and a value that is not a number falls back to a limit (the zoom), 0 (pitch, bearing) or
    /// the middle of the world (center)
    #[allow(
        clippy::manual_clamp,
        reason = "`f64::clamp` panics on bounds out of order or not numbers"
    )]
    pub fn clamp(&mut self) {
        if !self.center.x.is_finite() {
            self.center.x = 0.5;
        }
        if !self.center.y.is_finite() {
            self.center.y = 0.5;
        }
        // On the globe, the zoom follows the latitude to keep the globe's size (`shift_center`): the floor is the
        // equator's, lowered by the same amount, so the whole globe can be seen from any latitude
        let floor = if self.globe {
            self.min_zoom + lon_lat(self.center).y.to_radians().cos().max(1e-3).log2()
        } else {
            self.min_zoom
        };
        // `max` then `min` rather than `clamp`, which panics on bounds out of order or not numbers; the zoom stays
        // finite even then
        let zoom = self.zoom.max(floor).min(self.max_zoom);
        self.zoom = if zoom.is_finite() { zoom } else { MIN_ZOOM };
        self.pitch = self.pitch.max(0.0).min(self.max_pitch.max(0.0).min(PITCH_LIMIT));
        if !self.bearing.is_finite() {
            self.bearing = 0.0;
        }
        self.bearing = (self.bearing + 540.0).rem_euclid(360.0) - 180.0;
        self.center.x = self.center.x.rem_euclid(1.0);
        let lat_limit = mercator(0.0, MAX_LAT).y;
        self.center.y = self.center.y.clamp(lat_limit, 1.0 - lat_limit);
    }

    /// Levels the blend into the plane is put off by under this camera's tilt: how much farther than its center the
    /// camera sees, the top edge of the view against the center (log2 of their distances on the plane, 0 when looking
    /// straight down). A view of 1000 km at pitch 66 sees the ground 4,000 km away: on the plane, that far north is
    /// the Mercator's stretched polar caps, and the horizon a straight line.
    fn globe_tilt(&self) -> f64 {
        let pitch = self.pitch.to_radians();
        let top = pitch + FOV / 2.0;
        // The top edge at or past the horizon: the camera sees as far as the ground goes
        if top >= 89f64.to_radians() {
            return GLOBE_TILT_LEVELS;
        }
        let flat = 1.0 / (FOV / 2.0).cos();
        ((pitch.cos() / top.cos()) / flat).log2().clamp(0.0, GLOBE_TILT_LEVELS)
    }

    fn globe(&self) -> Globe {
        let factor = if self.globe {
            let zoom = self.zoom - self.globe_tilt();
            ((GLOBE_NONE - zoom) / (GLOBE_NONE - GLOBE_FULL)).clamp(0.0, 1.0)
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
        // (past the horizon, at `FAR_LIMIT` center distances)
        let mut far = (((PI / 2.0 - pitch).cos() * top_half + center_distance) * 1.01).min(center_distance * FAR_LIMIT);
        // …or past the whole sphere
        if globe.factor > 0.0 {
            far = far.max(center_distance + 2.0 * globe.radius);
        }
        // Valleys of the relief lie below the center: some margin
        if self.elevation != 0.0 {
            far *= 1.25;
        }
        let near = self.height / 50.0;
        let proj = glam::dcamera::rh::proj::opengl::perspective(FOV, self.width / self.height, near, far);
        // The center stands at its altitude: the local frame goes down under the camera
        let lat = (PI * (1.0 - 2.0 * self.center.y)).sinh().atan();
        let plane = self.world_size() / (2.0 * PI * self.body.radius * lat.cos().max(1e-6));
        let vertical = plane + (globe.radius / self.body.radius - plane) * globe.factor;
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
            far,
            lift,
            vertical,
            world_size: self.world_size(),
            center: self.center,
            globe,
            body: self.body,
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
        // Spheres: the antipode hidden, the center and a huge sphere around the antipode not
        let k = v.globe.radius / v.body.radius;
        assert!(v.behind_horizon(v.sphere(antipode), 1000.0 * k));
        assert!(!v.behind_horizon(v.sphere(v.center), 1000.0 * k));
        assert!(!v.behind_horizon(v.sphere(antipode), 8e6 * k));
        // The ECEF position without the matrix is the matrix's origin
        let p = geodesy::to_ecef(3.06, 50.63, 120.0);
        assert!((v.ecef(p) - v.ecef_matrix(p).w_axis.truncate()).length() < 1e-9);
    }

    #[test]
    fn deep_ground_of_the_body_is_not_hidden() {
        // A point 5 km under the sphere, where its horizon passes: inside the globe shrunk by 1 km, hidden; Mars's
        // ground goes 8 km deep (Hellas), its globe is shrunk by as much and the point shows
        let mut c = globe(1.5);
        c.body = Body::MARS;
        let deep = |c: &Camera| {
            let v = c.view();
            let (center, r) = (v.sphere_center(), v.globe.radius);
            let to_eye = v.eye - center;
            let d = to_eye.length();
            let u = to_eye / d;
            let w = u.cross(DVec3::X).normalize();
            let a = (r / d).acos();
            let p = center + (u * a.cos() + w * a.sin()) * r * (1.0 - 5e3 / v.body.radius);
            v.behind_horizon(p, 1e-6)
        };
        assert!(!deep(&c));
        c.body.relief.0 = -200.0;
        assert!(deep(&c));
    }

    #[test]
    fn globe_blends_into_the_plane() {
        assert!((globe(4.0).view().globe.factor - 1.0).abs() < 1e-12);
        assert!(globe(7.5).view().globe.factor.abs() < 1e-12);
        assert!(camera().view().globe.factor.abs() < 1e-12);
    }

    #[test]
    fn a_tilted_globe_stays_a_sphere_longer() {
        let tilted = |zoom: f64, pitch: f64| {
            let mut c = globe(zoom);
            c.max_pitch = PITCH_LIMIT;
            c.pitch = pitch;
            c.view().globe.factor
        };
        // Looking down, as before; Olympus Mons from the south at zoom 6.3, pitch 66: a sphere to its horizon
        assert!((tilted(6.0, 0.0) - 0.5).abs() < 1e-12);
        assert!((tilted(6.3, 66.0) - 1.0).abs() < 1e-12);
        // Never past zoom 10, whatever the tilt
        assert!(tilted(10.0, 85.0).abs() < 1e-12);
        assert!(tilted(9.5, 85.0) > 0.0);
        // The blend grows with the tilt
        let factors: Vec<f64> = (0..=85).step_by(5).map(|p| tilted(8.0, f64::from(p))).collect();
        assert!(factors.windows(2).all(|w| w[0] <= w[1]), "{factors:?}");
        assert!(tilted(7.0, 30.0) < tilted(7.0, 50.0));
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
        for body in [Body::EARTH, Body::MOON, Body::MARS] {
            for globe_zoom in [None, Some(3.0), Some(6.0)] {
                let mut c = camera();
                c.body = body;
                if let Some(z) = globe_zoom {
                    c.globe = true;
                    c.zoom = z;
                }
                let v = c.view();
                let center = lon_lat(c.center);
                let anchor = body.to_ecef(center.x, center.y, 0.0);
                assert!(v.ecef(anchor).length() < 1e-6, "anchor at the center");
                // Degrees of longitude for 300 m along the parallel
                let lon = center.x + 300.0 / (body.circumference() / 360.0 * center.y.to_radians().cos());
                let east = body.to_ecef(lon, center.y, 0.0);
                let through_matrix = v.ecef_matrix(anchor).transform_point3(east - anchor);
                let direct = v.position(mercator(lon, center.y), 0.0);
                let scale = v.pixels_per_metre(center.y) * 300.0;
                assert!(
                    (through_matrix - direct).length() < 0.02 * scale + 0.01,
                    "{body:?} {globe_zoom:?}: {through_matrix} vs {direct}"
                );
                // Up is up, scaled like the ground
                let high = v.ecef(body.to_ecef(center.x, center.y, 100.0));
                assert!(
                    (high.z - 100.0 * v.pixels_per_metre(center.y)).abs() < 1e-3 * high.z.abs().max(1.0),
                    "{body:?}: {high}"
                );
            }
        }
    }

    #[test]
    fn enu_frames_on_the_plane() {
        // At the center, on the ground: the origin of the local frame, east right, north up the screen (y south), the
        // scale of Web Mercator at that latitude
        let v = camera().view();
        let (lon, lat) = (2.424_573, 48.845_726);
        let m = v.enu_matrix(DVec3::new(lon, lat, 0.0));
        let k = 512.0 * 2f64.powi(14) / (2.0 * PI * 6_378_137.0 * lat.to_radians().cos());
        let expected = DMat4::from_cols(
            DVec4::new(k, 0.0, 0.0, 0.0),
            DVec4::new(0.0, -k, 0.0, 0.0),
            DVec4::new(0.0, 0.0, k, 0.0),
            DVec4::W,
        );
        assert!(m.abs_diff_eq(expected, 1e-9), "{m}");
        assert!((k - 0.317_9).abs() < 1e-3, "{k}");
        // 1 km north and 50 m up: where the Mercator plane puts that point, within the change of scale over 1 km
        let north = lat + 1000.0 / 111_210.0;
        let through = v
            .enu_matrix(DVec3::new(lon, lat, 50.0))
            .transform_point3(DVec3::new(0.0, 1000.0, 0.0));
        let direct = v.position(mercator(lon, north), 0.0) + DVec3::Z * 50.0 * k;
        assert!(
            (through - direct).length() < 0.002 * 1000.0 * k,
            "{through} vs {direct}"
        );
        // A point of another copy of the world is drawn on the one nearest to the center
        let wrapped = v.enu_matrix(DVec3::new(lon + 360.0, lat, 0.0));
        assert!(wrapped.w_axis.truncate().length() < 1e-6);
    }

    #[test]
    fn enu_frames_on_the_globe() {
        let v = globe(3.0).view();
        let k = v.globe.radius / v.body.radius;
        for (lon, lat) in [(2.424_573, 48.845_726), (30.0, 10.0), (-20.0, 60.0)] {
            let m = v.enu_matrix(DVec3::new(lon, lat, 500.0));
            let (e, n, u) = (m.x_axis.truncate(), m.y_axis.truncate(), m.z_axis.truncate());
            // A rigid frame scaled to the sphere: orthogonal axes of k pixels per metre, east × north = up once the local
            // frame's y (south) is flipped back, as the view does
            for axis in [e, n, u] {
                assert!((axis.length() / k - 1.0).abs() < 1e-9);
            }
            assert!(e.dot(n).abs() < 1e-9 * k * k && e.dot(u).abs() < 1e-9 * k * k);
            assert!((e.cross(n) / k + u).length() < 1e-9 * k);
            // Up is the sphere's radius there, the origin 500 m above the ground point
            let ground = v.sphere(mercator(lon, lat));
            let radial = (ground - v.sphere_center()).normalize();
            assert!((u / k - radial).length() < 1e-9, "{lon} {lat}");
            assert!((m.w_axis.truncate() - (ground + radial * 500.0 * k)).length() < 1e-6);
            // North points to the pole: towards +z of the body
            let pole = v.sphere(mercator(lon, 85.0));
            assert!(n.dot(pole - ground) > 0.0);
            // The origin is `geodetic`'s
            assert!((m.w_axis.truncate() - v.geodetic(DVec3::new(lon, lat, 500.0))).length() < 1e-6);
        }
    }

    #[test]
    fn enu_frames_stay_exact_in_f32() {
        // At zoom 22, the model-view-projection of a point 10 m east of the center, made in f64 and handed over in
        // f32 (as the custom layers get it), puts the point where f64 does, to a hundredth of a pixel
        let mut c = camera();
        c.zoom = 22.0;
        c.pitch = 60.0;
        let v = c.view();
        let at = DVec3::new(2.424_573 + 0.000_1, 48.845_726, 2.0);
        let mvp = v.view_proj * v.enu_matrix(at);
        let f32_mvp = mvp.as_mat4();
        let local = DVec3::new(3.0, -2.0, 1.5);
        let exact = mvp.project_point3(local);
        let rounded = f32_mvp.project_point3(local.as_vec3()).as_dvec3();
        let pixels = (exact - rounded).truncate() * DVec2::new(c.width, c.height) / 2.0;
        assert!(pixels.length() < 0.01, "{pixels}");
    }

    #[test]
    fn metres_follow_the_body() {
        // A zoom level is the same Web Mercator pyramid on any body: its pixels hold fewer metres of a smaller one
        for globe_on in [false, true] {
            let mut earth = camera();
            earth.globe = globe_on;
            earth.zoom = if globe_on { 3.0 } else { 14.0 };
            let mut moon = earth.clone();
            moon.body = Body::MOON;
            let (e, m) = (earth.view(), moon.view());
            let lat = lon_lat(earth.center).y;
            // The Earth's scale as before: Web Mercator's on the plane, the sphere's on the globe
            let expected = if globe_on {
                e.globe.radius / 6_378_137.0
            } else {
                e.world_size / (2.0 * PI * 6_378_137.0 * lat.to_radians().cos())
            };
            assert!((e.pixels_per_metre(lat) / expected - 1.0).abs() < 1e-12);
            assert!((e.vertical / expected - 1.0).abs() < 1e-12);
            // The Moon's, its radius over the Earth's larger; the globe itself the same size on screen
            let ratio = 6_378_137.0 / 1_737_400.0;
            assert!((m.pixels_per_metre(lat) / e.pixels_per_metre(lat) / ratio - 1.0).abs() < 1e-12);
            assert!((m.vertical / e.vertical / ratio - 1.0).abs() < 1e-12);
            assert!((m.globe.radius - e.globe.radius).abs() < 1e-9);
            assert_eq!(m.view_proj, e.view_proj);
        }
        // A camera over the relief stands the same number of metres higher: more pixels on the Moon
        let mut c = camera();
        c.elevation = 1000.0;
        let earth = c.view().lift;
        c.body = Body::MOON;
        assert!((c.view().lift / earth - 6_378_137.0 / 1_737_400.0).abs() < 1e-9);
    }

    #[test]
    fn whole_globe_fits_at_zoom_zero() {
        // Above the globe, the ground is missing
        let c = globe(0.0);
        let v = c.view();
        assert!(c.ground(&v, 600.0, 400.0).is_some());
        assert!(c.ground(&v, 5.0, 5.0).is_none());
    }

    #[test]
    fn pitch_past_the_horizon_keeps_a_finite_far_plane() {
        let mut c = camera();
        c.pitch = 80.0;
        c.clamp();
        assert_eq!(c.pitch, MAX_PITCH, "the default limit holds");
        c.max_pitch = 90.0;
        c.pitch = 89.0;
        c.clamp();
        assert_eq!(c.pitch, PITCH_LIMIT, "never past the hard limit");
        let view = c.view();
        assert!(view.far.is_finite() && view.far <= view.center_distance * FAR_LIMIT * 1.0001);
        // The top of the screen looks above the horizon: no ground there
        assert!(c.ground(&view, 600.0, 0.0).is_none());
        assert!(c.ground(&view, 600.0, 799.0).is_some());
    }

    #[test]
    fn limits_from_the_page_never_panic() {
        let mut c = camera();
        // A minimum above the maximum: put in order
        c.set_zoom_range(15.0, 14.0);
        assert_eq!((c.min_zoom, c.max_zoom), (14.0, 15.0));
        assert!((14.0..=15.0).contains(&c.zoom));
        // A bound that is not a number is left as it was
        c.set_zoom_range(3.0, f64::NAN);
        assert_eq!((c.min_zoom, c.max_zoom), (3.0, 15.0));
        c.set_zoom_range(f64::NEG_INFINITY, 18.0);
        assert_eq!((c.min_zoom, c.max_zoom), (3.0, 18.0));
        c.set_max_pitch(f64::NAN);
        assert_eq!(c.max_pitch, MAX_PITCH);
        c.set_max_pitch(200.0);
        assert_eq!(c.max_pitch, PITCH_LIMIT);
        c.set_max_pitch(-5.0);
        assert_eq!(c.max_pitch, 0.0);
        // Fields set directly, out of order or not numbers: clamp still holds
        c.min_zoom = 10.0;
        c.max_zoom = 2.0;
        c.max_pitch = f64::NAN;
        c.zoom = f64::NAN;
        c.pitch = f64::INFINITY;
        c.bearing = f64::NAN;
        c.center = DVec2::new(f64::NAN, f64::INFINITY);
        c.clamp();
        assert!(c.zoom.is_finite() && c.pitch.is_finite() && c.bearing.is_finite());
        assert!(c.center.is_finite());
        c.globe = true;
        c.min_zoom = f64::NAN;
        c.clamp();
        assert!(c.zoom.is_finite());
    }

    #[test]
    fn a_view_not_a_number_keeps_the_camera() {
        let mut c = camera();
        let before = c.clone();
        c.set_view(f64::NAN, 10.0, f64::NAN, f64::INFINITY, f64::NAN);
        assert_eq!(c.center, before.center);
        assert_eq!(
            (c.zoom, c.pitch, c.bearing),
            (before.zoom, before.pitch, before.bearing)
        );
        c.set_view(10.0, 20.0, 5.0, 30.0, 90.0);
        assert!((lon_lat(c.center) - DVec2::new(10.0, 20.0)).length() < 1e-9);
        assert_eq!((c.zoom, c.pitch, c.bearing), (5.0, 30.0, 90.0));
        assert!(c.view().view_proj.is_finite());
    }
}
