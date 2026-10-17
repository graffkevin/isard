//! The traversal: the tiles to draw, to stand in for their loading children, to load (in priority order) and to
//! prefetch for a view, by screen-space error.

use super::{Content, Tree};
use geo::camera::View;
use glam::DVec4;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// Load state of a tile, as the traversal sees it
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Readiness {
    /// Content drawable now
    Ready,
    /// No drawable content (none, unsupported, failed for good, or an external tileset already grafted)
    Empty,
    /// Content to load (or loading)
    Pending,
    /// Failed, waiting before a new attempt: not drawable, and its parent keeps standing in for it
    Unavailable,
}

impl Readiness {
    /// State of a tile from those of its contents: the least ready wins
    fn merge(self, other: Self) -> Self {
        let rank = |r: Self| match r {
            Self::Empty => 0,
            Self::Ready => 1,
            Self::Pending => 2,
            Self::Unavailable => 3,
        };
        if rank(other) > rank(self) { other } else { self }
    }
}

#[derive(Default, Debug)]
pub struct Selection {
    /// Tiles to draw
    pub draw: Vec<usize>,
    /// Parents standing in for children still loading: drawn after `draw`, only where no finer tile covers the
    /// screen (finest first)
    pub backfill: Vec<usize>,
    /// Tiles to load, in priority order
    pub request: Vec<usize>,
    /// Siblings out of view of refined tiles: loaded when the network has room, so turning the camera finds them
    /// ready (3DTilesRendererJS's loadSiblings)
    pub prefetch: Vec<usize>,
    /// Visible tiles (to keep in the cache and their downloads alive)
    pub visible: Vec<usize>,
}

/// Selects the tiles to draw and to load for a view. `max_error`: screen-space error threshold (CSS pixels).
pub fn select(tree: &Tree, root: usize, view: &View, max_error: f64, ready: &dyn Fn(usize) -> Readiness) -> Selection {
    let planes = frustum(view);
    let mut traversal = Traversal {
        tree,
        view,
        planes,
        max_error,
        ready,
        out: Selection::default(),
        requests: Vec::new(),
        measures: RefCell::default(),
    };
    traversal.visit(root);
    let mut requests = traversal.requests;
    // Largest screen-space error first (as Cesium does): coarse tiles and nearby tiles, the most visible gaps
    requests.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.2.total_cmp(&b.2)));
    let mut seen = HashSet::new();
    requests.retain(|r| seen.insert(r.0));
    let mut out = traversal.out;
    out.request = requests.into_iter().map(|r| r.0).collect();
    out.backfill.sort_by_key(|&i| std::cmp::Reverse(tree.nodes[i].depth));
    out
}

struct Traversal<'a> {
    tree: &'a Tree,
    view: &'a View,
    planes: [DVec4; 6],
    max_error: f64,
    ready: &'a dyn Fn(usize) -> Readiness,
    out: Selection,
    /// (tile, screen-space error, distance)
    requests: Vec<(usize, f64, f64)>,
    /// `measure` of the tiles met this frame
    measures: RefCell<HashMap<usize, Option<(f64, f64)>>>,
}

