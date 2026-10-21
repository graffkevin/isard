//! Network geometry as straight pieces, sorted by square of a grid over the network: what the network layer uploads
//! once, and the squares in view it draws each frame.
//!
//! An edge runs from node A to node B along its line (or straight). Each piece of that line knows its edge and its
//! fractions of the edge's length at both ends, so that the value along it can be worked out from the two nodes and
//! the edge's costs. Pieces longer than a square are split, so that a piece never reaches further than half a square
//! out of its own: the squares, widened by that much, bound everything drawn in them.

use crate::camera::View;
use crate::tiles::{ALL_PLANES, NO_FAR_PLANE, in_planes};
use glam::{DVec2, DVec3};

/// Squares along the longer side of the network
const SQUARES_ACROSS: f64 = 256.0;
/// Smallest square, Mercator units (about 500 m at the equator, 330 m at 48°)
const MIN_SQUARE: f64 = 500.0 / 40_075_016.686;
/// Most squares along a side of any grid: the squares grow past it (16 M squares at most)
const MAX_SIDE: u32 = 4096;

/// The grid of a network, Mercator
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Corner of the first square (north-west), Mercator
    pub min: DVec2,
    /// Side of a square, Mercator
    pub size: f64,
    /// Squares from west to east, at least 1
    pub cols: u32,
    /// Squares from north to south, at least 1; square `j * cols + i` is column `i` of row `j`
    pub rows: u32,
}

impl Grid {
    /// A grid of squares of side `size` from `min`, enough of them to cover `max`, at most `MAX_SIDE` along a side
    /// (larger squares then); bounds that are not numbers give one square at the origin
    #[must_use]
    pub fn over(min: DVec2, max: DVec2, size: f64) -> Self {
        let (min, max) = if min.is_finite() && max.is_finite() {
            (min, max)
        } else {
            (DVec2::ZERO, DVec2::ZERO)
        };
        let extent = (max - min).max(DVec2::ZERO);
        let size = if size.is_finite() && size > 0.0 { size } else { 1.0 };
        // A little under the limit: the rounding of the division never adds a square past it
        let size = size.max(extent.max_element() / f64::from(MAX_SIDE - 2));
        let squares = |e: f64| ((e / size) as u32).saturating_add(1).min(MAX_SIDE);

        Self {
            min,
            size,
            cols: squares(extent.x),
            rows: squares(extent.y),
        }
    }

    /// Number of squares
    #[must_use]
    pub fn len(&self) -> usize {
        self.cols as usize * self.rows as usize
    }

    /// Whether the grid has no square (never: a grid has at least one)
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The square holding a point (the nearest one for a point outside)
    #[must_use]
    pub fn square(&self, m: DVec2) -> u32 {
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
    /// Start, relative to the network origin (Mercator)
    pub from: [f32; 2],
    /// End, relative to the network origin (Mercator)
    pub to: [f32; 2],
    /// Fractions of the edge's length at `from` and `to` (0 at node A, 1 at node B)
    pub fractions: [f32; 2],
    /// Index of the edge the piece belongs to
    pub edge: u32,
}

/// Pieces of a network, square after square
pub struct Pieces {
    /// The squares the pieces are sorted by
    pub grid: Grid,
    /// Pieces of square `s`: `start[s] .. start[s + 1]`
    pub start: Vec<u32>,
    /// Every piece, square after square
    pub pieces: Vec<Piece>,
}

/// Lines of the edges: those of edge `e` are `points[start[e] .. start[e + 1]]`, from A to B (Mercator)
pub struct Lines<'a> {
    /// Where the line of each edge starts in `points`, with one more entry for the end of the last
    pub start: &'a [u32],
    /// Points of every line, one after the other, Mercator
    pub points: &'a [DVec2],
}

