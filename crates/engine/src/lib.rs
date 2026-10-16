//! isard: a WebGL2 map engine built for large, interactive data layers (accessibility, flooding, 3D meshes) on
//! Géoplateforme basemaps. Rendering only happens on demand: an idle map costs nothing.
//!
//! This crate is the WebAssembly core. The `isard` JavaScript package wraps it with the interactions (pan, zoom,
//! rotate, inertia), the resize handling, the decoding workers and a typed API. What needs neither WebGL nor the
//! browser lives in crates of its own, tested natively: `geo` (camera, projections, geodesy, tile selection) and
//! `tiles3d` (3D Tiles tree, traversal and decoding).
//!
//! - `engine`: the `Engine` exported to JavaScript: camera, ground (raster layers over the relief), frame loop,
//!   picking
//! - `layer`: the `Layer` trait of what is drawn over the ground, and the frame state it gets
//! - `render`: the WebGL2 layers (raster stack, network, 3D Tiles, globe base, sky) and their helpers; each layer
//!   over the ground exports its own methods of `Engine`
//! - `worker`: the entry points of the 3D Tiles decoding workers
//!
//! The crate also compiles natively, for the unit tests of its pure logic: what needs the browser (the WebGL2
//! context, image uploads) is behind `cfg(target_arch = "wasm32")` in `render::gl`.

mod engine;
mod js;
mod layer;
mod render;
mod worker;

pub use engine::Engine;