impl Traversal<'_> {
    /// Screen-space error and distance to the camera of a tile (pixels); None if out of view or behind the horizon.
    /// Computed once per tile and frame
    fn measure(&self, i: usize) -> Option<(f64, f64)> {
        if let Some(m) = self.measures.borrow().get(&i) {
            return *m;
        }
        let node = &self.tree.nodes[i];
        let center = self.view.geodetic(node.geodetic);
        let scale = self.view.pixels_per_metre(node.geodetic.y);
        let radius = node.radius * scale;
        let m = if self.planes.iter().any(|p| p.truncate().dot(center) + p.w < -radius)
            || self.view.behind_horizon(center, radius)
        {
            None
        } else {
            let distance = (center.distance(self.view.eye) - radius).max(1e-6);
            Some((node.error * scale * self.view.center_distance / distance, distance))
        };
        self.measures.borrow_mut().insert(i, m);
        m
    }

    /// A tile and its companions
    fn members(&self, i: usize) -> impl Iterator<Item = usize> + use<'_> {
        std::iter::once(i).chain(self.tree.nodes[i].companions.iter().copied())
    }

    /// Readiness of a tile with all its contents
    fn state(&self, i: usize) -> Readiness {
        self.members(i).fold(Readiness::Empty, |s, m| s.merge((self.ready)(m)))
    }

    /// Requests the contents of a tile still to load
    fn request(&mut self, i: usize, error: f64, distance: f64) {
        for m in self.members(i).collect::<Vec<_>>() {
            if (self.ready)(m) == Readiness::Pending {
                self.requests.push((m, error, distance));
            }
        }
    }

    /// The contents of a tile that are ready
    fn ready_members(&self, i: usize) -> impl Iterator<Item = usize> + use<'_> {
        self.members(i).filter(|&m| (self.ready)(m) == Readiness::Ready)
    }

    fn visit(&mut self, i: usize) {
        if let Some((error, distance)) = self.measure(i) {
            self.visit_visible(i, error, distance);
        }
    }

    fn visit_visible(&mut self, i: usize, error: f64, distance: f64) {
        // A visible tile: its downloads must survive the next frame
        let members: Vec<usize> = self.members(i).collect();
        self.out.visible.extend(&members);
        let node = &self.tree.nodes[i];
        let state = self.state(i);
        // An empty tile with children (a grafted external tileset, a pure grouping node) cannot stand for its area:
        // always go down
        let hollow = state == Readiness::Empty && !node.children.is_empty();
        let refine = (error > self.max_error || hollow) && !node.children.is_empty();
        if !refine {
            self.draw_or_request(i, state, error, distance);
            return;
        }
        if !node.replace {
            // ADD: the tile and its children together
            self.draw_or_request(i, state, error, distance);
            for &c in &node.children {
                self.visit(c);
            }
            return;
        }
        // REPLACE: the children take over; while some visible ones load, the parent fills only the pixels the ready
        // ones leave (as Cesium does), so neither a hole nor a whole area turning coarse again because one child
        // entered the view. A parent that cannot be drawn yet does not hold them back either
        if matches!(state, Readiness::Pending | Readiness::Unavailable) {
            self.request(i, error, distance);
        }
        if state == Readiness::Ready && !self.children_ready(i, 3) {
            let ready: Vec<usize> = self.ready_members(i).collect();
            self.out.backfill.extend(ready);
        }
        for &c in &node.children {
            if let Some((error, distance)) = self.measure(c) {
                self.visit_visible(c, error, distance);
            } else if self.state(c) == Readiness::Pending {
                // A sibling out of view: its download is kept, it loads when there is room
                for m in self.members(c).collect::<Vec<_>>() {
                    if (self.ready)(m) == Readiness::Pending {
                        self.out.visible.push(m);
                        self.out.prefetch.push(m);
                    }
                }
            }
        }
    }

    fn draw_or_request(&mut self, i: usize, state: Readiness, error: f64, distance: f64) {
        match state {
            Readiness::Ready => {
                let ready: Vec<usize> = self.ready_members(i).collect();
                self.out.draw.extend(ready);
            }
            // A content failed: the others still load
            Readiness::Pending | Readiness::Unavailable => self.request(i, error, distance),
            Readiness::Empty => {}
        }
    }

    /// Whether the visible children of a tile can replace it: drawable, or empty with drawable children
    fn children_ready(&self, i: usize, depth: u32) -> bool {
        self.tree.nodes[i].children.iter().all(|&c| {
            if self.measure(c).is_none() {
                return true;
            }
            let node = &self.tree.nodes[c];
            match self.state(c) {
                Readiness::Ready => true,
                Readiness::Pending | Readiness::Unavailable => false,
                Readiness::Empty => {
                    depth > 0 && !node.children.is_empty() && self.children_ready(c, depth - 1)
                        || node.children.is_empty() && !matches!(node.content, Content::Tileset(_))
                }
            }
        })
    }
}

