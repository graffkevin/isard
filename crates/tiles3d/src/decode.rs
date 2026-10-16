//! Decoding of 3D Tiles content: Batched 3D Model (`.b3dm`, 3D Tiles 1.0) and binary glTF (`.glb`, 3D Tiles 1.1),
//! into triangle meshes ready for the GPU. Pure Rust, no I/O: images stay encoded, the browser decodes them.
//!
//! Positions go through the glTF node transforms, the y-up to z-up rotation, `RTC_CENTER` and the tile transform, to
//! ECEF; they are stored in f32 relative to an anchor (the tile's center), exact to the millimetre over tens of
//! kilometres.
//!
//! Content comes from the network: every size and offset is checked (overflow included) before it is used or
//! allocated, so a malformed tile is an error, never a panic.

#![forbid(unsafe_code)]

use crate::meshopt;
use glam::{DMat4, DQuat, DVec2, DVec3};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// One textured triangle mesh
pub struct Primitive {
    /// x, y, z relative to the anchor (ECEF axes, metres)
    pub positions: Vec<f32>,
    /// u, v per vertex (zeros without texture coordinates)
    pub uvs: Vec<f32>,
    pub indices: Vec<u32>,
    /// Encoded image (JPEG, PNG, WebP) and its media type
    pub image: Option<(Vec<u8>, String)>,
}

pub struct Mesh {
    /// ECEF origin of the positions
    pub anchor: DVec3,
    pub primitives: Vec<Primitive>,
}

/// A Draco-compressed primitive (`KHR_draco_mesh_compression`), for an external Draco decoder
pub struct DracoRequest {
    pub mesh: usize,
    pub primitive: usize,
    /// The compressed geometry
    pub data: Vec<u8>,
    /// Draco attribute ids
    pub position: u32,
    pub uv: Option<u32>,
}

/// What a Draco decoder returns for a request: positions and uv as stored (before the node transforms), triangles
pub struct DracoGeometry {
    pub mesh: usize,
    pub primitive: usize,
    pub positions: Vec<f32>,
    pub uvs: Vec<f32>,
    pub indices: Vec<u32>,
}

/// Decodes a `.b3dm` or `.glb` tile. `transform`: the tile's world transform; `y_up`: glTF y-up to 3D Tiles z-up
/// (always for `.b3dm` of 3D Tiles 1.0, unless the tileset says `gltfUpAxis: Z`).
/// Draco primitives are decoded beforehand by the caller (`draco_requests`) and passed in `draco`.
pub fn decode(bytes: &[u8], transform: &DMat4, y_up: bool, draco: &[DracoGeometry]) -> Result<Mesh, String> {
    let (glb, rtc) = split(bytes)?;
    decode_glb(glb, transform, y_up, rtc, draco)
}

/// The Draco-compressed primitives of a tile, to decode before `decode`
pub fn draco_requests(bytes: &[u8]) -> Result<Vec<DracoRequest>, String> {
    let (glb, _) = split(bytes)?;
    let (json, bin) = parse_glb(glb)?;
    let gltf = Gltf::new(&json, bin, &[]);
    let mut out = Vec::new();
    for (m, mesh) in json["meshes"].as_array().into_iter().flatten().enumerate() {
        for (p, primitive) in mesh["primitives"].as_array().into_iter().flatten().enumerate() {
            let ext = &primitive["extensions"]["KHR_draco_mesh_compression"];
            if !ext.is_object() {
                continue;
            }
            let view = index(&ext["bufferView"]).ok_or("Draco extension without bufferView")?;
            let attribute = |name: &str| ext["attributes"][name].as_u64().and_then(|v| u32::try_from(v).ok());
            out.push(DracoRequest {
                mesh: m,
                primitive: p,
                data: gltf.view(view)?.to_vec(),
                position: attribute("POSITION").ok_or("Draco without POSITION")?,
                uv: attribute(&format!("TEXCOORD_{}", uv_set(&json, primitive))),
            });
        }
    }
    Ok(out)
}

