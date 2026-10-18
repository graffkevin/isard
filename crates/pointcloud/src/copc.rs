//! COPC: the info VLR (the octree's cube, the root spacing, the root hierarchy page), the hierarchy pages and the
//! octree they describe. A page lists nodes by key (depth, x, y, z) with the place of their LAZ chunk in the file and
//! their point count; a count of -1 points to a page of its own, loaded once the traversal reaches it
//! (`Hierarchy::add_page`), as implicit tiling grows a 3D Tiles tree.

use crate::las::{Header, f64_at, i32_at, u32_at, u64_at};
use glam::DVec3;
use std::collections::HashMap;

/// The COPC info VLR (user `copc`, record 1)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Info {
    /// Center of the octree's cube and half its side (CRS units)
    pub center: [f64; 3],
    pub halfsize: f64,
    /// Distance between the points of the root node (CRS units), halved at each level
    pub spacing: f64,
    pub root_page: Page,
}

/// Where a hierarchy page is in the file
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub offset: u64,
    pub size: u64,
}

/// The head of a COPC file: its header, the COPC info, the LAZ items, the CRS
#[derive(Clone, Debug, PartialEq)]
pub struct Copc {
    pub header: Header,
    pub info: Info,
    /// Data of the LAZ VLR (user `laszip encoded`, record 22204): how the chunks are compressed
    pub laz: Vec<u8>,
    /// The WKT of the CRS (user `LASF_Projection`, record 2112), if the VLRs hold it
    pub wkt: Option<String>,
}

/// Bytes a page may hold (32 bytes per entry: 128 Ki entries); a larger one is refused before it is fetched
pub const MAX_PAGE: u64 = 4 << 20;
/// Most nodes of a hierarchy: a page past it is refused
pub const MAX_NODES: usize = 1 << 20;
/// Deepest level read: 2^20 nodes along an axis, far past any file
pub const MAX_DEPTH: u32 = 20;

/// What `Copc::parse` found in the first bytes of a file
#[derive(Debug, PartialEq)]
pub enum Head {
    Ready(Box<Copc>),
    /// The VLRs run past the bytes given: the first `n` bytes are needed
    More(usize),
}

impl Copc {
    /// Reads the head of a COPC file from its first bytes: the header and the VLRs, which end at
    /// `point_data_offset` (`Head::More` when the bytes stop before)
    pub fn parse(bytes: &[u8]) -> Result<Head, String> {
        let header = Header::parse(bytes)?;
        if !(6..=8).contains(&header.point_format) {
            return Err(format!(
                "point format {}: COPC holds formats 6 to 8",
                header.point_format
            ));
        }
        let end = header.point_data_offset as usize;
        if bytes.len() < end {
            return Ok(Head::More(end));
        }
        let vlrs = header.vlrs(bytes)?;
        let find = |user: &str, record: u16| vlrs.iter().find(|v| v.user == user && v.record == record);
        let info = find("copc", 1).ok_or("no COPC info VLR: not a COPC file")?;
        let d = info.data;
        let truncated = || "truncated COPC info".to_owned();
        let [cx, cy, cz, halfsize, spacing] = [0, 8, 16, 24, 32].map(|at| f64_at(d, at).unwrap_or(f64::NAN));
        let root_page = Page {
            offset: u64_at(d, 40).ok_or_else(truncated)?,
            size: u64_at(d, 48).ok_or_else(truncated)?,
        };
        if ![cx, cy, cz, halfsize, spacing].iter().all(|v| v.is_finite()) || halfsize <= 0.0 || spacing <= 0.0 {
            return Err("bad COPC info".into());
        }
        let laz = find("laszip encoded", 22204).ok_or("no LAZ VLR")?;
        let wkt = find("LASF_Projection", 2112).map(|v| crate::las::text(v.data));
        Ok(Head::Ready(Box::new(Self {
            header,
            info: Info {
                center: [cx, cy, cz],
                halfsize,
                spacing,
                root_page,
            },
            laz: laz.data.to_vec(),
            wkt: wkt.filter(|w| !w.is_empty()),
        })))
    }
}

