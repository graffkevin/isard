//! 3D Tiles tileset tree and its traversal (pure Rust, no I/O, no GPU).
//!
//! The traversal is driven by the screen-space error, as in Cesium and 3DTilesRendererJS: a tile is refined when its
//! geometric error covers more than `max_error` pixels on screen. With REPLACE refinement, children replace their
//! parent only once all the visible ones are ready, so a loading area keeps its coarser tile instead of a hole.
//! Requests are ordered coarse first (the shallow tiles cover the view quickly), then nearest first.
//!
//! 3D Tiles 1.1: a tile with several contents (`contents`) is a node whose other contents are companion nodes,
//! loaded and drawn together with it. Implicit tiling (QUADTREE and OCTREE) grows the tree lazily: a placeholder node
//! stands for each subtree not loaded yet, its content the subtree file (`Content::Tileset`, so it is fetched like an
//! external tileset and handed back to `Tree::add`); a loaded subtree becomes nodes a few levels at a time, deeper
//! levels waiting behind placeholders that expand from the subtree already in memory (`Tree::expandable`).
//!
//! This module builds the tree (`Tree::add`); `json` reads the tileset, `volume` its bounding volumes, `implicit` the
//! implicit tiling, `traversal` selects the tiles for a view (`select`).

#![forbid(unsafe_code)]

mod implicit;
mod json;
mod traversal;
mod volume;

use geo::geodesy;
use glam::{DMat4, DVec3};
use implicit::{Coord, Implicit, Placeholder};
use json::{ContentJson, ImplicitJson, TileJson, TilesetJson};
use std::collections::HashMap;
use std::rc::Rc;
pub use traversal::{Readiness, Scratch, Selection, select, select_into};
use volume::Volume;

/// What the content of a tile is
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    None,
    /// `.b3dm`, `.pnts` or `.glb` (or an unknown extension: the decoder tells by the first bytes)
    Mesh(String),
    /// External tileset or implicit-tiling subtree, grafted under the tile once loaded (`Tree::add`)
    Tileset(String),
    /// A format this engine does not draw (instances, composites)
    Unsupported,
}

/// What downloaded content is, by its first bytes (its URI may not tell)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `b3dm`, `pnts` or `glTF` binary
    Mesh,
    /// JSON: an external tileset (or a JSON subtree)
    Json,
    /// Binary implicit-tiling subtree
    Subtree,
    Other,
}

/// The kind of downloaded content, by its magic bytes
#[must_use]
pub fn sniff(bytes: &[u8]) -> Kind {
    match bytes.get(..4) {
        Some(b"b3dm" | b"pnts" | b"glTF") => Kind::Mesh,
        Some(b"subt") => Kind::Subtree,
        _ if bytes.trim_ascii_start().starts_with(b"{") => Kind::Json,
        _ => Kind::Other,
    }
}

pub struct Node {
    /// Bounding sphere, ECEF
    pub center: DVec3,
    pub radius: f64,
    /// Longitude, latitude (degrees) and height (m) of `center`, computed once
    pub geodetic: DVec3,
    /// Metres
    pub error: f64,
    pub replace: bool,
    pub content: Content,
    pub children: Vec<usize>,
    /// Nodes holding the other contents of this tile (3D Tiles 1.1 `contents`): in no children list, selected
    /// with this node
    pub companions: Vec<usize>,
    /// World transform of the content
    pub transform: DMat4,
    /// glTF y-up (3D Tiles 1.0 default)
    pub y_up: bool,
    pub depth: u32,
}

/// Most nodes of a tree: an external tileset grafted past it is refused. Each graft adds a whole tileset, and a
/// tileset may graft others (itself under another name) without end
pub const MAX_NODES: usize = 1 << 20;

pub struct Tree {
    pub nodes: Vec<Node>,
    /// `MAX_NODES` (fewer in the tests)
    max_nodes: usize,
    /// Implicit-tiling placeholders not expanded yet, by node
    pending: HashMap<usize, Placeholder>,
    /// The tilesets added: their URL and the tileset they were grafted into (None for the root)
    tilesets: Vec<(String, Option<usize>)>,
    /// The tileset of each node
    node_tileset: Vec<usize>,
}

impl Default for Tree {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            max_nodes: MAX_NODES,
            pending: HashMap::new(),
            tilesets: Vec::new(),
            node_tileset: Vec::new(),
        }
    }
}

/// Content of a URI, by its extension; unknown ones are taken for meshes (the decoder checks the first bytes)
fn classify(url: &str, uri: &str) -> Content {
    let absolute = resolve(url, uri);
    let path = absolute.split(['?', '#']).next().unwrap_or("");
    let file = path.rsplit('/').next().unwrap_or("");
    let extension = file.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();

    match extension.as_str() {
        "json" => Content::Tileset(absolute),
        "i3dm" | "cmpt" | "gltf" | "vctr" | "geom" | "geojson" => Content::Unsupported,
        _ => Content::Mesh(absolute),
    }
}

