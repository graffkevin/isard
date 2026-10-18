//! Point clouds: COPC (Cloud Optimized Point Cloud: a LAZ 1.4 file whose points are grouped in the nodes of an
//! octree, each node a LAZ chunk read by an HTTP range request), the traversal of its octree for a view, and the points
//! ready for the GPU. No WebGL, no I/O: tested natively.
//!
//! - `las`: the LAS 1.4 header and its variable length records (the COPC info, the LAZ items, the WKT of the CRS)
//! - `copc`: the COPC info, the hierarchy pages and the octree they describe (`Hierarchy`), loaded page by page
//! - `traversal`: the nodes to draw and to load for a view, by screen-space error within a point budget
//! - `frame`: the coordinates of the file (its CRS) to ECEF offsets, through a grid of positions computed exactly
//! - `points`: the records of a chunk to the vertices of the GPU (`STRIDE` bytes each)
//! - `laz` (feature `laz`): the LAZ decompression of a chunk, by the `laz` crate
//! - `module` (feature `module`): the entry point of the point cloud module, run by the decoding workers
//!
//! The engine holds all of it but the LAZ decoder, which is the point cloud module of its own (`isard_points_bg.wasm`),
//! loaded by the decoding workers with the first point cloud: the engines do not grow by it.
//!
//! Every size and offset read from the network is checked before it is used or allocated: a malformed file is an
//! error, never a panic.

#![forbid(unsafe_code)]

pub mod copc;
pub mod frame;
pub mod las;
#[cfg(any(feature = "laz", test))]
pub mod laz;
#[cfg(feature = "module")]
mod module;
pub mod points;
pub mod traversal;
