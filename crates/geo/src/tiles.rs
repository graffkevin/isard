//! Tile selection: a quadtree walk that keeps the tiles inside the view frustum (and in front of the horizon on the
//! globe) and refines each one until it covers about `tile_px` CSS pixels on screen. Under a tilted camera, nearby
//! tiles get fine and the horizon stays coarse; past the screen center's distance, a tile may grow larger on screen
//! the farther it is (up to `FAR_COARSENING` times): what lies toward the horizon, seen at a grazing angle and in the
//! fog, takes fewer tiles (about half as many at 70°), and the pool of a layer holds the view.
//!
//! Tiles are tested through a grid of sample points projected like the GPU does (plane, sphere or a blend), so the
//! same code serves both projections.

use crate::camera::View;
use glam::{DVec2, DVec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    #[must_use]
    pub fn new(z: u8, x: u32, y: u32) -> Self {
        Self { z, x, y }
    }

    /// Mercator bounds: (min, max)
    #[must_use]
    pub fn bounds(self) -> (DVec2, DVec2) {
        let n = f64::from(1u32 << self.z);
        let min = DVec2::new(f64::from(self.x), f64::from(self.y)) / n;
        (min, min + DVec2::splat(1.0 / n))
    }

    #[must_use]
    pub fn parent(self) -> Option<Self> {
        (self.z > 0).then(|| Self::new(self.z - 1, self.x / 2, self.y / 2))
    }

    #[must_use]
    pub fn children(self) -> [Self; 4] {
        let (z, x, y) = (self.z + 1, self.x * 2, self.y * 2);
        [
            Self::new(z, x, y),
            Self::new(z, x + 1, y),
            Self::new(z, x, y + 1),
            Self::new(z, x + 1, y + 1),
        ]
    }

    /// Whole worlds to add to x so that the tile is drawn on the copy of the world nearest to the center
    #[must_use]
    pub fn shift(self, center: DVec2) -> f64 {
        let (min, max) = self.bounds();
        -(f64::midpoint(min.x, max.x) - center.x).round()
    }
}

/// Upper bound on the tiles of one selection
const MAX_TILES: usize = 400;
/// How much larger on screen than `tile_px` a far tile may stay: its distance over the screen center's, up to this
const FAR_COARSENING: f64 = 4.0;
/// Finest level a selection reaches
const MAX_Z: u8 = 30;
/// On the globe, tiles coarser than this are always split (they would be too curved for their mesh)
const GLOBE_MIN_Z: u8 = 2;

/// Half-angle of the foveated cone around the view axis (radians): tiles inside it load even while the camera moves
/// (Cesium's foveated loading)
pub const FOVEA: f64 = crate::camera::FOV * 0.3;

/// A tile to draw, with its loading priority
#[derive(Clone, Copy, Debug)]
pub struct Selected {
    pub tile: TileId,
    /// Lower loads first: distance to the camera, raised away from the screen center
    pub priority: f64,
    /// Inside the foveated cone
    pub central: bool,
}

