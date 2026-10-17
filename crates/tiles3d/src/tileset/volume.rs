//! The bounding volumes of tiles (sphere, oriented box, region) and the bounding sphere the traversal uses; the
//! subdivision of a box or region for implicit tiling.

use super::implicit::Coord;
use super::json::VolumeJson;
use geo::geodesy;
use glam::{DMat4, DVec3};
use std::f64::consts::TAU;

/// A bounding volume as the tileset gives it
#[derive(Clone, Copy, Debug)]
pub(super) enum Volume {
    Sphere([f64; 4]),
    Box([f64; 12]),
    /// Radians and metres, in ECEF already
    Region([f64; 6]),
}

impl Volume {
    pub(super) fn parse(v: &VolumeJson) -> Result<Self, String> {
        match (v.sphere, v.oriented_box, v.region) {
            (Some(s), _, _) => Ok(Self::Sphere(s)),
            (_, Some(b), _) => Ok(Self::Box(b)),
            (_, _, Some(r)) => Ok(Self::Region(r)),
            _ => Err("bounding volume without sphere, box or region".into()),
        }
    }

    /// Bounding sphere (ECEF)
    pub(super) fn sphere(&self, transform: &DMat4) -> Result<(DVec3, f64), String> {
        let scale = transform
            .x_axis
            .truncate()
            .length()
            .max(transform.y_axis.truncate().length())
            .max(transform.z_axis.truncate().length());
        let (center, radius) = match *self {
            Self::Sphere([x, y, z, r]) => (transform.transform_point3(DVec3::new(x, y, z)), r * scale),
            Self::Box(b) => {
                let half = DVec3::new(b[3], b[4], b[5]).length_squared()
                    + DVec3::new(b[6], b[7], b[8]).length_squared()
                    + DVec3::new(b[9], b[10], b[11]).length_squared();
                (
                    transform.transform_point3(DVec3::new(b[0], b[1], b[2])),
                    half.sqrt() * scale,
                )
            }
            Self::Region([west, south, east, north, low, high]) => {
                // Regions are in ECEF already (no transform); sample the curved box
                let east = if east < west { east + TAU } else { east };
                let (lon, lat) = (f64::midpoint(west, east), f64::midpoint(south, north));
                let center = geodesy::to_ecef(lon.to_degrees(), lat.to_degrees(), f64::midpoint(low, high));
                let mut radius = 0f64;
                for i in 0..=4 {
                    let x = west + (east - west) * f64::from(i) / 4.0;
                    for j in 0..=4 {
                        let y = south + (north - south) * f64::from(j) / 4.0;
                        for h in [low, high] {
                            let p = geodesy::to_ecef(x.to_degrees(), y.to_degrees(), h);
                            radius = radius.max(p.distance(center));
                        }
                    }
                }
                (center, radius)
            }
        };
        if center.is_finite() && radius.is_finite() && radius >= 0.0 {
            Ok((center, radius))
        } else {
            Err("bad bounding volume".into())
        }
    }

    /// Volume of the implicit tile `t` of a root volume (box or region), halved along x and y (and z for an octree)
    /// at each level
    pub(super) fn subdivide(&self, t: Coord, octree: bool) -> Self {
        let d = f64::from(t.level).exp2();
        // Center of index `i` along a halved axis, in -1..1
        let f = |i: u32| (2.0 * f64::from(i) + 1.0) / d - 1.0;
        match *self {
            Self::Box(b) => {
                let axis = |k: usize| DVec3::new(b[k], b[k + 1], b[k + 2]);
                let (x, y, z) = (axis(3), axis(6), axis(9));
                let c = axis(0) + x * f(t.x) + y * f(t.y) + if octree { z * f(t.z) } else { DVec3::ZERO };
                let z = if octree { z / d } else { z };
                let (x, y) = (x / d, y / d);
                Self::Box([c.x, c.y, c.z, x.x, x.y, x.z, y.x, y.y, y.z, z.x, z.y, z.z])
            }
            Self::Region([west, south, east, north, low, high]) => {
                let east = if east < west { east + TAU } else { east };
                let part = |lo: f64, hi: f64, i: u32| {
                    let step = (hi - lo) / d;
                    (lo + step * f64::from(i), lo + step * (f64::from(i) + 1.0))
                };
                let (w, e) = part(west, east, t.x);
                let (s, n) = part(south, north, t.y);
                let (l, h) = if octree { part(low, high, t.z) } else { (low, high) };
                Self::Region([w, s, e, n, l, h])
            }
            Self::Sphere(_) => *self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_and_box_volumes_bound_their_tiles() {
        let (w, s, e, n) = (
            3.0f64.to_radians(),
            50.58f64.to_radians(),
            3.12f64.to_radians(),
            50.68f64.to_radians(),
        );
        let region = Volume::Region([w, s, e, n, 0.0, 200.0]);
        let (center, radius) = region.sphere(&DMat4::IDENTITY).unwrap();
        for lon in [w, e] {
            for lat in [s, n] {
                for h in [0.0, 200.0] {
                    let p = geodesy::to_ecef(lon.to_degrees(), lat.to_degrees(), h);
                    assert!(p.distance(center) <= radius + 1e-6);
                }
            }
        }
        assert!(radius < 7000.0, "{radius}");
        // A box 2 km × 2 km × 100 m, moved by its transform
        let c = geodesy::to_ecef(3.06, 50.63, 0.0);
        let [east, north, up] = geodesy::enu(3.06, 50.63);
        let (x, y, z) = (east * 1000.0, north * 1000.0, up * 50.0);
        let b = Volume::Box([0.0, 0.0, 0.0, x.x, x.y, x.z, y.x, y.y, y.z, z.x, z.y, z.z]);
        let (center, radius) = b.sphere(&DMat4::from_translation(c)).unwrap();
        assert!(center.distance(c) < 1e-6 && (radius - (2e6f64 + 2500.0).sqrt()).abs() < 1e-6);
        // Implicit subdivision: the quadtree tile (1, 1, 0) is the east-south quarter
        let Volume::Box(q) = b.subdivide(
            Coord {
                level: 1,
                x: 1,
                y: 0,
                z: 0,
            },
            false,
        ) else {
            panic!()
        };
        let qc = DVec3::new(q[0], q[1], q[2]);
        assert!((qc - (x * 0.5 - y * 0.5)).length() < 1e-9);
        assert!((DVec3::new(q[3], q[4], q[5]) - x * 0.5).length() < 1e-9);
        assert!(
            (DVec3::new(q[9], q[10], q[11]) - z).length() < 1e-9,
            "z kept by a quadtree"
        );
        let Volume::Region(r) = region.subdivide(
            Coord {
                level: 2,
                x: 3,
                y: 0,
                z: 1,
            },
            true,
        ) else {
            panic!()
        };
        assert!(
            (r[0] - (w + (e - w) * 0.75)).abs() < 1e-12 && (r[4] - 50.0).abs() < 1e-9 && (r[5] - 100.0).abs() < 1e-9
        );
        // Bad volumes are errors, not NaN spheres
        assert!(
            Volume::Sphere([f64::NAN, 0.0, 0.0, 1.0])
                .sphere(&DMat4::IDENTITY)
                .is_err()
        );
    }
}
