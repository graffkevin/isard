//! Point Cloud tiles (`.pnts`, 3D Tiles 1.0): the feature table's positions (`POSITION`, or `POSITION_QUANTIZED` in
//! its quantized volume), colors (`RGBA`, `RGB`, `RGB565`, `CONSTANT_RGBA`) and normals (`NORMAL`, oct-encoded
//! `NORMAL_OCT16P`), around `RTC_CENTER`; or the same compressed by Draco (`3DTILES_draco_point_compression`), decoded
//! by the caller. The batch table (per-point properties, `BATCH_ID`) is not read.

use super::container::{slice, u32_at};
use super::gltf::index;
use super::{DracoGeometry, DracoRequest, Points};
use glam::{DMat4, DVec3};
use serde_json::Value;

/// Points a tile may hold: past it, refused before anything is allocated
pub const MAX_POINTS: usize = 1 << 24;

/// The feature table of a `.pnts`: its JSON and binary body
struct FeatureTable<'a> {
    json: Value,
    binary: &'a [u8],
    count: usize,
}

impl<'a> FeatureTable<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.get(..4) != Some(b"pnts") {
            return Err("not a pnts tile".into());
        }
        let json_length = u32_at(bytes, 12)?;
        let binary_length = u32_at(bytes, 16)?;
        let text = slice(bytes, 28, json_length).ok_or("truncated pnts feature table")?;
        let end = text
            .iter()
            .rposition(|&b| b != 0 && !b.is_ascii_whitespace())
            .map_or(0, |i| i + 1);
        let json: Value = serde_json::from_slice(&text[..end]).map_err(|e| format!("pnts feature table: {e}"))?;
        let binary = slice(bytes, 28 + json_length, binary_length).ok_or("truncated pnts feature table")?;
        let count = index(&json["POINTS_LENGTH"]).ok_or("pnts without POINTS_LENGTH")?;
        if count > MAX_POINTS {
            return Err(format!("pnts of {count} points: more than {MAX_POINTS}"));
        }
        Ok(Self { json, binary, count })
    }

    /// The bytes of a per-point semantic: `count` elements of `element` bytes from its byte offset, None when absent
    fn semantic(&self, name: &str, element: usize) -> Result<Option<&'a [u8]>, String> {
        let value = &self.json[name];
        if value.is_null() {
            return Ok(None);
        }
        let offset = index(&value["byteOffset"]).ok_or_else(|| format!("pnts {name} without byteOffset"))?;
        let length = self.count.checked_mul(element).ok_or("pnts too large")?;
        slice(self.binary, offset, length)
            .map(Some)
            .ok_or_else(|| format!("pnts {name} out of the feature table"))
    }

    /// A global value of N numbers: a JSON array, or N floats in the binary body (`{"byteOffset"}`); None when absent
    fn global<const N: usize>(&self, name: &str) -> Result<Option<[f64; N]>, String> {
        let value = &self.json[name];
        let bad = || format!("bad pnts {name}");
        match value {
            Value::Null => Ok(None),
            Value::Array(a) if a.len() == N => {
                let mut out = [0.0; N];
                for (o, v) in out.iter_mut().zip(a) {
                    *o = v.as_f64().filter(|v| v.is_finite()).ok_or_else(bad)?;
                }
                Ok(Some(out))
            }
            Value::Object(_) => {
                let offset = index(&value["byteOffset"]).ok_or_else(bad)?;
                let b = slice(self.binary, offset, N * 4).ok_or_else(bad)?;
                let mut out = [0.0; N];
                for (i, o) in out.iter_mut().enumerate() {
                    let v = f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
                    *o = f64::from(v);
                }
                if out.iter().all(|v| v.is_finite()) {
                    Ok(Some(out))
                } else {
                    Err(bad())
                }
            }
            _ => Err(bad()),
        }
    }
}

