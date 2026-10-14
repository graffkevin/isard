//! 3D Tiles tileset tree and its traversal (pure Rust, no I/O, no GPU).
//!
//! The traversal is driven by the screen-space error, as in Cesium and 3DTilesRendererJS: a tile is refined when its
//! geometric error covers more than `max_error` pixels on screen. With REPLACE refinement, children replace their
//! parent only once all the visible ones are ready, so a loading area keeps its coarser tile instead of a hole.
//! Requests are ordered coarse first (the shallow tiles cover the view quickly), then nearest first.

use crate::camera::View;
use crate::geodesy;
use glam::{DMat4, DVec3, DVec4};
use serde::Deserialize;

#[derive(Deserialize)]
struct TilesetJson {
    #[serde(default)]
    asset: AssetJson,
    root: TileJson,
}

#[derive(Deserialize, Default)]
struct AssetJson {
    #[serde(rename = "gltfUpAxis")]
    gltf_up_axis: Option<String>,
}

#[derive(Deserialize)]
struct TileJson {
    #[serde(rename = "boundingVolume")]
    bounding_volume: VolumeJson,
    #[serde(rename = "geometricError")]
    geometric_error: f64,
    refine: Option<String>,
    content: Option<ContentJson>,
    #[serde(default)]
    children: Vec<TileJson>,
    transform: Option<[f64; 16]>,
}

#[derive(Deserialize)]
struct ContentJson {
    uri: Option<String>,
    /// 3D Tiles 0.0 name
    url: Option<String>,
}

#[derive(Deserialize)]
struct VolumeJson {
    sphere: Option<[f64; 4]>,
    region: Option<[f64; 6]>,
    #[serde(rename = "box")]
    oriented_box: Option<[f64; 12]>,
}

/// What the content of a tile is
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    None,
    /// `.b3dm` or `.glb`
    Mesh(String),
    /// External tileset, grafted under the tile once loaded
    Tileset(String),
    /// A format this engine does not draw (points, instances, composites)
    Unsupported,
}

pub struct Node {
    /// Bounding sphere, ECEF
    pub center: DVec3,
    pub radius: f64,
    /// Metres
    pub error: f64,
    pub replace: bool,
    pub content: Content,
    pub children: Vec<usize>,
    /// World transform of the content
    pub transform: DMat4,
    /// glTF y-up (3D Tiles 1.0 default)
    pub y_up: bool,
    pub depth: u32,
}

#[derive(Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

impl Tree {
    /// Adds a tileset; `parent`: the tile it is grafted under (external tileset), None for the root. Relative URIs
    /// resolve against `url`. Returns the index of its root.
    pub fn add(&mut self, json: &[u8], url: &str, parent: Option<usize>) -> Result<usize, String> {
        let tileset: TilesetJson = serde_json::from_slice(json).map_err(|e| format!("tileset: {e}"))?;
        let y_up = !tileset
            .asset
            .gltf_up_axis
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case("z"));
        let (transform, depth, replace) = parent.map_or((DMat4::IDENTITY, 0, true), |p| {
            let n = &self.nodes[p];
            (n.transform, n.depth + 1, n.replace)
        });
        let root = self.add_tile(&tileset.root, url, &transform, replace, y_up, depth)?;
        if let Some(p) = parent {
            self.nodes[p].children.push(root);
        }
        Ok(root)
    }

    fn add_tile(
        &mut self,
        tile: &TileJson,
        url: &str,
        parent: &DMat4,
        replace: bool,
        y_up: bool,
        depth: u32,
    ) -> Result<usize, String> {
        if depth > 64 {
            return Err("tileset too deep".into());
        }
        let transform = tile.transform.map_or(*parent, |t| *parent * DMat4::from_cols_array(&t));
        let (center, radius) = volume(&tile.bounding_volume, &transform)?;
        let replace = tile
            .refine
            .as_deref()
            .map_or(replace, |r| !r.eq_ignore_ascii_case("add"));
        let content = match tile.content.as_ref().and_then(|c| c.uri.as_ref().or(c.url.as_ref())) {
            None => Content::None,
            Some(uri) => {
                let absolute = resolve(url, uri);
                let path = absolute.split(['?', '#']).next().unwrap_or("");
                let extension = path.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
                match extension.as_str() {
                    "json" => Content::Tileset(absolute),
                    "b3dm" | "glb" => Content::Mesh(absolute),
                    _ => Content::Unsupported,
                }
            }
        };
        let index = self.nodes.len();
        self.nodes.push(Node {
            center,
            radius,
            error: tile.geometric_error,
            replace,
            content,
            children: Vec::new(),
            transform,
            y_up,
            depth,
        });
        for child in &tile.children {
            let c = self.add_tile(child, url, &transform, replace, y_up, depth + 1)?;
            self.nodes[index].children.push(c);
        }
        Ok(index)
    }
}

