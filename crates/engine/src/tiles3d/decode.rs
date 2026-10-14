//! Decoding of 3D Tiles content: Batched 3D Model (`.b3dm`, 3D Tiles 1.0) and binary glTF (`.glb`, 3D Tiles 1.1),
//! into triangle meshes ready for the GPU. Pure Rust, no I/O: images stay encoded, the browser decodes them.
//!
//! Positions go through the glTF node transforms, the y-up to z-up rotation, `RTC_CENTER` and the tile transform, to
//! ECEF; they are stored in f32 relative to an anchor (the tile's center), exact to the millimetre over tens of
//! kilometres.

#![forbid(unsafe_code)]

use crate::tiles3d::meshopt;
use glam::{DMat4, DQuat, DVec2, DVec3};
use serde_json::Value;
use std::cell::RefCell;
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

/// Decodes a `.b3dm` or `.glb` tile. `transform`: the tile's world transform; `y_up`: glTF y-up to 3D Tiles z-up
/// (always for `.b3dm` of 3D Tiles 1.0, unless the tileset says `gltfUpAxis: Z`).
pub fn decode(bytes: &[u8], transform: &DMat4, y_up: bool) -> Result<Mesh, String> {
    match bytes.get(..4) {
        Some(b"b3dm") => decode_b3dm(bytes, transform, y_up),
        Some(b"glTF") => decode_glb(bytes, transform, y_up, DVec3::ZERO),
        _ => Err("unsupported tile content (b3dm and glb only)".into()),
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<usize, String> {
    let b = bytes.get(offset..offset + 4).ok_or("truncated tile")?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn decode_b3dm(bytes: &[u8], transform: &DMat4, y_up: bool) -> Result<Mesh, String> {
    let feature_json = u32_at(bytes, 12)?;
    let feature_binary = u32_at(bytes, 16)?;
    let batch_json = u32_at(bytes, 20)?;
    let batch_binary = u32_at(bytes, 24)?;
    let mut rtc = DVec3::ZERO;
    if feature_json > 0 {
        let text = bytes.get(28..28 + feature_json).ok_or("truncated feature table")?;
        let table: Value = serde_json::from_slice(text).map_err(|e| e.to_string())?;
        if let Some([x, y, z]) = table["RTC_CENTER"].as_array().map(Vec::as_slice) {
            rtc = DVec3::new(
                x.as_f64().unwrap_or(0.0),
                y.as_f64().unwrap_or(0.0),
                z.as_f64().unwrap_or(0.0),
            );
        }
    }
    let glb = 28 + feature_json + feature_binary + batch_json + batch_binary;
    decode_glb(bytes.get(glb..).ok_or("truncated b3dm")?, transform, y_up, rtc)
}

fn decode_glb(bytes: &[u8], transform: &DMat4, y_up: bool, rtc: DVec3) -> Result<Mesh, String> {
    if bytes.get(..4) != Some(b"glTF") || u32_at(bytes, 4)? != 2 {
        return Err("glTF 2.0 binary expected".into());
    }
    let json_length = u32_at(bytes, 12)?;
    let json: Value =
        serde_json::from_slice(bytes.get(20..20 + json_length).ok_or("truncated glTF")?).map_err(|e| e.to_string())?;
    let bin_start = 20 + json_length;
    let bin = if bytes.len() > bin_start + 8 {
        let length = u32_at(bytes, bin_start)?;
        bytes
            .get(bin_start + 8..bin_start + 8 + length)
            .ok_or("truncated glTF binary")?
    } else {
        &[]
    };
    for required in json["extensionsRequired"].as_array().into_iter().flatten() {
        let name = required.as_str().unwrap_or("");
        if !SUPPORTED.contains(&name) {
            return Err(format!("glTF extension {name} unsupported"));
        }
    }
    let gltf = Gltf {
        json: &json,
        bin,
        views: RefCell::default(),
    };
    // glTF y-up → z-up: +90° about x
    let axis = if y_up {
        DMat4::from_rotation_x(std::f64::consts::FRAC_PI_2)
    } else {
        DMat4::IDENTITY
    };
    let to_world = *transform * DMat4::from_translation(rtc) * axis;
    let mut out = Vec::new();
    let scene = json["scene"].as_u64().unwrap_or(0) as usize;
    let roots: Vec<usize> = match json["scenes"][scene]["nodes"].as_array() {
        Some(nodes) => nodes.iter().filter_map(Value::as_u64).map(|n| n as usize).collect(),
        None => (0..json["nodes"].as_array().map_or(0, Vec::len)).collect(),
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
    let anchor = if min.x.is_finite() {
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

/// glTF extensions this decoder handles (a tile requiring another one is refused)
const SUPPORTED: [&str; 4] = [
    "EXT_meshopt_compression",
    "KHR_mesh_quantization",
    "KHR_texture_transform",
    "KHR_materials_unlit",
];

struct Gltf<'a> {
    json: &'a Value,
    bin: &'a [u8],
    /// Buffer views decompressed so far (`EXT_meshopt_compression`)
    views: RefCell<HashMap<usize, Rc<[u8]>>>,
}

impl Gltf<'_> {
    fn node(
        &self,
        index: usize,
        parent: &DMat4,
        out: &mut Vec<(Primitive, Vec<DVec3>)>,
        depth: u32,
    ) -> Result<(), String> {
        if depth > 32 {
            return Err("glTF node hierarchy too deep".into());
        }
        let node = &self.json["nodes"][index];
        let local = if let Some(m) = node["matrix"].as_array() {
            let m: Vec<f64> = m.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect();
            DMat4::from_cols_slice(m.get(..16).ok_or("bad node matrix")?)
        } else {
            let v = |key: &str, default: &[f64]| -> Vec<f64> {
                node[key].as_array().map_or(default.to_vec(), |a| {
                    a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect()
                })
            };
            let (t, r, s) = (
                v("translation", &[0.0; 3]),
                v("rotation", &[0.0, 0.0, 0.0, 1.0]),
                v("scale", &[1.0; 3]),
            );
            DMat4::from_scale_rotation_translation(
                DVec3::new(s[0], s[1], s[2]),
                DQuat::from_xyzw(r[0], r[1], r[2], r[3]),
                DVec3::new(t[0], t[1], t[2]),
            )
        };
        let world = *parent * local;
        if let Some(mesh) = node["mesh"].as_u64() {
            for primitive in self.json["meshes"][mesh as usize]["primitives"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(p) = self.primitive(primitive, &world)? {
                    out.push(p);
                }
            }
        }
        for child in node["children"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
        {
            self.node(child as usize, &world, out, depth + 1)?;
        }
        Ok(())
    }

    /// Triangles only (mode 4, the default); other modes are skipped
    fn primitive(&self, primitive: &Value, world: &DMat4) -> Result<Option<(Primitive, Vec<DVec3>)>, String> {
        if primitive["mode"].as_u64().unwrap_or(4) != 4 {
            return Ok(None);
        }
        let Some(position) = primitive["attributes"]["POSITION"].as_u64() else {
            return Ok(None);
        };
        let positions = self.accessor(position as usize)?;
        let count = positions.len() / 3;
        let world_positions: Vec<DVec3> = positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[x, y, z]| world.transform_point3(DVec3::new(f64::from(x), f64::from(y), f64::from(z))))
            .collect();
        let mut uvs = match primitive["attributes"]["TEXCOORD_0"].as_u64() {
            Some(uv) => self.accessor(uv as usize)?,
            None => vec![0.0; count * 2],
        };
        let indices: Vec<u32> = match primitive["indices"].as_u64() {
            Some(i) => self.accessor(i as usize)?.iter().map(|&v| v as u32).collect(),
            None => (0..count as u32).collect(),
        };
        if uvs.len() != count * 2 || indices.iter().any(|&i| i as usize >= count) {
            return Err("inconsistent glTF primitive".into());
        }
        let material = &self.json["materials"][primitive["material"].as_u64().unwrap_or(u64::MAX) as usize];
        let base_color = &material["pbrMetallicRoughness"]["baseColorTexture"];
        let texture = base_color["index"].as_u64();
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
        let image = texture.and_then(|t| self.image(self.json["textures"][t as usize]["source"].as_u64()?));
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
        let bytes: Rc<[u8]> = if ext.is_object() {
            let offset = ext["byteOffset"].as_u64().unwrap_or(0) as usize;
            let length = ext["byteLength"].as_u64().ok_or("meshopt view without byteLength")? as usize;
            if ext["buffer"].as_u64().unwrap_or(0) != 0 {
                return Err("meshopt data outside the binary chunk".into());
            }
            let decoded = meshopt::decode(&meshopt::View {
                data: self
                    .bin
                    .get(offset..offset + length)
                    .ok_or("meshopt view out of the binary chunk")?,
                count: ext["count"].as_u64().ok_or("meshopt view without count")? as usize,
                stride: ext["byteStride"].as_u64().ok_or("meshopt view without byteStride")? as usize,
                mode: meshopt::Mode::parse(ext["mode"].as_str().unwrap_or(""))?,
                filter: meshopt::Filter::parse(ext["filter"].as_str())?,
            })?;
            decoded.into()
        } else {
            if view["buffer"].as_u64().unwrap_or(0) != 0 {
                return Err("buffer view outside the binary chunk".into());
            }
            let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
            let length = view["byteLength"].as_u64().ok_or("buffer view without byteLength")? as usize;
            self.bin
                .get(offset..offset + length)
                .ok_or("buffer view out of the binary chunk")?
                .into()
        };
        self.views.borrow_mut().insert(index, bytes.clone());
        Ok(bytes)
    }

    /// Accessor values as f32 (normalized integers mapped to [0, 1] or [-1, 1], other integers kept as values)
    fn accessor(&self, index: usize) -> Result<Vec<f32>, String> {
        let accessor = &self.json["accessors"][index];
        let count = accessor["count"].as_u64().ok_or("accessor without count")? as usize;
        let components = match accessor["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            _ => return Err("unsupported accessor type".into()),
        };
        let component_type = accessor["componentType"].as_u64().unwrap_or(0);
        let size = match component_type {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err(format!("unsupported component type {component_type}")),
        };
        let normalized = accessor["normalized"].as_bool().unwrap_or(false);
        let view_index = accessor["bufferView"].as_u64().ok_or("sparse accessors unsupported")? as usize;
        let bytes = self.view(view_index)?;
        let offset = accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = self.json["bufferViews"][view_index]["byteStride"]
            .as_u64()
            .map_or(components * size, |s| s as usize);
        let mut out = Vec::with_capacity(count * components);
        for i in 0..count {
            for c in 0..components {
                let at = offset + i * stride + c * size;
                let b = bytes.get(at..at + size).ok_or("accessor out of its buffer view")?;
                out.push(match (component_type, normalized) {
                    (5126, _) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    (5121, true) => f32::from(b[0]) / 255.0,
                    (5121, false) => f32::from(b[0]),
                    (5120, true) => (f32::from(b[0] as i8) / 127.0).max(-1.0),
                    (5120, false) => f32::from(b[0] as i8),
                    (5123, true) => f32::from(u16::from_le_bytes([b[0], b[1]])) / 65535.0,
                    (5123, false) => f32::from(u16::from_le_bytes([b[0], b[1]])),
                    (5122, true) => (f32::from(i16::from_le_bytes([b[0], b[1]])) / 32767.0).max(-1.0),
                    (5122, false) => f32::from(i16::from_le_bytes([b[0], b[1]])),
                    _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
                });
            }
        }
        Ok(out)
    }

    fn image(&self, index: u64) -> Option<(Vec<u8>, String)> {
        let image = &self.json["images"][index as usize];
        let bytes = self.view(image["bufferView"].as_u64()? as usize).ok()?;
        let mime = image["mimeType"].as_str().unwrap_or("image/jpeg").to_owned();
        Some((bytes.to_vec(), mime))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::geodesy;

    /// A tile of the Lille photomesh (Métropole Européenne de Lille, 2020, 5 cm)
    const LILLE: &[u8] = include_bytes!("../../tests/data/lille_L11_00.b3dm");

    #[test]
    fn decodes_a_lille_tile() {
        let mesh = decode(LILLE, &DMat4::IDENTITY, true).unwrap();
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
        let mesh = decode(LILLE, &DMat4::IDENTITY, true).unwrap();
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
        let original = decode(LILLE, &DMat4::IDENTITY, true).unwrap();
        let o = &original.primitives[0];
        for (name, bytes, tolerance) in [
            ("noq", &include_bytes!("../../tests/data/lille_noq.glb")[..], 0.01),
            ("q", &include_bytes!("../../tests/data/lille_q.glb")[..], 0.6),
            ("vpf", &include_bytes!("../../tests/data/lille_vpf.glb")[..], 0.6),
        ] {
            // The glb were extracted from the b3dm without its RTC_CENTER: given back as the tile transform
            let rtc = DMat4::from_translation(DVec3::new(
                4_056_896.270_481_2,
                202_047.449_822_138,
                4_900_910.163_547_64,
            ));
            let mesh = decode(bytes, &rtc, true).unwrap();
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

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"nope", &DMat4::IDENTITY, true).is_err());
        assert!(decode(&LILLE[..40], &DMat4::IDENTITY, true).is_err());
    }
}
