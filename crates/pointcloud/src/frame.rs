//! From the coordinates of a file to ECEF: a grid of positions over a node's square, each computed exactly by the
//! reader (the CRS's inverse projection, the geoid, the ellipsoid), interpolated between for each point. Over the
//! nodes drawn up close (tens to hundreds of metres) the interpolation is exact to well under a millimetre: the
//! projection and the ellipsoid curve by a few centimetres over a kilometre, a bilinear cell of a ninth of it by
//! their square. The CRS stays the page's (any projection of proj4), the points are decoded in the workers.

use glam::DVec3;

/// Most nodes along a side of a grid
pub const MAX_SIDE: usize = 17;

/// A grid of positions over a square of the CRS: at each of its `side × side` nodes, the ECEF position of the ground
/// (relative to the anchor, the height of the geoid there when the altitudes are above it) and the vertical. A point
/// is at `base(x, y) + z × vertical_scale × up(x, y)`
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    /// Corners of the square (CRS units)
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub side: usize,
    /// Metres of a unit of z
    pub vertical_scale: f64,
    /// Row after row (y), x along each: ECEF offsets from the anchor, and unit verticals
    pub base: Vec<DVec3>,
    pub up: Vec<DVec3>,
}

/// Numbers of a grid before its nodes: the corners, the side, the vertical scale
const HEAD: usize = 6;

impl Grid {
    /// The grid in the numbers the worker gets: min x, min y, max x, max y, side, vertical scale, then base and up of
    /// each node (6 numbers)
    #[must_use]
    pub fn numbers(&self) -> Vec<f64> {
        let mut out = vec![
            self.min[0],
            self.min[1],
            self.max[0],
            self.max[1],
            self.side as f64,
            self.vertical_scale,
        ];
        for (b, u) in self.base.iter().zip(&self.up) {
            out.extend(b.to_array());
            out.extend(u.to_array());
        }
        out
    }

    /// The grid of `numbers`; an error unless they hold a whole grid of finite numbers
    pub fn parse(numbers: &[f64]) -> Result<Self, String> {
        let bad = || "bad point cloud grid".to_owned();
        let [x0, y0, x1, y1, side, vertical_scale] =
            numbers.get(..HEAD).ok_or_else(bad)?.try_into().map_err(|_| bad())?;
        if !(2.0..=MAX_SIDE as f64).contains(&side) || side.fract() != 0.0 {
            return Err(bad());
        }
        let side = side as usize;
        let nodes = numbers.get(HEAD..).ok_or_else(bad)?;
        if nodes.len() != side * side * 6 || !numbers.iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
            return Err(bad());
        }
        let (mut base, mut up) = (Vec::new(), Vec::new());
        for n in nodes.as_chunks::<6>().0 {
            base.push(DVec3::new(n[0], n[1], n[2]));
            up.push(DVec3::new(n[3], n[4], n[5]));
        }
        Ok(Self {
            min: [x0, y0],
            max: [x1, y1],
            side,
            vertical_scale,
            base,
            up,
        })
    }

    /// The CRS positions of the nodes, row after row: what the reader computes `base` and `up` of
    #[must_use]
    pub fn nodes(min: [f64; 2], max: [f64; 2], side: usize) -> Vec<[f64; 2]> {
        let step = |i: usize, a: f64, b: f64| a + (b - a) * i as f64 / (side - 1) as f64;
        (0..side * side)
            .map(|k| [step(k % side, min[0], max[0]), step(k / side, min[1], max[1])])
            .collect()
    }

    /// The ECEF offset from the anchor of a point (CRS units); outside the square, the nearest cell carries on
    #[must_use]
    pub fn point(&self, x: f64, y: f64, z: f64) -> DVec3 {
        let last = (self.side - 1) as f64;
        let u = ((x - self.min[0]) / (self.max[0] - self.min[0]) * last).clamp(-1.0, last + 1.0);
        let v = ((y - self.min[1]) / (self.max[1] - self.min[1]) * last).clamp(-1.0, last + 1.0);
        // The cell, kept inside so that its four nodes exist; extrapolated past the edges
        let (i, j) = (u.floor().clamp(0.0, last - 1.0), v.floor().clamp(0.0, last - 1.0));
        let (s, t) = (u - i, v - j);
        let at = |di: usize, dj: usize| (j as usize + dj) * self.side + i as usize + di;
        let mix = |nodes: &[DVec3]| {
            let bottom = nodes[at(0, 0)].lerp(nodes[at(1, 0)], s);
            let top = nodes[at(0, 1)].lerp(nodes[at(1, 1)], s);
            bottom.lerp(top, t)
        };
        mix(&self.base) + mix(&self.up) * (z * self.vertical_scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::geodesy;

    /// A grid over a square of plate carrée degrees standing for a projected CRS: lon = x, lat = y
    fn degrees(min: [f64; 2], max: [f64; 2], side: usize, anchor: DVec3) -> Grid {
        let nodes = Grid::nodes(min, max, side);
        Grid {
            min,
            max,
            side,
            vertical_scale: 1.0,
            base: nodes
                .iter()
                .map(|&[x, y]| geodesy::to_ecef(x, y, 0.0) - anchor)
                .collect(),
            up: nodes.iter().map(|&[x, y]| geodesy::enu(x, y)[2]).collect(),
        }
    }

    #[test]
    fn interpolated_points_are_exact_to_the_millimetre() {
        // A square of about 1 km around Notre-Dame de Paris, 9 nodes a side
        let anchor = geodesy::to_ecef(2.35, 48.853, 0.0);
        let (min, max) = ([2.343, 48.849], [2.357, 48.858]);
        let grid = degrees(min, max, 9, anchor);
        for (k, z) in [(0, 0.0), (1, 35.0), (2, 120.0), (3, -20.0)] {
            let (x, y) = (2.3431 + 0.0037 * f64::from(k), 48.8491 + 0.0021 * f64::from(k));
            let exact = geodesy::to_ecef(x, y, z) - anchor;
            let error = (grid.point(x, y, z) - exact).length();
            assert!(error < 1e-3, "{error} m at {x}, {y}, {z}");
        }
        // The numbers handed over give back the grid
        assert_eq!(Grid::parse(&grid.numbers()).unwrap(), grid);
    }

    #[test]
    fn points_past_the_square_carry_on_from_its_edge() {
        let anchor = geodesy::to_ecef(2.35, 48.853, 0.0);
        let grid = degrees([2.343, 48.849], [2.357, 48.858], 3, anchor);
        let (x, y) = (2.3425, 48.8586);
        let error = (grid.point(x, y, 10.0) - (geodesy::to_ecef(x, y, 10.0) - anchor)).length();
        assert!(error < 0.05, "{error}");
    }

    #[test]
    fn malformed_numbers_are_errors() {
        let grid = degrees([0.0, 0.0], [1.0, 1.0], 2, DVec3::ZERO);
        let numbers = grid.numbers();
        assert!(Grid::parse(&numbers[..10]).is_err());
        let mut bad = numbers.clone();
        bad[4] = 1.0;
        assert!(Grid::parse(&bad).is_err());
        let mut bad = numbers.clone();
        bad[4] = 2.5;
        assert!(Grid::parse(&bad).is_err());
        let mut bad = numbers.clone();
        bad[9] = f64::NAN;
        assert!(Grid::parse(&bad).is_err());
        let mut bad = numbers;
        bad[2] = -1.0;
        assert!(Grid::parse(&bad).is_err());
    }
}
