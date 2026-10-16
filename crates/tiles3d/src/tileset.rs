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

#![forbid(unsafe_code)]

use geo::camera::View;
use geo::geodesy;
use glam::{DMat4, DVec3, DVec4};
use serde::Deserialize;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;
use std::rc::Rc;

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
    /// 3D Tiles 1.1: several contents
    #[serde(default)]
    contents: Vec<ContentJson>,
    #[serde(default)]
    children: Vec<TileJson>,
    transform: Option<[f64; 16]>,
    #[serde(rename = "implicitTiling")]
    implicit_tiling: Option<ImplicitJson>,
    /// 3D Tiles 1.0 extensions of the same (`3DTILES_implicit_tiling`, `3DTILES_multiple_contents`)
    #[serde(default)]
    extensions: HashMap<String, Value>,
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

#[derive(Deserialize)]
struct ImplicitJson {
    #[serde(rename = "subdivisionScheme")]
    subdivision_scheme: String,
    #[serde(rename = "subtreeLevels")]
    subtree_levels: u32,
    #[serde(rename = "availableLevels")]
    available_levels: Option<u32>,
    /// `3DTILES_implicit_tiling` name: the last level rather than the count
    #[serde(rename = "maximumLevel")]
    maximum_level: Option<u32>,
    subtrees: UriJson,
}

#[derive(Deserialize)]
struct UriJson {
    uri: String,
}

/// What the content of a tile is
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    None,
    /// `.b3dm` or `.glb` (or an unknown extension: the decoder tells by the first bytes)
    Mesh(String),
    /// External tileset or implicit-tiling subtree, grafted under the tile once loaded (`Tree::add`)
    Tileset(String),
    /// A format this engine does not draw (points, instances, composites)
    Unsupported,
}

/// What downloaded content is, by its first bytes (its URI may not tell)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `b3dm` or `glTF` binary
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
        Some(b"b3dm" | b"glTF") => Kind::Mesh,
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

#[derive(Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    /// Implicit-tiling placeholders not expanded yet, by node
    pending: HashMap<usize, Placeholder>,
}

/// A bounding volume as the tileset gives it
#[derive(Clone, Copy, Debug)]
enum Volume {
    Sphere([f64; 4]),
    Box([f64; 12]),
    /// Radians and metres, in ECEF already
    Region([f64; 6]),
}

impl Volume {
    fn parse(v: &VolumeJson) -> Result<Self, String> {
        match (v.sphere, v.oriented_box, v.region) {
            (Some(s), _, _) => Ok(Self::Sphere(s)),
            (_, Some(b), _) => Ok(Self::Box(b)),
            (_, _, Some(r)) => Ok(Self::Region(r)),
            _ => Err("bounding volume without sphere, box or region".into()),
        }
    }

    /// Bounding sphere (ECEF)
    fn sphere(&self, transform: &DMat4) -> Result<(DVec3, f64), String> {
        let scale = transform
            .x_axis
            .truncate()
            .length()
            .max(transform.y_axis.truncate().length())
            .max(transform.z_axis.truncate().length());
        let (center, radius) = match *self {
            Self::Sphere([x, y, z, r]) => (transform.transform_point3(DVec3::new(x, y, z)), r * scale),
            Self::Box(b) => {
                let half = DVec3::new(b[3], b[4], b[5]).length_squared()
                    + DVec3::new(b[6], b[7], b[8]).length_squared()
                    + DVec3::new(b[9], b[10], b[11]).length_squared();
                (
                    transform.transform_point3(DVec3::new(b[0], b[1], b[2])),
                    half.sqrt() * scale,
                )
            }
            Self::Region([west, south, east, north, low, high]) => {
                // Regions are in ECEF already (no transform); sample the curved box
                let east = if east < west { east + TAU } else { east };
                let (lon, lat) = (f64::midpoint(west, east), f64::midpoint(south, north));
                let center = geodesy::to_ecef(lon.to_degrees(), lat.to_degrees(), f64::midpoint(low, high));
                let mut radius = 0f64;
                for i in 0..=4 {
                    let x = west + (east - west) * f64::from(i) / 4.0;
                    for j in 0..=4 {
                        let y = south + (north - south) * f64::from(j) / 4.0;
                        for h in [low, high] {
                            let p = geodesy::to_ecef(x.to_degrees(), y.to_degrees(), h);
                            radius = radius.max(p.distance(center));
                        }
                    }
                }
                (center, radius)
            }
        };
        if center.is_finite() && radius.is_finite() && radius >= 0.0 {
            Ok((center, radius))
        } else {
            Err("bad bounding volume".into())
        }
    }