/// Frustum planes (local frame, normalized, inside when `dot(n, p) + w >= 0`)
fn frustum(view: &View) -> [DVec4; 6] {
    let m = view.view_proj.transpose();
    let (r0, r1, r2, r3) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2].map(|p| p / p.truncate().length())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{sphere_tile, tileset, tree, view};
    use super::*;
    use geo::camera::Camera;
    use geo::geodesy;

    #[test]
    fn far_view_draws_the_root_only() {
        let (tree, root) = tree();
        let s = select(&tree, root, &view(9.0), 16.0, &|_| Readiness::Ready);
        assert_eq!(s.draw, vec![root]);
    }

    #[test]
    fn close_view_refines_when_children_are_ready() {
        let (tree, root) = tree();
        let s = select(&tree, root, &view(16.0), 16.0, &|_| Readiness::Ready);
        assert!(
            !s.draw.contains(&root) && !s.draw.is_empty() && s.backfill.is_empty(),
            "{s:?}"
        );
    }

    #[test]
    fn parent_stays_while_children_load() {
        let (tree, root) = tree();
        let s = select(&tree, root, &view(16.0), 16.0, &|i| {
            if i == root {
                Readiness::Ready
            } else {
                Readiness::Pending
            }
        });
        assert!(
            s.draw.is_empty() && s.backfill == vec![root],
            "no hole while the children load: {s:?}"
        );
        assert!(!s.request.is_empty() && !s.request.contains(&root));
        // The children being loaded count as visible: their downloads are not aborted
        assert!(s.request.iter().all(|r| s.visible.contains(r)));
    }

    #[test]
    fn ready_children_show_while_their_parent_loads() {
        // The parent is not there yet and one child is slow: the others are drawn right away
        let (tree, root) = tree();
        let s = select(&tree, root, &view(16.0), 16.0, &|i| match i {
            0 | 1 => Readiness::Pending,
            _ => Readiness::Ready,
        });
        assert!(!s.draw.contains(&root) && s.draw.contains(&2), "{s:?}");
        assert!(s.request.contains(&root) && s.request.contains(&1));
    }

    #[test]
    fn siblings_out_of_view_are_prefetched() {
        // Close above child 0 (south-west): some of its siblings are out of view but load anyway, apart from the
        // visible tiles
        let (tree, root) = tree();
        let mut c = Camera::new(3.05, 50.62, 18.0);
        c.width = 1200.0;
        c.height = 800.0;
        let view = c.view();
        let planes = frustum(&view);
        let culled: Vec<usize> = tree.nodes[root]
            .children
            .iter()
            .copied()
            .filter(|&i| {
                let n = &tree.nodes[i];
                let center = view.ecef(n.center);
                let radius = n.radius * view.pixels_per_metre(geodesy::from_ecef(n.center).y);
                planes.iter().any(|p| p.truncate().dot(center) + p.w < -radius)
            })
            .collect();
        assert_ne!(culled, Vec::<usize>::new(), "some children must be out of view");
        let s = select(&tree, root, &view, 16.0, &|i| {
            if i == root {
                Readiness::Ready
            } else {
                Readiness::Pending
            }
        });
        assert!(
            culled.iter().all(|c| s.prefetch.contains(c) && !s.request.contains(c)),
            "{culled:?} {s:?}"
        );
        assert!(
            s.prefetch.iter().all(|r| s.visible.contains(r)),
            "their downloads are kept: {s:?}"
        );
    }

    #[test]
    fn coarse_tiles_load_first() {
        let (tree, root) = tree();
        let s = select(&tree, root, &view(16.0), 16.0, &|_| Readiness::Pending);
        assert_eq!(s.request.first(), Some(&root));
    }

    #[test]
    fn grafted_tilesets_are_always_traversed() {
        // The external tileset node (4) is empty once grafted; its own error is small, but its content is in the
        // grafted root, which must be drawn (else a hole)
        let (mut tree, root) = tree();
        let p = geodesy::to_ecef(3.07, 50.64, 30.0);
        let sub = format!(
            r#"{{"asset":{{"version":"1.0"}},"geometricError":2,"root":{{"boundingVolume":{{"sphere":[{},{},{},800]}},"geometricError":1,"content":{{"uri":"x.b3dm"}}}}}}"#,
            p.x, p.y, p.z
        );
        let grafted = tree
            .add(sub.as_bytes(), "https://example.org/mesh/sub/tileset.json", Some(4))
            .unwrap();
        let s = select(&tree, root, &view(16.0), 16.0, &|i| {
            if i == 4 { Readiness::Empty } else { Readiness::Ready }
        });
        assert!(s.draw.contains(&grafted), "{s:?}");
    }

    #[test]
    fn a_failed_child_keeps_its_parent() {
        let (tree, root) = tree();
        let s = select(&tree, root, &view(16.0), 16.0, &|i| {
            if i == 2 {
                Readiness::Unavailable
            } else {
                Readiness::Ready
            }
        });
        // The ready children show, the parent fills the area of the failed one rather than a hole
        assert_eq!(s.backfill, vec![root], "{s:?}");
        assert!(
            s.draw.contains(&1) && s.draw.contains(&3) && !s.draw.contains(&root),
            "{s:?}"
        );
    }

    #[test]
    fn tiles_out_of_view_are_culled() {
        let (tree, root) = tree();
        let mut c = Camera::new(3.06, 50.63, 16.0);
        c.width = 1200.0;
        c.height = 800.0;
        // 0.5° away: nothing of the tileset in view
        c.center = geo::camera::mercator(3.56, 50.63);
        let s = select(&tree, root, &c.view(), 16.0, &|_| Readiness::Ready);
        assert!(s.draw.is_empty() && s.visible.is_empty());
    }

    #[test]
    fn add_refinement_draws_the_parent_with_its_children() {
        let child = sphere_tile(3.06, 50.63, 800.0, 2.0, r#","content":{"uri":"a.glb"}"#);
        let root = sphere_tile(
            3.06,
            50.63,
            3000.0,
            64.0,
            &format!(r#","refine":"ADD","content":{{"uri":"r.glb"}},"children":[{child}]"#),
        );
        let tree = tileset(&root);
        assert!(!tree.nodes[0].replace && !tree.nodes[1].replace, "children inherit ADD");
        let s = select(&tree, 0, &view(16.0), 16.0, &|_| Readiness::Ready);
        assert_eq!(s.draw, vec![0, 1]);
        assert_eq!(s.backfill, Vec::<usize>::new());
    }

    #[test]
    fn several_contents_load_and_draw_together() {
        let root = sphere_tile(
            3.06,
            50.63,
            3000.0,
            64.0,
            r#","contents":[{"uri":"a.glb"},{"uri":"b.b3dm"},{"uri":"c.pnts"}]"#,
        );
        let tree = tileset(&root);
        assert_eq!(tree.nodes.len(), 3);
        assert_eq!(
            tree.nodes[0].content,
            Content::Mesh("https://example.org/t/a.glb".into())
        );
        assert_eq!(tree.nodes[0].companions, [1, 2]);
        assert_eq!(
            tree.nodes[1].content,
            Content::Mesh("https://example.org/t/b.b3dm".into())
        );
        assert_eq!(tree.nodes[2].content, Content::Unsupported);
        // Both meshes requested; drawn only once both are ready (the points stay empty)
        let pending = |i: usize| if i == 2 { Readiness::Empty } else { Readiness::Pending };
        let s = select(&tree, 0, &view(16.0), 16.0, &pending);
        assert_eq!(s.request, [0, 1]);
        assert_eq!(s.draw, Vec::<usize>::new());
        let half = |i: usize| match i {
            0 => Readiness::Ready,
            1 => Readiness::Pending,
            _ => Readiness::Empty,
        };
        let s = select(&tree, 0, &view(16.0), 16.0, &half);
        assert_eq!((s.draw.len(), s.request.as_slice()), (0, &[1][..]));
        let s = select(&tree, 0, &view(16.0), 16.0, &|i| {
            if i == 2 { Readiness::Empty } else { Readiness::Ready }
        });
        assert_eq!(s.draw, [0, 1]);
        assert_eq!(s.visible, [0, 1, 2], "no duplicates");
    }

    #[test]
    fn the_far_side_of_the_globe_is_culled() {
        // On the globe: a tile at Lille and one at its antipode, both under an ADD root covering the Earth
        let near = sphere_tile(3.06, 50.63, 1000.0, 1e5, r#","content":{"uri":"near.glb"}"#);
        let far = sphere_tile(-176.94, -50.63, 1000.0, 1e5, r#","content":{"uri":"far.glb"}"#);
        let root = format!(
            r#"{{"boundingVolume":{{"sphere":[0,0,0,7000000]}},"geometricError":1e7,"refine":"ADD","children":[{near},{far}]}}"#
        );
        let tree = tileset(&root);
        let mut c = Camera::new(3.06, 50.63, 2.0);
        c.width = 1200.0;
        c.height = 800.0;
        c.globe = true;
        let s = select(&tree, 0, &c.view(), 16.0, &|i| {
            if i == 0 { Readiness::Empty } else { Readiness::Pending }
        });
        assert!(s.request.contains(&1) && !s.request.contains(&2), "{s:?}");
        assert!(!s.visible.contains(&2));
    }
}
