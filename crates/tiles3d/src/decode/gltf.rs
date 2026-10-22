//! The glTF of a tile: its scene's node hierarchy walked down to world positions, its triangle primitives with
//! their texture coordinates and image, its point primitives with their colors and normals, its buffer views (sliced
//! from the binary chunk or decompressed by meshopt).

use super::container::{parse_glb, slice};
use super::meshopt;
use super::{DracoGeometry, Mesh, Points, Primitive, Properties, center, relative};
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

/// The material of a primitive (`Value::Null` without one)
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

/// Decodes a glTF binary into a mesh: its scene walked from the roots through the node transforms, then
/// `transform` · `rtc` · the y-up to z-up rotation (`y_up`), to ECEF. `draco`: the Draco primitives decoded by the
/// caller; `batch`: the b3dm's batch table and its length, for the features' properties without a property table
pub(super) fn decode_glb(
    bytes: &[u8],
    transform: &DMat4,
    y_up: bool,
    rtc: DVec3,
    draco: &[DracoGeometry],
    batch: Option<&(Value, usize)>,
) -> Result<Mesh, String> {
    let (json, bin) = parse_glb(bytes)?;

    check_extensions(&json)?;

    let gltf = Gltf::new(&json, bin, draco);
    // glTF y-up → z-up: +90° about x
    let axis = if y_up {
        DMat4::from_rotation_x(std::f64::consts::FRAC_PI_2)
    } else {
        DMat4::IDENTITY
    };
    let to_world = *transform * DMat4::from_translation(rtc) * axis;
    let mut out = Out::default();

    for node in scene_roots(&json) {
        gltf.node(node, &to_world, &mut out, 0)?;
    }

    let anchor = anchor(&out).unwrap_or_else(|| transform.transform_point3(rtc));
    let primitives = out
        .triangles
        .into_iter()
        .map(|(mut primitive, world)| {
            primitive.positions = relative(&world, anchor);

            primitive
        })
        .collect();
    let points = out
        .points
        .into_iter()
        .map(|(mut points, world)| {
            points.positions = relative(&world, anchor);

            points
        })
        .collect();
    let properties = feature_properties(&gltf, batch)?;

    Ok(Mesh {
        anchor,
        primitives,
        images: gltf
            .images
            .into_inner()
            .into_iter()
            .map(|(_, bytes, mime)| (bytes, mime))
            .collect(),
        points,
        properties,
    })
}

/// An error if the glTF requires an extension this decoder does not handle (`SUPPORTED`)
fn check_extensions(json: &Value) -> Result<(), String> {
    for required in json["extensionsRequired"].as_array().into_iter().flatten() {
        let name = required.as_str().unwrap_or("");

        if !SUPPORTED.contains(&name) {
            return Err(format!("glTF extension {name} unsupported"));
        }
    }

    Ok(())
}

/// The root nodes of the glTF's scene (`scene`, else the first); without scenes, the nodes no other node holds
fn scene_roots(json: &Value) -> Vec<usize> {
    let scene = index(&json["scene"]).unwrap_or(0);
    let nodes = json["nodes"].as_array().map_or(&[][..], Vec::as_slice);

    if let Some(roots) = json["scenes"][scene]["nodes"].as_array() {
        return roots.iter().filter_map(index).collect();
    }

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
}

/// The anchor of a tile: the center of the bounds of all its world positions (ECEF), None without finite ones
fn anchor(out: &Out) -> Option<DVec3> {
    let corners: Vec<DVec3> = out
        .triangles
        .iter()
        .map(|(_, world)| world)
        .chain(out.points.iter().map(|(_, world)| world))
        .filter_map(|world| bounds(world))
        .flatten()
        .collect();

    center(&corners)
}

