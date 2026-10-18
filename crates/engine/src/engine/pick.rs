//! Picking: what is under a screen point. The scene is drawn once more into an offscreen target, clipped to the
//! pixels around the cursor (scissor), and those pixels are read back (as deck.gl does): the cost does not grow with
//! the number of features. Within the radius, the feature nearest to the cursor wins. Fills are picked on the CPU
//! instead: the ground position under the cursor tested against their polygons.
//!
//! `pick` reads the pixels back at once, which waits for the GPU to finish the frame: fine for a click. For the
//! hover, `requestPick` draws the same pass but reads into a pixel buffer behind a fence, and `pollPick` hands the
//! result over on a later frame, once the GPU is done: the main thread never waits.

use super::{Engine, Slot};
use crate::js;
use crate::render::gl::{self, Objects};
use crate::render::raster::unpack_depth;
use geo::camera::{self, Camera, View};
use glam::DVec2;
use glow::HasContext;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
/** The answer to `requestPick`, from `pollPick` */
export interface PickResult {
  /** The screen point asked about (CSS pixels) */
  x: number;
  y: number;
  /** The feature there, if any */
  hit?: PickHit;
  /** The position there, on the relief; undefined above the horizon */
  lonLat?: [lon: number, lat: number];
}
"#;

/// Framebuffer of the picking pass, at the size of the canvas
pub(super) struct PickTarget {
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
    /// For the ground's depth (`pick_altitude`)
    depth: glow::Renderbuffer,
    size: (i32, i32),
}

/// The picking pass around a screen point: the layers drawn and the block of pixels read back
#[derive(Clone)]
struct FeaturePass {
    /// Ids of the layers drawn, by ordinal (alpha channel) - 1
    pickable: Vec<u32>,
    /// Their indices in the engine's layers
    slots: Vec<usize>,
    /// The cursor in device pixels (y up), the block around it (x0, y0, w, h) and the radius
    cursor: (i32, i32),
    block: (i32, i32, i32, i32),
    radius: i32,
}

impl FeaturePass {
    fn bytes(&self) -> usize {
        (self.block.2 * self.block.3 * 4) as usize
    }

    /// The hit in the pixels read back: layer id and feature index
    fn hit(&self, pixels: &[u8]) -> Option<(u32, u32)> {
        let [r, g, b, layer] = nearest(pixels, self.block, self.cursor, self.radius)?;
        let id = u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16;
        if layer == 0 || id == 0 {
            return None;
        }
        let layer_id = self.pickable.get(usize::from(layer) - 1)?;
        Some((*layer_id, id - 1))
    }
}

/// The pixel nearest to the cursor within `radius` that holds a feature (alpha not 0); `block`: x0, y0, width,
/// height of the pixels (RGBA rows, from the bottom)
fn nearest(pixels: &[u8], block: (i32, i32, i32, i32), cursor: (i32, i32), radius: i32) -> Option<[u8; 4]> {
    let (x0, y0, w, _) = block;
    let w = i64::from(w.max(1));
    // Squared distances in i64: a radius of 50 000 pixels squared is past i32
    let radius = i64::from(radius);
    let mut best: Option<(i64, [u8; 4])> = None;
    for (k, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        let (i, j) = (k as i64 % w + i64::from(x0), k as i64 / w + i64::from(y0));
        let d = (i - i64::from(cursor.0)).pow(2) + (j - i64::from(cursor.1)).pow(2);
        if pixel[3] != 0 && d <= radius * radius && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, *pixel));
        }
    }
    best.map(|(_, pixel)| pixel)
}

/// The block of pixels around the cursor (device pixels, y up) inside a canvas of `size`, and the radius in device
/// pixels: `radius` (device pixels) no larger than the canvas's diagonal, 0 if not a number
fn block_around(cursor: (i32, i32), radius: f64, size: (i32, i32)) -> ((i32, i32, i32, i32), i32) {
    let diagonal = f64::from(size.0).hypot(f64::from(size.1)).ceil();
    let r = radius.max(0.0).min(diagonal).round() as i32;
    let (px, py) = cursor;
    let (x0, y0) = (px.saturating_sub(r).max(0), py.saturating_sub(r).max(0));
    let (x1, y1) = (
        px.saturating_add(r).min(size.0 - 1),
        py.saturating_add(r).min(size.1 - 1),
    );
    ((x0, y0, x1 - x0 + 1, y1 - y0 + 1), r)
}

