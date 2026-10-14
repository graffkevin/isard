//! Entry points for the decoding workers: the same WebAssembly module runs in a pool of Web Workers (compiled once,
//! shared), which fetch and decode the 3D Tiles content off the main thread. The results are typed arrays ready for
//! the GPU, transferred to the main thread without copy.

use crate::tiles3d::decode;
use glam::DMat4;
use js_sys::{Array, Float32Array, Float64Array, Object, Reflect, Uint8Array, Uint32Array};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
/** A primitive of a tile decoded by `decodeTile`: vertices x, y, z, u, v around the anchor; the image still encoded */
export interface EncodedPrimitive {
  vertices: Float32Array<ArrayBuffer>;
  indices: Uint32Array<ArrayBuffer>;
  image?: Uint8Array<ArrayBuffer>;
  mime?: string;
}

/** A tile decoded by `decodeTile` */
export interface EncodedTile {
  /** ECEF origin of the vertices (metres) */
  anchor: Float64Array<ArrayBuffer>;
  primitives: EncodedPrimitive[];
}

/** A primitive ready for the GPU: its image decoded */
export interface DecodedPrimitive {
  vertices: Float32Array<ArrayBuffer>;
  indices: Uint32Array<ArrayBuffer>;
  bitmap: ImageBitmap | null;
}

/** A tile ready for the GPU, as the decoding workers deliver it */
export interface DecodedTile {
  anchor: Float64Array<ArrayBuffer>;
  primitives: DecodedPrimitive[];
}

/** What the engine expects from a pool of decoding workers (`Engine.setTileLoader`) */
export interface TileLoaderLike {
  load(id: number, url: string, transform: Float64Array, yUp: boolean): Promise<DecodedTile>;
  abort(id: number): void;
}
"#;

/// Decodes a `.b3dm` / `.glb` tile: `{ anchor: Float64Array(3), primitives: [{ vertices: Float32Array (x, y, z, u,
/// v per vertex, relative to the anchor), indices: Uint32Array, image?: Uint8Array, mime?: string }] }`.
/// `transform`: the tile's world transform (16 numbers, column-major).
#[wasm_bindgen(js_name = decodeTile, unchecked_return_type = "EncodedTile")]
pub fn decode_tile(bytes: &[u8], transform: &[f64], y_up: bool) -> Result<Object, JsValue> {
    let transform = DMat4::from_cols_slice(transform.get(..16).ok_or("transform: 16 numbers expected")?);
    let mesh = decode::decode(bytes, &transform, y_up)?;
    let out = Object::new();
    let set = |target: &Object, key: &str, value: &JsValue| Reflect::set(target, &JsValue::from_str(key), value);
    set(&out, "anchor", &Float64Array::from(&mesh.anchor.to_array()[..]))?;
    let primitives = Array::new();
    for p in &mesh.primitives {
        let vertices: Vec<f32> = p
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .zip(p.uvs.as_chunks::<2>().0)
            .flat_map(|(&[x, y, z], &[u, v])| [x, y, z, u, v])
            .collect();
        let primitive = Object::new();
        set(&primitive, "vertices", &Float32Array::from(&vertices[..]))?;
        set(&primitive, "indices", &Uint32Array::from(&p.indices[..]))?;
        if let Some((image, mime)) = &p.image {
            set(&primitive, "image", &Uint8Array::from(&image[..]))?;
            set(&primitive, "mime", &JsValue::from_str(mime))?;
        }
        primitives.push(&primitive);
    }
    set(&out, "primitives", &primitives)?;
    Ok(out)
}
