//! The engine exported to JavaScript: the camera and its interactions, the ground (raster layers over the relief)
//! and the frame loop. The layers drawn over the ground (`Layer`) export their own methods, next to them; the picking
//! is in `pick`.

mod pick;
mod viewshed;

use crate::js;
use crate::layer::{Frame, Layer, Loading};
use crate::render::gl::{self, Grid};
use crate::render::globe::{self, GlobeBase};
use crate::render::raster::{
    DrawContext, RasterOptions, RasterPass, RasterSource, STACK, terrain_geoid, terrain_levels,
};
use crate::render::sky::{self, Atmosphere, Sky};
use geo::camera::{self, Camera, View};
use glam::DVec2;
use glow::HasContext;
use pick::{AsyncPick, PickTarget};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
/** Camera, with the conventions of MapLibre */
export interface Camera {
  center: [lon: number, lat: number];
  zoom: number;
  /** Degrees from the vertical, 0 to 70 */
  pitch: number;
  /** Degrees clockwise from the north */
  bearing: number;
}

/** The feature under a screen point */
export interface PickHit {
  /** Id of the layer */
  layer: number;
  /** Index of the feature in the layer (the edge of a network) */
  index: number;
}

/** Tile counters of a raster layer */
export interface RasterLayerStats {
  id: number;
  /** Tiles drawn in the last frame (stand-ins included) */
  drawn: number;
  /** Tiles held on the GPU */
  loaded: number;
  /** Requests in flight */
  inFlight: number;
  /** Arrived tiles waiting for their upload */
  queued: number;
  /** Drawn with an ancestor's texture: the tile itself is not loaded yet */
  standIns: number;
  /** Waiting before a new attempt after errors */
  cooling: number;
  /** Not served (404), covered by an ancestor */
  missing: number;
}
"#;

/// What the engine draws, bottom to top
enum Slot {
    /// The ground: consecutive raster layers are composited in one pass over the relief
    Raster(Box<RasterSource>),
    /// Drawn over the ground
    Layer(Box<dyn Layer>),
}

#[wasm_bindgen]
pub struct Engine {
    pub(crate) gl: glow::Context,
    pub(crate) raw: web_sys::WebGl2RenderingContext,
    /// Pool of decoding workers for 3D Tiles (`load`, `abort`), set by the JavaScript side
    #[cfg_attr(
        not(feature = "tiles3d"),
        allow(dead_code, reason = "read by the 3D Tiles layers only")
    )]
    pub(crate) loader: Option<JsValue>,
    camera: Camera,
    pixel_ratio: f64,
    layers: Vec<(u32, Slot)>,
    next_id: u32,
    frame: u64,
    pub(crate) redraw: Rc<dyn Fn()>,
    background: [f32; 3],
    /// The tile mesh shared by the ground and the globe base
    grid: Grid,
    globe: GlobeBase,
    sky: Sky,
    raster_pass: RasterPass,
    /// Relief under the raster layers: the elevation source and its vertical exaggeration
    terrain: Option<(RasterSource, f32)>,
    /// Camera of the last frame and when it last changed (ms)
    last_camera: [u64; 5],
    moved_at: f64,
    /// Destination of a camera flight, with a generation number
    preload: Option<(u64, Camera)>,
    preload_generation: u64,
    /// Offscreen target of the picking pass, created on the first pick
    pick_target: Option<PickTarget>,
    /// The pixel buffer of `requestPick` and the request waiting for the GPU
    async_pick: Option<AsyncPick>,
    /// How tiles load and show
    loading: Loading,
    /// View of the last frame: what is on screen, for the picking of positions
    drawn: Option<View>,
    /// Last drag or zoom point (CSS pixels) and the altitude of the ground grabbed there (metres, exaggeration
    /// included): the next move from that point keeps the same ground point under the cursor
    grab: Option<(DVec2, f64)>,
    /// Glyphs of the symbol layers
    pub(crate) glyphs: Option<crate::render::symbols::GlyphAtlas>,
    /// Icons of the symbol layers (RGBA, premultiplied)
    pub(crate) icons: Option<crate::render::symbols::IconAtlas>,
    /// The user moved the camera (drag, zoom, turn) since it was last set: finer relief then keeps the eye in place
    /// (`Camera::settle_elevation`); a camera set by the application keeps its center instead
    handled: bool,
    /// The collision grid of the labels, kept from frame to frame
    label_grid: Option<geo::collision::Grid>,
    /// What can be seen from a point, drawn over the frame
    viewshed: Option<viewshed::Viewshed>,
}

