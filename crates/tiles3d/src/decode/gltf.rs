//! The glTF of a tile: its scene's node hierarchy walked down to world positions, its triangle primitives with
//! their texture coordinates and image, its buffer views (sliced from the binary chunk or decompressed by meshopt).

use super::accessor::index_reader;
use super::container::{parse_glb, slice};
use super::{DracoGeometry, Mesh, Primitive};
use crate::meshopt;
use glam::{DMat4, DQuat, DVec2, DVec3};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// A JSON index or count as usize (None if absent, negative or too large for this platform)
pub(super) fn index(v: &Value) -> Option<usize> {
    v.as_u64().and_then(|n| usize::try_from(n).ok())
}

/// A byte offset: 0 when absent, an error when invalid
pub(super) fn offset(v: &Value) -> Result<usize, String> {
    if v.is_null() {
        return Ok(0);
    }
    index(v).ok_or_else(|| "bad byteOffset".into())
}

fn material<'a>(json: &'a Value, primitive: &Value) -> &'a Value {
    &json["materials"][index(&primitive["material"]).unwrap_or(usize::MAX)]
}

/// Texture coordinate set of the base color texture of a primitive (`texCoord`, overridden by
/// `KHR_texture_transform`)
pub(super) fn uv_set(json: &Value, primitive: &Value) -> u64 {
    let base = &material(json, primitive)["pbrMetallicRoughness"]["baseColorTexture"];
    base["extensions"]["KHR_texture_transform"]["texCoord"]
        .as_u64()
        .or(base["texCoord"].as_u64())
        .unwrap_or(0)
}

/// Translation, rotation, scale or matrix of a node: `default` when absent, an error unless N numbers
fn numbers<const N: usize>(value: &Value, default: [f64; N]) -> Result<[f64; N], String> {
    match value {
        Value::Null => Ok(default),
        Value::Array(a) if a.len() == N => {
            let mut out = [0.0; N];
            for (o, v) in out.iter_mut().zip(a) {
                *o = v.as_f64().ok_or("bad glTF node transform")?;
            }
            Ok(out)
        }
        _ => Err("bad glTF node transform".into()),
    }
}

pub(super) fn decode_glb(
    bytes: &[u8],
    transform: &DMat4,
    y_up: bool,
    rtc: DVec3,
    draco: &[DracoGeometry],
) -> Result<Mesh, String> {
    let (json, bin) = parse_glb(bytes)?;
    for required in json["extensionsRequired"].as_array().into_iter().flatten() {
        let name = required.as_str().unwrap_or("");
        if !SUPPORTED.contains(&name) {
            return Err(format!("glTF extension {name} unsupported"));
        }
    }
    let gltf = Gltf::new(&json, bin, draco);
    // glTF y-up → z-up: +90° about x
    let axis = if y_up {
        DMat4::from_rotation_x(std::f64::consts::FRAC_PI_2)
    } else {
        DMat4::IDENTITY
    };
    let to_world = *transform * DMat4::from_translation(rtc) * axis;
    let mut out = Vec::new();
    let scene = index(&json["scene"]).unwrap_or(0);
    let nodes = json["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    let roots: Vec<usize> = if let Some(roots) = json["scenes"][scene]["nodes"].as_array() {
        roots.iter().filter_map(index).collect()
    } else {
        // No scene: the nodes no other node holds
        let mut child = vec![false; nodes.len()];
        for c in nodes
            .iter()
            .flat_map(|n| n["children"].as_array().into_iter().flatten())
        {
            if let Some(slot) = index(c).and_then(|c| child.get_mut(c)) {
                *slot = true;
            }
        }
        (0..nodes.len()).filter(|&n| !child[n]).collect()
    };
    for node in roots {
        gltf.node(node, &to_world, &mut out, 0)?;
    }
    // Anchor: center of the positions' bounds
    let (mut min, mut max) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
    for (_, world) in &out {
        for p in world {
            min = min.min(*p);
            max = max.max(*p);
        }
    }
    let anchor = if min.is_finite() && max.is_finite() {
        (min + max) / 2.0
    } else {
        transform.transform_point3(rtc)
    };
    let primitives = out
        .into_iter()
        .map(|(mut primitive, world)| {
            primitive.positions = world.iter().flat_map(|p| (*p - anchor).as_vec3().to_array()).collect();
            primitive
        })
        .collect();
    Ok(Mesh { anchor, primitives })
}

