//! The nodes' requests: their chunks sent to the decoding workers with the grid that positions their points (made
//! here from the page's CRS), the nearest first, held to the server's rate by a token bucket; aborted when they leave
//! the selection

use super::{Arrival, Cloud, PointCloudLayer};
use crate::js;
use crate::layer::Frame;
use crate::render::tiles3d::fetch::js_get;
use crate::render::tiles3d::next_request;
use crate::render::tiles3d::state::{COOLDOWN_MS, Loading, State};
use geo::geodesy;
use glam::DVec3;
use pointcloud::copc::Content;
use pointcloud::frame::Grid;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// Requests of a frame fetched with a high priority
const URGENT_REQUESTS: usize = 4;

/// A token bucket: `rate` requests a second at most, a second's worth at once
#[derive(Clone, Copy, Debug, Default)]
pub struct Bucket {
    tokens: f64,
    /// When the tokens were last counted (ms)
    at: f64,
}

impl Bucket {
    /// Takes a token at `now` (ms), false if there is none left; no limit when `rate` is 0
    pub(super) fn take(&mut self, rate: f64, now: f64) -> bool {
        if rate <= 0.0 {
            return true;
        }

        let elapsed = (now - self.at).max(0.0) / 1000.0;

        self.tokens = (self.tokens + elapsed * rate).min(rate.max(1.0));
        self.at = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;

            true
        } else {
            false
        }
    }
}

/// Nodes along a side of the grid of a square of `side` metres: a node every 125 m at most (the interpolation then
/// within a millimetre), 2 to 9
#[must_use]
fn grid_side(side: f64) -> usize {
    ((side / 125.0).ceil() as usize + 1).clamp(2, 9)
}

impl PointCloudLayer {
    pub(super) fn abort(&self, loading: &Loading) {
        if let Some(loader) = &self.loader
            && let Ok(abort) = js_get(loader, "abort").dyn_into::<js_sys::Function>()
        {
            let _ = abort.call1(loader, &JsValue::from(loading.id));
        }
    }

    /// Asks the workers for a node's points: its chunk, how to decompress it, and the grid that positions them
    fn request(&mut self, node: usize, urgent: bool) -> Result<(), String> {
        let (Some(cloud), Some(loader)) = (&self.cloud, &self.loader) else {
            return Ok(());
        };
        let Content::Points { offset, size, count } = cloud.hierarchy.nodes[node].content else {
            return Ok(());
        };
        let (grid, anchor, up, base) = node_grid(cloud, node)?;
        let load: js_sys::Function = js_get(loader, "loadPoints")
            .dyn_into()
            .map_err(|_| "the decoding workers do not decode point clouds")?;
        let id = next_request();
        let args = js_sys::Array::new();

        for value in [
            JsValue::from(id),
            JsValue::from_str(&self.url),
            JsValue::from(offset as f64),
            JsValue::from(size),
            js_sys::Uint8Array::from(&cloud.copc.laz[..]).into(),
            js_sys::Float64Array::from(&cloud.layout.numbers()[..]).into(),
            js_sys::Float64Array::from(&grid.numbers()[..]).into(),
            JsValue::from(count),
            JsValue::from_str(if urgent { "high" } else { "low" }),
        ] {
            args.push(&value);
        }

        let promise: js_sys::Promise = load
            .apply(loader, &args)
            .map_err(|e| format!("{e:?}"))?
            .dyn_into()
            .map_err(|_| "loadPoints gave no promise")?;

        self.anchors.insert(node, (anchor, up, base));
        self.states
            .replace(node, State::Loading(Loading { id, controller: None }));

        let (inbox, redraw) = (self.inbox.clone(), self.redraw.clone());

        wasm_bindgen_futures::spawn_local(async move {
            let result = JsFuture::from(promise).await.map_err(|e| format!("{e:?}"));

            inbox.borrow_mut().push(Arrival::Points(node, id, result));
            redraw();
        });

        Ok(())
    }

    /// Requests held back by the rate: a frame once the next token is there
    pub(super) fn wake(&self, rate: f64) {
        if self.waking.replace(true) {
            return;
        }

        let (waking, redraw) = (self.waking.clone(), self.redraw.clone());
        let ms = (1000.0 / rate.max(0.1)).ceil() as i32;

        wasm_bindgen_futures::spawn_local(async move {
            js::sleep(ms).await;
            waking.set(false);
            redraw();
        });
    }

    /// Requests the nodes selected, the nearest first and as many as the frame allows within the GPU `budget`
    /// (bytes); whether some were held back by the rate
    pub(super) fn request_nodes(&mut self, frame: &Frame, rate: f64, budget: usize, now: f64) -> bool {
        let mut requests = Vec::new();
        let max_requests = (frame.loading.mesh_requests / 2).max(2);

        self.states
            .plan_requests(&self.selection.request, &[], max_requests, budget, &mut requests);
        for (rank, &node) in requests.iter().enumerate() {
            if !self.bucket.take(rate, now) {
                return true;
            }

            if let Err(e) = self.request(node, rank < URGENT_REQUESTS) {
                self.fail(&e);
                self.states.replace(node, State::Failed(now + COOLDOWN_MS));
            }
        }

        false
    }
}

/// The grid positioning a node's points, its anchor (the center of its cube), the vertical there and the altitude of
/// the anchor
fn node_grid(cloud: &Cloud, node: usize) -> Result<(Grid, DVec3, DVec3, f64), String> {
    let (min, side) = cloud.hierarchy.nodes[node].key.cube(&cloud.hierarchy.info);
    let k = grid_side(side * cloud.crs.metres);
    let (lo, hi) = ([min[0], min[1]], [min[0] + side, min[1] + side]);
    let nodes = Grid::nodes(lo, hi, k);
    let mut xy: Vec<f64> = nodes.iter().flatten().copied().collect();

    xy.extend([min[0] + side / 2.0, min[1] + side / 2.0]);

    let lonlat = cloud.crs.inverse(&xy)?;
    let (pairs, center) = lonlat.as_chunks::<2>().0.split_at(k * k);
    let [lon, lat] = center[0];
    let base = (min[2] + side / 2.0) * cloud.crs.vertical;
    let anchor = geodesy::to_ecef(lon, lat, cloud.crs.ground(lon, lat) + base);
    let up = geodesy::enu(lon, lat)[2];
    let grid = Grid {
        min: lo,
        max: hi,
        side: k,
        vertical_scale: cloud.crs.vertical,
        base: pairs
            .iter()
            .map(|&[lon, lat]| geodesy::to_ecef(lon, lat, cloud.crs.ground(lon, lat)) - anchor)
            .collect(),
        up: pairs.iter().map(|&[lon, lat]| geodesy::enu(lon, lat)[2]).collect(),
    };

    Ok((grid, anchor, up, base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bucket_holds_requests_to_their_rate() {
        let mut b = Bucket::default();
        // A second's worth at once, then one every 125 ms at 8 a second
        let first: usize = (0..20).filter(|_| b.take(8.0, 1000.0)).count();

        assert_eq!(first, 8);
        assert!(!b.take(8.0, 1100.0));
        assert!(b.take(8.0, 1130.0));
        assert!(!b.take(8.0, 1130.0));
        // No limit
        assert!((0..100).all(|_| b.take(0.0, 0.0)));
    }

    #[test]
    fn grids_get_a_node_every_125_metres() {
        assert_eq!(grid_side(10.0), 2);
        assert_eq!(grid_side(250.0), 3);
        assert_eq!(grid_side(1000.0), 9);
        assert_eq!(grid_side(50_000.0), 9);
    }
}
