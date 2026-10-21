//! The tileset JSON as the tree reads it (serde): only the fields it uses, unknown ones ignored.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// A tileset file (`tileset.json`, an external tileset)
#[derive(Deserialize)]
pub(super) struct TilesetJson {
    /// Its `asset` (default when absent)
    #[serde(default)]
    pub(super) asset: AssetJson,
    /// Its root tile
    pub(super) root: TileJson,
}

/// The `asset` of a tileset
#[derive(Deserialize, Default)]
pub(super) struct AssetJson {
    /// `Y` (the 3D Tiles 1.0 default when absent) or `Z`: the up axis of its glTF
    #[serde(rename = "gltfUpAxis")]
    pub(super) gltf_up_axis: Option<String>,
}

/// A tile of a tileset and its children
#[derive(Deserialize)]
pub(super) struct TileJson {
    /// Its bounding volume (sphere, region or box)
    #[serde(rename = "boundingVolume")]
    pub(super) bounding_volume: VolumeJson,
    /// Metres: the error of the tile's content, the screen-space error is computed from
    #[serde(rename = "geometricError")]
    pub(super) geometric_error: f64,
    /// `REPLACE` or `ADD`, inherited from the parent when absent
    pub(super) refine: Option<String>,
    /// Its single content (3D Tiles 1.0)
    pub(super) content: Option<ContentJson>,
    /// 3D Tiles 1.1: several contents
    #[serde(default)]
    pub(super) contents: Vec<ContentJson>,
    /// Its child tiles
    #[serde(default)]
    pub(super) children: Vec<TileJson>,
    /// Column-major 4x4 matrix from the tile's frame to its parent's
    pub(super) transform: Option<[f64; 16]>,
    /// 3D Tiles 1.1: the implicit tiling rooted at this tile
    #[serde(rename = "implicitTiling")]
    pub(super) implicit_tiling: Option<ImplicitJson>,
    /// 3D Tiles 1.0 extensions of the same (`3DTILES_implicit_tiling`, `3DTILES_multiple_contents`)
    #[serde(default)]
    pub(super) extensions: HashMap<String, Value>,
}

/// The content of a tile
#[derive(Deserialize)]
pub(super) struct ContentJson {
    /// Its URI, relative to the tileset (an implicit tiling template for implicit tiles)
    pub(super) uri: Option<String>,
    /// 3D Tiles 0.0 name
    pub(super) url: Option<String>,
}

/// A bounding volume: one of the three is given
#[derive(Deserialize)]
pub(super) struct VolumeJson {
    /// Center x, y, z and radius, in the tile's frame (metres)
    pub(super) sphere: Option<[f64; 4]>,
    /// West, south, east, north (radians), minimum and maximum height (metres above the ellipsoid)
    pub(super) region: Option<[f64; 6]>,
    /// Center then three half-axes, in the tile's frame (metres)
    #[serde(rename = "box")]
    pub(super) oriented_box: Option<[f64; 12]>,
}

/// The implicit tiling of a tile (`implicitTiling`, or the `3DTILES_implicit_tiling` extension)
#[derive(Deserialize)]
pub(super) struct ImplicitJson {
    /// `QUADTREE` or `OCTREE`
    #[serde(rename = "subdivisionScheme")]
    pub(super) subdivision_scheme: String,
    /// Levels of each subtree file
    #[serde(rename = "subtreeLevels")]
    pub(super) subtree_levels: u32,
    /// Levels of the whole tiling (3D Tiles 1.1)
    #[serde(rename = "availableLevels")]
    pub(super) available_levels: Option<u32>,
    /// `3DTILES_implicit_tiling` name: the last level rather than the count
    #[serde(rename = "maximumLevel")]
    pub(super) maximum_level: Option<u32>,
    /// The URI template of the subtree files
    pub(super) subtrees: UriJson,
}

/// An object holding a URI
#[derive(Deserialize)]
pub(super) struct UriJson {
    /// A template with `{level}`, `{x}`, `{y}`, `{z}`
    pub(super) uri: String,
}
