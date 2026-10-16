//! The loading state of each tile of a 3D Tiles layer, with the counts kept as the states change (no pass over every
//! tile per frame for them), and the choice of the requests of a frame. Pure logic, the GPU meshes given as a type
//! parameter and the time by the caller: tested natively.

use std::collections::HashSet;
use tiles3d::tileset::Readiness;

/// After an error, a tile is asked again after this delay (ms)
pub const COOLDOWN_MS: f64 = 5_000.0;
/// Downloads of siblings out of view at a time
pub const PREFETCH_REQUESTS: usize = 4;
/// Attempts at the root tileset before the layer gives up, and the pause before the first retry (ms, doubled at each)
pub const ROOT_ATTEMPTS: u32 = 4;
pub const ROOT_RETRY_MS: f64 = 2_000.0;

/// What the state keeps of a tile on the GPU
pub trait Resident {
    /// GPU memory (bytes)
    fn bytes(&self) -> usize;
    /// Last frame it was drawn
    fn used(&self) -> u64;
    fn touch(&mut self, frame: u64);
}

pub enum State<M> {
    Unloaded,
    Loading(Loading),
    Ready(M),
    /// No drawable content, for good (none, unsupported, grafted external tileset)
    Empty,
    /// Errored: asked again after this time (ms)
    Failed(f64),
}

/// A request in flight: its number (arrivals of an older request are ignored) and how to cancel it
pub struct Loading {
    pub id: u32,
    /// Fetched on this thread; None when a worker of the loader has it
    pub controller: Option<web_sys::AbortController>,
}

/// The states of the tiles of a tree, by node
pub struct States<M> {
    tiles: Vec<State<M>>,
    /// Tiles loading
    loading: HashSet<usize>,
    /// Tiles on the GPU, and their memory
    ready: usize,
    gpu_bytes: usize,
    /// Buffer of `take_stale`
    visible: HashSet<usize>,
}

impl<M> Default for States<M> {
    fn default() -> Self {
        Self {
            tiles: Vec::new(),
            loading: HashSet::new(),
            ready: 0,
            gpu_bytes: 0,
            visible: HashSet::new(),
        }
    }
}

impl<M: Resident> States<M> {
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn get(&self, tile: usize) -> Option<&State<M>> {
        self.tiles.get(tile)
    }