/// Fields of a new node shared by its companions
struct Base {
    center: DVec3,
    radius: f64,
    error: f64,
    replace: bool,
    transform: DMat4,
    y_up: bool,
    depth: u32,
}

impl Tree {
    /// Adds a tileset; `parent`: the tile it is grafted under (external tileset), None for the root. Relative URIs
    /// resolve against `url`. Returns the index of its root.
    ///
    /// Under an implicit-tiling placeholder, `json` is its subtree file (binary or JSON; ignored when the subtree is
    /// in memory already, see `expandable`): the tiles it makes available are grafted under the placeholder.
    ///
    /// An external tileset already among the tilesets above `parent` (one that grafts itself, directly or through
    /// others) is refused, and so is anything once the tree holds `MAX_NODES` nodes.
    pub fn add(&mut self, json: &[u8], url: &str, parent: Option<usize>) -> Result<usize, String> {
        if self.nodes.len() >= self.max_nodes {
            return Err(format!("tileset: more than {} tiles", self.max_nodes));
        }
        if let Some(p) = parent
            && let Some(placeholder) = self.pending.remove(&p)
        {
            let expanded = self.expand(p, placeholder, json, url);
            let tileset = self.node_tileset.get(p).copied().unwrap_or(0);

            self.node_tileset.resize(self.nodes.len(), tileset);

            return expanded;
        }

        let above = parent.and_then(|p| self.node_tileset.get(p).copied());
        let mut ancestor = above;

        while let Some(t) = ancestor {
            if self.tilesets[t].0 == url {
                return Err(format!("tileset {url} includes itself"));
            }
            ancestor = self.tilesets[t].1;
        }

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
        let first = self.nodes.len();
        let added = self.add_tile(&tileset.root, url, &transform, replace, y_up, depth);
        let Ok(root) = added else {
            // Nothing half-added: the nodes of a refused tileset are not in the tree
            self.nodes.truncate(first);
            self.pending.retain(|&node, _| node < first);

            return added;
        };

        self.tilesets.push((url.to_owned(), above));
        self.node_tileset.resize(self.nodes.len(), self.tilesets.len() - 1);
        if let Some(p) = parent {
            self.nodes[p].children.push(root);
        }

        Ok(root)
    }

    /// Whether a tile is an implicit placeholder whose subtree is in memory already: `add(&[], "", Some(tile))`
    /// expands it without downloading anything
    #[must_use]
    pub fn expandable(&self, tile: usize) -> bool {
        self.pending.get(&tile).is_some_and(|p| p.subtree.is_some())
    }