/// A picking request waiting for the GPU (`requestPick`)
struct PendingPick {
    /// Signaled once the pixels are in the buffer; None: nothing was read back, the answer is ready
    fence: Option<glow::Fence>,
    /// The screen point (CSS pixels)
    at: (f64, f64),
    /// Layers looked in (all if None)
    only: Option<Vec<u32>>,
    feature: Option<FeaturePass>,
    /// The ground's depth was read after the feature pixels (relief)
    depth: bool,
    /// The camera then, and the view the depth was drawn with
    camera: Camera,
    drawn: Option<View>,
}

/// The pixel buffer of the asynchronous picking and the request waiting for it
pub(super) struct AsyncPick {
    buffer: glow::Buffer,
    pending: Option<PendingPick>,
}

impl AsyncPick {
    /// Call with the context it was made on
    pub(super) unsafe fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by the picking, not used afterwards
        unsafe {
            if let Some(fence) = self.pending.take().and_then(|p| p.fence) {
                gl.delete_sync(fence);
            }
            gl.delete_buffer(self.buffer);
        }
    }
}

#[wasm_bindgen]
impl Engine {
    /// Feature under a screen point (CSS pixels), or the nearest one within `radius` CSS pixels: its layer and its
    /// index there (the edge of a network, the feature of a line, circle or fill layer), undefined if none. The
    /// pickable layers are drawn with each feature colored by its number; then the fills are tested on the ground.
    /// `only`: the ids of the layers to look in (all if undefined): the others are not drawn and hide nothing.
    /// Reads the pixels back at once (the GPU finishes its work first): for clicks; `requestPick` for the hover
    #[wasm_bindgen(unchecked_return_type = "PickHit | undefined")]
    #[allow(clippy::needless_pass_by_value)]
    pub fn pick(&mut self, x: f64, y: f64, radius: Option<f64>, only: Option<Box<[u32]>>) -> JsValue {
        let only = only.as_deref();
        let hit = self
            .pick_feature(x, y, radius.unwrap_or(0.0), only)
            .or_else(|| self.pick_fill(x, y, only));
        hit_value(hit)
    }

    /// Asks what is under a screen point without waiting for the GPU (as `pick`, with the position there): the
    /// picking pass is drawn now and read back into a buffer; `pollPick` gives the answer on a later frame. A new
    /// request replaces the one in flight. Returns false if nothing could be asked (no WebGL2 fence)
    #[wasm_bindgen(js_name = requestPick)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn request_pick(&mut self, x: f64, y: f64, radius: Option<f64>, only: Option<Box<[u32]>>) -> bool {
        // The request in flight is dropped: only the latest one counts
        if let Some(fence) = self
            .async_pick
            .as_mut()
            .and_then(|a| a.pending.take())
            .and_then(|p| p.fence)
        {
            // SAFETY: a fence of this context, not used afterwards
            unsafe { self.gl.delete_sync(fence) };
        }
        let only = only.map(Vec::from);
        let feature = self.feature_pass(x, y, radius.unwrap_or(0.0), only.as_deref());
        // The relief's depth under the cursor: for the fills and the position
        let size = self.canvas_size();
        let cursor = self.device_point(x, y, size);
        let depth = self.terrain.is_some() && self.drawn.is_some() && cursor.is_some();
        let mut pending = PendingPick {
            fence: None,
            at: (x, y),
            only,
            feature,
            depth,
            camera: self.camera.clone(),
            drawn: self.drawn.clone(),
        };
        if self.ensure_async_pick().is_err() {
            return false;
        }
        if pending.feature.is_some() || depth {
            match self.read_async(&pending, cursor) {
                Ok(fence) => pending.fence = Some(fence),
                Err(_) => return false,
            }
        }
        if let Some(pick) = &mut self.async_pick {
            pick.pending = Some(pending);
        }
        true
    }

    /// The answer to the last `requestPick` once the GPU has delivered it (then forgotten), undefined while it is not
    /// there yet (or nothing was asked, or the context was lost)
    #[wasm_bindgen(js_name = pollPick, unchecked_return_type = "PickResult | undefined")]
    pub fn poll_pick(&mut self) -> JsValue {
        let Some(pick) = &mut self.async_pick else {
            return JsValue::UNDEFINED;
        };
        let Some(pending) = &pick.pending else {
            return JsValue::UNDEFINED;
        };
        let lost = self.raw.is_context_lost();
        if let Some(fence) = pending.fence
            && !lost
        {
            // SAFETY: status of a fence of this context
            if unsafe { self.gl.get_sync_status(fence) } != glow::SIGNALED {
                return JsValue::UNDEFINED;
            }
        }
        let Some(pending) = pick.pending.take() else {
            return JsValue::UNDEFINED;
        };
        let mut bytes =
            vec![0u8; pending.feature.as_ref().map_or(0, FeaturePass::bytes) + 4 * usize::from(pending.depth)];
        // SAFETY: the fence is signaled: the buffer holds the pixels, read without a stall; deleted after
        unsafe {
            let gl = &self.gl;
            if let Some(fence) = pending.fence {
                if !lost && !bytes.is_empty() {
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(pick.buffer));
                    gl.get_buffer_sub_data(glow::PIXEL_PACK_BUFFER, 0, &mut bytes);
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                }
                gl.delete_sync(fence);
            }
        }
        if lost {
            return JsValue::UNDEFINED;
        }
        self.resolve(&pending, &bytes)
    }
}