/// The line of edge `e` from A to B: its points, or its two nodes (put in `ends`)
fn line_of<'a>(
    e: usize,
    nodes: &[DVec2],
    edges: &[u32],
    lines: Option<&'a Lines>,
    ends: &'a mut [DVec2; 2],
) -> &'a [DVec2] {
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
    // Points that are not numbers are left out (and the pieces that reach them)
    let (mut min, mut max) = (DVec2::INFINITY, DVec2::NEG_INFINITY);
    let mut ends = [DVec2::ZERO; 2];

    for e in 0..edge_count {
        for &p in line_of(e, nodes, edges, lines, &mut ends) {
            if p.is_finite() {
                min = min.min(p);
                max = max.max(p);
            }
        }
    }

    let extent = max - min;
    let grid = Grid::over(min, max, (extent.x.max(extent.y) / SQUARES_ACROSS).max(MIN_SQUARE));
    let size = grid.size;
    let finite = |w: &[DVec2]| w[0].is_finite() && w[1].is_finite();
    // Two passes over the edges: the pieces of each square counted, then written straight at their place (no
    // unsorted copy: millions of pieces for a region)
    let f32s = |p: DVec2| [p.x as f32, p.y as f32];
    let each = |visit: &mut dyn FnMut(u32, Piece)| {
        let mut ends = [DVec2::ZERO; 2];

        for e in 0..edge_count {
            let points = line_of(e, nodes, edges, lines, &mut ends);
            let total: f64 = points
                .windows(2)
                .filter(|w| finite(w))
                .map(|w| w[0].distance(w[1]))
                .sum();
            let mut along = 0.0;

            for w in points.windows(2).filter(|w| finite(w)) {
                let length = w[0].distance(w[1]);
                let parts = (length / size).ceil().max(1.0) as usize;

                for k in 0..parts {
                    let (t0, t1) = (k as f64 / parts as f64, (k + 1) as f64 / parts as f64);
                    let (a, b) = (w[0].lerp(w[1], t0), w[0].lerp(w[1], t1));
                    let fraction = |t: f64| {
                        if total > 0.0 { (along + length * t) / total } else { t }
                    };

                    visit(
                        grid.square((a + b) / 2.0),
                        Piece {
                            from: f32s(a - origin),
                            to: f32s(b - origin),
                            fractions: [fraction(t0) as f32, fraction(t1) as f32],
                            edge: e as u32,
                        },
                    );
                }
                along += length;
            }
        }
    };
    let squares = grid.len();
    let mut start = vec![0u32; squares + 1];

    each(&mut |s, _| start[s as usize + 1] += 1);
    for s in 0..squares {
        start[s + 1] += start[s];
    }

    let mut fill = start.clone();
    let mut pieces = vec![Piece::default(); start[squares] as usize];

    each(&mut |s, piece| {
        pieces[fill[s as usize] as usize] = piece;
        fill[s as usize] += 1;
    });

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
    ranges(&squares_in_view(grid, view, heights, shift, 0.0, true), start)
}

/// Squares of `grid` that may show in `view`, in increasing order. What a square holds reaches at most half a square
/// plus `margin` (Mercator) out of it; `heights` and `shift` as in `visible`. `far`: squares beyond the far plane are
/// left out (not for what is kept on the far plane rather than clipped, as the shadow volumes of the fills)
#[must_use]
pub fn squares_in_view(grid: &Grid, view: &View, heights: (f64, f64), shift: f64, margin: f64, far: bool) -> Vec<u32> {
    let mut squares = Vec::new();

    squares_in_view_into(grid, view, (heights, shift, margin, far), &mut squares);

    squares
}

/// `squares_in_view` into `out` (cleared first), with (heights, shift, margin, far)
fn squares_in_view_into(grid: &Grid, view: &View, (heights, shift, margin, far): Look, out: &mut Vec<u32>) {
    out.clear();

    let reach = 0.5 + margin.max(0.0) / grid.size;
    let planes = if far { ALL_PLANES } else { NO_FAR_PLANE };

    visit(
        grid,
        view,
        (heights, shift, reach, planes),
        (0, 0, grid.cols, grid.rows),
        out,
    );
    out.sort_unstable();
}

