//! From the page's arrays to what a symbol layer uploads: the labels, the table of their colors and the quads of
//! their glyphs and icons, whatever the lengths of the arrays.

use super::{LABEL_FLOATS, Label, QUAD_FLOATS, Quad, TEXELS};
use crate::render::features::stops_of;
use crate::render::gl::{self, TABLE_WIDTH as WIDTH};
use crate::render::overlay;
use glam::DVec2;

/// Labels as `SymbolLayer::set_labels` takes them, ready to upload
pub(super) struct Built {
    /// The labels, as many as the anchors, boxes and styles all describe
    pub(super) labels: Vec<Label>,
    /// Per label: offset, fill, halo, halo width, icon color (`gl::table_rows` rows of RGBA32F texels)
    pub(super) table: Vec<f32>,
    /// The glyph and icon quads of these labels
    pub(super) quads: Vec<Quad>,
    /// Bounds of the anchors (Mercator)
    pub(super) min: DVec2,
    /// Bounds of the anchors (Mercator)
    pub(super) max: DVec2,
    /// Middle of the bounds, the origin of the offsets in the table
    pub(super) origin: DVec2,
}

/// The labels of `SymbolLayer::set_labels`, from arrays of any lengths (as many labels as all three describe) and
/// quads of any label numbers (those of no label left out)
pub(super) fn build(anchors: &[f64], boxes: &[f32], styles: &[f32], quads: &[f32]) -> Built {
    let n = (anchors.len() / 2)
        .min(boxes.len() / 4)
        .min(styles.len() / LABEL_FLOATS);
    let mut merc = overlay::points(anchors);

    merc.truncate(n);

    let (min, max) = overlay::bounds(&merc).unwrap_or((DVec2::ZERO, DVec2::ZERO));
    let origin = (min + max) / 2.0;
    let labels = merc
        .iter()
        .enumerate()
        .map(|(i, &m)| {
            let s = &styles[i * LABEL_FLOATS..(i + 1) * LABEL_FLOATS];

            Label {
                merc: m,
                rect: [boxes[i * 4], boxes[i * 4 + 1], boxes[i * 4 + 2], boxes[i * 4 + 3]],
                icon: [s[20], s[21], s[22], s[23]],
                // A flag that is not a number is none
                flags: s[9].clamp(0.0, 65535.0) as u32,
                direction: DVec2::new(f64::from(s[10]), f64::from(s[11])),
                size: stops_of(&s[12..20]).unwrap_or([[0.0; 4], [1.0; 4]]),
                icon_size: stops_of(&s[28..36]).unwrap_or([[0.0; 4], [1.0; 4]]),
            }
        })
        .collect();
    // Per label: offset, fill, halo, halo width, icon color
    let rows = gl::table_rows(n * TEXELS);
    let mut table = vec![0f32; rows * WIDTH * 4];

    for (i, m) in merc.iter().enumerate() {
        let o = (*m - origin).as_vec2();
        let s = &styles[i * LABEL_FLOATS..(i + 1) * LABEL_FLOATS];
        let t = i * TEXELS * 4;

        table[t..t + 2].copy_from_slice(&[o.x, o.y]);
        table[t + 4..t + 12].copy_from_slice(&s[..8]);
        table[t + 12] = s[8];
        table[t + 16..t + 20].copy_from_slice(&s[24..28]);
    }

    let data: Vec<Quad> = quads
        .as_chunks::<QUAD_FLOATS>()
        .0
        .iter()
        .filter(|q| (q[0] as usize) < n)
        .map(|q| Quad {
            label: q[0] as u32,
            rect: [q[1], q[2], q[3], q[4]],
            uv: [q[5], q[6], q[7], q[8]],
            scale: q[9],
            angle: q[10],
        })
        .collect();

    Built {
        labels,
        table,
        quads: data,
        min,
        max,
        origin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::symbols::flag;

    #[test]
    fn labels_read_their_icons() {
        let mut style = [0f32; LABEL_FLOATS];

        style[9] = (flag::TEXT | flag::ICON | flag::ICON_ALONG) as f32;
        style[20..24].copy_from_slice(&[-6.0, -6.0, 6.0, 6.0]);
        style[24..28].copy_from_slice(&[0.2, 0.4, 0.6, 0.5]);
        style[28..36].copy_from_slice(&[0.0, 10.0, 20.0, 30.0, 1.0, 1.0, 2.0, 2.0]);

        let mut quad = [0f32; QUAD_FLOATS];

        quad[9] = -2.0;
        quad[10] = 1.5;

        let built = build(&[0.5, 0.5], &[0.0; 4], &style, &quad);
        let label = &built.labels[0];

        assert_eq!(label.flags, flag::TEXT | flag::ICON | flag::ICON_ALONG);
        assert_eq!(label.icon, [-6.0, -6.0, 6.0, 6.0]);
        assert_eq!(label.icon_size, [[0.0, 10.0, 20.0, 30.0], [1.0, 1.0, 2.0, 2.0]]);
        // The icon color in the fifth texel of the label
        assert_eq!(&built.table[16..20], &[0.2, 0.4, 0.6, 0.5]);
        assert_eq!((built.quads[0].scale, built.quads[0].angle), (-2.0, 1.5));
        // Flags that are not a number, or negative: none
        style[9] = f32::NAN;
        assert_eq!(build(&[0.5, 0.5], &[0.0; 4], &style, &[]).labels[0].flags, 0);
        style[9] = -3.0;
        assert_eq!(build(&[0.5, 0.5], &[0.0; 4], &style, &[]).labels[0].flags, 0);
    }

    #[test]
    fn malformed_labels_do_not_panic() {
        let style = [1.0f32; LABEL_FLOATS];
        // Two anchors (one not a number), one box, two styles: one label
        let built = build(
            &[0.5, 0.5, f64::NAN, 0.2],
            &[0.0, 0.0, 10.0, 5.0, 1.0],
            &[style, style].concat(),
            &[],
        );

        assert_eq!(built.labels.len(), 1);
        assert_eq!(built.table.len(), gl::table_rows(TEXELS) * WIDTH * 4);
        assert!(built.origin.is_finite());

        // Quads of no label (past it, negative, not a number: 0 as usize, the first) and a partial one
        let mut quads = vec![0f32; QUAD_FLOATS * 4 + 3];

        quads[QUAD_FLOATS] = 1.0;
        quads[QUAD_FLOATS * 2] = -5.0;
        quads[QUAD_FLOATS * 3] = f32::NAN;

        let built = build(&[0.5, 0.5], &[0.0; 4], &style, &quads);

        assert_eq!(built.quads.len(), 3);
        assert!(built.quads.iter().all(|q| q.label == 0));

        let none = build(&[], &[], &[], &quads);

        assert!(none.labels.is_empty() && none.quads.is_empty());
    }
}
