//! What arrives in a tile, made ready for the pool: the altitudes cleaned of the server's no-data and raised to the
//! ellipsoid, their coarse copy for the CPU; the URL of a tile; the depth read back by the picking. Pure functions,
//! tested natively.

use super::{CPU_HEIGHTS, NO_DATA, TILE};
use geo::camera::lon_lat;
use geo::geoid;
use geo::tiles::TileId;

/// Depth in [0, 1] from an RGBA8 pixel of `shaders::DEPTH_FRAGMENT`
#[must_use]
pub fn unpack_depth(pixel: [u8; 4]) -> f64 {
    let [r, g, b, _] = pixel.map(|c| f64::from(c) / 255.0);
    r + g / 255.0 + b / 65025.0
}

/// The URL of a tile from a template: `{z}`, `{x}`, `{y}` or `{bbox}`
pub fn tile_url(template: &str, tile: TileId) -> String {
    template
        .replace("{z}", &tile.z.to_string())
        .replace("{x}", &tile.x.to_string())
        .replace("{y}", &tile.y.to_string())
        .replace("{bbox}", &bbox(tile))
}

/// Box of a tile in EPSG:3857 metres, `west,south,east,north`, for a WMS `GetMap`
pub fn bbox(tile: TileId) -> String {
    const HALF: f64 = 20_037_508.342_789_244;
    let (min, max) = tile.bounds();
    let x = |m: f64| -HALF + m * 2.0 * HALF;
    let y = |m: f64| HALF - m * 2.0 * HALF;
    format!("{},{},{},{}", x(min.x), y(max.y), x(max.x), y(min.y))
}

/// No-data (outside the coverage) takes the lowest altitude of the tile: neither pit nor wall at the coast; 0 if the
/// whole tile is no-data
pub fn fill_no_data(heights: &mut [f32]) {
    let lowest = heights
        .iter()
        .copied()
        .filter(|&h| h > NO_DATA && h.is_finite())
        .fold(f32::INFINITY, f32::min);
    let fill = if lowest.is_finite() { lowest } else { 0.0 };
    for h in heights {
        if *h <= NO_DATA || !h.is_finite() {
            *h = fill;
        }
    }
}

/// Altitudes of a tile to heights above the ellipsoid, those of 3D Tiles: plus the geoid height at each pixel center
pub fn to_ellipsoid(tile: TileId, heights: &mut [f32]) {
    let (min, max) = tile.bounds();
    let size = TILE as usize;
    let at = |k: usize, lo: f64, hi: f64| lo + (hi - lo) * (k as f64 + 0.5) / size as f64;
    let lons: Vec<f64> = (0..size)
        .map(|i| lon_lat(glam::DVec2::new(at(i, min.x, max.x), 0.5)).x)
        .collect();
    for (row, line) in heights.chunks_exact_mut(size).enumerate() {
        let lat = lon_lat(glam::DVec2::new(0.5, at(row, min.y, max.y))).y;
        for (h, &lon) in line.iter_mut().zip(&lons) {
            *h += geoid::height(lon, lat) as f32;
        }
    }
}

/// `CPU_HEIGHTS`² altitudes out of a full 256 × 256 grid: the mean of each block
pub fn subsample(heights: &[f32]) -> Box<[f32]> {
    let block = TILE as usize / CPU_HEIGHTS;
    let mut out = vec![0f32; CPU_HEIGHTS * CPU_HEIGHTS];
    for (k, value) in out.iter_mut().enumerate() {
        let (bx, by) = (k % CPU_HEIGHTS * block, k / CPU_HEIGHTS * block);
        let sum: f32 = (0..block)
            .flat_map(|y| (0..block).map(move |x| heights[(by + y) * TILE as usize + bx + x]))
            .sum();
        *value = sum / (block * block) as f32;
    }
    out.into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_round_trips_through_the_packing() {
        // The shader's packing, in f64
        let pack = |z: f64| {
            let e = [z, z * 255.0, z * 65025.0].map(f64::fract);
            let r = e[0] - e[1] / 255.0;
            let g = e[1] - e[2] / 255.0;
            [r, g, e[2], 1.0].map(|c| (c * 255.0).round() as u8)
        };
        for z in [0.0, 0.25, 0.5, 0.731, 0.999] {
            assert!((unpack_depth(pack(z)) - z).abs() < 1e-5, "{z}");
        }
        assert_eq!(unpack_depth([0, 0, 0, 255]), 0.0);
    }

    #[test]
    fn urls_fill_the_template() {
        let t = TileId::new(3, 4, 2);
        assert_eq!(tile_url("https://t/{z}/{x}/{y}.png", t), "https://t/3/4/2.png");
        // The whole world at level 0
        let b = bbox(TileId::new(0, 0, 0));
        let v: Vec<f64> = b.split(',').map(|s| s.parse().unwrap()).collect();
        assert!((v[0] + 20_037_508.342_789_244).abs() < 1e-6 && (v[3] - 20_037_508.342_789_244).abs() < 1e-6);
        assert!(v[0] < v[2] && v[1] < v[3]);
    }

    #[test]
    fn no_data_takes_the_lowest_altitude() {
        let mut h = vec![12.0, -99_999.0, 5.0, f32::NAN, 30.0];
        fill_no_data(&mut h);
        assert_eq!(h, vec![12.0, 5.0, 5.0, 5.0, 30.0]);
        let mut all = vec![-99_999.0; 4];
        fill_no_data(&mut all);
        assert_eq!(all, vec![0.0; 4]);
    }

    #[test]
    fn subsampling_averages_blocks() {
        let size = TILE as usize;
        let heights: Vec<f32> = (0..size * size).map(|k| (k % size / 8) as f32).collect();
        let s = subsample(&heights);
        assert_eq!(s.len(), CPU_HEIGHTS * CPU_HEIGHTS);
        // Columns of 8 pixels of the same value: each block keeps it
        assert_eq!(s[0], 0.0);
        assert_eq!(s[5], 5.0);
        assert_eq!(s[CPU_HEIGHTS + 7], 7.0);
    }

    #[test]
    fn ellipsoid_heights_add_the_geoid() {
        // A tile over Paris: the geoid is some 40-50 m above the ellipsoid there
        let n = 1u32 << 10;
        let m = geo::camera::mercator(2.35, 48.85);
        let tile = TileId::new(10, (m.x * f64::from(n)) as u32, (m.y * f64::from(n)) as u32);
        let mut h = vec![0f32; (TILE * TILE) as usize];
        to_ellipsoid(tile, &mut h);
        assert!(h.iter().all(|&v| v > 30.0 && v < 60.0), "{}", h[0]);
    }
}