/// Tiles to draw, by priority (screen center and near the camera first). `tile_px`: target size of a tile on screen
/// (CSS pixels).
#[must_use]
pub fn select(view: &View, min_z: u8, max_z: u8, tile_px: f64) -> Vec<Selected> {
    // Tile coordinates are u32: levels past 30 do not exist
    let (min_z, max_z) = (min_z.min(MAX_Z), max_z.min(MAX_Z));
    let globe = view.globe.factor > 0.0;
    let mut out = Vec::new();
    let mut stack = vec![TileId::new(0, 0, 0)];
    while let Some(tile) = stack.pop() {
        let Some((apparent, distance, angle)) = measure(view, tile) else {
            continue;
        };
        let fine_enough = apparent <= tile_px * (distance / view.center_distance).clamp(1.0, FAR_COARSENING);
        let refine = if tile.z < min_z {
            // Below the layer's first level: go down only if that level would be about the right size, as this tile
            // measures it, or at the view center, where a tile of that level has its size on the plane (large tiles of
            // the globe seen from a pitched camera have their samples far away, and measure too small)
            let first = 2f64.powi(i32::from(min_z - tile.z));
            apparent / first > tile_px / 2.0
                || (contains(tile, view.center) && view.world_size / 2f64.powi(i32::from(min_z)) > tile_px / 2.0)
        } else {
            tile.z < max_z && (!fine_enough || (globe && tile.z < GLOBE_MIN_Z))
        };
        if refine && out.len() + stack.len() < MAX_TILES {
            stack.extend(tile.children());
        } else if tile.z >= min_z {
            let priority = distance * (1.0 + 3.0 * angle / (crate::camera::FOV / 2.0));
            out.push(Selected {
                tile,
                priority,
                central: angle <= FOVEA,
            });
        }
    }
    out.sort_by(|p, q| p.priority.total_cmp(&q.priority));
    out
}

/// Whether a tile holds a Mercator point (on any copy of the world)
fn contains(tile: TileId, m: DVec2) -> bool {
    let (min, max) = tile.bounds();
    (min.x..max.x).contains(&m.x.rem_euclid(1.0)) && (min.y..max.y).contains(&m.y)
}

/// Size on screen (CSS pixels), distance to the camera and angle from the view axis (radians, of its point nearest
/// to the axis) of a visible tile; None if out of view
fn measure(view: &View, tile: TileId) -> Option<(f64, f64, f64)> {
    // Coarse tiles are large and curved: more samples
    let n = if tile.z < 4 { 5 } else { 3 };
    let (min, max) = tile.bounds();
    let shift = tile.shift(view.center);
    let mut points = [DVec3::ZERO; 25];
    for j in 0..n {
        for i in 0..n {
            let t = DVec2::new(i as f64, j as f64) / (n - 1) as f64;
            points[j * n + i] = view.position(min + (max - min) * t, shift);
        }
    }
    // The samples stand at the altitude of the camera's center (`view.lift`, its ground's: 0 without relief or over
    // the sea), for the size and the distance
    let up = DVec3::new(0.0, 0.0, view.lift);
    for p in &mut points[..n * n] {
        *p += up;
    }
    let points = &points[..n * n];
    // On the globe, the tiles of the first levels are split whatever they measure: their samples, a quarter of the
    // world apart, can all lie beyond one plane of the frustum, or out of the cap the camera sees, while the sphere
    // bulging between them is in view
    let coarse = view.globe.factor > 0.0 && tile.z < GLOBE_MIN_Z;
    if !coarse && !visible(view, tile, shift, points, up) {
        return None;
    }
    // Longest side between neighbouring samples, scaled to the whole tile
    let mut side = 0f64;
    for j in 0..n {
        for i in 0..n {
            let p = points[j * n + i];
            if i + 1 < n {
                side = side.max(p.distance(points[j * n + i + 1]));
            }
            if j + 1 < n {
                side = side.max(p.distance(points[(j + 1) * n + i]));
            }
        }
    }
    let distance = points
        .iter()
        .map(|p| p.distance(view.eye))
        .fold(f64::INFINITY, f64::min)
        .max(1e-9);
    // The tile's nearest point may be well inside it: the camera can stand right above a large tile
    let distance = distance.min(nearest_on_plane(view, min, max, shift));
    let (min_m, max_m) = (min, max);
    let angle = if (min_m.x..max_m.x).contains(&view.center.x) && (min_m.y..max_m.y).contains(&view.center.y) {
        0.0
    } else {
        let axis = -view.eye.normalize();
        points
            .iter()
            .map(|p| (*p - view.eye).normalize().dot(axis).clamp(-1.0, 1.0).acos())
            .fold(f64::INFINITY, f64::min)
    };
    Some((side * (n - 1) as f64 * view.center_distance / distance, distance, angle))
}

