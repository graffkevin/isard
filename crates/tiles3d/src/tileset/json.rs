//! The tileset JSON as the tree reads it (serde): only the fields it uses, unknown ones ignored.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Deserialize)]
pub(super) struct TilesetJson {
    #[serde(default)]
    pub(super) asset: AssetJson,
    pub(super) root: TileJson,
}

#[derive(Deserialize, Default)]
pub(super) struct AssetJson {
    #[serde(rename = "gltfUpAxis")]
    pub(super) gltf_up_axis: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct TileJson {
    #[serde(rename = "boundingVolume")]
    pub(super) bounding_volume: VolumeJson,
    #[serde(rename = "geometricError")]
    pub(super) geometric_error: f64,
    pub(super) refine: Option<String>,
    pub(super) content: Option<ContentJson>,
    /// 3D Tiles 1.1: several contents
    #[serde(default)]
    pub(super) contents: Vec<ContentJson>,
    #[serde(default)]
    pub(super) children: Vec<TileJson>,
    pub(super) transform: Option<[f64; 16]>,
    #[serde(rename = "implicitTiling")]
    pub(super) implicit_tiling: Option<ImplicitJson>,
    /// 3D Tiles 1.0 extensions of the same (`3DTILES_implicit_tiling`, `3DTILES_multiple_contents`)
    #[serde(default)]
    pub(super) extensions: HashMap<String, Value>,
}

#[derive(Deserialize)]
pub(super) struct ContentJson {
    pub(super) uri: Option<String>,
    /// 3D Tiles 0.0 name
    pub(super) url: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct VolumeJson {
    pub(super) sphere: Option<[f64; 4]>,
    pub(super) region: Option<[f64; 6]>,
    #[serde(rename = "box")]
    pub(super) oriented_box: Option<[f64; 12]>,
}

#[derive(Deserialize)]
pub(super) struct ImplicitJson {
    #[serde(rename = "subdivisionScheme")]
    pub(super) subdivision_scheme: String,
    #[serde(rename = "subtreeLevels")]
    pub(super) subtree_levels: u32,
    #[serde(rename = "availableLevels")]
    pub(super) available_levels: Option<u32>,
    /// `3DTILES_implicit_tiling` name: the last level rather than the count
    #[serde(rename = "maximumLevel")]
    pub(super) maximum_level: Option<u32>,
    pub(super) subtrees: UriJson,
}

#[derive(Deserialize)]
pub(super) struct UriJson {
    pub(super) uri: String,
}
