//! The traversal of a point cloud's octree for a view: nodes taken by priority (their screen-space error, more for
//! those near the middle of the screen), each refined while the spacing of its points covers more than `max_error`
//! pixels, until the point budget is spent (as Potree does). Points refine additively: a node's points stay drawn
//! under its children's. What is visible and not loaded is requested in the same order, the pages of the hierarchy
//! first reached.

use crate::copc::{Content, Hierarchy};
use geo::camera::View;
use glam::{DVec3, DVec4};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// Load state of a node's chunk, as the traversal sees it
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Readiness {
    Ready,
    /// To load, or loading
    Pending,
    /// Failed, waiting before a new attempt
    Unavailable,
}

#[derive(Default, Debug)]
pub struct Selection {
    /// Nodes to draw, by priority, and the level their points are sized at: the deepest level drawn below each that
    /// covers it whole (its own depth when a child is missing)
    pub draw: Vec<(usize, u32)>,
    /// Nodes whose chunk to load, by priority
    pub request: Vec<usize>,
    /// Nodes whose hierarchy page to load, by priority
    pub pages: Vec<usize>,
    /// The nodes of the selection, drawn or not (their downloads are kept)
    pub visible: Vec<usize>,
    /// Points of the selection, loaded or not
    pub points: u64,
}

/// A node in the queue: its priority and index
struct Queued(f64, usize);

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        // The lower index first among equals: the same selection from frame to frame
        self.0.total_cmp(&other.0).then(other.1.cmp(&self.1))
    }
}

/// Selects the nodes to draw and to load for a view, into `out` (cleared first). `max_error`: the spacing of the
/// points (CSS pixels) above which a node is refined; `budget`: the points selected at most (the first node always)
pub fn select(
    tree: &Hierarchy,
    view: &View,
    max_error: f64,
    budget: u64,
    ready: &dyn Fn(usize) -> Readiness,
    out: &mut Selection,
) {
    out.draw.clear();
    out.request.clear();
    out.pages.clear();
    out.visible.clear();
    out.points = 0;

    let planes = frustum(view);
    let mut queue = BinaryHeap::new();

    if let Some((_, priority)) = measure(tree, 0, view, &planes) {
        queue.push(Queued(priority, 0));
    }

    let mut drawn = HashMap::new();

    while let Some(Queued(_, i)) = queue.pop() {
        let node = &tree.nodes[i];

        match node.content {
            Content::Page(_) => {
                out.pages.push(i);
                out.visible.push(i);
                continue;
            }
            Content::Points { count, .. } => {
                if out.points > 0 && out.points + u64::from(count) > budget {
                    break;
                }
                out.points += u64::from(count);
                out.visible.push(i);

                match ready(i) {
                    Readiness::Ready => {
                        drawn.insert(i, out.draw.len());
                        out.draw.push((i, node.key.depth));
                    }
                    Readiness::Pending => out.request.push(i),
                    Readiness::Unavailable => {}
                }
            }
            Content::Empty => {}
        }

        let Some((error, _)) = measure(tree, i, view, &planes) else {
            continue;
        };

        if error <= max_error {
            continue;
        }

        for &c in &node.children {
            if let Some((_, priority)) = measure(tree, c, view, &planes) {
                queue.push(Queued(priority, c));
            }
        }
    }

    // The size level of each node drawn, from the nodes drawn below it
    for k in 0..out.draw.len() {
        let (i, _) = out.draw[k];

        out.draw[k].1 = level(tree, i, &drawn);
    }
}

/// The deepest level drawn below a drawn node that covers it whole: its own depth if a child holding points is not
/// drawn
fn level(tree: &Hierarchy, i: usize, drawn: &HashMap<usize, usize>) -> u32 {
    let depth = tree.nodes[i].key.depth;
    let mut deepest = u32::MAX;

    for &c in &tree.nodes[i].children {
        match cover(tree, c, drawn) {
            Some(l) => deepest = deepest.min(l),
            None => return depth,
        }
    }

    if deepest == u32::MAX { depth } else { deepest }
}

/// The level a child covers its cube down to: None if points there are not drawn, `u32::MAX` if it holds none
fn cover(tree: &Hierarchy, c: usize, drawn: &HashMap<usize, usize>) -> Option<u32> {
    let node = &tree.nodes[c];

    match node.content {
        Content::Points { .. } => drawn.contains_key(&c).then(|| level(tree, c, drawn)),
        Content::Page(_) => None,
        Content::Empty => node
            .children
            .iter()
            .try_fold(u32::MAX, |deepest, &g| cover(tree, g, drawn).map(|l| deepest.min(l))),
    }
}

/// Screen-space error (CSS pixels: the spacing of the node's points at its distance) and priority of a visible node;
/// None out of the view
fn measure(tree: &Hierarchy, i: usize, view: &View, planes: &[DVec4; 6]) -> Option<(f64, f64)> {
    let node = &tree.nodes[i];
    let center = view.geodetic(node.geodetic);
    let scale = view.pixels_per_metre(node.geodetic.y);
    let radius = node.radius * scale;

    if planes.iter().any(|p| p.truncate().dot(center) + p.w < -radius) || view.behind_horizon(center, radius) {
        return None;
    }

    let distance = (center.distance(view.eye) - radius).max(1e-6);
    let error = node.spacing * scale * view.center_distance / distance;
    // Nearer the middle of the screen first: the angle from the line of sight (the eye to the center)
    let sight = (DVec3::new(0.0, 0.0, view.lift) - view.eye).normalize_or_zero();
    let angle = sight
        .dot((center - view.eye).normalize_or_zero())
        .clamp(-1.0, 1.0)
        .acos();

    Some((error, error / (1.0 + 2.0 * angle)))
}