/// Bounding sphere (ECEF) of a bounding volume
fn volume(v: &VolumeJson, transform: &DMat4) -> Result<(DVec3, f64), String> {
    let scale = transform
        .x_axis
        .truncate()
        .length()
        .max(transform.y_axis.truncate().length())
        .max(transform.z_axis.truncate().length());
    if let Some([x, y, z, r]) = v.sphere {
        return Ok((transform.transform_point3(DVec3::new(x, y, z)), r * scale));
    }
    if let Some(b) = v.oriented_box {
        let half = DVec3::new(b[3], b[4], b[5]).length_squared()
            + DVec3::new(b[6], b[7], b[8]).length_squared()
            + DVec3::new(b[9], b[10], b[11]).length_squared();
        return Ok((
            transform.transform_point3(DVec3::new(b[0], b[1], b[2])),
            half.sqrt() * scale,
        ));
    }
    if let Some([west, south, east, north, low, high]) = v.region {
        // Regions are in ECEF already (no transform); sample the curved box
        let (lon, lat) = (f64::midpoint(west, east), f64::midpoint(south, north));
        let center = geodesy::to_ecef(lon.to_degrees(), lat.to_degrees(), f64::midpoint(low, high));
        let mut radius = 0f64;
        for x in [west, lon, east] {
            for y in [south, lat, north] {
                for h in [low, high] {
                    radius = radius.max(geodesy::to_ecef(x.to_degrees(), y.to_degrees(), h).distance(center));
                }
            }
        }
        return Ok((center, radius));
    }
    Err("bounding volume without sphere, box or region".into())
}

/// Resolves a relative URI against the URL of its tileset; query strings of the tileset are kept (signed URLs)
fn resolve(base: &str, uri: &str) -> String {
    if uri.contains("://") || uri.starts_with("data:") {
        return uri.to_owned();
    }
    let (path, query) = base.split_once('?').map_or((base, ""), |(p, q)| (p, q));
    let root = if let Some(rest) = uri.strip_prefix('/') {
        let origin_end = path
            .find("://")
            .map_or(0, |i| path[i + 3..].find('/').map_or(path.len(), |j| i + 3 + j));
        format!("{}/{rest}", &path[..origin_end])
    } else {
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        format!("{dir}{}", uri.replace('\\', "/"))
    };
    if query.is_empty() || uri.contains('?') {
        root
    } else {
        format!("{root}?{query}")
    }
}

/// Load state of a tile, as the traversal sees it
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Readiness {
    /// Content drawable now
    Ready,
    /// No drawable content (none, unsupported, failed for good, or an external tileset already grafted)
    Empty,
    /// Content to load (or loading)
    Pending,
}

#[derive(Default, Debug)]
pub struct Selection {
    /// Tiles to draw
    pub draw: Vec<usize>,
    /// Tiles to load, in priority order
    pub request: Vec<usize>,
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
    };
    traversal.visit(root);
    let mut requests = traversal.requests;
    requests.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)));
    requests.dedup_by_key(|r| r.0);
    let mut out = traversal.out;
    out.request = requests.into_iter().map(|r| r.0).collect();
    out
}

struct Traversal<'a> {
    tree: &'a Tree,
    view: &'a View,
    planes: [DVec4; 6],
    max_error: f64,
    ready: &'a dyn Fn(usize) -> Readiness,
    out: Selection,
    /// (tile, depth, distance)
    requests: Vec<(usize, u32, f64)>,
}

