//! Network geometry as straight pieces, sorted by square of a grid over the network: what the network layer uploads
//! once, and the squares in view it draws each frame.
//!
//! An edge runs from node A to node B along its line (or straight). Each piece of that line knows its edge and its
//! fractions of the edge's length at both ends, so that the value along it can be worked out from the two nodes and
//! the edge's costs. Pieces longer than a square are split, so that a piece never reaches further than half a square
//! out of its own: the squares, widened by that much, bound everything drawn in them.

use crate::geo::camera::View;
use crate::geo::tiles::in_frustum;
use glam::{DVec2, DVec3};

/// Squares along the longer side of the network
const SQUARES_ACROSS: f64 = 256.0;
/// Smallest square, Mercator units (about 500 m at the equator, 330 m at 48°)
const MIN_SQUARE: f64 = 500.0 / 40_075_016.686;

/// The grid of a network, Mercator
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub min: DVec2,
    pub size: f64,
    pub cols: u32,
    pub rows: u32,
}

impl Grid {
    fn square(&self, m: DVec2) -> u32 {
        let c = ((m - self.min) / self.size).floor();
        let (i, j) = (
            (c.x.max(0.0) as u32).min(self.cols - 1),
            (c.y.max(0.0) as u32).min(self.rows - 1),
        );
        j * self.cols + i
    }
}

/// One piece, as uploaded (28 bytes): its ends relative to the network origin (Mercator), its fractions of the
/// edge's length at both ends, its edge
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Piece {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub fractions: [f32; 2],
    pub edge: u32,
}

/// Pieces of a network, square after square
pub struct Pieces {
    pub grid: Grid,
    /// Pieces of square `s`: `start[s] .. start[s + 1]`
    pub start: Vec<u32>,
    pub pieces: Vec<Piece>,
}

/// Lines of the edges: those of edge `e` are `points[start[e] .. start[e + 1]]`, from A to B (Mercator)
pub struct Lines<'a> {
    pub start: &'a [u32],
    pub points: &'a [DVec2],
}

/// The line of edge `e` from A to B: its points, or its two nodes (put in `ends`)
fn line_of<'a>(e: usize, nodes: &[DVec2], edges: &[u32], lines: Option<&'a Lines>, ends: &'a mut [DVec2; 2]) -> &'a [DVec2] {
    if let Some(lines) = lines
        && e + 1 < lines.start.len()
    {
        let (s, t) = (lines.start[e] as usize, lines.start[e + 1] as usize);
        if t >= s + 2 && t <= lines.points.len() {
            return &lines.points[s..t];
        }
    }
    *ends = [nodes[edges[e * 2] as usize], nodes[edges[e * 2 + 1] as usize]];
    ends
}

/// Cuts the edges into pieces. `nodes`: Mercator positions; `edges`: node index pairs; `lines`: their shapes (an
/// edge with fewer than two points is straight); `origin`: subtracted from every point.
#[must_use]
pub fn build(nodes: &[DVec2], edges: &[u32], lines: Option<&Lines>, origin: DVec2) -> Pieces {
    let edge_count = edges.len() / 2;
    let (mut min, mut max) = (DVec2::INFINITY, DVec2::NEG_INFINITY);
    let mut ends = [DVec2::ZERO; 2];
    for e in 0..edge_count {
        for &p in line_of(e, nodes, edges, lines, &mut ends) {
            min = min.min(p);
            max = max.max(p);
        }
    }
    if edge_count == 0 {
        min = DVec2::ZERO;
        max = DVec2::ZERO;
    }
    let extent = max - min;
    let size = (extent.x.max(extent.y) / SQUARES_ACROSS).max(MIN_SQUARE);
    let grid = Grid {
        min,
        size,
        cols: (extent.x / size) as u32 + 1,
        rows: (extent.y / size) as u32 + 1,
    };
    // Pieces with their square, then a counting sort by square
    let mut squares_of: Vec<u32> = Vec::with_capacity(edge_count * 2);
    let mut unsorted: Vec<Piece> = Vec::with_capacity(edge_count * 2);
    let f32s = |p: DVec2| [p.x as f32, p.y as f32];
    for e in 0..edge_count {
        let points = line_of(e, nodes, edges, lines, &mut ends);
        let total: f64 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
        let mut along = 0.0;
        for w in points.windows(2) {
            let length = w[0].distance(w[1]);
            let parts = (length / size).ceil().max(1.0) as usize;
            for k in 0..parts {
                let (t0, t1) = (k as f64 / parts as f64, (k + 1) as f64 / parts as f64);
                let (a, b) = (w[0].lerp(w[1], t0), w[0].lerp(w[1], t1));
                let fraction = |t: f64| {
                    if total > 0.0 {
                        (along + length * t) / total
                    } else {
                        t
                    }
                };
                squares_of.push(grid.square((a + b) / 2.0));
                unsorted.push(Piece {
                    from: f32s(a - origin),
                    to: f32s(b - origin),
                    fractions: [fraction(t0) as f32, fraction(t1) as f32],
                    edge: e as u32,
                });
            }
            along += length;
        }
    }
    let squares = (grid.cols * grid.rows) as usize;
    let mut start = vec![0u32; squares + 1];
    for &s in &squares_of {
        start[s as usize + 1] += 1;
    }
    for s in 0..squares {
        start[s + 1] += start[s];
    }
    let mut fill = start.clone();
    let mut pieces = vec![Piece::default(); unsorted.len()];
    for (&s, piece) in squares_of.iter().zip(unsorted) {
        pieces[fill[s as usize] as usize] = piece;
        fill[s as usize] += 1;
    }
    Pieces { grid, start, pieces }
}