fn frustum(view: &View) -> [DVec4; 6] {
    let m = view.view_proj.transpose();
    let (r0, r1, r2, r3) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);

    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2].map(|p| p / p.truncate().length())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copc::{Copc, Head, parse_page};
    use geo::camera::Camera;

    /// The hierarchy of the Paris tile placed on the plate carrée: x, y of Lambert-93 mapped linearly onto the tile's
    /// longitudes and latitudes (enough for the traversal)
    fn paris() -> Hierarchy {
        let Head::Ready(copc) = Copc::parse(include_bytes!("../tests/data/paris_header.bin")).unwrap() else {
            panic!("the whole head is there");
        };
        let mut h = Hierarchy::new(copc.info);
        let added = h
            .add_page(0, parse_page(include_bytes!("../tests/data/paris_page.bin")).unwrap())
            .unwrap();

        for i in std::iter::once(0).chain(added) {
            let [x, y, z] = h.center(i);
            let lon = 2.3434 + (x - 652_000.0) / 1000.0 * 0.0137;
            let lat = 48.8487 + (y - 6_861_000.0) / 1000.0 * 0.009;

            h.place(i, DVec3::new(lon, lat, z + 44.0), 1.0, 1.0);
        }

        h
    }

    fn view(zoom: f64, pitch: f64) -> View {
        let mut c = Camera::new(2.35, 48.853, zoom);

        c.width = 800.0;
        c.height = 600.0;
        c.pitch = pitch;

        c.view()
    }

    #[test]
    fn far_views_take_the_root_only() {
        let tree = paris();
        let mut s = Selection::default();

        select(&tree, &view(12.0, 0.0), 2.0, 3_000_000, &|_| Readiness::Ready, &mut s);
        assert_eq!(s.draw, vec![(0, 0)]);
        assert_eq!(s.points, 62_403);
        // Nothing loaded: the root requested
        select(&tree, &view(12.0, 0.0), 2.0, 3_000_000, &|_| Readiness::Pending, &mut s);
        assert_eq!((s.draw.len(), s.request.as_slice()), (0, &[0][..]));
    }

    #[test]
    fn close_views_refine_within_the_budget() {
        let tree = paris();
        let mut s = Selection::default();

        select(&tree, &view(18.0, 50.0), 2.0, 3_000_000, &|_| Readiness::Ready, &mut s);
        assert!(s.draw.len() > 8, "{}", s.draw.len());
        assert!(s.draw.iter().any(|&(i, _)| tree.nodes[i].key.depth >= 3));
        assert!(s.points <= 3_000_000);
        // Coarse first: the root leads
        assert_eq!(s.draw[0].0, 0);

        // A small budget stops the traversal early, the first node always taken
        let mut small = Selection::default();

        select(
            &tree,
            &view(18.0, 50.0),
            2.0,
            200_000,
            &|_| Readiness::Ready,
            &mut small,
        );
        assert!(small.points <= 200_000 && small.draw.len() < s.draw.len());
        select(&tree, &view(18.0, 50.0), 2.0, 10, &|_| Readiness::Ready, &mut small);
        assert_eq!(small.draw.len(), 1);
    }

    #[test]
    fn nodes_out_of_view_are_left_out() {
        let tree = paris();
        let mut s = Selection::default();
        // Looking at the north-east corner: no node of the south-west at depth 2 or more
        let mut c = Camera::new(2.3565, 48.8573, 19.0);

        c.width = 800.0;
        c.height = 600.0;
        select(&tree, &c.view(), 2.0, 3_000_000, &|_| Readiness::Ready, &mut s);

        let far = s
            .draw
            .iter()
            .filter(|&&(i, _)| {
                let k = tree.nodes[i].key;

                k.depth >= 2 && k.x < 1 << (k.depth - 1) && k.y < 1 << (k.depth - 1)
            })
            .count();

        assert_eq!(far, 0);
        assert_ne!(s.draw.len(), 0);
    }

    #[test]
    fn sizes_follow_the_deepest_level_covering_a_node() {
        let tree = paris();
        let mut s = Selection::default();

        select(&tree, &view(18.0, 50.0), 2.0, 3_000_000, &|_| Readiness::Ready, &mut s);
        for &(i, level) in &s.draw {
            assert!(level >= tree.nodes[i].key.depth);
        }

        // A node with a child not drawn keeps its own depth
        let missing = tree.nodes[0].children[0];

        select(
            &tree,
            &view(18.0, 50.0),
            2.0,
            3_000_000,
            &|i| {
                if i == missing {
                    Readiness::Pending
                } else {
                    Readiness::Ready
                }
            },
            &mut s,
        );
        assert_eq!(s.draw[0], (0, 0));
        assert!(s.request.contains(&missing));
    }

    #[test]
    fn pages_not_loaded_are_asked_for() {
        let Head::Ready(copc) = Copc::parse(include_bytes!("../tests/data/paris_header.bin")).unwrap() else {
            panic!("the whole head is there");
        };
        let mut tree = Hierarchy::new(copc.info);

        tree.place(0, DVec3::new(2.35, 48.853, 566.0), 1.0, 1.0);

        let mut s = Selection::default();

        select(&tree, &view(16.0, 0.0), 2.0, 3_000_000, &|_| Readiness::Pending, &mut s);
        assert_eq!((s.pages.as_slice(), s.request.len()), (&[0][..], 0));
    }
}
