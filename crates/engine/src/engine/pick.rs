//! Picking: what is under a screen point. The scene is drawn once more into an offscreen target, clipped to the
//! pixels around the cursor (scissor), and those pixels are read back (as deck.gl does): the cost does not grow with
//! the number of features. Within the radius, the feature nearest to the cursor wins. Fills are picked on the CPU
//! instead: the ground position under the cursor tested against their polygons.

use super::{Engine, Slot};
use crate::js;
use crate::render::gl::Objects;
use crate::render::raster::unpack_depth;
use geo::camera::Camera;
use glow::HasContext;
use wasm_bindgen::prelude::*;

/// Framebuffer of the picking pass, at the size of the canvas
pub(super) struct PickTarget {
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
    /// For the ground's depth (`pick_altitude`)
    depth: glow::Renderbuffer,
    size: (i32, i32),
}

#[wasm_bindgen]
impl Engine {
    /// Feature under a screen point (CSS pixels), or the nearest one within `radius` CSS pixels: its layer and its
    /// index there (the edge of a network, the feature of a line, circle or fill layer), undefined if none. The
    /// pickable layers are drawn with each feature colored by its number; then the fills are tested on the ground.
    /// `only`: the ids of the layers to look in (all if undefined): the others are not drawn and hide nothing
    #[wasm_bindgen(unchecked_return_type = "PickHit | undefined")]
    #[allow(clippy::needless_pass_by_value)]
    pub fn pick(&mut self, x: f64, y: f64, radius: Option<f64>, only: Option<Box<[u32]>>) -> JsValue {
        let only = only.as_deref();
        let hit = self
            .pick_feature(x, y, radius.unwrap_or(0.0), only)
            .or_else(|| self.pick_fill(x, y, only));
        hit.map_or(JsValue::UNDEFINED, |(layer, index)| {
            js::object(&[("layer", layer.into()), ("index", index.into())])
        })
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

impl Engine {
    /// Altitude (metres, exaggeration included) of the relief drawn under a screen point (CSS pixels), None without
    /// relief or off the ground. The ground of the last frame is drawn again into the picking target, its depth
    /// packed in the color, clipped to that pixel, and the pixel read back.
    pub(super) fn pick_altitude(&mut self, x: f64, y: f64) -> Option<f64> {
        self.terrain.as_ref()?;
        let view = self.drawn.clone()?;
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
        let (terrain, _) = self.terrain.as_ref()?;
        let mut pixel = [0u8; 4];
        // SAFETY: offscreen pass on the live context, default framebuffer and state restored after
        unsafe {
            let gl = &self.gl;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
            gl.viewport(0, 0, size.0, size.1);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(px, py, 1, 1);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.depth_mask(true);
            self.raster_pass.draw_depth(gl, &view, terrain);
            gl.read_pixels(
                px,
                py,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixel)),
            );
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.enable(glow::BLEND);
            gl.enable(glow::CULL_FACE);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        if pixel[3] == 0 {
            return None;
        }
        let depth = unpack_depth(pixel);
        let ndc = glam::DVec4::new(
            2.0 * x / self.camera.width - 1.0,
            1.0 - 2.0 * y / self.camera.height,
            2.0 * depth - 1.0,
            1.0,
        );
        let p = view.inverse * ndc;
        Some(Camera::altitude_of(&view, p.truncate() / p.w))
    }

    /// Fill under a screen point: the topmost layer whose polygons hold the ground position there. The ground's depth
    /// is read back only when there is a fill layer to test
    fn pick_fill(&mut self, x: f64, y: f64, only: Option<&[u32]>) -> Option<(u32, u32)> {
        let any = self.layers.iter().any(|(id, slot)| {
            matches!(slot, Slot::Layer(layer) if layer.picks_ground()) && only.is_none_or(|o| o.contains(id))
        });
        if !any {
            return None;
        }
        let altitude = self.pick_altitude(x, y).unwrap_or(self.camera.elevation);
        let m = self.camera.ground_at(&self.camera.view(), x, y, altitude)?;
        let zoom = self.camera.zoom;
        self.layers.iter().rev().find_map(|(id, slot)| match slot {
            Slot::Layer(layer) if only.is_none_or(|o| o.contains(id)) => {
                layer.pick_ground(m, zoom).map(|index| (*id, index))
            }
            Slot::Layer(_) | Slot::Raster(_) => None,
        })
    }

    /// Layer id and feature index under a screen point, or nearest to it within `radius` CSS pixels (see `pick`)
    fn pick_feature(&mut self, x: f64, y: f64, radius: f64, only: Option<&[u32]>) -> Option<(u32, u32)> {
        let pickable: Vec<(u32, usize)> = self
            .layers
            .iter()
            .enumerate()
            .filter(|(_, (id, slot))| {
                only.is_none_or(|o| o.contains(id)) && matches!(slot, Slot::Layer(layer) if layer.pickable())
            })
            .map(|(i, (id, _))| (*id, i))
            .collect();
        // Their ordinal goes in the alpha channel
        if pickable.is_empty() || pickable.len() > 255 {
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
        // The block of pixels around the cursor, inside the canvas
        let r = (radius.max(0.0) * self.pixel_ratio).round() as i32;
        let (x0, y0) = ((px - r).max(0), (py - r).max(0));
        let (x1, y1) = ((px + r).min(size.0 - 1), (py + r).min(size.1 - 1));
        let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
        let target = self.pick_target(size).ok()?;
        let view = self.camera.view();
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        // SAFETY: offscreen pass on the live context, default framebuffer restored after
        unsafe {
            let gl = &self.gl;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
            gl.viewport(0, 0, size.0, size.1);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x0, y0, w, h);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            // Depth too: the blocks of extrusion layers hide one another
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            let frame = self.frame_state(&view);
            for (ordinal, &(_, index)) in pickable.iter().enumerate() {
                if let Slot::Layer(layer) = &self.layers[index].1 {
                    layer.draw_pick(&frame, (ordinal + 1) as u8);
                }
            }
            gl.read_pixels(
                x0,
                y0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
            gl.disable(glow::SCISSOR_TEST);
            gl.enable(glow::BLEND);
            gl.enable(glow::CULL_FACE);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        // The hit nearest to the cursor
        let mut best: Option<(i32, [u8; 4])> = None;
        for (k, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            let (i, j) = (k as i32 % w + x0, k as i32 / w + y0);
            let d = (i - px).pow(2) + (j - py).pow(2);
            if pixel[3] != 0 && d <= r * r && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, *pixel));
            }
        }
        let [r, g, b, layer] = best?.1;
        let id = u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16;
        if layer == 0 || id == 0 {
            return None;
        }
        let (layer_id, _) = pickable.get(usize::from(layer) - 1)?;
        Some((*layer_id, id - 1))
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