/// The properties of the features: the property table of the first primitive with one (3D Tiles 1.1), else the
/// b3dm's batch table, None without either
fn feature_properties(gltf: &Gltf, batch: Option<&(Value, usize)>) -> Result<Option<Properties>, String> {
    let table = gltf.json["meshes"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| m["primitives"].as_array().into_iter().flatten())
        .find_map(|p| index(&p["extensions"]["EXT_mesh_features"]["featureIds"][0]["propertyTable"]));

    match table {
        Some(table) => super::metadata::property_table(gltf, table),
        None => Ok(batch.and_then(|(table, count)| super::metadata::batch_table(table, *count))),
    }
}

/// The corners of the bounds of positions, None if not finite
fn bounds(positions: &[DVec3]) -> Option<[DVec3; 2]> {
    let (mut min, mut max) = (DVec3::INFINITY, DVec3::NEG_INFINITY);

    for p in positions {
        min = min.min(*p);
        max = max.max(*p);
    }

    (min.is_finite() && max.is_finite()).then_some([min, max])
}

/// The primitives of the scene with their world positions: triangles, points
#[derive(Default)]
struct Out {
    triangles: Vec<(Primitive, Vec<DVec3>)>,
    points: Vec<(Points, Vec<DVec3>)>,
}

/// Positions and uv as stored (before the node transforms), triangle indices: shared by the nodes using a mesh
type Geometry = (Rc<[f32]>, Rc<[f32]>, Rc<[u32]>);

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
/// Values a tile may hand over in all, counted for each primitive of each node (5 per vertex: its position and uv,
/// and its indices): an accessor is decoded once however many use it, but each use is a primitive to upload, and one
/// without buffer view costs no byte of the file, so a tiny file could otherwise ask for gigabytes. An accessor is
/// decoded only if it fits in what is left. 16 M values: 64 MB, far above a real tile
const MAX_VALUES: usize = 1 << 24;

/// An image of the glTF used by a primitive: its index in the glTF, its bytes, its media type
type Image = (usize, Rc<[u8]>, String);

/// A glTF being decoded: its JSON and binary chunk, the buffer views, accessors and images read so far (each once
/// however many primitives use it), and the budgets that bound a malicious file
pub(super) struct Gltf<'a> {
    /// The JSON chunk
    pub(super) json: &'a Value,
    /// The binary chunk (empty without)
    bin: &'a [u8],
    /// Draco primitives decoded by the caller
    draco: &'a [DracoGeometry],
    /// Buffer views decompressed so far (`EXT_meshopt_compression`)
    views: RefCell<HashMap<usize, Rc<[u8]>>>,
    /// Accessors read so far, by index (`floats`, `indices`)
    pub(super) floats: RefCell<HashMap<usize, Rc<[f32]>>>,
    /// Index accessors read so far, by index
    pub(super) indices: RefCell<HashMap<usize, Rc<[u32]>>>,
    /// Indices 0 to n - 1 of the primitives without indices, by n
    sequences: RefCell<HashMap<usize, Rc<[u32]>>>,
    /// Images used so far, in that order: their index in the glTF, bytes and media type
    images: RefCell<Vec<Image>>,
    /// Node visits left
    visits: Cell<usize>,
    /// Values left to decode (`MAX_VALUES`)
    values: Cell<usize>,
}

impl<'a> Gltf<'a> {
    /// A glTF from its chunks, nothing read yet, its node visits bounded by its node count
    pub(super) fn new(json: &'a Value, bin: &'a [u8], draco: &'a [DracoGeometry]) -> Self {
        let nodes = json["nodes"].as_array().map_or(0, Vec::len);

        Self {
            json,
            bin,
            draco,
            views: RefCell::default(),
            floats: RefCell::default(),
            indices: RefCell::default(),
            sequences: RefCell::default(),
            images: RefCell::default(),
            visits: Cell::new(nodes.saturating_mul(VISITS_PER_NODE)),
            values: Cell::new(MAX_VALUES),
        }
    }

