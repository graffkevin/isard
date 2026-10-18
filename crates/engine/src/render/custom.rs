//! Custom layers: the page's own WebGL drawing (raw WebGL, three.js…) in the engine's frame, at its place among the
//! layers, as `MapLibre`'s custom layers. The layer calls back into JavaScript while the frame is drawn; meanwhile
//! `Engine.customMatrix` turns a position into the model-view-projection matrix of a local east-north-up frame in metres
//! there, made in f64 relative to the camera and handed over in f32: exact at any zoom, on the plane and the globe.
//!
//! The page's code gets the canvas bound, blending on, no culling, and in `3d` mode the engine's depth buffer (tested
//! and written: the relief and the 3D layers hide it and are hidden by it); whatever state it changes is set back
//! after the callback (`gl::reset_after_page`). A map without custom layers runs none of this.

use crate::engine::Engine;
use crate::layer::{Frame, Layer};
use crate::render::gl;
use geo::camera::View;
use glow::HasContext;
use std::cell::Cell;
use wasm_bindgen::prelude::*;

thread_local! {
    /// The view of the frame while a custom layer's callback runs, None otherwise (`Engine::custom_matrix`)
    static VIEW: Cell<Option<View>> = const { Cell::new(None) };
}

/// A layer drawn by the page
pub struct CustomLayer {
    /// `() => void`, which never throws (the package catches the page's errors)
    render: js_sys::Function,
    /// Draws with the depth buffer of the 3D layers
    depth: bool,
}

impl Layer for CustomLayer {
    fn draw(&mut self, frame: &Frame) -> bool {
        let gl = frame.gl;
        VIEW.set(Some(frame.view.clone()));
        // SAFETY: state changes on a live context, set back after the callback
        unsafe {
            gl.disable(glow::CULL_FACE);
            gl.front_face(glow::CCW);
            if self.depth {
                gl.enable(glow::DEPTH_TEST);
            }
        }
        let _ = self.render.call0(&JsValue::NULL);
        VIEW.set(None);
        let size = |side: f64| (side * frame.pixel_ratio).round() as i32;
        gl::reset_after_page(gl, size(frame.camera.width), size(frame.camera.height));
        false
    }

    /// Nothing of its own on the GPU: the page frees its objects (`onRemove`)
    fn delete(&mut self, _gl: &glow::Context) {}
}

#[wasm_bindgen]
impl Engine {
    /// Model-view-projection matrix (16 numbers, column-major, for `uniformMatrix4fv`) of the local frame at a
    /// longitude, latitude (degrees) and altitude (metres): x east, y north, z up, in metres. Undefined outside the
    /// callback of a custom layer. Static: the engine is borrowed while it draws
    #[wasm_bindgen(js_name = customMatrix)]
    #[must_use]
    pub fn custom_matrix(lon: f64, lat: f64, altitude: f64) -> Option<Vec<f32>> {
        // Taken out and put back: no borrow to track
        let view = VIEW.take();
        let m = view
            .as_ref()
            .map(|v| (v.view_proj * v.enu_matrix(glam::DVec3::new(lon, lat, altitude))).to_cols_array());
        VIEW.set(view);
        Some(m?.map(|v| v as f32).to_vec())
    }

    /// A layer drawn by the page: `render()` is called at its place in every frame (`customMatrix` gives the
    /// matrices of that frame meanwhile); `depth`: with the depth buffer of the 3D layers (`3d` mode), else over
    /// what is drawn before it
    #[wasm_bindgen(js_name = addCustomLayer)]
    pub fn add_custom_layer(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "() => void")] render: js_sys::Function,
        depth: bool,
    ) -> u32 {
        self.add_layer(Box::new(CustomLayer { render, depth }))
    }
}
