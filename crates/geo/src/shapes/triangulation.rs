//! The fill meshes: each polygon triangulated by earcut, its triangles clipped to the grid squares, the prism's bottom
//! cap and walls added, then the triangles sorted by square (`chunk`).

use super::{Polygons, SQUARE, area, polygon_rings, ring};
use crate::pieces::{Grid, sort_by_square};
use glam::DVec2;

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
    /// A vertex at a Mercator point, its position split into `hi` and `lo`
    pub(super) fn new(m: DVec2, feature: u32, level: f32) -> Self {
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
pub(super) fn squares(min: DVec2, max: DVec2, square: f64) -> impl Iterator<Item = (DVec2, DVec2)> {
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
pub(super) fn crossings(a: DVec2, b: DVec2, square: f64) -> Vec<f64> {
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
pub(super) const CHUNKS_ACROSS: f64 = 16.0;
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
