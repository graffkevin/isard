//! The placement of a symbol layer's labels each frame: where each anchor lands on screen, how its text and icon
//! turn along their line and scale with the zoom, and which parts fit in the collision grid shared by the symbol
//! layers (`MapLibre`'s overlap and optional rules); an unchanged placement takes its boxes again without working
//! anything out.

use super::{Label, SymbolLayer, SymbolStyle, flag};
use crate::layer::Frame;
use crate::render::features::stops;
use geo::collision;
use glam::{DVec2, DVec4};

/// What a placed label shows, and how its parts turn (the first number of its state, `SymbolLayer::visible`)
mod shown {
    /// The text shows
    pub const TEXT: u32 = 1;
    /// The icon shows
    pub const ICON: u32 = 2;
    /// The text turns by the angle of the state, the icon too (half a turn more with `FLIP`)
    pub const TURN_TEXT: u32 = 4;
    /// The icon turns by the angle of the state
    pub const TURN_ICON: u32 = 8;
    /// Half a turn more for the icon (its text kept upright the other way)
    pub const FLIP: u32 = 16;
}
/// Labels whose anchor is further than this out of the screen are not placed (CSS pixels)
const MARGIN: f64 = 100.0;

/// What a placement depends on: the camera (and the screen), the labels, the relief under them, the style, and the
/// collision grid as the layers above left it
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct PlaceKey {
    camera: [u64; 9],
    relief: (u64, u32),
    style: [u32; 8],
    grid: u64,
}

impl PlaceKey {
    /// The key of a placement of `style`'s labels in this frame, the grid as the layers above left it
    pub(super) fn new(frame: &Frame, style: &SymbolStyle, grid: &collision::Grid) -> Self {
        let c = frame.camera;

        Self {
            camera: [
                c.center.x,
                c.center.y,
                c.zoom,
                c.pitch,
                c.bearing,
                c.width,
                c.height,
                c.elevation,
                f64::from(u8::from(c.globe)),
            ]
            .map(f64::to_bits),
            relief: frame
                .relief
                .map_or((0, 0), |(source, e)| (source.generation(), e.to_bits())),
            style: [
                style.size[0][0],
                style.size[0][1],
                style.size[0][2],
                style.size[0][3],
                style.size[1][0],
                style.size[1][1],
                style.size[1][2],
                style.size[1][3],
            ]
            .map(f32::to_bits),
            grid: grid.fingerprint(),
        }
    }
}

/// The box of a label on screen around its anchor `at`: its rect (CSS pixels at the shaped size) scaled and turned by
/// `angle` (radians, clockwise on screen), as an axis-aligned box
fn label_box(at: DVec2, rect: [f32; 4], scale: f64, angle: f64) -> collision::Rect {
    let r = rect.map(|v| f64::from(v) * scale);
    let (cs, sn) = (angle.cos(), angle.sin());
    let (mut lo, mut hi) = (DVec2::INFINITY, DVec2::NEG_INFINITY);

    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let p = DVec2::new(x * cs - y * sn, x * sn + y * cs);

        lo = lo.min(p);
        hi = hi.max(p);
    }

    [at.x + lo.x, at.y + lo.y, at.x + hi.x, at.y + hi.y]
}

/// Angle on screen of a line from `at` to `ahead` (radians, clockwise)
fn angle_of(at: DVec2, ahead: DVec2) -> f64 {
    let d = ahead - at;

    d.y.atan2(d.x)
}

/// An angle turned half a turn if upside down
fn keep_upright(mut angle: f64) -> f64 {
    if angle > std::f64::consts::FRAC_PI_2 {
        angle -= std::f64::consts::PI;
    } else if angle < -std::f64::consts::FRAC_PI_2 {
        angle += std::f64::consts::PI;
    }

    angle
}