    /// Tiles loading, tiles on the GPU, their memory (bytes)
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.loading.len(), self.ready, self.gpu_bytes)
    }

    /// New nodes in the tree (an external tileset grafted): unloaded, or empty when `empty` says they have no content
    pub fn grow(&mut self, nodes: usize, empty: impl Fn(usize) -> bool) {
        let first = self.tiles.len();
        for i in first..nodes {
            self.tiles.push(if empty(i) { State::Empty } else { State::Unloaded });
        }
    }

    /// Sets the state of a tile, the counts following; the previous state
    pub fn replace(&mut self, tile: usize, state: State<M>) -> Option<State<M>> {
        let slot = self.tiles.get_mut(tile)?;
        match &state {
            State::Loading(_) => {
                self.loading.insert(tile);
            }
            State::Ready(mesh) => {
                self.ready += 1;
                self.gpu_bytes += mesh.bytes();
            }
            _ => {}
        }
        let old = std::mem::replace(slot, state);
        match &old {
            State::Loading(_) => {
                if !matches!(self.tiles[tile], State::Loading(_)) {
                    self.loading.remove(&tile);
                }
            }
            State::Ready(mesh) => {
                self.ready -= 1;
                self.gpu_bytes -= mesh.bytes();
            }
            _ => {}
        }
        Some(old)
    }

    /// Whether `id` is the request a tile waits for (not one aborted, then asked again)
    pub fn is_current(&self, tile: usize, id: u32) -> bool {
        matches!(self.tiles.get(tile), Some(State::Loading(l)) if l.id == id)
    }

    pub fn is_loading(&self, tile: usize) -> bool {
        self.loading.contains(&tile)
    }

    /// Unloaded, or failed (asked again once its cooldown is over: the traversal says when)
    pub fn requestable(&self, tile: usize) -> bool {
        matches!(self.tiles.get(tile), Some(State::Unloaded | State::Failed(_)))
    }

    /// Whether the tile failed before (its next request bypasses the HTTP cache)
    pub fn failed_before(&self, tile: usize) -> bool {
        matches!(self.tiles.get(tile), Some(State::Failed(_)))
    }

    pub fn readiness(&self, tile: usize, now: f64) -> Readiness {
        match self.tiles.get(tile) {
            Some(State::Ready(_)) => Readiness::Ready,
            Some(State::Empty) => Readiness::Empty,
            Some(State::Failed(until)) if now < *until => Readiness::Unavailable,
            _ => Readiness::Pending,
        }
    }

    /// Marks the meshes of these tiles drawn in `frame`
    pub fn touch(&mut self, tiles: &[usize], frame: u64) {
        for &i in tiles {
            if let Some(State::Ready(mesh)) = self.tiles.get_mut(i) {
                mesh.touch(frame);
            }
        }
    }

    /// The requests of tiles that left the view, to abort: their tiles are unloaded again
    pub fn take_stale(&mut self, visible: &[usize], out: &mut Vec<Loading>) {
        self.visible.clear();
        self.visible.extend(visible.iter().copied());
        let stale: Vec<usize> = self
            .loading
            .iter()
            .copied()
            .filter(|i| !self.visible.contains(i))
            .collect();
        for i in stale {
            if let Some(State::Loading(loading)) = self.replace(i, State::Unloaded) {
                out.push(loading);
            }
        }
    }

    /// The tiles to request this frame, in order, into `out`: those of `request` while fewer than `max_requests`
    /// load, then, once every tile in view is asked, a few siblings out of view (`prefetch`)
    pub fn plan_requests(&self, request: &[usize], prefetch: &[usize], max_requests: usize, out: &mut Vec<usize>) {
        out.clear();
        let mut in_flight = self.loading.len();
        for &i in request {
            if in_flight >= max_requests {
                break;
            }
            if self.requestable(i) {
                out.push(i);
                in_flight += 1;
            }
        }
        // Siblings out of view: only with the visible tiles all requested, a few at a time (bandwidth stays for what
        // is on screen)
        if request.iter().all(|i| !self.requestable(*i) || out.contains(i)) {
            let prefetching = prefetch.iter().filter(|&&i| self.is_loading(i)).count();
            let free = PREFETCH_REQUESTS
                .saturating_sub(prefetching)
                .min(max_requests.saturating_sub(in_flight));
            let first = out.len();
            for &i in prefetch {
                if out.len() - first >= free {
                    break;
                }
                if matches!(self.tiles.get(i), Some(State::Unloaded)) && !out.contains(&i) {
                    out.push(i);
                }
            }
        }
    }

    /// Tiles to unload to bring the GPU memory back under 90 % of `budget`, least recently drawn first (never one
    /// drawn in `frame`); empty while under the budget
    pub fn over_budget(&self, budget: usize, frame: u64) -> Vec<usize> {
        if self.gpu_bytes <= budget {
            return Vec::new();
        }
        let mut candidates: Vec<(u64, usize, usize)> = self
            .tiles
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s {
                State::Ready(mesh) if mesh.used() < frame => Some((mesh.used(), i, mesh.bytes())),
                _ => None,
            })
            .collect();
        candidates.sort_unstable();
        let mut bytes = self.gpu_bytes;
        let mut out = Vec::new();
        for (_, i, size) in candidates {
            if bytes <= budget * 9 / 10 {
                break;
            }
            bytes -= size;
            out.push(i);
        }
        out
    }

    /// Takes every state out (the layer is deleted), leaving the tiles empty
    pub fn drain(&mut self) -> Vec<State<M>> {
        self.loading.clear();
        self.ready = 0;
        self.gpu_bytes = 0;
        std::mem::take(&mut self.tiles)
    }
}

