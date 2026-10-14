//! geo-render: a WebGL2 map engine built for large, interactive data layers (accessibility, flooding) on
//! Géoplateforme basemaps. Rendering only happens on demand: an idle map costs nothing.
//!
//! This crate is the WebAssembly core. The `geo-render` JavaScript package wraps it with the interactions (pan, zoom,
//! rotate, inertia), the resize handling and a typed API.

pub mod b3dm;
pub mod camera;
pub mod geodesy;
pub mod tiles;
pub mod tileset;

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
mod tiles3d;

#[cfg(target_arch = "wasm32")]
pub use engine::Engine;
