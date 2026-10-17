//! Geodesy of the body the map is drawn on (the Earth's WGS 84 by default): body-centered coordinates (ECEF,
//! EPSG:4978 on the Earth, the frame of 3D Tiles) to longitude, latitude and height, and the local east-north-up axes.

use glam::DVec3;
use std::f64::consts::PI;

/// A celestial body: its ellipsoid of revolution (a sphere when not flattened), in metres
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Equatorial radius (semi-major axis)
    pub radius: f64,
    /// Flattening, 0 for a sphere
    pub flattening: f64,
}

impl Body {
    /// The WGS 84 ellipsoid
    pub const EARTH: Self = Self {
        radius: 6_378_137.0,
        flattening: 1.0 / 298.257_223_563,
    };
    /// The Moon's mean sphere (IAU)
    pub const MOON: Self = Self {
        radius: 1_737_400.0,
        flattening: 0.0,
    };

    /// Square of the first eccentricity
    fn e2(&self) -> f64 {
        self.flattening * (2.0 - self.flattening)
    }

    /// Length of the equator (m): the width of the Web Mercator world
    #[must_use]
    pub fn circumference(&self) -> f64 {
        2.0 * PI * self.radius
    }

    /// Longitude, latitude (degrees) and ellipsoidal height (m) to body-centered coordinates
    #[must_use]
    pub fn to_ecef(&self, lon: f64, lat: f64, h: f64) -> DVec3 {
        let (a, e2) = (self.radius, self.e2());
        let (lon, lat) = (lon.to_radians(), lat.to_radians());
        let n = a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        DVec3::new(
            (n + h) * lat.cos() * lon.cos(),
            (n + h) * lat.cos() * lon.sin(),
            (n * (1.0 - e2) + h) * lat.sin(),
        )
    }

    /// Body-centered coordinates to longitude, latitude (degrees) and ellipsoidal height (m): Bowring's method,
    /// refined three times (sub-millimetre near the surface)
    #[must_use]
    pub fn from_ecef(&self, p: DVec3) -> DVec3 {
        let (a, e2) = (self.radius, self.e2());
        let lon = p.y.atan2(p.x);
        let r = p.x.hypot(p.y);
        let mut lat = p.z.atan2(r * (1.0 - e2));
        for _ in 0..3 {
            let n = a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
            let h = r / lat.cos() - n;
            lat = p.z.atan2(r * (1.0 - e2 * n / (n + h)));
        }
        let n = a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        let h = if lat.cos().abs() > 1e-9 {
            r / lat.cos() - n
        } else {
            p.z.abs() - n * (1.0 - e2)
        };
        DVec3::new(lon.to_degrees(), lat.to_degrees(), h)
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::EARTH
    }
}

/// Longitude, latitude (degrees) and ellipsoidal height (m) to ECEF, on WGS 84
#[must_use]
pub fn to_ecef(lon: f64, lat: f64, h: f64) -> DVec3 {
    Body::EARTH.to_ecef(lon, lat, h)
}

/// ECEF to longitude, latitude (degrees) and ellipsoidal height (m), on WGS 84 (`Body::from_ecef`)
#[must_use]
pub fn from_ecef(p: DVec3) -> DVec3 {
    Body::EARTH.from_ecef(p)
}

/// East, north and up unit vectors (ECEF axes) at a longitude and latitude (degrees)
#[must_use]
pub fn enu(lon: f64, lat: f64) -> [DVec3; 3] {
    let (lon, lat) = (lon.to_radians(), lat.to_radians());
    [
        DVec3::new(-lon.sin(), lon.cos(), 0.0),
        DVec3::new(-lat.sin() * lon.cos(), -lat.sin() * lon.sin(), lat.cos()),
        DVec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for (lon, lat, h) in [
            (2.424_573, 48.845_726, 65.0),
            (3.06, 50.63, 20.0),
            (-120.0, -33.0, 4000.0),
            (0.0, 89.9, 0.0),
        ] {
            let back = from_ecef(to_ecef(lon, lat, h));
            assert!((back.x - lon).abs() < 1e-9 && (back.y - lat).abs() < 1e-9, "{back}");
            assert!((back.z - h).abs() < 1e-4, "{back}");
        }
    }

    #[test]
    fn lille_rtc_center_is_on_the_ground() {
        // RTC_CENTER of a tile of the Lille photomesh
        let g = from_ecef(DVec3::new(4_056_896.270_481, 202_047.449_822, 4_900_910.163_548));
        assert!(
            (g.x - 2.85).abs() < 0.1 && (g.y - 50.55).abs() < 0.1 && g.z.abs() < 2000.0,
            "{g}"
        );
    }

    #[test]
    fn up_is_the_ellipsoid_normal() {
        let [east, north, up] = enu(3.06, 50.63);
        let p = to_ecef(3.06, 50.63, 0.0);
        let q = to_ecef(3.06, 50.63, 100.0);
        assert!(((q - p).normalize() - up).length() < 1e-9);
        assert!(east.dot(north).abs() < 1e-12 && east.dot(up).abs() < 1e-12);
    }

    #[test]
    fn the_earth_is_wgs84() {
        assert_eq!(Body::default(), Body::EARTH);
        assert_eq!(Body::EARTH.radius, 6_378_137.0);
        assert_eq!(Body::EARTH.to_ecef(3.06, 50.63, 20.0), to_ecef(3.06, 50.63, 20.0));
        // The pole: the semi-minor axis
        let pole = Body::EARTH.to_ecef(0.0, 90.0, 0.0);
        assert!((pole.z - 6_356_752.314_245).abs() < 1e-3, "{pole}");
        assert!((Body::EARTH.circumference() - 40_075_016.686).abs() < 1e-3);
    }

    #[test]
    fn the_moon_is_a_sphere() {
        let moon = Body::MOON;
        for (lon, lat, h) in [
            (23.473, 0.674, 0.0),
            (-3.6, 26.1, 500.0),
            (180.0, -89.9, -2000.0),
            (0.0, 90.0, 0.0),
        ] {
            let p = moon.to_ecef(lon, lat, h);
            assert!((p.length() - (1_737_400.0 + h)).abs() < 1e-6, "{p}");
            let back = moon.from_ecef(p);
            assert!((back.y - lat).abs() < 1e-9 && (back.z - h).abs() < 1e-6, "{back}");
            if lat.abs() < 90.0 {
                assert!(
                    ((back.x - lon + 540.0).rem_euclid(360.0) - 180.0).abs() < 1e-9,
                    "{back}"
                );
            }
        }
        assert!((moon.circumference() - 10_916_406.152).abs() < 1e-3);
    }
}