/// Places a label whose anchor is at `at` on screen, its text and icon at their `scales`, `line` the angle of its line
/// on screen (radians, clockwise) if it follows one: its boxes taken in the grid (and pushed to `boxes`), and its state
/// if it shows (see `SymbolLayer::visible`)
fn place_label(
    label: &Label,
    at: DVec2,
    (scale, icon_scale): (f64, f64),
    line: Option<f64>,
    grid: &mut collision::Grid,
    boxes: &mut Vec<collision::Rect>,
) -> Option<[f32; 4]> {
    let f = label.flags;
    // The text kept upright along its line, the icon turned with it
    let raw = line.unwrap_or(0.0);
    let angle = keep_upright(raw);
    let mut bits = 0;
    let text_angle = if f & flag::TEXT_ALONG != 0 {
        bits |= shown::TURN_TEXT;

        angle
    } else {
        0.0
    };
    let icon_angle = if f & flag::ICON_ALONG != 0 {
        bits |= shown::TURN_ICON;

        // Upside down: half a turn more than the upright angle
        if f & flag::ICON_UPRIGHT != 0 || raw.abs() <= std::f64::consts::FRAC_PI_2 {
            angle
        } else {
            bits |= shown::FLIP;

            raw
        }
    } else {
        0.0
    };
    let text_box = label_box(at, label.rect, scale, text_angle);
    let icon_box = label_box(at, label.icon, icon_scale, icon_angle);
    // A part shrunk to nothing at this zoom is none
    let mut parts = f;

    if scale <= 0.0 {
        parts &= !flag::TEXT;
    }
    if icon_scale <= 0.0 {
        parts &= !flag::ICON;
    }

    let (text, icon) = decide(
        parts,
        f & flag::TEXT_ALLOW_OVERLAP != 0 || !grid.collides(&text_box),
        f & flag::ICON_ALLOW_OVERLAP != 0 || !grid.collides(&icon_box),
    );

    for (part, ignore, r) in [
        (text, flag::TEXT_IGNORE_PLACEMENT, text_box),
        (icon, flag::ICON_IGNORE_PLACEMENT, icon_box),
    ] {
        if part && f & ignore == 0 {
            grid.insert(r);
            boxes.push(r);
        }
    }

    (text || icon).then(|| {
        bits |= (u32::from(text) * shown::TEXT) | (u32::from(icon) * shown::ICON);

        [bits as f32, angle as f32, scale as f32, icon_scale as f32]
    })
}

/// Whether a label's text and icon show, from its flags and whether each fits where it is (overlapping nothing taken,
/// or allowed to overlap): `MapLibre`'s rule, a part that is not optional showing only with the other
fn decide(flags: u32, text_fits: bool, icon_fits: bool) -> (bool, bool) {
    let has_text = flags & flag::TEXT != 0;
    let has_icon = flags & flag::ICON != 0;
    let (mut text, mut icon) = (has_text && text_fits, has_icon && icon_fits);
    let icon_alone = flags & flag::TEXT_OPTIONAL != 0 || !has_text;
    let text_alone = flags & flag::ICON_OPTIONAL != 0 || !has_icon;

    if !icon_alone && !text_alone {
        text &= icon;
        icon = text;
    } else if !text_alone {
        text &= icon;
    } else if !icon_alone {
        icon &= text;
    }

    (text, icon)
}

impl SymbolLayer {
    /// Out of its zoom range or hidden: no label shows, nothing placed to take again
    pub(super) fn hide_all(&mut self) {
        self.placed = None;
        if self.visible.iter().any(|&v| v != 0.0) {
            self.visible.fill(0.0);
            self.uploaded = false;
        }
    }

    /// Places each label near enough the screen in its order, into `visible` (cleared before) and `grid`, the boxes
    /// it takes added to `boxes`
    pub(super) fn place_labels(&mut self, frame: &Frame, grid: &mut collision::Grid, boxes: &mut Vec<collision::Rect>) {
        let (camera, view) = (frame.camera, frame.view);
        let layer_scale = stops(&self.style.size, camera.zoom);

        for (label, state) in self.labels.iter().zip(self.visible.as_chunks_mut::<4>().0) {
            let altitude = frame
                .relief
                .and_then(|(source, e)| source.altitude(label.merc).map(|h| h * f64::from(e)))
                .unwrap_or(0.0);
            let Some((at, distance)) = screen_point(frame, label.merc, altitude) else {
                continue;
            };

            if at.x < -MARGIN || at.y < -MARGIN || at.x > camera.width + MARGIN || at.y > camera.height + MARGIN {
                continue;
            }

            // Smaller further away, as MapLibre's perspective ratio: a tilted view keeps its far labels sparse
            let perspective = (0.5 + 0.5 * view.center_distance / distance).clamp(0.5, 1.5);
            let scale = layer_scale * stops(&label.size, camera.zoom) * perspective;
            let icon_scale = layer_scale * stops(&label.icon_size, camera.zoom) * perspective;

            if scale <= 0.0 && icon_scale <= 0.0 {
                continue;
            }

            // The angle of its line on screen
            let mut line = None;

            if label.direction != DVec2::ZERO && label.flags & (flag::TEXT_ALONG | flag::ICON_ALONG) != 0 {
                // A step of one pixel along the line
                let step = label.direction.normalize() / view.world_size;

                line = screen_point(frame, label.merc + step, altitude).map(|(ahead, _)| angle_of(at, ahead));
            }

            if let Some(placed) = place_label(label, at, (scale, icon_scale), line, grid, boxes) {
                *state = placed;
            }
        }
    }
}