impl Pieces {
    /// See `visible`
    #[must_use]
    pub fn visible(&self, view: &View, heights: (f64, f64), shift: f64) -> Vec<(u32, u32)> {
        visible(&self.grid, &self.start, view, heights, shift)
    }
}

/// Ranges of pieces (`first..end`) in the squares of `grid` that may show in `view`, merged when consecutive; `start`:
/// first piece of each square (and the end). `heights`: lowest and highest altitudes the network may stand at
/// (pixels of the local frame); `shift`: whole worlds added to x (the copy of the world nearest to the center).
#[must_use]
pub fn visible(grid: &Grid, start: &[u32], view: &View, heights: (f64, f64), shift: f64) -> Vec<(u32, u32)> {
    let mut squares = Vec::new();
    visit(grid, view, heights, shift, (0, 0, grid.cols, grid.rows), &mut squares);
    squares.sort_unstable();
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for s in squares {
        let (first, end) = (start[s as usize], start[s as usize + 1]);
        if first == end {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last.1 == first => last.1 = end,
            _ => ranges.push((first, end)),
        }
    }
    ranges
}

/// Quadtree descent over the squares `x0..x1, y0..y1`: blocks out of the frustum are left out whole
fn visit(grid: &Grid, view: &View, heights: (f64, f64), shift: f64, block: (u32, u32, u32, u32), out: &mut Vec<u32>) {
    let (x0, y0, x1, y1) = block;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // The block, widened by half a square (pieces reach that far out of their square)
    let lo = grid.min + (DVec2::new(f64::from(x0), f64::from(y0)) - 0.5) * grid.size;
    let hi = grid.min + (DVec2::new(f64::from(x1), f64::from(y1)) + 0.5) * grid.size;
    let mut corners = [DVec3::ZERO; 18];
    for (k, corner) in corners.iter_mut().enumerate() {
        let (i, j, h) = (k % 3, (k / 3) % 3, if k < 9 { heights.0 } else { heights.1 });
        let m = lo + (hi - lo) * DVec2::new(i as f64, j as f64) / 2.0;
        *corner = view.position(m, shift) + DVec3::Z * h;
    }
    if !in_frustum(view, &corners) {
        return;
    }
    if x1 - x0 == 1 && y1 - y0 == 1 {
        out.push(y0 * grid.cols + x0);
        return;
    }
    let (mx, my) = (x0 + (x1 - x0).div_ceil(2), y0 + (y1 - y0).div_ceil(2));
    for b in [(x0, y0, mx, my), (mx, y0, x1, my), (x0, my, mx, y1), (mx, my, x1, y1)] {
        visit(grid, view, heights, shift, b, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::camera::{Camera, mercator};

    fn middle(q: &Piece) -> DVec2 {
        DVec2::new(f64::from(q.from[0] + q.to[0]), f64::from(q.from[1] + q.to[1])) / 2.0
    }

    /// A grid of streets south-east of Paris, `n` × `n` nodes 100 m apart
    fn town(n: u32) -> (Vec<DVec2>, Vec<u32>) {
        let o = mercator(2.35, 48.85);
        let step = 100.0 / 40_075_016.686 / 48.85f64.to_radians().cos();
        let nodes = (0..n * n)
            .map(|k| o + DVec2::new(f64::from(k % n), f64::from(k / n)) * step)
            .collect();
        let mut edges = Vec::new();
        for k in 0..n * n {
            if k % n + 1 < n {
                edges.extend([k, k + 1]);
            }
            if k / n + 1 < n {
                edges.extend([k, k + n]);
            }
        }
        (nodes, edges)
    }

    #[test]
    fn every_piece_lies_in_its_square() {
        let (nodes, edges) = town(200);
        let p = build(&nodes, &edges, None, DVec2::ZERO);
        assert_eq!(p.pieces.len(), edges.len() / 2);
        for s in 0..(p.grid.cols * p.grid.rows) as usize {
            for piece in &p.pieces[p.start[s] as usize..p.start[s + 1] as usize] {
                assert_eq!(p.grid.square(middle(piece)) as usize, s);
            }
        }
    }

    #[test]
    fn lines_give_pieces_and_fractions() {
        // One edge A → B bent through a middle point: two pieces, halves of its length
        // (shorter than a square, so not split further)
        let nodes = [DVec2::new(0.5, 0.3), DVec2::new(0.500_002, 0.3)];
        let points = [nodes[0], DVec2::new(0.500_001, 0.300_001), nodes[1]];
        let lines = Lines {
            start: &[0, 3],
            points: &points,
        };
        let p = build(&nodes, &[0, 1], Some(&lines), DVec2::new(0.5, 0.3));
        assert_eq!(p.pieces.len(), 2);
        let mut fractions: Vec<_> = p.pieces.iter().map(|q| q.fractions).collect();
        fractions.sort_by(|a, b| a[0].total_cmp(&b[0]));
        assert!(fractions[0][0].abs() < 1e-6 && (fractions[0][1] - 0.5).abs() < 1e-6);
        assert!((fractions[1][1] - 1.0).abs() < 1e-6);
        assert!(p.pieces.iter().all(|q| q.edge == 0));
    }

    #[test]
    fn long_edges_are_split() {
        // A 50 km straight edge: split to the square size, fractions continuous
        let nodes = [mercator(2.0, 48.0), mercator(2.7, 48.0)];
        let p = build(&nodes, &[0, 1], None, nodes[0]);
        assert!(p.pieces.len() > 50, "{}", p.pieces.len());
        let length = |q: &Piece| f64::from((q.to[0] - q.from[0]).hypot(q.to[1] - q.from[1]));
        assert!(p.pieces.iter().all(|q| length(q) <= p.grid.size * 1.001));
    }

    #[test]
    fn only_squares_in_view() {
        // 20 km × 20 km of streets, a view of a few hundred metres inside it
        let (nodes, edges) = town(200);
        let p = build(&nodes, &edges, None, DVec2::ZERO);
        let mut c = Camera::new(2.45, 48.78, 16.0);
        c.width = 1200.0;
        c.height = 800.0;
        let view = c.view();
        let ranges = p.visible(&view, (0.0, 0.0), 0.0);
        let drawn: u32 = ranges.iter().map(|(a, b)| b - a).sum();
        assert!(drawn > 0 && (drawn as usize) < p.pieces.len() / 50, "{drawn} of {}", p.pieces.len());
        // Every piece on screen is drawn
        let center = c.center;
        let on_screen = p
            .pieces
            .iter()
            .enumerate()
            .filter(|(_, q)| middle(q).distance(center) * view.world_size < 300.0)
            .map(|(k, _)| k as u32);
        for k in on_screen {
            assert!(ranges.iter().any(|(a, b)| (*a..*b).contains(&k)));
        }
        // Tilted, more squares; far away, none
        c.pitch = 70.0;
        let tilted: u32 = p.visible(&c.view(), (0.0, 0.0), 0.0).iter().map(|(a, b)| b - a).sum();
        assert!(tilted > drawn);
        let mut far = Camera::new(-70.0, 40.0, 14.0);
        far.width = 1200.0;
        far.height = 800.0;
        assert_eq!(p.visible(&far.view(), (0.0, 0.0), 0.0), Vec::new());
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore = "timing"]
    fn build_a_million_edges() {
        let n = 1000u32;
        let nodes: Vec<DVec2> = (0..n * 500).map(|k| DVec2::new(0.5 + f64::from(k % n) * 2e-6, 0.3 + f64::from(k / n) * 2e-6)).collect();
        let mut edges = Vec::new();
        let (mut start, mut points) = (vec![0u32], Vec::new());
        for k in 0..n * 500 {
            for b in [k + 1, k + n] {
                if (b == k + 1 && k % n + 1 < n) || (b == k + n && b < n * 500) {
                    edges.extend([k, b]);
                    let (pa, pb) = (nodes[k as usize], nodes[b as usize]);
                    points.extend([pa, (pa + pb) / 2.0 + DVec2::splat(3e-7), pb]);
                    start.push(points.len() as u32);
                }
            }
        }
        let lines = Lines { start: &start, points: &points };
        let t = std::time::Instant::now();
        let p = build(&nodes, &edges, Some(&lines), DVec2::new(0.5, 0.3));
        eprintln!("{} pieces in {:?}", p.pieces.len(), t.elapsed());
    }
}