/// A node of the octree: its depth and position along each axis at that depth (0 to 2^depth - 1)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub depth: u32,
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

impl Key {
    pub const ROOT: Self = Self {
        depth: 0,
        x: 0,
        y: 0,
        z: 0,
    };

    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        (self.depth > 0).then(|| Self {
            depth: self.depth - 1,
            x: self.x >> 1,
            y: self.y >> 1,
            z: self.z >> 1,
        })
    }

    /// Whether `self` is `other` or below it
    #[must_use]
    pub fn within(&self, other: &Self) -> bool {
        let Some(shift) = self.depth.checked_sub(other.depth) else {
            return false;
        };
        (self.x >> shift, self.y >> shift, self.z >> shift) == (other.x, other.y, other.z)
    }

    /// Its cube in the octree of `info`: minimum corner and side (CRS units)
    #[must_use]
    pub fn cube(&self, info: &Info) -> ([f64; 3], f64) {
        let side = 2.0 * info.halfsize / f64::from(1u32 << self.depth.min(31));
        let index = [self.x, self.y, self.z];
        let min = [0, 1, 2].map(|i| info.center[i] - info.halfsize + f64::from(index[i]) * side);
        (min, side)
    }
}

/// An entry of a hierarchy page
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: Key,
    /// Where its chunk is in the file (or its page, `count` -1), and its size
    pub offset: u64,
    pub size: u32,
    /// Points of its chunk; -1: the entry is a page
    pub count: i32,
}

/// Bytes of an entry
const ENTRY: usize = 32;

/// The entries of a hierarchy page; a malformed entry (a key out of its level, a negative size) is an error
pub fn parse_page(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    if !bytes.len().is_multiple_of(ENTRY) {
        return Err("hierarchy page of a wrong size".into());
    }
    bytes
        .as_chunks::<ENTRY>()
        .0
        .iter()
        .map(|e| {
            let at = |i| i32_at(e, i).unwrap_or(-1);
            let (depth, x, y, z) = (at(0), at(4), at(8), at(12));
            let (Ok(depth), Ok(x), Ok(y), Ok(z)) = (
                u32::try_from(depth),
                u32::try_from(x),
                u32::try_from(y),
                u32::try_from(z),
            ) else {
                return Err("negative key in a hierarchy page".to_owned());
            };
            if depth > MAX_DEPTH || [x, y, z].iter().any(|&v| v >> depth != 0) {
                return Err("key out of its level in a hierarchy page".into());
            }
            let size = i32_at(e, 24).unwrap_or(-1);
            let count = i32_at(e, 28).unwrap_or(-2);
            if size < 0 || count < -1 {
                return Err("bad entry in a hierarchy page".into());
            }
            Ok(Entry {
                key: Key { depth, x, y, z },
                offset: u64_at(e, 16).unwrap_or(0),
                size: u32_at(e, 24).unwrap_or(0),
                count,
            })
        })
        .collect()
}

/// What a node holds
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    /// A LAZ chunk of `count` points at `offset`, `size` bytes
    Points { offset: u64, size: u32, count: u32 },
    /// Its page, not loaded yet: the node and those below it are in there
    Page(Page),
    /// No point (a node of no points, or one missing between a node and its parent)
    Empty,
}

pub struct Node {
    pub key: Key,
    pub content: Content,
    pub children: Vec<usize>,
    /// Where it is, set by the reader (`Hierarchy::place`): longitude, latitude (degrees) and ellipsoidal height of
    /// its center, the radius of its bounding sphere and the spacing of its points (metres)
    pub geodetic: DVec3,
    pub radius: f64,
    pub spacing: f64,
}

/// The octree of a COPC file, as far as its pages are loaded
pub struct Hierarchy {
    pub info: Info,
    pub nodes: Vec<Node>,
    index: HashMap<Key, usize>,
    /// `MAX_NODES` (fewer in the tests)
    max_nodes: usize,
}