/// How a layer looks for its squares in view (as `squares_in_view` takes them): heights, shift, margin, far
pub type Look = ((f64, f64), f64, f64, bool);

/// The squares in view of a layer and the ranges of items they hold, kept from frame to frame: their buffers reused,
/// and nothing worked out again for the view they were worked out for (the picking pass after a frame, a frame
/// redrawn for another layer). `invalidate` when the grid or the items change
#[derive(Default)]
pub struct Visible {
    /// What they were worked out for: the view (matrix, center, scale, globe) and the look
    key: Option<[u64; 26]>,
    squares: Vec<u32>,
    ranges: Vec<(u32, u32)>,
}

impl Visible {
    /// The bits of everything the squares in view depend on: the view's matrix, center, scale, globe blend and
    /// vertical scale, and the look
    fn key(view: &View, ((low, high), shift, margin, far): Look) -> [u64; 26] {
        let mut key = [0u64; 26];

        for (k, v) in view.view_proj.to_cols_array().into_iter().enumerate() {
            key[k] = v.to_bits();
        }

        let rest = [
            view.center.x,
            view.center.y,
            view.world_size,
            view.globe.factor,
            view.vertical,
            low,
            high,
            shift,
            margin,
            f64::from(u8::from(far)),
        ];

        for (k, v) in rest.into_iter().enumerate() {
            key[16 + k] = v.to_bits();
        }

        key
    }

    /// The ranges of items (`start`: the first of each square, and the end) in the squares of `grid` in view, worked
    /// out again only for another view or look
    pub fn update(&mut self, grid: &Grid, start: &[u32], view: &View, look: Look) -> &[(u32, u32)] {
        let key = Self::key(view, look);

        if self.key != Some(key) {
            squares_in_view_into(grid, view, look, &mut self.squares);
            ranges_into(&self.squares, start, &mut self.ranges);
            self.key = Some(key);
        }

        &self.ranges
    }

    /// The ranges of the last `update` if it was for this view and look (without changing anything: the picking pass)
    #[must_use]
    pub fn cached(&self, view: &View, look: Look) -> Option<&[(u32, u32)]> {
        (self.key == Some(Self::key(view, look))).then_some(self.ranges.as_slice())
    }

    /// The ranges of the last `update`
    #[must_use]
    pub fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    /// The squares in view of the last `update`, in increasing order
    #[must_use]
    pub fn squares(&self) -> &[u32] {
        &self.squares
    }

    /// Forgets what was worked out: the grid or the items changed
    pub fn invalidate(&mut self) {
        self.key = None;
    }
}

/// Ranges of items (`first..end`) of some squares (in increasing order), merged when consecutive; `start`: first item
/// of each square (and the end). Empty squares and squares out of `start` are skipped
#[must_use]
pub fn ranges(squares: &[u32], start: &[u32]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();

    ranges_into(squares, start, &mut out);

    out
}

/// `ranges` into `out` (cleared first)
pub fn ranges_into(squares: &[u32], start: &[u32], out: &mut Vec<(u32, u32)>) {
    out.clear();

    for &s in squares {
        let (Some(&first), Some(&end)) = (start.get(s as usize), start.get(s as usize + 1)) else {
            continue;
        };

        if first >= end {
            continue;
        }

        match out.last_mut() {
            Some(last) if last.1 == first => last.1 = end,
            _ => out.push((first, end)),
        }
    }
}

