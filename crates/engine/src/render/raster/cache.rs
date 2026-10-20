//! The bookkeeping of a raster source: which tile is in which slot of its pool, in least recently used order, which
//! tiles failed, and each frame what to draw (stand-ins included) and what to load. Pure logic, the time given by the
//! caller: tested natively.

use super::{COOLDOWN_MAX_MS, COOLDOWN_MS, COVER_LEVELS, CPU_HEIGHTS, DESCENDANT_LEVELS};
use geo::tiles::{Selected, TileId};
use std::collections::{BTreeSet, HashMap, VecDeque};

/// Missing tiles (404) remembered at most: past it, the oldest are forgotten (asked again if wanted again). Browsing
/// the world over a regional source would otherwise remember every tile outside its region
const MAX_MISSING: usize = 1 << 16;
/// Tiles in cooldown remembered before those whose cooldown is over are let go
const MAX_COOLING: usize = 4096;

/// A tile held in the pool
struct Loaded {
    slot: u16,
    /// Elevation tiles: altitudes subsampled to `CPU_HEIGHTS`², rows from the north
    heights: Option<Box<[f32]>>,
    /// Last frame it was drawn
    used: u64,
    /// Upload time (ms), for the fade-in
    at: f64,
}

/// What a source gives to one tile of the pass: texture slots and uv rects of the tile and of its stand-in
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub front: Option<(u16, [f32; 4])>,
    pub back: Option<(u16, [f32; 4])>,
    pub fade: f32,
}

impl Sample {
    pub const NONE: Self = Self {
        front: None,
        back: None,
        fade: 1.0,
    };

    /// Appends the 11 floats of a layer of an instance: uv of the tile, uv of the stand-in, both slots (-1: none) and
    /// the fade
    pub fn write(&self, data: &mut Vec<f32>) {
        let uv = |s: Option<(u16, [f32; 4])>| s.map_or([0.0; 4], |(_, uv)| uv);
        let slot = |s: Option<(u16, [f32; 4])>| s.map_or(-1.0, |(slot, _)| f32::from(slot));
        data.extend_from_slice(&uv(self.front));
        data.extend_from_slice(&uv(self.back));
        data.extend_from_slice(&[slot(self.front), slot(self.back), self.fade]);
    }
}

/// Uv rect of `tile` inside `source` (the tile itself or one of its ancestors)
#[must_use]
pub fn crop(tile: TileId, source: TileId) -> [f32; 4] {
    let scale = f64::from(1u32 << (tile.z - source.z).min(31));
    let u0 = (f64::from(tile.x) / scale - f64::from(source.x)) as f32;
    let v0 = (f64::from(tile.y) / scale - f64::from(source.y)) as f32;
    let step = (1.0 / scale) as f32;
    [u0, v0, u0 + step, v0 + step]
}

/// Result of a slot request (`Cache::install`)
#[derive(Debug, PartialEq, Eq)]
pub enum Install {
    /// The tile goes into this slot
    Slot(u16),
    /// Already in the pool (a duplicate arrival): installing it again would leak a slot
    Duplicate,
    /// Every slot is held by this frame (drawn, or standing in)
    Full,
}

/// How a frame loads: the camera's state and the loading profile
#[derive(Clone, Copy, Debug, Default)]
pub struct Motion {
    /// During a camera flight, the levels flown through are skipped
    pub flying: bool,
    /// Covered tiles off the screen center wait (the camera moves, foveated loading)
    pub defer_off_center: bool,
}

/// A frame's plan, its buffers kept from frame to frame
#[derive(Default)]
pub struct Plan {
    /// Tiles to draw (stand-ins included), when this source is the base of a pass
    pub tiles: Vec<TileId>,
    /// Tiles to request, in order
    pub missing: Vec<TileId>,
    /// Whether each tile of `missing` is urgent (fetched with a high priority): in view near the screen center, or a
    /// coarse tile covering a hole; the others (off-center, stand-ins of failed tiles, flight destination) are not
    pub urgent: Vec<bool>,
    /// Coarse tiles to request first (nothing covers their area yet)
    cover: Vec<TileId>,
    /// Some tiles wait for the camera to rest
    pub deferred: bool,
}

