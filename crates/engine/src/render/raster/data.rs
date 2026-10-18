//! What arrives in a tile, made ready for the pool: the altitudes cleaned of the server's no-data and raised to the
//! ellipsoid, their coarse copy for the CPU; the URL of a tile; the depth read back by the picking. Pure functions,
//! tested natively.

use super::{CPU_HEIGHTS, TILE};
use geo::camera::lon_lat;
use geo::geodesy::Body;
use geo::geoid;
use geo::tiles::TileId;

/// Altitudes this far below the lowest ground of the body are the server's no-data (-99999 on the Géoplateforme,
/// -32768 in 16-bit rasters): under -1250 m on the Earth
const NO_DATA_MARGIN: f64 = 800.0;

/// Depth in [0, 1] from an RGBA8 pixel of `shaders::DEPTH_FRAGMENT`
#[must_use]
pub fn unpack_depth(pixel: [u8; 4]) -> f64 {
    let [r, g, b, _] = pixel.map(|c| f64::from(c) / 255.0);
    r + g / 255.0 + b / 65025.0
}

/// The URL of a tile from a template: `{z}`, `{x}`, `{y}` or `{bbox}` (in the Mercator metres of `body`)
pub fn tile_url(template: &str, tile: TileId, body: &Body) -> String {
    template
        .replace("{z}", &tile.z.to_string())
        .replace("{x}", &tile.x.to_string())
        .replace("{y}", &tile.y.to_string())
        .replace("{bbox}", &bbox(tile, body))
}

/// Box of a tile in the Mercator metres of the body (EPSG:3857 on the Earth), `west,south,east,north`, for a WMS
/// `GetMap` or an `ArcGIS` `exportImage`
pub fn bbox(tile: TileId, body: &Body) -> String {
    let half = body.half_world();
    let (min, max) = tile.bounds();
    let x = |m: f64| -half + m * 2.0 * half;
    let y = |m: f64| half - m * 2.0 * half;
    format!("{},{},{},{}", x(min.x), y(max.y), x(max.x), y(min.y))
}

/// Levels of the relief: coarser, it would not show; finer, even a 1 m grid (RGE ALTI) gains nothing at a 16 × 16 mesh
const TERRAIN_MIN_Z: u8 = 6;
const TERRAIN_MAX_Z: u8 = 15;

/// First and last levels of the relief of a body, `max_z` asked (the finest by default): from level 6 on the Earth,
/// as many levels coarser as the body's relief is larger for its size (it shows from further away: level 3 on the
/// Moon and Mars)
#[must_use]
pub fn terrain_levels(body: &Body, max_z: Option<u8>) -> (u8, u8) {
    let coarser = body.relief_scale().log2().round().clamp(0.0, f64::from(TERRAIN_MIN_Z));
    let min_z = TERRAIN_MIN_Z - coarser as u8;
    (min_z, max_z.unwrap_or(TERRAIN_MAX_Z).clamp(min_z, TERRAIN_MAX_Z))
}

/// Whether the altitudes of a relief are raised by the RAF20 geoid: when asked (not by default: the altitudes as
/// served), and only on the Earth, whose geoid it is
#[must_use]
pub fn terrain_geoid(geoid: Option<bool>, body: &Body) -> bool {
    geoid.unwrap_or(false) && body.is_earth()
}

/// Below this, an altitude of the body is no-data
#[must_use]
pub fn no_data(body: &Body) -> f32 {
    (body.relief.0 - NO_DATA_MARGIN) as f32
}

/// The 1-bit validity mask an `ArcGIS` image service appends to its raw grids (`format=bsq`), one bit per pixel, rows
/// from the north, most significant bit first: the pixels it marks invalid turn into no-data
pub fn apply_mask(heights: &mut [f32], mask: &[u8]) {
    for (k, h) in heights.iter_mut().enumerate() {
        if mask.get(k / 8).is_some_and(|byte| byte & (0x80 >> (k % 8)) == 0) {
            *h = f32::NAN;
        }
    }
}

