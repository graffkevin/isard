//! geo-render: a WebGL2 map engine built for large, interactive data layers (accessibility, flooding, 3D meshes) on
//! Géoplateforme basemaps. Rendering only happens on demand: an idle map costs nothing.
//!
//! This crate is the WebAssembly core. The `geo-render` JavaScript package wraps it with the interactions (pan, zoom,
//! rotate, inertia), the resize handling, the decoding workers and a typed API.
//!
//! - `geo`: camera and projections (Web Mercator, globe), geodesy, tile selection — pure Rust, tested natively
//! - `tiles3d`: 3D Tiles tree, traversal and decoding (pure, tested), its WebGL layer and the worker entry point
//! - `render`: the WebGL2 layers (raster stack, network, globe base) and their helpers
//! - `engine`: the `Engine` exported to JavaScript

pub mod geo;
pub mod tiles3d;

#[cfg(target_arch = "wasm32")]
mod engine;
#[cfg(target_arch = "wasm32")]
mod js;
#[cfg(target_arch = "wasm32")]
mod render;

#[cfg(target_arch = "wasm32")]
pub use engine::Engine;
