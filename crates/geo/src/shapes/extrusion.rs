//! Extruded polygons (buildings): a roof per polygon and a shaded wall along every edge, then the blocks sorted by
//! square of their centroids (`chunk_blocks`).

use super::{Polygons, area, polygon_rings, ring};
use crate::pieces::{Grid, sort_by_square};
use glam::DVec2;

/// A vertex of an extruded polygon (32 bytes): its position and its polygon's centroid relative to the layer origin,
/// its feature (its polygon's number here, which the layer replaces by the polygon's feature), its level (0: foot of
/// a wall, 1: roof) and its shade (the light on its face)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Block {
    /// Position relative to the layer origin, Mercator
    pub offset: [f32; 2],
    /// Its polygon's centroid relative to the layer origin, Mercator (where the height is read)
    pub centroid: [f32; 2],
    /// Number of the polygon in the input, until the layer writes its feature there
    pub feature: u32,
    /// 0: foot of a wall, 1: roof
    pub level: f32,
    /// Light on its face, 1 on the roofs, 0.72 to 0.92 on the walls
    pub shade: f32,
    /// Padding to 32 bytes
    pub pad: f32,
}

/// Extruded polygons: roofs then walls, as indexed triangles, polygon after polygon
#[derive(Default)]
pub struct Blocks {
    /// Every vertex, roofs sharing theirs, walls with four each
    pub vertices: Vec<Block>,
    /// Triangles, three indices each
    pub indices: Vec<u32>,
    /// The indices of each polygon extruded (`first..end`), its centroid (Mercator) and how far its points lie from
    /// it at most (until `chunk_blocks` sorts the indices)
    pub parts: Vec<(u32, u32, DVec2, f64)>,
}

/// Light on the walls: from the north-west and above (x east, y south)
const LIGHT: DVec2 = DVec2::new(-std::f64::consts::FRAC_1_SQRT_2, -std::f64::consts::FRAC_1_SQRT_2);

/// Extrudes polygons (buildings): a roof per polygon (earcut, holes included) and a wall along every edge of its
/// rings, shaded by the side it faces. Positions relative to `origin`.
#[must_use]
pub fn extrude(polygons: &Polygons, origin: DVec2) -> Blocks {
    let mut out = Blocks::default();
    let mut earcut = earcut::Earcut::new();
    let (mut flat, mut holes, mut triangles): (Vec<[f64; 2]>, Vec<u32>, Vec<u32>) =
        (Vec::new(), Vec::new(), Vec::new());
    let f32s = |p: DVec2| [p.x as f32, p.y as f32];

    for (p, rings) in polygon_rings(polygons).enumerate() {
        let rings: Vec<&[DVec2]> = rings.map(|r| ring(polygons, r)).filter(|r| r.len() >= 3).collect();
        let Some(outer) = rings.first() else {
            continue;
        };
        let first_index = out.indices.len() as u32;
        let centroid = outer.iter().sum::<DVec2>() / outer.len() as f64;
        let radius = outer.iter().map(|q| q.distance(centroid)).fold(0.0, f64::max);
        let c = f32s(centroid - origin);
        let vertex = |m: DVec2, level: f32, shade: f32| Block {
            offset: f32s(m - origin),
            centroid: c,
            feature: p as u32,
            level,
            shade,
            pad: 0.0,
        };

        // Roof
        flat.clear();
        holes.clear();

        let mut all: Vec<DVec2> = Vec::new();

        for (k, r) in rings.iter().enumerate() {
            if k > 0 {
                holes.push(all.len() as u32);
            }
            all.extend_from_slice(r);
        }

        let base = all[0];

        flat.extend(all.iter().map(|&q| (q - base).to_array()));
        earcut.earcut(flat.iter().copied(), &holes, &mut triangles);

        let start = out.vertices.len() as u32;

        out.vertices.extend(all.iter().map(|&m| vertex(m, 1.0, 1.0)));
        out.indices.extend(triangles.iter().map(|&i| start + i));
        push_walls(&mut out, &rings, vertex);

        // Its holes reach no further than its outer ring
        let radius = if radius.is_finite() { radius } else { 0.0 };

        out.parts
            .push((first_index, out.indices.len() as u32, centroid, radius));
    }

    out
}

