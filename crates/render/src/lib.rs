//! geo-render: a WebGL2 map engine built for large, interactive data layers (accessibility, flooding) on
//! Géoplateforme basemaps. Rendering only happens on demand: an idle map costs nothing.
//!
//! This crate is the WebAssembly core. The `geo-render` JavaScript package wraps it with the interactions (pan, zoom,
//! rotate, inertia), the resize handling and a typed API.

pub mod camera;
pub mod tiles;

#[cfg(target_arch = "wasm32")]
mod engine;
#[cfg(target_arch = "wasm32")]
mod gl;
#[cfg(target_arch = "wasm32")]
mod globe;
#[cfg(target_arch = "wasm32")]
mod network;
#[cfg(target_arch = "wasm32")]
mod raster;

#[cfg(target_arch = "wasm32")]
pub use engine::Engine;
