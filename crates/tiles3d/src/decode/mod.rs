//! Decoding of 3D Tiles content: Batched 3D Model (`.b3dm`, 3D Tiles 1.0) and binary glTF (`.glb`, 3D Tiles 1.1),
//! into triangle meshes ready for the GPU. Pure Rust, no I/O: images stay encoded, the browser decodes them.
//!
//! Positions go through the glTF node transforms, the y-up to z-up rotation, `RTC_CENTER` and the tile transform, to
//! ECEF; they are stored in f32 relative to an anchor (the tile's center), exact to the millimetre over tens of
//! kilometres.
//!
//! Content comes from the network: every size and offset is checked (overflow included) before it is used or
//! allocated, so a malformed tile is an error, never a panic.
//!
//! `container` opens the b3dm and glb containers, `gltf` walks the scene and builds the primitives, `accessor` reads
//! their values.

#![forbid(unsafe_code)]

mod accessor;
mod container;
mod gltf;

use container::{parse_glb, split};
use glam::{DMat4, DVec3};
use gltf::{Gltf, decode_glb, index, uv_set};
use std::rc::Rc;

/// One textured triangle mesh. A mesh used by several nodes of the scene (instances) gives a primitive per node: its
/// positions are its own (moved by the node), its texture coordinates, triangles and image shared
pub struct Primitive {
    /// x, y, z relative to the anchor (ECEF axes, metres)
    pub positions: Vec<f32>,
    /// u, v per vertex (zeros without texture coordinates)
    pub uvs: Rc<[f32]>,
    pub indices: Rc<[u32]>,
    /// Its image in `Mesh::images`
    pub image: Option<usize>,
    /// Base color factor of its material (linear RGBA, white by default), which multiplies the image
    pub color: [f32; 4],
}

pub struct Mesh {
    /// ECEF origin of the positions
    pub anchor: DVec3,
    pub primitives: Vec<Primitive>,
    /// Encoded images (JPEG, PNG, WebP, KTX2) and their media types, each once however many primitives use it
    pub images: Vec<(Rc<[u8]>, String)>,
}

impl Mesh {
    /// The encoded image of a primitive and its media type
    #[must_use]
    pub fn image(&self, primitive: &Primitive) -> Option<(&[u8], &str)> {
        let (bytes, mime) = self.images.get(primitive.image?)?;
        Some((bytes, mime))
    }
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

#[cfg(test)]
mod tests {
    use super::container::{BIN_CHUNK, JSON_CHUNK};
    use super::*;
    use geo::geodesy;
    use serde_json::{Value, json};

    /// A tile of the Lille photomesh (Métropole Européenne de Lille, 2020, 5 cm)
    const LILLE: &[u8] = include_bytes!("../../tests/data/lille_L11_00.b3dm");

    #[test]
    fn decodes_a_lille_tile() {
        let mesh = decode(LILLE, &DMat4::IDENTITY, true, &[]).unwrap();
        assert_eq!(mesh.primitives.len(), 1);
        let p = &mesh.primitives[0];
        assert_eq!(p.positions.len(), 59 * 3);
        assert_eq!(p.uvs.len(), 59 * 2);
        assert_eq!(p.indices.len(), 171);
        let (bytes, mime) = mesh.image(p).unwrap();
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
        let bytes = include_bytes!("../../tests/data/lille_draco.glb");
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
        assert_eq!(*mesh.primitives[0].indices, [0, 1, 2]);
        assert!(mesh.primitives[0].image.is_some());
    }

    /// The tile with a KTX2 texture (`KHR_texture_basisu`): the KTX2 image is the one handed over
    #[test]
    fn basisu_textures_prefer_the_ktx2_image() {
        let mesh = decode(
            include_bytes!("../../tests/data/lille_ktx2.glb"),
            &DMat4::IDENTITY,
            true,
            &[],
        )
        .unwrap();
        let (bytes, mime) = mesh.image(&mesh.primitives[0]).unwrap();
        assert_eq!(mime, "image/ktx2");
        assert_eq!(&bytes[..12], b"\xabKTX 20\xbb\r\n\x1a\n");
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"nope", &DMat4::IDENTITY, true, &[]).is_err());
        assert!(decode(&LILLE[..40], &DMat4::IDENTITY, true, &[]).is_err());
    }

    /// A glTF binary from its JSON and binary chunk
    pub(super) fn glb(json: &Value, bin: &[u8]) -> Vec<u8> {
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

    pub(super) fn f32s(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// One triangle: positions (x, y, z) then uv in a buffer, its accessors and node given by `json`
    pub(super) fn triangle(json: &Value, bin: &[u8]) -> Result<Mesh, String> {
        decode(&glb(json, bin), &DMat4::IDENTITY, false, &[])
    }

    /// Vertex `i` of the first primitive, ECEF
    pub(super) fn vertex(mesh: &Mesh, i: usize) -> DVec3 {
        let p = &mesh.primitives[0].positions[i * 3..i * 3 + 3];
        mesh.anchor + DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
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
            include_bytes!("../../tests/data/lille_noq.glb"),
            include_bytes!("../../tests/data/lille_q.glb"),
            include_bytes!("../../tests/data/lille_vpf.glb"),
            include_bytes!("../../tests/data/lille_draco.glb"),
            include_bytes!("../../tests/data/lille_ktx2.glb"),
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