/// Pause before attempt `attempt` (1 for the first retry) at the root tileset, None once the layer gives up
#[must_use]
pub fn root_retry_delay(attempt: u32) -> Option<f64> {
    (attempt < ROOT_ATTEMPTS).then(|| ROOT_RETRY_MS * f64::from(1u32 << (attempt.saturating_sub(1)).min(16)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mesh {
        bytes: usize,
        used: u64,
    }

    impl Resident for Mesh {
        fn bytes(&self) -> usize {
            self.bytes
        }
        fn used(&self) -> u64 {
            self.used
        }
        fn touch(&mut self, frame: u64) {
            self.used = frame;
        }
    }

    fn loading(id: u32) -> State<Mesh> {
        State::Loading(Loading { id, controller: None })
    }

    fn states(n: usize) -> States<Mesh> {
        let mut s = States::default();
        s.grow(n, |_| false);
        s
    }

    #[test]
    fn counts_follow_the_states() {
        let mut s = states(4);
        s.grow(6, |i| i == 5);
        assert_eq!(s.len(), 6);
        assert_eq!(s.readiness(5, 0.0), Readiness::Empty);
        s.replace(0, loading(1));
        s.replace(1, loading(2));
        assert_eq!(s.counts(), (2, 0, 0));
        assert!(s.is_current(0, 1) && !s.is_current(0, 2));
        s.replace(0, State::Ready(Mesh { bytes: 100, used: 0 }));
        assert_eq!(s.counts(), (1, 1, 100));
        assert_eq!(s.readiness(0, 0.0), Readiness::Ready);
        // A new request of a loading tile keeps it counted once
        s.replace(1, loading(3));
        assert_eq!(s.counts(), (1, 1, 100));
        s.replace(1, State::Failed(1000.0));
        assert_eq!(s.readiness(1, 500.0), Readiness::Unavailable);
        assert_eq!(s.readiness(1, 1500.0), Readiness::Pending);
        assert!(s.failed_before(1) && s.requestable(1));
        s.replace(0, State::Unloaded);
        assert_eq!(s.counts(), (0, 0, 0));
        assert!(s.replace(99, State::Empty).is_none());
    }

    #[test]
    fn stale_requests_are_taken_out() {
        let mut s = states(4);
        for i in 0..3 {
            s.replace(i, loading(i as u32 + 1));
        }
        let mut out = Vec::new();
        s.take_stale(&[1], &mut out);
        let mut ids: Vec<u32> = out.iter().map(|l| l.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, [1, 3]);
        assert_eq!(s.counts().0, 1);
        assert!(s.requestable(0) && s.is_loading(1));
    }

    #[test]
    fn requests_stay_within_the_limit_and_prefetch_last() {
        let mut s = states(10);
        s.replace(0, loading(1));
        let mut out = Vec::new();
        // One loading, three more allowed: tiles 1 to 3; tile 4 waits, so no prefetch
        s.plan_requests(&[1, 2, 3, 4], &[8, 9], 4, &mut out);
        assert_eq!(out, [1, 2, 3]);
        // Everything in view asked: siblings fill the room left
        s.plan_requests(&[0, 1], &[7, 8, 9], 4, &mut out);
        assert_eq!(out, [1, 7, 8]);
        // A ready tile is not asked again
        s.replace(1, State::Ready(Mesh { bytes: 1, used: 0 }));
        s.plan_requests(&[1], &[], 4, &mut out);
        assert_eq!(out, [] as [usize; 0]);
    }

    #[test]
    fn the_least_recently_drawn_go_first_beyond_the_budget() {
        let mut s = states(4);
        for (i, used) in [(0, 5), (1, 2), (2, 9), (3, 1)] {
            s.replace(i, State::Ready(Mesh { bytes: 100, used }));
        }
        assert_eq!(s.over_budget(400, 10), [] as [usize; 0]);
        // 400 bytes over a budget of 300: down to 270, the two oldest go
        assert_eq!(s.over_budget(300, 10), [3, 1]);
        // Tiles drawn this frame stay
        s.touch(&[3, 1], 10);
        assert_eq!(s.over_budget(300, 10), [0, 2]);
    }

    #[test]
    fn the_root_is_retried_a_few_times() {
        assert_eq!(root_retry_delay(1), Some(2_000.0));
        assert_eq!(root_retry_delay(2), Some(4_000.0));
        assert_eq!(root_retry_delay(3), Some(8_000.0));
        assert_eq!(root_retry_delay(ROOT_ATTEMPTS), None);
    }
}