/// Whether the samples of a tile, and on the globe its point nearest to the one under the camera, are in the frustum
/// and in front of the horizon. Lifted, the tile counts as visible if any of 0 to twice the altitude of the camera's
/// center is (valleys and peaks around the center); a center at sea level tests sea level only
fn visible(view: &View, tile: TileId, shift: f64, points: &[DVec3], up: DVec3) -> bool {
    let mut samples = [DVec3::ZERO; 26];
    samples[..points.len()].copy_from_slice(points);
    let mut count = points.len();
    if view.globe.factor > 0.0 {
        samples[count] = nearest_under(view, tile, shift) + up;
        count += 1;
    }
    let samples = &samples[..count];
    let in_view = if view.lift == 0.0 {
        in_frustum(view, samples)
    } else {
        let mut column = [DVec3::ZERO; 78];
        for (k, layer) in [-1.0, 0.0, 1.0].into_iter().zip(column.chunks_exact_mut(count)) {
            for (q, p) in layer.iter_mut().zip(samples) {
                *q = *p + up * k;
            }
        }
        in_frustum(view, &column[..count * 3])
    };
    in_view && (view.globe.factor < 0.999 || samples.iter().any(|p| view.faces_camera(*p)))
}

/// The tile's point nearest to the one under the camera (local frame). The samples of a coarse tile are far apart
/// (a quarter of the world for the root): from a camera close to the sphere, the cap it sees may lie between them
/// all, and the whole quadtree would be culled.
fn nearest_under(view: &View, tile: TileId, shift: f64) -> DVec3 {
    let under = view.sphere_to_mercator(view.eye);
    let (min, max) = tile.bounds();
    // The copy of the world of the tile
    let x = under.x + (f64::midpoint(min.x, max.x) - under.x).round();
    view.position(DVec2::new(x.clamp(min.x, max.x), under.y.clamp(min.y, max.y)), shift)
}

/// Distance from the camera to the tile's rectangle on the plane (exact for the Mercator plane, a bound otherwise)
fn nearest_on_plane(view: &View, min: DVec2, max: DVec2, shift: f64) -> f64 {
    if view.globe.factor > 0.0 {
        return f64::INFINITY;
    }
    let a = view.position(min, shift);
    let b = view.position(max, shift);
    let nearest = DVec2::new(view.eye.x.clamp(a.x, b.x), view.eye.y.clamp(a.y, b.y));
    (DVec3::new(nearest.x, nearest.y, view.lift) - view.eye).length()
}

/// Conservative test of sample points against the frustum: out only if all are beyond the same plane
pub(crate) fn in_frustum(view: &View, points: &[DVec3]) -> bool {
    in_planes(view, points, ALL_PLANES)
}

/// The six planes of the frustum (`in_planes`): left, right, bottom, top, near, far
pub(crate) const ALL_PLANES: u8 = 0b11_1111;
/// The frustum's planes but the far one
pub(crate) const NO_FAR_PLANE: u8 = 0b01_1111;