/// The camera counts as moving until this long after its last change (ms)
const MOVING_MS: f64 = 100.0;

#[wasm_bindgen]
impl Engine {
    /// `redraw` is called when the engine needs a new frame (a tile arrived)
    #[wasm_bindgen(constructor)]
    pub fn new(
        canvas: &web_sys::HtmlCanvasElement,
        #[wasm_bindgen(unchecked_param_type = "() => void")] redraw: js_sys::Function,
    ) -> Result<Engine, JsValue> {
        let attributes = web_sys::WebGlContextAttributes::new();
        attributes.set_alpha(false);
        attributes.set_antialias(true);
        attributes.set_depth(true);
        // 3D Tiles: parents standing in for loading children only fill the pixels no finer tile covers
        attributes.set_stencil(true);
        attributes.set_power_preference(web_sys::WebGlPowerPreference::HighPerformance);
        let context = canvas
            .get_context_with_context_options("webgl2", &attributes)?
            .ok_or("WebGL2 is not available")?
            .dyn_into::<web_sys::WebGl2RenderingContext>()?;
        // The raw context too: decoded tiles are uploaded straight from JavaScript typed arrays
        let raw = context.clone();
        let gl = crate::render::gl::context(context)?;
        let redraw: Rc<dyn Fn()> = Rc::new(move || {
            let _ = redraw.call0(&JsValue::NULL);
        });
        // What was created is deleted again if a step fails
        let grid = Grid::new(&gl)?;
        let globe = GlobeBase::new(&gl, &grid).inspect_err(|_| grid.delete(&gl))?;
        let raster_pass = RasterPass::new(&gl, &grid).inspect_err(|_| {
            globe.delete(&gl);
            grid.delete(&gl);
        })?;
        let sky = Sky::new(&gl).inspect_err(|_| {
            raster_pass.delete(&gl);
            globe.delete(&gl);
            grid.delete(&gl);
        })?;
        Ok(Self {
            gl,
            raw,
            loader: None,
            camera: Camera::new(2.424_573, 48.845_726, 13.0),
            pixel_ratio: 1.0,
            layers: Vec::new(),
            next_id: 1,
            frame: 0,
            redraw,
            background: [0.95, 0.95, 0.94],
            grid,
            globe,
            sky,
            raster_pass,
            terrain: None,
            last_camera: [0; 5],
            moved_at: 0.0,
            preload: None,
            preload_generation: 0,
            pick_target: None,
            viewshed: None,
            async_pick: None,
            loading: Loading::IMMEDIATE,
            drawn: None,
            grab: None,
            glyphs: None,
            icons: None,
            handled: false,
            label_grid: None,
        })
    }

    /// Viewport in CSS pixels and device pixel ratio
    pub fn resize(&mut self, width: f64, height: f64, pixel_ratio: f64) {
        // A size that is not a number keeps the last one
        if width.is_finite() && height.is_finite() {
            self.camera.width = width.max(1.0);
            self.camera.height = height.max(1.0);
        }
        if pixel_ratio.is_finite() && pixel_ratio > 0.0 {
            self.pixel_ratio = pixel_ratio;
        }
    }

    #[wasm_bindgen(js_name = setCamera)]
    pub fn set_camera(&mut self, lon: f64, lat: f64, zoom: f64, pitch: f64, bearing: f64) {
        self.camera.set_view(lon, lat, zoom, pitch, bearing);
        self.handled = false;
    }

    #[must_use]
    #[wasm_bindgen(unchecked_return_type = "Camera")]
    pub fn camera(&self) -> JsValue {
        let c = camera::lon_lat(self.camera.center);
        js::object(&[
            ("center", js::pair(c.x, c.y)),
            ("zoom", self.camera.zoom.into()),
            ("pitch", self.camera.pitch.into()),
            ("bearing", self.camera.bearing.into()),
        ])
    }

    #[wasm_bindgen(js_name = setZoomRange)]
    pub fn set_zoom_range(&mut self, min: f64, max: f64) {
        self.camera.set_zoom_range(min, max);
    }

