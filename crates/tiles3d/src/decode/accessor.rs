//! The accessors of a glTF: their layout, their components read as f32 or exact indices, from interleaved buffer
//! views or zeros, then their sparse substitutions.

use super::container::slice;
use super::gltf::{Gltf, index, offset};
use serde_json::Value;
use std::rc::Rc;

/// Most values of an accessor without buffer view (zeros, possibly sparse)
const MAX_ZEROS: usize = 1 << 22;

/// Layout of an accessor
pub(super) struct Layout {
    count: usize,
    components: usize,
    /// The glTF component type (5120 i8 to 5126 f32)
    pub(super) component_type: u64,
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

/// Bytes of a glTF component type, an error for an unknown one
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
pub(super) fn index_reader(component_type: u64) -> Result<Read<u32>, String> {
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

impl Gltf<'_> {
    /// Accessor values as f32 (normalized integers mapped to [0, 1] or [-1, 1], other integers kept as values), read
    /// once per tile however many primitives and nodes use them
    pub(super) fn floats(&self, index: usize) -> Result<Rc<[f32]>, String> {
        if let Some(values) = self.floats.borrow().get(&index) {
            return Ok(values.clone());
        }

        let values: Rc<[f32]> = self
            .accessor(index, |l| float_reader(l.component_type, l.normalized))?
            .into();

        self.floats.borrow_mut().insert(index, values.clone());

        Ok(values)
    }

    /// Accessor values as triangle indices, read once per tile as `floats`
    pub(super) fn indices(&self, index: usize) -> Result<Rc<[u32]>, String> {
        if let Some(values) = self.indices.borrow().get(&index) {
            return Ok(values.clone());
        }

        let values: Rc<[u32]> = self.accessor(index, |l| index_reader(l.component_type))?.into();

        self.indices.borrow_mut().insert(index, values.clone());

        Ok(values)
    }

    /// Values of an accessor, read by `reader`: from its buffer view (interleaved or not), or zeros without one, then
    /// its sparse substitutions
    pub(super) fn accessor<T: Copy + Default>(
        &self,
        index: usize,
        reader: impl Fn(&Layout) -> Result<Read<T>, String>,
    ) -> Result<Vec<T>, String> {
        let accessor = &self.json["accessors"][index];
        let layout = Layout::parse(accessor)?;
        let read = reader(&layout)?;
        let (count, components, size, element) = (layout.count, layout.components, layout.size, layout.element());
        let total = count.checked_mul(components).ok_or("accessor too large")?;

        // Spent by the primitives that use them (`Gltf::spend`); not more than the budget left, before the allocation
        self.within(total)?;

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
}

#[cfg(test)]
mod tests {
    use super::super::tests::{f32s, triangle, vertex};
    use glam::DVec3;
    use serde_json::json;

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
        assert_eq!(*p.uvs, [0.0, 0.0, 0.0, 0.0, 0.5, 0.25]);

        // A sparse index past the accessor, too many sparse values: errors
        let mut bad = json.clone();

        bad["accessors"][1]["sparse"]["indices"]["componentType"] = json!(5125);
        assert!(triangle(&bad, &bin).is_err());

        let mut bad = json;

        bad["accessors"][0]["sparse"]["count"] = json!(4);
        assert!(triangle(&bad, &bin).is_err());
    }
}
