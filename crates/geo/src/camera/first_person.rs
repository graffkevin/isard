//! First person: the view from an eye standing above the center of the camera (a street-level view), looking any
//! way, its near and far planes in metres.

use super::{Camera, View};
use glam::{DMat4, DVec3};
use std::f64::consts::PI;

/// An eye standing above the center of the camera, looking any way
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FirstPerson {
    /// Metres above the ground of the center (its `elevation`)
    pub height: f64,
    /// Vertical field of view, radians
    pub fov: f64,
}

/// Near plane of a first-person view, metres
const FIRST_PERSON_NEAR: f64 = 0.1;
/// Far plane of a first-person view, metres
const FIRST_PERSON_FAR: f64 = 30_000.0;

impl Camera {
    /// The view from an eye standing `eye.height` metres above the center, turned by `bearing` and `pitch` (0 straight
    /// down, 90 level, 180 straight up): the same local frame as around the center, the focal length in place of the
    /// distance to the center (the tiles measured from the eye as ever)
    pub(super) fn first_person_view(&self, eye: FirstPerson) -> View {
        let globe = self.globe();
        let fov = eye.fov.clamp(0.1, 2.5);
        let focal = 0.5 * self.height / (fov / 2.0).tan();
        let lat = (PI * (1.0 - 2.0 * self.center.y)).sinh().atan();
        let plane = self.world_size() / (2.0 * PI * self.body.radius * lat.cos().max(1e-6));
        let vertical = plane + (globe.radius / self.body.radius - plane) * globe.factor;
        let lift = self.elevation * vertical;
        let (near, far) = (FIRST_PERSON_NEAR * vertical, FIRST_PERSON_FAR * vertical);
        let proj = glam::dcamera::rh::proj::opengl::perspective(fov, self.width / self.height, near, far);
        let view = DMat4::from_rotation_x(-self.pitch.to_radians())
            * DMat4::from_rotation_z(self.bearing.to_radians())
            * DMat4::from_scale(DVec3::new(1.0, -1.0, 1.0))
            * DMat4::from_translation(DVec3::new(0.0, 0.0, -lift - eye.height * vertical));
        let view_proj = proj * view;

        View {
            view_proj,
            inverse: view_proj.inverse(),
            eye: view.inverse().transform_point3(DVec3::ZERO),
            center_distance: focal,
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
