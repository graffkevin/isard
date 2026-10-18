//! The COPC layer: a point cloud read from one file by HTTP range requests. Its head (header, VLRs) first, then the
//! pages of its hierarchy as the traversal reaches them, both fetched here and parsed by `pointcloud::copc`; the
//! chunks of the nodes in view go to the decoding workers, which decompress them with the point cloud module and
//! position their points through the grid made here (`pointcloud::frame`), and come back as vertices, uploaded a few
//! per frame. The CRS is the page's (`resolve`: the WKT of the file to the inverse projection, from proj4 or built in),
//! the altitudes raised by the RAF20 geoid where they are above it (IGN69). Nodes are kept within a GPU memory
//! budget (least recently drawn unloaded first), their downloads aborted when they leave the selection; requests are
//! held to a rate when the server limits it (the Géoplateforme: 10 a second).

use super::{ColorBy, Draw, Edl, GpuPoints, Look, PointProgram};
use crate::engine::Engine;
use crate::js;
use crate::layer::{Frame, Layer};
use crate::render::tiles3d::fetch::{fetch_range, js_get};
use crate::render::tiles3d::next_request;
use crate::render::tiles3d::state::{COOLDOWN_MS, Loading, Resident, State, States, root_retry_delay};
use geo::camera::View;
use geo::{geodesy, geoid};
use glam::DVec3;
use pointcloud::copc::{Content, Copc, Head, Hierarchy, MAX_PAGE, parse_page};
use pointcloud::frame::Grid;
use pointcloud::points::{INTENSITY, Layout, RETURNS, STRIDE};
use pointcloud::traversal::{Readiness, Selection, select};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// Bytes asked for first: the header and the VLRs of a COPC file (1.5 KB with a WKT), its info VLR at 375
const HEAD_BYTES: u64 = 65_536;
/// The most the VLRs may hold
const MAX_HEAD: usize = 16 << 20;
/// Requests of a frame fetched with a high priority
const URGENT_REQUESTS: usize = 4;
/// Ids of the picking pass: 24 bits, 0 for none
const MAX_PICK: u32 = (1 << 24) - 1;

/// What the page says of the CRS of a file
struct Crs {
    /// x, y pairs of the CRS to longitudes, latitudes (degrees): `(xy: Float64Array) => Float64Array`
    inverse: js_sys::Function,
    /// Altitudes above the geoid (IGN69): raised by the RAF20 geoid height to ellipsoidal heights
    geoid: bool,
    /// Metres of a unit of z, and of x and y (measured on the projection at the center)
    vertical: f64,
    metres: f64,
}

impl Crs {
    /// Longitudes and latitudes of x, y pairs
    fn inverse(&self, xy: &[f64]) -> Result<Vec<f64>, String> {
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
    fn ground(&self, lon: f64, lat: f64) -> f64 {
        if self.geoid { geoid::height(lon, lat) } else { 0.0 }
    }
}

/// A point cloud once its head is read
struct Cloud {
    copc: Copc,
    hierarchy: Hierarchy,
    crs: Crs,
    layout: Layout,
}

/// The points of a node on the GPU, and where they are: the anchor of their positions, the vertical there, the
/// altitude of the anchor (CRS z, metres)
pub struct NodePoints {
    points: GpuPoints,
    anchor: DVec3,
    up: DVec3,
    base: f64,
    used: u64,
}

impl Resident for NodePoints {
    fn bytes(&self) -> usize {
        self.points.bytes
    }

    fn used(&self) -> u64 {
        self.used
    }

    fn touch(&mut self, frame: u64) {
        self.used = frame;
    }
}

enum Arrival {
    Head(Result<Vec<u8>, String>),
    Page(usize, Result<Vec<u8>, String>),
    /// A node's vertices (a `Uint8Array`) by its request number
    Points(usize, u32, Result<JsValue, String>),
}

/// The style of a point cloud, from the page (`setPointCloudStyle`)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// None: by RGB when the file has colors, else by class
    pub color_by: Option<ColorBy>,
    pub size: f64,
    pub max_size: f64,
    pub adaptive: bool,
    pub round: bool,
    /// Eye-Dome Lighting: strength (0: off) and radius (CSS pixels)
    pub edl: (f64, f64),
    /// Altitudes and intensities mapped onto the ramp: NaN for the file's (the header's z, the root's intensities)
    pub elevation: [f64; 2],
    pub intensity: [f64; 2],
    pub max_error: f64,
    pub budget: u64,
    /// Requests a second at most, 0 for no limit
    pub rate: f64,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color_by: None,
            size: 2.0,
            max_size: 4.0,
            adaptive: true,
            round: true,
            edl: (0.0, 1.4),
            elevation: [f64::NAN; 2],
            intensity: [f64::NAN; 2],
            max_error: 1.5,
            budget: 3_000_000,
            rate: 0.0,
        }
    }
}

