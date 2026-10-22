//! The style of a point cloud as the page sets it (`setPointCloudStyle`): its numbers parsed, out of range ones to
//! the defaults

use crate::render::points::ColorBy;

/// The style of a point cloud, from the page (`setPointCloudStyle`)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// None: by RGB when the file has colors, else by class
    pub color_by: Option<ColorBy>,
    /// CSS pixels: the size, or the smallest with `adaptive`
    pub size: f64,
    /// CSS pixels: the largest with `adaptive`
    pub max_size: f64,
    /// Sizes following the spacing of the points (from `size` to `max_size`), else fixed at `size`
    pub adaptive: bool,
    /// Discs rather than squares
    pub round: bool,
    /// Eye-Dome Lighting: strength (0: off) and radius (CSS pixels)
    pub edl: (f64, f64),
    /// Altitudes and intensities mapped onto the ramp: NaN for the file's (the header's z, the root's intensities)
    pub elevation: [f64; 2],
    /// Intensities mapped onto the ramp, as `elevation`
    pub intensity: [f64; 2],
    /// Spacing of the points (CSS pixels) above which a node is refined
    pub max_error: f64,
    /// Points selected at most; the GPU memory kept follows it (`gpu_budget`)
    pub budget: u64,
    /// Requests a second at most, 0 for no limit
    pub rate: f64,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color_by: None,
            size: 2.0,
            max_size: 4.0,
            adaptive: true,
            round: true,
            edl: (0.0, 1.4),
            elevation: [f64::NAN; 2],
            intensity: [f64::NAN; 2],
            max_error: 1.5,
            budget: 3_000_000,
            rate: 0.0,
        }
    }
}

impl Style {
    /// The style of the page's numbers: color by (-1 auto, then `ColorBy`), size, max size, adaptive (0, 1), round
    /// (0, 1), EDL strength and radius, elevation min and max, intensity min and max (NaN: the file's), max screen
    /// space error, point budget, requests a second; a number missing or out of its range keeps the default
    #[must_use]
    pub fn parse(n: &[f64]) -> Self {
        let d = Self::default();
        let at = |i: usize| n.get(i).copied().unwrap_or(f64::NAN);
        let positive = |i: usize, default: f64| Some(at(i)).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(default);

        Self {
            color_by: Some(at(0)).filter(|v| *v >= 0.0).map(ColorBy::from_number),
            size: positive(1, d.size).min(64.0),
            max_size: positive(2, d.max_size).min(64.0),
            adaptive: at(3) != 0.0,
            round: at(4) != 0.0,
            edl: (
                Some(at(5)).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0),
                positive(6, d.edl.1).min(8.0),
            ),
            elevation: [at(7), at(8)],
            intensity: [at(9), at(10)],
            max_error: positive(11, d.max_error),
            budget: positive(12, d.budget as f64).min(1e9) as u64,
            rate: Some(at(13)).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_take_the_page_s_numbers() {
        let s = Style::parse(&[
            2.0,
            3.0,
            10.0,
            0.0,
            1.0,
            1.5,
            2.0,
            30.0,
            90.0,
            f64::NAN,
            f64::NAN,
            4.0,
            1e6,
            8.0,
        ]);

        assert_eq!(s.color_by, Some(ColorBy::Elevation));
        assert_eq!((s.size, s.max_size, s.adaptive, s.round), (3.0, 10.0, false, true));
        assert_eq!(s.edl, (1.5, 2.0));
        assert_eq!(s.elevation, [30.0, 90.0]);
        assert!(s.intensity[0].is_nan());
        assert_eq!((s.max_error, s.budget, s.rate), (4.0, 1_000_000, 8.0));

        // Nothing given, or out of range: the defaults
        let d = Style::parse(&[
            -1.0,
            -2.0,
            f64::NAN,
            1.0,
            1.0,
            -1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            -5.0,
            -1.0,
        ]);

        assert_eq!(d.color_by, None);
        assert_eq!(
            (d.size, d.max_size, d.edl, d.max_error, d.budget, d.rate),
            (2.0, 4.0, (0.0, 1.4), 1.5, 3_000_000, 0.0)
        );
        assert_eq!(Style::parse(&[]).budget, 3_000_000);
    }
}
