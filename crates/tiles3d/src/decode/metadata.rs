//! The properties of a tile's features, one row per feature: a property table of `EXT_structural_metadata` (3D Tiles
//! 1.1: scalars, booleans, strings and enums, read from their buffer views), or the JSON of a b3dm batch table (3D
//! Tiles 1.0, its properties given as arrays). Vectors, matrices, arrays and the batch table's binary body are left
//! out (null): what a feature's attributes are shown with. Plain values, not JSON: the engine hands them to
//! JavaScript as objects.

use super::gltf::{Gltf, index};
use serde_json::Value;

/// Features of a table at most: past it, the properties are left out
const MAX_FEATURES: usize = 1 << 20;

/// A property's value for a feature
#[derive(Clone, Debug, PartialEq)]
pub enum Property {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
}

/// The properties of a tile's features: their names, and each feature's values in that order
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Properties {
    pub names: Vec<String>,
    pub features: Vec<Vec<Property>>,
}

/// The properties of property table `table` of the glTF's `EXT_structural_metadata`, None if there is none
pub(super) fn property_table(gltf: &Gltf, table: usize) -> Result<Option<Properties>, String> {
    let metadata = &gltf.json["extensions"]["EXT_structural_metadata"];
    let table = &metadata["propertyTables"][table];
    let schema = &metadata["schema"];
    let Some(class) = table["class"].as_str().map(|c| &schema["classes"][c]) else {
        return Ok(None);
    };
    let Some(count) = index(&table["count"]).filter(|&c| c > 0 && c <= MAX_FEATURES) else {
        return Ok(None);
    };
    gltf.spend(count)?;
    let mut out = Properties {
        names: Vec::new(),
        features: vec![Vec::new(); count],
    };
    for (name, property) in table["properties"].as_object().into_iter().flatten() {
        let definition = &class["properties"][name];
        let values = column(gltf, schema, definition, property, count)?;
        out.names.push(name.clone());
        for (feature, value) in out.features.iter_mut().zip(values) {
            feature.push(value);
        }
    }
    Ok(Some(out))
}

/// The values of one property for the `count` features, null where it has none (its no-data value)
fn column(
    gltf: &Gltf,
    schema: &Value,
    definition: &Value,
    property: &Value,
    count: usize,
) -> Result<Vec<Property>, String> {
    let nulls = || vec![Property::Null; count];
    if definition["array"].as_bool() == Some(true) {
        return Ok(nulls());
    }
    let Some(view) = index(&property["values"]) else {
        return Ok(nulls());
    };
    let bytes = gltf.view(view)?;
    let no_data = &definition["noData"];
    let values = match definition["type"].as_str() {
        Some("SCALAR") => {
            let component = definition["componentType"].as_str().unwrap_or("FLOAT32");
            numbers(&bytes, component, count)
                .into_iter()
                .map(|v| v.filter(|&v| !same(v, no_data)))
                // Shown at the precision stored, once told from the no-data value
                .map(|v| v.map(|v| if component == "FLOAT32" { round_f32(v as f32) } else { v }))
                .map(|v| v.map_or(Property::Null, Property::Number))
                .collect()
        }
        Some("BOOLEAN") => (0..count)
            .map(|i| {
                bytes
                    .get(i / 8)
                    .map_or(Property::Null, |b| Property::Bool(b >> (i % 8) & 1 == 1))
            })
            .collect(),
        Some("STRING") => {
            let Some(offsets) = index(&property["stringOffsets"]) else {
                return Ok(nulls());
            };
            let offsets = gltf.view(offsets)?;
            let kind = property["stringOffsetType"].as_str().unwrap_or("UINT32");
            let offsets = numbers(&offsets, kind, count + 1);
            let at = |k: usize| offsets.get(k).copied().flatten().map(|n| n as usize);
            (0..count)
                .map(|i| match (at(i), at(i + 1)) {
                    (Some(a), Some(b)) if a <= b => bytes.get(a..b).map_or(Property::Null, |s| {
                        let text = String::from_utf8_lossy(s);
                        if no_data.as_str() == Some(&*text) {
                            Property::Null
                        } else {
                            Property::Text(text.into_owned())
                        }
                    }),
                    _ => Property::Null,
                })
                .collect()
        }
        Some("ENUM") => {
            let Some(name) = definition["enumType"].as_str() else {
                return Ok(nulls());
            };
            let kind = schema["enums"][name]["valueType"].as_str().unwrap_or("UINT16");
            let names = schema["enums"][name]["values"]
                .as_array()
                .map_or(&[][..], Vec::as_slice);
            numbers(&bytes, kind, count)
                .into_iter()
                .map(|v| {
                    names
                        .iter()
                        .find(|e| v.is_some() && e["value"].as_f64() == v)
                        .and_then(|e| e["name"].as_str())
                        .map_or(Property::Null, |n| Property::Text(n.into()))
                })
                .collect()
        }
        _ => nulls(),
    };
    Ok(values)
}

