//! Where things are: camera and projections, WGS 84 geodesy and the French geoid, Web Mercator tile selection. No
//! WebGL, no I/O: tested natively

#![forbid(unsafe_code)]

pub mod camera;
pub mod collision;
pub mod geodesy;
pub mod geoid;
pub mod page;
pub mod pieces;
pub mod shapes;
pub mod tiles;