impl Plan {
    /// A tile to request, urgent or not
    fn want(&mut self, tile: TileId, urgent: bool) {
        self.missing.push(tile);
        self.urgent.push(urgent);
    }
}

pub struct Cache {
    pub min_z: u8,
    pub max_z: u8,
    /// Slots of the pool
    capacity: usize,
    slots: HashMap<TileId, Loaded>,
    free: Vec<u16>,
    /// The tiles of the pool by the last frame they were drawn: least recently used first
    lru: BTreeSet<(u64, TileId)>,
    /// Tiles that kept failing: time (ms) they may be asked again, failures so far
    failed: HashMap<TileId, (f64, u32)>,
    /// Tiles not served (404), never asked again while remembered: the order they came in (`MAX_MISSING`)
    missing: HashMap<TileId, u64>,
    missing_order: VecDeque<(u64, TileId)>,
    missing_count: u64,
}

impl Cache {
    pub fn new(min_z: u8, max_z: u8, slots: usize) -> Self {
        let slots = slots.min(usize::from(u16::MAX)) as u16;
        Self {
            min_z,
            max_z,
            capacity: usize::from(slots),
            slots: HashMap::new(),
            free: (0..slots).rev().collect(),
            lru: BTreeSet::new(),
            failed: HashMap::new(),
            missing: HashMap::new(),
            missing_order: VecDeque::new(),
            missing_count: 0,
        }
    }

    pub fn contains(&self, tile: TileId) -> bool {
        self.slots.contains_key(&tile)
    }

