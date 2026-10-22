//! The head and the hierarchy pages of a COPC file: fetched by range requests (the head retried after a network
//! error, the pages after their cooldown), parsed by `pointcloud::copc`, the CRS asked of the page (`resolve`) and the
//! nodes of each page placed by it

use super::{Arrival, Cloud, PointCloudLayer};
use crate::js;
use crate::render::tiles3d::fetch::{fetch_range, js_get};
use crate::render::tiles3d::state::{COOLDOWN_MS, State, root_retry_delay};
use geo::{geodesy, geoid};
use glam::DVec3;
use pointcloud::copc::{Content, Copc, Head, Hierarchy, MAX_PAGE, parse_page};
use pointcloud::points::Layout;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// Bytes asked for first: the header and the VLRs of a COPC file (1.5 KB with a WKT), its info VLR at 375
pub(super) const HEAD_BYTES: u64 = 65_536;
/// The most the VLRs may hold
const MAX_HEAD: usize = 16 << 20;

/// What the page says of the CRS of a file
pub(super) struct Crs {
    /// x, y pairs of the CRS to longitudes, latitudes (degrees): `(xy: Float64Array) => Float64Array`
    inverse: js_sys::Function,
    /// Altitudes above the geoid (IGN69): raised by the RAF20 geoid height to ellipsoidal heights
    geoid: bool,
    /// Metres of a unit of z, and of x and y (measured on the projection at the center)
    pub(super) vertical: f64,
    pub(super) metres: f64,
}

impl Crs {
    /// Longitudes and latitudes of x, y pairs
    pub(super) fn inverse(&self, xy: &[f64]) -> Result<Vec<f64>, String> {
        let out = self
            .inverse
            .call1(&JsValue::NULL, &js_sys::Float64Array::from(xy))
            .map_err(|e| format!("{e:?}"))?;
        let out: js_sys::Float64Array = out.dyn_into().map_err(|_| "the CRS's inverse gave no Float64Array")?;
        let out = out.to_vec();

        if out.len() != xy.len() || !out.iter().all(|v| v.is_finite()) {
            return Err("the CRS's inverse gave positions that are not numbers".into());
        }

        Ok(out)
    }

    /// The ground (the geoid, or the ellipsoid) under a longitude and latitude
    pub(super) fn ground(&self, lon: f64, lat: f64) -> f64 {
        if self.geoid { geoid::height(lon, lat) } else { 0.0 }
    }
}

impl PointCloudLayer {
    pub(super) fn fetch_head(&self, length: u64, delay_ms: f64) {
        let (inbox, redraw, url) = (self.inbox.clone(), self.redraw.clone(), self.url.clone());

        wasm_bindgen_futures::spawn_local(async move {
            if delay_ms > 0.0 {
                js::sleep(delay_ms as i32).await;
            }

            let bytes = fetch_range(&url, 0, length, "high").await;

            inbox.borrow_mut().push(Arrival::Head(bytes));
            redraw();
        });
    }

    fn fetch_page(&mut self, node: usize) {
        let Some(page) = self.cloud.as_ref().and_then(|c| c.hierarchy.page(node)) else {
            return;
        };

        self.pages.insert(node, None);

        let (inbox, redraw, url) = (self.inbox.clone(), self.redraw.clone(), self.url.clone());

        wasm_bindgen_futures::spawn_local(async move {
            let bytes = fetch_range(&url, page.offset, page.size, "high").await;

            inbox.borrow_mut().push(Arrival::Page(node, bytes));
            redraw();
        });
    }

    /// The head arrived: the COPC info, the CRS from the page, the root placed
    fn read_head(&mut self, bytes: &[u8]) -> Result<(), String> {
        let copc = match Copc::parse(bytes)? {
            Head::Ready(copc) => *copc,
            Head::More(n) if n <= MAX_HEAD && bytes.len() < n => {
                self.fetch_head(n as u64, 0.0);

                return Ok(());
            }
            Head::More(_) => return Err("VLRs too large".into()),
        };
        let wkt = copc.wkt.as_deref().map_or(JsValue::UNDEFINED, JsValue::from_str);
        let resolved = self.resolve.call1(&JsValue::NULL, &wkt).map_err(|e| {
            e.dyn_ref::<js_sys::Error>()
                .map_or_else(|| format!("{e:?}"), |e| String::from(e.message()))
        })?;
        let inverse: js_sys::Function = js_get(&resolved, "inverse")
            .dyn_into()
            .map_err(|_| "the CRS has no inverse")?;
        let vertical = js_get(&resolved, "verticalScale")
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0);
        let mut crs = Crs {
            inverse,
            geoid: js_get(&resolved, "geoid").is_truthy(),
            vertical: vertical.unwrap_or(1.0),
            metres: 1.0,
        };
        // Metres of a unit of the CRS: the distance between the center and a point a half-side east of it
        let info = copc.info;
        let [cx, cy, _] = info.center;
        let lonlat = crs.inverse(&[cx, cy, cx + info.halfsize, cy])?;
        let a = geodesy::to_ecef(lonlat[0], lonlat[1], 0.0);
        let b = geodesy::to_ecef(lonlat[2], lonlat[3], 0.0);

