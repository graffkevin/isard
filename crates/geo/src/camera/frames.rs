//! Where a point is, in each frame of reference of the camera: Mercator and the body's unit vectors, then a `View`'s
//! local frame (pixels around the center), ECEF and east-north-up frames placed in it, and what the horizon hides.

use super::{Globe, MAX_LAT};
use crate::geodesy::{self, Body};
use glam::{DMat3, DMat4, DVec2, DVec3};
use std::f64::consts::PI;

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

/// Matrices of one frame, derived from the camera
#[derive(Clone, Debug)]
pub struct View {
    /// Local frame to clip space
    pub view_proj: DMat4,
    /// Clip space back to the local frame (unprojecting screen points)
    pub inverse: DMat4,
    /// Camera position in the local frame
    pub eye: DVec3,
    /// Distance from the camera to the center, in pixels
    pub center_distance: f64,
    /// Near plane distance, in pixels
    pub near: f64,
    /// Far plane distance, in pixels
    pub far: f64,
    /// Altitude of the center in the local frame (pixels): the plane (or sphere) the camera turns around
    pub lift: f64,
    /// Pixels of the local frame per metre of altitude
    pub vertical: f64,
    /// Width of the world in pixels at the camera's zoom (`Camera::world_size`)
    pub world_size: f64,
    /// Camera center, Mercator: the origin of the local frame
    pub center: DVec2,
    /// The sphere and how far the view is blended into it
    pub globe: Globe,
    /// The body of the camera
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

    /// Position in the local frame of a Mercator point `h` pixels above the ground, on the world's copy nearest to
    /// the center: up the plane's vertical and the sphere's normal, blended as the layers draw it (`PROJECT`)
    #[must_use]
    pub fn surface(&self, m: DVec2, h: f64) -> DVec3 {
        let shift = -(m.x - self.center.x).round();
        let flat = DVec3::new(
            (m.x + shift - self.center.x) * self.world_size,
            (m.y - self.center.y) * self.world_size,
            h,
        );

        if self.globe.factor <= 0.0 {
            return flat;
        }

        let sphere = self.sphere(m);
        let normal = sphere / self.globe.radius + DVec3::Z;

        flat.lerp(sphere + normal * h, self.globe.factor)
    }

    /// Position on the sphere (local frame, pixels) of a Mercator point, whatever the blend: the body vector turned
    /// into the local axes, scaled by the radius, the center at the origin
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
