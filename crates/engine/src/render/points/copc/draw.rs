//! The draws of a point cloud: its look (the colors and sizes of its style, the ranges of its file), its nodes ready
//! drawn with Eye-Dome Lighting when the style asks for it, the picking pass and the points it picked

use super::PointCloudLayer;
use crate::js;
use crate::layer::Frame;
use crate::render::points::{ColorBy, Draw, Edl, Look};
use crate::render::tiles3d::state::State;
use geo::camera::View;
use geo::geodesy;
use glam::DVec3;
use pointcloud::points::{INTENSITY, RETURNS};
use wasm_bindgen::prelude::*;

/// Ids of the picking pass: 24 bits, 0 for none
const MAX_PICK: u32 = (1 << 24) - 1;

impl PointCloudLayer {
    /// The look of the style: its colors (by RGB when the file has colors, else by class) and the range of its ramp
    fn look(&self) -> Look {
        let s = &self.style;
        let rgb = self.cloud.as_ref().is_some_and(|c| c.layout.format >= 7);
        let color_by = s
            .color_by
            .unwrap_or(if rgb { ColorBy::Rgb } else { ColorBy::Classification });
        let file = |range: [f64; 2], fallback: [f64; 2]| {
            if range.iter().all(|v| v.is_finite()) {
                range
            } else {
                fallback
            }
        };
        let range = match color_by {
            ColorBy::Elevation => {
                let z = self.cloud.as_ref().map_or([0.0, 100.0], |c| {
                    let h = &c.copc.header;

                    [h.min[2] * c.crs.vertical, h.max[2] * c.crs.vertical]
                });

                file(s.elevation, z)
            }
            ColorBy::Intensity => file(s.intensity, self.auto_intensity.unwrap_or([0.0, 65535.0])),
            _ => [0.0, 1.0],
        };

        Look {
            color_by,
            size: s.size,
            max_size: s.max_size,
            adaptive: s.adaptive,
            round: s.round,
            range,
            classes: true,
            lit: false,
        }
    }

    /// The nodes selected and ready, and their draws
    fn draws<'a>(&'a self, view: &View) -> (Vec<usize>, Vec<Draw<'a>>) {
        let Some(cloud) = &self.cloud else {
            return (Vec::new(), Vec::new());
        };
        let info = &cloud.hierarchy.info;

        self.selection
            .draw
            .iter()
            .filter_map(|&(i, level)| {
                let Some(State::Ready(node)) = self.states.get(i) else {
                    return None;
                };
                let lat = cloud.hierarchy.nodes[i].geodetic.y;
                let spacing = info.spacing * cloud.crs.metres / f64::from(1u32 << level.min(31));

                Some((
                    i,
                    Draw {
                        points: &node.points,
                        matrix: view.view_proj * view.ecef_matrix(node.anchor),
                        up: node.up,
                        base: node.base,
                        spacing: spacing * view.pixels_per_metre(lat) * view.center_distance,
                    },
                ))
            })
            .unzip()
    }

    /// Draws the nodes ready, into the EDL framebuffer then over the frame when the style asks for EDL
    pub(super) fn render(&mut self, frame: &Frame) {
        let (gl, view) = (frame.gl, frame.view);
        let look = self.look();
        let (strength, radius) = self.style.edl;

        if strength > 0.0 && self.edl.is_none() {
            match Edl::new(gl) {
                Ok(edl) => self.edl = Some(edl),
                Err(e) => {
                    self.fail(&format!("Eye-Dome Lighting: {e}"));
                    self.style.edl.0 = 0.0;
                }
            }
        }

        // With EDL, into its framebuffer at the size of the canvas, then laid over the frame
        let edl = strength > 0.0 && self.edl.is_some();

        if edl && let Some(pass) = self.edl.as_mut() {
            let size = (
                (frame.camera.width * frame.pixel_ratio).round() as i32,
                (frame.camera.height * frame.pixel_ratio).round() as i32,
            );

            pass.begin(gl, size);
        }

        let (_, draws) = self.draws(view);

        self.program.draw(gl, frame.pixel_ratio, &look, &draws, None);

        if edl && let Some(pass) = &self.edl {
            pass.finish(gl, view, strength, radius * frame.pixel_ratio);
        }
    }

    /// The picking pass: each point an id of its own, the nodes numbered in turn, as many as 24 bits hold
    pub(super) fn draw_picking(&self, frame: &Frame, layer: u8) {
        let (nodes, draws) = self.draws(frame.view);
        let mut picked = Vec::new();
        let mut first = 1u32;

        for (&i, d) in nodes.iter().zip(&draws) {
            let count = d.points.count as u32;

            if first.saturating_add(count) > MAX_PICK {
                break;
            }
            picked.push((i, first - 1));
            first += count;
        }

        let firsts: Vec<u32> = picked.iter().map(|&(_, f)| f).collect();

        self.program.draw(
            frame.gl,
            frame.pixel_ratio,
            &self.look(),
            &draws[..firsts.len()],
            Some((layer, &firsts)),
        );
        *self.picked.borrow_mut() = picked;
    }

    /// The point of a picked id: its node and index there
    fn picked_point(&self, index: u32) -> Option<(usize, u32)> {
        let picked = self.picked.borrow();
        let k = picked.partition_point(|&(_, first)| first <= index);
        let &(node, first) = picked.get(k.checked_sub(1)?)?;

        Some((node, index - first))
    }

    /// The attributes of a picked point, read back from the GPU
    pub(super) fn point(&self, gl: &glow::Context, index: u32) -> Option<JsValue> {
        let (node, i) = self.picked_point(index)?;
        let cloud = self.cloud.as_ref()?;
        let Some(State::Ready(n)) = self.states.get(node) else {
            return None;
        };
        let v = n.points.read(gl, i)?;
        let f = |at: usize| f64::from(f32::from_le_bytes([v[at], v[at + 1], v[at + 2], v[at + 3]]));
        let offset = DVec3::new(f(0), f(4), f(8));
        let g = geodesy::from_ecef(n.anchor + offset);
        let mut entries = vec![
            ("lonLat", js::pair(g.x, g.y)),
            ("height", g.z.into()),
            ("altitude", (n.base + offset.dot(n.up)).into()),
            ("classification", v[15].into()),
            ("intensity", u16::from_le_bytes([v[INTENSITY], v[INTENSITY + 1]]).into()),
            ("returnNumber", v[RETURNS].into()),
            ("numberOfReturns", v[RETURNS + 1].into()),
        ];

        if cloud.layout.format >= 7 {
            let rgb = js_sys::Array::of3(&v[12].into(), &v[13].into(), &v[14].into());

            entries.push(("rgb", rgb.into()));
        }

        Some(js::object(&entries))
    }
}
