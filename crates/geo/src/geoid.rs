//! Height of the geoid above the WGS 84 ellipsoid over mainland France, from IGN's RAF20 grid (RGF93 v2b to
//! NGF-IGN69). The terrain model gives altitudes (NGF-IGN69) and 3D Tiles ellipsoidal heights: adding the geoid
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
/// Beyond the grid, its edge value holds this far (Corsica's south, the coasts) before the geoid height drops to 0
const MARGIN: f64 = 1.0;

fn node(column: usize, row: usize) -> f64 {
    let i = (row * COLUMNS + column) * 2;
    f64::from(u16::from_le_bytes([GRID[i], GRID[i + 1]])) / 100.0
}

/// Geoid height (m) above the ellipsoid at a longitude and latitude (degrees); 0 away from mainland France
#[must_use]
pub fn height(lon: f64, lat: f64) -> f64 {
    let x = (lon - WEST) / STEP_LON;
    let y = (NORTH - lat) / STEP_LAT;
    let (last_x, last_y) = ((COLUMNS - 1) as f64, (ROWS - 1) as f64);
    if x < -MARGIN / STEP_LON
        || y < -MARGIN / STEP_LAT
        || x > last_x + MARGIN / STEP_LON
        || y > last_y + MARGIN / STEP_LAT
    {
        return 0.0;
    }
    let (x, y) = (x.clamp(0.0, last_x), y.clamp(0.0, last_y));
    // Cell corner, kept one node inside so that its right and lower neighbours exist
    let (c, r) = ((x as usize).min(COLUMNS - 2), (y as usize).min(ROWS - 2));
    let (tx, ty) = (x - c as f64, y - r as f64);
    let top = node(c, r) * (1.0 - tx) + node(c + 1, r) * tx;
    let bottom = node(c, r + 1) * (1.0 - tx) + node(c + 1, r + 1) * tx;
    top * (1.0 - ty) + bottom * ty
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
    }
}