    /// Takes `count` values from the tile's budget, before they are handed over: an error past `MAX_VALUES`
    pub(super) fn spend(&self, count: usize) -> Result<(), String> {
        let left = self.within(count)?;

        self.values.set(left);

        Ok(())
    }

    /// The budget left after `count` values, an error if they do not fit (nothing spent)
    pub(super) fn within(&self, count: usize) -> Result<usize, String> {
        self.values
            .get()
            .checked_sub(count)
            .ok_or_else(|| "glTF tile too large".into())
    }

    /// Walks node `index` and its children (depth-first), pushing the primitives of their meshes to `out` in world
    /// positions; `parent`: the world transform above it. Errors past `MAX_DEPTH` or the visits budget
    fn node(&self, index: usize, parent: &DMat4, out: &mut Out, depth: u32) -> Result<(), String> {
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
                if primitive["mode"].as_u64() == Some(0) {
                    if let Some(p) = self.points((mesh, index), primitive, &world)? {
                        out.points.push(p);
                    }
                } else if let Some(p) = self.primitive((mesh, index), primitive, &world)? {
                    out.triangles.push(p);
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

            let uvs: Rc<[f32]> = if g.uvs.is_empty() {
                vec![0.0; count * 2].into()
            } else {
                g.uvs.as_slice().into()
            };

            return Ok(Some((g.positions.as_slice().into(), uvs, g.indices.as_slice().into())));
        }

        let Some(position) = self::index(&primitive["attributes"]["POSITION"]) else {
            return Ok(None);
        };
        let positions = self.floats(position)?;
        let count = positions.len() / 3;
        let uv = &primitive["attributes"][format!("TEXCOORD_{}", uv_set(self.json, primitive))];
        let uv = self::index(uv);
        let indices = self::index(&primitive["indices"]);
        // This use of the mesh: its vertices and indices, counted before any is made
        let index_count = match indices {
            Some(i) => self.indices(i)?.len(),
            None => count,
        };

        self.spend(count.saturating_mul(5).saturating_add(index_count))?;

        let uvs = match uv {
            Some(uv) => self.floats(uv)?,
            None => vec![0.0; count * 2].into(),
        };
        let indices = if let Some(i) = indices {
            self.indices(i)?
        } else {
            let n = u32::try_from(count).map_err(|_| "too many vertices")?;

            self.sequences
                .borrow_mut()
                .entry(count)
                .or_insert_with(|| (0..n).collect())
                .clone()
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

        let Some((positions, uvs, indices)) = self.geometry(key, primitive)? else {
            return Ok(None);
        };
        let count = positions.len() / 3;
        // The feature of each vertex: the first feature ID set given by an attribute (`EXT_mesh_features`), else a
        // b3dm's `_BATCHID`; not for Draco primitives (their attributes are the decoder's)
        let feature_set = &primitive["extensions"]["EXT_mesh_features"]["featureIds"][0];
        let attribute = feature_set["attribute"]
            .as_u64()
            .map_or_else(|| "_BATCHID".to_owned(), |n| format!("_FEATURE_ID_{n}"));
        let features = match self::index(&primitive["attributes"][attribute]) {
            Some(accessor) if !primitive["extensions"]["KHR_draco_mesh_compression"].is_object() => {
                self.spend(count)?;

                Some(self.floats(accessor)?).filter(|f| f.len() == count)
            }
            _ => None,
        };

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
        let pbr = &material(self.json, primitive)["pbrMetallicRoughness"];
        let base_color = &pbr["baseColorTexture"];
        let texture = index(&base_color["index"]);
        // Out-of-range or missing factors fall back to white
        let color = numbers(&pbr["baseColorFactor"], [1.0; 4])
            .unwrap_or([1.0; 4])
            .map(|c| c.clamp(0.0, 1.0) as f32);
        // KHR_texture_transform (gltfpack uses it for quantized uv): uv' = offset + rotation · (scale · uv)
        let transform = &base_color["extensions"]["KHR_texture_transform"];
        let uvs = if transform.is_object() {
            let mut uvs = uvs.to_vec();
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

            uvs.into()
        } else {
            uvs
        };
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
                color,
                features,
            },
            world_positions,
        )))
    }

    /// A point primitive (mode 0): its positions, colors (`COLOR_0` times the material's base color, else that color)
    /// and normals, from the accessors or the Draco decoder
    fn points(
        &self,
        (mesh, index): (usize, usize),
        primitive: &Value,
        world: &DMat4,
    ) -> Result<Option<(Points, Vec<DVec3>)>, String> {
        let attributes = &primitive["attributes"];
        let [positions, colors, normals]: [Rc<[f32]>; 3] =
            if primitive["extensions"]["KHR_draco_mesh_compression"].is_object() {
                let g = self
                    .draco
                    .iter()
                    .find(|g| g.mesh == mesh && g.primitive == index)
                    .ok_or("Draco geometry not decoded (Draco needs the decoding workers)")?;

                self.spend((g.positions.len() / 3).saturating_mul(5))?;

                [
                    g.positions.as_slice().into(),
                    g.colors.as_slice().into(),
                    g.normals.as_slice().into(),
                ]
            } else {
                let Some(position) = self::index(&attributes["POSITION"]) else {
                    return Ok(None);
                };
                let positions = self.floats(position)?;

                self.spend((positions.len() / 3).saturating_mul(5))?;

                let optional = |name: &str| match self::index(&attributes[name]) {
                    Some(a) => self.floats(a),
                    None => Ok(Rc::from(Vec::new())),
                };

                [positions, optional("COLOR_0")?, optional("NORMAL")?]
            };
        let count = positions.len() / 3;
        let colored = colors.len() == count * 3 || colors.len() == count * 4;

        if !positions.len().is_multiple_of(3)
            || !(colors.is_empty() || colored)
            || !(normals.is_empty() || normals.len() == count * 3)
        {
            return Err("inconsistent glTF points".into());
        }

        let factor = numbers(
            &material(self.json, primitive)["pbrMetallicRoughness"]["baseColorFactor"],
            [1.0; 4],
        )
        .unwrap_or([1.0; 4])
        .map(|c| c.clamp(0.0, 1.0));
        let base = if colors.is_empty() {
            vec![[255; 4]; count]
        } else {
            super::pnts::float_colors(&colors, count)
        };
        let colors = base
            .into_iter()
            .map(|c| std::array::from_fn(|i| (f64::from(c[i]) * factor[i]).round() as u8))
            .collect();
        let world_positions = positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&p| world.transform_point3(DVec3::from(p.map(f64::from))))
            .collect();
        let normals = normals
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&n| {
                world
                    .transform_vector3(DVec3::from(n.map(f64::from)))
                    .normalize_or_zero()
                    .as_vec3()
                    .to_array()
            })
            .collect();

        Ok(Some((
            Points {
                positions: Vec::new(),
                colors,
                normals,
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

    /// An image of the glTF as its place in `Mesh::images`: added on its first use, its bytes shared with its buffer
    /// view
    fn image(&self, index: usize) -> Option<usize> {
        if let Some(k) = self.images.borrow().iter().position(|(i, _, _)| *i == index) {
            return Some(k);
        }

        let image = &self.json["images"][index];
        let bytes = self.view(self::index(&image["bufferView"])?).ok()?;
        let mime = image["mimeType"].as_str().unwrap_or("image/jpeg").to_owned();
        let mut images = self.images.borrow_mut();

        images.push((index, bytes, mime));

        Some(images.len() - 1)
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

        assert_eq!(*p.uvs, [0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        assert_eq!(*p.indices, [0, 1, 2]);
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
            *triangle(&json, &bin).unwrap().primitives[0].uvs,
            [0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn the_base_color_factor_of_the_material_is_kept() {
        let bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let mut json = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 36}],
            "bufferViews": [{"buffer": 0, "byteLength": 36}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}],
            "materials": [{"pbrMetallicRoughness": {"baseColorFactor": [0.8, 0.1, 0.2, 0.5]}}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "material": 0}]}],
            "nodes": [{"mesh": 0}]
        });

        assert_eq!(triangle(&json, &bin).unwrap().primitives[0].color, [0.8, 0.1, 0.2, 0.5]);
        // Without material, or a factor of the wrong size: white; out of range: clamped
        json["meshes"][0]["primitives"][0]["material"] = Value::Null;
        assert_eq!(triangle(&json, &bin).unwrap().primitives[0].color, [1.0; 4]);
        json["meshes"][0]["primitives"][0]["material"] = json!(0);
        json["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"] = json!([0.5, 0.5]);
        assert_eq!(triangle(&json, &bin).unwrap().primitives[0].color, [1.0; 4]);
        json["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"] = json!([2.0, -1.0, 0.5, 1.0]);
        assert_eq!(triangle(&json, &bin).unwrap().primitives[0].color, [1.0, 0.0, 0.5, 1.0]);
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
    fn instances_share_their_accessors_and_images() {
        // One textured mesh used by three nodes, moved apart; the image bytes after the positions and uv
        let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

        bin.extend(f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
        bin.extend([0xFF, 0xD8, 0xFF, 0xE0]);

        let json = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 64}],
            "bufferViews": [
                {"buffer": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 24},
                {"buffer": 0, "byteOffset": 60, "byteLength": 4}
            ],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2"}
            ],
            "images": [{"bufferView": 2, "mimeType": "image/jpeg"}],
            "textures": [{"source": 0}],
            "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "material": 0}]}],
            "nodes": [
                {"mesh": 0},
                {"mesh": 0, "translation": [5.0, 0.0, 0.0]},
                {"mesh": 0, "translation": [10.0, 0.0, 0.0]}
            ]
        });
        let mesh = triangle(&json, &bin).unwrap();
        let [a, b, c] = &mesh.primitives[..] else {
            panic!("three primitives expected");
        };

        assert!(Rc::ptr_eq(&a.uvs, &b.uvs) && Rc::ptr_eq(&a.uvs, &c.uvs));
        assert!(Rc::ptr_eq(&a.indices, &c.indices));
        assert_ne!(a.positions, c.positions);
        // The image once, its bytes those of its buffer view
        assert_eq!(mesh.images.len(), 1);
        assert_eq!((a.image, b.image, c.image), (Some(0), Some(0), Some(0)));
        assert_eq!(mesh.image(b).unwrap(), (&[0xFF, 0xD8, 0xFF, 0xE0][..], "image/jpeg"));
    }

    /// A helix of 2000 colored points with normals (mode 0), as written and compressed by gltfpack (meshopt,
    /// quantized positions and normals)
    #[test]
    fn point_primitives_keep_their_colors_and_normals() {
        let plain = super::super::decode(
            include_bytes!("../../tests/data/helix_points.glb"),
            &DMat4::IDENTITY,
            false,
            &[],
        )
        .unwrap();

        assert!(plain.primitives.is_empty());

        let p = &plain.points[0];

        assert_eq!((p.positions.len(), p.colors.len(), p.normals.len()), (6000, 2000, 2000));

        let at = |mesh: &Mesh, i: usize| {
            let q = &mesh.points[0].positions[i * 3..i * 3 + 3];

            mesh.anchor + DVec3::new(f64::from(q[0]), f64::from(q[1]), f64::from(q[2]))
        };

        // The node's translation applied: the first point at (110, 0, 0)
        assert!((at(&plain, 0) - DVec3::new(110.0, 0.0, 0.0)).length() < 1e-4);
        assert_eq!(p.colors[0], [254, 0, 200, 255]);
        assert!((DVec3::from(p.normals[0].map(f64::from)) - DVec3::X).length() < 1e-6);

        let packed = super::super::decode(
            include_bytes!("../../tests/data/helix_points_meshopt.glb"),
            &DMat4::IDENTITY,
            false,
            &[],
        )
        .unwrap();
        let q = &packed.points[0];

        assert_eq!(q.colors.len(), 2000);

        // gltfpack reorders the points: each has its counterpart within 2 mm (14-bit positions over 30 m), with its
        // color and its normal (8 bits)
        for i in (0..2000).step_by(37) {
            let target = at(&packed, i);
            let j = (0..2000)
                .min_by(|&a, &b| {
                    (at(&plain, a) - target)
                        .length()
                        .total_cmp(&(at(&plain, b) - target).length())
                })
                .unwrap();

            assert!((at(&plain, j) - target).length() < 2e-3, "{i}");
            assert_eq!(q.colors[i], p.colors[j]);

            let (a, b) = (
                DVec3::from(p.normals[j].map(f64::from)),
                DVec3::from(q.normals[i].map(f64::from)),
            );

            assert!(a.dot(b) > 0.999, "{i}");
        }
    }

    #[test]
    fn point_colors_take_the_base_color() {
        let bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let mut json = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 24}],
            "bufferViews": [{"buffer": 0, "byteLength": 24}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 2, "type": "VEC3"}],
            "materials": [{"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.5, 0.0, 1.0]}}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "mode": 0, "material": 0}]}],
            "nodes": [{"mesh": 0}]
        });
        let mesh = triangle(&json, &bin).unwrap();

        assert_eq!(mesh.points[0].colors, [[255, 128, 0, 255]; 2]);
        assert_eq!(mesh.points[0].normals.len(), 0);
        // Colors of the wrong count: an error
        json["accessors"]
            .as_array_mut()
            .unwrap()
            .push(json!({"bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3"}));
        json["meshes"][0]["primitives"][0]["attributes"]["COLOR_0"] = json!(1);
        assert!(triangle(&json, &bin).is_err());
        // Lines and strips are left out
        json["meshes"][0]["primitives"][0]["mode"] = json!(1);

        let mesh = triangle(&json, &bin).unwrap();

        assert!(mesh.points.is_empty() && mesh.primitives.is_empty());
    }

    #[test]
    fn draco_points_come_from_the_decoder() {
        let bin = [0u8; 8];
        let json = json!({
            "asset": {"version": "2.0"},
            "extensionsUsed": ["KHR_draco_mesh_compression"],
            "buffers": [{"byteLength": 8}],
            "bufferViews": [{"buffer": 0, "byteLength": 8}],
            "accessors": [
                {"componentType": 5126, "count": 2, "type": "VEC3"},
                {"componentType": 5121, "normalized": true, "count": 2, "type": "VEC3"}
            ],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "COLOR_0": 1}, "mode": 0, "extensions": {
                "KHR_draco_mesh_compression": {"bufferView": 0, "attributes": {"POSITION": 3, "COLOR_0": 5}}
            }}]}],
            "nodes": [{"mesh": 0}]
        });
        let bytes = super::super::tests::glb(&json, &bin);
        let requests = super::super::draco_requests(&bytes).unwrap();
        let r = &requests[0];

        assert!(r.points);
        assert_eq!((r.position, r.color, r.normal, r.uv), (3, Some(5), None, None));

        let geometry = DracoGeometry {
            positions: vec![0.0, 0.0, 0.0, 2.0, 0.0, 0.0],
            colors: vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            ..DracoGeometry::default()
        };
        let mesh = super::super::decode(&bytes, &DMat4::IDENTITY, false, &[geometry]).unwrap();

        assert_eq!(mesh.points[0].colors, [[255, 0, 0, 255], [0, 255, 0, 255]]);
        assert!((mesh.anchor - DVec3::X).length() < 1e-9);
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