/// Whether a value is the no-data value of its property
fn same(v: f64, no_data: &Value) -> bool {
    // Equal, or the same once stored as f32 (the no-data value of a FLOAT32 property is written in f64)
    no_data
        .as_f64()
        .is_some_and(|n| v.to_bits() == n.to_bits() || (v as f32).to_bits() == (n as f32).to_bits())
}

/// An f32 to 7 significant digits (394.8, not 394.79998779296875)
fn round_f32(value: f32) -> f64 {
    let v = f64::from(value);
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let scale = 10f64.powi(6 - v.abs().log10().floor() as i32);
    (v * scale).round() / scale
}

/// `count` numbers of a component type read from little-endian bytes (None past the end, or not finite)
fn numbers(bytes: &[u8], component: &str, count: usize) -> Vec<Option<f64>> {
    let size = match component {
        "INT8" | "UINT8" => 1,
        "INT16" | "UINT16" => 2,
        "INT32" | "UINT32" | "FLOAT32" => 4,
        "INT64" | "UINT64" | "FLOAT64" => 8,
        _ => return vec![None; count],
    };
    (0..count)
        .map(|i| {
            let b = bytes.get(i * size..(i + 1) * size)?;
            let mut a = [0u8; 8];
            a[..size].copy_from_slice(b);
            let v = match component {
                "INT8" => f64::from(i8::from_le_bytes([a[0]])),
                "UINT8" => f64::from(a[0]),
                "INT16" => f64::from(i16::from_le_bytes([a[0], a[1]])),
                "UINT16" => f64::from(u16::from_le_bytes([a[0], a[1]])),
                "INT32" => f64::from(i32::from_le_bytes([a[0], a[1], a[2], a[3]])),
                "UINT32" => f64::from(u32::from_le_bytes([a[0], a[1], a[2], a[3]])),
                "INT64" => i64::from_le_bytes(a) as f64,
                "UINT64" => u64::from_le_bytes(a) as f64,
                "FLOAT32" => f64::from(f32::from_le_bytes([a[0], a[1], a[2], a[3]])),
                _ => f64::from_le_bytes(a),
            };
            v.is_finite().then_some(v)
        })
        .collect()
}