/// The walls of one extruded polygon: a quad along every edge of its rings, each with its own four vertices (flat
/// shading), shaded by the side it faces. `vertex` makes a vertex of the polygon (position, level, shade)
fn push_walls(out: &mut Blocks, rings: &[&[DVec2]], vertex: impl Fn(DVec2, f32, f32) -> Block) {
    for (k, r) in rings.iter().enumerate() {
        // Outward normal: the inside is on the left of a counterclockwise outer ring and of a clockwise hole
        let sign = if (area(r) > 0.0) == (k == 0) { 1.0 } else { -1.0 };

        for i in 0..r.len() {
            let (a, b) = (r[i], r[(i + 1) % r.len()]);
            let d = b - a;

            if d.length_squared() == 0.0 {
                continue;
            }

            let normal = DVec2::new(d.y, -d.x).normalize() * sign;
            let shade = (0.72 + 0.2 * normal.dot(LIGHT).max(0.0)) as f32;
            let s = out.vertices.len() as u32;

            out.vertices.extend([
                vertex(a, 0.0, shade),
                vertex(b, 0.0, shade),
                vertex(b, 1.0, shade),
                vertex(a, 1.0, shade),
            ]);
            out.indices.extend([s, s + 1, s + 2, s, s + 2, s + 3]);
        }
    }
}

/// Squares of the culling grid of extruded blocks along the longer side of their extent
const BLOCK_SQUARES_ACROSS: f64 = 32.0;
/// Smallest square of that grid, Mercator (about 100 m at the equator)
const MIN_BLOCK_SQUARE: f64 = 100.0 / 40_075_016.686;

/// The blocks sorted by square of a grid over their centroids, as `chunk` does for fills: only the squares in view
/// are drawn
pub struct BlockChunks {
    /// The squares the blocks are sorted by
    pub grid: Grid,
    /// Indices of square `s`: `indices[start[s] .. start[s + 1]]`
    pub start: Vec<u32>,
    /// How far a block reaches out of its centroid at most (Mercator): the margin of the squares in view
    pub reach: f64,
}

/// Sorts the indices of extruded blocks by square of their polygon's centroid, each polygon's triangles kept together
/// and the polygons in their order within a square
#[must_use]
pub fn chunk_blocks(blocks: &mut Blocks) -> BlockChunks {
    let (min, max) = blocks
        .parts
        .iter()
        .map(|p| p.2)
        .filter(|c| c.is_finite())
        .fold((DVec2::INFINITY, DVec2::NEG_INFINITY), |(a, b), c| (a.min(c), b.max(c)));
    let extent = (max - min).max_element();
    let grid = Grid::over(min, max, (extent / BLOCK_SQUARES_ACROSS).max(MIN_BLOCK_SQUARE));
    let square_of: Vec<u32> = blocks.parts.iter().map(|p| grid.square(p.2)).collect();
    let (order, part_start) = sort_by_square(&square_of, grid.len());
    // The parts in their new order, and where each one starts
    let mut indices = Vec::with_capacity(blocks.indices.len());
    let mut at = Vec::with_capacity(order.len() + 1);

    for &k in &order {
        at.push(indices.len() as u32);

        let (first, end, _, _) = blocks.parts[k as usize];

        indices.extend_from_slice(&blocks.indices[first as usize..end as usize]);
    }
    at.push(indices.len() as u32);

    let start = part_start.iter().map(|&p| at[p as usize]).collect();

    blocks.indices = indices;

    let reach = blocks.parts.iter().map(|p| p.3).fold(0.0, f64::max);

    // Their ranges were those of the indices before
    blocks.parts.clear();

    BlockChunks { grid, start, reach }
}