    /// Tiles in the pool
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Tiles in the pool and their slots
    pub fn loaded(&self) -> impl Iterator<Item = (TileId, u16)> + '_ {
        self.slots.iter().map(|(&tile, loaded)| (tile, loaded.slot))
    }

    /// Marks a tile of the pool drawn in `frame`; its slot
    fn touch(&mut self, tile: TileId, frame: u64) -> Option<&Loaded> {
        let loaded = self.slots.get_mut(&tile)?;
        if loaded.used != frame {
            self.lru.remove(&(loaded.used, tile));
            loaded.used = frame;
            self.lru.insert((frame, tile));
        }
        Some(loaded)
    }

    /// A slot for an arrived tile: a free one, else the least recently used one not drawn in `frame`. `heights`: the
    /// coarse copy of an elevation tile; `now`: the upload time (ms)
    pub fn install(&mut self, tile: TileId, heights: Option<Box<[f32]>>, frame: u64, now: f64) -> Install {
        if self.slots.contains_key(&tile) {
            return Install::Duplicate;
        }
        let Some(slot) = self.free.pop().or_else(|| self.evict(frame)) else {
            return Install::Full;
        };
        self.slots.insert(
            tile,
            Loaded {
                slot,
                heights,
                used: frame,
                at: now,
            },
        );
        self.lru.insert((frame, tile));
        Install::Slot(slot)
    }

    /// Frees the least recently used slot not drawn in the current frame
    fn evict(&mut self, frame: u64) -> Option<u16> {
        let &(used, tile) = self.lru.first()?;
        if used >= frame {
            return None;
        }
        self.lru.remove(&(used, tile));
        self.slots.remove(&tile).map(|l| l.slot)
    }

    /// The tile arrived: its failures are forgotten
    pub fn succeeded(&mut self, tile: TileId) {
        self.failed.remove(&tile);
        self.missing.remove(&tile);
    }

    /// No tile there (404): never asked again, while among the last `MAX_MISSING` missing tiles
    pub fn missing(&mut self, tile: TileId) {
        self.failed.remove(&tile);
        self.missing_count += 1;
        self.missing.insert(tile, self.missing_count);
        self.missing_order.push_back((self.missing_count, tile));
        while self.missing.len() > MAX_MISSING || self.missing_order.len() > 2 * MAX_MISSING {
            let Some((order, old)) = self.missing_order.pop_front() else {
                break;
            };
            // An entry of a tile marked missing again later (or that arrived since) is stale
            if self.missing.get(&old) == Some(&order) {
                self.missing.remove(&old);
            }
        }
    }

    /// A transient error after the retries: asked again after a delay doubled at each new failure
    pub fn failed(&mut self, tile: TileId, now: f64) {
        if self.failed.len() >= MAX_COOLING {
            // The tiles whose cooldown is over go first: they may be asked again anyway
            self.failed.retain(|_, (until, _)| *until > now);
        }
        let strikes = self.failed.get(&tile).map_or(0, |f| f.1) + 1;
        let delay = (COOLDOWN_MS * f64::from(1u32 << (strikes - 1).min(8))).min(COOLDOWN_MAX_MS);
        self.failed.insert(tile, (now + delay, strikes));
    }

    /// Whether the tile failed before (its next request bypasses the HTTP cache)
    pub fn failed_before(&self, tile: TileId) -> bool {
        self.failed.contains_key(&tile) || self.missing.contains_key(&tile)
    }

    pub fn is_failed(&self, tile: TileId, now: f64) -> bool {
        self.missing.contains_key(&tile) || self.failed.get(&tile).is_some_and(|&(until, _)| now < until)
    }

    /// Tiles waiting before a new attempt, and tiles not served: the missing ones counted as they come, only the few
    /// in cooldown walked
    pub fn failure_counts(&self, now: f64) -> (usize, usize) {
        let cooling = self.failed.values().filter(|(until, _)| *until > now).count();
        (cooling, self.missing.len())
    }

    fn loaded_ancestor(&self, tile: TileId) -> Option<TileId> {
        let mut ancestor = tile.parent();
        while let Some(a) = ancestor {
            if self.slots.contains_key(&a) {
                return Some(a);
            }
            ancestor = a.parent();
        }
        None
    }

    /// Lists the loaded descendants of a tile, down to `levels` levels
    fn push_descendants(&self, tile: TileId, levels: u8, out: &mut Vec<TileId>) {
        if levels == 0 {
            return;
        }
        for child in tile.children() {
            if self.slots.contains_key(&child) {
                out.push(child);
            } else {
                self.push_descendants(child, levels - 1, out);
            }
        }
    }

    /// Marks a tile of the pool as held by `frame` (drawn, or standing in): it is not evicted by this frame's
    /// uploads. Whether the tile is in the pool and was not held yet
    fn hold(&mut self, tile: TileId, frame: u64) -> bool {
        let held = self.slots.get(&tile).is_some_and(|l| l.used != frame);
        self.touch(tile, frame);
        held
    }

    /// Chooses what to draw and what to load for the tiles `wanted` (by priority) and a flight destination
    /// (`preload`), into `plan`. The tiles the frame relies on (those drawn, their stand-ins) are held in the pool
    /// before the frame's uploads, and only as many tiles are asked for as the pool has room left: a view wanting
    /// more tiles than the pool holds (a tilted view to the horizon over large tiles) keeps its nearest ones and
    /// draws the farthest with their stand-ins for good, instead of evicting and reloading them without end
    pub fn plan(
        &mut self,
        plan: &mut Plan,
        wanted: &[Selected],
        preload: &[TileId],
        frame: u64,
        now: f64,
        motion: Motion,
    ) {
        plan.tiles.clear();
        plan.missing.clear();
        plan.urgent.clear();
        plan.cover.clear();
        plan.deferred = false;
        // Slots held by this frame
        let mut held = 0;
        for selected in wanted {
            let tile = selected.tile;
            if self.hold(tile, frame) {
                held += 1;
                plan.tiles.push(tile);
                continue;
            }
            // Stand-in: nearest loaded ancestor (cropped to this tile by `sample`), or its loaded descendants
            // (zooming out)
            let ancestor = self.loaded_ancestor(tile);
            let covered = ancestor.is_some();
            if let Some(ancestor) = ancestor {
                held += usize::from(self.hold(ancestor, frame));
                plan.tiles.push(tile);
            } else {
                let before = plan.tiles.len();
                self.push_descendants(tile, DESCENDANT_LEVELS, &mut plan.tiles);
                for k in before..plan.tiles.len() {
                    held += usize::from(self.hold(plan.tiles[k], frame));
                }
                if plan.tiles.len() == before {
                    // Nothing yet: drawn anyway, the other layers of the pass may cover it
                    plan.tiles.push(tile);
                }
                // Nothing loaded above: a coarse tile first, to cover the area quickly
                let level = tile.z.saturating_sub(COVER_LEVELS).max(self.min_z);
                let mut coarse = tile;
                while coarse.z > level {
                    coarse = coarse.parent().unwrap_or(coarse);
                }
                if coarse != tile && !self.is_failed(coarse, now) && !plan.cover.contains(&coarse) {
                    plan.cover.push(coarse);
                }
            }
            // During a flight, the levels flown through are skipped (Cesium's skip LOD): only the coarse coverage
            // and the destination load
            if motion.flying {
                if covered {
                    plan.deferred = true;
                } else if tile.z <= self.min_z.saturating_add(COVER_LEVELS) && !self.is_failed(tile, now) {
                    // Too coarse to have a coarser cover: the tile itself
                    plan.want(tile, true);
                }
                continue;
            }
            // While the camera moves, covered tiles off the screen center wait (foveated loading, as in Cesium):
            // the downloads go where the eye is
            if motion.defer_off_center && covered && !selected.central {
                plan.deferred = true;
                continue;
            }
            if self.is_failed(tile, now) {
                // No tile here (or it kept failing): its nearest ancestor that can load stands in
                let mut ancestor = tile.parent();
                while let Some(a) = ancestor {
                    if a.z < self.min_z || self.slots.contains_key(&a) {
                        break;
                    }
                    if !self.is_failed(a, now) {
                        plan.want(a, false);
                        break;
                    }
                    ancestor = a.parent();
                }
            } else {
                plan.want(tile, selected.central);
            }
        }
        // Destination of a camera flight: loaded after the current view, kept in the pool
        for &tile in preload {
            if self.hold(tile, frame) {
                held += 1;
            } else if !self.slots.contains_key(&tile) && !self.is_failed(tile, now) {
                plan.want(tile, false);
            }
        }
        // Coverage first (urgent: holes), then the tiles by priority, then the flight destination; past the room
        // left in the pool, the last ones wait (the farthest tiles, then the destination)
        plan.urgent.splice(0..0, plan.cover.iter().map(|_| true));
        plan.missing.splice(0..0, plan.cover.drain(..));
        let room = self.capacity.saturating_sub(held);
        plan.missing.truncate(room);
        plan.urgent.truncate(room);
    }

    /// What this source draws on the area of `tile`: its texture (or the deepest level it has, enlarged), with the
    /// stand-in to mix while it fades in; or its nearest loaded ancestor alone. Also whether it is fading in
    pub fn sample(&mut self, tile: TileId, frame: u64, now: f64, fade_ms: f64) -> (Sample, bool) {
        let mut target = tile;
        while target.z > self.max_z {
            target = target.parent().unwrap_or(target);
        }
        if target.z < self.min_z {
            return (Sample::NONE, false);
        }
        let front = self.touch(target, frame).map(|loaded| {
            let fade = if fade_ms > 0.0 {
                ((now - loaded.at) / fade_ms).clamp(0.0, 1.0)
            } else {
                1.0
            };
            (loaded.slot, fade as f32)
        });
        if let Some((slot, fade)) = front
            && fade >= 1.0
        {
            let sample = Sample {
                front: Some((slot, crop(tile, target))),
                back: None,
                fade,
            };
            return (sample, false);
        }
        let back = self.loaded_ancestor(target).and_then(|ancestor| {
            let slot = self.touch(ancestor, frame)?.slot;
            Some((slot, crop(tile, ancestor)))
        });
        if let Some((slot, fade)) = front {
            let sample = Sample {
                front: Some((slot, crop(tile, target))),
                back,
                fade,
            };
            (sample, true)
        } else {
            let sample = Sample {
                front: back,
                back: None,
                fade: 1.0,
            };
            (sample, false)
        }
    }

    /// Where the altitudes of `tile` are in the pool: offset (2) and scale (1) of the tile in the deepest loaded
    /// elevation tile covering it, and its slot (-1: none yet, flat)
    pub fn heights(&mut self, tile: TileId, frame: u64) -> [f32; 4] {
        let mut target = tile;
        while target.z > self.max_z {
            target = target.parent().unwrap_or(target);
        }
        let source = if self.slots.contains_key(&target) {
            Some(target)
        } else {
            self.loaded_ancestor(target)
        };
        let Some(slot) = source.and_then(|s| self.touch(s, frame)).map(|l| l.slot) else {
            return [0.0, 0.0, 1.0, -1.0];
        };
        let [u0, v0, u1, _] = crop(tile, source.unwrap_or(tile));
        [u0, v0, u1 - u0, f32::from(slot)]
    }

    /// Altitude (metres) at a Mercator point, from the finest elevation tile loaded there; None if none is
    pub fn altitude(&self, m: glam::DVec2) -> Option<f64> {
        if !m.is_finite() {
            return None;
        }
        // Across the antimeridian x wraps around the world (a point of another copy of the world), y stops at the
        // poles
        let m = glam::DVec2::new(m.x.rem_euclid(1.0), m.y).clamp(glam::DVec2::ZERO, glam::DVec2::splat(1.0 - 1e-9));
        for z in (self.min_z..=self.max_z).rev() {
            let n = f64::from(1u32 << z);
            let tile = TileId::new(z, (m.x * n) as u32, (m.y * n) as u32);
            let Some(heights) = self.slots.get(&tile).and_then(|l| l.heights.as_deref()) else {
                continue;
            };
            // Bilinear, samples at the cell centers
            let side = CPU_HEIGHTS as f64;
            let p = ((m * n - glam::DVec2::new(f64::from(tile.x), f64::from(tile.y))) * side - 0.5)
                .clamp(glam::DVec2::ZERO, glam::DVec2::splat(side - 1.0));
            let (i, f) = (p.floor(), p - p.floor());
            let at = |x: f64, y: f64| {
                let (x, y) = ((x as usize).min(CPU_HEIGHTS - 1), (y as usize).min(CPU_HEIGHTS - 1));
                f64::from(heights[y * CPU_HEIGHTS + x])
            };
            let top = at(i.x, i.y) * (1.0 - f.x) + at(i.x + 1.0, i.y) * f.x;
            let bottom = at(i.x, i.y + 1.0) * (1.0 - f.x) + at(i.x + 1.0, i.y + 1.0) * f.x;
            return Some(top * (1.0 - f.y) + bottom * f.y);
        }
        None
    }
}

