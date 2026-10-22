//! Polygons as triangles, for the fill layers: each polygon is triangulated (earcut, holes included), then cut along
//! a grid of fixed Mercator squares so that no triangle spans more than a square: on the globe the pieces follow the
//! sphere, and neighbours cut along the same grid lines share their vertices (no cracks).
//!
//! Over the relief a fill is drawn as a shadow volume: the polygon lifted into a prism from below the lowest ground to
//! above the highest, its faces counted in the stencil where they lie behind the ground (z-fail), the ground inside
//! the prism then colored. So `Mesh` holds the prism: the triangles twice (the top and the bottom caps) and the
//! walls along the rings, every face turned outwards, which the counting needs (an overlap of two polygons still
//! counts as inside). On flat ground only the top cap is drawn, and only it is built (`triangulate_with`).
//!
//! The vertices are shared: the caps index the points of the rings (earcut's indices), a triangle cut by the grid adds
//! the points of its pieces, the bottom cap is the top one's vertices at level 0, the walls stand on those of the
//! rings and add their crossings of the grid lines. A polygon of n points costs about 2n vertices in relief.
//!
//! Positions are split into a high and a low f32 part (`hi + lo` = the f64 Mercator coordinate): a polygon as large as
//! the world (a veil with a hole) keeps its edges to the centimetre at any zoom, the GPU subtracting the camera center
//! part by part.
//!
//! Submodules: `triangulation` (the fill meshes: earcut, the cut along the grid, the prism, the chunks by square) and
//! `extrusion` (extruded blocks: roofs, shaded walls, their chunks by square).

mod extrusion;
#[cfg(test)]
mod tests;
mod triangulation;

pub use extrusion::{Block, BlockChunks, Blocks, chunk_blocks, extrude};
pub use triangulation::{Chunks, Mesh, Vertex, chunk, square_for, triangulate, triangulate_in, triangulate_with};

use glam::DVec2;

/// Side of the grid squares, Mercator (about 300 km at the equator): on the globe a chord that long sags less than
/// 2 km under the sphere
pub const SQUARE: f64 = 1.0 / 128.0;

/// Polygons in Mercator: polygon `p` has rings `polygon_start[p] .. polygon_start[p + 1]` (the first is the outer one,
/// the others holes), ring `r` the points `points[ring_start[r] .. ring_start[r + 1]]` (closing point optional)
pub struct Polygons<'a> {
    /// Points of every ring, one after the other, Mercator 0-1
    pub points: &'a [DVec2],
    /// Where each ring starts in `points`, with one more entry for the end of the last
    pub ring_start: &'a [u32],
    /// Where each polygon starts in the rings, with one more entry for the end of the last
    pub polygon_start: &'a [u32],
}

/// Polygons kept by a layer (a copy of `Polygons`), to build their mesh again and test points against them
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OwnedPolygons {
    /// As `Polygons::points`
    pub points: Vec<DVec2>,
    /// As `Polygons::ring_start`
    pub ring_start: Vec<u32>,
    /// As `Polygons::polygon_start`
    pub polygon_start: Vec<u32>,
}

impl OwnedPolygons {
    /// A copy of borrowed polygons, offsets kept as they are (checked when read)
    #[must_use]
    pub fn new(polygons: &Polygons) -> Self {
        Self {
            points: polygons.points.to_vec(),
            ring_start: polygons.ring_start.to_vec(),
            polygon_start: polygons.polygon_start.to_vec(),
        }
    }

    /// The polygons borrowed, to triangulate or extrude them
    #[must_use]
    pub fn as_polygons(&self) -> Polygons<'_> {
        Polygons {
            points: &self.points,
            ring_start: &self.ring_start,
            polygon_start: &self.polygon_start,
        }
    }
}

impl Polygons<'_> {
    /// The rings `first..last` (`polygon_rings`) as `triangulate` takes them: without their closing point, empty if
    /// their bounds are off or a point is not a number
    pub fn rings(&self, rings: std::ops::Range<usize>) -> impl Iterator<Item = &[DVec2]> {
        rings.map(|r| ring(self, r))
    }
}

/// The rings of each polygon (`first..last`), clipped to the rings there are and so that two polygons never share a
/// ring: a polygon starting before the end of the previous one starts at that end. Malformed offsets (every polygon
/// claiming every ring) cost no more than the rings there are, never rings × polygons
pub fn polygon_rings(polygons: &Polygons) -> impl Iterator<Item = std::ops::Range<usize>> {
    let rings = polygons.ring_start.len().saturating_sub(1);
    let starts = polygons.polygon_start;
    let mut end = 0;

    (0..starts.len().saturating_sub(1)).map(move |p| {
        let first = (starts[p] as usize).min(rings).max(end);
        let last = (starts[p + 1] as usize).min(rings).max(first);

        end = last;

        first..last
    })
}

/// Points of a ring without its closing point; empty if its bounds are off or a point is not finite
fn ring<'a>(polygons: &Polygons<'a>, r: usize) -> &'a [DVec2] {
    let (Some(&s), Some(&e)) = (polygons.ring_start.get(r), polygons.ring_start.get(r + 1)) else {
        return &[];
    };
    let e = (e as usize).min(polygons.points.len());
    let points = &polygons.points[(s as usize).min(e)..e];

    if !points.iter().all(|p| p.is_finite()) {
        return &[];
    }

    match points {
        [first, .., last] if first == last => &points[..points.len() - 1],
        _ => points,
    }
}

/// Twice the signed area (positive: counterclockwise with x and y as drawn on paper, y up)
fn area(points: &[DVec2]) -> f64 {
    let n = points.len();

    (0..n).map(|i| points[i].perp_dot(points[(i + 1) % n])).sum()
}
