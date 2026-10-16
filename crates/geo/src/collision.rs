//! Label placement: boxes on screen taken greedily in priority order, a box showing only if it overlaps none taken
//! before (`MapLibre`'s collision, without its fading). A uniform grid over the screen keeps each test to the few
//! boxes nearby, so thousands of labels place in well under a millisecond.

/// A box on screen, CSS pixels: min x, min y, max x, max y
pub type Rect = [f64; 4];

pub struct Grid {
    cell: f64,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<usize>>,
    boxes: Vec<Rect>,
}

impl Grid {
    /// A grid over a screen of `width` × `height` CSS pixels
    #[must_use]
    pub fn new(width: f64, height: f64) -> Self {
        let cell = 64.0;
        let cols = (width / cell).ceil().max(1.0) as usize;
        let rows = (height / cell).ceil().max(1.0) as usize;
        Self {
            cell,
            cols,
            rows,
            cells: vec![Vec::new(); cols * rows],
            boxes: Vec::new(),
        }
    }

    /// Cells a box touches, clamped to the screen
    fn span(&self, r: &Rect) -> (usize, usize, usize, usize) {
        let clamp = |v: f64, n: usize| ((v / self.cell).floor().max(0.0) as usize).min(n - 1);
        (
            clamp(r[0], self.cols),
            clamp(r[1], self.rows),
            clamp(r[2], self.cols),
            clamp(r[3], self.rows),
        )
    }

    /// Whether a box overlaps one already taken
    #[must_use]
    pub fn collides(&self, r: &Rect) -> bool {
        let (i0, j0, i1, j1) = self.span(r);
        (j0..=j1).any(|j| {
            (i0..=i1).any(|i| {
                self.cells[j * self.cols + i].iter().any(|&k| {
                    let b = &self.boxes[k];
                    r[0] < b[2] && b[0] < r[2] && r[1] < b[3] && b[1] < r[3]
                })
            })
        })
    }

    /// Takes a box: later ones overlapping it will not show
    pub fn insert(&mut self, r: Rect) {
        let (i0, j0, i1, j1) = self.span(&r);
        let k = self.boxes.len();
        self.boxes.push(r);
        for j in j0..=j1 {
            for i in i0..=i1 {
                self.cells[j * self.cols + i].push(k);
            }
        }
    }

    /// Takes the box if it fits; whether it did
    pub fn place(&mut self, r: Rect) -> bool {
        if self.collides(&r) {
            return false;
        }
        self.insert(r);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_boxes_are_refused_in_order() {
        let mut g = Grid::new(800.0, 600.0);
        assert!(g.place([10.0, 10.0, 110.0, 30.0]));
        // Overlaps the first: refused, and takes nothing
        assert!(!g.place([100.0, 20.0, 200.0, 40.0]));
        assert!(g.place([111.0, 10.0, 200.0, 30.0]));
        // Across many cells, and off the screen
        assert!(g.place([300.0, 100.0, 700.0, 500.0]));
        assert!(!g.place([650.0, 450.0, 900.0, 700.0]));
        assert!(g.place([-50.0, -50.0, 5.0, 5.0]));
    }
}
