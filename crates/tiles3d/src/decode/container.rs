//! The containers of a tile: the Batched 3D Model header and feature table (`RTC_CENTER`), the chunks of a glTF
//! binary. Every length is checked against the bytes before it is read.

use super::gltf::index;
use glam::DVec3;
use serde_json::Value;

/// A b3dm's batch table (its JSON) and its length
pub(super) type BatchTable = (Value, usize);

/// The glTF of a tile, its RTC center, and for a b3dm its batch table
pub(super) fn split(bytes: &[u8]) -> Result<(&[u8], DVec3, Option<BatchTable>), String> {
    match bytes.get(..4) {
        Some(b"b3dm") => b3dm_glb(bytes),
        Some(b"glTF") => Ok((bytes, DVec3::ZERO, None)),
        _ if bytes.trim_ascii_start().starts_with(b"{") => {
            Err("a JSON document (external tileset?), not a mesh".into())
        }
        _ => Err("unsupported tile content (b3dm, pnts and glb only)".into()),
    }
}

/// The little-endian u32 at `offset`, as a usize; an error past the end
pub(super) fn u32_at(bytes: &[u8], offset: usize) -> Result<usize, String> {
    let b = slice(bytes, offset, 4).ok_or("truncated tile")?;

    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// `length` bytes from `start`, None if out of `bytes` (overflow included)
pub(super) fn slice(bytes: &[u8], start: usize, length: usize) -> Option<&[u8]> {
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

/// The glTF binary inside a b3dm, after its 28-byte header, feature table and batch table; with the feature table's
/// `RTC_CENTER` (JSON or binary, zero without) and the batch table's JSON
fn b3dm_glb(bytes: &[u8]) -> Result<(&[u8], DVec3, Option<BatchTable>), String> {
    let feature_json = u32_at(bytes, 12)?;
    let feature_binary = u32_at(bytes, 16)?;
    let batch_json = u32_at(bytes, 20)?;
    let batch_binary = u32_at(bytes, 24)?;
    let mut rtc = DVec3::ZERO;
    let mut batch_length = 0;

    if feature_json > 0 {
        let text = slice(bytes, 28, feature_json).ok_or("truncated feature table")?;
        let table: Value = serde_json::from_slice(trim_padding(text)).map_err(|e| e.to_string())?;

        batch_length = index(&table["BATCH_LENGTH"]).unwrap_or(0);

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
    // The batch table's JSON, if it reads (a broken one only loses the properties)
    let batch = (batch_json > 0)
        .then(|| slice(bytes, 28 + feature_json + feature_binary, batch_json))
        .flatten()
        .and_then(|text| serde_json::from_slice::<Value>(trim_padding(text)).ok())
        .map(|table| (table, batch_length));

    Ok((bytes.get(glb..).ok_or("truncated b3dm")?, rtc, batch))
}

/// Chunk type of the JSON chunk of a glTF binary (`JSON` in ASCII)
pub(super) const JSON_CHUNK: usize = 0x4E4F_534A;
/// Chunk type of the binary chunk of a glTF binary (`BIN\0` in ASCII)
pub(super) const BIN_CHUNK: usize = 0x004E_4942;

/// JSON and binary chunk of a glTF binary
pub(super) fn parse_glb(bytes: &[u8]) -> Result<(Value, &[u8]), String> {
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

#[cfg(test)]
mod tests {
    use super::super::decode;
    use super::super::tests::{f32s, glb};
    use super::*;
    use glam::DMat4;
    use serde_json::json;

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
}