/// No-data (outside the coverage, under `below`) takes the lowest altitude of the tile: neither pit nor wall at the
/// coast; 0 if the whole tile is no-data
pub fn fill_no_data(heights: &mut [f32], below: f32) {
    let lowest = heights
        .iter()
        .copied()
        .filter(|&h| h > below && h.is_finite())
        .fold(f32::INFINITY, f32::min);
    let fill = if lowest.is_finite() { lowest } else { 0.0 };
    for h in heights {
        if *h <= below || !h.is_finite() {
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
    fn the_geoid_only_when_asked_and_on_the_earth() {
        assert!(!terrain_geoid(None, &Body::EARTH));
        assert!(terrain_geoid(Some(true), &Body::EARTH));
        assert!(!terrain_geoid(Some(false), &Body::EARTH));
        assert!(!terrain_geoid(Some(true), &Body::MOON));
        assert!(!terrain_geoid(Some(true), &Body::MARS));
    }

    #[test]
    fn urls_fill_the_template() {
        let t = TileId::new(3, 4, 2);
        assert_eq!(
            tile_url("https://t/{z}/{x}/{y}.png", t, &Body::EARTH),
            "https://t/3/4/2.png"
        );
        // The whole world at level 0: EPSG:3857 on the Earth, as written before the bodies
        let b = bbox(TileId::new(0, 0, 0), &Body::EARTH);
        assert_eq!(
            b,
            "-20037508.342789244,-20037508.342789244,20037508.342789244,20037508.342789244"
        );
        let v: Vec<f64> = bbox(t, &Body::EARTH).split(',').map(|s| s.parse().unwrap()).collect();
        assert!(v[0] < v[2] && v[1] < v[3]);
        // The Moon's Mercator metres: its half circumference
        let moon: Vec<f64> = bbox(TileId::new(1, 1, 0), &Body::MOON)
            .split(',')
            .map(|s| s.parse().unwrap())
            .collect();
        let half = std::f64::consts::PI * 1_737_400.0;
        assert!(moon[0].abs() < 1e-6 && moon[1].abs() < 1e-6, "{moon:?}");
        assert!(
            (moon[2] - half).abs() < 1e-6 && (moon[3] - half).abs() < 1e-6,
            "{moon:?}"
        );
    }

    #[test]
    fn no_data_takes_the_lowest_altitude() {
        let below = no_data(&Body::EARTH);
        assert_eq!(below, -1250.0);
        let mut h = vec![12.0, -99_999.0, 5.0, f32::NAN, 30.0];
        fill_no_data(&mut h, below);
        assert_eq!(h, vec![12.0, 5.0, 5.0, 5.0, 30.0]);
        let mut all = vec![-99_999.0; 4];
        fill_no_data(&mut all, below);
        assert_eq!(all, vec![0.0; 4]);
    }

    #[test]
    fn deep_ground_of_other_bodies_is_data() {
        // Hellas and the South Pole-Aitken basin lie 8 and 9 km deep: data there; the 16-bit no-data is not
        let mut mars = vec![-8201.0, 21_241.0, -32_768.0, -1500.0];
        fill_no_data(&mut mars, no_data(&Body::MARS));
        assert_eq!(mars, vec![-8201.0, 21_241.0, -8201.0, -1500.0]);
        let mut moon = vec![-9126.0, 10_767.0, -3.4e38];
        fill_no_data(&mut moon, no_data(&Body::MOON));
        assert_eq!(moon, vec![-9126.0, 10_767.0, -9126.0]);
    }

    #[test]
    fn relief_starts_coarser_on_smaller_bodies() {
        assert_eq!(terrain_levels(&Body::EARTH, None), (6, 15));
        assert_eq!(terrain_levels(&Body::EARTH, Some(3)), (6, 6));
        assert_eq!(terrain_levels(&Body::EARTH, Some(20)), (6, 15));
        assert_eq!(terrain_levels(&Body::MOON, Some(8)), (3, 8));
        assert_eq!(terrain_levels(&Body::MARS, Some(8)), (3, 8));
        // A body flatter than the Earth starts at level 6 all the same
        let flat = Body {
            relief: (-10.0, 10.0),
            ..Body::MOON
        };
        assert_eq!(terrain_levels(&flat, None), (6, 15));
    }

    #[test]
    fn the_mask_marks_no_data() {
        // Ten pixels: the 2nd and the 10th invalid (bits 0), the mask's last byte only partly used
        let mut h: Vec<f32> = (0..10).map(|k| k as f32 * 100.0).collect();
        apply_mask(&mut h, &[0b1011_1111, 0b1000_0000]);
        assert!(h[1].is_nan() && h[9].is_nan());
        assert_eq!(h.iter().filter(|v| v.is_nan()).count(), 2);
        fill_no_data(&mut h, no_data(&Body::MARS));
        assert_eq!(h[1], 0.0);
        assert_eq!(h[9], 0.0);
        assert_eq!(h[8], 800.0);
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
