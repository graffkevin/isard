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

use crate::pieces::{Grid, sort_by_square};
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

/// A vertex as uploaded (24 bytes): its position in two parts, its feature (its polygon's number here, which the
/// layer replaces by the polygon's feature), its level (0: bottom of the prism, 1: top)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// Mercator position rounded to f32
    pub hi: [f32; 2],
    /// What the rounding left out: `hi + lo` is the f64 position
    pub lo: [f32; 2],
    /// Number of the polygon in the input, until the layer writes its feature there
    pub feature: u32,
    /// 0: bottom of the prism, 1: top (the flat fill draws only level 1)
    pub level: f32,
}

impl Vertex {
    fn new(m: DVec2, feature: u32, level: f32) -> Self {
        let hi = [m.x as f32, m.y as f32];
        let lo = [(m.x - f64::from(hi[0])) as f32, (m.y - f64::from(hi[1])) as f32];

        Self { hi, lo, feature, level }
    }

    /// The f64 position back
    #[must_use]
    pub fn position(&self) -> DVec2 {
        DVec2::new(
            f64::from(self.hi[0]) + f64::from(self.lo[0]),
            f64::from(self.hi[1]) + f64::from(self.lo[1]),
        )
    }
}

/// The prism of every polygon: `indices[..top]` is the top cap (also the flat fill), `indices[top..walls]` the
/// bottom cap, `indices[walls..]` the walls. The top cap uses `vertices[..top_vertices]` only: on flat ground, that
/// part is all there is to upload
#[derive(Default)]
pub struct Mesh {
    /// The vertices of the top cap first (`..top_vertices`), then those of the bottom cap and of the walls
    pub vertices: Vec<Vertex>,
    /// Triangles, three indices each, counterclockwise seen from outside the prism
    pub indices: Vec<u32>,
    /// End of the top cap in `indices`
    pub top: usize,
    /// End of the bottom cap in `indices`, start of the walls (`top` without the prism)
    pub walls: usize,
    /// Vertices the top cap uses
    pub top_vertices: usize,
    /// Mercator bounds of the points (zero when there are none)
    pub min: DVec2,
    /// Upper Mercator bound, as `min`
    pub max: DVec2,
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

/// Most points of a triangle clipped to a square: one more per side at most
const CLIPPED: usize = 8;

/// Clips a triangle to the square `[lo, hi]` (Sutherland-Hodgman, one side at a time): the points of the convex piece
/// left, and their count. On the stack: millions of triangles may be cut
fn clip(triangle: &[DVec2; 3], lo: DVec2, hi: DVec2) -> ([DVec2; CLIPPED], usize) {
    let mut out = [DVec2::ZERO; CLIPPED];

    out[..3].copy_from_slice(triangle);

    let mut n = 3;

    // Each side: the coordinate (0: x, 1: y), the bound and whether points below it are kept
    for (axis, bound, below) in [(0, lo.x, false), (0, hi.x, true), (1, lo.y, false), (1, hi.y, true)] {
        let (input, count) = (out, n);

        n = 0;

        let inside = |p: DVec2| if below { p[axis] <= bound } else { p[axis] >= bound };

        for i in 0..count {
            let (p, q) = (input[i], input[(i + 1) % count]);

            if inside(p) {
                out[n] = p;
                n += 1;
            }

            if inside(p) != inside(q) {
                let t = (bound - p[axis]) / (q[axis] - p[axis]);
                let mut x = p.lerp(q, t);

                // Exactly on the grid line: the neighbour computes the same point
                x[axis] = bound;
                out[n] = x;
                n += 1;
            }
        }

        if n == 0 {
            break;
        }
    }

    (out, n)
}

/// Most grid squares one triangle may cover (16 worlds): beyond, coordinates are garbage and the triangle is dropped
const MAX_SQUARES: i64 = 1 << 18;
/// Most grid lines one wall may cross (beyond, it is left whole)
const MAX_CROSSINGS: f64 = 4096.0;

/// Grid squares (`[lo, hi]` corners) a box touches; none for a non-finite or absurdly large box
fn squares(min: DVec2, max: DVec2, square: f64) -> impl Iterator<Item = (DVec2, DVec2)> {
    let (i0, i1) = ((min.x / square).floor() as i64, (max.x / square).ceil() as i64);
    let (j0, j1) = ((min.y / square).floor() as i64, (max.y / square).ceil() as i64);
    let (w, h) = (i1.saturating_sub(i0).max(1), j1.saturating_sub(j0).max(1));
    let sane = min.is_finite() && max.is_finite() && w.saturating_mul(h) <= MAX_SQUARES;
    let (j1, i1) = if sane {
        (j1.max(j0 + 1), i1.max(i0 + 1))
    } else {
        (j0, i0)
    };

    (j0..j1).flat_map(move |j| {
        (i0..i1).map(move |i| {
            let lo = DVec2::new(i as f64, j as f64) * square;

            (lo, lo + square)
        })
    })
}

/// Fractions along `a` → `b` where it crosses the grid lines, with 0 and 1, in order
fn crossings(a: DVec2, b: DVec2, square: f64) -> Vec<f64> {
    let mut t = vec![0.0, 1.0];

    for axis in 0..2 {
        let (p, q) = (a[axis], b[axis]);

        if (p - q).abs() < f64::EPSILON {
            continue;
        }

        let (lo, hi) = (p.min(q), p.max(q));

        if !(lo.is_finite() && hi.is_finite()) || (hi - lo) / square > MAX_CROSSINGS {
            continue;
        }

        let mut k = (lo / square).floor() + 1.0;

        while k * square < hi {
            t.push((k * square - p) / (q - p));
            k += 1.0;
        }
    }
    t.sort_by(f64::total_cmp);
    t.dedup();

    t
}

/// Triangulates the polygons (see the module), cut along the grid of `SQUARE`
#[must_use]
pub fn triangulate(polygons: &Polygons) -> Mesh {
    triangulate_in(polygons, SQUARE)
}

/// Squares across the extent of a fill layer when it is cut finer (`square_for`)
const CHUNKS_ACROSS: f64 = 16.0;
/// Finest cut: `SQUARE` halved at most this many times (about 75 m at the equator)
const MAX_HALVINGS: i32 = 12;

/// Side of the squares to cut polygons within `min..max` along: `SQUARE` halved until at least `CHUNKS_ACROSS` squares
/// span their extent, so that the squares in view bound what is drawn. Its grid lines are lines of the grid of
/// `SQUARE` too: on the globe, the pieces still follow the sphere
#[must_use]
pub fn square_for(min: DVec2, max: DVec2) -> f64 {
    let extent = (max - min).max_element();

    if !extent.is_finite() || extent <= 0.0 {
        return SQUARE;
    }

    let halvings = (SQUARE * CHUNKS_ACROSS / extent)
        .log2()
        .ceil()
        .clamp(0.0, f64::from(MAX_HALVINGS));

    SQUARE / f64::from(1u32 << halvings as u32)
}

/// Triangulates the polygons (see the module), cut along the grid of `square` (`SQUARE` or a fraction of it by a
/// power of two)
#[must_use]
pub fn triangulate_in(polygons: &Polygons, square: f64) -> Mesh {
    triangulate_with(polygons, square, true)
}

/// Points of a polygon's pieces cut by the grid looked for among this many vertices added last (`triangulate_with`)
const RECENT: usize = 64;

/// A wall of the prism: its ends (outwards: the inside on the left of a → b) and their top vertices, its polygon
struct Wall {
    a: DVec2,
    b: DVec2,
    ends: (u32, u32),
    polygon: u32,
}

/// Triangulates the polygons as `triangulate_in`; `prism` false: the top cap only (`indices.len() == top`), what a
/// flat fill draws. The top cap is the same either way.
///
/// Four steps, kept in one function as they share the vertex numbering: (1) each polygon's rings become the top
/// vertices, earcut's triangles and, for the prism, the walls to build; (2) those triangles are clipped to the grid
/// squares (the top cap); (3) the bottom cap copies the top one at level 0, turned the other way; (4) the walls are
/// cut at the grid lines and stood on the rings' vertices.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one algorithm over shared vertex numbering, its steps commented"
)]
pub fn triangulate_with(polygons: &Polygons, square: f64, prism: bool) -> Mesh {
    let square = if square.is_finite() && square > 0.0 {
        square.min(SQUARE)
    } else {
        SQUARE
    };
    let mut mesh = Mesh {
        min: DVec2::INFINITY,
        max: DVec2::NEG_INFINITY,
        ..Mesh::default()
    };
    // Earcut's triangles, as indices into the vertices (the points of the rings, level 1), and those points in f64
    let mut caps: Vec<[u32; 3]> = Vec::new();
    let mut exact: Vec<DVec2> = Vec::new();
    let mut walls: Vec<Wall> = Vec::new();
    let mut earcut = earcut::Earcut::new();
    let (mut flat, mut holes, mut triangles, mut all): (Vec<[f64; 2]>, Vec<u32>, Vec<u32>, Vec<DVec2>) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());

    // 1. Each polygon: its rings' points as top vertices (in `all`, then `exact`), its walls, earcut's triangles
    for (p, rings) in polygon_rings(polygons).enumerate() {
        let first = rings.start;

        flat.clear();
        holes.clear();
        all.clear();

        let base = mesh.vertices.len() as u32;

        for r in rings {
            let points = ring(polygons, r);

            if points.len() < 3 {
                continue;
            }
            if r > first {
                holes.push(all.len() as u32);
            }

            let start = base + all.len() as u32;

            all.extend_from_slice(points);

            let n = points.len();

            if prism {
                // Walls turned outwards: away from the polygon's inside (outside of a hole)
                let outer = r == first;
                let counterclockwise = area(points) > 0.0;

                for i in 0..n {
                    let (a, b) = (points[i], points[(i + 1) % n]);
                    let ends = (start + i as u32, start + ((i + 1) % n) as u32);

                    // With the inside on the left of a → b (counterclockwise outer ring, clockwise hole), the outward
                    // normal is (dy, -dx): the order the wall triangles below assume
                    walls.push(if outer == counterclockwise {
                        Wall {
                            a,
                            b,
                            ends,
                            polygon: p as u32,
                        }
                    } else {
                        Wall {
                            a: b,
                            b: a,
                            ends: (ends.1, ends.0),
                            polygon: p as u32,
                        }
                    });
                }
            }

            for &a in points {
                mesh.min = mesh.min.min(a);
                mesh.max = mesh.max.max(a);
            }
        }
        if all.len() < 3 {
            continue;
        }
        mesh.vertices.extend(all.iter().map(|&q| Vertex::new(q, p as u32, 1.0)));
        exact.extend_from_slice(&all);

        // Relative to the first point: earcut works in f64 but its predicates like small numbers
        let origin = all[0];

        flat.extend(all.iter().map(|&q| (q - origin).to_array()));
        earcut.earcut(flat.iter().copied(), &holes, &mut triangles);

        for t in triangles.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| all[i as usize]);
            // Counterclockwise: the top cap faces up
            let [i, j, k] = t.map(|i| base + i);

            caps.push(if (b - a).perp_dot(c - a) >= 0.0 {
                [i, j, k]
            } else {
                [i, k, j]
            });
        }
    }

    // 2. The top cap, cut along the grid.
    // The vertices added by the pieces of the current polygon start at `added`; the points of a piece are looked for
    // among the last `RECENT` of them only (a polygon cut into thousands of pieces stays linear)
    let mut added = exact.len();
    let mut polygon = u32::MAX;

    for t in caps {
        let tri = t.map(|i| exact[i as usize]);
        let p = mesh.vertices[t[0] as usize].feature;

        if p != polygon {
            (polygon, added) = (p, exact.len());
        }

        let recent = exact.len().saturating_sub(RECENT);
        let min = tri[0].min(tri[1]).min(tri[2]);
        let max = tri[0].max(tri[1]).max(tri[2]);
        // Inside one square (most triangles of small polygons): as it is, without clipping
        let mut cells = squares(min, max, square);

        if cells.next().is_some() && cells.next().is_none() {
            if area(&tri).abs() >= 1e-30 {
                mesh.indices.extend(t);
            }
            continue;
        }

        for (lo, hi) in squares(min, max, square) {
            let (points, n) = clip(&tri, lo, hi);
            let piece = &points[..n];

            if piece.len() < 3 || area(piece).abs() < 1e-30 {
                continue;
            }

            // A convex piece: a fan of its points, still counterclockwise. A corner of the triangle is its vertex, a
            // point on a grid line the one of the piece next to it if it was just added
            let mut fan = [0u32; CLIPPED];

            for (k, &q) in piece.iter().enumerate() {
                fan[k] = if let Some(c) = tri.iter().position(|&corner| corner == q) {
                    t[c]
                } else if let Some(i) = (recent.max(added)..exact.len()).rev().find(|&i| exact[i] == q) {
                    i as u32
                } else {
                    mesh.vertices.push(Vertex::new(q, p, 1.0));
                    exact.push(q);

                    exact.len() as u32 - 1
                };
            }

            for k in 1..piece.len() - 1 {
                mesh.indices.extend([fan[0], fan[k], fan[k + 1]]);
            }
        }
    }
    mesh.top = mesh.indices.len();
    mesh.top_vertices = mesh.vertices.len();
    mesh.walls = mesh.top;
    if prism {
        // 3. The bottom cap faces down: the same triangles the other way round, on the same vertices at level 0
        let lift = mesh.top_vertices as u32;

        mesh.vertices.extend_from_within(..mesh.top_vertices);
        for v in &mut mesh.vertices[mesh.top_vertices..] {
            v.level = 0.0;
        }
        mesh.indices.extend_from_within(..mesh.top);
        for t in mesh.indices[mesh.top..].as_chunks_mut::<3>().0 {
            *t = [t[0] + lift, t[2] + lift, t[1] + lift];
        }
        mesh.walls = mesh.indices.len();

        // 4. The walls: two triangles per piece between grid lines, from the top vertices to their copies below
        // (`bottom`: the copy at `+ lift` for a ring vertex, the next vertex for one added on a grid line)
        for Wall { a, b, ends, polygon } in walls {
            // Split where they cross the grid lines, as the caps are: the walls meet the caps vertex to vertex. Each
            // point of the wall: its top vertex (the bottom one follows it)
            let cuts = crossings(a, b, square);
            let mut previous = ends.0;

            for (k, w) in cuts.windows(2).enumerate() {
                let next = if k + 2 == cuts.len() {
                    ends.1
                } else {
                    let v = mesh.vertices.len() as u32;
                    let m = a.lerp(b, w[1]);

                    mesh.vertices
                        .extend([Vertex::new(m, polygon, 1.0), Vertex::new(m, polygon, 0.0)]);

                    v
                };
                let bottom = |top: u32| if top < lift { top + lift } else { top + 1 };
                let (u0, v0, v1, u1) = (bottom(previous), bottom(next), next, previous);

                mesh.indices.extend([u0, v0, v1, u0, v1, u1]);
                previous = next;
            }
        }
    }
    if mesh.vertices.is_empty() {
        mesh.min = DVec2::ZERO;
        mesh.max = DVec2::ZERO;
    }

    mesh
}