fn f32_at(b: &[u8], i: usize) -> f32 {
    f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// A unit normal from its two oct-encoded bytes (`NORMAL_OCT16P`)
#[must_use]
pub fn oct_decode(x: u8, y: u8) -> [f32; 3] {
    let (mut u, mut v) = (f32::from(x) / 255.0 * 2.0 - 1.0, f32::from(y) / 255.0 * 2.0 - 1.0);
    let w = 1.0 - u.abs() - v.abs();
    if w < 0.0 {
        (u, v) = ((1.0 - v.abs()) * u.signum(), (1.0 - u.abs()) * v.signum());
    }
    let n = glam::Vec3::new(u, v, w).normalize_or_zero();
    n.to_array()
}

/// The Draco-compressed points of a `.pnts` (`3DTILES_draco_point_compression`), for the external Draco decoder
pub fn draco_request(bytes: &[u8]) -> Result<Option<DracoRequest>, String> {
    let table = FeatureTable::parse(bytes)?;
    let ext = &table.json["extensions"]["3DTILES_draco_point_compression"];
    if !ext.is_object() {
        return Ok(None);
    }
    let offset = index(&ext["byteOffset"]).ok_or("pnts Draco without byteOffset")?;
    let length = index(&ext["byteLength"]).ok_or("pnts Draco without byteLength")?;
    let data = slice(table.binary, offset, length).ok_or("pnts Draco out of the feature table")?;
    let property = |name: &str| ext["properties"][name].as_u64().and_then(|v| u32::try_from(v).ok());
    Ok(Some(DracoRequest {
        mesh: 0,
        primitive: 0,
        data: data.to_vec(),
        position: property("POSITION").ok_or("pnts Draco without POSITION")?,
        uv: None,
        color: property("RGBA").or(property("RGB")),
        normal: property("NORMAL"),
        points: true,
    }))
}

/// The points of a `.pnts` in world coordinates (`transform`), with their colors (white without) and normals (none
/// without); Draco points from `draco`
pub fn decode(bytes: &[u8], transform: &DMat4, draco: &[DracoGeometry]) -> Result<(Vec<DVec3>, Points), String> {
    let table = FeatureTable::parse(bytes)?;
    let count = table.count;
    let rtc = table.global::<3>("RTC_CENTER")?.map_or(DVec3::ZERO, DVec3::from);
    let to_world = *transform * DMat4::from_translation(rtc);
    let compressed = table.json["extensions"]["3DTILES_draco_point_compression"].is_object();
    let geometry = if compressed {
        let g = draco
            .iter()
            .find(|g| g.mesh == 0 && g.primitive == 0)
            .ok_or("Draco points not decoded (Draco needs the decoding workers)")?;
        if g.positions.len() != count * 3 {
            return Err("Draco points of a wrong count".into());
        }
        Some(g)
    } else {
        None
    };
    // Positions in the tile's frame
    let local: Vec<DVec3> = if let Some(g) = geometry {
        g.positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(f64::from).into())
            .collect()
    } else if let Some(b) = table.semantic("POSITION", 12)? {
        b.as_chunks::<12>()
            .0
            .iter()
            .map(|e| {
                DVec3::new(
                    f64::from(f32_at(e, 0)),
                    f64::from(f32_at(e, 4)),
                    f64::from(f32_at(e, 8)),
                )
            })
            .collect()
    } else if let Some(b) = table.semantic("POSITION_QUANTIZED", 6)? {
        let offset = table
            .global::<3>("QUANTIZED_VOLUME_OFFSET")?
            .ok_or("pnts without QUANTIZED_VOLUME_OFFSET")?;
        let scale = table
            .global::<3>("QUANTIZED_VOLUME_SCALE")?
            .ok_or("pnts without QUANTIZED_VOLUME_SCALE")?;
        let (offset, scale) = (DVec3::from(offset), DVec3::from(scale));
        b.as_chunks::<6>()
            .0
            .iter()
            .map(|e| {
                let q = [0, 2, 4].map(|i| f64::from(u16::from_le_bytes([e[i], e[i + 1]])) / 65535.0);
                offset + DVec3::from(q) * scale
            })
            .collect()
    } else {
        return Err("pnts without positions".into());
    };
    let world: Vec<DVec3> = local.iter().map(|&p| to_world.transform_point3(p)).collect();
    let colors = colors(&table, geometry)?;
    let normals = normals(&table, geometry, transform)?;
    Ok((
        world,
        Points {
            positions: Vec::new(),
            colors,
            normals,
        },
    ))
}

/// The colors of the points: per point, else the constant one, else white
fn colors(table: &FeatureTable, geometry: Option<&DracoGeometry>) -> Result<Vec<[u8; 4]>, String> {
    let count = table.count;
    if let Some(g) = geometry.filter(|g| !g.colors.is_empty()) {
        return Ok(float_colors(&g.colors, count));
    }
    if let Some(b) = table.semantic("RGBA", 4)? {
        return Ok(b.as_chunks::<4>().0.to_vec());
    }
    if let Some(b) = table.semantic("RGB", 3)? {
        return Ok(b.as_chunks::<3>().0.iter().map(|&[r, g, b]| [r, g, b, 255]).collect());
    }
    if let Some(b) = table.semantic("RGB565", 2)? {
        return Ok(b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&e| {
                let v = u16::from_le_bytes(e);
                let scale = |value: u16, max: u16| (u32::from(value) * 255 / u32::from(max)) as u8;
                [scale(v >> 11, 31), scale((v >> 5) & 63, 63), scale(v & 31, 31), 255]
            })
            .collect());
    }
    let constant = &table.json["CONSTANT_RGBA"];
    if let Some(c) = constant.as_array().filter(|c| c.len() == 4) {
        let c: [u8; 4] = std::array::from_fn(|i| c[i].as_u64().unwrap_or(255).min(255) as u8);
        return Ok(vec![c; count]);
    }
    Ok(vec![[255; 4]; count])
}