/// Positions and uv as stored (before the node transforms), triangle indices
type Geometry = (Vec<f32>, Vec<f32>, Vec<u32>);

/// glTF extensions this decoder handles (a tile requiring another one is refused)
const SUPPORTED: [&str; 6] = [
    "EXT_meshopt_compression",
    "KHR_mesh_quantization",
    "KHR_texture_transform",
    "KHR_materials_unlit",
    "KHR_draco_mesh_compression",
    "KHR_texture_basisu",
];

/// Deepest node hierarchy followed
const MAX_DEPTH: u32 = 32;
/// Node visits allowed per node of the file: a hierarchy is a tree (each node once), some slack for files sharing
/// nodes, none for a graph whose shared children would multiply the visits
const VISITS_PER_NODE: usize = 4;
/// Values (f32 components, indices) a tile may decode in all: accessors are read again for each primitive and node
/// that uses them, and one without buffer view costs no byte of the file, so a tiny file could otherwise ask for
/// gigabytes. 16 M values: 64 MB, far above a real tile
const MAX_VALUES: usize = 1 << 24;

pub(super) struct Gltf<'a> {
    pub(super) json: &'a Value,
    bin: &'a [u8],
    /// Draco primitives decoded by the caller
    draco: &'a [DracoGeometry],
    /// Buffer views decompressed so far (`EXT_meshopt_compression`)
    views: RefCell<HashMap<usize, Rc<[u8]>>>,
    /// Node visits left
    visits: Cell<usize>,
    /// Values left to decode (`MAX_VALUES`)
    values: Cell<usize>,
}

impl<'a> Gltf<'a> {
    pub(super) fn new(json: &'a Value, bin: &'a [u8], draco: &'a [DracoGeometry]) -> Self {
        let nodes = json["nodes"].as_array().map_or(0, Vec::len);
        Self {
            json,
            bin,
            draco,
            views: RefCell::default(),
            visits: Cell::new(nodes.saturating_mul(VISITS_PER_NODE)),
            values: Cell::new(MAX_VALUES),
        }
    }

    /// Takes `count` values from the tile's budget, before they are allocated: an error past `MAX_VALUES`
    pub(super) fn spend(&self, count: usize) -> Result<(), String> {
        let left = self.values.get().checked_sub(count).ok_or("glTF tile too large")?;
        self.values.set(left);
        Ok(())
    }