impl PickTarget {
    /// Call with the target's context
    pub(super) unsafe fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by the target, not used afterwards
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_texture(self.texture);
            gl.delete_renderbuffer(self.depth);
        }
    }
}

/// A hit as JavaScript: `{ layer, index }` or undefined
fn hit_value(hit: Option<(u32, u32)>) -> JsValue {
    hit.map_or(JsValue::UNDEFINED, |(layer, index)| {
        js::object(&[("layer", layer.into()), ("index", index.into())])
    })
}

impl Engine {
    /// The canvas in device pixels
    fn canvas_size(&self) -> (i32, i32) {
        (
            (self.camera.width * self.pixel_ratio).round() as i32,
            (self.camera.height * self.pixel_ratio).round() as i32,
        )
    }

    /// A screen point (CSS pixels) in device pixels, y up; None outside the canvas
    fn device_point(&self, x: f64, y: f64, size: (i32, i32)) -> Option<(i32, i32)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let (px, py) = (
            (x * self.pixel_ratio) as i32,
            size.1 - 1 - (y * self.pixel_ratio) as i32,
        );
        (px >= 0 && py >= 0 && px < size.0 && py < size.1).then_some((px, py))
    }

    /// Draws the ground's depth at a pixel into the bound picking target (its color holds the depth, packed). Call
    /// with the target bound and the viewport set; the state is set back but for the framebuffer
    unsafe fn draw_ground_depth(&self, view: &View, (px, py): (i32, i32)) {
        let Some((terrain, _)) = self.terrain.as_ref() else {
            return;
        };
        let gl = &self.gl;
        gl::reset_state(gl);
        // SAFETY: offscreen pass on the live context, the state set back after
        unsafe {
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(px, py, 1, 1);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            self.raster_pass.draw_depth(gl, view, terrain);
        }
        gl::reset_state(gl);
    }

    /// Altitude (metres, exaggeration included) of the relief drawn under a screen point (CSS pixels), None without
    /// relief or off the ground. The ground of the last frame is drawn again into the picking target, its depth
    /// packed in the color, clipped to that pixel, and the pixel read back.
    pub(super) fn pick_altitude(&mut self, x: f64, y: f64) -> Option<f64> {
        self.terrain.as_ref()?;
        let view = self.drawn.clone()?;
        let size = self.canvas_size();
        let cursor = self.device_point(x, y, size)?;
        let target = self.pick_target(size).ok()?;
        let mut pixel = [0u8; 4];
        // SAFETY: offscreen pass on the live context, default framebuffer restored after
        unsafe {
            let gl = &self.gl;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
            gl.viewport(0, 0, size.0, size.1);
            self.draw_ground_depth(&view, cursor);
            gl.read_pixels(
                cursor.0,
                cursor.1,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixel)),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        altitude_of_depth(&self.camera, &view, x, y, pixel)
    }

    /// Fill under a screen point: the topmost layer whose polygons hold the ground position there. The ground's depth
    /// is read back only when there is a fill layer to test
    fn pick_fill(&mut self, x: f64, y: f64, only: Option<&[u32]>) -> Option<(u32, u32)> {
        if !self.picks_ground(only) {
            return None;
        }
        let altitude = self.pick_altitude(x, y).unwrap_or(self.camera.elevation);
        let m = self.camera.ground_at(&self.camera.view(), x, y, altitude)?;
        self.fill_at(m, only)
    }

    /// Whether a layer looked in is picked on the ground (fills)
    fn picks_ground(&self, only: Option<&[u32]>) -> bool {
        self.layers.iter().any(|(id, slot)| {
            matches!(slot, Slot::Layer(layer) if layer.picks_ground()) && only.is_none_or(|o| o.contains(id))
        })
    }

    /// The topmost layer picked on the ground holding a Mercator position, and its feature there
    fn fill_at(&self, m: DVec2, only: Option<&[u32]>) -> Option<(u32, u32)> {
        let zoom = self.camera.zoom;
        self.layers.iter().rev().find_map(|(id, slot)| match slot {
            Slot::Layer(layer) if only.is_none_or(|o| o.contains(id)) => {
                layer.pick_ground(m, zoom).map(|index| (*id, index))
            }
            Slot::Layer(_) | Slot::Raster(_) => None,
        })
    }

    /// The picking pass around a screen point (CSS pixels), None if no layer is pickable or the point is off the
    /// canvas
    fn feature_pass(&self, x: f64, y: f64, radius: f64, only: Option<&[u32]>) -> Option<FeaturePass> {
        let (pickable, slots): (Vec<u32>, Vec<usize>) = self
            .layers
            .iter()
            .enumerate()
            .filter(|(_, (id, slot))| {
                only.is_none_or(|o| o.contains(id)) && matches!(slot, Slot::Layer(layer) if layer.pickable())
            })
            .map(|(i, (id, _))| (*id, i))
            .unzip();
        // Their ordinal goes in the alpha channel
        if pickable.is_empty() || pickable.len() > 255 {
            return None;
        }
        let size = self.canvas_size();
        let (px, py) = self.device_point(x, y, size)?;
        // The block of pixels around the cursor, inside the canvas
        let (block, radius) = block_around((px, py), radius * self.pixel_ratio, size);
        Some(FeaturePass {
            pickable,
            slots,
            cursor: (px, py),
            block,
            radius,
        })
    }

    /// Draws a picking pass into the bound picking target. Call with the target bound and the viewport set; the state
    /// is set back (`gl::reset_state`) but for the framebuffer.
    ///
    /// In relief the ground's depth is drawn first (no color), clipped to the block as the rest: the layers test
    /// against it as on screen, and a feature behind a hill is not picked. It is the ground of the last frame, drawn
    /// with that frame's view (its tiles are placed for it): the whole pass is, what is on screen.
    unsafe fn draw_feature_pass(&self, pass: &FeaturePass) {
        let view = match (&self.terrain, &self.drawn) {
            (Some(_), Some(drawn)) => drawn.clone(),
            _ => self.camera.view(),
        };
        let (x0, y0, w, h) = pass.block;
        let gl = &self.gl;
        gl::reset_state(gl);
        // SAFETY: offscreen pass on the live context, the state set back after
        unsafe {
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x0, y0, w, h);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            // Depth too: the blocks of extrusion layers hide one another, the hills what lies behind them
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            if let Some((terrain, _)) = &self.terrain {
                gl.color_mask(false, false, false, false);
                gl.enable(glow::DEPTH_TEST);
                self.raster_pass.draw_depth(gl, &view, terrain);
                gl.disable(glow::DEPTH_TEST);
                gl.color_mask(true, true, true, true);
            }
            let frame = self.frame_state(&view);
            for (ordinal, &index) in pass.slots.iter().enumerate() {
                if let Some((_, Slot::Layer(layer))) = self.layers.get(index) {
                    layer.draw_pick(&frame, (ordinal + 1) as u8);
                }
            }
        }
        gl::reset_state(gl);
    }

    /// Layer id and feature index under a screen point, or nearest to it within `radius` CSS pixels (see `pick`)
    fn pick_feature(&mut self, x: f64, y: f64, radius: f64, only: Option<&[u32]>) -> Option<(u32, u32)> {
        let pass = self.feature_pass(x, y, radius, only)?;
        let size = self.canvas_size();
        let target = self.pick_target(size).ok()?;
        let mut pixels = vec![0u8; pass.bytes()];
        let (x0, y0, w, h) = pass.block;
        // SAFETY: offscreen pass on the live context, default framebuffer restored after
        unsafe {
            let gl = &self.gl;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
            gl.viewport(0, 0, size.0, size.1);
            self.draw_feature_pass(&pass);
            gl.read_pixels(
                x0,
                y0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        pass.hit(&pixels)
    }

    /// The pixel buffer of the asynchronous picking, created on the first request
    fn ensure_async_pick(&mut self) -> Result<glow::Buffer, String> {
        if let Some(pick) = &self.async_pick {
            return Ok(pick.buffer);
        }
        let mut objects = Objects::new(&self.gl);
        let buffer = objects.buffer()?;
        objects.keep();
        self.async_pick = Some(AsyncPick { buffer, pending: None });
        Ok(buffer)
    }

    /// Draws the passes of a request and reads their pixels into the picking buffer (the feature block first, then
    /// the ground's depth pixel), behind a fence
    fn read_async(&mut self, pending: &PendingPick, cursor: Option<(i32, i32)>) -> Result<glow::Fence, String> {
        let size = self.canvas_size();
        let target = self.pick_target(size)?;
        let buffer = self.ensure_async_pick()?;
        let feature_bytes = pending.feature.as_ref().map_or(0, FeaturePass::bytes);
        let bytes = feature_bytes + 4 * usize::from(pending.depth);
        // SAFETY: offscreen passes on the live context; the pack buffer and the default framebuffer unbound after
        unsafe {
            let gl = &self.gl;
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
            // Allocated again at each request: the reads of the one replaced may still be under way
            gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, bytes as i32, glow::STREAM_READ);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
            gl.viewport(0, 0, size.0, size.1);
            if let Some(pass) = &pending.feature {
                self.draw_feature_pass(pass);
                let (x0, y0, w, h) = pass.block;
                gl.read_pixels(
                    x0,
                    y0,
                    w,
                    h,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::BufferOffset(0),
                );
            }
            if let (true, Some(view), Some(cursor)) = (pending.depth, &pending.drawn, cursor) {
                self.draw_ground_depth(view, cursor);
                gl.read_pixels(
                    cursor.0,
                    cursor.1,
                    1,
                    1,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::BufferOffset(feature_bytes as u32),
                );
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            // A pack buffer left bound would turn the next `pick` read into an error
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            let fence = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0)?;
            // Sent to the GPU now, so that the fence can signal without waiting for the next frame
            gl.flush();
            Ok(fence)
        }
    }

    /// The answer to a request from its pixels: the feature hit (else the fill under the ground position) and the
    /// position
    fn resolve(&self, pending: &PendingPick, bytes: &[u8]) -> JsValue {
        let (x, y) = pending.at;
        let only = pending.only.as_deref();
        let feature_bytes = pending.feature.as_ref().map_or(0, FeaturePass::bytes);
        let altitude = if pending.depth {
            let pixel = bytes
                .get(feature_bytes..feature_bytes + 4)
                .and_then(|p| <[u8; 4]>::try_from(p).ok());
            pending
                .drawn
                .as_ref()
                .zip(pixel)
                .and_then(|(view, pixel)| altitude_of_depth(&pending.camera, view, x, y, pixel))
        } else {
            None
        };
        let camera = &pending.camera;
        let m = camera.ground_at(&camera.view(), x, y, altitude.unwrap_or(camera.elevation));
        let hit = pending
            .feature
            .as_ref()
            .and_then(|pass| pass.hit(bytes.get(..feature_bytes)?))
            // A layer removed meanwhile is no hit
            .filter(|(layer, _)| self.layers.iter().any(|(id, _)| id == layer))
            .or_else(|| {
                m.filter(|_| self.picks_ground(only))
                    .and_then(|m| self.fill_at(m, only))
            });
        let lon_lat = m.map(camera::lon_lat);
        js::object(&[
            ("x", x.into()),
            ("y", y.into()),
            ("hit", hit_value(hit)),
            ("lonLat", lon_lat.map_or(JsValue::UNDEFINED, |p| js::pair(p.x, p.y))),
        ])
    }

    /// The picking framebuffer, (re)created at the canvas size
    fn pick_target(&mut self, size: (i32, i32)) -> Result<glow::Framebuffer, String> {
        if let Some(target) = &self.pick_target
            && target.size == size
        {
            return Ok(target.framebuffer);
        }
        if let Some(old) = self.pick_target.take() {
            // SAFETY: the previous target, replaced
            unsafe { old.delete(&self.gl) };
        }
        let gl = &self.gl;
        // Everything created is deleted again if a step fails
        let mut objects = Objects::new(gl);
        let texture = objects.texture()?;
        let framebuffer = objects.framebuffer()?;
        let depth = objects.renderbuffer()?;
        objects.keep();
        // SAFETY: storage and attachments of objects just created on the live context
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, size.0, size.1);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, size.0, size.1);
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(depth),
            );
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.pick_target = Some(PickTarget {
            framebuffer,
            texture,
            depth,
            size,
        });
        Ok(framebuffer)
    }
}