    #[wasm_bindgen(js_name = setBackground)]
    pub fn set_background(&mut self, r: f32, g: f32, b: f32) {
        self.background = [r, g, b];
    }

    /// Globe (true) or Web Mercator (false)
    #[wasm_bindgen(js_name = setGlobe)]
    pub fn set_globe(&mut self, globe: bool) {
        self.camera.globe = globe;
    }

    /// How tiles load and show: `true`, immediately (everything visible loads, no fade, the default); `false`,
    /// smoothly (levels skipped during flights, off-center tiles waiting while the camera moves, fade-in)
    #[wasm_bindgen(js_name = setImmediateLoading)]
    pub fn set_immediate_loading(&mut self, immediate: bool) {
        self.loading = if immediate { Loading::IMMEDIATE } else { Loading::SMOOTH };
    }

    /// The body the map is drawn on: its equatorial radius (metres) and flattening (0 for a sphere), the halo of its
    /// atmosphere around the globe (RGB 0-1 and its strength, 4 numbers; `undefined` for an airless body), and the
    /// lowest and highest altitudes of its ground (metres: what its relief may raise into view). WGS 84, the Earth's
    /// blue halo and the range of its relief worldwide (-450 m to 8900 m) by default; set before the layers and the
    /// relief, whose shaders may take its size (`ISARD_CIRCUMFERENCE` of the ground layers)
    #[wasm_bindgen(js_name = setBody)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_body(&mut self, radius: f64, flattening: f64, halo: Option<Box<[f32]>>, lowest: f64, highest: f64) {
        if radius.is_finite() && radius > 0.0 && (0.0..1.0).contains(&flattening) {
            // Not a range around the ellipsoid: 1 % of the radius up and down
            let relief = if lowest.is_finite() && highest.is_finite() && lowest <= 0.0 && highest >= 0.0 {
                (lowest, highest)
            } else {
                (-0.01 * radius, 0.01 * radius)
            };
            self.camera.body = geo::geodesy::Body {
                radius,
                flattening,
                relief,
            };
        }
        self.sky.halo = sky::halo(halo.as_deref());
    }

    /// Space behind the globe (stars, atmosphere halo), in the globe projection
    #[wasm_bindgen(js_name = setSky)]
    pub fn set_sky(&mut self, enabled: bool) {
        self.sky.enabled = enabled;
    }

    /// Sky over the plane (`MapLibre`'s `setSky`): colors of the sky, the horizon and the fog (RGB 0-1, 9 numbers),
    /// then the sky-horizon, horizon-fog and fog-ground blends; `undefined` for none (the background color shows)
    #[wasm_bindgen(js_name = setAtmosphere)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_atmosphere(&mut self, settings: Option<Box<[f32]>>) {
        self.sky.atmosphere = settings.as_deref().and_then(|s| {
            let &[sr, sg, sb, hr, hg, hb, fr, fg, fb, sky_horizon, horizon_fog, fog_ground] = s else {
                return None;
            };
            Some(Atmosphere {
                sky: [sr, sg, sb],
                horizon: [hr, hg, hb],
                fog: [fr, fg, fb],
                sky_horizon_blend: sky_horizon,
                horizon_fog_blend: horizon_fog,
                fog_ground_blend: fog_ground,
            })
        });
    }

    /// Highest pitch the camera accepts, degrees (up to 85)
    #[wasm_bindgen(js_name = setMaxPitch)]
    pub fn set_max_pitch(&mut self, max: f64) {
        self.camera.set_max_pitch(max);
    }

    /// Relief under the raster layers, from raw altitudes (`image/x-bil;bits=32`, 256 × 256, or the same followed by
    /// the validity mask of an `ArcGIS` raw grid): `url` is a template with `{bbox}` (WMS `GetMap` in EPSG:3857, or in
    /// the Mercator metres of another body) or `{z}/{x}/{y}`; `undefined` turns it off. `exaggeration` scales the
    /// heights (1: true relief); `max_z`: finest level loaded (up to 15, from 6 on the Earth, coarser on bodies of a
    /// larger relief for their size: `terrain_levels`); `geoid`: altitudes raised by the RAF20 geoid height (heights
    /// above the ellipsoid, those of 3D Tiles; over France, on the Earth only), else left as served (the default);
    /// `loader`: gives the
    /// altitudes of each tile instead of fetching `url` (an encoding decoded by the page: Terrarium, Terrain-RGB)
    #[wasm_bindgen(js_name = setTerrain)]
    pub fn set_terrain(
        &mut self,
        url: Option<String>,
        exaggeration: f64,
        max_z: Option<u8>,
        geoid: Option<bool>,
        #[wasm_bindgen(
            unchecked_param_type = "((z: number, x: number, y: number, signal: AbortSignal) => Promise<Float32Array | null>) | undefined"
        )]
        loader: Option<js_sys::Function>,
    ) -> Result<(), JsValue> {
        let exaggeration = exaggeration.max(0.0) as f32;
        let body = self.body();
        let (min_z, max_z) = terrain_levels(&body, max_z);
        let geoid = terrain_geoid(geoid, &body);
        match (url, &mut self.terrain) {
            (Some(url), Some((source, current)))
                if source.options.url == url
                    && source.options.max_z == max_z
                    && source.options.geoid == geoid
                    && source.options.body == body
                    && source.options.loader == loader =>
            {
                *current = exaggeration;
            }
            (Some(url), _) => {
                let source = RasterSource::new(
                    &self.gl,
                    RasterOptions {
                        url,
                        min_z,
                        max_z,
                        opacity: 1.0,
                        saturation: 1.0,
                        elevation: true,
                        body,
                        geoid,
                        size: 256,
                        loader,
                    },
                )?;
                if let Some((old, _)) = self.terrain.replace((source, exaggeration)) {
                    old.delete(&self.gl);
                }
            }
            (None, _) => {
                if let Some((old, _)) = self.terrain.take() {
                    old.delete(&self.gl);
                }
            }
        }
        (self.redraw)();
        Ok(())
    }

    /// Color of the globe under the tiles (polar caps)
    #[wasm_bindgen(js_name = setGlobeColor)]
    pub fn set_globe_color(&mut self, r: f32, g: f32, b: f32) {
        self.globe.color = [r, g, b];
    }

    /// Tile counters of the raster layers, for diagnostics
    #[must_use]
    #[wasm_bindgen(unchecked_return_type = "RasterLayerStats[]")]
    pub fn stats(&self) -> js_sys::Array {
        // The relief comes last, with id 0
        let terrain = self.terrain.as_ref().map(|(source, _)| (0, source));
        self.layers
            .iter()
            .filter_map(|(id, slot)| match slot {
                Slot::Raster(layer) => Some((*id, layer.as_ref())),
                Slot::Layer(_) => None,
            })
            .chain(terrain)
            .map(|(id, layer)| {
                let s = layer.stats;
                js::object(&[
                    ("id", id.into()),
                    ("drawn", s.drawn.into()),
                    ("loaded", s.loaded.into()),
                    ("inFlight", s.in_flight.into()),
                    ("queued", s.queued.into()),
                    ("standIns", s.stand_ins.into()),
                    ("cooling", s.cooling.into()),
                    ("missing", s.missing.into()),
                ])
            })
            .collect()
    }

    /// Tiles a raster layer wants for the view of the last frame, three numbers per tile (z, x, y), the nearest to
    /// the eye first: the level each area of the ground is drawn at, for the labels of a vector style to follow it
    #[must_use]
    #[wasm_bindgen(js_name = wantedTiles)]
    pub fn wanted_tiles(&self, id: u32) -> Vec<u32> {
        match self.layers.iter().find(|(i, _)| *i == id) {
            Some((_, Slot::Raster(layer))) => layer.wanted().flat_map(|t| [u32::from(t.z), t.x, t.y]).collect(),
            _ => Vec::new(),
        }
    }

    /// Loads in advance the tiles of a camera flight's destination; `clearPreload` when the flight ends
    pub fn preload(&mut self, lon: f64, lat: f64, zoom: f64, pitch: f64, bearing: f64) {
        let mut camera = self.camera.clone();
        camera.set_view(lon, lat, zoom, pitch, bearing);
        self.preload_generation += 1;
        self.preload = Some((self.preload_generation, camera));
    }

    #[wasm_bindgen(js_name = clearPreload)]
    pub fn clear_preload(&mut self) {
        self.preload = None;
    }

    /// Drags the ground from one screen point to another (CSS pixels); in relief, the ground point grabbed stays
    /// under the cursor
    pub fn pan(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
            return;
        }
        let (from, to) = (DVec2::new(x0, y0), DVec2::new(x1, y1));
        let altitude = self.grabbed(from);
        self.camera.pan_at(from, to, altitude);
        self.grab = Some((to, altitude));
        self.handled = true;
    }

    #[wasm_bindgen(js_name = zoomAt)]
    pub fn zoom_at(&mut self, delta: f64, x: f64, y: f64) {
        if ![delta, x, y].iter().all(|v| v.is_finite()) {
            return;
        }
        let at = DVec2::new(x, y);
        // The relief kept on the CPU first: a wheel notch never waits for the GPU (a read back stalls it)
        let altitude = match self.grab {
            Some((point, altitude)) if point == at => altitude,
            _ => match self.cpu_altitude(x, y) {
                Some(altitude) => altitude,
                None => self.grabbed(at),
            },
        };
        self.camera.zoom_at_altitude(delta, at, altitude);
        self.grab = Some((at, altitude));
        self.handled = true;
    }

    /// Turns and tilts around the screen center, in degrees
    pub fn rotate(&mut self, bearing: f64, pitch: f64) {
        if !bearing.is_finite() || !pitch.is_finite() {
            return;
        }
        self.camera.bearing += bearing;
        self.camera.pitch += pitch;
        self.camera.clamp();
        self.handled = true;
    }

    /// Position under a screen point (CSS pixels), undefined above the horizon; on the relief itself
    #[wasm_bindgen(unchecked_return_type = "[lon: number, lat: number] | undefined")]
    pub fn ground(&mut self, x: f64, y: f64) -> JsValue {
        let altitude = self.pick_altitude(x, y).unwrap_or(self.camera.elevation);
        self.camera
            .ground_at(&self.camera.view(), x, y, altitude)
            .map(camera::lon_lat)
            .map_or(JsValue::UNDEFINED, |p| js::pair(p.x, p.y))
    }

    /// Screen point (CSS pixels) of a ground position, or `height` metres above it, undefined behind the camera
    #[must_use]
    #[wasm_bindgen(unchecked_return_type = "[x: number, y: number] | undefined")]
    pub fn project(&self, lon: f64, lat: f64, height: Option<f64>) -> JsValue {
        let view = self.camera.view();
        let m = camera::mercator(lon, lat);
        // On the relief: its altitude there (from the coarse copy kept on the CPU)
        let altitude = self
            .terrain
            .as_ref()
            .and_then(|(terrain, exaggeration)| terrain.altitude(m).map(|h| h * f64::from(*exaggeration)))
            .unwrap_or(0.0)
            + height.filter(|h| h.is_finite()).unwrap_or(0.0);
        // On the plane or the sphere, on the copy of the world nearest to the center; past the horizon of the
        // globe, hidden by the body
        let shift = -(m.x - self.camera.center.x).round();
        let p = view.position(m, shift);
        if view.globe.factor > 0.0 && !view.faces_camera(p) {
            return JsValue::UNDEFINED;
        }
        let clip = view.view_proj * glam::DVec4::new(p.x, p.y, p.z + altitude * view.vertical, 1.0);
        if clip.w <= 0.0 {
            return JsValue::UNDEFINED;
        }
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        js::pair(
            f64::midpoint(x, 1.0) * self.camera.width,
            f64::midpoint(-y, 1.0) * self.camera.height,
        )
    }

    /// Longitudes and latitudes the view covers on the ground, `[west, south, east, north]` (degrees): the screen's
    /// border cast on the ground, or on the far plane where it passes the horizon
    #[must_use]
    pub fn bounds(&self) -> Box<[f64]> {
        const STEPS: u32 = 8;
        let view = self.camera.view();
        let (w, h) = (self.camera.width, self.camera.height);
        let (mut lo, mut hi) = (DVec2::INFINITY, DVec2::NEG_INFINITY);
        for k in 0..=STEPS {
            let t = f64::from(k) / f64::from(STEPS);
            for (x, y) in [(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)] {
                let m = self
                    .camera
                    .ground_at(&view, x, y, self.camera.elevation)
                    .unwrap_or_else(|| {
                        let (_, far) = self.camera.ray(&view, x, y);
                        self.camera.center + DVec2::new(far.x, far.y) / view.world_size
                    });
                let p = camera::lon_lat(m);
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        Box::new([lo.x.max(-180.0), lo.y.max(-85.06), hi.x.min(180.0), hi.y.min(85.06)])
    }

    /// Raster tiles in Web Mercator; `url` holds {z}, {x} and {y}. `size`: side of the tiles' textures (256 by
    /// default); `loader`: draws each tile instead of fetching `url`, `(z, x, y, signal) => Promise<ImageBitmap |
    /// null>`, an image of `size` × `size` (null: nothing there)
    #[wasm_bindgen(js_name = addRaster)]
    #[allow(clippy::too_many_arguments)]
    pub fn add_raster(
        &mut self,
        url: String,
        min_z: u8,
        max_z: u8,
        opacity: f32,
        saturation: f32,
        size: Option<i32>,
        #[wasm_bindgen(
            unchecked_param_type = "((z: number, x: number, y: number, signal: AbortSignal) => Promise<ImageBitmap | null>) | undefined"
        )]
        loader: Option<js_sys::Function>,
    ) -> Result<u32, JsValue> {
        let layer = RasterSource::new(
            &self.gl,
            RasterOptions {
                url,
                min_z,
                max_z,
                opacity,
                saturation,
                elevation: false,
                body: self.body(),
                geoid: false,
                size: size.unwrap_or(256).clamp(16, 1024),
                loader,
            },
        )?;
        Ok(self.push(Slot::Raster(Box::new(layer))))
    }

    #[wasm_bindgen(js_name = setRasterStyle)]
    pub fn set_raster_style(&mut self, id: u32, opacity: f32, saturation: f32) {
        if let Some(Slot::Raster(layer)) = self.slot(id) {
            layer.options.opacity = opacity;
            layer.options.saturation = saturation;
        }
    }

    #[wasm_bindgen(js_name = removeLayer)]
    pub fn remove_layer(&mut self, id: u32) {
        if let Some(index) = self.layers.iter().position(|(i, _)| *i == id) {
            match self.layers.remove(index).1 {
                Slot::Raster(layer) => layer.delete(&self.gl),
                Slot::Layer(mut layer) => layer.delete(&self.gl),
            }
            for (_, slot) in &mut self.layers {
                if let Slot::Layer(layer) = slot {
                    layer.forget(id);
                }
            }
        }
    }

    /// Moves a layer to a drawing position (0 = bottom)
    #[wasm_bindgen(js_name = moveLayer)]
    pub fn move_layer(&mut self, id: u32, index: usize) {
        if let Some(from) = self.layers.iter().position(|(i, _)| *i == id) {
            let layer = self.layers.remove(from);
            self.layers.insert(index.min(self.layers.len()), layer);
        }
    }

    /// Draws a frame; returns true while tiles are still loading (a new frame will be asked through `redraw`)
    /// The sky and the globe base, under everything; both lie as deep as the lowest ground under relief (a canyon at
    /// the horizon shows the halo beyond, not the base)
    fn draw_backdrop(&self, view: &View) {
        let depth = globe::depth(view, self.terrain.as_ref().map(|(_, e)| *e));
        self.sky.draw(&self.gl, view, depth);
        self.globe.draw(&self.gl, view, depth);
    }

    pub fn render(&mut self) -> bool {
        self.frame += 1;
        // The camera turns around the relief under its center (sea level without relief)
        let key_of = |c: &Camera| [c.center.x, c.center.y, c.zoom, c.pitch, c.bearing].map(f64::to_bits);
        if self.terrain.is_none() {
            self.camera.elevation = 0.0;
        } else if let Some(h) = ground_elevation(self.terrain.as_ref(), self.camera.center) {
            if self.handled {
                // Moved by the user (or finer relief under a camera they moved): the eye stays, the orbited point
                // comes down to the ground under the center, nothing moves on screen
                let still = key_of(&self.camera) == self.last_camera;
                self.camera.settle_elevation(h);
                if still {
                    self.last_camera = key_of(&self.camera);
                }
            } else {
                // Set by the application: its center holds, the camera rises and sinks with the ground
                self.camera.elevation = h;
            }
        }
        let view = self.camera.view();
        let (w, h) = (
            self.camera.width * self.pixel_ratio,
            self.camera.height * self.pixel_ratio,
        );
        let mut loading = false;
        self.begin_frame(w, h);
        // Camera motion, for the foveated loading
        let key = key_of(&self.camera);
        let now = js_sys::Date::now();
        if key != self.last_camera {
            self.last_camera = key;
            self.moved_at = now;
        }
        let moving = now - self.moved_at < MOVING_MS;
        let preload = self.preload.as_ref().map(|(generation, camera)| {
            let mut camera = camera.clone();
            camera.width = self.camera.width;
            camera.height = self.camera.height;
            (*generation, camera.view())
        });
        self.drawn = Some(view.clone());
        self.draw_backdrop(&view);
        let redraw = self.redraw.clone();
        let ctx = DrawContext {
            view: &view,
            frame: self.frame,
            redraw: &redraw,
            moving,
            preload: preload.as_ref().map(|(generation, v)| (*generation, v)),
            loading: self.loading,
            now,
        };
        self.raster_pass.begin_frame();
        if let Some((terrain, _)) = &mut self.terrain {
            loading |= terrain.prepare(&self.gl, &ctx);
        }
        // Taken out while they draw: each layer gets the engine's state, the rasters the relief to drape on
        let mut layers = std::mem::take(&mut self.layers);
        self.place_labels(&mut layers, &view);
        let mut i = 0;
        while i < layers.len() {
            if let Slot::Layer(layer) = &mut layers[i].1 {
                loading |= layer.draw(&self.frame_state(&view));
                i += 1;
                continue;
            }
            // Consecutive raster layers: one composited pass (up to STACK layers)
            let mut j = i;
            while j < layers.len() && j - i < STACK && matches!(layers[j].1, Slot::Raster(_)) {
                j += 1;
            }
            let mut stack: Vec<&mut RasterSource> = layers[i..j]
                .iter_mut()
                .filter_map(|(_, slot)| match slot {
                    Slot::Raster(source) => Some(source.as_mut()),
                    Slot::Layer(_) => None,
                })
                .collect();
            for source in &mut stack {
                loading |= source.prepare(&self.gl, &ctx);
            }
            let terrain = self
                .terrain
                .as_mut()
                .map(|(source, exaggeration)| (source, *exaggeration));
            let fog = self.sky.atmosphere.map(|a| a.fog(&view));
            self.raster_pass.draw(
                &self.gl,
                &view,
                &mut stack,
                terrain,
                self.frame,
                now,
                self.loading.fade_ms,
                fog,
            );
            for source in &stack {
                source.finish(&self.redraw);
            }
            i = j;
        }
        self.layers = layers;
        if let Some((terrain, _)) = &self.terrain {
            terrain.finish(&self.redraw);
        }
        self.draw_viewshed(&view);
        loading
    }
}

