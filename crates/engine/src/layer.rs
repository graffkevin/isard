//! What a layer is to the engine. The raster layers are the ground: the engine composites them in one pass over the
//! relief. Everything drawn over the ground (networks, 3D Tiles…) is a `Layer`: a new kind of layer is a type that
//! implements it, with its own JavaScript API in its own module (`#[wasm_bindgen] impl Engine`), and the engine itself
//! does not change.

use crate::render::raster::{RasterPass, RasterSource};
use geo::camera::{Camera, View};
use std::any::Any;

/// How tiles load and show: the engine's two profiles
#[derive(Clone, Copy, Debug)]
pub struct Loading {
    /// New tiles fade in over their stand-in during this time (ms); 0: they show at once
    pub fade_ms: f64,
    /// During a camera flight, the levels flown through are skipped (Cesium's skip LOD)
    pub skip_levels_in_flight: bool,
    /// While the camera moves, covered tiles off the screen center wait (foveated loading)
    pub defer_off_center: bool,
    /// Time spent uploading arrived tiles to the GPU per frame and layer (ms; at least one tile)
    pub upload_budget_ms: f64,
    /// Requests in flight per raster layer, and per 3D Tiles layer
    pub raster_requests: usize,
    #[cfg_attr(
        not(feature = "tiles3d"),
        allow(dead_code, reason = "read by the 3D Tiles layers only")
    )]
    pub mesh_requests: usize,
}

impl Loading {
    /// Everything visible loads and shows as soon as it arrives (the default)
    pub const IMMEDIATE: Self = Self {
        fade_ms: 0.0,
        skip_levels_in_flight: false,
        defer_off_center: false,
        upload_budget_ms: 8.0,
        raster_requests: 24,
        mesh_requests: 20,
    };
    /// Fewer downloads while moving, tiles fading in: smoother on slow networks and GPUs
    pub const SMOOTH: Self = Self {
        fade_ms: 200.0,
        skip_levels_in_flight: true,
        defer_off_center: true,
        upload_budget_ms: 3.0,
        raster_requests: 16,
        mesh_requests: 12,
    };
}

/// What a layer gets to draw a frame
pub struct Frame<'a> {
    pub gl: &'a glow::Context,
    /// The same context, for uploads straight from JavaScript typed arrays
    #[cfg_attr(
        not(feature = "tiles3d"),
        allow(dead_code, reason = "read by the 3D Tiles layers only")
    )]
    pub raw: &'a web_sys::WebGl2RenderingContext,
    pub camera: &'a Camera,
    pub view: &'a View,
    pub pixel_ratio: f64,
    /// Frame number
    #[cfg_attr(
        not(feature = "tiles3d"),
        allow(dead_code, reason = "read by the 3D Tiles layers only")
    )]
    pub number: u64,
    #[cfg_attr(
        not(feature = "tiles3d"),
        allow(dead_code, reason = "read by the 3D Tiles layers only")
    )]
    pub loading: Loading,
    /// The relief, if any: its elevation source and vertical exaggeration
    pub relief: Option<(&'a RasterSource, f32)>,
    /// The ground drawn so far this frame: its tiles can be drawn again (`RasterPass::redraw`)
    pub ground: &'a RasterPass,
    /// Fog of the atmosphere: color and range (see `Atmosphere::fog`)
    pub fog: Option<([f32; 3], [f32; 2])>,
    /// The glyph atlas of the symbol layers and its size, once the page has made one
    pub glyphs: Option<(glow::Texture, (i32, i32))>,
}

/// A layer drawn over the ground. The frame starts with blending on, back faces culled, no depth test: a layer that
/// changes this state sets it back.
pub trait Layer: Any {
    /// Loads what the view needs and draws it; true while content is still loading (a new frame will be asked
    /// through `redraw`)
    fn draw(&mut self, frame: &Frame) -> bool;

    /// Whether `pick` finds features of this layer
    fn pickable(&self) -> bool {
        false
    }

    /// Picking pass into the bound framebuffer, blending and culling off: each feature drawn with its index + 1 in RGB
    /// (24 bits, little end in red) and `layer` in alpha
    fn draw_pick(&self, _frame: &Frame, _layer: u8) {}

    /// Feature at a ground position (Mercator), for the layers picked on the CPU (fills), at the camera's zoom
    fn pick_ground(&self, _m: glam::DVec2, _zoom: f64) -> Option<u32> {
        None
    }

    /// Whether `pick_ground` may find features (the picking then needs the ground position under the cursor)
    fn picks_ground(&self) -> bool {
        false
    }

    /// Places its labels before the frame is drawn, the top layer first, all layers sharing one collision grid
    fn place(&mut self, _frame: &Frame, _grid: &mut geo::collision::Grid) {}

    /// Layer `id` was removed: references to it (a network coloring buildings) are dropped
    fn forget(&mut self, _id: u32) {}

    /// Frees its GPU objects and stops its downloads; the layer is dropped right after
    fn delete(&mut self, gl: &glow::Context);
}