/// Altitude (metres) of the ground at a screen point (CSS pixels) from its depth packed in a pixel (`draw_depth`),
/// drawn with `view`; None where no ground was drawn
fn altitude_of_depth(camera: &Camera, view: &View, x: f64, y: f64, pixel: [u8; 4]) -> Option<f64> {
    if pixel[3] == 0 {
        return None;
    }
    let depth = unpack_depth(pixel);
    let ndc = glam::DVec4::new(
        2.0 * x / camera.width - 1.0,
        1.0 - 2.0 * y / camera.height,
        2.0 * depth - 1.0,
        1.0,
    );
    let p = view.inverse * ndc;
    Some(Camera::altitude_of(view, p.truncate() / p.w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_hit_within_the_radius_wins() {
        // A 3 × 3 block from (10, 20), the cursor in its middle
        let mut pixels = vec![0u8; 9 * 4];
        // A corner (distance² 2) and the pixel right of the cursor (distance² 1)
        pixels[0..4].copy_from_slice(&[1, 0, 0, 1]);
        pixels[5 * 4..6 * 4].copy_from_slice(&[2, 0, 0, 1]);
        assert_eq!(nearest(&pixels, (10, 20, 3, 3), (11, 21), 1), Some([2, 0, 0, 1]));
        // Radius 0: the cursor's pixel only, empty
        assert_eq!(nearest(&pixels, (10, 20, 3, 3), (11, 21), 0), None);
        let pass = FeaturePass {
            pickable: vec![7, 9],
            slots: vec![0, 1],
            cursor: (11, 21),
            block: (10, 20, 3, 3),
            radius: 1,
        };
        // Feature 2 (id 2 = index 1) of the first layer drawn
        assert_eq!(pass.hit(&pixels), Some((7, 1)));
        assert_eq!(pass.bytes(), 36);
        // An ordinal past the layers drawn: no hit
        pixels[5 * 4 + 3] = 5;
        assert_eq!(pass.hit(&pixels), None);
    }

    #[test]
    fn the_radius_stays_within_the_canvas() {
        let size = (300, 200);
        assert_eq!(block_around((10, 20), 2.0, size), ((8, 18, 5, 5), 2));
        // Clipped at the edges of the canvas
        assert_eq!(block_around((0, 199), 3.0, size), ((0, 196, 4, 4), 3));
        // Infinite, huge, negative or not a number: at most the diagonal, the block the canvas at most
        for radius in [f64::INFINITY, 1e12, 50_000.0] {
            let ((x0, y0, w, h), r) = block_around((150, 100), radius, size);
            assert_eq!((x0, y0, w, h, r), (0, 0, 300, 200, 361), "{radius}");
        }
        assert_eq!(block_around((10, 20), f64::NAN, size), ((10, 20, 1, 1), 0));
        assert_eq!(block_around((10, 20), -4.0, size), ((10, 20, 1, 1), 0));
        // A radius whose square is past i32 still finds the pixel
        let mut pixels = vec![0u8; 4];
        pixels[3] = 1;
        assert_eq!(
            nearest(&pixels, (0, 0, 1, 1), (40_000, 40_000), 60_000),
            Some([0, 0, 0, 1])
        );
        assert_eq!(nearest(&pixels, (0, 0, 1, 1), (40_000, 40_000), 50_000), None);
    }
}
