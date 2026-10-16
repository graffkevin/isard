//! Polygons as triangles, for the fill layers: each polygon is triangulated (earcut, holes included), then cut along
//! a grid of fixed Mercator squares so that no triangle spans more than a square: on the globe the pieces follow the
//! sphere, and neighbours cut along the same grid lines share their vertices (no cracks).
//!
//! Over the relief a fill is drawn as a shadow volume: the polygon lifted into a prism from below the lowest ground to
//! above the highest, its faces counted in the stencil where they lie behind the ground (z-fail), the ground inside
//! the prism then colored. So `Mesh` holds the prism: the triangles twice (the top and the bottom caps) and the
//! walls along the rings, every face turned outwards, which the counting needs (an overlap of two polygons still
//! counts as inside). On flat ground only the top cap is drawn.
//!
//! Positions are split into a high and a low f32 part (`hi + lo` = the f64 Mercator coordinate): a polygon as large as
//! the world (a veil with a hole) keeps its edges to the centimetre at any zoom, the GPU subtracting the camera center
//! part by part.

use glam::DVec2;

/// Side of the grid squares, Mercator (about 300 km at the equator): on the globe a chord that long sags less than
/// 2 km under the sphere
pub const SQUARE: f64 = 1.0 / 128.0;

/// Polygons in Mercator: polygon `p` has rings `polygon_start[p] .. polygon_start[p + 1]` (the first is the outer one,
/// the others holes), ring `r` the points `points[ring_start[r] .. ring_start[r + 1]]` (closing point optional)
pub struct Polygons<'a> {
    pub points: &'a [DVec2],
    pub ring_start: &'a [u32],
    pub polygon_start: &'a [u32],
}

/// A vertex as uploaded (24 bytes): its position in two parts, its polygon, its level (0: bottom of the prism, 1: top)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub hi: [f32; 2],
    pub lo: [f32; 2],
    pub polygon: u32,
    pub level: f32,
}

impl Vertex {
    fn new(m: DVec2, polygon: u32, level: f32) -> Self {
        let hi = [m.x as f32, m.y as f32];
        let lo = [(m.x - f64::from(hi[0])) as f32, (m.y - f64::from(hi[1])) as f32];
        Self { hi, lo, polygon, level }
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
/// bottom cap, `indices[walls..]` the walls
#[derive(Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub top: usize,
    pub walls: usize,
    /// Mercator bounds of the points
    pub min: DVec2,
    pub max: DVec2,
}

/// Points of a ring without its closing point
fn ring<'a>(polygons: &Polygons<'a>, r: usize) -> &'a [DVec2] {
    let (s, e) = (polygons.ring_start[r] as usize, polygons.ring_start[r + 1] as usize);
    let points = &polygons.points[s.min(e)..e.min(polygons.points.len())];
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