impl Style {
    /// The style of the page's numbers: color by (-1 auto, then `ColorBy`), size, max size, adaptive (0, 1), round
    /// (0, 1), EDL strength and radius, elevation min and max, intensity min and max (NaN: the file's), max screen
    /// space error, point budget, requests a second; a number missing or out of its range keeps the default
    #[must_use]
    pub fn parse(n: &[f64]) -> Self {
        let d = Self::default();
        let at = |i: usize| n.get(i).copied().unwrap_or(f64::NAN);
        let positive = |i: usize, default: f64| Some(at(i)).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(default);
        Self {
            color_by: Some(at(0)).filter(|v| *v >= 0.0).map(ColorBy::from_number),
            size: positive(1, d.size).min(64.0),
            max_size: positive(2, d.max_size).min(64.0),
            adaptive: at(3) != 0.0,
            round: at(4) != 0.0,
            edl: (
                Some(at(5)).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0),
                positive(6, d.edl.1).min(8.0),
            ),
            elevation: [at(7), at(8)],
            intensity: [at(9), at(10)],
            max_error: positive(11, d.max_error),
            budget: positive(12, d.budget as f64).min(1e9) as u64,
            rate: Some(at(13)).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0),
        }
    }
}

/// A token bucket: `rate` requests a second at most, a second's worth at once
#[derive(Clone, Copy, Debug, Default)]
pub struct Bucket {
    tokens: f64,
    /// When the tokens were last counted (ms)
    at: f64,
}