        crs.metres = a.distance(b) / info.halfsize;
        if !(crs.metres.is_finite() && crs.metres > 0.0) {
            return Err("the CRS does not place the points".into());
        }

        let h = &copc.header;
        let layout = Layout {
            format: h.point_format,
            record_length: usize::from(h.record_length),
            scale: h.scale,
            offset: h.offset,
        };
        let mut cloud = Cloud {
            hierarchy: Hierarchy::new(info),
            copc,
            crs,
            layout,
        };

        place(&mut cloud, &[0])?;
        self.states.grow(1, |_| true);
        self.cloud = Some(cloud);

        Ok(())
    }

    /// A page arrived: its nodes added and placed
    fn read_page(&mut self, node: usize, bytes: &[u8]) -> Result<(), String> {
        let Some(cloud) = &mut self.cloud else {
            return Ok(());
        };
        let added = cloud.hierarchy.add_page(node, parse_page(bytes)?)?;

        place(cloud, &added)?;

        let nodes = &cloud.hierarchy.nodes;

        // The page's node may now hold points
        if matches!(nodes[node].content, Content::Points { .. }) && matches!(self.states.get(node), Some(State::Empty))
        {
            self.states.replace(node, State::Unloaded);
        }
        self.states
            .grow(nodes.len(), |i| !matches!(nodes[i].content, Content::Points { .. }));

        Ok(())
    }

    /// The head arrived: read, or given up (not COPC, a CRS the page does not know), or asked again after a network
    /// error while attempts remain
    pub(super) fn receive_head(&mut self, bytes: Result<Vec<u8>, String>) {
        let attempt = self.head.unwrap_or(0);

        match bytes {
            Ok(bytes) => {
                if let Err(e) = self.read_head(&bytes) {
                    self.fail(&e);
                    self.head = None;
                }
            }
            Err(e) => {
                if let Some(ms) = root_retry_delay(attempt + 1) {
                    self.fail(&format!("{e}; retried in {} s", ms / 1000.0));
                    self.head = Some(attempt + 1);
                    self.fetch_head(HEAD_BYTES, ms);
                } else {
                    self.fail(&e);
                    self.head = None;
                }
            }
        }
    }

    /// A hierarchy page arrived: read, or asked again after the cooldown (`now`, ms)
    pub(super) fn receive_page(&mut self, node: usize, bytes: Result<Vec<u8>, String>, now: f64) {
        match bytes.and_then(|b| self.read_page(node, &b)) {
            Ok(()) => {
                self.pages.remove(&node);
            }
            Err(e) => {
                self.fail(&format!("hierarchy page: {e}"));
                self.pages.insert(node, Some(now + COOLDOWN_MS));
            }
        }
    }

    /// Fetches the hierarchy pages selected, but those cooling down or too large; whether some were held back by the
    /// rate (requests a second)
    pub(super) fn request_pages(&mut self, rate: f64, now: f64) -> bool {
        // Taken for the loop (fetching borrows the layer), put back after
        let pages = std::mem::take(&mut self.selection.pages);
        let mut held = false;

        for &node in &pages {
            let cooling = self.pages.get(&node).is_some_and(|until| until.is_none_or(|t| now < t));
            let fits = self
                .cloud
                .as_ref()
                .and_then(|c| c.hierarchy.page(node))
                .is_some_and(|p| p.size <= MAX_PAGE);

            if !cooling && fits {
                if self.bucket.take(rate, now) {
                    self.fetch_page(node);
                } else {
                    held = true;
                }
            }
        }
        self.selection.pages = pages;

        held
    }
}

/// Places nodes: the longitude, latitude and ellipsoidal height of their centers, from the page's CRS
fn place(cloud: &mut Cloud, nodes: &[usize]) -> Result<(), String> {
    if nodes.is_empty() {
        return Ok(());
    }

    let centers: Vec<[f64; 3]> = nodes.iter().map(|&i| cloud.hierarchy.center(i)).collect();
    let xy: Vec<f64> = centers.iter().flat_map(|c| [c[0], c[1]]).collect();
    let lonlat = cloud.crs.inverse(&xy)?;

    for ((&i, c), ll) in nodes.iter().zip(&centers).zip(lonlat.as_chunks::<2>().0) {
        let h = cloud.crs.ground(ll[0], ll[1]) + c[2] * cloud.crs.vertical;

        cloud
            .hierarchy
            .place(i, DVec3::new(ll[0], ll[1], h), cloud.crs.metres, cloud.crs.vertical);
    }

    Ok(())
}