/// Plans `frames` frames of a source of 256 slots over the tiles `wanted` and a flight destination, as the engine does:
/// the plan, the first eight tiles asked arriving, every tile drawn sampled. For the benchmarks (`cargo bench -p
/// isard`); returns the tiles drawn in all
#[must_use]
pub fn simulate_plans(wanted: &[Selected], preload: &[TileId], frames: u64) -> usize {
    let mut cache = Cache::new(0, 19, 256);
    let mut plan = Plan::default();
    let mut drawn = 0;
    for frame in 1..=frames {
        let now = frame as f64 * 16.0;
        cache.plan(&mut plan, wanted, preload, frame, now, Motion::default());
        for &tile in plan.missing.iter().take(8) {
            cache.install(tile, None, frame, now);
        }
        for &tile in &plan.tiles {
            let _ = cache.sample(tile, frame, now, 200.0);
        }
        drawn += plan.tiles.len();
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(tile: TileId) -> Selected {
        Selected {
            tile,
            priority: 0.0,
            central: true,
        }
    }

    #[test]
    fn crop_finds_the_tile_in_its_ancestor() {
        assert_eq!(crop(TileId::new(5, 3, 7), TileId::new(5, 3, 7)), [0.0, 0.0, 1.0, 1.0]);
        // The south-east quarter of its parent
        assert_eq!(crop(TileId::new(5, 3, 7), TileId::new(4, 1, 3)), [0.5, 0.5, 1.0, 1.0]);
        assert_eq!(crop(TileId::new(6, 4, 0), TileId::new(4, 1, 0)), [0.0, 0.0, 0.25, 0.25]);
    }

    #[test]
    fn samples_write_eleven_floats() {
        let mut data = Vec::new();
        Sample::NONE.write(&mut data);
        assert_eq!(data, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -1.0, 1.0]);
        data.clear();
        let s = Sample {
            front: Some((3, [0.0, 0.0, 1.0, 1.0])),
            back: Some((9, [0.5, 0.5, 1.0, 1.0])),
            fade: 0.25,
        };
        s.write(&mut data);
        assert_eq!(data, [0.0, 0.0, 1.0, 1.0, 0.5, 0.5, 1.0, 1.0, 3.0, 9.0, 0.25]);
    }

    #[test]
    fn the_least_recently_used_slot_is_recycled_never_one_on_screen() {
        let mut cache = Cache::new(0, 20, 2);
        let (a, b, c) = (TileId::new(3, 0, 0), TileId::new(3, 1, 0), TileId::new(3, 2, 0));
        assert_eq!(cache.install(a, None, 1, 0.0), Install::Slot(0));
        assert_eq!(cache.install(b, None, 2, 0.0), Install::Slot(1));
        assert_eq!(cache.install(b, None, 2, 0.0), Install::Duplicate);
        // `a` drawn again later than `b`: `b` goes
        cache.sample(a, 3, 0.0, 0.0);
        assert_eq!(cache.install(c, None, 4, 0.0), Install::Slot(1));
        assert!(cache.contains(a) && cache.contains(c) && !cache.contains(b));
        // Both drawn in the current frame: no slot
        cache.sample(a, 5, 0.0, 0.0);
        cache.sample(c, 5, 0.0, 0.0);
        assert_eq!(cache.install(b, None, 5, 0.0), Install::Full);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn failures_cool_down_and_missing_tiles_stay_missing() {
        let mut cache = Cache::new(0, 20, 4);
        let (a, b) = (TileId::new(4, 1, 1), TileId::new(4, 2, 1));
        cache.failed(a, 1000.0);
        assert!(cache.is_failed(a, 2000.0) && !cache.is_failed(a, 3001.0));
        // Twice the delay after a second failure
        cache.failed(a, 1000.0);
        assert!(cache.is_failed(a, 4999.0) && !cache.is_failed(a, 5001.0));
        cache.missing(b);
        assert!(cache.is_failed(b, 1e12));
        assert_eq!(cache.failure_counts(2000.0), (1, 1));
        cache.succeeded(a);
        assert!(!cache.failed_before(a) && cache.failed_before(b));
    }

    #[test]
    fn plan_covers_holes_with_a_coarse_tile_then_stands_in_with_ancestors() {
        let mut cache = Cache::new(0, 20, 16);
        let mut plan = Plan::default();
        let tile = TileId::new(10, 500, 300);
        cache.plan(&mut plan, &[selected(tile)], &[], 1, 0.0, Motion::default());
        // Nothing loaded: drawn anyway, its grand-parent asked first, then itself
        assert_eq!(plan.tiles, [tile]);
        let coarse = tile.parent().unwrap().parent().unwrap();
        assert_eq!(plan.missing, [coarse, tile]);
        assert_eq!(plan.urgent, [true, true]);
        // The grand-parent arrived: it stands in, only the tile is asked
        cache.install(coarse, None, 1, 0.0);
        cache.plan(&mut plan, &[selected(tile)], &[], 2, 0.0, Motion::default());
        assert_eq!(plan.tiles, [tile]);
        assert_eq!(plan.missing, [tile]);
        let (sample, fading) = cache.sample(tile, 2, 0.0, 0.0);
        assert!(!fading);
        assert_eq!(sample.front, Some((0, crop(tile, coarse))));
        // Off the screen center while the camera moves: it waits
        let off = Selected {
            central: false,
            ..selected(tile)
        };
        let motion = Motion {
            defer_off_center: true,
            ..Motion::default()
        };
        cache.plan(&mut plan, &[off], &[], 3, 0.0, motion);
        assert!(plan.missing.is_empty() && plan.deferred);
        // Off the center at rest: loaded, not urgent; the flight destination neither
        let far = TileId::new(10, 900, 900);
        cache.plan(&mut plan, &[off], &[far], 4, 0.0, Motion::default());
        assert_eq!(plan.missing, [tile, far]);
        assert_eq!(plan.urgent, [false, false]);
    }

    #[test]
    fn a_view_wanting_more_tiles_than_the_pool_holds_keeps_its_nearest_ones() {
        let mut cache = Cache::new(0, 20, 4);
        let mut plan = Plan::default();
        let root = TileId::new(0, 0, 0);
        let [p, q, r, s, t] = [0, 1, 2, 3, 4].map(|x| TileId::new(3, x, 0));
        for tile in [root, p, q, r] {
            cache.install(tile, None, 1, 0.0);
        }
        // Five tiles wanted, by priority: three loaded, two standing in with the root; the pool is full of them, so
        // nothing more is asked for, and an arrival finds no slot
        let wanted: Vec<Selected> = [p, q, r, s, t].into_iter().map(selected).collect();
        cache.plan(&mut plan, &wanted, &[], 2, 0.0, Motion::default());
        assert_eq!(plan.tiles, [p, q, r, s, t]);
        assert_eq!(plan.missing, []);
        assert_eq!(cache.install(s, None, 2, 0.0), Install::Full);
        // Fewer wanted: two slots are held (p and the root), the others are free for the two tiles asked for
        let wanted: Vec<Selected> = [p, s, t].into_iter().map(selected).collect();
        cache.plan(&mut plan, &wanted, &[], 3, 0.0, Motion::default());
        assert_eq!(plan.missing, [s, t]);
        assert_eq!(cache.install(s, None, 3, 0.0), Install::Slot(2));
        assert!(cache.contains(p) && cache.contains(root) && !cache.contains(q));
        // Now three are held: room for one, the nearest of the rest
        let wanted: Vec<Selected> = [p, s, t, TileId::new(3, 5, 0)].into_iter().map(selected).collect();
        cache.plan(&mut plan, &wanted, &[], 4, 0.0, Motion::default());
        assert_eq!(plan.missing, [t]);
    }

    #[test]
    fn a_failed_tile_asks_its_nearest_ancestor_instead() {
        let mut cache = Cache::new(0, 20, 16);
        let mut plan = Plan::default();
        let tile = TileId::new(8, 100, 100);
        cache.install(TileId::new(4, 6, 6), None, 1, 0.0);
        cache.missing(tile);
        cache.plan(&mut plan, &[selected(tile)], &[], 2, 0.0, Motion::default());
        assert_eq!(plan.missing, [tile.parent().unwrap()]);
        assert_eq!(plan.urgent, [false]);
    }

    #[test]
    fn descendants_stand_in_when_zooming_out() {
        let mut cache = Cache::new(0, 20, 16);
        let mut plan = Plan::default();
        let tile = TileId::new(6, 10, 10);
        let children = tile.children();
        cache.install(children[0], None, 1, 0.0);
        cache.install(children[3].children()[1], None, 1, 0.0);
        cache.plan(&mut plan, &[selected(tile)], &[], 2, 0.0, Motion::default());
        assert_eq!(plan.tiles, [children[0], children[3].children()[1]]);
    }

    #[test]
    fn fading_tiles_mix_with_their_stand_in() {
        let mut cache = Cache::new(0, 20, 16);
        let (parent, tile) = (TileId::new(3, 1, 1), TileId::new(4, 2, 2));
        cache.install(parent, None, 1, 0.0);
        cache.install(tile, None, 1, 1000.0);
        let (s, fading) = cache.sample(tile, 2, 1100.0, 200.0);
        assert!(fading);
        assert!((s.fade - 0.5).abs() < 1e-6);
        assert_eq!(s.back.map(|b| b.1), Some([0.0, 0.0, 0.5, 0.5]));
        // Done fading: alone
        let (s, fading) = cache.sample(tile, 3, 1300.0, 200.0);
        assert!(!fading && s.back.is_none());
    }

    #[test]
    fn altitudes_come_from_the_finest_tile_bilinear() {
        let mut cache = Cache::new(0, 20, 16);
        let (coarse, fine) = (TileId::new(2, 1, 1), TileId::new(3, 2, 2));
        let flat = vec![10.0; CPU_HEIGHTS * CPU_HEIGHTS].into_boxed_slice();
        // A slope along x in the fine tile
        let slope: Box<[f32]> = (0..CPU_HEIGHTS * CPU_HEIGHTS)
            .map(|k| (k % CPU_HEIGHTS) as f32)
            .collect();
        cache.install(coarse, Some(flat), 1, 0.0);
        assert_eq!(cache.altitude(glam::DVec2::new(0.3, 0.3)), Some(10.0));
        cache.install(fine, Some(slope), 1, 0.0);
        // The middle of the fine tile, between samples 15 and 16
        let h = cache.altitude(glam::DVec2::new(0.3125, 0.3125)).unwrap();
        assert!((h - 15.5).abs() < 1e-9, "{h}");
        // Elsewhere in the coarse tile
        assert_eq!(cache.altitude(glam::DVec2::new(0.4, 0.4)), Some(10.0));
        assert_eq!(cache.altitude(glam::DVec2::new(0.9, 0.9)), None);
        assert_eq!(cache.heights(TileId::new(4, 5, 5), 2), [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn altitudes_wrap_across_the_antimeridian() {
        let mut cache = Cache::new(0, 20, 16);
        // Two tiles at the ends of the world, west at 100 m, east at 900 m
        let (west, east) = (TileId::new(3, 0, 3), TileId::new(3, 7, 3));
        cache.install(
            west,
            Some(vec![100.0; CPU_HEIGHTS * CPU_HEIGHTS].into_boxed_slice()),
            1,
            0.0,
        );
        cache.install(
            east,
            Some(vec![900.0; CPU_HEIGHTS * CPU_HEIGHTS].into_boxed_slice()),
            1,
            0.0,
        );
        let y = 3.5 / 8.0;
        // Just east of the antimeridian, on the next copy of the world: the western tile, not the eastern one
        assert_eq!(cache.altitude(glam::DVec2::new(1.01, y)), Some(100.0));
        // Just west of it, on the copy before
        assert_eq!(cache.altitude(glam::DVec2::new(-0.01, y)), Some(900.0));
        assert_eq!(cache.altitude(glam::DVec2::new(0.99, y)), Some(900.0));
        assert_eq!(cache.altitude(glam::DVec2::new(f64::NAN, y)), None);
        assert_eq!(cache.altitude(glam::DVec2::new(f64::INFINITY, y)), None);
    }

    #[test]
    fn missing_tiles_are_remembered_within_a_bound() {
        let mut cache = Cache::new(0, 20, 4);
        let first = TileId::new(20, 0, 0);
        for k in 0..=MAX_MISSING as u32 {
            cache.missing(TileId::new(20, k, 0));
        }
        // The oldest forgotten, the newest kept, the count without walking them
        assert!(!cache.is_failed(first, 0.0));
        assert!(cache.is_failed(TileId::new(20, MAX_MISSING as u32, 0), 0.0));
        assert_eq!(cache.failure_counts(0.0), (0, MAX_MISSING));
        // A tile marked missing again keeps its place; one that arrived is forgotten
        let again = TileId::new(20, 5, 0);
        cache.missing(again);
        cache.succeeded(TileId::new(20, 6, 0));
        for k in 0..MAX_MISSING as u32 - 2 {
            cache.missing(TileId::new(19, k, 0));
        }
        assert!(cache.is_failed(again, 0.0));
        assert!(cache.missing.len() <= MAX_MISSING && cache.missing_order.len() <= 2 * MAX_MISSING);
        // Tiles in cooldown: those whose cooldown is over let go past the bound
        for k in 0..MAX_COOLING as u32 + 10 {
            cache.failed(TileId::new(18, k, 0), f64::from(k) * 100_000.0);
        }
        assert!(cache.failed.len() < MAX_COOLING);
    }
}