impl Bucket {
    /// Takes a token at `now` (ms), false if there is none left; no limit when `rate` is 0
    pub fn take(&mut self, rate: f64, now: f64) -> bool {
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
pub fn grid_side(side: f64) -> usize {
    ((side / 125.0).ceil() as usize + 1).clamp(2, 9)
}

/// The 2nd and 98th percentiles of the intensities of vertices (to 16 units): the range the intensities are mapped on
#[must_use]
pub fn intensity_range(vertices: &[u8]) -> Option<[f64; 2]> {
    let mut histogram = vec![0u32; 4096];
    let mut count = 0u32;
    for v in vertices.as_chunks::<STRIDE>().0 {
        histogram[usize::from(u16::from_le_bytes([v[INTENSITY], v[INTENSITY + 1]]) >> 4)] += 1;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let at = |q: f64| {
        let target = (f64::from(count - 1) * q).round() as u32;
        let mut seen = 0;
        let bin = histogram.iter().position(|&n| {
            seen += n;
            seen > target
        });
        f64::from(bin.unwrap_or(0) as u32 * 16)
    };
    let (lo, hi) = (at(0.02), at(0.98));
    Some([lo, if hi > lo { hi } else { lo + 16.0 }])
}

#[derive(Default, Clone, Copy)]
pub struct PointCloudStats {
    pub points: u64,
    pub nodes: usize,
    pub loaded: usize,
    pub in_flight: usize,
    pub gpu_bytes: usize,
    pub known: usize,
}

pub struct PointCloudLayer {
    url: String,
    /// The page's CRS for a WKT: `(wkt: string | undefined) => { inverse, geoid, verticalScale }`
    resolve: js_sys::Function,
    /// Attempts at the head so far, None once given up
    head: Option<u32>,
    cloud: Option<Cloud>,
    states: States<NodePoints>,
    /// Where each node requested is: its anchor, vertical and altitude, for its upload
    anchors: HashMap<usize, (DVec3, DVec3, f64)>,
    /// Pages loading, and pages that failed (asked again after this time, ms)
    pages: HashMap<usize, Option<f64>>,
    inbox: Rc<RefCell<Vec<Arrival>>>,
    /// Arrived, waiting for their upload
    queue: VecDeque<(usize, JsValue)>,
    loader: Option<JsValue>,
    program: PointProgram,
    edl: Option<Edl>,
    pub style: Style,
    /// The intensities of the first node, when the page gives no range
    auto_intensity: Option<[f64; 2]>,
    bucket: Bucket,
    /// A frame is asked for once the bucket has a token again (requests held back)
    waking: Rc<std::cell::Cell<bool>>,
    selection: Selection,
    pub stats: PointCloudStats,
    /// The nodes of the last picking pass and the id of their first point
    picked: RefCell<Vec<(usize, u32)>>,
    redraw: Rc<dyn Fn()>,
}

impl PointCloudLayer {
    pub fn new(
        gl: &glow::Context,
        url: String,
        resolve: js_sys::Function,
        loader: Option<JsValue>,
        redraw: &Rc<dyn Fn()>,
    ) -> Result<Self, String> {
        let mut layer = Self {
            url,
            resolve,
            head: Some(0),
            cloud: None,
            states: States::default(),
            anchors: HashMap::new(),
            pages: HashMap::new(),
            inbox: Rc::default(),
            queue: VecDeque::new(),
            loader,
            program: PointProgram::new(gl)?,
            edl: None,
            style: Style::default(),
            auto_intensity: None,
            bucket: Bucket::default(),
            waking: Rc::default(),
            selection: Selection::default(),
            stats: PointCloudStats::default(),
            picked: RefCell::default(),
            redraw: redraw.clone(),
        };
        if layer.loader.is_none() {
            layer.fail("point clouds are decoded by the decoding workers: `workers: false` leaves them out");
            layer.head = None;
            return Ok(layer);
        }
        layer.fetch_head(HEAD_BYTES, 0.0);
        Ok(layer)
    }

    fn fail(&self, error: &str) {
        web_sys::console::error_1(&JsValue::from_str(&format!("Point cloud {}: {error}", self.url)));
    }

    fn fetch_head(&self, length: u64, delay_ms: f64) {
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

    fn receive(&mut self, gl: &glow::Context, raw: &web_sys::WebGl2RenderingContext, frame: u64, budget_ms: f64) {
        let now = js_sys::Date::now();
        let arrivals: Vec<_> = self.inbox.borrow_mut().drain(..).collect();
        for arrival in arrivals {
            match arrival {
                Arrival::Head(bytes) => {
                    let attempt = self.head.unwrap_or(0);
                    match bytes {
                        Ok(bytes) => {
                            // A file that is not COPC, or a CRS the page does not know: given up
                            if let Err(e) = self.read_head(&bytes) {
                                self.fail(&e);
                                self.head = None;
                            }
                        }
                        // A network error is tried again
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
                Arrival::Page(node, bytes) => match bytes.and_then(|b| self.read_page(node, &b)) {
                    Ok(()) => {
                        self.pages.remove(&node);
                    }
                    Err(e) => {
                        self.fail(&format!("hierarchy page: {e}"));
                        self.pages.insert(node, Some(now + COOLDOWN_MS));
                    }
                },
                Arrival::Points(node, id, result) if self.states.is_current(node, id) => match result {
                    Ok(vertices) => self.queue.push_back((node, vertices)),
                    Err(_) => {
                        self.states.replace(node, State::Failed(now + COOLDOWN_MS));
                    }
                },
                Arrival::Points(..) => {}
            }
        }
        let start = js::now();
        let mut uploaded = 0;
        while uploaded == 0 || js::now() - start < budget_ms {
            let Some((node, vertices)) = self.queue.pop_front() else {
                break;
            };
            uploaded += 1;
            if !matches!(self.states.get(node), Some(State::Loading(_))) {
                continue;
            }
            let Ok(array) = vertices.dyn_into::<js_sys::Uint8Array>() else {
                self.states.replace(node, State::Failed(now + COOLDOWN_MS));
                continue;
            };
            if self.auto_intensity.is_none() {
                self.auto_intensity = intensity_range(&array.to_vec());
            }
            let Some(&(anchor, up, base)) = self.anchors.get(&node) else {
                continue;
            };
            match GpuPoints::upload_js(gl, raw, &array) {
                Ok(points) => {
                    self.states.replace(
                        node,
                        State::Ready(NodePoints {
                            points,
                            anchor,
                            up,
                            base,
                            used: frame,
                        }),
                    );
                }
                Err(e) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!("Point cloud upload: {e:?}")));
                    self.states.replace(node, State::Failed(now + COOLDOWN_MS));
                }
            }
        }
    }

    fn abort(&self, loading: &Loading) {
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
    fn wake(&self, rate: f64) {
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

    fn look(&self) -> Look {
        let s = &self.style;
        let rgb = self.cloud.as_ref().is_some_and(|c| c.layout.format >= 7);
        let color_by = s
            .color_by
            .unwrap_or(if rgb { ColorBy::Rgb } else { ColorBy::Classification });
        let file = |range: [f64; 2], fallback: [f64; 2]| {
            if range.iter().all(|v| v.is_finite()) {
                range
            } else {
                fallback
            }
        };
        let range = match color_by {
            ColorBy::Elevation => {
                let z = self.cloud.as_ref().map_or([0.0, 100.0], |c| {
                    let h = &c.copc.header;
                    [h.min[2] * c.crs.vertical, h.max[2] * c.crs.vertical]
                });
                file(s.elevation, z)
            }
            ColorBy::Intensity => file(s.intensity, self.auto_intensity.unwrap_or([0.0, 65535.0])),
            _ => [0.0, 1.0],
        };
        Look {
            color_by,
            size: s.size,
            max_size: s.max_size,
            adaptive: s.adaptive,
            round: s.round,
            range,
            classes: true,
            lit: false,
        }
    }

    /// The nodes selected and ready, and their draws
    fn draws<'a>(&'a self, view: &View) -> (Vec<usize>, Vec<Draw<'a>>) {
        let Some(cloud) = &self.cloud else {
            return (Vec::new(), Vec::new());
        };
        let info = &cloud.hierarchy.info;
        self.selection
            .draw
            .iter()
            .filter_map(|&(i, level)| {
                let Some(State::Ready(node)) = self.states.get(i) else {
                    return None;
                };
                let lat = cloud.hierarchy.nodes[i].geodetic.y;
                let spacing = info.spacing * cloud.crs.metres / f64::from(1u32 << level.min(31));
                Some((
                    i,
                    Draw {
                        points: &node.points,
                        matrix: view.view_proj * view.ecef_matrix(node.anchor),
                        up: node.up,
                        base: node.base,
                        spacing: spacing * view.pixels_per_metre(lat) * view.center_distance,
                    },
                ))
            })
            .unzip()
    }

    fn draw_cloud(&mut self, frame: &Frame) -> bool {
        let (gl, view) = (frame.gl, frame.view);
        self.receive(gl, frame.raw, frame.number, frame.loading.upload_budget_ms);
        let waiting = self.head.is_some() && self.cloud.is_none();
        let Some(cloud) = &self.cloud else {
            return waiting && self.head == Some(0);
        };
        let now = js_sys::Date::now();
        let states = &self.states;
        let ready = |i: usize| match states.get(i) {
            Some(State::Ready(_)) => Readiness::Ready,
            Some(State::Failed(until)) if now < *until => Readiness::Unavailable,
            _ => Readiness::Pending,
        };
        let mut selection = std::mem::take(&mut self.selection);
        select(
            &cloud.hierarchy,
            view,
            self.style.max_error,
            self.style.budget,
            &ready,
            &mut selection,
        );
        let mut stale = Vec::new();
        self.states.take_stale(&selection.visible, &mut stale);
        for loading in &stale {
            self.abort(loading);
        }
        // Pages first: the nodes under them wait for them
        let rate = self.style.rate;
        let mut held = false;
        for &node in &selection.pages {
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
        let mut requests = Vec::new();
        let max_requests = (frame.loading.mesh_requests / 2).max(2);
        let budget = gpu_budget(self.style.budget);
        self.states
            .plan_requests(&selection.request, &[], max_requests, budget, &mut requests);
        for (rank, &node) in requests.iter().enumerate() {
            if !self.bucket.take(rate, now) {
                held = true;
                break;
            }
            if let Err(e) = self.request(node, rank < URGENT_REQUESTS) {
                self.fail(&e);
                self.states.replace(node, State::Failed(now + COOLDOWN_MS));
            }
        }
        let drawn: Vec<usize> = selection.draw.iter().map(|&(i, _)| i).collect();
        self.states.touch(&drawn, frame.number);
        for i in self.states.over_budget(budget, frame.number) {
            if let Some(State::Ready(node)) = self.states.replace(i, State::Unloaded) {
                node.points.delete(gl);
            }
        }
        self.selection = selection;
        self.render(frame);
        let (in_flight, loaded, gpu_bytes) = self.states.counts();
        self.stats = PointCloudStats {
            points: self
                .selection
                .draw
                .iter()
                .filter_map(|&(i, _)| match self.states.get(i) {
                    Some(State::Ready(n)) => Some(n.points.count as u64),
                    _ => None,
                })
                .sum(),
            nodes: self.selection.draw.len(),
            loaded,
            in_flight,
            gpu_bytes,
            known: self.cloud.as_ref().map_or(0, |c| c.hierarchy.nodes.len()),
        };
        if !self.queue.is_empty() {
            (self.redraw)();
        }
        if held {
            self.wake(rate);
        }
        held || in_flight > 0 || !self.queue.is_empty() || self.pages.values().any(Option::is_none)
    }

    fn render(&mut self, frame: &Frame) {
        let (gl, view) = (frame.gl, frame.view);
        let look = self.look();
        let (strength, radius) = self.style.edl;
        if strength > 0.0 && self.edl.is_none() {
            match Edl::new(gl) {
                Ok(edl) => self.edl = Some(edl),
                Err(e) => {
                    self.fail(&format!("Eye-Dome Lighting: {e}"));
                    self.style.edl.0 = 0.0;
                }
            }
        }
        // With EDL, into its framebuffer at the size of the canvas, then laid over the frame
        let edl = strength > 0.0 && self.edl.is_some();
        if edl && let Some(pass) = self.edl.as_mut() {
            let size = (
                (frame.camera.width * frame.pixel_ratio).round() as i32,
                (frame.camera.height * frame.pixel_ratio).round() as i32,
            );
            pass.begin(gl, size);
        }
        let (_, draws) = self.draws(view);
        self.program.draw(gl, frame.pixel_ratio, &look, &draws, None);
        if edl && let Some(pass) = &self.edl {
            pass.finish(gl, view, strength, radius * frame.pixel_ratio);
        }
    }

    /// The point of a picked id: its node and index there
    fn picked_point(&self, index: u32) -> Option<(usize, u32)> {
        let picked = self.picked.borrow();
        let k = picked.partition_point(|&(_, first)| first <= index);
        let &(node, first) = picked.get(k.checked_sub(1)?)?;
        Some((node, index - first))
    }

    /// The attributes of a picked point, read back from the GPU
    fn point(&self, gl: &glow::Context, index: u32) -> Option<JsValue> {
        let (node, i) = self.picked_point(index)?;
        let cloud = self.cloud.as_ref()?;
        let Some(State::Ready(n)) = self.states.get(node) else {
            return None;
        };
        let v = n.points.read(gl, i)?;
        let f = |at: usize| f64::from(f32::from_le_bytes([v[at], v[at + 1], v[at + 2], v[at + 3]]));
        let offset = DVec3::new(f(0), f(4), f(8));
        let g = geodesy::from_ecef(n.anchor + offset);
        let mut entries = vec![
            ("lonLat", js::pair(g.x, g.y)),
            ("height", g.z.into()),
            ("altitude", (n.base + offset.dot(n.up)).into()),
            ("classification", v[15].into()),
            ("intensity", u16::from_le_bytes([v[INTENSITY], v[INTENSITY + 1]]).into()),
            ("returnNumber", v[RETURNS].into()),
            ("numberOfReturns", v[RETURNS + 1].into()),
        ];
        if cloud.layout.format >= 7 {
            let rgb = js_sys::Array::of3(&v[12].into(), &v[13].into(), &v[14].into());
            entries.push(("rgb", rgb.into()));
        }
        Some(js::object(&entries))
    }
}

/// The GPU memory the nodes may take: twice the point budget's vertices, 64 MB at least
fn gpu_budget(points: u64) -> usize {
    ((points as usize).saturating_mul(STRIDE * 2)).max(64 << 20)
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

impl Layer for PointCloudLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        self.draw_cloud(frame)
    }

    fn pickable(&self) -> bool {
        !self.selection.draw.is_empty()
    }

    fn draw_pick(&self, frame: &Frame, layer: u8) {
        let (nodes, draws) = self.draws(frame.view);
        // Each point an id of its own: the nodes numbered in turn, as many as 24 bits hold
        let mut picked = Vec::new();
        let mut first = 1u32;
        for (&i, d) in nodes.iter().zip(&draws) {
            let count = d.points.count as u32;
            if first.saturating_add(count) > MAX_PICK {
                break;
            }
            picked.push((i, first - 1));
            first += count;
        }
        let firsts: Vec<u32> = picked.iter().map(|&(_, f)| f).collect();
        self.program.draw(
            frame.gl,
            frame.pixel_ratio,
            &self.look(),
            &draws[..firsts.len()],
            Some((layer, &firsts)),
        );
        *self.picked.borrow_mut() = picked;
    }

    fn delete(&mut self, gl: &glow::Context) {
        for state in self.states.drain() {
            match state {
                State::Loading(loading) => self.abort(&loading),
                State::Ready(node) => node.points.delete(gl),
                _ => {}
            }
        }
        self.queue.clear();
        self.program.delete(gl);
        if let Some(edl) = &self.edl {
            edl.delete(gl);
        }
    }
}

impl Drop for PointCloudLayer {
    /// Stops the downloads
    fn drop(&mut self) {
        for i in 0..self.states.len() {
            if let Some(State::Loading(loading)) = self.states.get(i) {
                self.abort(loading);
            }
        }
    }
}

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r"
/** Counters of a point cloud layer */
export interface PointCloudStats {
  /** Points drawn in the last frame */
  points: number;
  /** Nodes drawn in the last frame */
  nodes: number;
  /** Nodes held on the GPU */
  loaded: number;
  /** Downloads under way */
  inFlight: number;
  /** GPU memory of the points, MB */
  gpuMb: number;
  /** Nodes known in the hierarchy (its pages loaded so far) */
  known: number;
}

/** What a point cloud says of itself once its head is read */
export interface PointCloudInfo {
  /** Points in the file */
  points: number;
  /** LAS point format (6 to 8: 7 and 8 with colors) */
  format: number;
  /** The WKT of its CRS, if the file has one */
  wkt?: string;
  /** The center of its cube: longitude, latitude */
  center: [lon: number, lat: number];
  /** Its altitudes, lowest and highest (metres) */
  altitudes: [min: number, max: number];
}

/** A point picked in a point cloud */
export interface PointCloudPoint {
  lonLat: [lon: number, lat: number];
  /** Metres above the ellipsoid */
  height: number;
  /** Its z in the file's vertical datum (metres) */
  altitude: number;
  classification: number;
  intensity: number;
  returnNumber: number;
  numberOfReturns: number;
  /** 8-bit color, files with colors only */
  rgb?: [r: number, g: number, b: number];
}

/** The CRS of a point cloud, as the page resolves it from the file's WKT (`addPointCloud`) */
export interface PointCloudCrs {
  /** x, y pairs of the CRS to longitudes, latitudes (degrees) */
  inverse(xy: Float64Array): Float64Array;
  /** The altitudes are above the geoid (IGN69): raised by the RAF20 geoid height */
  geoid: boolean;
  /** Metres of a unit of z */
  verticalScale: number;
}
";

#[wasm_bindgen]
impl Engine {
    /// A COPC point cloud from its URL (HTTP range requests). `resolve(wkt)` gives the CRS of the file from its WKT
    /// (undefined if it has none): its inverse projection, whether its altitudes are above the geoid, the metres of
    /// a unit of z; it may throw (an unknown CRS), the layer then stays empty
    #[wasm_bindgen(js_name = addPointCloud)]
    pub fn add_point_cloud(
        &mut self,
        url: String,
        #[wasm_bindgen(unchecked_param_type = "(wkt: string | undefined) => PointCloudCrs")] resolve: js_sys::Function,
    ) -> Result<u32, JsValue> {
        let layer = PointCloudLayer::new(&self.gl, url, resolve, self.loader.clone(), &self.redraw)?;
        Ok(self.add_layer(Box::new(layer)))
    }

    /// The style of a point cloud: 14 numbers (see `Style::parse`), the colors of the 256 classes (RGBA,
    /// alpha 0 hides a class) and the ramp of the altitudes (256 RGBA texels)
    #[wasm_bindgen(js_name = setPointCloudStyle)]
    pub fn set_point_cloud_style(&mut self, id: u32, style: &[f64], palette: &[u8], ramp: &[u8]) {
        if let Some((layer, gl)) = self.layer_mut::<PointCloudLayer>(id) {
            layer.style = Style::parse(style);
            layer.program.set_palette(gl, palette);
            layer.program.set_ramp(gl, ramp);
            (self.redraw)();
        }
    }

    /// Counters of a point cloud layer, undefined if there is none with this id
    #[wasm_bindgen(js_name = pointCloudStats, unchecked_return_type = "PointCloudStats | undefined")]
    #[must_use]
    pub fn point_cloud_stats(&self, id: u32) -> JsValue {
        self.layer::<PointCloudLayer>(id).map_or(JsValue::UNDEFINED, |layer| {
            let s = layer.stats;
            js::object(&[
                ("points", (s.points as f64).into()),
                ("nodes", s.nodes.into()),
                ("loaded", s.loaded.into()),
                ("inFlight", s.in_flight.into()),
                ("gpuMb", (s.gpu_bytes as f64 / 1_048_576.0).into()),
                ("known", s.known.into()),
            ])
        })
    }

    /// What a point cloud says of itself, undefined before its head is read
    #[wasm_bindgen(js_name = pointCloudInfo, unchecked_return_type = "PointCloudInfo | undefined")]
    #[must_use]
    pub fn point_cloud_info(&self, id: u32) -> JsValue {
        let Some(cloud) = self.layer::<PointCloudLayer>(id).and_then(|l| l.cloud.as_ref()) else {
            return JsValue::UNDEFINED;
        };
        let h = &cloud.copc.header;
        let root = &cloud.hierarchy.nodes[0].geodetic;
        let mut entries = vec![
            ("points", (h.point_count as f64).into()),
            ("format", h.point_format.into()),
            ("center", js::pair(root.x, root.y)),
            (
                "altitudes",
                js::pair(h.min[2] * cloud.crs.vertical, h.max[2] * cloud.crs.vertical),
            ),
        ];
        if let Some(wkt) = &cloud.copc.wkt {
            entries.push(("wkt", JsValue::from_str(wkt)));
        }
        js::object(&entries)
    }

    /// The point of a point cloud picked by `pick` or `pollPick` (its index there), read back from the GPU; undefined
    /// once it is no longer drawn
    #[wasm_bindgen(js_name = pointCloudPoint, unchecked_return_type = "PointCloudPoint | undefined")]
    #[must_use]
    pub fn point_cloud_point(&self, id: u32, index: u32) -> JsValue {
        self.layer::<PointCloudLayer>(id)
            .and_then(|layer| layer.point(&self.gl, index))
            .unwrap_or(JsValue::UNDEFINED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_take_the_page_s_numbers() {
        let s = Style::parse(&[
            2.0,
            3.0,
            10.0,
            0.0,
            1.0,
            1.5,
            2.0,
            30.0,
            90.0,
            f64::NAN,
            f64::NAN,
            4.0,
            1e6,
            8.0,
        ]);
        assert_eq!(s.color_by, Some(ColorBy::Elevation));
        assert_eq!((s.size, s.max_size, s.adaptive, s.round), (3.0, 10.0, false, true));
        assert_eq!(s.edl, (1.5, 2.0));
        assert_eq!(s.elevation, [30.0, 90.0]);
        assert!(s.intensity[0].is_nan());
        assert_eq!((s.max_error, s.budget, s.rate), (4.0, 1_000_000, 8.0));
        // Nothing given, or out of range: the defaults
        let d = Style::parse(&[
            -1.0,
            -2.0,
            f64::NAN,
            1.0,
            1.0,
            -1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            -5.0,
            -1.0,
        ]);
        assert_eq!(d.color_by, None);
        assert_eq!(
            (d.size, d.max_size, d.edl, d.max_error, d.budget, d.rate),
            (2.0, 4.0, (0.0, 1.4), 1.5, 3_000_000, 0.0)
        );
        assert_eq!(Style::parse(&[]).budget, 3_000_000);
    }

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

    #[test]
    fn intensities_range_over_their_bulk() {
        let mut v = Vec::new();
        for i in 0..1000u16 {
            let mut vertex = [0u8; STRIDE];
            vertex[INTENSITY..INTENSITY + 2].copy_from_slice(&(if i == 999 { 60_000 } else { i }).to_le_bytes());
            v.extend(vertex);
        }
        let [lo, hi] = intensity_range(&v).unwrap();
        assert!((lo - 16.0).abs() < 1.0 && (hi - 976.0).abs() < 1.0, "{lo} {hi}");
        assert_eq!(intensity_range(&[]), None);
        assert_eq!(intensity_range(&[0; STRIDE]), Some([0.0, 16.0]));
    }

    #[test]
    fn gpu_budgets_follow_the_point_budget() {
        assert_eq!(gpu_budget(3_000_000), 120_000_000);
        assert_eq!(gpu_budget(10), 64 << 20);
    }
}
