//! Height of the geoid above the WGS 84 ellipsoid over mainland France, from IGN's RAF20 grid (RGF93 v2b to
//! NGF-IGN69). The IGN's terrain model gives altitudes (NGF-IGN69) and 3D Tiles ellipsoidal heights: adding the geoid
//! height to the altitudes puts both on the same surface (about 45 m apart in France).
//!
//! `raf20.bin`: every fourth node of `fr_ign_RAF20.tif` (PROJ data, converted by IGN from `RAF20.tac`, Licence
//! Ouverte), in centimetres as little-endian `u16`, northern row first. Bilinear interpolation of it stays within
//! 0.33 m of the full grid.

/// Grid nodes, northern row first
const GRID: &[u8; COLUMNS * ROWS * 2] = include_bytes!("raf20.bin");
const COLUMNS: usize = 106;
const ROWS: usize = 96;
const WEST: f64 = -5.5;
const NORTH: f64 = 51.5;
/// Node spacing in degrees (8' of longitude, 6' of latitude)
const STEP_LON: f64 = 2.0 / 15.0;
const STEP_LAT: f64 = 0.1;
/// Beyond the grid, its edge value holds this far (degrees: Corsica's south, the coasts)
const MARGIN: f64 = 1.0;
/// Then fades smoothly to 0 over this distance (degrees): no step in the surface at the border of the grid
const FADE: f64 = 3.0;

fn node(column: usize, row: usize) -> f64 {
    let i = (row * COLUMNS + column) * 2;

    f64::from(u16::from_le_bytes([GRID[i], GRID[i + 1]])) / 100.0
}

/// Geoid height (m) above the ellipsoid at a longitude and latitude (degrees). Outside the grid, the value of its
/// nearest edge, held over `MARGIN` degrees then faded to 0 over `FADE` degrees (continuous everywhere); 0 away from
/// mainland France and for non-finite input
#[must_use]
pub fn height(lon: f64, lat: f64) -> f64 {
    if !lon.is_finite() || !lat.is_finite() {
        return 0.0;
    }

    let x = (lon - WEST) / STEP_LON;
    let y = (NORTH - lat) / STEP_LAT;
    let (last_x, last_y) = ((COLUMNS - 1) as f64, (ROWS - 1) as f64);
    // Distance outside the grid (degrees, the larger of both axes)
    let outside = f64::max(
        (-x).max(x - last_x).max(0.0) * STEP_LON,
        (-y).max(y - last_y).max(0.0) * STEP_LAT,
    );

    if outside >= MARGIN + FADE {
        return 0.0;
    }

    let t = ((outside - MARGIN) / FADE).clamp(0.0, 1.0);
    let weight = 1.0 - t * t * (3.0 - 2.0 * t);
    let (x, y) = (x.clamp(0.0, last_x), y.clamp(0.0, last_y));
    // Cell corner, kept one node inside so that its right and lower neighbours exist
    let (c, r) = ((x as usize).min(COLUMNS - 2), (y as usize).min(ROWS - 2));
    let (tx, ty) = (x - c as f64, y - r as f64);
    let top = node(c, r) * (1.0 - tx) + node(c + 1, r) * tx;
    let bottom = node(c, r + 1) * (1.0 - tx) + node(c + 1, r + 1) * tx;

    (top * (1.0 - ty) + bottom * ty) * weight
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_full_grid() {
        // Bilinear values of the full RAF20 grid
        for (lon, lat, expected) in [(3.06, 50.63, 43.944), (6.87, 45.92, 52.897)] {
            assert!(
                (height(lon, lat) - expected).abs() < 0.33,
                "{lon} {lat}: {}",
                height(lon, lat)
            );
        }
    }

    #[test]
    fn edges() {
        // Grid corners, its edge held within the margin, nothing far away
        assert!((height(WEST, NORTH) - 53.87).abs() < 0.01);
        assert!((height(9.0, 41.5) - height(8.5, 42.0)).abs() < 1e-9);
        assert!(height(-61.5, 16.2).abs() < f64::EPSILON);
        assert!(height(f64::NAN, 45.0).abs() < f64::EPSILON && height(2.0, f64::INFINITY).abs() < f64::EPSILON);
    }

    #[test]
    fn continuous_everywhere() {
        // Small steps along lines crossing the grid, its margin and the fade: no jump larger than the slope allows
        for lat in [30.0, 38.5, 41.0, 45.0, 51.5, 52.0, 56.0] {
            let mut previous = height(-15.0, lat);

            for i in 1..=3000 {
                let h = height(-15.0 + f64::from(i) * 0.01, lat);

                assert!((h - previous).abs() < 0.5, "{lat}: step {}", h - previous);
                previous = h;
            }
        }

        for lon in [-12.0, -6.0, 0.0, 8.5, 10.0, 13.0] {
            let mut previous = height(lon, 34.0);

            for i in 1..=2400 {
                let h = height(lon, 34.0 + f64::from(i) * 0.01);

                assert!((h - previous).abs() < 0.5, "{lon}: step {}", h - previous);
                previous = h;
            }
        }
    }
}