/// The properties of a b3dm batch table given as JSON arrays, one row per feature (`BATCH_LENGTH` of them), None
/// without any
pub(super) fn batch_table(table: &Value, count: usize) -> Option<Properties> {
    let properties = table.as_object()?;
    if count == 0 || count > MAX_FEATURES {
        return None;
    }
    let mut out = Properties {
        names: Vec::new(),
        features: vec![Vec::new(); count],
    };
    for (name, values) in properties {
        // Binary properties (a byteOffset) and the hierarchy extension are left out
        let Some(values) = values.as_array() else {
            continue;
        };
        out.names.push(name.clone());
        for (k, feature) in out.features.iter_mut().enumerate() {
            feature.push(match values.get(k) {
                Some(Value::Bool(b)) => Property::Bool(*b),
                Some(Value::Number(n)) => n.as_f64().map_or(Property::Null, Property::Number),
                Some(Value::String(s)) => Property::Text(s.clone()),
                _ => Property::Null,
            });
        }
    }
    (!out.names.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::super::decode;
    use super::super::tests::{f32s, glb};
    use super::Property::{Bool, Null, Number, Text};
    use glam::DMat4;
    use serde_json::json;

    #[test]
    fn a_property_table_gives_each_feature_its_properties() {
        // A triangle of 3 vertices, features 0, 1, 1; two buildings: heights (the second has none), names, a flag
        let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]); // positions, 36 bytes
        bin.extend(f32s(&[0.0, 1.0, 1.0])); // feature IDs at 36, 12 bytes
        bin.extend(f32s(&[12.5, f32::MAX])); // heights at 48, 8 bytes
        bin.extend(b"abcdefg\0"); // names at 56: "abc", "defg"
        bin.extend([0u32, 3, 7].iter().flat_map(|v| v.to_le_bytes())); // their offsets at 64, 12 bytes
        bin.extend([0b10u8, 0, 0, 0]); // flags at 76: false, true
        let json = json!({
            "asset": {"version": "2.0"},
            "extensionsUsed": ["EXT_mesh_features", "EXT_structural_metadata"],
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 12},
                {"buffer": 0, "byteOffset": 48, "byteLength": 8},
                {"buffer": 0, "byteOffset": 56, "byteLength": 8},
                {"buffer": 0, "byteOffset": 64, "byteLength": 12},
                {"buffer": 0, "byteOffset": 76, "byteLength": 1}
            ],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "SCALAR"}
            ],
            "meshes": [{"primitives": [{
                "attributes": {"POSITION": 0, "_FEATURE_ID_0": 1},
                "extensions": {"EXT_mesh_features": {"featureIds": [{"attribute": 0, "featureCount": 2, "propertyTable": 0}]}}
            }]}],
            "nodes": [{"mesh": 0}],
            "extensions": {"EXT_structural_metadata": {
                "schema": {"classes": {"building": {"properties": {
                    "hauteur": {"type": "SCALAR", "componentType": "FLOAT32", "noData": 3.402_823_466_385_288_6e38},
                    "nom": {"type": "STRING"},
                    "leger": {"type": "BOOLEAN"}
                }}}},
                "propertyTables": [{"class": "building", "count": 2, "properties": {
                    "hauteur": {"values": 2},
                    "nom": {"values": 3, "stringOffsets": 4},
                    "leger": {"values": 5}
                }}]
            }}
        });
        let mesh = decode(&glb(&json, &bin), &DMat4::IDENTITY, false, &[]).unwrap();
        assert_eq!(mesh.primitives[0].features.as_deref(), Some(&[0.0, 1.0, 1.0][..]));
        let properties = mesh.properties.unwrap();
        assert_eq!(properties.names, ["hauteur", "leger", "nom"]);
        assert_eq!(
            properties.features,
            [
                vec![Number(12.5), Bool(false), Text("abc".into())],
                vec![Null, Bool(true), Text("defg".into())]
            ]
        );
    }

    #[test]
    fn a_batch_table_gives_its_arrays() {
        let table = json!({"id": ["a", "b"], "h": [3, 4], "binary": {"byteOffset": 0, "componentType": "FLOAT"}});
        let properties = super::batch_table(&table, 2).unwrap();
        assert_eq!(properties.names, ["h", "id"]);
        assert_eq!(
            properties.features,
            [vec![Number(3.0), Text("a".into())], vec![Number(4.0), Text("b".into())]]
        );
        assert_eq!(super::batch_table(&json!({}), 2), None);
        assert!((super::round_f32(394.8) - 394.8).abs() < 1e-12);
    }
}
