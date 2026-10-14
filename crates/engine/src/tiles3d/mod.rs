//! 3D Tiles: the tileset tree and its traversal, the decoding of b3dm / glb content (pure Rust, tested natively), the
//! WebGL layer and the entry point of the decoding workers

pub mod decode;
pub mod meshopt;
pub mod tileset;

#[cfg(target_arch = "wasm32")]
pub(crate) mod layer;
#[cfg(target_arch = "wasm32")]
mod worker;