/// `in_frustum` against some of the frustum's planes (one bit each, `ALL_PLANES`)
pub(crate) fn in_planes(view: &View, points: &[DVec3], planes: u8) -> bool {
    // One bit per plane, kept while every point is beyond it
    let mut outside = planes;
    for p in points {
        let c = view.view_proj * p.extend(1.0);
        let bits = u8::from(c.x < -c.w)
            | u8::from(c.x > c.w) << 1
            | u8::from(c.y < -c.w) << 2
            | u8::from(c.y > c.w) << 3
            | u8::from(c.z < -c.w) << 4
            | u8::from(c.z > c.w) << 5;
        outside &= bits;
        if outside == 0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;

    fn select(view: &View, min_z: u8, max_z: u8, tile_px: f64) -> Vec<TileId> {
        super::select(view, min_z, max_z, tile_px)
            .into_iter()
            .map(|s| s.tile)
            .collect()
    }

    fn camera(zoom: f64, pitch: f64) -> Camera {
        let mut c = Camera::new(2.424_573, 48.845_726, zoom);
        c.width = 1600.0;
        c.height = 900.0;
        c.pitch = pitch;
        c
    }

    #[test]
    fn a_coarse_first_level_shows_under_a_pitched_globe() {
        // The relief of Mars from level 3 (elevation tiles of 2896 px): over Olympus Mons at zoom 6, pitched, on the
        // globe; the tiles of level 0 to 2 measure too small there (their samples far from the camera)
        let mut c = Camera::new(-133.5, 14.0, 6.0);
        c.globe = true;
        c.width = 960.0;
        c.height = 600.0;
        c.pitch = 75.0;
        let tiles = select(&c.view(), 3, 8, 2896.0);
        assert!(tiles.iter().any(|&t| contains(t, c.center)), "{tiles:?}");
        // Zoomed out, still none: the Earth's relief from level 6 does not load on the globe
        c.zoom = 4.0;
        assert_eq!(select(&c.view(), 3, 8, 2896.0), Vec::new());
        let mut earth = camera(6.5, 60.0);
        earth.globe = true;
        assert_eq!(select(&earth.view(), 6, 15, 2896.0), Vec::new());
    }

    #[test]
    fn a_globe_seen_from_close_and_pitched_has_tiles() {
        // Olympus Mons from the south at zoom 6.3, pitch 66, a sphere to the horizon: no sample of the coarse tiles
        // faces the camera, the tiles under it must still be found
        let mut c = Camera::new(-133.6, 15.6, 6.3);
        c.globe = true;
        c.width = 1790.0;
        c.height = 678.0;
        c.max_pitch = 85.0;
        c.pitch = 66.0;
        let view = c.view();
        assert!((view.globe.factor - 1.0).abs() < 1e-12);
        let tiles = select(&view, 0, 7, 362.0);
        assert!(tiles.iter().any(|&t| contains(t, c.center)), "{tiles:?}");
        assert!(tiles.len() < MAX_TILES);
    }

    #[test]
    fn a_globe_seen_along_its_horizon_has_tiles() {
        // Down Valles Marineris from the west at zoom 6.6, pitch 70: every sample of the root tile lies beyond one
        // plane of the frustum
        let mut c = Camera::new(-80.0, -6.5, 6.6);
        c.globe = true;
        c.width = 1790.0;
        c.height = 678.0;
        c.max_pitch = 85.0;
        c.pitch = 70.0;
        c.bearing = 100.0;
        let tiles = select(&c.view(), 0, 7, 362.0);
        assert!(tiles.iter().any(|&t| contains(t, c.center)), "{tiles:?}");
        assert!(tiles.len() < MAX_TILES);
    }

    #[test]
    fn far_tiles_coarsen_with_their_distance() {
        // Pitched 70° over Paris, a 1575 × 855 map: about 90 tiles (156 if every one covered `tile_px`), within the 96
        // slots of a pool of 512 px tiles with their stand-ins; the nearest as fine as before
        let mut c = camera(15.6, 70.0);
        c.width = 1575.0;
        c.height = 855.0;
        let tiles = select(&c.view(), 0, 19, 362.0);
        assert!(tiles.len() <= 96, "{}", tiles.len());
        assert_eq!(tiles.iter().map(|t| t.z).max(), Some(18));
        // Seen from above, nothing changes: every tile is about as far as the center
        let flat = camera(14.0, 0.0);
        assert!(select(&flat.view(), 0, 19, 362.0).iter().all(|t| t.z == 15));
    }

    #[test]
    fn flat_view_uses_one_level() {
        let c = camera(14.0, 0.0);
        let tiles = select(&c.view(), 0, 19, 362.0);
        assert!(tiles.iter().all(|t| t.z == 15), "{tiles:?}");
        // 1600 × 900 px at 256 px per tile: about 7 × 4 tiles, plus the borders
        assert!((20..=60).contains(&tiles.len()), "{}", tiles.len());
    }

    #[test]
    fn tilted_view_coarsens_towards_the_horizon() {
        let c = camera(15.0, 70.0);
        let tiles = select(&c.view(), 0, 19, 362.0);
        let (lo, hi) = (
            tiles.iter().map(|t| t.z).min().unwrap(),
            tiles.iter().map(|t| t.z).max().unwrap(),
        );
        assert!(hi - lo >= 3, "{lo}..{hi}");
        assert!(tiles.len() < MAX_TILES);
        // The screen center first (foveated), not the tile nearest to the camera
        let (min, max) = tiles[0].bounds();
        assert!((min.x..max.x).contains(&c.center.x) && (min.y..max.y).contains(&c.center.y));
    }

    #[test]
    fn screen_center_loads_first() {
        let c = camera(14.0, 0.0);
        let selected = super::select(&c.view(), 0, 19, 362.0);
        let first = selected[0].tile.bounds();
        assert!((first.0.x..first.1.x).contains(&c.center.x) && (first.0.y..first.1.y).contains(&c.center.y));
        assert!(selected[0].central);
        // The screen corners are outside the fovea
        assert!(selected.iter().any(|s| !s.central));
        let central = selected.iter().filter(|s| s.central).count();
        assert!(central < selected.len() / 2, "{central} of {}", selected.len());
    }

    #[test]
    fn max_zoom_caps_the_levels() {
        let c = camera(20.0, 0.0);
        assert!(select(&c.view(), 0, 17, 362.0).iter().all(|t| t.z == 17));
    }

    #[test]
    fn tiles_cover_the_screen_center() {
        let c = camera(13.4, 45.0);
        let tiles = select(&c.view(), 0, 19, 362.0);
        assert!(tiles.iter().any(|t| {
            let (min, max) = t.bounds();
            (min.x..max.x).contains(&c.center.x) && (min.y..max.y).contains(&c.center.y)
        }));
    }

    #[test]
    fn layer_too_fine_for_the_view_is_skipped() {
        // Hillshade (from zoom 6) on a view of the whole of Europe: nothing rather than hundreds of tiny tiles
        let c = camera(3.0, 0.0);
        assert_eq!(select(&c.view(), 6, 17, 362.0).len(), 0);
        assert_ne!(select(&camera(5.5, 0.0).view(), 6, 17, 362.0).len(), 0);
    }

    #[test]
    fn globe_shows_the_near_side_only() {
        let mut c = camera(1.5, 0.0);
        c.globe = true;
        let tiles = select(&c.view(), 0, 19, 362.0);
        assert!(!tiles.is_empty() && tiles.len() < 64, "{}", tiles.len());
        assert!(tiles.iter().all(|t| t.z >= GLOBE_MIN_Z));
        // Nothing on the far side: tropics beyond 125° of longitude from France (near the north pole, every
        // longitude faces the camera)
        let far = tiles.iter().filter(|t| {
            let (min, max) = t.bounds();
            let (x, y) = (f64::midpoint(min.x, max.x), f64::midpoint(min.y, max.y));
            !(0.15..0.85).contains(&x) && (0.4..0.6).contains(&y) && t.z >= 3
        });
        let far: Vec<_> = far.collect();
        assert!(far.is_empty(), "{far:?}");
    }

    #[test]
    fn levels_past_31_do_not_overflow() {
        let c = camera(3.0, 0.0);
        assert_eq!(select(&c.view(), 40, 255, 362.0), Vec::<TileId>::new());
        assert!(
            select(&camera(25.0, 0.0).view(), 0, 255, 362.0)
                .iter()
                .all(|t| t.z <= MAX_Z)
        );
    }

    #[test]
    fn globe_and_mercator_agree_when_zoomed_in() {
        let mut c = camera(12.0, 30.0);
        let flat = select(&c.view(), 0, 19, 362.0);
        c.globe = true;
        assert_eq!(flat, select(&c.view(), 0, 19, 362.0));
    }
}