/// The triangles of a fill mesh sorted by square of a grid, the top cap first: only the squares in view are drawn
pub struct Chunks {
    /// The squares the triangles are sorted by
    pub grid: Grid,
    /// Indices of square `s` in the top cap: `indices[top[s] .. top[s + 1]]`
    pub top: Vec<u32>,
    /// The same for the bottom cap and the walls, after the top cap
    pub rest: Vec<u32>,
}

/// Sorts the triangles of a mesh cut along `square` (`triangulate_in`) by square, the top cap and the rest each on
/// their own, keeping their order within a square: triangles of different squares never overlap, so a fill draws the
/// same. Every triangle lies in its square (a wall on its border), so the squares in view bound what is drawn
#[must_use]
pub fn chunk(mesh: &mut Mesh, square: f64) -> Chunks {
    let min = (mesh.min / square).floor() * square;
    let grid = if min.is_finite() && mesh.max.is_finite() {
        Grid::over(min, mesh.max, square)
    } else {
        Grid::over(DVec2::ZERO, DVec2::ZERO, 1.0)
    };
    let squares = grid.len();
    let (top, end) = (mesh.top, mesh.indices.len() / 3 * 3);
    let mut sort = |range: std::ops::Range<usize>| -> Vec<u32> {
        let triangles = &mesh.indices[range.clone()];
        let square_of: Vec<u32> = triangles
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let c = t.iter().map(|&i| mesh.vertices[i as usize].position()).sum::<DVec2>() / 3.0;

                grid.square(c)
            })
            .collect();
        let (order, start) = sort_by_square(&square_of, squares);
        let mut sorted = Vec::with_capacity(triangles.len());

        for &t in &order {
            let t = t as usize * 3;

            sorted.extend_from_slice(&triangles[t..t + 3]);
        }
        mesh.indices[range.clone()].copy_from_slice(&sorted);

        start.into_iter().map(|s| s * 3 + range.start as u32).collect()
    };
    let top_start = sort(0..top);
    let rest = sort(top..end);

    Chunks {
        grid,
        top: top_start,
        rest,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    /// Points of the prism in 3D (z = level), and its triangles
    fn faces(mesh: &Mesh, range: std::ops::Range<usize>) -> Vec<[DVec3; 3]> {
        mesh.indices[range]
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                [t[0], t[1], t[2]].map(|i| {
                    let v = mesh.vertices[i as usize];

                    v.position().extend(f64::from(v.level) * 1e-3)
                })
            })
            .collect()
    }

    fn square_with_hole(size: f64) -> (Vec<DVec2>, Vec<u32>, Vec<u32>) {
        let o = DVec2::new(0.5, 0.33);
        let s = size;
        let points = vec![
            o,
            o + DVec2::new(s, 0.0),
            o + DVec2::new(s, s),
            o + DVec2::new(0.0, s),
            o,
            // The hole, the same way round (any way works)
            o + DVec2::new(s * 0.25, s * 0.25),
            o + DVec2::new(s * 0.75, s * 0.25),
            o + DVec2::new(s * 0.75, s * 0.75),
            o + DVec2::new(s * 0.25, s * 0.75),
        ];

        (points, vec![0, 5, 9], vec![0, 2])
    }

    fn mesh(size: f64) -> Mesh {
        let (points, ring_start, polygon_start) = square_with_hole(size);

        triangulate(&Polygons {
            points: &points,
            ring_start: &ring_start,
            polygon_start: &polygon_start,
        })
    }

    fn total_area(mesh: &Mesh) -> f64 {
        faces(mesh, 0..mesh.top)
            .iter()
            .map(|[a, b, c]| (b.truncate() - a.truncate()).perp_dot(c.truncate() - a.truncate()) / 2.0)
            .sum()
    }

    #[test]
    fn the_top_cap_covers_the_polygon_without_its_hole() {
        let s = 1e-3;
        let m = mesh(s);

        assert!(
            (total_area(&m) - s * s * 0.75).abs() < s * s * 1e-9,
            "{}",
            total_area(&m)
        );
        // Every top triangle faces up
        assert!(
            faces(&m, 0..m.top)
                .iter()
                .all(|[a, b, c]| (*b - *a).cross(*c - *a).z > 0.0)
        );
    }

    #[test]
    fn large_polygons_are_cut_along_the_grid() {
        // 10 squares across: no piece wider than a square, the area kept
        let s = SQUARE * 10.0;
        let m = mesh(s);

        assert!((total_area(&m) - s * s * 0.75).abs() < s * s * 1e-9);

        for [a, b, c] in faces(&m, 0..m.top) {
            let (lo, hi) = (a.min(b).min(c), a.max(b).max(c));

            assert!(hi.x - lo.x <= SQUARE * (1.0 + 1e-9) && hi.y - lo.y <= SQUARE * (1.0 + 1e-9));
        }
    }

    #[test]
    fn the_prism_is_closed_and_turned_outwards() {
        let m = mesh(1e-3);
        let all = faces(&m, 0..m.indices.len());
        // Closed: the area vectors of its faces add up to nothing
        let sum: DVec3 = all.iter().map(|[a, b, c]| (*b - *a).cross(*c - *a)).sum();
        let scale: f64 = all.iter().map(|[a, b, c]| (*b - *a).cross(*c - *a).length()).sum();

        assert!(sum.length() < scale * 1e-9, "{sum:?}");

        // Outwards: each wall faces away from the middle of the ring it borders (the outer ring's walls away from the
        // center, the hole's towards it)
        let center = DVec2::new(0.5 + 5e-4, 0.33 + 5e-4);

        for [a, b, c] in faces(&m, m.walls..m.indices.len()) {
            let n = (b - a).cross(c - a).truncate();
            let middle = ((a + b + c) / 3.0).truncate();
            let outer = (middle - center).abs().max_element() > 3e-4;
            let away = n.dot(middle - center) > 0.0;

            assert_eq!(away, outer, "wall at {middle:?}");
        }
    }

    #[test]
    fn extruded_blocks_have_a_roof_and_walls() {
        let (points, ring_start, polygon_start) = square_with_hole(1e-4);
        let polygons = Polygons {
            points: &points,
            ring_start: &ring_start,
            polygon_start: &polygon_start,
        };
        let b = extrude(&polygons, DVec2::new(0.5, 0.33));

        // 8 roof triangles (a square ring around a hole), 8 walls of 2 triangles
        assert_eq!(b.indices.len(), 8 * 3 + 8 * 6);

        let walls: Vec<_> = b.vertices.iter().filter(|v| v.level == 0.0).collect();

        assert_eq!(walls.len(), 16);

        // The walls facing north and west are lit, the others not (outside and in the hole: 4 walls, 2 feet each)
        let lit = b.vertices.iter().filter(|v| v.level == 0.0 && v.shade > 0.75).count();

        assert_eq!(lit, 8);
    }

    #[test]
    fn small_layers_are_cut_finer_on_the_same_grid() {
        assert_eq!(square_for(DVec2::ZERO, DVec2::ONE), SQUARE);

        let s = square_for(DVec2::new(0.5, 0.3), DVec2::new(0.505, 0.302));

        assert!(
            s < SQUARE && 0.005 / s >= CHUNKS_ACROSS && 0.005 / s < 2.0 * CHUNKS_ACROSS,
            "{s}"
        );
        // A power of two of the squares: their grid lines are lines of the coarse grid
        assert_eq!((SQUARE / s).fract(), 0.0);
        assert!((SQUARE / s).log2().fract().abs() < 1e-12);
        assert_eq!(square_for(DVec2::ONE, DVec2::ONE), SQUARE);
    }

    #[test]
    fn chunks_sort_triangles_by_square_keeping_their_order() {
        let size = 2e-3;
        let (points, ring_start, polygon_start) = square_with_hole(size);
        let polygons = Polygons {
            points: &points,
            ring_start: &ring_start,
            polygon_start: &polygon_start,
        };
        let square = square_for(points[0], points[2]);
        let mut m = triangulate_in(&polygons, square);
        let before = m.indices.clone();
        let place: std::collections::HashMap<[u32; 3], usize> = before
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
            .map(|(k, t)| (*t, k))
            .collect();
        let area = total_area(&m);
        let c = chunk(&mut m, square);

        assert_eq!(c.top.len(), c.grid.len() + 1);
        assert_eq!(*c.top.last().unwrap() as usize, m.top);
        assert_eq!(*c.rest.last().unwrap() as usize, m.indices.len());
        assert!(c.grid.len() > 100, "{}", c.grid.len());

        // The same triangles, each in its square
        let mut sorted = before.as_chunks::<3>().0.to_vec();
        let mut after = m.indices.as_chunks::<3>().0.to_vec();

        sorted.sort_unstable();
        after.sort_unstable();
        assert_eq!(sorted, after);
        assert!((total_area(&m) - area).abs() < area * 1e-12);

        for s in 0..c.grid.len() {
            for t in m.indices[c.top[s] as usize..c.top[s + 1] as usize].as_chunks::<3>().0 {
                let p = t.iter().map(|&i| m.vertices[i as usize].position()).sum::<DVec2>() / 3.0;

                assert_eq!(c.grid.square(p) as usize, s);
            }

            // Order kept within a square
            let order: Vec<usize> = m.indices[c.top[s] as usize..c.top[s + 1] as usize]
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| place[t])
                .collect();

            assert!(order.is_sorted());
        }
    }

    #[test]
    fn caps_and_walls_share_their_vertices() {
        let m = mesh(1e-3);

        // 8 points: the top cap indexes them, the bottom cap their copies at level 0, the walls stand on both
        assert_eq!((m.top_vertices, m.vertices.len()), (8, 16));
        assert_eq!((m.top, m.walls, m.indices.len()), (8 * 3, 16 * 3, 16 * 3 + 8 * 6));
        assert!(m.vertices[..8].iter().all(|v| v.level > 0.5));
        assert!(m.vertices[8..].iter().all(|v| v.level < 0.5));
        assert!(m.indices[..m.top].iter().all(|&i| i < 8));

        // Cut by the grid: the walls add their crossings, two vertices each, the pieces their points
        let big = mesh(SQUARE * 3.0);
        let n = big.vertices.len() as u32;

        assert!(big.indices.iter().all(|&i| i < n));
        assert!(big.indices[..big.top].iter().all(|&i| (i as usize) < big.top_vertices));
        assert!(
            big.vertices.len() * 3 < big.indices.len() * 2,
            "{} {}",
            big.vertices.len(),
            big.indices.len()
        );
    }

    #[test]
    fn flat_meshes_are_the_top_cap_of_the_prism() {
        let (points, ring_start, polygon_start) = square_with_hole(SQUARE * 3.0);
        let polygons = Polygons {
            points: &points,
            ring_start: &ring_start,
            polygon_start: &polygon_start,
        };
        let prism = triangulate_with(&polygons, SQUARE, true);
        let flat = triangulate_with(&polygons, SQUARE, false);

        assert_eq!(
            (flat.top, flat.walls, flat.indices.len()),
            (prism.top, prism.top, prism.top)
        );
        assert_eq!(flat.vertices.len(), prism.top_vertices);
        assert_eq!(flat.vertices, prism.vertices[..prism.top_vertices]);
        assert_eq!(flat.indices, prism.indices[..prism.top]);
        assert_eq!((flat.min, flat.max), (prism.min, prism.max));
    }

    #[test]
    fn polygons_starting_inside_the_previous_one_share_no_ring() {
        let (mut points, mut ring_start, mut polygon_start) = square_with_hole(SQUARE * 1.5);
        let o = DVec2::new(0.6, 0.4);

        points.extend([o, o + DVec2::X * 1e-3, o + DVec2::ONE * 1e-3, o]);
        ring_start.push(points.len() as u32);
        // Polygon 1 starts at ring 1 (inside polygon 0), polygon 2 at ring 3, past the rings
        polygon_start.extend([1, 3, 4]);

        let polygons = Polygons {
            points: &points,
            ring_start: &ring_start,
            polygon_start: &polygon_start,
        };
        let rings: Vec<_> = polygon_rings(&polygons).collect();

        assert_eq!(rings, [0..2, 2..2, 2..3, 3..3]);
        assert_eq!(
            polygons.rings(rings[2].clone()).map(<[DVec2]>::len).collect::<Vec<_>>(),
            [3]
        );

        let kept = OwnedPolygons::new(&polygons);
        let (a, b) = (triangulate(&polygons), triangulate(&kept.as_polygons()));

        assert_eq!((a.vertices, a.indices), (b.vertices, b.indices));
    }

    #[test]
    fn overlapping_polygons_cost_only_the_rings_there_are() {
        // 10,000 polygons each claiming all 10,000 rings: each ring goes to one polygon only
        let points = [DVec2::ZERO, DVec2::X * 1e-6, DVec2::ONE * 1e-6];
        let ring_start: Vec<u32> = (0..=10_000).map(|_| 0).chain([3]).collect();
        let polygon_start: Vec<u32> = (0..=10_000).map(|p| if p % 2 == 0 { 0 } else { u32::MAX }).collect();
        let polygons = Polygons {
            points: &points,
            ring_start: &ring_start,
            polygon_start: &polygon_start,
        };
        let total: usize = polygon_rings(&polygons).map(|r| r.len()).sum();

        assert_eq!(total, ring_start.len() - 1);
        assert_eq!(extrude(&polygons, DVec2::ZERO).parts.len(), 1);
    }

    #[test]
    fn blocks_are_sorted_by_square_polygon_by_polygon() {
        // A row of 40 small squares 50 m apart, from east to west
        let step = 50.0 / 40_075_016.686;
        let (mut points, mut ring_start, mut polygon_start) = (Vec::new(), vec![0u32], vec![0u32]);

        for k in (0..40).rev() {
            let p = DVec2::new(0.5 + f64::from(k) * step, 0.33);
            let s = step * 0.4;

            points.extend([p, p + DVec2::new(s, 0.0), p + DVec2::new(s, s), p + DVec2::new(0.0, s)]);
            ring_start.push(points.len() as u32);
            polygon_start.push(ring_start.len() as u32 - 1);
        }

        let polygons = Polygons {
            points: &points,
            ring_start: &ring_start,
            polygon_start: &polygon_start,
        };
        let mut blocks = extrude(&polygons, DVec2::new(0.5, 0.33));
        let before = blocks.indices.clone();
        let chunks = chunk_blocks(&mut blocks);

        assert_eq!(chunks.start.len(), chunks.grid.len() + 1);
        assert_eq!(*chunks.start.last().unwrap() as usize, before.len());
        assert!(chunks.reach > 0.0 && chunks.reach < step);

        let mut a = before.as_chunks::<3>().0.to_vec();
        let mut b = blocks.indices.as_chunks::<3>().0.to_vec();

        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);

        // West first now (the last polygons), each polygon in its square, its triangles together, in their order
        let feature_of = |i: u32| blocks.vertices[i as usize].feature;

        assert!(feature_of(blocks.indices[0]) >= 38);

        let mut seen = Vec::new();

        for s in 0..chunks.grid.len() {
            let range = chunks.start[s] as usize..chunks.start[s + 1] as usize;
            let mut features: Vec<u32> = blocks.indices[range].iter().map(|&i| feature_of(i)).collect();

            features.dedup();
            assert!(features.is_sorted(), "{features:?}");
            seen.extend(features);
        }
        seen.sort_unstable();
        assert_eq!(seen, (0..40).collect::<Vec<_>>());
    }

    #[test]
    fn malformed_polygons_do_not_panic_or_hang() {
        let huge = DVec2::new(1e300, 1e300);
        let points = [
            DVec2::ZERO,
            huge,
            DVec2::new(0.0, 1e300),
            DVec2::new(f64::NAN, 0.0),
            DVec2::new(f64::INFINITY, 1.0),
            DVec2::new(0.1, 0.1),
        ];

        // A huge triangle, a ring with NaN and infinity, rings whose starts point past the points
        for (ring_start, polygon_start) in [
            (vec![0, 3], vec![0, 1]),
            (vec![2, 6], vec![0, 1]),
            (vec![10, 4], vec![0, 1]),
            (vec![9, 20], vec![0, 1]),
            (vec![0, 3], vec![0, 5]),
            // Polygons claiming every ring there could be, or running backwards
            (vec![0, 3, u32::MAX], vec![0, u32::MAX, 0]),
            (vec![0, 3, 6], vec![0, 2, 0, 2, 1]),
            (vec![0, 3], vec![u32::MAX, 0, u32::MAX]),
        ] {
            let polygons = Polygons {
                points: &points,
                ring_start: &ring_start,
                polygon_start: &polygon_start,
            };
            let _ = triangulate(&polygons);
            let _ = extrude(&polygons, DVec2::ZERO);
        }
        assert_eq!(squares(DVec2::ZERO, huge, SQUARE).count(), 0);
        assert_eq!(squares(DVec2::NAN, DVec2::ONE, SQUARE).count(), 0);
        assert_eq!(crossings(DVec2::ZERO, huge, SQUARE), [0.0, 1.0]);
    }

    #[test]
    fn positions_keep_the_f64_precision() {
        let p = DVec2::new(0.512_345_678_901_234, 0.354_321_098_765_432);
        let v = Vertex::new(p, 0, 1.0);

        assert!(v.position().distance(p) < 1e-14);
    }
}