    /// Volume of the implicit tile `t` of a root volume (box or region), halved along x and y (and z for an octree)
    /// at each level
    fn subdivide(&self, t: Coord, octree: bool) -> Self {
        let d = f64::from(t.level).exp2();
        // Center of index `i` along a halved axis, in -1..1
        let f = |i: u32| (2.0 * f64::from(i) + 1.0) / d - 1.0;
        match *self {
            Self::Box(b) => {
                let axis = |k: usize| DVec3::new(b[k], b[k + 1], b[k + 2]);
                let (x, y, z) = (axis(3), axis(6), axis(9));
                let c = axis(0) + x * f(t.x) + y * f(t.y) + if octree { z * f(t.z) } else { DVec3::ZERO };
                let z = if octree { z / d } else { z };
                let (x, y) = (x / d, y / d);
                Self::Box([c.x, c.y, c.z, x.x, x.y, x.z, y.x, y.y, y.z, z.x, z.y, z.z])
            }
            Self::Region([west, south, east, north, low, high]) => {
                let east = if east < west { east + TAU } else { east };
                let part = |lo: f64, hi: f64, i: u32| {
                    let step = (hi - lo) / d;
                    (lo + step * f64::from(i), lo + step * (f64::from(i) + 1.0))
                };
                let (w, e) = part(west, east, t.x);
                let (s, n) = part(south, north, t.y);
                let (l, h) = if octree { part(low, high, t.z) } else { (low, high) };
                Self::Region([w, s, e, n, l, h])
            }
            Self::Sphere(_) => *self,
        }
    }
}

/// Coordinates of an implicit tile
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Coord {
    level: u32,
    x: u32,
    y: u32,
    z: u32,
}

impl Coord {
    fn children(self, octree: bool) -> impl Iterator<Item = Self> {
        let n = if octree { 8 } else { 4 };
        (0..n).map(move |k| Self {
            level: self.level + 1,
            x: self.x * 2 + (k & 1),
            y: self.y * 2 + ((k >> 1) & 1),
            z: self.z * 2 + ((k >> 2) & 1),
        })
    }

    /// Fills a template URI (`{level}`, `{x}`, `{y}`, `{z}`)
    fn fill(self, template: &str) -> String {
        template
            .replace("{level}", &self.level.to_string())
            .replace("{x}", &self.x.to_string())
            .replace("{y}", &self.y.to_string())
            .replace("{z}", &self.z.to_string())
    }
}

/// Morton index: the bits of x, y (and z) interleaved, x lowest
fn morton(x: u32, y: u32, z: u32, octree: bool) -> u64 {
    let k = if octree { 3 } else { 2 };
    let mut m = 0u64;
    for i in 0..32u32 {
        let bit = |v: u32| u64::from((v >> i) & 1);
        let at = i * k;
        if at >= 64 {
            break;
        }
        let put = |v: u32, offset: u32| bit(v).checked_shl(at + offset).unwrap_or(0);
        m |= put(x, 0) | put(y, 1);
        if octree {
            m |= put(z, 2);
        }
    }
    m
}

/// Coordinates of a tile relative to the root of a subtree, `local` levels below it
fn relative(subtree: &Subtree, tile: Coord, local: u32) -> (u32, u32, u32) {
    let r = subtree.root;
    let shift = |v: u32| v.checked_shl(local).unwrap_or(0);
    (
        tile.x.wrapping_sub(shift(r.x)),
        tile.y.wrapping_sub(shift(r.y)),
        tile.z.wrapping_sub(shift(r.z)),
    )
}

/// Deepest subtree file allowed: its bitstreams index `branching^levels` bits in u64
const MAX_SUBTREE_LEVELS: [u32; 2] = [31, 20];
/// Levels of a subtree turned into nodes at once (the deeper ones wait behind placeholders)
const CHUNK_LEVELS: [u32; 2] = [4, 3];
/// Implicit levels: tile coordinates are u32
const MAX_LEVELS: u32 = 31;