/// The glTF binary of a tile and its `RTC_CENTER`
fn split(bytes: &[u8]) -> Result<(&[u8], DVec3), String> {
    match bytes.get(..4) {
        Some(b"b3dm") => b3dm_glb(bytes),
        Some(b"glTF") => Ok((bytes, DVec3::ZERO)),
        _ if bytes.trim_ascii_start().starts_with(b"{") => {
            Err("a JSON document (external tileset?), not a mesh".into())
        }
        _ => Err("unsupported tile content (b3dm and glb only)".into()),
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<usize, String> {
    let b = slice(bytes, offset, 4).ok_or("truncated tile")?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// `length` bytes from `start`, None if out of `bytes` (overflow included)
fn slice(bytes: &[u8], start: usize, length: usize) -> Option<&[u8]> {
    bytes.get(start..start.checked_add(length)?)
}

/// A JSON chunk without the padding some writers leave (spaces, zeros)
fn trim_padding(text: &[u8]) -> &[u8] {
    let end = text
        .iter()
        .rposition(|&b| b != 0 && !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    &text[..end]
}

fn b3dm_glb(bytes: &[u8]) -> Result<(&[u8], DVec3), String> {
    let feature_json = u32_at(bytes, 12)?;
    let feature_binary = u32_at(bytes, 16)?;
    let batch_json = u32_at(bytes, 20)?;
    let batch_binary = u32_at(bytes, 24)?;
    let mut rtc = DVec3::ZERO;
    if feature_json > 0 {
        let text = slice(bytes, 28, feature_json).ok_or("truncated feature table")?;
        let table: Value = serde_json::from_slice(trim_padding(text)).map_err(|e| e.to_string())?;
        let center = &table["RTC_CENTER"];
        if let Some([x, y, z]) = center.as_array().map(Vec::as_slice) {
            rtc = DVec3::new(
                x.as_f64().unwrap_or(0.0),
                y.as_f64().unwrap_or(0.0),
                z.as_f64().unwrap_or(0.0),
            );
        } else if !center.is_null() {
            // In the feature table's binary body: three f32
            let binary = slice(bytes, 28 + feature_json, feature_binary).ok_or("truncated feature table")?;
            let b = index(&center["byteOffset"])
                .and_then(|offset| slice(binary, offset, 12))
                .ok_or("RTC_CENTER out of the feature table")?;
            let f = |i: usize| f64::from(f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]));
            rtc = DVec3::new(f(0), f(4), f(8));
        }
    }
    let glb = [feature_json, feature_binary, batch_json, batch_binary]
        .into_iter()
        .try_fold(28usize, usize::checked_add)
        .ok_or("bad b3dm header")?;
    Ok((bytes.get(glb..).ok_or("truncated b3dm")?, rtc))
}

/// Chunk types of a glTF binary
const JSON_CHUNK: usize = 0x4E4F_534A;
const BIN_CHUNK: usize = 0x004E_4942;

/// JSON and binary chunk of a glTF binary
fn parse_glb(bytes: &[u8]) -> Result<(Value, &[u8]), String> {
    if bytes.get(..4) != Some(b"glTF") || u32_at(bytes, 4)? != 2 {
        return Err("glTF 2.0 binary expected".into());
    }
    // The declared length bounds the chunks (trailing bytes are not part of it)
    let bytes = &bytes[..u32_at(bytes, 8)?.min(bytes.len())];
    let json_length = u32_at(bytes, 12)?;
    if u32_at(bytes, 16)? != JSON_CHUNK {
        return Err("glTF binary without its JSON chunk".into());
    }
    let text = slice(bytes, 20, json_length).ok_or("truncated glTF")?;
    let json: Value = serde_json::from_slice(trim_padding(text)).map_err(|e| e.to_string())?;
    let bin_start = 20 + json_length;
    // The binary chunk, if any, comes second; another chunk type there is not it
    let bin = match (u32_at(bytes, bin_start), u32_at(bytes, bin_start + 4)) {
        (Ok(length), Ok(BIN_CHUNK)) => slice(bytes, bin_start + 8, length).ok_or("truncated glTF binary")?,
        _ => &[],
    };
    Ok((json, bin))
}

/// A JSON index or count as usize (None if absent, negative or too large for this platform)
fn index(v: &Value) -> Option<usize> {
    v.as_u64().and_then(|n| usize::try_from(n).ok())
}

/// A byte offset: 0 when absent, an error when invalid
fn offset(v: &Value) -> Result<usize, String> {
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
fn uv_set(json: &Value, primitive: &Value) -> u64 {
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

fn decode_glb(
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
/// Most values of an accessor without buffer view (zeros, possibly sparse)
const MAX_ZEROS: usize = 1 << 22;

/// Layout of an accessor
struct Layout {
    count: usize,
    components: usize,
    component_type: u64,
    /// Bytes per component
    size: usize,
    normalized: bool,
}

impl Layout {
    fn parse(accessor: &Value) -> Result<Self, String> {
        let count = index(&accessor["count"]).ok_or("accessor without count")?;
        let components = match accessor["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            _ => return Err("unsupported accessor type".into()),
        };
        let component_type = accessor["componentType"].as_u64().unwrap_or(0);
        Ok(Self {
            count,
            components,
            component_type,
            size: component_size(component_type)?,
            normalized: accessor["normalized"].as_bool().unwrap_or(false),
        })
    }

    /// Bytes per element
    fn element(&self) -> usize {
        self.components * self.size
    }
}

fn component_size(component_type: u64) -> Result<usize, String> {
    match component_type {
        5120 | 5121 => Ok(1),
        5122 | 5123 => Ok(2),
        5125 | 5126 => Ok(4),
        _ => Err(format!("unsupported component type {component_type}")),
    }
}

/// Reads one component from its little-endian bytes (at least its size)
type Read<T> = fn(&[u8]) -> T;

/// Components as f32: normalized integers mapped to [0, 1] or [-1, 1], other integers kept as values
fn float_reader(component_type: u64, normalized: bool) -> Result<Read<f32>, String> {
    Ok(match (component_type, normalized) {
        (5126, _) => |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        (5121, true) => |b| f32::from(b[0]) / 255.0,
        (5121, false) => |b| f32::from(b[0]),
        (5120, true) => |b| (f32::from(b[0] as i8) / 127.0).max(-1.0),
        (5120, false) => |b| f32::from(b[0] as i8),
        (5123, true) => |b| f32::from(u16::from_le_bytes([b[0], b[1]])) / 65535.0,
        (5123, false) => |b| f32::from(u16::from_le_bytes([b[0], b[1]])),
        (5122, true) => |b| (f32::from(i16::from_le_bytes([b[0], b[1]])) / 32767.0).max(-1.0),
        (5122, false) => |b| f32::from(i16::from_le_bytes([b[0], b[1]])),
        (5125, _) => |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
        _ => return Err(format!("unsupported component type {component_type}")),
    })
}

/// Unsigned integer components (indices), exact
fn index_reader(component_type: u64) -> Result<Read<u32>, String> {
    Ok(match component_type {
        5121 => |b| u32::from(b[0]),
        5123 => |b| u32::from(u16::from_le_bytes([b[0], b[1]])),
        5125 => |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        _ => return Err(format!("indices of component type {component_type}")),
    })
}

/// Whether `count` elements of `element` bytes, `stride` apart from `offset`, fit in `length` bytes
fn fits(length: usize, offset: usize, stride: usize, count: usize, element: usize) -> bool {
    count == 0
        || (count - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(offset))
            .and_then(|n| n.checked_add(element))
            .is_some_and(|end| end <= length)
}

struct Gltf<'a> {
    json: &'a Value,
    bin: &'a [u8],
    /// Draco primitives decoded by the caller
    draco: &'a [DracoGeometry],
    /// Buffer views decompressed so far (`EXT_meshopt_compression`)
    views: RefCell<HashMap<usize, Rc<[u8]>>>,
    /// Node visits left
    visits: Cell<usize>,
}

impl<'a> Gltf<'a> {
    fn new(json: &'a Value, bin: &'a [u8], draco: &'a [DracoGeometry]) -> Self {
        let nodes = json["nodes"].as_array().map_or(0, Vec::len);
        Self {
            json,
            bin,
            draco,
            views: RefCell::default(),
            visits: Cell::new(nodes.saturating_mul(VISITS_PER_NODE)),
        }
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
        let uvs = match self::index(uv) {
            Some(uv) => self.floats(uv)?,
            None => vec![0.0; count * 2],
        };
        let indices: Vec<u32> = match self::index(&primitive["indices"]) {
            Some(i) => self.accessor(i, |l| index_reader(l.component_type))?,
            None => (0..u32::try_from(count).map_err(|_| "too many vertices")?).collect(),
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
    fn view(&self, index: usize) -> Result<Rc<[u8]>, String> {
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

    /// Accessor values as f32 (normalized integers mapped to [0, 1] or [-1, 1], other integers kept as values)
    fn floats(&self, index: usize) -> Result<Vec<f32>, String> {
        self.accessor(index, |l| float_reader(l.component_type, l.normalized))
    }

    /// Values of an accessor, read by `reader`: from its buffer view (interleaved or not), or zeros without one, then
    /// its sparse substitutions
    fn accessor<T: Copy + Default>(
        &self,
        index: usize,
        reader: impl Fn(&Layout) -> Result<Read<T>, String>,
    ) -> Result<Vec<T>, String> {
        let accessor = &self.json["accessors"][index];
        let layout = Layout::parse(accessor)?;
        let read = reader(&layout)?;
        let (count, components, size, element) = (layout.count, layout.components, layout.size, layout.element());
        let total = count.checked_mul(components).ok_or("accessor too large")?;
        let mut out = if let Some(view_index) = self::index(&accessor["bufferView"]) {
            let bytes = self.view(view_index)?;
            let start = offset(&accessor["byteOffset"])?;
            let stride = match &self.json["bufferViews"][view_index]["byteStride"] {
                Value::Null => element,
                s => self::index(s).filter(|&s| s >= element).ok_or("bad byteStride")?,
            };
            if !fits(bytes.len(), start, stride, count, element) {
                return Err("accessor out of its buffer view".into());
            }
            let mut out = Vec::with_capacity(total);
            for i in 0..count {
                let e = slice(&bytes, start + i * stride, element).ok_or("accessor out of its buffer view")?;
                out.extend((0..components).map(|c| read(&e[c * size..])));
            }
            out
        } else if total <= MAX_ZEROS {
            vec![T::default(); total]
        } else {
            return Err("accessor without buffer view too large".into());
        };
        let sparse = &accessor["sparse"];
        if !sparse.is_null() {
            self.sparse(sparse, &layout, read, &mut out)?;
        }
        Ok(out)
    }

    /// Applies the substitutions of a sparse accessor: values (tightly packed) at the given element indices
    fn sparse<T: Copy>(&self, sparse: &Value, layout: &Layout, read: Read<T>, out: &mut [T]) -> Result<(), String> {
        let count = index(&sparse["count"]).ok_or("sparse accessor without count")?;
        if count > layout.count {
            return Err("sparse accessor with too many values".into());
        }
        let (indices, values) = (&sparse["indices"], &sparse["values"]);
        let index_type = indices["componentType"].as_u64().unwrap_or(0);
        let (index_read, index_size) = (index_reader(index_type)?, component_size(index_type)?);
        let index_bytes = self.view(index(&indices["bufferView"]).ok_or("sparse indices without bufferView")?)?;
        let index_start = offset(&indices["byteOffset"])?;
        let value_bytes = self.view(index(&values["bufferView"]).ok_or("sparse values without bufferView")?)?;
        let value_start = offset(&values["byteOffset"])?;
        let (components, size, element) = (layout.components, layout.size, layout.element());
        if !fits(index_bytes.len(), index_start, index_size, count, index_size)
            || !fits(value_bytes.len(), value_start, element, count, element)
        {
            return Err("sparse accessor out of its buffer views".into());
        }
        for k in 0..count {
            let i = slice(&index_bytes, index_start + k * index_size, index_size)
                .map(index_read)
                .ok_or("sparse accessor out of its buffer views")? as usize;
            let v = slice(&value_bytes, value_start + k * element, element).ok_or("sparse values out of range")?;
            let target = out
                .get_mut(i.saturating_mul(components)..)
                .and_then(|o| o.get_mut(..components))
                .ok_or("sparse index out of the accessor")?;
            for (c, t) in target.iter_mut().enumerate() {
                *t = read(&v[c * size..]);
            }
        }
        Ok(())
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
    use super::*;
    use geo::geodesy;

    /// A tile of the Lille photomesh (Métropole Européenne de Lille, 2020, 5 cm)
    const LILLE: &[u8] = include_bytes!("../tests/data/lille_L11_00.b3dm");

    #[test]
    fn decodes_a_lille_tile() {
        let mesh = decode(LILLE, &DMat4::IDENTITY, true, &[]).unwrap();
        assert_eq!(mesh.primitives.len(), 1);
        let p = &mesh.primitives[0];
        assert_eq!(p.positions.len(), 59 * 3);
        assert_eq!(p.uvs.len(), 59 * 2);
        assert_eq!(p.indices.len(), 171);
        let (bytes, mime) = p.image.as_ref().unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
        // Near Lille, close to the ellipsoid
        let g = geodesy::from_ecef(mesh.anchor);
        assert!(
            (g.x - 2.9).abs() < 0.5 && (g.y - 50.6).abs() < 0.3 && g.z.abs() < 1000.0,
            "{g}"
        );
    }

    #[test]
    fn y_up_puts_the_mesh_on_the_ground() {
        // With the y-up rotation the vertices hug the ellipsoid; without it they would be kilometres off
        let mesh = decode(LILLE, &DMat4::IDENTITY, true, &[]).unwrap();
        let p = &mesh.primitives[0].positions;
        for &[x, y, z] in p.as_chunks::<3>().0 {
            let ecef = mesh.anchor + DVec3::new(f64::from(x), f64::from(y), f64::from(z));
            assert!(geodesy::from_ecef(ecef).z.abs() < 500.0);
        }
    }

    /// The same tile compressed by gltfpack: meshopt alone, meshopt with 14-bit quantization, with the exponential
    /// filter (`EXT_meshopt_compression`, `KHR_mesh_quantization`, `KHR_texture_transform`)
    #[test]
    fn meshopt_variants_decode_like_the_original() {
        let original = decode(LILLE, &DMat4::IDENTITY, true, &[]).unwrap();
        let o = &original.primitives[0];
        for (name, bytes, tolerance) in [
            ("noq", &include_bytes!("../tests/data/lille_noq.glb")[..], 0.01),
            ("q", &include_bytes!("../tests/data/lille_q.glb")[..], 0.6),
            ("vpf", &include_bytes!("../tests/data/lille_vpf.glb")[..], 0.6),
        ] {
            // The glb were extracted from the b3dm without its RTC_CENTER: given back as the tile transform
            let rtc = DMat4::from_translation(DVec3::new(
                4_056_896.270_481_2,
                202_047.449_822_138,
                4_900_910.163_547_64,
            ));
            let mesh = decode(bytes, &rtc, true, &[]).unwrap();
            let p = &mesh.primitives[0];
            assert_eq!(p.indices.len(), o.indices.len());
            assert!(p.image.is_some());
            let shift = mesh.anchor - original.anchor;
            let at = |positions: &[f32], i: u32| {
                let i = i as usize * 3;
                DVec3::new(
                    f64::from(positions[i]),
                    f64::from(positions[i + 1]),
                    f64::from(positions[i + 2]),
                )
            };
            // Every decoded triangle has a counterpart in the original within the quantization step
            for t in p.indices.as_chunks::<3>().0 {
                let corners = t.map(|i| at(&p.positions, i) + shift);
                let matched = o.indices.as_chunks::<3>().0.iter().any(|u| {
                    corners
                        .iter()
                        .all(|c| u.iter().any(|&j| (at(&o.positions, j) - *c).length() < tolerance))
                });
                if !matched {
                    let nearest = corners.map(|c| {
                        (0..o.positions.len() as u32 / 3)
                            .map(|j| (at(&o.positions, j) - c).length())
                            .fold(f64::MAX, f64::min)
                    });
                    panic!("{name}: triangle {t:?} without counterpart, nearest original vertices {nearest:?}");
                }
            }
            // Same texture coordinates range once the texture transform is applied
            let range = |uvs: &[f32]| uvs.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
            let (a, b) = (range(&p.uvs), range(&o.uvs));
            assert!((a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3, "{a:?} vs {b:?}");
        }
    }

    /// The tile compressed with Draco (gltf-pipeline): its geometry is decoded outside (workers), then given back
    #[test]
    fn draco_primitives_are_requested_then_used() {
        let bytes = include_bytes!("../tests/data/lille_draco.glb");
        let requests = draco_requests(bytes).unwrap();
        assert_eq!(requests.len(), 1);
        let r = &requests[0];
        assert_eq!((r.mesh, r.primitive, r.position, r.uv), (0, 0, 0, Some(1)));
        assert!(r.data.len() > 100);
        // Without the decoded geometry, the tile cannot be built
        assert!(decode(bytes, &DMat4::IDENTITY, true, &[]).is_err());
        // With it (here a single triangle standing in for the Draco output), the usual path applies
        let geometry = DracoGeometry {
            mesh: 0,
            primitive: 0,
            positions: vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0],
            uvs: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            indices: vec![0, 1, 2],
        };
        let mesh = decode(bytes, &DMat4::IDENTITY, true, &[geometry]).unwrap();
        assert_eq!(mesh.primitives[0].indices, [0, 1, 2]);
        assert!(mesh.primitives[0].image.is_some());
    }

    /// The tile with a KTX2 texture (`KHR_texture_basisu`): the KTX2 image is the one handed over
    #[test]
    fn basisu_textures_prefer_the_ktx2_image() {
        let mesh = decode(
            include_bytes!("../tests/data/lille_ktx2.glb"),
            &DMat4::IDENTITY,
            true,
            &[],
        )
        .unwrap();
        let (bytes, mime) = mesh.primitives[0].image.as_ref().unwrap();
        assert_eq!(mime, "image/ktx2");
        assert_eq!(&bytes[..12], b"\xabKTX 20\xbb\r\n\x1a\n");
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"nope", &DMat4::IDENTITY, true, &[]).is_err());
        assert!(decode(&LILLE[..40], &DMat4::IDENTITY, true, &[]).is_err());
    }

    use serde_json::json;

    /// A glTF binary from its JSON and binary chunk
    pub(crate) fn glb(json: &Value, bin: &[u8]) -> Vec<u8> {
        let mut text = serde_json::to_vec(json).unwrap();
        while !text.len().is_multiple_of(4) {
            text.push(b' ');
        }
        let mut bin = bin.to_vec();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let total = 20 + text.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
        let mut out = Vec::new();
        for v in [0x4654_6C67, 2, total, text.len(), JSON_CHUNK] {
            out.extend((v as u32).to_le_bytes());
        }
        out.extend(text);
        if !bin.is_empty() {
            out.extend((bin.len() as u32).to_le_bytes());
            out.extend((BIN_CHUNK as u32).to_le_bytes());
            out.extend(bin);
        }
        out
    }

    fn f32s(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// One triangle: positions (x, y, z) then uv in a buffer, its accessors and node given by `json`
    fn triangle(json: &Value, bin: &[u8]) -> Result<Mesh, String> {
        decode(&glb(json, bin), &DMat4::IDENTITY, false, &[])
    }

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

    /// Vertex `i` of the first primitive, ECEF
    fn vertex(mesh: &Mesh, i: usize) -> DVec3 {
        let p = &mesh.primitives[0].positions[i * 3..i * 3 + 3];
        mesh.anchor + DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
    }

    #[test]
    fn sparse_accessors_substitute_their_values() {
        // Base positions all at the origin but the substituted vertices 1 and 2; uv without buffer view (zeros) and
        // one sparse value
        let mut bin = f32s(&[0.0; 9]);
        bin.extend(f32s(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0])); // 36: values
        bin.extend([1u8, 0, 2, 0]); // 60: u16 indices
        bin.extend(f32s(&[0.5, 0.25])); // 64: uv value
        bin.extend([2u8, 0, 0, 0]); // 72: u8 index
        let json = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 76}],
            "bufferViews": [
                {"buffer": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 24},
                {"buffer": 0, "byteOffset": 60, "byteLength": 4},
                {"buffer": 0, "byteOffset": 64, "byteLength": 8},
                {"buffer": 0, "byteOffset": 72, "byteLength": 1}
            ],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "sparse": {
                    "count": 2,
                    "indices": {"bufferView": 2, "componentType": 5123},
                    "values": {"bufferView": 1}
                }},
                {"componentType": 5126, "count": 3, "type": "VEC2", "sparse": {
                    "count": 1,
                    "indices": {"bufferView": 4, "componentType": 5121},
                    "values": {"bufferView": 3}
                }}
            ],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1}}]}],
            "nodes": [{"mesh": 0}]
        });
        let mesh = triangle(&json, &bin).unwrap();
        let p = &mesh.primitives[0];
        let at = |i: usize| vertex(&mesh, i);
        assert!(at(0).length() < 1e-6 && (at(1) - DVec3::X).length() < 1e-6 && (at(2) - DVec3::Y).length() < 1e-6);
        assert_eq!(p.uvs, [0.0, 0.0, 0.0, 0.0, 0.5, 0.25]);
        // A sparse index past the accessor, too many sparse values: errors
        let mut bad = json.clone();
        bad["accessors"][1]["sparse"]["indices"]["componentType"] = json!(5125);
        assert!(triangle(&bad, &bin).is_err());
        let mut bad = json;
        bad["accessors"][0]["sparse"]["count"] = json!(4);
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
    fn rtc_center_may_be_binary() {
        // A b3dm whose feature table gives RTC_CENTER in its binary body
        let inner = glb(
            &json!({
                "asset": {"version": "2.0"},
                "buffers": [{"byteLength": 36}],
                "bufferViews": [{"buffer": 0, "byteLength": 36}],
                "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}],
                "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
                "nodes": [{"mesh": 0}]
            }),
            &f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
        );
        let table = br#"{"BATCH_LENGTH":0,"RTC_CENTER":{"byteOffset":0}}    "#;
        let binary = f32s(&[1000.0, 2000.0, 3000.0]);
        let mut b3dm = b"b3dm".to_vec();
        let total = 28 + table.len() + binary.len() + inner.len();
        for v in [1, total, table.len(), binary.len(), 0, 0] {
            b3dm.extend((v as u32).to_le_bytes());
        }
        b3dm.extend(table);
        b3dm.extend(binary);
        b3dm.extend(&inner);
        let mesh = decode(&b3dm, &DMat4::IDENTITY, false, &[]).unwrap();
        assert!(
            (mesh.anchor - DVec3::new(1000.5, 2000.5, 3000.0)).length() < 1e-6,
            "{}",
            mesh.anchor
        );
        // Its offset out of the binary body: an error
        let mut bad = b3dm.clone();
        bad[28 + table.len() - 7] = b'9';
        assert!(decode(&bad, &DMat4::IDENTITY, false, &[]).is_err());
        // Header sizes that overflow
        let mut bad = b3dm;
        bad[12..28].fill(0xff);
        assert!(decode(&bad, &DMat4::IDENTITY, false, &[]).is_err());
    }

    #[test]
    fn only_a_bin_chunk_is_the_binary() {
        let mut bytes = with_nodes_glb();
        assert!(decode(&bytes, &DMat4::IDENTITY, false, &[]).is_ok());
        // Change the type of the second chunk: no binary any more, the accessor is out of range
        let json_length = u32_at(&bytes, 12).unwrap();
        bytes[20 + json_length + 4] = b'X';
        assert!(decode(&bytes, &DMat4::IDENTITY, false, &[]).is_err());
        // A JSON document is told apart
        assert!(
            decode(b"  {\"asset\":{}}", &DMat4::IDENTITY, false, &[])
                .err()
                .unwrap()
                .contains("JSON")
        );
    }

    fn with_nodes_glb() -> Vec<u8> {
        glb(
            &json!({
                "asset": {"version": "2.0"},
                "buffers": [{"byteLength": 36}],
                "bufferViews": [{"buffer": 0, "byteLength": 36}],
                "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}],
                "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
                "nodes": [{"mesh": 0}]
            }),
            &f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
        )
    }

    /// Deterministic pseudo-random numbers (xorshift)
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    /// JSON pointers of every value of a document
    fn pointers(v: &Value, at: String, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, v) in o {
                    pointers(v, format!("{at}/{k}"), out);
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    pointers(v, format!("{at}/{i}"), out);
                }
            }
            _ => {}
        }
        out.push(at);
    }

    /// Values a malicious or broken file may hold
    fn odd_value(r: &mut Random) -> Value {
        let values = [
            json!(0),
            json!(1),
            json!(2),
            json!(3),
            json!(4),
            json!(7),
            json!(255),
            json!(65_535),
            json!(u32::MAX),
            json!(u64::from(u32::MAX) + 2),
            json!(u64::MAX),
            json!(-1),
            json!(1e300),
            json!(0.5),
            json!("VEC3"),
            json!("SCALAR"),
            json!("MAT4"),
            json!("ATTRIBUTES"),
            json!("TRIANGLES"),
            json!("INDICES"),
            json!("OCTAHEDRAL"),
            json!("QUATERNION"),
            json!(null),
            json!([]),
            json!([0, 0]),
            json!([0, 1, 0, 1]),
            json!(vec![1.0; 16]),
            json!({"count": 2, "indices": {"bufferView": 0, "componentType": 5123}, "values": {"bufferView": 1}}),
            json!(5120),
            json!(5121),
            json!(5122),
            json!(5123),
            json!(5125),
            json!(5126),
        ];
        values[r.below(values.len())].clone()
    }

    /// Keys an attacker may add to any object
    const KEYS: [&str; 12] = [
        "sparse",
        "byteStride",
        "byteOffset",
        "count",
        "matrix",
        "children",
        "mesh",
        "bufferView",
        "filter",
        "texCoord",
        "translation",
        "rotation",
    ];

    fn try_all(bytes: &[u8]) {
        let _ = decode(bytes, &DMat4::IDENTITY, true, &[]);
        if let Ok(requests) = draco_requests(bytes) {
            // Stand-in geometry for the Draco primitives, possibly inconsistent
            let draco: Vec<DracoGeometry> = requests
                .iter()
                .map(|r| DracoGeometry {
                    mesh: r.mesh,
                    primitive: r.primitive,
                    positions: vec![0.0; 9],
                    uvs: vec![0.0; 4],
                    indices: vec![0, 1, 5],
                })
                .collect();
            let _ = decode(bytes, &DMat4::IDENTITY, true, &draco);
        }
    }

    /// Mutated fixtures (bytes flipped, truncated, JSON fields given odd values) never panic, hang or allocate
    /// wildly: every outcome is a mesh or an error
    #[test]
    fn malformed_tiles_never_panic() {
        let fixtures: [&[u8]; 6] = [
            LILLE,
            include_bytes!("../tests/data/lille_noq.glb"),
            include_bytes!("../tests/data/lille_q.glb"),
            include_bytes!("../tests/data/lille_vpf.glb"),
            include_bytes!("../tests/data/lille_draco.glb"),
            include_bytes!("../tests/data/lille_ktx2.glb"),
        ];
        let mut r = Random(0x9E37_79B9_7F4A_7C15);
        for fixture in fixtures {
            // Bytes
            for _ in 0..300 {
                let mut bytes = fixture.to_vec();
                match r.below(3) {
                    0 => bytes.truncate(r.below(fixture.len())),
                    1 => {
                        for _ in 0..=r.below(8) {
                            let i = r.below(bytes.len());
                            bytes[i] ^= 1 << r.below(8);
                        }
                    }
                    _ => {
                        // A header or chunk length overwritten
                        let i = r.below(40);
                        let v = [0u32, 1, 0xffff_fff0, u32::MAX, fixture.len() as u32][r.below(5)];
                        bytes[i..i + 4].copy_from_slice(&v.to_le_bytes());
                    }
                }
                try_all(&bytes);
            }
            // JSON fields
            let (glb_bytes, _) = split(fixture).unwrap();
            let (json, bin) = parse_glb(glb_bytes).unwrap();
            let mut paths = Vec::new();
            pointers(&json, String::new(), &mut paths);
            for _ in 0..1000 {
                let mut json = json.clone();
                for _ in 0..=r.below(3) {
                    let path = &paths[r.below(paths.len())];
                    if r.below(4) == 0 {
                        if let Some(Value::Object(o)) = json.pointer_mut(path) {
                            o.insert(KEYS[r.below(KEYS.len())].to_owned(), odd_value(&mut r));
                        }
                    } else if let Some(v) = json.pointer_mut(path) {
                        *v = odd_value(&mut r);
                    }
                }
                try_all(&glb(&json, bin));
            }
        }
    }
}
