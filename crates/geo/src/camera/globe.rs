//! The globe projection of a camera: the sphere, its radius and local axes at the center, and how far the view is
//! blended into the Mercator plane (put off by a tilted camera, which sees much farther than its center).

use super::{Camera, FOV, GLOBE_FULL, GLOBE_NONE, GLOBE_TILT_LEVELS, lon_lat};
use glam::DVec3;
use std::f64::consts::PI;

/// Sphere of a view, in the local frame
#[derive(Clone, Copy, Debug)]
pub struct Globe {
    /// 1 = sphere, 0 = Mercator plane
    pub factor: f64,
    /// Pixels
    pub radius: f64,
    /// Local axes at the center, in body coordinates (`unit`): east…
    pub east: DVec3,
    /// …north (the local frame's -y)…
    pub north: DVec3,
    /// …and up, the unit vector of the center
    pub up: DVec3,
}

impl Camera {
    /// Levels the blend into the plane is put off by under this camera's tilt: how much farther than its center the
    /// camera sees, the top edge of the view against the center (log2 of their distances on the plane, 0 when looking
    /// straight down). A view of 1000 km at pitch 66 sees the ground 4,000 km away: on the plane, that far north is
    /// the Mercator's stretched polar caps, and the horizon a straight line.
    pub(super) fn globe_tilt(&self) -> f64 {
        let pitch = self.pitch.to_radians();
        let top = pitch + FOV / 2.0;

        // The top edge at or past the horizon: the camera sees as far as the ground goes (the ratio below would grow
        // without bound, then turn negative and the log NaN)
        if top.cos() <= 1e-9 {
            return GLOBE_TILT_LEVELS;
        }

        let flat = 1.0 / (FOV / 2.0).cos();

        ((pitch.cos() / top.cos()) / flat).log2().clamp(0.0, GLOBE_TILT_LEVELS)
    }

    /// The sphere of this camera: its blend with the plane (zoom put off by the tilt), its radius in pixels and the
    /// local axes at the center
    pub(super) fn globe(&self) -> Globe {
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
}
