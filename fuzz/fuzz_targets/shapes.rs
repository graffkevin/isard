//! Polygons as the wasm API receives them (`addFills`, `addExtrusions`): points, ring and polygon offsets unchecked,
//! through `shapes::triangulate` and `shapes::extrude`. Points are quantized to a grid (degenerate, collinear and
//! duplicate points come often) inside a small Mercator area, so the grid cut stays a few squares.
#![no_main]

use arbitrary::Arbitrary;
use geo::shapes::{Polygons, extrude, triangulate};
use glam::DVec2;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Input {
    /// Points on a 65536² grid over a square of `side` (Mercator)
    points: Vec<(u16, u16)>,
    ring_start: Vec<u16>,
    polygon_start: Vec<u16>,
    /// 0: a 1/32 square (16 grid squares), else down to a few metres
    scale: u8,
}

fuzz_target!(|input: Input| {
    let side = (1.0 / 32.0) / f64::from(1u32 << (input.scale % 16));
    let origin = DVec2::new(0.5, 0.33);
    let points: Vec<DVec2> = input
        .points
        .iter()
        .map(|&(x, y)| origin + DVec2::new(f64::from(x), f64::from(y)) * (side / 65535.0))
        .collect();
    let ring_start: Vec<u32> = input.ring_start.iter().map(|&i| u32::from(i)).collect();
    let polygon_start: Vec<u32> = input.polygon_start.iter().map(|&i| u32::from(i)).collect();
    let polygons = Polygons {
        points: &points,
        ring_start: &ring_start,
        polygon_start: &polygon_start,
    };
    let mesh = triangulate(&polygons);
    let n = mesh.vertices.len() as u32;
    assert!(mesh.indices.iter().all(|&i| i < n), "index out of the vertices");
    let blocks = extrude(&polygons, origin);
    let n = blocks.vertices.len() as u32;
    assert!(blocks.indices.iter().all(|&i| i < n), "index out of the blocks");
});