impl Engine {
    /// The body the map is drawn on
    pub(crate) fn body(&self) -> geo::geodesy::Body {
        self.camera.body
    }

    /// Sets the state every layer starts from, whatever the last frame or a picking pass left (`gl::reset_state`: the
    /// far side of the globe faces away and is culled), then clears the canvas (the depth written, no scissor: all of
    /// it is cleared)
    fn begin_frame(&self, w: f64, h: f64) {
        let gl = &self.gl;
        gl::reset_state(gl);
        // SAFETY: frame setup on the live context
        unsafe {
            gl.viewport(0, 0, w.round() as i32, h.round() as i32);
            let [r, g, b] = self.background;
            gl.clear_color(r, g, b, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
        }
    }

    /// Altitude of the ground grabbed at a screen point: the one grabbed by the last move if it ended there, else
    /// the relief under the point, else the center's
    fn grabbed(&mut self, at: DVec2) -> f64 {
        match self.grab {
            Some((point, altitude)) if point == at => altitude,
            _ => self.pick_altitude(at.x, at.y).unwrap_or(self.camera.elevation),
        }
    }

    /// Places the labels before drawing, from the top layer down: a label shows only where none placed before it does
    fn place_labels(&mut self, layers: &mut [(u32, Slot)], view: &View) {
        let (width, height) = (self.camera.width, self.camera.height);
        let mut grid = self
            .label_grid
            .take()
            .unwrap_or_else(|| geo::collision::Grid::new(width, height));
        grid.reset(width, height);
        {
            let frame = self.frame_state(view);
            for (_, slot) in layers.iter_mut().rev() {
                if let Slot::Layer(layer) = slot {
                    layer.place(&frame, &mut grid);
                }
            }
        }
        self.label_grid = Some(grid);
    }

    /// Altitude (metres, exaggeration included) of the relief under a screen point from the copy kept on the CPU,
    /// without the GPU: the ground point under the ray at an altitude, the altitude there, and again. None without
    /// relief tiles there
    fn cpu_altitude(&self, x: f64, y: f64) -> Option<f64> {
        self.terrain.as_ref()?;
        let view = self.camera.view();
        let mut altitude = self.camera.elevation;
        for _ in 0..3 {
            let m = self.camera.ground_at(&view, x, y, altitude)?;
            altitude = ground_elevation(self.terrain.as_ref(), m)?;
        }
        Some(altitude)
    }

    /// What a layer gets to draw a frame of this view (`Layer::draw`, `Layer::draw_pick`)
    fn frame_state<'a>(&'a self, view: &'a View) -> Frame<'a> {
        Frame {
            gl: &self.gl,
            raw: &self.raw,
            camera: &self.camera,
            view,
            pixel_ratio: self.pixel_ratio,
            number: self.frame,
            loading: self.loading,
            relief: self.terrain.as_ref().map(|(source, e)| (source, *e)),
            ground: &self.raster_pass,
            fog: self.sky.atmosphere.map(|a| a.fog(view)),
            glyphs: self.glyphs.as_ref().map(|g| (g.texture, g.size)),
            icons: self.icons.as_ref().map(|i| (i.texture, i.size)),
        }
    }

    /// Adds a layer on top; returns its id
    pub(crate) fn add_layer(&mut self, layer: Box<dyn Layer>) -> u32 {
        self.push(Slot::Layer(layer))
    }

    /// The layer of this id if it is a `T`, with the context to update it
    pub(crate) fn layer_mut<T: Layer>(&mut self, id: u32) -> Option<(&mut T, &glow::Context)> {
        let Some((_, Slot::Layer(layer))) = self.layers.iter_mut().find(|(i, _)| *i == id) else {
            return None;
        };
        let layer: &mut dyn Any = layer.as_mut();
        Some((layer.downcast_mut()?, &self.gl))
    }

    /// The layer of this id if it is a `T`
    pub(crate) fn layer<T: Layer>(&self, id: u32) -> Option<&T> {
        match self.layers.iter().find(|(i, _)| *i == id) {
            Some((_, Slot::Layer(layer))) => (layer.as_ref() as &dyn Any).downcast_ref(),
            _ => None,
        }
    }

    fn push(&mut self, slot: Slot) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.layers.push((id, slot));
        id
    }