impl Hierarchy {
    /// The root alone, its page to load (node 0)
    #[must_use]
    pub fn new(info: Info) -> Self {
        let root = Node {
            key: Key::ROOT,
            content: Content::Page(info.root_page),
            children: Vec::new(),
            geodetic: DVec3::ZERO,
            radius: 0.0,
            spacing: 0.0,
        };
        Self {
            info,
            nodes: vec![root],
            index: HashMap::from([(Key::ROOT, 0)]),
            max_nodes: MAX_NODES,
        }
    }

    /// The page of a node, to load (its content `Page`)
    #[must_use]
    pub fn page(&self, node: usize) -> Option<Page> {
        match self.nodes.get(node)?.content {
            Content::Page(page) => Some(page),
            _ => None,
        }
    }

    /// Adds the entries of the page of `node`: the node itself and those below it (an entry outside its subtree is
    /// left out). Nodes missing between an entry and its parent are added empty. Returns the nodes added, to place.
    /// A page past `MAX_NODES` is refused whole; the node is then empty
    pub fn add_page(&mut self, node: usize, mut entries: Vec<Entry>) -> Result<Vec<usize>, String> {
        let Some(top) = self.nodes.get(node).map(|n| n.key) else {
            return Err("no such node".into());
        };
        // The page is consumed whatever happens: the node holds what the page says of it, or nothing
        self.nodes[node].content = Content::Empty;
        entries.retain(|e| e.key.within(&top));
        if self.nodes.len().saturating_add(entries.len()) > self.max_nodes {
            return Err(format!("point cloud: more than {} nodes", self.max_nodes));
        }
        // In any order: a child before its parent adds it empty, its entry then fills it
        let mut added = Vec::new();
        for e in entries {
            let content = match e.count {
                -1 if e.size > 0 && u64::from(e.size) <= MAX_PAGE => Content::Page(Page {
                    offset: e.offset,
                    size: u64::from(e.size),
                }),
                1.. if e.size > 0 => Content::Points {
                    offset: e.offset,
                    size: e.size,
                    count: e.count.unsigned_abs(),
                },
                _ => Content::Empty,
            };
            // The page's own node (or one it lists twice) takes the content
            if let Some(&i) = self.index.get(&e.key) {
                if i == node || self.nodes[i].content == Content::Empty {
                    self.nodes[i].content = content;
                }
                continue;
            }
            let i = self.insert(e.key, &mut added);
            self.nodes[i].content = content;
        }
        Ok(added)
    }

    /// The node of a key, added with the empty nodes missing above it
    fn insert(&mut self, key: Key, added: &mut Vec<usize>) -> usize {
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        // Under the root, always there: the recursion ends
        let parent = key.parent().map_or(0, |p| self.insert(p, added));
        let i = self.nodes.len();
        self.nodes.push(Node {
            key,
            content: Content::Empty,
            children: Vec::new(),
            geodetic: DVec3::ZERO,
            radius: 0.0,
            spacing: 0.0,
        });
        self.nodes[parent].children.push(i);
        self.index.insert(key, i);
        added.push(i);
        i
    }

    /// The center of a node's cube (CRS units), to place
    #[must_use]
    pub fn center(&self, node: usize) -> [f64; 3] {
        let (min, side) = self.nodes[node].key.cube(&self.info);
        min.map(|m| m + side / 2.0)
    }

