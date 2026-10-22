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
//! Frames of reference, from the page to the GPU:
//! - Geographic: longitude and latitude in degrees, heights in metres (`set_view`, `View::geodetic`).
//! - Mercator: x east and y south in [0, 1] over the world (`mercator`, `lon_lat`); the camera center and the tiles.
//! - Body: unit vectors from the body's center, x at lon 0 lat 0, y at lon 90° E, z the north pole (`unit`); ECEF in
//!   metres is the same axes scaled (`View::ecef`).
//! - Local frame of a view: pixels at the current zoom relative to the center, x east, y south, z up. The sphere
//!   touches the plane at the center, its own center at (0, 0, -radius); `Globe::east`, `north`, `up` turn body
//!   vectors into it. Altitudes are lifted by `View::vertical` pixels per metre.
//! - Eye space and clip space: `View::view_proj` (eye looking down -z, y up, OpenGL clip cube).
//! - Screen: CSS pixels from the top left of the viewport (`Camera::ray`, `ground`, `pan`, `zoom_at`).
//!
//! Submodules, one subject each:
//! - `frames`: Mercator and body coordinates, `View` and its frames of reference (local frame, ECEF, ENU).
//! - `globe`: the sphere of a view, its blend into the plane and how a tilt puts it off.
//! - `first_person`: the view from an eye standing above the center (a street-level view).
//! - `ground`: rays through screen points and where they meet the surface; the ground a point stands over.
//! - `interactions`: pan, zoom at a point, settling on finer relief.

mod first_person;
mod frames;
mod globe;
mod ground;
mod interactions;
#[cfg(test)]
mod tests;

pub use first_person::FirstPerson;
pub use frames::{View, lon_lat, mercator, unit};
pub use globe::Globe;

use crate::geodesy::Body;
use glam::{DMat4, DVec2, DVec3};
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
/// Default lower zoom bound (`Camera::min_zoom`); on the globe the floor is lowered with the latitude (`clamp`)
pub const MIN_ZOOM: f64 = 0.0;
/// Default upper zoom bound (`Camera::max_zoom`)
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

/// The map camera as the page sets it (`MapLibre` conventions): where it looks, how close and how tilted, the
/// viewport and the projection. `view` turns it into the matrices of a frame; `clamp` keeps it in its limits.
#[derive(Clone, Debug)]
pub struct Camera {
    /// Center in Mercator [0, 1]
    pub center: DVec2,
    /// Zoom level: the world is `TILE_SIZE * 2^zoom` CSS pixels wide
    pub zoom: f64,
    /// Degrees from the vertical
    pub pitch: f64,
    /// Degrees clockwise from the north
    pub bearing: f64,
    /// Viewport in CSS pixels
    pub width: f64,
    /// Viewport height, CSS pixels
    pub height: f64,
    /// Lowest zoom `clamp` allows (lowered with the latitude on the globe)
    pub min_zoom: f64,
    /// Highest zoom `clamp` allows
    pub max_zoom: f64,
    /// Degrees, up to `PITCH_LIMIT`
    pub max_pitch: f64,
    /// Globe projection (else Web Mercator)
    pub globe: bool,
    /// Altitude of the center (metres): the camera turns around the ground of the relief, not sea level
    pub elevation: f64,
    /// The body the map is drawn on: its size turns zoom levels into metres
    pub body: Body,
    /// Seen from an eye standing above the center instead of around it (a street-level view): `pitch` then from
    /// straight down (0) to straight up (180)
    pub first_person: Option<FirstPerson>,
}

impl Camera {
    /// A camera over a longitude and latitude (degrees) at a zoom, looking straight down north up, on the Earth in Web
    /// Mercator; the viewport is 1 × 1 until the page sets it
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
            first_person: None,
        }
    }

    /// Width of the world in CSS pixels at the current zoom
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
    #[expect(
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
        // A street-level eye looks any way but straight down or up; the camera around a point to its highest pitch
        self.pitch = if self.first_person.is_some() {
            self.pitch.max(1.0).min(179.0)
        } else {
            self.pitch.max(0.0).min(self.max_pitch.max(0.0).min(PITCH_LIMIT))
        };
        if !self.bearing.is_finite() {
            self.bearing = 0.0;
        }
        self.bearing = (self.bearing + 540.0).rem_euclid(360.0) - 180.0;
        self.center.x = self.center.x.rem_euclid(1.0);

        let lat_limit = mercator(0.0, MAX_LAT).y;

        self.center.y = self.center.y.clamp(lat_limit, 1.0 - lat_limit);
    }

    /// The matrices of a frame: around the center at `pitch` and `bearing`, the eye as far as gives the zoom's scale at
    /// the center (or from the first-person eye). Called whenever the camera moved
    #[must_use]
    pub fn view(&self) -> View {
        if let Some(eye) = self.first_person {
            return self.first_person_view(eye);
        }

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
}
