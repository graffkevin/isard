//! 3D Tiles: the tileset tree and its traversal, the decoding of b3dm / glb content (meshopt, quantization; Draco
//! and KTX2 are decoded by the official modules in the workers and handed over). No WebGL, no I/O: tested natively

#![forbid(unsafe_code)]

pub mod decode;
pub mod meshopt;
pub mod tileset;
