//! The engine exported to JavaScript

use crate::geo::camera::{self, Camera};
use crate::js;
use crate::render::gl::Grid;
use crate::render::globe::GlobeBase;
use crate::render::network::{NetworkLayer, NetworkStyle};
use crate::render::raster::{DrawContext, RasterOptions, RasterPass, RasterSource, STACK};
use crate::tiles3d::layer::Tiles3dLayer;
use glam::DVec2;
use glow::HasContext;
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

/** The network edge under a screen point */
export interface PickHit {
  /** Id of the network layer */
  layer: number;
  /** Index of the edge in the layer */
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

/** Counters of a 3D Tiles layer */
export interface Tiles3dStats {
  /** Tiles drawn in the last frame */
  drawn: number;
  /** Meshes held on the GPU */
  loaded: number;
  inFlight: number;
  /** GPU memory of the meshes, MB */
  gpuMb: number;
  /** Tiles known in the tree (external tilesets included once loaded) */
  tiles: number;
}
"#;

enum Layer {
    Raster(RasterSource),
    Network(NetworkLayer),
    Tiles3d(Tiles3dLayer),
}

#[wasm_bindgen]
pub struct Engine {
    gl: glow::Context,
    raw: web_sys::WebGl2RenderingContext,
    /// Pool of decoding workers for 3D Tiles (`load`, `abort`), set by the JavaScript side
    loader: Option<JsValue>,
    camera: Camera,
    pixel_ratio: f64,
    layers: Vec<(u32, Layer)>,
    next_id: u32,
    frame: u64,
    redraw: Rc<dyn Fn()>,
    background: [f32; 3],
    globe: GlobeBase,
    raster_pass: RasterPass,
    /// Camera of the last frame and when it last changed (ms)
    last_camera: [u64; 5],
    moved_at: f64,
    /// Destination of a camera flight, with a generation number
    preload: Option<(u64, Camera)>,
    preload_generation: u64,
    /// Offscreen target of the picking pass, created on the first pick
    pick_target: Option<PickTarget>,
}

/// Framebuffer of the picking pass, at the size of the canvas
struct PickTarget {
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
    size: (i32, i32),
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
        attributes.set_power_preference(web_sys::WebGlPowerPreference::HighPerformance);
        let context = canvas
            .get_context_with_context_options("webgl2", &attributes)?
            .ok_or("WebGL2 is not available")?
            .dyn_into::<web_sys::WebGl2RenderingContext>()?;
        // The raw context too: decoded tiles are uploaded straight from JavaScript typed arrays
        let raw = context.clone();
        let gl = glow::Context::from_webgl2_context(context);
        let redraw: Rc<dyn Fn()> = Rc::new(move || {
            let _ = redraw.call0(&JsValue::NULL);
        });
        let grid = Grid::new(&gl)?;
        let globe = GlobeBase::new(&gl, &grid)?;
        let raster_pass = RasterPass::new(&gl, &grid)?;
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
            globe,
            raster_pass,
            last_camera: [0; 5],
            moved_at: 0.0,
            preload: None,
            preload_generation: 0,
            pick_target: None,
        })
    }

    /// Viewport in CSS pixels and device pixel ratio
    pub fn resize(&mut self, width: f64, height: f64, pixel_ratio: f64) {
        self.camera.width = width.max(1.0);
        self.camera.height = height.max(1.0);
        self.pixel_ratio = pixel_ratio;
    }

    #[wasm_bindgen(js_name = setCamera)]
    pub fn set_camera(&mut self, lon: f64, lat: f64, zoom: f64, pitch: f64, bearing: f64) {
        self.camera.center = camera::mercator(lon, lat);
        self.camera.zoom = zoom;
        self.camera.pitch = pitch;
        self.camera.bearing = bearing;
        self.camera.clamp();
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
        self.camera.min_zoom = min;
        self.camera.max_zoom = max;
        self.camera.clamp();
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

    /// Color of the globe under the tiles (polar caps)
    #[wasm_bindgen(js_name = setGlobeColor)]
    pub fn set_globe_color(&mut self, r: f32, g: f32, b: f32) {
        self.globe.color = [r, g, b];
    }

    /// Tile counters of the raster layers, for diagnostics
    #[must_use]
    #[wasm_bindgen(unchecked_return_type = "RasterLayerStats[]")]
    pub fn stats(&self) -> js_sys::Array {
        self.layers
            .iter()
            .filter_map(|(id, layer)| match layer {
                Layer::Raster(layer) => {
                    let s = layer.stats;
                    Some(js::object(&[
                        ("id", (*id).into()),
                        ("drawn", s.drawn.into()),
                        ("loaded", s.loaded.into()),
                        ("inFlight", s.in_flight.into()),
                        ("queued", s.queued.into()),
                        ("standIns", s.stand_ins.into()),
                        ("cooling", s.cooling.into()),
                        ("missing", s.missing.into()),
                    ]))
                }
                _ => None,
            })
            .collect()
    }

    /// Loads in advance the tiles of a camera flight's destination; `clearPreload` when the flight ends
    pub fn preload(&mut self, lon: f64, lat: f64, zoom: f64, pitch: f64, bearing: f64) {
        let mut camera = self.camera.clone();
        camera.center = camera::mercator(lon, lat);
        camera.zoom = zoom;
        camera.pitch = pitch;
        camera.bearing = bearing;
        camera.clamp();
        self.preload_generation += 1;
        self.preload = Some((self.preload_generation, camera));
    }

    #[wasm_bindgen(js_name = clearPreload)]
    pub fn clear_preload(&mut self) {
        self.preload = None;
    }

    /// Feature under a screen point (CSS pixels): the network layer and the index of its edge, undefined if none.
    ///
    /// The network layers are drawn once more into an offscreen target, each edge colored by its number, clipped to
    /// the one pixel under the cursor (scissor), and that pixel is read back (as deck.gl does): the cost does not
    /// grow with the number of features.
    #[wasm_bindgen(unchecked_return_type = "PickHit | undefined")]
    pub fn pick(&mut self, x: f64, y: f64) -> JsValue {
        self.pick_edge(x, y).map_or(JsValue::UNDEFINED, |(layer, index)| {
            js::object(&[("layer", layer.into()), ("index", index.into())])
        })
    }

    /// Drags the ground from one screen point to another (CSS pixels)
    pub fn pan(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.camera.pan(DVec2::new(x0, y0), DVec2::new(x1, y1));
    }

    #[wasm_bindgen(js_name = zoomAt)]
    pub fn zoom_at(&mut self, delta: f64, x: f64, y: f64) {
        self.camera.zoom_at(delta, DVec2::new(x, y));
    }

    /// Turns and tilts around the screen center, in degrees
    pub fn rotate(&mut self, bearing: f64, pitch: f64) {
        self.camera.bearing += bearing;
        self.camera.pitch += pitch;
        self.camera.clamp();
    }

    /// Position under a screen point (CSS pixels), undefined above the horizon
    #[must_use]
    #[wasm_bindgen(unchecked_return_type = "[lon: number, lat: number] | undefined")]
    pub fn ground(&self, x: f64, y: f64) -> JsValue {
        self.camera
            .ground(&self.camera.view(), x, y)
            .map(camera::lon_lat)
            .map_or(JsValue::UNDEFINED, |p| js::pair(p.x, p.y))
    }

    /// Screen point (CSS pixels) of a ground position, undefined behind the camera
    #[must_use]
    #[wasm_bindgen(unchecked_return_type = "[x: number, y: number] | undefined")]
    pub fn project(&self, lon: f64, lat: f64) -> JsValue {
        let view = self.camera.view();
        let p = (camera::mercator(lon, lat) - self.camera.center) * view.world_size;
        let clip = view.view_proj * glam::DVec4::new(p.x, p.y, 0.0, 1.0);
        if clip.w <= 0.0 {
            return JsValue::UNDEFINED;
        }
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        js::pair(
            f64::midpoint(x, 1.0) * self.camera.width,
            f64::midpoint(-y, 1.0) * self.camera.height,
        )
    }

    /// Raster tiles in Web Mercator; `url` holds {z}, {x} and {y}
    #[wasm_bindgen(js_name = addRaster)]
    pub fn add_raster(
        &mut self,
        url: String,
        min_z: u8,
        max_z: u8,
        opacity: f32,
        saturation: f32,
    ) -> Result<u32, JsValue> {
        let layer = RasterSource::new(
            &self.gl,
            RasterOptions {
                url,
                min_z,
                max_z,
                opacity,
                saturation,
            },
        )?;
        Ok(self.push(Layer::Raster(layer)))
    }

    #[wasm_bindgen(js_name = setRasterStyle)]
    pub fn set_raster_style(&mut self, id: u32, opacity: f32, saturation: f32) {
        if let Some(Layer::Raster(layer)) = self.layer(id) {
            layer.options.opacity = opacity;
            layer.options.saturation = saturation;
        }
    }

    /// Network of `lon_lat.length / 2` nodes (degrees, interleaved) and `edges.length / 2` segments (node index
    /// pairs, interleaved)
    #[wasm_bindgen(js_name = addNetwork)]
    pub fn add_network(
        &mut self,
        lon_lat: &[f64],
        edges: &[u32],
        width: f32,
        max: f32,
        contour: f32,
        opacity: f32,
    ) -> Result<u32, JsValue> {
        let style = NetworkStyle {
            width,
            max,
            contour,
            opacity,
        };
        let layer = NetworkLayer::new(&self.gl, lon_lat, edges, style)?;
        Ok(self.push(Layer::Network(layer)))
    }

    /// Pool of workers that fetch and decode 3D Tiles content off the main thread: an object with
    /// `load(id, url, transform, yUp) → Promise<{ anchor, primitives }>` and `abort(id)`. Without it, tiles are
    /// decoded on the main thread. Set it before adding 3D Tiles layers.
    #[wasm_bindgen(js_name = setTileLoader)]
    pub fn set_tile_loader(&mut self, #[wasm_bindgen(unchecked_param_type = "TileLoaderLike | null")] loader: JsValue) {
        self.loader = (!loader.is_null() && !loader.is_undefined()).then_some(loader);
    }

    /// 3D Tiles (b3dm / glb textured meshes, external tilesets) from the URL of a `tileset.json`. `max_error`:
    /// screen-space error (CSS pixels) above which a tile is refined; 16 is the usual value, lower is finer.
    #[wasm_bindgen(js_name = addTiles3d)]
    pub fn add_tiles3d(&mut self, url: String, max_error: f64) -> Result<u32, JsValue> {
        let layer = Tiles3dLayer::new(&self.gl, url, max_error, self.loader.clone(), &self.redraw)?;
        Ok(self.push(Layer::Tiles3d(layer)))
    }

    /// Screen-space error threshold and opacity of a 3D Tiles layer
    #[wasm_bindgen(js_name = setTiles3dStyle)]
    pub fn set_tiles3d_style(&mut self, id: u32, max_error: f64, opacity: f32) {
        if let Some(Layer::Tiles3d(layer)) = self.layer(id) {
            layer.max_error = max_error;
            layer.opacity = opacity;
        }
    }

    /// Counters of a 3D Tiles layer, undefined if there is none with this id
    #[wasm_bindgen(js_name = tiles3dStats, unchecked_return_type = "Tiles3dStats | undefined")]
    #[must_use]
    pub fn tiles3d_stats(&self, id: u32) -> JsValue {
        self.layers
            .iter()
            .find_map(|(i, layer)| match layer {
                Layer::Tiles3d(layer) if *i == id => {
                    let s = layer.stats;
                    Some(js::object(&[
                        ("drawn", s.drawn.into()),
                        ("loaded", s.loaded.into()),
                        ("inFlight", s.in_flight.into()),
                        ("gpuMb", (s.gpu_bytes as f64 / 1_048_576.0).into()),
                        ("tiles", s.tiles.into()),
                    ]))
                }
                _ => None,
            })
            .unwrap_or(JsValue::UNDEFINED)
    }

    /// One value per node; negative = unreached
    #[wasm_bindgen(js_name = setNetworkValues)]
    pub fn set_network_values(&mut self, id: u32, values: &[f32]) {
        if let Some(index) = self.layers.iter().position(|(i, _)| *i == id)
            && let Layer::Network(layer) = &mut self.layers[index].1
        {
            layer.set_values(&self.gl, values);
        }
    }

    #[wasm_bindgen(js_name = setNetworkStyle)]
    pub fn set_network_style(&mut self, id: u32, width: f32, max: f32, contour: f32, opacity: f32) {
        if let Some(Layer::Network(layer)) = self.layer(id) {
            layer.style = NetworkStyle {
                width,
                max,
                contour,
                opacity,
            };
        }
    }

    #[wasm_bindgen(js_name = removeLayer)]
    pub fn remove_layer(&mut self, id: u32) {
        if let Some(index) = self.layers.iter().position(|(i, _)| *i == id) {
            match self.layers.remove(index).1 {
                Layer::Raster(layer) => layer.delete(&self.gl),
                Layer::Network(layer) => layer.delete(&self.gl),
                Layer::Tiles3d(mut layer) => layer.delete(&self.gl),
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
    pub fn render(&mut self) -> bool {
        self.frame += 1;
        let view = self.camera.view();
        let (w, h) = (
            self.camera.width * self.pixel_ratio,
            self.camera.height * self.pixel_ratio,
        );
        let mut loading = false;
        // SAFETY: frame setup on the live context
        unsafe {
            let gl = &self.gl;
            gl.viewport(0, 0, w.round() as i32, h.round() as i32);
            let [r, g, b] = self.background;
            gl.clear_color(r, g, b, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            gl.enable(glow::BLEND);
            // The far side of the globe faces away: culled
            gl.enable(glow::CULL_FACE);
            gl.front_face(glow::CW);
            gl.blend_func_separate(
                glow::SRC_ALPHA,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }
        // Camera motion, for the foveated loading
        let key = [
            self.camera.center.x,
            self.camera.center.y,
            self.camera.zoom,
            self.camera.pitch,
            self.camera.bearing,
        ]
        .map(f64::to_bits);
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
        self.globe.draw(&self.gl, &view);
        let ctx = DrawContext {
            view: &view,
            frame: self.frame,
            redraw: &self.redraw,
            moving,
            preload: preload.as_ref().map(|(generation, v)| (*generation, v)),
        };
        let mut i = 0;
        while i < self.layers.len() {
            if let Layer::Tiles3d(layer) = &mut self.layers[i].1 {
                loading |= layer.draw(&self.gl, &self.raw, &view, self.frame, &self.redraw);
                i += 1;
                continue;
            }
            if matches!(self.layers[i].1, Layer::Network(_)) {
                if let Layer::Network(layer) = &self.layers[i].1 {
                    // Line quads are built on screen, either way round
                    // SAFETY: state changes on the live context
                    unsafe { self.gl.disable(glow::CULL_FACE) };
                    layer.draw(&self.gl, &self.camera, &view, self.pixel_ratio);
                    // SAFETY: state change on the live context
                    unsafe { self.gl.enable(glow::CULL_FACE) };
                }
                i += 1;
                continue;
            }
            // Consecutive raster layers: one composited pass (up to STACK layers)
            let mut j = i;
            while j < self.layers.len() && j - i < STACK && matches!(self.layers[j].1, Layer::Raster(_)) {
                j += 1;
            }
            let mut stack: Vec<&mut RasterSource> = self.layers[i..j]
                .iter_mut()
                .filter_map(|(_, layer)| match layer {
                    Layer::Raster(source) => Some(source),
                    Layer::Network(_) | Layer::Tiles3d(_) => None,
                })
                .collect();
            for source in &mut stack {
                loading |= source.prepare(&self.gl, &ctx);
            }
            self.raster_pass.draw(&self.gl, &view, &mut stack, self.frame);
            for source in &stack {
                source.finish(&self.redraw);
            }
            i = j;
        }
        loading
    }
}

impl Engine {
    /// Network layer id and edge index under a screen point (see `pick`)
    fn pick_edge(&mut self, x: f64, y: f64) -> Option<(u32, u32)> {
        let networks: Vec<(u32, usize)> = self
            .layers
            .iter()
            .enumerate()
            .filter(|(_, (_, layer))| matches!(layer, Layer::Network(_)))
            .map(|(i, (id, _))| (*id, i))
            .collect();
        if networks.is_empty() || networks.len() > 255 {
            return None;
        }
        let size = (
            (self.camera.width * self.pixel_ratio).round() as i32,
            (self.camera.height * self.pixel_ratio).round() as i32,
        );
        let (px, py) = (
            (x * self.pixel_ratio) as i32,
            size.1 - 1 - (y * self.pixel_ratio) as i32,
        );
        if px < 0 || py < 0 || px >= size.0 || py >= size.1 {
            return None;
        }
        let target = self.pick_target(size).ok()?;
        let view = self.camera.view();
        let mut pixel = [0u8; 4];
        // SAFETY: offscreen pass on the live context, default framebuffer restored after
        unsafe {
            let gl = &self.gl;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
            gl.viewport(0, 0, size.0, size.1);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(px, py, 1, 1);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            for (ordinal, &(_, index)) in networks.iter().enumerate() {
                if let Layer::Network(layer) = &self.layers[index].1 {
                    layer.draw_pick(gl, &self.camera, &view, self.pixel_ratio, (ordinal + 1) as u8);
                }
            }
            gl.read_pixels(
                px,
                py,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixel)),
            );
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        let [r, g, b, layer] = pixel;
        let id = u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16;
        if layer == 0 || id == 0 {
            return None;
        }
        let (layer_id, _) = networks.get(usize::from(layer) - 1)?;
        Some((*layer_id, id - 1))
    }

    /// The picking framebuffer, (re)created at the canvas size
    fn pick_target(&mut self, size: (i32, i32)) -> Result<glow::Framebuffer, String> {
        if let Some(target) = &self.pick_target
            && target.size == size
        {
            return Ok(target.framebuffer);
        }
        // SAFETY: object creation on the live context; the previous target is deleted
        unsafe {
            let gl = &self.gl;
            if let Some(old) = self.pick_target.take() {
                gl.delete_framebuffer(old.framebuffer);
                gl.delete_texture(old.texture);
            }
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, size.0, size.1);
            let framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.pick_target = Some(PickTarget {
                framebuffer,
                texture,
                size,
            });
            Ok(framebuffer)
        }
    }

    fn push(&mut self, layer: Layer) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.layers.push((id, layer));
        id
    }

    fn layer(&mut self, id: u32) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|(i, _)| *i == id).map(|(_, l)| l)
    }
}