    fn slot(&mut self, id: u32) -> Option<&mut Slot> {
        self.layers.iter_mut().find(|(i, _)| *i == id).map(|(_, slot)| slot)
    }
}

impl Drop for Engine {
    /// Frees every GPU object of the engine and its layers (`free()` from JavaScript): deleting on a lost context is
    /// harmless. The downloads stop as the sources drop
    fn drop(&mut self) {
        let gl = &self.gl;
        for (_, slot) in self.layers.drain(..) {
            match slot {
                Slot::Raster(layer) => layer.delete(gl),
                Slot::Layer(mut layer) => layer.delete(gl),
            }
        }
        if let Some((terrain, _)) = self.terrain.take() {
            terrain.delete(gl);
        }
        self.raster_pass.delete(gl);
        self.globe.delete(gl);
        self.sky.delete(gl);
        self.grid.delete(gl);
        // SAFETY: objects owned by the engine, not used afterwards
        unsafe {
            if let Some(target) = self.pick_target.take() {
                target.delete(gl);
            }
            if let Some(mut viewshed) = self.viewshed.take() {
                viewshed.delete(gl);
            }
            if let Some(mut pick) = self.async_pick.take() {
                pick.delete(gl);
            }
            if let Some(atlas) = self.glyphs.take() {
                gl.delete_texture(atlas.texture);
            }
            if let Some(atlas) = self.icons.take() {
                gl.delete_texture(atlas.texture);
            }
        }
    }
}

/// Altitude of the relief at a Mercator point (metres, exaggeration included), from the coarse copy kept on the CPU:
/// what the camera orbits; None without relief or before its tiles
fn ground_elevation(terrain: Option<&(RasterSource, f32)>, m: DVec2) -> Option<f64> {
    let (source, exaggeration) = terrain?;
    source.altitude(m).map(|h| h * f64::from(*exaggeration))
}