/// Colors as floats (a Draco decoder's, a glTF accessor's): three or four per point, 0 to 1 (or 0 to 255 when not
/// normalized)
pub(super) fn float_colors(colors: &[f32], count: usize) -> Vec<[u8; 4]> {
    let components = colors.len().checked_div(count).unwrap_or(3);
    let scale = if colors.iter().any(|&c| c > 1.0) { 1.0 } else { 255.0 };
    let byte = |c: f32| (c * scale).round().clamp(0.0, 255.0) as u8;
    if !(3..=4).contains(&components) {
        return vec![[255; 4]; count];
    }
    colors
        .chunks_exact(components)
        .map(|c| [byte(c[0]), byte(c[1]), byte(c[2]), c.get(3).map_or(255, |&a| byte(a))])
        .collect()
}

/// The unit normals of the points in world axes (the transform's rotation), empty without
fn normals(table: &FeatureTable, geometry: Option<&DracoGeometry>, transform: &DMat4) -> Result<Vec<[f32; 3]>, String> {
    let local: Vec<[f32; 3]> = if let Some(g) = geometry.filter(|g| !g.normals.is_empty()) {
        if g.normals.len() != table.count * 3 {
            return Err("Draco normals of a wrong count".into());
        }
        g.normals.as_chunks::<3>().0.to_vec()
    } else if let Some(b) = table.semantic("NORMAL", 12)? {
        b.as_chunks::<12>()
            .0
            .iter()
            .map(|e| [f32_at(e, 0), f32_at(e, 4), f32_at(e, 8)])
            .collect()
    } else if let Some(b) = table.semantic("NORMAL_OCT16P", 2)? {
        b.as_chunks::<2>().0.iter().map(|&[x, y]| oct_decode(x, y)).collect()
    } else {
        return Ok(Vec::new());
    };
    Ok(local
        .iter()
        .map(|n| {
            transform
                .transform_vector3(DVec3::from(n.map(f64::from)))
                .normalize_or_zero()
                .as_vec3()
                .to_array()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::{Mesh, decode as decode_tile, draco_requests};
    use super::*;
    use geo::geodesy;

    const RGB: &[u8] = include_bytes!("../../tests/data/pointCloudRGB.pnts");

    fn points(bytes: &[u8]) -> Mesh {
        decode_tile(bytes, &DMat4::IDENTITY, true, &[]).unwrap()
    }

    /// The point `i` of a decoded tile, ECEF
    fn at(mesh: &Mesh, i: usize) -> DVec3 {
        let p = &mesh.points[0].positions[i * 3..i * 3 + 3];
        mesh.anchor + DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
    }

    /// Cesium's point cloud samples (Apache-2.0): 1000 points in a 10 m sphere off Cape Canaveral
    #[test]
    fn decodes_positions_and_colors() {
        let mesh = points(RGB);
        assert!(mesh.primitives.is_empty());
        let p = &mesh.points[0];
        assert_eq!((p.positions.len(), p.colors.len(), p.normals.len()), (3000, 1000, 0));
        let center = DVec3::new(1_215_012.882_887_673_8, -4_736_313.051_199_594, 4_081_605.221_260_42);
        for i in 0..1000 {
            assert!((at(&mesh, i) - center).length() < 5.0 + 1e-3);
        }
        assert!(p.colors.iter().all(|c| c[3] == 255));
        assert!(p.colors.iter().any(|c| c[0] != c[1]));
        // y up does not apply to points
        let g = geodesy::from_ecef(mesh.anchor);
        assert!((g.x + 75.6).abs() < 0.5 && (g.y - 40.0).abs() < 1.0, "{g}");
    }

    #[test]
    fn quantized_positions_and_oct_encoded_normals() {
        let plain = points(include_bytes!("../../tests/data/pointCloudNormals.pnts"));
        let quantized = points(include_bytes!("../../tests/data/pointCloudQuantizedOctEncoded.pnts"));
        let oct = points(include_bytes!("../../tests/data/pointCloudNormalsOctEncoded.pnts"));
        // Same sphere: the quantized positions within the quantization step (10 m / 65535)
        for i in [0, 1, 500, 999] {
            assert!((at(&plain, i) - at(&quantized, i)).length() < 1e-3);
        }
        // Normals of unit length pointing out of the sphere, the oct-encoded ones close to the plain
        let center = DVec3::new(1_215_012.882_887_673_8, -4_736_313.051_199_594, 4_081_605.221_260_42);
        for i in [0, 10, 999] {
            let n = DVec3::from(plain.points[0].normals[i].map(f64::from));
            assert!((n.length() - 1.0).abs() < 1e-5);
            assert!(n.dot((at(&plain, i) - center).normalize()) > 0.99);
            let m = DVec3::from(oct.points[0].normals[i].map(f64::from));
            assert!(n.dot(m) > 0.999, "{n} {m}");
        }
    }

    #[test]
    fn rgba_rgb565_constant_and_no_color() {
        let rgb = points(RGB);
        let rgba = points(include_bytes!("../../tests/data/pointCloudRGBA.pnts"));
        assert!(rgba.points[0].colors.iter().any(|c| c[3] < 255));
        let c565 = points(include_bytes!("../../tests/data/pointCloudRGB565.pnts"));
        // RGB565 rounds the RGB colors to 5 and 6 bits (a step of 8 or 4)
        for (a, b) in rgb.points[0].colors.iter().zip(&c565.points[0].colors) {
            assert!(a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 9), "{a:?} {b:?}");
        }
        let constant = points(include_bytes!("../../tests/data/pointCloudConstantColor.pnts"));
        assert!(constant.points[0].colors.iter().all(|c| *c == [255, 255, 0, 51]));
        let none = points(include_bytes!("../../tests/data/pointCloudNoColor.pnts"));
        assert!(none.points[0].colors.iter().all(|c| *c == [255; 4]));
    }

    #[test]
    fn oct_decoding_covers_both_hemispheres() {
        assert_eq!(oct_decode(128, 128).map(|v| (v * 100.0).round()), [0.0, 0.0, 100.0]);
        let n = oct_decode(0, 0);
        assert!((glam::Vec3::from(n).length() - 1.0).abs() < 1e-5 && n[2] < 0.0);
        assert_eq!(oct_decode(255, 128).map(|v| (v * 10.0).round()), [10.0, 0.0, 0.0]);
    }

    #[test]
    fn draco_points_are_requested_then_used() {
        let bytes = include_bytes!("../../tests/data/pointCloudDraco.pnts");
        let requests = draco_requests(bytes).unwrap();
        assert_eq!(requests.len(), 1);
        let r = &requests[0];
        assert!(r.points);
        assert_eq!((r.position, r.color, r.normal, r.uv), (0, Some(1), Some(2), None));
        assert_eq!(r.data.len(), 15_300);
        assert!(decode_tile(bytes, &DMat4::IDENTITY, true, &[]).is_err());
        // Stand-in output of the Draco module: positions, colors as bytes in floats, normals
        let geometry = DracoGeometry {
            mesh: 0,
            primitive: 0,
            positions: (0..3000).map(|i| (i % 7) as f32).collect(),
            uvs: Vec::new(),
            indices: Vec::new(),
            colors: [255.0, 0.0, 128.0].repeat(1000),
            normals: [0.0, 0.0, 1.0].repeat(1000),
        };
        let mesh = decode_tile(bytes, &DMat4::IDENTITY, true, &[geometry]).unwrap();
        let p = &mesh.points[0];
        assert_eq!(p.colors[5], [255, 0, 128, 255]);
        assert_eq!(p.normals.len(), 1000);
    }

    #[test]
    fn malformed_tables_are_errors() {
        let mut bad = RGB.to_vec();
        // POINTS_LENGTH past the binary body
        let text = String::from_utf8_lossy(&bad[28..28 + 148]).replace("1000", "9000");
        bad[28..28 + 148].copy_from_slice(text.as_bytes());
        assert!(decode_tile(&bad, &DMat4::IDENTITY, true, &[]).is_err());
        assert!(decode_tile(&RGB[..100], &DMat4::IDENTITY, true, &[]).is_err());
        let mut bad = RGB.to_vec();
        bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_tile(&bad, &DMat4::IDENTITY, true, &[]).is_err());
    }

    #[test]
    fn mutated_tiles_never_panic() {
        let fixtures: [&[u8]; 4] = [
            RGB,
            include_bytes!("../../tests/data/pointCloudQuantizedOctEncoded.pnts"),
            include_bytes!("../../tests/data/pointCloudDraco.pnts"),
            include_bytes!("../../tests/data/pointCloudRGB565.pnts"),
        ];
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n.max(1) as u64) as usize
        };
        for fixture in fixtures {
            for _ in 0..500 {
                let mut bytes = fixture.to_vec();
                for _ in 0..=next(6) {
                    let i = next(bytes.len().min(400));
                    bytes[i] ^= 1 << next(8);
                }
                if next(4) == 0 {
                    bytes.truncate(next(fixture.len()));
                }
                let _ = decode_tile(&bytes, &DMat4::IDENTITY, true, &[]);
                let _ = draco_requests(&bytes);
            }
        }
    }
}
