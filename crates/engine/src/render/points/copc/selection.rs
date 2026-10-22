//! The nodes of a frame: those selected for the view (`pointcloud::traversal`), the requests of the others aborted,
//! the GPU memory kept within its budget (least recently drawn unloaded first), the counters of the frame

use super::{PointCloudLayer, PointCloudStats};
use crate::layer::Frame;
use crate::render::tiles3d::state::State;
use geo::camera::View;
use pointcloud::points::STRIDE;
use pointcloud::traversal::{Readiness, select};

impl PointCloudLayer {
    /// Selects the nodes to draw and to load for `view` (`pointcloud::traversal::select`), and aborts the requests
    /// of the nodes no longer in view
    pub(super) fn select_nodes(&mut self, view: &View, now: f64) {
        let Some(cloud) = &self.cloud else {
            return;
        };
        let states = &self.states;
        let ready = |i: usize| match states.get(i) {
            Some(State::Ready(_)) => Readiness::Ready,
            Some(State::Failed(until)) if now < *until => Readiness::Unavailable,
            _ => Readiness::Pending,
        };
        let mut selection = std::mem::take(&mut self.selection);

        select(
            &cloud.hierarchy,
            view,
            self.style.max_error,
            self.style.budget,
            &ready,
            &mut selection,
        );

        let mut stale = Vec::new();

        self.states.take_stale(&selection.visible, &mut stale);
        for loading in &stale {
            self.abort(loading);
        }
        self.selection = selection;
    }

    /// Marks the nodes drawn this frame, then unloads the least recently drawn ones past the GPU `budget` (bytes)
    pub(super) fn unload_over_budget(&mut self, frame: &Frame, budget: usize) {
        let drawn: Vec<usize> = self.selection.draw.iter().map(|&(i, _)| i).collect();

        self.states.touch(&drawn, frame.number);
        for i in self.states.over_budget(budget, frame.number) {
            if let Some(State::Ready(node)) = self.states.replace(i, State::Unloaded) {
                node.points.delete(frame.gl);
            }
        }
    }

    /// The counters of this frame (`stats`); the requests in flight
    pub(super) fn update_stats(&mut self) -> usize {
        let (in_flight, loaded, gpu_bytes) = self.states.counts();

        self.stats = PointCloudStats {
            points: self
                .selection
                .draw
                .iter()
                .filter_map(|&(i, _)| match self.states.get(i) {
                    Some(State::Ready(n)) => Some(n.points.count as u64),
                    _ => None,
                })
                .sum(),
            nodes: self.selection.draw.len(),
            loaded,
            in_flight,
            gpu_bytes,
            known: self.cloud.as_ref().map_or(0, |c| c.hierarchy.nodes.len()),
        };

        in_flight
    }
}

/// The GPU memory the nodes may take: twice the point budget's vertices, 64 MB at least
pub(super) fn gpu_budget(points: u64) -> usize {
    ((points as usize).saturating_mul(STRIDE * 2)).max(64 << 20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_budgets_follow_the_point_budget() {
        assert_eq!(gpu_budget(3_000_000), 120_000_000);
        assert_eq!(gpu_budget(10), 64 << 20);
    }
}
