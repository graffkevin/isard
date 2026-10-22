//! The COPC layer: a point cloud read from one file by HTTP range requests. Its head (header, VLRs) first, then the
//! pages of its hierarchy as the traversal reaches them, both fetched here and parsed by `pointcloud::copc`; the
//! chunks of the nodes in view go to the decoding workers, which decompress them with the point cloud module and
//! position their points through the grid made here (`pointcloud::frame`), and come back as vertices, uploaded a few
//! per frame. The CRS is the page's (`resolve`: the WKT of the file to the inverse projection, from proj4 or built in),
//! the altitudes raised by the RAF20 geoid where they are above it (IGN69). Nodes are kept within a GPU memory
//! budget (least recently drawn unloaded first), their downloads aborted when they leave the selection; requests are
//! held to a rate when the server limits it (the Géoplateforme: 10 a second).
//!
//! - `hierarchy`: the head and the hierarchy pages, the CRS of the file and the nodes placed by it
//! - `nodes`: the nodes' requests to the decoding workers, their grids, the rate of the requests
//! - `upload`: what arrived taken in, the nodes' vertices put on the GPU
//! - `selection`: the nodes selected for the view, the GPU memory budget, the counters of a frame
//! - `draw`: the look and the draws, Eye-Dome Lighting, the picking pass
//! - `style`: the style the page sets

mod draw;
mod hierarchy;
mod nodes;
mod selection;
mod style;
mod upload;

use style::Style;

use super::{Edl, GpuPoints, PointProgram};
use crate::engine::Engine;
use crate::js;
use crate::layer::{Frame, Layer};
use crate::render::tiles3d::state::{Resident, State, States};
use glam::DVec3;
use hierarchy::{Crs, HEAD_BYTES};
use nodes::Bucket;
use pointcloud::copc::{Copc, Hierarchy};
use pointcloud::points::Layout;
use pointcloud::traversal::Selection;
use selection::gpu_budget;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use wasm_bindgen::prelude::*;

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

/// What the last frame of a point cloud did, for the page's statistics
#[derive(Default, Clone, Copy)]
pub struct PointCloudStats {
    /// Points drawn
    pub points: u64,
    /// Nodes drawn
    pub nodes: usize,
    /// Nodes on the GPU
    pub loaded: usize,
    /// Nodes requested, not arrived yet
    pub in_flight: usize,
    /// Bytes of the nodes on the GPU
    pub gpu_bytes: usize,
    /// Nodes of the hierarchy known so far (its pages loaded)
    pub known: usize,
}

/// A COPC point cloud streamed by node: the header and hierarchy pages, the nodes' states, the requests throttled
/// by a token bucket, and the program that draws them
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
    /// Current style, set by `setPointCloudStyle`
    pub style: Style,
    /// The intensities of the first node, when the page gives no range
    auto_intensity: Option<[f64; 2]>,
    bucket: Bucket,
    /// A frame is asked for once the bucket has a token again (requests held back)
    waking: Rc<std::cell::Cell<bool>>,
    selection: Selection,
    /// Statistics of the last frame
    pub stats: PointCloudStats,
    /// The nodes of the last picking pass and the id of their first point
    picked: RefCell<Vec<(usize, u32)>>,
    redraw: Rc<dyn Fn()>,
}

impl PointCloudLayer {
    /// Starts with the head of `url` (its header and root page); `resolve` gives the page's CRS for the file's WKT,
    /// `loader` the decoding workers if any, `redraw` is called when something arrives
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

    /// A frame of the cloud: what arrived taken in, the nodes selected, pages then nodes requested (within the rate),
    /// the GPU memory kept within the budget, the nodes drawn. Whether more frames are needed (loading, held back)
    fn draw_cloud(&mut self, frame: &Frame) -> bool {
        self.receive(frame.gl, frame.raw, frame.number, frame.loading.upload_budget_ms);

        let waiting = self.head.is_some() && self.cloud.is_none();

        if self.cloud.is_none() {
            return waiting && self.head == Some(0);
        }

        let now = js_sys::Date::now();

        self.select_nodes(frame.view, now);

        // Pages first: the nodes under them wait for them
        let rate = self.style.rate;
        let budget = gpu_budget(self.style.budget);
        let pages_held = self.request_pages(rate, now);
        let nodes_held = self.request_nodes(frame, rate, budget, now);
        let held = pages_held || nodes_held;

        self.unload_over_budget(frame, budget);
        self.render(frame);

        let in_flight = self.update_stats();

        if !self.queue.is_empty() {
            (self.redraw)();
        }
        if held {
            self.wake(rate);
        }

        held || in_flight > 0 || !self.queue.is_empty() || self.pages.values().any(Option::is_none)
    }
}

impl Layer for PointCloudLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        self.draw_cloud(frame)
    }

    fn pickable(&self) -> bool {
        !self.selection.draw.is_empty()
    }

    fn draw_pick(&self, frame: &Frame, layer: u8) {
        self.draw_picking(frame, layer);
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
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) when a WebGL object cannot be created or a shader fails to compile (the error carries its
    /// info log). The file itself is read later: a bad file, or a `resolve` that throws, leaves the layer empty
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