    fn node(
        &self,
        index: usize,
        parent: &DMat4,
        out: &mut Vec<(Primitive, Vec<DVec3>)>,
        depth: u32,
    ) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("glTF node hierarchy too deep".into());
        }
        let Some(visits) = self.visits.get().checked_sub(1) else {
            return Err("glTF nodes shared too often (not a tree)".into());
        };
        self.visits.set(visits);
        let node = &self.json["nodes"][index];
        let local = if node["matrix"].is_null() {
            let t = numbers(&node["translation"], [0.0; 3])?;
            let r = numbers(&node["rotation"], [0.0, 0.0, 0.0, 1.0])?;
            let s = numbers(&node["scale"], [1.0; 3])?;
            DMat4::from_scale_rotation_translation(DVec3::from(s), DQuat::from_array(r), DVec3::from(t))
        } else {
            DMat4::from_cols_array(&numbers(&node["matrix"], [0.0; 16])?)
        };
        let world = *parent * local;
        if let Some(mesh) = self::index(&node["mesh"]) {
            for (index, primitive) in self.json["meshes"][mesh]["primitives"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                if let Some(p) = self.primitive((mesh, index), primitive, &world)? {
                    out.push(p);
                }
            }
        }
        for child in node["children"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(self::index)
        {
            self.node(child, &world, out, depth + 1)?;
        }
        Ok(())
    }

    /// Geometry of a primitive as stored: from the Draco decoder (`KHR_draco_mesh_compression`) or the accessors
    fn geometry(&self, (mesh, index): (usize, usize), primitive: &Value) -> Result<Option<Geometry>, String> {
        if primitive["extensions"]["KHR_draco_mesh_compression"].is_object() {
            let g = self
                .draco
                .iter()
                .find(|g| g.mesh == mesh && g.primitive == index)
                .ok_or("Draco geometry not decoded (Draco needs the decoding workers)")?;
            let count = g.positions.len() / 3;
            self.spend(count.saturating_mul(5).saturating_add(g.indices.len()))?;
            let uvs = if g.uvs.is_empty() {
                vec![0.0; count * 2]
            } else {
                g.uvs.clone()
            };
            return Ok(Some((g.positions.clone(), uvs, g.indices.clone())));
        }
        let Some(position) = self::index(&primitive["attributes"]["POSITION"]) else {
            return Ok(None);
        };
        let positions = self.floats(position)?;
        let count = positions.len() / 3;
        let uv = &primitive["attributes"][format!("TEXCOORD_{}", uv_set(self.json, primitive))];
        let uvs = if let Some(uv) = self::index(uv) {
            self.floats(uv)?
        } else {
            self.spend(count.saturating_mul(2))?;
            vec![0.0; count * 2]
        };
        let indices: Vec<u32> = if let Some(i) = self::index(&primitive["indices"]) {
            self.accessor(i, |l| index_reader(l.component_type))?
        } else {
            self.spend(count)?;
            (0..u32::try_from(count).map_err(|_| "too many vertices")?).collect()
        };
        Ok(Some((positions, uvs, indices)))
    }

    /// Triangles only (mode 4, the default); other modes are skipped
    fn primitive(
        &self,
        key: (usize, usize),
        primitive: &Value,
        world: &DMat4,
    ) -> Result<Option<(Primitive, Vec<DVec3>)>, String> {
        if primitive["mode"].as_u64().unwrap_or(4) != 4 {
            return Ok(None);
        }
        let Some((positions, mut uvs, indices)) = self.geometry(key, primitive)? else {
            return Ok(None);
        };
        let count = positions.len() / 3;
        if !positions.len().is_multiple_of(3) || uvs.len() != count * 2 || indices.iter().any(|&i| i as usize >= count)
        {
            return Err("inconsistent glTF primitive".into());
        }
        let world_positions: Vec<DVec3> = positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[x, y, z]| world.transform_point3(DVec3::new(f64::from(x), f64::from(y), f64::from(z))))
            .collect();
        let base_color = &material(self.json, primitive)["pbrMetallicRoughness"]["baseColorTexture"];
        let texture = index(&base_color["index"]);
        // KHR_texture_transform (gltfpack uses it for quantized uv): uv' = offset + rotation · (scale · uv)
        let transform = &base_color["extensions"]["KHR_texture_transform"];
        if transform.is_object() {
            let pair = |key: &str, default: f64| {
                let v = &transform[key];
                DVec2::new(v[0].as_f64().unwrap_or(default), v[1].as_f64().unwrap_or(default))
            };
            let (offset, scale) = (pair("offset", 0.0), pair("scale", 1.0));
            let (sin, cos) = transform["rotation"].as_f64().unwrap_or(0.0).sin_cos();
            for uv in uvs.as_chunks_mut::<2>().0 {
                let (u, v) = (f64::from(uv[0]) * scale.x, f64::from(uv[1]) * scale.y);
                *uv = [
                    (cos * u + sin * v + offset.x) as f32,
                    (-sin * u + cos * v + offset.y) as f32,
                ];
            }
        }
        // KHR_texture_basisu: the KTX2 image first (transcoded by the workers), else the plain one
        let image = texture.and_then(|t| {
            let texture = &self.json["textures"][t];
            let source = index(&texture["extensions"]["KHR_texture_basisu"]["source"]).or(index(&texture["source"]))?;
            self.image(source)
        });
        Ok(Some((
            Primitive {
                positions: Vec::new(),
                uvs,
                indices,
                image,
            },
            world_positions,
        )))
    }

    /// The bytes of a buffer view: a slice of the binary chunk, or decompressed (`EXT_meshopt_compression`)
    pub(super) fn view(&self, index: usize) -> Result<Rc<[u8]>, String> {
        if let Some(view) = self.views.borrow().get(&index) {
            return Ok(view.clone());
        }
        let view = &self.json["bufferViews"][index];
        let ext = &view["extensions"]["EXT_meshopt_compression"];
        let field = |v: &Value, name: &str| self::index(&v[name]).ok_or(format!("buffer view without {name}"));
        let bytes: Rc<[u8]> = if ext.is_object() {
            if ext["buffer"].as_u64().unwrap_or(0) != 0 {
                return Err("meshopt data outside the binary chunk".into());
            }
            let (count, stride) = (field(ext, "count")?, field(ext, "byteStride")?);
            // The decoded size is the view's
            if count.checked_mul(stride) != Some(field(view, "byteLength")?) {
                return Err("meshopt view of the wrong size".into());
            }
            let decoded = meshopt::decode(&meshopt::View {
                data: slice(self.bin, offset(&ext["byteOffset"])?, field(ext, "byteLength")?)
                    .ok_or("meshopt view out of the binary chunk")?,
                count,
                stride,
                mode: meshopt::Mode::parse(ext["mode"].as_str().unwrap_or(""))?,
                filter: meshopt::Filter::parse(ext["filter"].as_str())?,
            })?;
            decoded.into()
        } else {
            if view["buffer"].as_u64().unwrap_or(0) != 0 {
                return Err("buffer view outside the binary chunk".into());
            }
            slice(self.bin, offset(&view["byteOffset"])?, field(view, "byteLength")?)
                .ok_or("buffer view out of the binary chunk")?
                .into()
        };
        self.views.borrow_mut().insert(index, bytes.clone());
        Ok(bytes)
    }

    fn image(&self, index: usize) -> Option<(Vec<u8>, String)> {
        let image = &self.json["images"][index];
        let bytes = self.view(self::index(&image["bufferView"])?).ok()?;
        let mime = image["mimeType"].as_str().unwrap_or("image/jpeg").to_owned();
        Some((bytes.to_vec(), mime))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{f32s, triangle, vertex};
    use super::*;
    use serde_json::json;

    #[test]
    fn interleaved_attributes_follow_the_byte_stride() {
        // x y z u v per vertex, 20 bytes apart
        let bin = f32s(&[
            0.0, 0.0, 0.0, 0.1, 0.2, //
            1.0, 0.0, 0.0, 0.3, 0.4, //
            0.0, 1.0, 0.0, 0.5, 0.6,
        ]);
        let json = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 60}],
            "bufferViews": [{"buffer": 0, "byteLength": 60, "byteStride": 20}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 0, "byteOffset": 12, "componentType": 5126, "count": 3, "type": "VEC2"}
            ],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1}}]}],
            "nodes": [{"mesh": 0}]
        });
        let mesh = triangle(&json, &bin).unwrap();
        let p = &mesh.primitives[0];
        assert_eq!(p.uvs, [0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        assert_eq!(p.indices, [0, 1, 2]);
        assert!((vertex(&mesh, 1) - DVec3::X).length() < 1e-6);
        // A stride smaller than an element is refused
        let mut bad = json;
        bad["bufferViews"][0]["byteStride"] = json!(8);
        assert!(triangle(&bad, &bin).is_err());
    }

    #[test]
    fn the_texture_coordinate_set_of_the_material_is_used() {
        let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        bin.extend(f32s(&[9.0; 6]));
        bin.extend(f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
        let json = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 84}],
            "bufferViews": [
                {"buffer": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 24},
                {"buffer": 0, "byteOffset": 60, "byteLength": 24}
            ],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2"},
                {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2"}
            ],
            "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0, "texCoord": 1}}}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1, "TEXCOORD_1": 2}, "material": 0}]}],
            "nodes": [{"mesh": 0}]
        });
        assert_eq!(
            triangle(&json, &bin).unwrap().primitives[0].uvs,
            [0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
        );
    }

    /// The one-triangle tile of the tests below, with `nodes` and `scenes` given
    fn with_nodes(nodes: &Value, scenes: Option<&Value>) -> Result<Mesh, String> {
        let bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let mut json = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 36}],
            "bufferViews": [{"buffer": 0, "byteLength": 36}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "nodes": nodes
        });
        if let Some(scenes) = scenes {
            json["scenes"] = scenes.clone();
        }
        triangle(&json, &bin)
    }

    #[test]
    fn node_transforms_of_the_wrong_size_are_errors() {
        assert!(with_nodes(&json!([{"mesh": 0, "translation": [1.0, 2.0]}]), None).is_err());
        assert!(with_nodes(&json!([{"mesh": 0, "rotation": [0.0, 0.0, 1.0]}]), None).is_err());
        assert!(with_nodes(&json!([{"mesh": 0, "scale": "big"}]), None).is_err());
        assert!(with_nodes(&json!([{"mesh": 0, "matrix": vec![1.0; 15]}]), None).is_err());
        let moved = with_nodes(&json!([{"mesh": 0, "translation": [10.0, 0.0, 0.0]}]), None).unwrap();
        assert!((moved.anchor.x - 10.5).abs() < 1e-9);
    }

    #[test]
    fn node_graphs_neither_hang_nor_overflow() {
        // A cycle
        assert!(
            with_nodes(
                &json!([{"mesh": 0, "children": [1]}, {"children": [0]}]),
                Some(&json!([{"nodes": [0]}]))
            )
            .is_err()
        );
        // 40 levels, each node holding the next twice: 2^40 paths
        let nodes: Vec<Value> = (0..40)
            .map(|i| json!({"mesh": 0, "children": [i + 1, i + 1]}))
            .collect();
        assert!(with_nodes(&json!(nodes), Some(&json!([{"nodes": [0]}]))).is_err());
        // Without scenes, the roots are the nodes no other holds: a child is not drawn twice
        let mesh = with_nodes(&json!([{"children": [1]}, {"mesh": 0}]), None).unwrap();
        assert_eq!(mesh.primitives.len(), 1);
    }

    #[test]
    fn huge_counts_are_errors_not_allocations() {
        let bin = f32s(&[0.0; 9]);
        for count in [u64::from(u32::MAX), u64::MAX, 1 << 40, 4] {
            let json = json!({
                "asset": {"version": "2.0"},
                "buffers": [{"byteLength": 36}],
                "bufferViews": [{"buffer": 0, "byteLength": 36, "byteStride": 12}],
                "accessors": [{"bufferView": 0, "componentType": 5126, "count": count, "type": "VEC3"}],
                "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
                "nodes": [{"mesh": 0}]
            });
            assert!(triangle(&json, &bin).is_err(), "{count}");
            let mut no_view = json;
            no_view["accessors"][0].as_object_mut().unwrap().remove("bufferView");
            assert!(triangle(&no_view, &bin).is_err() || count == 4, "{count}");
        }
    }

    #[test]
    fn values_read_again_count_against_one_budget() {
        // One accessor without buffer view, under the per-accessor cap, used by many primitives of many nodes: a
        // few hundred bytes asking for gigabytes, an error before
        let primitives: Vec<Value> = (0..64).map(|_| json!({"attributes": {"POSITION": 0}})).collect();
        let nodes: Vec<Value> = (0..64).map(|_| json!({"mesh": 0})).collect();
        let json = json!({
            "asset": {"version": "2.0"},
            "accessors": [{"componentType": 5126, "count": 1_000_000, "type": "VEC3"}],
            "meshes": [{"primitives": primitives}],
            "nodes": nodes
        });
        let bytes = super::super::tests::glb(&json, &[]);
        assert!(bytes.len() < 4096);
        assert_eq!(
            super::super::decode(&bytes, &DMat4::IDENTITY, false, &[])
                .err()
                .as_deref(),
            Some("glTF tile too large")
        );
        // The same accessor once fits
        let mut once = json;
        once["meshes"][0]["primitives"] = json!([{"attributes": {"POSITION": 0}}]);
        once["nodes"] = json!([{"mesh": 0}]);
        assert!(triangle(&once, &[]).is_ok());
    }
}