/// Clips a convex polygon to the square `[lo, hi]` (Sutherland-Hodgman, one side at a time)
fn clip(polygon: &[DVec2], lo: DVec2, hi: DVec2) -> Vec<DVec2> {
    let mut out = polygon.to_vec();
    // Each side: the coordinate (0: x, 1: y), the bound and whether points below it are kept
    for (axis, bound, below) in [(0, lo.x, false), (0, hi.x, true), (1, lo.y, false), (1, hi.y, true)] {
        let input = std::mem::take(&mut out);
        let inside = |p: DVec2| if below { p[axis] <= bound } else { p[axis] >= bound };
        for (i, &p) in input.iter().enumerate() {
            let q = input[(i + 1) % input.len()];
            if inside(p) {
                out.push(p);
            }
            if inside(p) != inside(q) {
                let t = (bound - p[axis]) / (q[axis] - p[axis]);
                let mut x = p.lerp(q, t);
                // Exactly on the grid line: the neighbour computes the same point
                x[axis] = bound;
                out.push(x);
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// Grid squares (`[lo, hi]` corners) a box touches
fn squares(min: DVec2, max: DVec2) -> impl Iterator<Item = (DVec2, DVec2)> {
    let (i0, i1) = ((min.x / SQUARE).floor() as i64, (max.x / SQUARE).ceil() as i64);
    let (j0, j1) = ((min.y / SQUARE).floor() as i64, (max.y / SQUARE).ceil() as i64);
    (j0..j1.max(j0 + 1)).flat_map(move |j| {
        (i0..i1.max(i0 + 1)).map(move |i| {
            let lo = DVec2::new(i as f64, j as f64) * SQUARE;
            (lo, lo + SQUARE)
        })
    })
}

/// Fractions along `a` → `b` where it crosses the grid lines, with 0 and 1, in order
fn crossings(a: DVec2, b: DVec2) -> Vec<f64> {
    let mut t = vec![0.0, 1.0];
    for axis in 0..2 {
        let (p, q) = (a[axis], b[axis]);
        if (p - q).abs() < f64::EPSILON {
            continue;
        }
        let (lo, hi) = (p.min(q), p.max(q));
        let mut k = (lo / SQUARE).floor() + 1.0;
        while k * SQUARE < hi {
            t.push((k * SQUARE - p) / (q - p));
            k += 1.0;
        }
    }
    t.sort_by(f64::total_cmp);
    t.dedup();
    t
}

/// Triangulates the polygons (see the module)
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn triangulate(polygons: &Polygons) -> Mesh {
    let mut mesh = Mesh {
        min: DVec2::INFINITY,
        max: DVec2::NEG_INFINITY,
        ..Mesh::default()
    };
    let mut caps: Vec<[DVec2; 3]> = Vec::new();
    let mut cap_polygon: Vec<u32> = Vec::new();
    let mut walls: Vec<(DVec2, DVec2, u32)> = Vec::new();
    let mut earcut = earcut::Earcut::new();
    let (mut flat, mut holes, mut triangles): (Vec<[f64; 2]>, Vec<u32>, Vec<u32>) =
        (Vec::new(), Vec::new(), Vec::new());
    let count = polygons.polygon_start.len().saturating_sub(1);
    for p in 0..count {
        let (first, last) = (
            polygons.polygon_start[p] as usize,
            (polygons.polygon_start[p + 1] as usize).min(polygons.ring_start.len().saturating_sub(1)),
        );
        flat.clear();
        holes.clear();
        let mut all: Vec<DVec2> = Vec::new();
        for r in first..last {
            let points = ring(polygons, r);
            if points.len() < 3 {
                continue;
            }
            if r > first {
                holes.push(all.len() as u32);
            }
            all.extend_from_slice(points);
            // Walls turned outwards: away from the polygon's inside (outside of a hole)
            let outer = r == first;
            let counterclockwise = area(points) > 0.0;
            for i in 0..points.len() {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                // With the inside on the left of a → b (counterclockwise outer ring, clockwise hole), the outward
                // normal is (dy, -dx): the order the wall triangles below assume
                if outer == counterclockwise {
                    walls.push((a, b, p as u32));
                } else {
                    walls.push((b, a, p as u32));
                }
                mesh.min = mesh.min.min(a);
                mesh.max = mesh.max.max(a);
            }
        }
        if all.len() < 3 {
            continue;
        }
        // Relative to the first point: earcut works in f64 but its predicates like small numbers
        let base = all[0];
        flat.extend(all.iter().map(|&q| (q - base).to_array()));
        earcut.earcut(flat.iter().copied(), &holes, &mut triangles);
        for t in triangles.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| all[i as usize]);
            // Counterclockwise: the top cap faces up
            let (b, c) = if (b - a).perp_dot(c - a) >= 0.0 { (b, c) } else { (c, b) };
            caps.push([a, b, c]);
            cap_polygon.push(p as u32);
        }
    }
    // The caps, cut along the grid
    let mut top: Vec<(DVec2, u32)> = Vec::new();
    for (tri, &p) in caps.iter().zip(&cap_polygon) {
        let min = tri[0].min(tri[1]).min(tri[2]);
        let max = tri[0].max(tri[1]).max(tri[2]);
        for (lo, hi) in squares(min, max) {
            let piece = clip(tri, lo, hi);
            if piece.len() < 3 || area(&piece).abs() < 1e-30 {
                continue;
            }
            // A convex piece: a fan, still counterclockwise
            for k in 1..piece.len() - 1 {
                top.extend([(piece[0], p), (piece[k], p), (piece[k + 1], p)]);
            }
        }
    }
    for &(m, p) in &top {
        mesh.indices.push(mesh.vertices.len() as u32);
        mesh.vertices.push(Vertex::new(m, p, 1.0));
    }
    mesh.top = mesh.indices.len();
    // The bottom cap faces down: the same triangles the other way round
    for t in top.as_chunks::<3>().0 {
        for &(m, p) in [&t[0], &t[2], &t[1]] {
            mesh.indices.push(mesh.vertices.len() as u32);
            mesh.vertices.push(Vertex::new(m, p, 0.0));
        }
    }
    mesh.walls = mesh.indices.len();
    for (a, b, p) in walls {
        // Split where they cross the grid lines, as the caps are: the walls meet the caps vertex to vertex
        let cuts = crossings(a, b);
        for w in cuts.windows(2) {
            let (u, v) = (a.lerp(b, w[0]), a.lerp(b, w[1]));
            let base = mesh.vertices.len() as u32;
            mesh.vertices.extend([
                Vertex::new(u, p, 0.0),
                Vertex::new(v, p, 0.0),
                Vertex::new(v, p, 1.0),
                Vertex::new(u, p, 1.0),
            ]);
            mesh.indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    if mesh.vertices.is_empty() {
        mesh.min = DVec2::ZERO;
        mesh.max = DVec2::ZERO;
    }
    mesh
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
    fn positions_keep_the_f64_precision() {
        let p = DVec2::new(0.512_345_678_901_234, 0.354_321_098_765_432);
        let v = Vertex::new(p, 0, 1.0);
        assert!(v.position().distance(p) < 1e-14);
    }
}

/// A vertex of an extruded polygon (32 bytes): its position and its polygon's centroid relative to the layer origin,
/// its polygon, its level (0: foot of a wall, 1: roof) and its shade (the light on its face)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Block {
    pub offset: [f32; 2],
    pub centroid: [f32; 2],
    pub polygon: u32,
    pub level: f32,
    pub shade: f32,
    pub pad: f32,
}

/// Extruded polygons: roofs then walls, as indexed triangles
#[derive(Default)]
pub struct Blocks {
    pub vertices: Vec<Block>,
    pub indices: Vec<u32>,
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
    for p in 0..polygons.polygon_start.len().saturating_sub(1) {
        let (first, last) = (
            polygons.polygon_start[p] as usize,
            (polygons.polygon_start[p + 1] as usize).min(polygons.ring_start.len().saturating_sub(1)),
        );
        let rings: Vec<&[DVec2]> = (first..last)
            .map(|r| ring(polygons, r))
            .filter(|r| r.len() >= 3)
            .collect();
        let Some(outer) = rings.first() else {
            continue;
        };
        let centroid = outer.iter().sum::<DVec2>() / outer.len() as f64;
        let c = f32s(centroid - origin);
        let vertex = |m: DVec2, level: f32, shade: f32| Block {
            offset: f32s(m - origin),
            centroid: c,
            polygon: p as u32,
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
        // Walls, each with its own vertices (flat shading)
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
    out
}
