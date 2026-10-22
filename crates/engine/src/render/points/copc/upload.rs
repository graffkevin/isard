//! What arrives for a point cloud (the head, hierarchy pages, nodes' vertices from the workers) taken in each frame,
//! and the vertices put on the GPU a few at a time, where their request anchored them

use super::{Arrival, NodePoints, PointCloudLayer};
use crate::js;
use crate::render::points::GpuPoints;
use crate::render::tiles3d::state::{COOLDOWN_MS, State};
use pointcloud::points::{INTENSITY, STRIDE};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The 2nd and 98th percentiles of the intensities of vertices (to 16 units): the range the intensities are mapped on
#[must_use]
fn intensity_range(vertices: &[u8]) -> Option<[f64; 2]> {
    let mut histogram = vec![0u32; 4096];
    let mut count = 0u32;

    for v in vertices.as_chunks::<STRIDE>().0 {
        histogram[usize::from(u16::from_le_bytes([v[INTENSITY], v[INTENSITY + 1]]) >> 4)] += 1;
        count += 1;
    }
    if count == 0 {
        return None;
    }

    let at = |q: f64| {
        let target = (f64::from(count - 1) * q).round() as u32;
        let mut seen = 0;
        let bin = histogram.iter().position(|&n| {
            seen += n;

            seen > target
        });

        f64::from(bin.unwrap_or(0) as u32 * 16)
    };
    let (lo, hi) = (at(0.02), at(0.98));

    Some([lo, if hi > lo { hi } else { lo + 16.0 }])
}

impl PointCloudLayer {
    /// Takes in what arrived (the head, hierarchy pages, nodes' points), then uploads the nodes queued within
    /// `budget_ms` (at least one)
    pub(super) fn receive(
        &mut self,
        gl: &glow::Context,
        raw: &web_sys::WebGl2RenderingContext,
        frame: u64,
        budget_ms: f64,
    ) {
        let now = js_sys::Date::now();
        let arrivals: Vec<_> = self.inbox.borrow_mut().drain(..).collect();

        for arrival in arrivals {
            match arrival {
                Arrival::Head(bytes) => self.receive_head(bytes),
                Arrival::Page(node, bytes) => self.receive_page(node, bytes, now),
                Arrival::Points(node, id, result) if self.states.is_current(node, id) => match result {
                    Ok(vertices) => self.queue.push_back((node, vertices)),
                    Err(_) => {
                        self.states.replace(node, State::Failed(now + COOLDOWN_MS));
                    }
                },
                Arrival::Points(..) => {}
            }
        }
        self.upload_queued(gl, raw, frame, budget_ms, now);
    }

    /// Uploads the queued nodes in arrival order for `budget_ms` at most (at least one): those no longer loading are
    /// dropped
    fn upload_queued(
        &mut self,
        gl: &glow::Context,
        raw: &web_sys::WebGl2RenderingContext,
        frame: u64,
        budget_ms: f64,
        now: f64,
    ) {
        let start = js::now();
        let mut uploaded = 0;

        while uploaded == 0 || js::now() - start < budget_ms {
            let Some((node, vertices)) = self.queue.pop_front() else {
                break;
            };

            uploaded += 1;
            if matches!(self.states.get(node), Some(State::Loading(_))) {
                self.upload_node(gl, raw, node, vertices, frame, now);
            }
        }
    }

    /// Puts a node's vertices (a `Uint8Array` from the workers) on the GPU, where `request` anchored it; a failure
    /// waits for the cooldown (`now`, ms)
    fn upload_node(
        &mut self,
        gl: &glow::Context,
        raw: &web_sys::WebGl2RenderingContext,
        node: usize,
        vertices: JsValue,
        frame: u64,
        now: f64,
    ) {
        let Ok(array) = vertices.dyn_into::<js_sys::Uint8Array>() else {
            self.states.replace(node, State::Failed(now + COOLDOWN_MS));

            return;
        };

        if self.auto_intensity.is_none() {
            self.auto_intensity = intensity_range(&array.to_vec());
        }

        let Some(&(anchor, up, base)) = self.anchors.get(&node) else {
            return;
        };

        match GpuPoints::upload_js(gl, raw, &array) {
            Ok(points) => {
                self.states.replace(
                    node,
                    State::Ready(NodePoints {
                        points,
                        anchor,
                        up,
                        base,
                        used: frame,
                    }),
                );
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("Point cloud upload: {e:?}")));
                self.states.replace(node, State::Failed(now + COOLDOWN_MS));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensities_range_over_their_bulk() {
        let mut v = Vec::new();

        for i in 0..1000u16 {
            let mut vertex = [0u8; STRIDE];

            vertex[INTENSITY..INTENSITY + 2].copy_from_slice(&(if i == 999 { 60_000 } else { i }).to_le_bytes());
            v.extend(vertex);
        }

        let [lo, hi] = intensity_range(&v).unwrap();

        assert!((lo - 16.0).abs() < 1.0 && (hi - 976.0).abs() < 1.0, "{lo} {hi}");
        assert_eq!(intensity_range(&[]), None);
        assert_eq!(intensity_range(&[0; STRIDE]), Some([0.0, 16.0]));
    }
}
