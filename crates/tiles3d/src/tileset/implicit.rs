//! Implicit tiling (3D Tiles 1.1, and `3DTILES_implicit_tiling`): QUADTREE and OCTREE tilesets whose tiles are
//! given by the availability bits of subtree files rather than listed. Placeholders stand for what is not expanded
//! yet; a subtree becomes nodes a few levels at a time.

use super::json::ImplicitJson;
use super::volume::Volume;
use super::{Base, Content, Tree, classify, resolve};
use crate::bytes::trim_padding;
use glam::{DMat4, DVec3};
use serde_json::Value;
use std::rc::Rc;

/// Coordinates of an implicit tile
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Coord {
    /// Level in the whole tiling, 0 at its root tile
    pub(super) level: u32,
    /// Column at that level, 0 to 2^level - 1
    pub(super) x: u32,
    /// Row at that level, 0 to 2^level - 1
    pub(super) y: u32,
    /// Height at that level, 0 to 2^level - 1 (always 0 in a quadtree)
    pub(super) z: u32,
}

impl Coord {
    /// Its 4 (quadtree) or 8 (octree) children, in Morton order
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
pub(super) struct Implicit {
    /// OCTREE, else QUADTREE
    octree: bool,
    /// Levels of each subtree file
    subtree_levels: u32,
    /// Levels of the whole tiling (at most `MAX_LEVELS`)
    available_levels: u32,
    /// Template URI of the subtree files, resolved against the tileset
    subtrees: String,
    /// Template URIs of the contents, resolved against the tileset
    contents: Vec<String>,
    /// Bounding volume of the root tile, halved at each level
    volume: Volume,
    /// Geometric error of the root tile (metres), halved at each level
    error: f64,
    /// World transform of the root tile, shared by all
    transform: DMat4,
    /// REPLACE refinement, shared by all
    replace: bool,
    /// glTF y-up, shared by all
    y_up: bool,
}

impl Implicit {
    /// Children per tile: 4 or 8
    fn branching(&self) -> u64 {
        if self.octree { 8 } else { 4 }
    }
}

/// Availability bits of a subtree
enum Bits {
    /// Every bit the same (`constant`)
    Constant(bool),
    /// A bitstream, least significant bit first in each byte
    Stream(Vec<u8>),
}

impl Bits {
    /// Bit `i`, false past the end of the stream
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
pub(super) struct Subtree {
    /// Its root tile
    root: Coord,
    /// Its file, resolved
    url: String,
    /// Availability of its tiles (`tileAvailability`), level after level, Morton order within one
    tiles: Bits,
    /// Availability of each content of its tiles (`contentAvailability`), in the order of `Implicit::contents`
    contents: Vec<Bits>,
    /// Availability of the subtrees below its last level (`childSubtreeAvailability`), Morton order
    children: Bits,
}

/// An implicit node waiting for its expansion: `tile` and below, from `subtree` if already loaded (else its file is
/// the content of the node)
pub(super) struct Placeholder {
    /// The tiling it belongs to
    tiling: Rc<Implicit>,
    /// The implicit tile it stands for
    tile: Coord,
    /// The subtree holding `tile`, None while its file is to download
    pub(super) subtree: Option<Rc<Subtree>>,
}

/// The little-endian u64 at `at`, None past the end
fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    let b = bytes.get(at..at + 8)?;

    Some(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Parses a subtree file (binary `.subtree` or JSON) rooted at `root` (3D Tiles 1.1, Implicit Tiling, Subtrees)
fn parse_subtree(bytes: &[u8], url: &str, root: Coord, tiling: &Implicit) -> Result<Subtree, String> {
    let bad = || "bad subtree".to_string();
    // Binary format: magic `subt`, version 1, JSON length and binary length (u64), then the JSON and binary chunks;
    // a JSON subtree has no binary chunk (its buffers would be external, unsupported)
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
    // Buffer views: only the internal buffer (the binary chunk, no `uri`) is read
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
    // Availability: a `constant` (0 or 1), or a `bitstream` buffer view of at least `count` bits (`bufferView` in
    // `3DTILES_implicit_tiling`)
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
    // Bit counts: (n^levels - 1) / (n - 1) tiles over the subtree's levels, n^levels child subtrees below its last
    let n = tiling.branching();
    let (levels, child_bits) = (tiling.subtree_levels, n.pow(tiling.subtree_levels));
    let tile_bits = (child_bits - 1) / (n - 1);
    // One content availability per content of the tiles (an object alone in `3DTILES_implicit_tiling`)
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

impl Tree {
    /// A node standing for the implicit tile `tile` and below until they are expanded: its content is the subtree
    /// file (the one to download, or the one already in memory)
    pub(super) fn placeholder(
        &mut self,
        tiling: &Rc<Implicit>,
        tile: Coord,
        subtree: Option<Rc<Subtree>>,
        depth: u32,
    ) -> usize {
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
    pub(super) fn expand(
        &mut self,
        p: usize,
        placeholder: Placeholder,
        bytes: &[u8],
        url: &str,
    ) -> Result<usize, String> {
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
    /// The implicit tiling of a tile: its scheme and levels checked, its templates resolved against the tileset's
    /// `url`, its root's volume and `base` fields kept for the tiles below. Errors on a sphere or bad levels
    pub(super) fn new(
        json: &ImplicitJson,
        url: &str,
        contents: &[&String],
        volume: Volume,
        base: &Base,
    ) -> Result<Self, String> {
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

#[cfg(test)]
mod tests {
    use super::super::tests::view;
    use super::super::{Node, Readiness, select};
    use super::*;
    use geo::camera::Camera;

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

    /// Subtree 0/0/0 of `implicit()`: the root (no content), tiles 1/0/0 and 1/1/1 (with content), subtree 2/0/0 below
    fn root_subtree() -> Vec<u8> {
        let mut binary = vec![0u8; 24];

        binary[0] = 0b1_0011;
        binary[8] = 0b1_0010;
        binary[16] = 0b1;

        subtree_file(
            r#"{"buffers":[{"byteLength":24}],"bufferViews":[
                {"buffer":0,"byteOffset":0,"byteLength":1},{"buffer":0,"byteOffset":8,"byteLength":1},
                {"buffer":0,"byteOffset":16,"byteLength":2}],
                "tileAvailability":{"bitstream":0},"contentAvailability":[{"bitstream":1}],
                "childSubtreeAvailability":{"bitstream":2}}"#,
            &binary,
        )
    }

    #[test]
    fn implicit_tiling_grows_from_its_subtrees() {
        let mut tree = implicit();

        assert_eq!(tree.nodes.len(), 1);

        let url = |s: &str| format!("https://example.org/implicit/{s}?key=1");

        assert_eq!(tree.nodes[0].content, Content::Tileset(url("subtrees/0/0/0.subtree")));

        let file = root_subtree();

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
        let file = root_subtree();
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
}
