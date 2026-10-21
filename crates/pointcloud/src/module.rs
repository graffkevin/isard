//! The point cloud module (`isard_points_bg.wasm`): what the decoding workers run on the chunks of a COPC file, loaded
//! with the first point cloud. Nothing of the engine: the LAZ decoder, the records to vertices.

use crate::frame::Grid;
use crate::points::{Layout, vertices};
use wasm_bindgen::prelude::*;

/// The vertices (`STRIDE` bytes each, see `points`) of a COPC chunk of `count` points. `laz`: the data of the LAZ
/// VLR; `layout`: format, record length, scale and offset of the header; `grid`: the positions of the node's square
/// (`Grid::numbers`)
#[wasm_bindgen(js_name = decodeCopc)]
pub fn decode_copc(chunk: &[u8], laz: &[u8], layout: &[f64], grid: &[f64], count: u32) -> Result<Vec<u8>, JsValue> {
    let error = |e: String| JsValue::from_str(&e);
    let layout = Layout::parse(layout).map_err(error)?;
    let grid = Grid::parse(grid).map_err(error)?;
    let records = crate::laz::decompress(chunk, laz, layout.record_length, count as usize).map_err(error)?;

    vertices(&records, &layout, &grid).map_err(error)
}
