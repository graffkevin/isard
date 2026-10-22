//! Ground queries: the ray through a screen point, where it meets the surface at an altitude (on the plane, the
//! sphere, or their blend by Newton's method), and the ground point a point of the local frame stands over.

use super::{Camera, View};
use glam::{DMat3, DVec2, DVec3, DVec4};

/// Root of `f` (3 unknowns, 3 equations in pixels) from `start` by Newton's method, the Jacobian by finite steps of
/// `step` (about a pixel's worth of each unknown); None if it does not come within a thousandth of a pixel
fn newton(start: DVec3, step: DVec3, f: impl Fn(DVec3) -> DVec3) -> Option<DVec3> {
    let mut u = start;

    for _ in 0..8 {
        let e = f(u);

        if e.length() < 1e-3 {
            return Some(u);
        }

        let column = |axis: DVec3| (f(u + axis) - e) / axis.length();
        let jacobian = DMat3::from_cols(
            column(DVec3::X * step.x),
            column(DVec3::Y * step.y),
            column(DVec3::Z * step.z),
        );

        if jacobian.determinant().abs() < 1e-300 {
            return None;
        }
        u -= jacobian.inverse() * e;

        if !u.is_finite() {
            return None;
        }
    }

    (f(u).length() < 1e-3).then_some(u)
}

impl Camera {
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
        let f = view.globe.factor;
        let sphere = || {
            let (d, c, r) = (b - a, view.sphere_center(), view.globe.radius + h);
            let (qa, qb, qc) = (d.dot(d), 2.0 * d.dot(a - c), (a - c).length_squared() - r * r);
            let disc = qb * qb - 4.0 * qa * qc;

            if disc < 0.0 {
                return None;
            }

            let t = (-qb - disc.sqrt()) / (2.0 * qa);

            (t >= 0.0).then(|| view.sphere_to_mercator(a + d * t))
        };
        let plane = || {
            let t = (a.z - h) / (a.z - b.z);

            if !(0.0..=1.0).contains(&t) || b.z >= a.z {
                return None;
            }

            let p = a + (b - a) * t;

            Some(self.center + DVec2::new(p.x, p.y) / view.world_size)
        };

        if f >= 1.0 {
            return sphere();
        }
        if f <= 0.0 {
            return plane();
        }

        // While blending, the surface drawn lies between the two: from the hit on the nearer one, the point of the
        // blended surface on the ray (unknowns: the point and how far along the ray)
        let start = if f >= 0.5 {
            sphere().or_else(plane)
        } else {
            plane().or_else(sphere)
        }?;
        let d = b - a;
        let t = (view.surface(start, h) - a).dot(d) / d.length_squared();
        let scale = DVec3::new(1.0 / view.world_size, 1.0 / view.world_size, 1.0 / d.length());
        let solved = newton(DVec3::new(start.x, start.y, t), scale, |u| {
            view.surface(DVec2::new(u.x, u.y), h) - (a + d * u.z)
        });

        if let Some(u) = solved.filter(|u| u.z >= 0.0) {
            return Some(DVec2::new(u.x, u.y));
        }

        // Not converged (the ray grazing the surface, or passing over it)
        Self::ground_by_steps(view, a, d, altitude)
    }

    /// The first point (Mercator) of the ray `a + d * t`, `t` in 0..1 (local frame), under the surface at `altitude`
    /// (metres): found by steps, then halved; None if the ray never goes under. The fallback of `ground_at` when
    /// Newton's method does not converge on the blended surface
    fn ground_by_steps(view: &View, a: DVec3, d: DVec3, altitude: f64) -> Option<DVec2> {
        let under = |t: f64| Self::stand(view, a + d * t).1 <= altitude;
        // Closer together near the eye, where the ground is seen
        let steps = 64;
        let mut far = (1..=steps)
            .map(|k| (f64::from(k) / f64::from(steps)).powi(2))
            .find(|&t| under(t))?;
        let mut near = far - (2.0 * far.sqrt() - 1.0 / f64::from(steps)) / f64::from(steps);

        for _ in 0..50 {
            let mid = f64::midpoint(near, far);

            if under(mid) {
                far = mid;
            } else {
                near = mid;
            }
        }

        Some(Self::stand(view, a + d * far).0)
    }

    /// Altitude (metres, exaggeration included) of a point of the local frame
    #[must_use]
    pub fn altitude_of(view: &View, p: DVec3) -> f64 {
        Self::stand(view, p).1
    }

    /// The ground point (Mercator) a point of the local frame stands over, and its altitude above it (metres,
    /// exaggeration included): up the sphere's normal, the plane's vertical, or while blending their blend
    fn stand(view: &View, p: DVec3) -> (DVec2, f64) {
        let f = view.globe.factor;
        let sphere = || {
            (
                view.sphere_to_mercator(p),
                (p.distance(view.sphere_center()) - view.globe.radius) / view.vertical,
            )
        };
        let plane = || {
            (
                view.center + DVec2::new(p.x, p.y) / view.world_size,
                p.z / view.vertical,
            )
        };

        if f >= 1.0 {
            return sphere();
        }
        if f <= 0.0 {
            return plane();
        }

        // While blending, from the nearer one (unknowns: the ground point and the height in pixels)
        let (start, h) = if f >= 0.5 { sphere() } else { plane() };
        let scale = DVec3::new(1.0 / view.world_size, 1.0 / view.world_size, 1.0);

        newton(DVec3::new(start.x, start.y, h * view.vertical), scale, |u| {
            view.surface(DVec2::new(u.x, u.y), u.z) - p
        })
        .map_or((start, h), |u| (DVec2::new(u.x, u.y), u.z / view.vertical))
    }
}