    /// Places a node: its center's longitude, latitude and ellipsoidal height, and the metres of a CRS unit
    /// horizontally and vertically (its radius and spacing follow)
    pub fn place(&mut self, node: usize, geodetic: DVec3, metres: f64, vertical: f64) {
        let n = &mut self.nodes[node];
        let (_, side) = n.key.cube(&self.info);
        let half = side / 2.0;
        n.geodetic = geodetic;
        n.radius = (2.0 * (half * metres).powi(2) + (half * vertical).powi(2)).sqrt();
        n.spacing = self.info.spacing * metres / f64::from(1u32 << n.key.depth.min(31));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARIS: &[u8] = include_bytes!("../tests/data/paris_header.bin");
    const PARIS_PAGE: &[u8] = include_bytes!("../tests/data/paris_page.bin");
    const AUTZEN: &[u8] = include_bytes!("../tests/data/autzen_header.bin");

    fn paris() -> Copc {
        match Copc::parse(PARIS).unwrap() {
            Head::Ready(copc) => *copc,
            Head::More(_) => panic!("the whole head is there"),
        }
    }

    #[test]
    fn reads_the_head_of_a_lidar_hd_tile() {
        let copc = paris();
        assert_eq!(copc.info.center, [652_500.0, 6_861_500.0, 522.21]);
        assert_eq!(copc.info.halfsize, 500.0);
        assert!((copc.info.spacing - 6.802_721).abs() < 1e-6);
        assert_eq!(
            copc.info.root_page,
            Page {
                offset: 102_014_894,
                size: 12_832
            }
        );
        assert_eq!(copc.laz.len(), 40);
        assert!(copc.wkt.as_deref().unwrap().contains("Lambert-93"));
        // Cut before the end of the VLRs: the bytes needed
        assert_eq!(Copc::parse(&PARIS[..600]).unwrap(), Head::More(1418));
    }

    #[test]
    fn reads_the_head_of_autzen_with_its_colors() {
        let Head::Ready(copc) = Copc::parse(AUTZEN).unwrap() else {
            panic!("the whole head is there");
        };
        assert_eq!((copc.header.point_format, copc.header.record_length), (7, 36));
        assert!(copc.wkt.as_deref().unwrap().contains("Oregon GIC Lambert (ft)"));
    }

    #[test]
    fn reads_the_root_page() {
        let entries = parse_page(PARIS_PAGE).unwrap();
        assert_eq!(entries.len(), 401);
        let root = entries.iter().find(|e| e.key == Key::ROOT).unwrap();
        assert_eq!((root.offset, root.size, root.count), (101_315_967, 697_096, 62_403));
        assert!(entries.iter().all(|e| e.count > 0 && e.key.depth <= 5));
        let mut h = Hierarchy::new(paris().info);
        let added = h.add_page(0, entries).unwrap();
        assert_eq!(added.len(), 400);
        assert_eq!(h.nodes.len(), 401);
        assert_eq!(
            h.nodes[0].content,
            Content::Points {
                offset: 101_315_967,
                size: 697_096,
                count: 62_403
            }
        );
        // Every node under its parent, each child one level deeper
        for (i, n) in h.nodes.iter().enumerate() {
            for &c in &n.children {
                assert_eq!(h.nodes[c].key.parent(), Some(n.key), "{i}");
            }
        }
        let total: u64 = h
            .nodes
            .iter()
            .map(|n| match n.content {
                Content::Points { count, .. } => u64::from(count),
                _ => 0,
            })
            .sum();
        assert_eq!(total, paris().header.point_count);
    }

    #[test]
    fn cubes_halve_at_each_level() {
        let info = paris().info;
        let close = |(a, s): ([f64; 3], f64), (b, t): ([f64; 3], f64)| {
            (s - t).abs() < 1e-9 && a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-9)
        };
        assert!(close(Key::ROOT.cube(&info), ([652_000.0, 6_861_000.0, 22.21], 1000.0)));
        let k = Key {
            depth: 2,
            x: 3,
            y: 1,
            z: 0,
        };
        assert!(close(k.cube(&info), ([652_750.0, 6_861_250.0, 22.21], 250.0)));
        assert!(k.within(&Key::ROOT) && k.within(&k) && !Key::ROOT.within(&k));
        assert_eq!(k.parent().unwrap().parent(), Some(Key::ROOT));
    }

    fn entry(depth: u32, x: u32, y: u32, z: u32, count: i32) -> Entry {
        Entry {
            key: Key { depth, x, y, z },
            offset: 1000,
            size: 10,
            count,
        }
    }