/// An implicit tileset: its root tile and how it subdivides
struct Implicit {
    octree: bool,
    subtree_levels: u32,
    available_levels: u32,
    /// Template URIs, resolved against the tileset
    subtrees: String,
    contents: Vec<String>,
    volume: Volume,
    error: f64,
    transform: DMat4,
    replace: bool,
    y_up: bool,
}

impl Implicit {
    fn branching(&self) -> u64 {
        if self.octree { 8 } else { 4 }
    }
}

/// Availability bits of a subtree
enum Bits {
    Constant(bool),
    Stream(Vec<u8>),
}

impl Bits {
    fn get(&self, i: u64) -> bool {
        match self {
            Self::Constant(c) => *c,
            Self::Stream(s) => usize::try_from(i / 8)
                .ok()
                .and_then(|b| s.get(b))
                .is_some_and(|b| b >> (i % 8) & 1 == 1),
        }
    }
}

/// A loaded subtree: availability of its tiles, of their contents, of the subtrees below it
struct Subtree {
    root: Coord,
    url: String,
    tiles: Bits,
    contents: Vec<Bits>,
    children: Bits,
}

/// An implicit node waiting for its expansion: `tile` and below, from `subtree` if already loaded (else its file is
/// the content of the node)
struct Placeholder {
    tiling: Rc<Implicit>,
    tile: Coord,
    subtree: Option<Rc<Subtree>>,
}