    /// Pushes a node and the companions holding its other contents; returns its index
    fn push(&mut self, base: &Base, contents: Vec<Content>) -> usize {
        let index = self.nodes.len();
        let mut contents = contents.into_iter();
        let node = |content: Content| Node {
            center: base.center,
            radius: base.radius,
            geodetic: geodesy::from_ecef(base.center),
            error: base.error,
            replace: base.replace,
            content,
            children: Vec::new(),
            companions: Vec::new(),
            transform: base.transform,
            y_up: base.y_up,
            depth: base.depth,
        };

        self.nodes.push(node(contents.next().unwrap_or(Content::None)));
        for content in contents {
            let companion = self.nodes.len();

            self.nodes.push(node(content));
            self.nodes[index].companions.push(companion);
        }

        index
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
        if self.nodes.len() >= self.max_nodes {
            return Err(format!("tileset: more than {} tiles", self.max_nodes));
        }

        let transform = tile.transform.map_or(*parent, |t| *parent * DMat4::from_cols_array(&t));
        let volume = Volume::parse(&tile.bounding_volume)?;
        let (center, radius) = volume.sphere(&transform)?;
        let replace = tile
            .refine
            .as_deref()
            .map_or(replace, |r| !r.eq_ignore_ascii_case("add"));
        let multiple: Vec<ContentJson> = tile
            .extensions
            .get("3DTILES_multiple_contents")
            .and_then(|e| serde_json::from_value(e["contents"].clone()).ok())
            .unwrap_or_default();
        let uris: Vec<&String> = tile
            .content
            .iter()
            .chain(&tile.contents)
            .chain(&multiple)
            .filter_map(|c| c.uri.as_ref().or(c.url.as_ref()))
            .collect();
        let base = Base {
            center,
            radius,
            error: tile.geometric_error,
            replace,
            transform,
            y_up,
            depth,
        };
        let extension = match (&tile.implicit_tiling, tile.extensions.get("3DTILES_implicit_tiling")) {
            (None, Some(e)) => {
                Some(serde_json::from_value::<ImplicitJson>(e.clone()).map_err(|e| format!("implicit tiling: {e}"))?)
            }
            _ => None,
        };

        if let Some(implicit) = tile.implicit_tiling.as_ref().or(extension.as_ref()) {
            let tiling = Implicit::new(implicit, url, &uris, volume, &base)?;
            let root = Coord {
                level: 0,
                x: 0,
                y: 0,
                z: 0,
            };

            return Ok(self.placeholder(&Rc::new(tiling), root, None, depth));
        }

        let contents = uris.iter().map(|uri| classify(url, uri)).collect();
        let index = self.push(&base, contents);

        for child in &tile.children {
            let c = self.add_tile(child, url, &transform, replace, y_up, depth + 1)?;

            self.nodes[index].children.push(c);
        }

        Ok(index)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use geo::camera::{Camera, View};

    /// A small REPLACE tileset over Lille: a root, four children, one external tileset
    pub(super) fn tree() -> (Tree, usize) {
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

    pub(super) fn view(zoom: f64) -> View {
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

    /// A sphere tile at a longitude and latitude, as JSON
    pub(super) fn sphere_tile(lon: f64, lat: f64, radius: f64, error: f64, extra: &str) -> String {
        let p = geodesy::to_ecef(lon, lat, 30.0);

        format!(
            r#"{{"boundingVolume":{{"sphere":[{},{},{},{radius}]}},"geometricError":{error}{extra}}}"#,
            p.x, p.y, p.z
        )
    }

    pub(super) fn tileset(root: &str) -> Tree {
        let mut tree = Tree::default();
        let json = format!(r#"{{"asset":{{"version":"1.1"}},"geometricError":500,"root":{root}}}"#);

        tree.add(json.as_bytes(), "https://example.org/t/tileset.json", None)
            .unwrap();

        tree
    }

    #[test]
    fn content_kinds_by_their_bytes() {
        assert_eq!(sniff(b"b3dm...."), Kind::Mesh);
        assert_eq!(sniff(b"glTF...."), Kind::Mesh);
        assert_eq!(sniff(b"subt...."), Kind::Subtree);
        assert_eq!(sniff(b" \n{\"asset\":{}}"), Kind::Json);
        assert_eq!(sniff(b"pnts"), Kind::Mesh);
        assert_eq!(sniff(b"i3dm"), Kind::Other);
        assert_eq!(sniff(b""), Kind::Other);
    }

    #[test]
    fn tilesets_grafting_themselves_are_refused() {
        let (mut tree, _) = tree();
        let external = 4;
        let sub = "https://example.org/mesh/sub/tileset.json?key=1";
        // The external tileset holds a tile whose content is itself
        let p = geodesy::to_ecef(3.07, 50.64, 30.0);
        let json = format!(
            r#"{{"asset":{{"version":"1.0"}},"geometricError":5,"root":{{"boundingVolume":{{"sphere":[{},{},{},800]}},"geometricError":2,"content":{{"uri":"tileset.json"}}}}}}"#,
            p.x, p.y, p.z
        );
        let root = tree.add(json.as_bytes(), sub, Some(external)).unwrap();

        assert_eq!(tree.nodes[root].content, Content::Tileset(sub.into()));

        let before = tree.nodes.len();
        let err = tree.add(json.as_bytes(), sub, Some(root)).unwrap_err();

        assert!(err.contains("includes itself"), "{err}");
        // So is the root tileset, deeper down
        assert!(
            tree.add(
                json.as_bytes(),
                "https://example.org/mesh/tileset.json?key=1",
                Some(root)
            )
            .is_err()
        );
        assert_eq!(tree.nodes.len(), before);
        // Another tileset is grafted, and the same one elsewhere (a sibling, not an ancestor) too
        assert!(
            tree.add(
                json.as_bytes(),
                "https://example.org/mesh/sub/other.json?key=1",
                Some(root)
            )
            .is_ok()
        );
        assert!(tree.add(json.as_bytes(), sub, Some(0)).is_ok());
    }

    #[test]
    fn a_tree_stops_growing_at_its_limit() {
        let (mut tree, root) = tree();

        tree.max_nodes = 7;

        let json = |n: usize| {
            let children = vec![sphere_tile(3.06, 50.63, 100.0, 1.0, ""); n].join(",");

            format!(
                r#"{{"asset":{{"version":"1.0"}},"geometricError":5,"root":{}}}"#,
                sphere_tile(3.06, 50.63, 800.0, 2.0, &format!(r#","children":[{children}]"#))
            )
        };
        // 5 nodes and 3 more: past the limit, refused, nothing added
        let err = tree
            .add(json(2).as_bytes(), "https://example.org/x.json", Some(root))
            .unwrap_err();

        assert!(err.contains("more than 7"), "{err}");
        assert_eq!(tree.nodes.len(), 5);
        assert!(
            tree.add(json(1).as_bytes(), "https://example.org/x.json", Some(root))
                .is_ok()
        );
        assert_eq!(tree.nodes.len(), 7);
        assert!(
            tree.add(json(0).as_bytes(), "https://example.org/y.json", Some(root))
                .is_err()
        );
    }
}