    #[test]
    fn pages_below_pages_load_later() {
        let mut h = Hierarchy::new(paris().info);
        // The root page: the root, one child with its points, one child whose page is elsewhere, a grandchild whose
        // parent is missing (added empty), an entry outside the root's subtree is impossible here
        let added = h
            .add_page(
                0,
                vec![
                    entry(1, 1, 0, 0, -1),
                    entry(0, 0, 0, 0, 5),
                    entry(1, 0, 0, 0, 7),
                    entry(2, 3, 3, 3, 9),
                ],
            )
            .unwrap();
        assert_eq!(added.len(), 4);
        let child = h.index[&Key {
            depth: 1,
            x: 1,
            y: 0,
            z: 0,
        }];
        assert_eq!(h.page(child), Some(Page { offset: 1000, size: 10 }));
        let missing = h.index[&Key {
            depth: 1,
            x: 1,
            y: 1,
            z: 1,
        }];
        assert_eq!(h.nodes[missing].content, Content::Empty);
        assert_eq!(h.nodes[missing].children.len(), 1);
        // Its page: the node itself, a child; an entry of another subtree is left out
        let added = h
            .add_page(
                child,
                vec![entry(1, 1, 0, 0, 3), entry(2, 2, 0, 1, 4), entry(2, 0, 0, 0, 4)],
            )
            .unwrap();
        assert_eq!(added.len(), 1);
        assert!(matches!(h.nodes[child].content, Content::Points { count: 3, .. }));
        assert_eq!(h.nodes[child].children.len(), 1);
    }

    #[test]
    fn malformed_pages_are_errors() {
        assert!(parse_page(&PARIS_PAGE[..33]).is_err());
        let mut bad = PARIS_PAGE[..64].to_vec();
        // x past its level
        bad[4..8].copy_from_slice(&5i32.to_le_bytes());
        bad[0..4].copy_from_slice(&1i32.to_le_bytes());
        assert!(parse_page(&bad).is_err());
        let mut bad = PARIS_PAGE[..64].to_vec();
        bad[0..4].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(parse_page(&bad).is_err());
        let mut bad = PARIS_PAGE[..64].to_vec();
        bad[28..32].copy_from_slice(&(-7i32).to_le_bytes());
        assert!(parse_page(&bad).is_err());
    }

    #[test]
    fn hierarchies_stay_within_their_bounds() {
        let mut h = Hierarchy::new(paris().info);
        h.max_nodes = 10;
        let many: Vec<Entry> = (0..64).map(|i| entry(3, i % 8, i / 8, 0, 1)).collect();
        assert!(h.add_page(0, many).is_err());
        assert_eq!(h.nodes.len(), 1);
        assert_eq!(h.nodes[0].content, Content::Empty);
        // A page of a node already loaded adds nothing
        assert!(h.add_page(0, vec![entry(1, 0, 0, 0, 2)]).is_ok());
        assert!(h.add_page(99, vec![]).is_err());
    }

    #[test]
    fn nodes_are_placed_in_metres() {
        let mut h = Hierarchy::new(paris().info);
        h.add_page(0, parse_page(PARIS_PAGE).unwrap()).unwrap();
        assert_eq!(h.center(0), [652_500.0, 6_861_500.0, 522.21]);
        h.place(0, DVec3::new(2.35, 48.85, 80.0), 1.0, 1.0);
        assert!((h.nodes[0].radius - 500.0 * 3f64.sqrt()).abs() < 1e-9);
        assert!((h.nodes[0].spacing - 6.802_721).abs() < 1e-6);
        // In feet, a node's sphere and spacing in metres
        let deep = h.nodes.iter().position(|n| n.key.depth == 3).unwrap();
        h.place(deep, DVec3::ZERO, 0.3048, 0.3048);
        assert!((h.nodes[deep].spacing - 6.802_721 * 0.3048 / 8.0).abs() < 1e-6);
    }
}