/// A JSON chunk without its padding (spaces, zeros)
fn trim_padding(text: &[u8]) -> &[u8] {
    let end = text
        .iter()
        .rposition(|&b| b != 0 && !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    &text[..end]
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    let b = bytes.get(at..at + 8)?;
    Some(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Parses a subtree file (binary `.subtree` or JSON) rooted at `root`
fn parse_subtree(bytes: &[u8], url: &str, root: Coord, tiling: &Implicit) -> Result<Subtree, String> {
    let bad = || "bad subtree".to_string();
    let (json, binary): (Value, &[u8]) = if bytes.starts_with(b"subt") {
        if bytes.get(4..8) != Some(&1u32.to_le_bytes()[..]) {
            return Err("subtree version 1 expected".into());
        }
        let json_length = u64_at(bytes, 8).and_then(|n| usize::try_from(n).ok()).ok_or_else(bad)?;
        let binary_length = u64_at(bytes, 16)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(bad)?;
        let json_end = json_length.checked_add(24).ok_or_else(bad)?;
        let text = bytes.get(24..json_end).ok_or("truncated subtree")?;
        let binary = json_end
            .checked_add(binary_length)
            .and_then(|end| bytes.get(json_end..end))
            .ok_or("truncated subtree")?;
        (
            serde_json::from_slice(trim_padding(text)).map_err(|e| format!("subtree: {e}"))?,
            binary,
        )
    } else {
        (
            serde_json::from_slice(trim_padding(bytes)).map_err(|e| format!("subtree: {e}"))?,
            &[],
        )
    };
    let index = |v: &Value| v.as_u64().and_then(|n| usize::try_from(n).ok());
    let view = |i: usize| -> Result<&[u8], String> {
        let view = &json["bufferViews"][i];
        let buffer = &json["buffers"][index(&view["buffer"]).ok_or_else(bad)?];
        if !buffer.is_object() || !buffer["uri"].is_null() {
            return Err("subtree buffers outside the subtree file are unsupported".into());
        }
        let start = if view["byteOffset"].is_null() {
            0
        } else {
            index(&view["byteOffset"]).ok_or_else(bad)?
        };
        let length = index(&view["byteLength"]).ok_or_else(bad)?;
        start
            .checked_add(length)
            .and_then(|end| binary.get(start..end))
            .ok_or_else(|| "subtree buffer view out of range".into())
    };
    let bits = |v: &Value, count: u64| -> Result<Bits, String> {
        if let Some(c) = v["constant"].as_u64() {
            return Ok(Bits::Constant(c == 1));
        }
        let stream = index(&v["bitstream"])
            .or(index(&v["bufferView"]))
            .ok_or("subtree availability")?;
        let stream = view(stream)?;
        if (stream.len() as u64) < count.div_ceil(8) {
            return Err("subtree availability too short".into());
        }
        Ok(Bits::Stream(stream.to_vec()))
    };
    let n = tiling.branching();
    let (levels, child_bits) = (tiling.subtree_levels, n.pow(tiling.subtree_levels));
    let tile_bits = (child_bits - 1) / (n - 1);
    let contents = match &json["contentAvailability"] {
        Value::Array(a) => a.iter().collect(),
        Value::Null => Vec::new(),
        one => vec![one],
    };
    Ok(Subtree {
        root,
        url: url.to_owned(),
        tiles: bits(&json["tileAvailability"], tile_bits)?,
        contents: (0..tiling.contents.len())
            .map(|k| {
                contents
                    .get(k)
                    .map_or(Ok(Bits::Constant(false)), |c| bits(c, tile_bits))
            })
            .collect::<Result<_, _>>()?,
        children: if levels > 0 {
            bits(&json["childSubtreeAvailability"], child_bits)?
        } else {
            Bits::Constant(false)
        },
    })
}

/// Content of a URI, by its extension; unknown ones are taken for meshes (the decoder checks the first bytes)
fn classify(url: &str, uri: &str) -> Content {
    let absolute = resolve(url, uri);
    let path = absolute.split(['?', '#']).next().unwrap_or("");
    let file = path.rsplit('/').next().unwrap_or("");
    let extension = file.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
    match extension.as_str() {
        "json" => Content::Tileset(absolute),
        "pnts" | "i3dm" | "cmpt" | "gltf" | "vctr" | "geom" | "geojson" => Content::Unsupported,
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
    pub fn add(&mut self, json: &[u8], url: &str, parent: Option<usize>) -> Result<usize, String> {
        if let Some(p) = parent
            && let Some(placeholder) = self.pending.remove(&p)
        {
            return self.expand(p, placeholder, json, url);
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
        let root = self.add_tile(&tileset.root, url, &transform, replace, y_up, depth)?;
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

    /// A node standing for the implicit tile `tile` and below until they are expanded: its content is the subtree
    /// file (the one to download, or the one already in memory)
    fn placeholder(&mut self, tiling: &Rc<Implicit>, tile: Coord, subtree: Option<Rc<Subtree>>, depth: u32) -> usize {
        let url = subtree
            .as_ref()
            .map_or_else(|| tile.fill(&tiling.subtrees), |s| s.url.clone());
        let base = tiling.base(tile, depth);
        let index = self.push(&base, vec![Content::Tileset(url)]);
        self.pending.insert(
            index,
            Placeholder {
                tiling: tiling.clone(),
                tile,
                subtree,
            },
        );
        index
    }

    /// Expands a placeholder: its tile and the levels below it, grafted under it. Returns the tile's node, or the
    /// placeholder itself (left empty) if the tile is not available
    fn expand(&mut self, p: usize, placeholder: Placeholder, bytes: &[u8], url: &str) -> Result<usize, String> {
        let Placeholder { tiling, tile, subtree } = placeholder;
        let subtree = match subtree {
            Some(s) => s,
            None => Rc::new(parse_subtree(bytes, url, tile, &tiling)?),
        };
        // Once expanded the placeholder is a plain empty node
        self.nodes[p].content = Content::None;
        let local = tile.level - subtree.root.level;
        if !subtree.tiles.get(tiling.bit(&subtree, tile, local)) {
            return Ok(p);
        }
        let depth = self.nodes[p].depth + 1;
        let chunk = CHUNK_LEVELS[usize::from(tiling.octree)];
        let node = self.expand_tile(&tiling, &subtree, tile, chunk, depth);
        self.nodes[p].children.push(node);
        Ok(node)
    }

    /// Nodes of an available implicit tile and of its available descendants, `levels` deep; deeper ones and child
    /// subtrees as placeholders
    fn expand_tile(
        &mut self,
        tiling: &Rc<Implicit>,
        subtree: &Rc<Subtree>,
        tile: Coord,
        levels: u32,
        depth: u32,
    ) -> usize {
        let local = tile.level - subtree.root.level;
        let bit = tiling.bit(subtree, tile, local);
        let contents = tiling
            .contents
            .iter()
            .zip(&subtree.contents)
            .filter(|(_, available)| available.get(bit))
            .map(|(template, _)| classify("", &tile.fill(template)))
            .collect();
        let index = self.push(&tiling.base(tile, depth), contents);
        if tile.level + 1 >= tiling.available_levels {
            return index;
        }
        for child in tile.children(tiling.octree) {
            let node = if local + 1 == tiling.subtree_levels {
                // The root of a child subtree
                let (x, y, z) = relative(subtree, child, local + 1);
                subtree
                    .children
                    .get(morton(x, y, z, tiling.octree))
                    .then(|| self.placeholder(tiling, child, None, depth + 1))
            } else if !subtree.tiles.get(tiling.bit(subtree, child, local + 1)) {
                None
            } else if levels > 1 {
                Some(self.expand_tile(tiling, subtree, child, levels - 1, depth + 1))
            } else {
                Some(self.placeholder(tiling, child, Some(subtree.clone()), depth + 1))
            };
            if let Some(node) = node {
                self.nodes[index].children.push(node);
            }
        }
        index
    }
}

impl Implicit {
    fn new(json: &ImplicitJson, url: &str, contents: &[&String], volume: Volume, base: &Base) -> Result<Self, String> {
        let octree = match json.subdivision_scheme.as_str() {
            "QUADTREE" => false,
            "OCTREE" => true,
            other => return Err(format!("implicit tiling {other} unsupported")),
        };
        if matches!(volume, Volume::Sphere(_)) {
            return Err("implicit tiling of a bounding sphere".into());
        }
        let subtree_levels = json.subtree_levels;
        if subtree_levels == 0 || subtree_levels > MAX_SUBTREE_LEVELS[usize::from(octree)] {
            return Err("implicit tiling: bad subtreeLevels".into());
        }
        let available_levels = json
            .available_levels
            .or(json.maximum_level.map(|m| m.saturating_add(1)))
            .ok_or("implicit tiling without availableLevels")?
            .min(MAX_LEVELS);
        if available_levels == 0 {
            return Err("implicit tiling: bad availableLevels".into());
        }
        Ok(Self {
            octree,
            subtree_levels,
            available_levels,
            subtrees: resolve(url, &json.subtrees.uri),
            contents: contents.iter().map(|uri| resolve(url, uri)).collect(),
            volume,
            error: base.error,
            transform: base.transform,
            replace: base.replace,
            y_up: base.y_up,
        })
    }

    /// Bit of a tile in the availability of its subtree: levels one after the other, Morton order within a level
    fn bit(&self, subtree: &Subtree, tile: Coord, local: u32) -> u64 {
        let n = self.branching();
        let (x, y, z) = relative(subtree, tile, local);
        (n.pow(local) - 1) / (n - 1) + morton(x, y, z, self.octree)
    }

    /// Bounding sphere, error and the rest of an implicit tile
    fn base(&self, tile: Coord, depth: u32) -> Base {
        let (center, radius) = self
            .volume
            .subdivide(tile, self.octree)
            .sphere(&self.transform)
            .unwrap_or((DVec3::ZERO, 0.0));
        Base {
            center,
            radius,
            error: self.error / f64::from(tile.level).exp2(),
            replace: self.replace,
            transform: self.transform,
            y_up: self.y_up,
            depth,
        }
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
    use super::*;
    use geo::camera::Camera;

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

    /// A sphere tile at a longitude and latitude, as JSON
    fn sphere_tile(lon: f64, lat: f64, radius: f64, error: f64, extra: &str) -> String {
        let p = geodesy::to_ecef(lon, lat, 30.0);
        format!(
            r#"{{"boundingVolume":{{"sphere":[{},{},{},{radius}]}},"geometricError":{error}{extra}}}"#,
            p.x, p.y, p.z
        )
    }

    fn tileset(root: &str) -> Tree {
        let mut tree = Tree::default();
        let json = format!(r#"{{"asset":{{"version":"1.1"}},"geometricError":500,"root":{root}}}"#);
        tree.add(json.as_bytes(), "https://example.org/t/tileset.json", None)
            .unwrap();
        tree
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
    fn region_and_box_volumes_bound_their_tiles() {
        let (w, s, e, n) = (
            3.0f64.to_radians(),
            50.58f64.to_radians(),
            3.12f64.to_radians(),
            50.68f64.to_radians(),
        );
        let region = Volume::Region([w, s, e, n, 0.0, 200.0]);
        let (center, radius) = region.sphere(&DMat4::IDENTITY).unwrap();
        for lon in [w, e] {
            for lat in [s, n] {
                for h in [0.0, 200.0] {
                    let p = geodesy::to_ecef(lon.to_degrees(), lat.to_degrees(), h);
                    assert!(p.distance(center) <= radius + 1e-6);
                }
            }
        }
        assert!(radius < 7000.0, "{radius}");
        // A box 2 km × 2 km × 100 m, moved by its transform
        let c = geodesy::to_ecef(3.06, 50.63, 0.0);
        let [east, north, up] = geodesy::enu(3.06, 50.63);
        let (x, y, z) = (east * 1000.0, north * 1000.0, up * 50.0);
        let b = Volume::Box([0.0, 0.0, 0.0, x.x, x.y, x.z, y.x, y.y, y.z, z.x, z.y, z.z]);
        let (center, radius) = b.sphere(&DMat4::from_translation(c)).unwrap();
        assert!(center.distance(c) < 1e-6 && (radius - (2e6f64 + 2500.0).sqrt()).abs() < 1e-6);
        // Implicit subdivision: the quadtree tile (1, 1, 0) is the east-south quarter
        let Volume::Box(q) = b.subdivide(
            Coord {
                level: 1,
                x: 1,
                y: 0,
                z: 0,
            },
            false,
        ) else {
            panic!()
        };
        let qc = DVec3::new(q[0], q[1], q[2]);
        assert!((qc - (x * 0.5 - y * 0.5)).length() < 1e-9);
        assert!((DVec3::new(q[3], q[4], q[5]) - x * 0.5).length() < 1e-9);
        assert!(
            (DVec3::new(q[9], q[10], q[11]) - z).length() < 1e-9,
            "z kept by a quadtree"
        );
        let Volume::Region(r) = region.subdivide(
            Coord {
                level: 2,
                x: 3,
                y: 0,
                z: 1,
            },
            true,
        ) else {
            panic!()
        };
        assert!(
            (r[0] - (w + (e - w) * 0.75)).abs() < 1e-12 && (r[4] - 50.0).abs() < 1e-9 && (r[5] - 100.0).abs() < 1e-9
        );
        // Bad volumes are errors, not NaN spheres
        assert!(
            Volume::Sphere([f64::NAN, 0.0, 0.0, 1.0])
                .sphere(&DMat4::IDENTITY)
                .is_err()
        );
    }

    /// A quadtree over a region around Lille: subtrees of 2 levels, 4 levels in all
    fn implicit() -> Tree {
        let r = |d: f64| d.to_radians();
        let json = format!(
            r#"{{"asset":{{"version":"1.1"}},"geometricError":1000,"root":{{
                "boundingVolume":{{"region":[{},{},{},{},0,100]}},"geometricError":500,"refine":"REPLACE",
                "content":{{"uri":"content/{{level}}/{{x}}/{{y}}.glb"}},
                "implicitTiling":{{"subdivisionScheme":"QUADTREE","subtreeLevels":2,"availableLevels":4,
                    "subtrees":{{"uri":"subtrees/{{level}}/{{x}}/{{y}}.subtree"}}}}}}}}"#,
            r(3.0),
            r(50.58),
            r(3.12),
            r(50.68)
        );
        let mut tree = Tree::default();
        tree.add(json.as_bytes(), "https://example.org/implicit/tileset.json?key=1", None)
            .unwrap();
        tree
    }

    /// A binary subtree file
    fn subtree_file(json: &str, binary: &[u8]) -> Vec<u8> {
        let mut text = json.as_bytes().to_vec();
        while !text.len().is_multiple_of(8) {
            text.push(b' ');
        }
        let mut out = b"subt".to_vec();
        out.extend(1u32.to_le_bytes());
        out.extend((text.len() as u64).to_le_bytes());
        out.extend((binary.len() as u64).to_le_bytes());
        out.extend(text);
        out.extend(binary);
        out
    }

    #[test]
    fn implicit_tiling_grows_from_its_subtrees() {
        let mut tree = implicit();
        assert_eq!(tree.nodes.len(), 1);
        let url = |s: &str| format!("https://example.org/implicit/{s}?key=1");
        assert_eq!(tree.nodes[0].content, Content::Tileset(url("subtrees/0/0/0.subtree")));
        // Subtree 0/0/0: the root (no content), tiles 1/0/0 and 1/1/1 (with content), subtree 2/0/0 below
        let mut binary = vec![0u8; 24];
        binary[0] = 0b1_0011;
        binary[8] = 0b1_0010;
        binary[16] = 0b1;
        let file = subtree_file(
            r#"{"buffers":[{"byteLength":24}],"bufferViews":[
                {"buffer":0,"byteOffset":0,"byteLength":1},{"buffer":0,"byteOffset":8,"byteLength":1},
                {"buffer":0,"byteOffset":16,"byteLength":2}],
                "tileAvailability":{"bitstream":0},"contentAvailability":[{"bitstream":1}],
                "childSubtreeAvailability":{"bitstream":2}}"#,
            &binary,
        );
        assert!(!tree.expandable(0));
        let root = tree.add(&file, &url("subtrees/0/0/0.subtree"), Some(0)).unwrap();
        assert_eq!(tree.nodes[0].content, Content::None, "the placeholder, once expanded");
        assert_eq!(tree.nodes[0].children, [root]);
        assert_eq!(tree.nodes[root].content, Content::None);
        let children: Vec<&Node> = tree.nodes[root].children.iter().map(|&c| &tree.nodes[c]).collect();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].content, Content::Mesh(url("content/1/0/0.glb")));
        assert_eq!(children[1].content, Content::Mesh(url("content/1/1/1.glb")));
        assert!((children[0].error - 250.0).abs() < 1e-9);
        // The south-west child is west and south of the north-east one
        let (a, b) = (children[0].geodetic, children[1].geodetic);
        assert!(a.x < b.x && a.y < b.y, "{a} {b}");
        // Below 1/0/0, the placeholder of subtree 2/0/0
        assert_eq!(children[0].children.len(), 1);
        assert_eq!(children[1].children, Vec::<usize>::new());
        let placeholder = children[0].children[0];
        assert_eq!(
            tree.nodes[placeholder].content,
            Content::Tileset(url("subtrees/2/0/0.subtree"))
        );
        // Subtree 2/0/0 in JSON, everything available: its root and 4 children, the last level
        let before = tree.nodes.len();
        let constant = br#"{"tileAvailability":{"constant":1},"contentAvailability":[{"constant":1}],
            "childSubtreeAvailability":{"constant":0}}"#;
        let sub = tree
            .add(constant, &url("subtrees/2/0/0.subtree"), Some(placeholder))
            .unwrap();
        assert_eq!(tree.nodes.len(), before + 5);
        assert_eq!(tree.nodes[sub].content, Content::Mesh(url("content/2/0/0.glb")));
        assert_eq!(tree.nodes[sub].children.len(), 4);
        assert!(
            tree.nodes[sub]
                .children
                .iter()
                .all(|&c| tree.nodes[c].children.is_empty())
        );
        // The traversal goes through the placeholders down to the fine tiles
        let mut c = Camera::new(3.015, 50.5925, 17.0);
        c.width = 1200.0;
        c.height = 800.0;
        let s = select(&tree, 0, &c.view(), 16.0, &|i| match tree.nodes[i].content {
            Content::Mesh(_) => Readiness::Ready,
            _ => Readiness::Empty,
        });
        assert!(s.draw.iter().any(|&i| tree.nodes[i].depth >= 4), "{s:?}");
    }

    #[test]
    fn deep_subtrees_expand_a_few_levels_at_a_time() {
        let r = |d: f64| d.to_radians();
        let json = format!(
            r#"{{"asset":{{"version":"1.1"}},"geometricError":1000,"root":{{
                "boundingVolume":{{"region":[{},{},{},{},0,100]}},"geometricError":500,
                "content":{{"uri":"c/{{level}}/{{x}}/{{y}}/{{z}}"}},
                "implicitTiling":{{"subdivisionScheme":"OCTREE","subtreeLevels":6,"availableLevels":6,
                    "subtrees":{{"uri":"s/{{level}}/{{x}}/{{y}}/{{z}}.json"}}}}}}}}"#,
            r(3.0),
            r(50.58),
            r(3.12),
            r(50.68)
        );
        let mut tree = Tree::default();
        tree.add(json.as_bytes(), "https://example.org/o/tileset.json", None)
            .unwrap();
        let constant = br#"{"tileAvailability":{"constant":1},"contentAvailability":[{"constant":1}],
            "childSubtreeAvailability":{"constant":0}}"#;
        tree.add(constant, "https://example.org/o/s/0/0/0/0.json", Some(0))
            .unwrap();
        // 3 levels (1 + 8 + 64 tiles), then 512 placeholders expanding from memory
        assert_eq!(tree.nodes.len(), 1 + 73 + 512);
        // Contents without extension are taken for meshes (the decoder tells by the bytes)
        assert_eq!(
            tree.nodes[1].content,
            Content::Mesh("https://example.org/o/c/0/0/0/0".into())
        );
        let placeholder = tree.nodes.len() - 1;
        assert!(tree.expandable(placeholder));
        let before = tree.nodes.len();
        let tile = tree.add(&[], "", Some(placeholder)).unwrap();
        assert_eq!(tree.nodes.len(), before + 73);
        assert_eq!(
            tree.nodes[tile].content,
            Content::Mesh("https://example.org/o/c/3/7/7/7".into())
        );
        assert!(!tree.expandable(placeholder));
    }

    #[test]
    fn malformed_implicit_tilesets_are_errors() {
        let mut tree = implicit();
        // Too short a bitstream, an external buffer, garbage
        let short = subtree_file(
            r#"{"buffers":[{"byteLength":1}],"bufferViews":[{"buffer":0,"byteLength":1}],
                "tileAvailability":{"bitstream":0},"childSubtreeAvailability":{"bitstream":0}}"#,
            &[0xff],
        );
        assert!(tree.add(&short, "", Some(0)).is_err());
        let mut tree = implicit();
        let external = br#"{"buffers":[{"byteLength":8,"uri":"a.bin"}],"bufferViews":[{"buffer":0,"byteLength":8}],
            "tileAvailability":{"bitstream":0},"childSubtreeAvailability":{"constant":0}}"#;
        assert!(tree.add(external, "", Some(0)).is_err());
        let mut tree = implicit();
        assert!(
            tree.add(b"subt\x01\0\0\0\xff\xff\xff\xff\xff\xff\xff\xff", "", Some(0))
                .is_err()
        );
        // Schemes and levels
        for implicit in [
            r#""subdivisionScheme":"S2","subtreeLevels":2,"availableLevels":2"#,
            r#""subdivisionScheme":"QUADTREE","subtreeLevels":0,"availableLevels":2"#,
            r#""subdivisionScheme":"QUADTREE","subtreeLevels":40,"availableLevels":2"#,
            r#""subdivisionScheme":"QUADTREE","subtreeLevels":2,"availableLevels":0"#,
        ] {
            let json = format!(
                r#"{{"asset":{{"version":"1.1"}},"geometricError":1,"root":{{"boundingVolume":{{"region":[0,0,0.1,0.1,0,1]}},
                "geometricError":1,"implicitTiling":{{{implicit},"subtrees":{{"uri":"s"}}}}}}}}"#
            );
            assert!(Tree::default().add(json.as_bytes(), "", None).is_err(), "{implicit}");
        }
    }

    #[test]
    fn mutated_subtrees_never_panic() {
        let mut binary = vec![0u8; 24];
        binary[0] = 0b1_0011;
        binary[8] = 0b1_0010;
        binary[16] = 0b1;
        let file = subtree_file(
            r#"{"buffers":[{"byteLength":24}],"bufferViews":[
                {"buffer":0,"byteOffset":0,"byteLength":1},{"buffer":0,"byteOffset":8,"byteLength":1},
                {"buffer":0,"byteOffset":16,"byteLength":2}],
                "tileAvailability":{"bitstream":0},"contentAvailability":[{"bitstream":1}],
                "childSubtreeAvailability":{"bitstream":2}}"#,
            &binary,
        );
        let mut r = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move |n: usize| {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            (r % n as u64) as usize
        };
        for _ in 0..3000 {
            let mut bytes = file.clone();
            for _ in 0..=next(4) {
                let i = next(bytes.len());
                bytes[i] = [0, 0xff, b'9', b'"', b'{', next(256) as u8][next(6)];
            }
            bytes.truncate(bytes.len() - next(8));
            let mut tree = implicit();
            if tree.add(&bytes, "", Some(0)).is_ok() {
                let _ = select(&tree, 0, &view(16.0), 16.0, &|_| Readiness::Pending);
            }
        }
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

    #[test]
    fn content_kinds_by_their_bytes() {
        assert_eq!(sniff(b"b3dm...."), Kind::Mesh);
        assert_eq!(sniff(b"glTF...."), Kind::Mesh);
        assert_eq!(sniff(b"subt...."), Kind::Subtree);
        assert_eq!(sniff(b" \n{\"asset\":{}}"), Kind::Json);
        assert_eq!(sniff(b"pnts"), Kind::Other);
        assert_eq!(sniff(b""), Kind::Other);
    }
}
