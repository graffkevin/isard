//! WGS 84 geodesy: Earth-centered coordinates (ECEF, EPSG:4978, the frame of 3D Tiles) to longitude, latitude and
//! height, and the local east-north-up axes.

use glam::DVec3;

/// WGS 84 semi-major axis (m)
pub const A: f64 = 6_378_137.0;
/// WGS 84 flattening
const F: f64 = 1.0 / 298.257_223_563;
const E2: f64 = F * (2.0 - F);

/// Longitude, latitude (degrees) and ellipsoidal height (m) to ECEF
#[must_use]
pub fn to_ecef(lon: f64, lat: f64, h: f64) -> DVec3 {
    let (lon, lat) = (lon.to_radians(), lat.to_radians());
    let n = A / (1.0 - E2 * lat.sin().powi(2)).sqrt();
    DVec3::new(
        (n + h) * lat.cos() * lon.cos(),
        (n + h) * lat.cos() * lon.sin(),
        (n * (1.0 - E2) + h) * lat.sin(),
    )
}

/// ECEF to longitude, latitude (degrees) and ellipsoidal height (m): Bowring's method, refined three times (sub-millimetre
/// near the surface)
#[must_use]
pub fn from_ecef(p: DVec3) -> DVec3 {
    let lon = p.y.atan2(p.x);
    let r = p.x.hypot(p.y);
    let mut lat = p.z.atan2(r * (1.0 - E2));
    for _ in 0..3 {
        let n = A / (1.0 - E2 * lat.sin().powi(2)).sqrt();
        let h = r / lat.cos() - n;
        lat = p.z.atan2(r * (1.0 - E2 * n / (n + h)));
    }
    let n = A / (1.0 - E2 * lat.sin().powi(2)).sqrt();
    let h = if lat.cos().abs() > 1e-9 {
        r / lat.cos() - n
    } else {
        p.z.abs() - n * (1.0 - E2)
    };
    DVec3::new(lon.to_degrees(), lat.to_degrees(), h)
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
}