/// Screen point (CSS pixels) of a Mercator point at `altitude` (metres) and its distance to the eye (pixels of the
/// local frame); none past the horizon of the globe, hidden by the body (as `Engine::project`), or behind the camera
fn screen_point(frame: &Frame, m: DVec2, altitude: f64) -> Option<(DVec2, f64)> {
    let (camera, view) = (frame.camera, frame.view);
    let shift = -(m.x - camera.center.x).round();
    let ground = view.position(m, shift);

    if view.globe.factor > 0.0 && !view.faces_camera(ground) {
        return None;
    }

    let p = ground + glam::DVec3::Z * altitude * view.vertical;
    let clip = view.view_proj * DVec4::new(p.x, p.y, p.z, 1.0);

    (clip.w > 0.0)
        .then(|| {
            let at = DVec2::new(
                f64::midpoint(clip.x / clip.w, 1.0) * camera.width,
                f64::midpoint(-clip.y / clip.w, 1.0) * camera.height,
            );

            (at, clip.w)
        })
        // An anchor that is not a number is nowhere
        .filter(|(at, _)| at.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    #[test]
    fn label_boxes_turn_with_the_text() {
        let at = DVec2::new(100.0, 50.0);
        let rect = [-20.0, -5.0, 20.0, 5.0];
        let flat = label_box(at, rect, 1.0, 0.0);

        assert_eq!(flat, [80.0, 45.0, 120.0, 55.0]);
        // Twice as large
        assert_eq!(label_box(at, rect, 2.0, 0.0), [60.0, 40.0, 140.0, 60.0]);

        // A quarter turn: the box stands up
        let turned = label_box(at, rect, 1.0, FRAC_PI_2);

        for (a, b) in turned.iter().zip([95.0, 30.0, 105.0, 70.0]) {
            assert!((a - b).abs() < 1e-9, "{turned:?}");
        }
    }

    #[test]
    fn text_along_a_line_stays_upright() {
        let upright = |at, ahead| keep_upright(angle_of(at, ahead));
        let o = DVec2::ZERO;

        assert_eq!(upright(o, DVec2::new(1.0, 0.0)), 0.0);
        // Running right to left: turned half a turn, read left to right
        assert!(upright(o, DVec2::new(-1.0, 0.0)).abs() < 1e-12);
        assert!((upright(o, DVec2::new(1.0, 1.0)) - PI / 4.0).abs() < 1e-12);
        assert!((upright(o, DVec2::new(-1.0, -1.0)) - PI / 4.0).abs() < 1e-12);
        assert!(upright(o, DVec2::new(0.0, -1.0)).abs() <= FRAC_PI_2);
    }

    #[test]
    fn text_and_icon_show_together_unless_optional() {
        use flag::{ICON, ICON_OPTIONAL, TEXT, TEXT_OPTIONAL};
        let both = TEXT | ICON;

        // Neither optional: both or nothing
        assert_eq!(decide(both, true, true), (true, true));
        assert_eq!(decide(both, true, false), (false, false));
        assert_eq!(decide(both, false, true), (false, false));
        // The text optional: the icon alone where the text does not fit, never the text alone
        assert_eq!(decide(both | TEXT_OPTIONAL, false, true), (false, true));
        assert_eq!(decide(both | TEXT_OPTIONAL, true, false), (false, false));
        // The icon optional: the text alone
        assert_eq!(decide(both | ICON_OPTIONAL, true, false), (true, false));
        assert_eq!(decide(both | ICON_OPTIONAL, false, true), (false, false));

        // Both optional: each on its own
        let either = both | TEXT_OPTIONAL | ICON_OPTIONAL;

        assert_eq!(decide(either, false, true), (false, true));
        assert_eq!(decide(either, true, false), (true, false));
        // Only a text, only an icon
        assert_eq!(decide(TEXT, true, false), (true, false));
        assert_eq!(decide(ICON, false, true), (false, true));
        assert_eq!(decide(ICON, true, false), (false, false));
        assert_eq!(decide(0, true, true), (false, false));
    }

    #[test]
    fn a_label_and_its_icon_collide_as_two_boxes() {
        // A point of interest: its icon at the anchor, its text below; another label's text over the first's text
        let mut grid = collision::Grid::new(800.0, 600.0);
        let at = DVec2::new(400.0, 300.0);
        let icon = label_box(at, [-8.0, -8.0, 8.0, 8.0], 1.0, 0.0);
        let text = label_box(at, [-30.0, 10.0, 30.0, 24.0], 1.0, 0.0);
        let (t, i) = decide(flag::TEXT | flag::ICON, !grid.collides(&text), !grid.collides(&icon));

        assert!(t && i);
        grid.insert(text);
        grid.insert(icon);

        // Its text over the first text, its icon clear of both: with an optional text, its icon alone shows
        let other = DVec2::new(410.0, 345.0);
        let icon2 = label_box(other, [-8.0, -8.0, 8.0, 8.0], 1.0, 0.0);
        let text2 = label_box(other, [-30.0, -30.0, 30.0, -16.0], 1.0, 0.0);

        assert!(grid.collides(&text2) && !grid.collides(&icon2));

        let flags = flag::TEXT | flag::ICON | flag::TEXT_OPTIONAL;

        assert_eq!(decide(flags, false, true), (false, true));
        // A box between the icon and the text of the first label is free: they are not one box
        assert!(!grid.collides(&[440.0, 290.0, 450.0, 300.0]));

        // An icon twice as large, turned with its line: its box scales and turns
        let turned = label_box(at, [-10.0, -2.0, 10.0, 2.0], 2.0, FRAC_PI_2);

        for (a, b) in turned.iter().zip([396.0, 280.0, 404.0, 320.0]) {
            assert!((a - b).abs() < 1e-9, "{turned:?}");
        }
    }

    #[test]
    fn icons_turn_with_their_line_and_texts_stay_upright() {
        let label = |flags| Label {
            merc: DVec2::ZERO,
            rect: [-20.0, -5.0, 20.0, 5.0],
            icon: [-4.0, -3.0, 4.0, 3.0],
            flags,
            direction: DVec2::X,
            size: [[0.0; 4], [1.0; 4]],
            icon_size: [[0.0; 4], [1.0; 4]],
        };
        let at = DVec2::new(100.0, 100.0);
        let mut boxes = Vec::new();
        // A one-way arrow on a line running down to the left: the arrow points along it, flipped from upright
        let mut grid = collision::Grid::new(800.0, 600.0);
        let arrow = label(flag::ICON | flag::ICON_ALONG);
        let line = 0.75 * PI;
        let state = place_label(&arrow, at, (1.0, 1.0), Some(line), &mut grid, &mut boxes).unwrap();
        let bits = state[0] as u32;

        assert_eq!(bits, shown::ICON | shown::TURN_ICON | shown::FLIP);
        assert!((f64::from(state[1]) + PI / 4.0).abs() < 1e-6);

        // A shield stretched around its text turns with it, upright
        let mut grid = collision::Grid::new(800.0, 600.0);
        let shield = label(flag::TEXT | flag::ICON | flag::TEXT_ALONG | flag::ICON_ALONG | flag::ICON_UPRIGHT);
        let state = place_label(&shield, at, (1.0, 1.0), Some(line), &mut grid, &mut boxes).unwrap();

        assert_eq!(
            state[0] as u32,
            shown::TEXT | shown::ICON | shown::TURN_TEXT | shown::TURN_ICON
        );

        // A horizontal label: nothing turns; its icon ignoring the placement takes no room, its text does
        let mut grid = collision::Grid::new(800.0, 600.0);

        boxes.clear();

        let poi = label(flag::TEXT | flag::ICON | flag::ICON_IGNORE_PLACEMENT);
        let state = place_label(&poi, at, (1.0, 2.0), None, &mut grid, &mut boxes).unwrap();

        assert_eq!(state, [(shown::TEXT | shown::ICON) as f32, 0.0, 1.0, 2.0]);
        assert_eq!(boxes, [[80.0, 95.0, 120.0, 105.0]]);
        // The same label again over it: its text does not fit, and its icon needs its text
        assert!(place_label(&poi, at, (1.0, 2.0), None, &mut grid, &mut boxes).is_none());

        // Shrunk to nothing, the text is none: the icon shows alone
        let mut grid = collision::Grid::new(800.0, 600.0);
        let state = place_label(&poi, at, (0.0, 2.0), None, &mut grid, &mut boxes).unwrap();

        assert_eq!(state[0] as u32, shown::ICON);
    }
}