/// Stable counting sort of items by square: the order to put them in (item indices) and the first position of each
/// of the `squares` squares (and the end). Items keep their relative order within a square
#[must_use]
pub fn sort_by_square(square_of: &[u32], squares: usize) -> (Vec<u32>, Vec<u32>) {
    let mut start = vec![0u32; squares + 1];

    for &s in square_of {
        start[(s as usize).min(squares.saturating_sub(1)) + 1] += 1;
    }
    for s in 0..squares {
        start[s + 1] += start[s];
    }

    let mut fill = start.clone();
    let mut order = vec![0u32; square_of.len()];

    for (item, &s) in square_of.iter().enumerate() {
        let s = (s as usize).min(squares.saturating_sub(1));

        order[fill[s] as usize] = item as u32;
        fill[s] += 1;
    }

    (order, start)
}

/// What a square holds and the planes it is tested against: heights, shift, how far (in squares) it reaches out of
/// the square, the planes of the frustum (`tiles::in_planes`)
type Bounds = ((f64, f64), f64, f64, u8);

/// Quadtree descent over the squares `x0..x1, y0..y1`: blocks out of the frustum are left out whole
fn visit(grid: &Grid, view: &View, bounds: Bounds, block: (u32, u32, u32, u32), out: &mut Vec<u32>) {
    let (heights, shift, reach, planes) = bounds;
    let (x0, y0, x1, y1) = block;

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    // The block, widened by what its squares hold reaches out of them
    let lo = grid.min + (DVec2::new(f64::from(x0), f64::from(y0)) - reach) * grid.size;
    let hi = grid.min + (DVec2::new(f64::from(x1), f64::from(y1)) + reach) * grid.size;
    let mut corners = [DVec3::ZERO; 18];

    for (k, corner) in corners.iter_mut().enumerate() {
        let (i, j, h) = (k % 3, (k / 3) % 3, if k < 9 { heights.0 } else { heights.1 });
        let m = lo + (hi - lo) * DVec2::new(i as f64, j as f64) / 2.0;

        *corner = view.position(m, shift) + DVec3::Z * h;
    }
    if !in_planes(view, &corners, planes) {
        return;
    }
    if x1 - x0 == 1 && y1 - y0 == 1 {
        out.push(y0 * grid.cols + x0);

        return;
    }

    let (mx, my) = (x0 + (x1 - x0).div_ceil(2), y0 + (y1 - y0).div_ceil(2));

    for b in [(x0, y0, mx, my), (mx, y0, x1, my), (x0, my, mx, y1), (mx, my, x1, y1)] {
        visit(grid, view, bounds, b, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Camera, mercator};

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
    fn items_sorted_by_square_keep_their_order() {
        let (order, start) = sort_by_square(&[2, 0, 2, 1, 0], 3);

        assert_eq!(order, [1, 4, 3, 0, 2]);
        assert_eq!(start, [0, 2, 3, 5]);
        // Consecutive squares merge, empty ones are skipped
        assert_eq!(ranges(&[0, 1, 2], &start), [(0, 5)]);
        assert_eq!(ranges(&[0, 2], &start), [(0, 2), (3, 5)]);
        assert_eq!(ranges(&[0, 7], &[0, 0, 4]), Vec::new());

        let grid = Grid::over(DVec2::ZERO, DVec2::new(1.0, 0.5), 0.25);

        assert_eq!((grid.cols, grid.rows, grid.len()), (5, 3, 15));
        assert_eq!(grid.square(DVec2::new(0.3, 0.3)), 6);
        assert_eq!(grid.square(DVec2::new(-1.0, 9.0)), 10);
    }

    #[test]
    fn a_margin_widens_the_squares_in_view() {
        // Squares of 100 m around a point: a view next to them sees them only with a margin
        let o = mercator(2.35, 48.85);
        let size = 100.0 / 40_075_016.686;
        let grid = Grid::over(o, o + DVec2::splat(size * 4.0), size);
        let mut c = Camera::new(2.35, 48.85, 18.0);

        c.width = 400.0;
        c.height = 400.0;
        c.center = o + DVec2::new(size * 6.5, size * 2.0);

        let view = c.view();

        assert_eq!(
            squares_in_view(&grid, &view, (0.0, 0.0), 0.0, 0.0, true),
            Vec::<u32>::new()
        );

        let wide = squares_in_view(&grid, &view, (0.0, 0.0), 0.0, size * 2.0, true);

        assert!(!wide.is_empty() && wide.is_sorted());
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

        assert!(
            drawn > 0 && (drawn as usize) < p.pieces.len() / 50,
            "{drawn} of {}",
            p.pieces.len()
        );

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

    #[test]
    fn grids_stay_small_whatever_their_bounds() {
        // Bounds that are not numbers: one square
        for (min, max) in [
            (DVec2::ZERO, DVec2::new(f64::INFINITY, 1.0)),
            (DVec2::new(f64::NAN, 0.0), DVec2::ONE),
            (DVec2::NEG_INFINITY, DVec2::INFINITY),
        ] {
            let grid = Grid::over(min, max, 1e-6);

            assert_eq!(grid.len(), 1, "{min} {max}");
            assert_eq!(grid.square(DVec2::new(f64::NAN, 0.5)), 0);
        }

        // Tiny squares over a wide extent: larger squares, at most MAX_SIDE a side, still covering the extent
        let grid = Grid::over(DVec2::ZERO, DVec2::new(1e6, 1.0), 1e-9);

        assert!(grid.cols <= MAX_SIDE && grid.rows <= MAX_SIDE);
        assert!(f64::from(grid.cols) * grid.size >= 1e6);
        assert_eq!(grid.square(DVec2::new(1e6, 1.0)) % grid.cols, grid.cols - 1);
    }

    #[test]
    fn points_not_numbers_are_left_out_of_the_pieces() {
        let nodes = [
            DVec2::new(0.5, 0.5),
            DVec2::new(0.5001, 0.5),
            DVec2::new(f64::INFINITY, 0.5),
        ];
        let points = [DVec2::new(0.5, 0.5), DVec2::new(f64::NAN, 1.0), DVec2::new(0.5001, 0.5)];
        let lines = Lines {
            start: &[0, 3],
            points: &points,
        };
        // Edge 0 along a line with a point not a number, edge 1 to a node at infinity: no piece reaches them
        let p = build(&nodes, &[0, 1, 0, 2], Some(&lines), nodes[0]);

        assert!(p.grid.len() < 100_000, "{}", p.grid.len());
        assert!(
            p.pieces
                .iter()
                .all(|q| q.from.iter().chain(&q.to).all(|v| v.is_finite()))
        );

        let only_infinite = build(&nodes, &[2, 2], None, DVec2::ZERO);

        assert!(only_infinite.pieces.is_empty() && only_infinite.grid.len() == 1);
    }

    #[test]
    fn visible_squares_are_kept_for_the_same_view() {
        let (nodes, edges) = town(50);
        let p = build(&nodes, &edges, None, DVec2::ZERO);
        let mut c = Camera::new(2.36, 48.84, 16.0);

        c.width = 800.0;
        c.height = 600.0;

        let view = c.view();
        let look = ((0.0, 0.0), 0.0, 0.0, true);
        let mut visible = Visible::default();

        assert!(visible.cached(&view, look).is_none());

        let ranges = visible.update(&p.grid, &p.start, &view, look).to_vec();

        assert_eq!(ranges, p.visible(&view, (0.0, 0.0), 0.0));
        assert_eq!(visible.cached(&view, look), Some(ranges.as_slice()));
        assert!(visible.squares().is_sorted() && !visible.squares().is_empty());
        // Another view or another look: worked out again
        c.bearing = 30.0;
        assert!(visible.cached(&c.view(), look).is_none());
        assert!(visible.cached(&view, ((0.0, 1.0), 0.0, 0.0, true)).is_none());
        visible.invalidate();
        assert!(visible.cached(&view, look).is_none());
    }
}