impl Traversal<'_> {
    /// Center (local frame), radius (pixels) and distance to the camera of a tile; None if out of view
    fn measure(&self, i: usize) -> Option<(f64, f64)> {
        let node = &self.tree.nodes[i];
        let center = self.view.ecef(node.center);
        let g = geodesy::from_ecef(node.center);
        let radius = node.radius * self.view.pixels_per_metre(g.y);
        if self.planes.iter().any(|p| p.truncate().dot(center) + p.w < -radius) {
            return None;
        }
        let distance = (center.distance(self.view.eye) - radius).max(1e-6);
        let error = node.error * self.view.pixels_per_metre(g.y) * self.view.center_distance / distance;
        Some((error, distance))
    }

    fn request(&mut self, i: usize, distance: f64) {
        // A requested tile is in view: its download must survive the next frame
        self.out.visible.push(i);
        self.requests.push((i, self.tree.nodes[i].depth, distance));
    }

    fn visit(&mut self, i: usize) {
        let Some((error, distance)) = self.measure(i) else {
            return;
        };
        self.out.visible.push(i);
        let node = &self.tree.nodes[i];
        let state = (self.ready)(i);
        let refine = error > self.max_error && !node.children.is_empty();
        if !refine {
            match state {
                Readiness::Ready => self.out.draw.push(i),
                Readiness::Pending => self.request(i, distance),
                Readiness::Empty => {}
            }
            return;
        }
        if !node.replace {
            // ADD: the tile and its children together
            match state {
                Readiness::Ready => self.out.draw.push(i),
                Readiness::Pending => self.request(i, distance),
                Readiness::Empty => {}
            }
            for &c in &node.children {
                self.visit(c);
            }
            return;
        }
        // REPLACE: the children take over once all the visible ones can be drawn
        if self.children_ready(i, 3) {
            for &c in &node.children {
                self.visit(c);
            }
            return;
        }
        match state {
            Readiness::Ready => self.out.draw.push(i),
            Readiness::Pending => self.request(i, distance),
            Readiness::Empty => {}
        }
        self.load_children(i, 3);
    }

    /// Whether the visible children of a tile can replace it: drawable, or empty with drawable children
    fn children_ready(&self, i: usize, depth: u32) -> bool {
        self.tree.nodes[i].children.iter().all(|&c| {
            if self.measure(c).is_none() {
                return true;
            }
            match (self.ready)(c) {
                Readiness::Ready => true,
                Readiness::Pending => false,
                Readiness::Empty => {
                    depth > 0 && !self.tree.nodes[c].children.is_empty() && self.children_ready(c, depth - 1)
                        || self.tree.nodes[c].children.is_empty()
                            && !matches!(self.tree.nodes[c].content, Content::Tileset(_))
                }
            }
        })
    }

    /// Requests what the visible children need to replace their parent
    fn load_children(&mut self, i: usize, depth: u32) {
        for c in self.tree.nodes[i].children.clone() {
            let Some((_, distance)) = self.measure(c) else { continue };
            match (self.ready)(c) {
                Readiness::Pending => self.request(c, distance),
                Readiness::Empty if depth > 0 => self.load_children(c, depth - 1),
                _ => {}
            }
        }
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
    use super::*;
    use crate::camera::Camera;

    /// A small REPLACE tileset over Lille: a root, four children, one external tileset
    fn tree() -> (Tree, usize) {
        let c = geodesy::to_ecef(3.06, 50.63, 30.0);
        let child = |dx: f64, dy: f64, uri: &str| {
            let p = geodesy::to_ecef(3.06 + dx, 50.63 + dy, 30.0);
            format!(
                r#"{{"boundingVolume":{{"sphere":[{},{},{},800]}},"geometricError":2,"content":{{"uri":"{uri}"}}}}"#,
                p.x, p.y, p.z
            )
        };
        let json = format!(
            r#"{{"asset":{{"version":"1.0"}},"geometricError":500,"root":{{"boundingVolume":{{"sphere":[{},{},{},3000]}},"geometricError":64,"refine":"REPLACE","content":{{"uri":"root.b3dm"}},"children":[{},{},{},{}]}}}}"#,
            c.x,
            c.y,
            c.z,
            child(-0.01, -0.01, "a.b3dm"),
            child(0.01, -0.01, "b.b3dm"),
            child(-0.01, 0.01, "c.glb"),
            child(0.01, 0.01, "sub/tileset.json"),
        );
        let mut tree = Tree::default();
        let root = tree
            .add(json.as_bytes(), "https://example.org/mesh/tileset.json?key=1", None)
            .unwrap();
        (tree, root)
    }

    fn view(zoom: f64) -> View {
        let mut c = Camera::new(3.06, 50.63, zoom);
        c.width = 1200.0;
        c.height = 800.0;
        c.view()
    }

    #[test]
    fn parses_and_resolves_uris() {
        let (tree, root) = tree();
        assert_eq!(tree.nodes.len(), 5);
        assert_eq!(
            tree.nodes[root].content,
            Content::Mesh("https://example.org/mesh/root.b3dm?key=1".into())
        );
        assert_eq!(
            tree.nodes[4].content,
            Content::Tileset("https://example.org/mesh/sub/tileset.json?key=1".into())
        );
        assert!(tree.nodes[root].replace && tree.nodes[root].y_up);
        assert_eq!(resolve("https://h.org/a/b/t.json", "/x.b3dm"), "https://h.org/x.b3dm");
        assert_eq!(
            resolve("https://h.org/a/t.json", "https://o.org/x.glb"),
            "https://o.org/x.glb"
        );
    }

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
        assert!(!s.draw.contains(&root) && !s.draw.is_empty(), "{s:?}");
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
        assert_eq!(s.draw, vec![root], "no hole while the children load");
        assert!(!s.request.is_empty() && !s.request.contains(&root));
        // The children being loaded count as visible: their downloads are not aborted
        assert!(s.request.iter().all(|r| s.visible.contains(r)));
    }

    #[test]
    fn coarse_tiles_load_first() {
        let (tree, root) = tree();
        let s = select(&tree, root, &view(16.0), 16.0, &|_| Readiness::Pending);
        assert_eq!(s.request.first(), Some(&root));
    }

    #[test]
    fn tiles_out_of_view_are_culled() {
        let (tree, root) = tree();
        let mut c = Camera::new(3.06, 50.63, 16.0);
        c.width = 1200.0;
        c.height = 800.0;
        // 0.5° away: nothing of the tileset in view
        c.center = crate::camera::mercator(3.56, 50.63);
        let s = select(&tree, root, &c.view(), 16.0, &|_| Readiness::Ready);
        assert!(s.draw.is_empty() && s.visible.is_empty());
    }
}
