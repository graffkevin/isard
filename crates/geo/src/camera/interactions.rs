//! Interactions: panning and zooming at a screen point (the grabbed ground staying under the cursor, also on the
//! globe), and settling the orbited point on finer relief without moving the eye.

use super::{Camera, MAX_LAT, lon_lat, mercator};
use glam::DVec2;

impl Camera {
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
    /// distance, so nothing moves on screen. The slide is the plane's: on the globe it holds for the small rises of
    /// finer relief, the ground under the center being tangent to the sphere (a tilted globe keeps its curve to zoom
    /// 10, where mountains show). If the new ground would come too close to the eye, the altitude is just set.
    pub fn settle_elevation(&mut self, elevation: f64) {
        // A street-level eye stands on the ground: it rises and sinks with it, where it is
        if self.first_person.is_some() {
            self.elevation = elevation;

            return;
        }

        let delta = elevation - self.elevation;
        let view = self.view();
        let pitch = self.pitch.to_radians();

        if delta == 0.0 {
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
