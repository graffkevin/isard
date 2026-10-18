//! Entry points for the decoding workers: the same WebAssembly module runs in a pool of Web Workers (compiled once,
//! shared), which fetch and decode the 3D Tiles content off the main thread. The results are typed arrays ready for
//! the GPU, transferred to the main thread without copy.

use glam::DMat4;
use js_sys::{Array, Float32Array, Float64Array, Object, Reflect, Uint8Array, Uint32Array};
use tiles3d::decode;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
/**
 * A primitive of a tile decoded by `decodeTile`: vertices x, y, z, u, v around the anchor; the image still encoded,
 * the same array for the primitives sharing it
 */
export interface EncodedPrimitive {
  vertices: Float32Array<ArrayBuffer>;
  indices: Uint32Array<ArrayBuffer>;
  image?: Uint8Array<ArrayBuffer>;
  mime?: string;
  /** Base color factor of its material (RGBA 0-1), when not white */
  color?: Float32Array<ArrayBuffer>;
}

/** A tile decoded by `decodeTile` */
export interface EncodedTile {
  /** ECEF origin of the vertices (metres) */
  anchor: Float64Array<ArrayBuffer>;
  primitives: EncodedPrimitive[];
}

/** A texture transcoded for the GPU (KTX2 / Basis Universal): a WebGL internal format and its mip levels */
export interface CompressedTexture {
  /** WebGL internal format (compressed, or RGBA8 when the GPU supports none of the targets) */
  format: number;
  levels: { width: number; height: number; data: Uint8Array<ArrayBuffer> }[];
}

/**
 * A primitive ready for the GPU: its image decoded (bitmap) or transcoded (texture); primitives given the same bitmap
 * or texture object share one GPU texture
 */
export interface DecodedPrimitive {
  vertices: Float32Array<ArrayBuffer>;
  indices: Uint32Array<ArrayBuffer>;
  bitmap: ImageBitmap | null;
  texture?: CompressedTexture;
  /** Base color factor of its material (RGBA 0-1), when not white */
  color?: Float32Array<ArrayBuffer>;
}

/** A Draco-compressed primitive of a tile (`KHR_draco_mesh_compression`), to decode with the Draco module */
export interface DracoRequest {
  mesh: number;
  primitive: number;
  data: Uint8Array<ArrayBuffer>;
  /** Draco attribute ids */
  position: number;
  uv?: number;
}

/** The Draco module's output for a request: positions and uv as stored, triangles */
export interface DracoGeometry {
  mesh: number;
  primitive: number;
  positions: Float32Array;
  uvs?: Float32Array;
  indices: Uint32Array;
}

/** Compressed texture formats supported by the GPU */
export interface TextureFormats {
  astc: boolean;
  bptc: boolean;
  etc: boolean;
  s3tc: boolean;
}

/** A tile ready for the GPU, as the decoding workers deliver it */
export interface DecodedTile {
  anchor: Float64Array<ArrayBuffer>;
  primitives: DecodedPrimitive[];
}

/** What the engine expects from a pool of decoding workers (`Engine.setTileLoader`) */
export interface TileLoaderLike {
  /**
   * `reload`: bypass the HTTP cache (the tile failed before: the cache may hold the error); `priority`: the Fetch
   * Priority for the download (the first tiles in view of a frame are urgent)
   */
  load(
    id: number,
    url: string,
    transform: Float64Array,
    yUp: boolean,
    reload: boolean,
    priority?: 'high' | 'low',
  ): Promise<DecodedTile>;
  abort(id: number): void;
}
"#;

/// The Draco-compressed primitives of a tile, to decode before `decodeTile`
#[wasm_bindgen(js_name = dracoRequests, unchecked_return_type = "DracoRequest[]")]
pub fn draco_requests(bytes: &[u8]) -> Result<Array, JsValue> {
    let out = Array::new();
    for r in decode::draco_requests(bytes)? {
        let request = Object::new();
        Reflect::set(&request, &"mesh".into(), &(r.mesh as u32).into())?;
        Reflect::set(&request, &"primitive".into(), &(r.primitive as u32).into())?;
        Reflect::set(&request, &"data".into(), &Uint8Array::from(&r.data[..]))?;
        Reflect::set(&request, &"position".into(), &r.position.into())?;
        if let Some(uv) = r.uv {
            Reflect::set(&request, &"uv".into(), &uv.into())?;
        }
        out.push(&request);
    }
    Ok(out)
}

/// Draco geometries handed over from JavaScript
fn draco_geometries(value: &JsValue) -> Result<Vec<decode::DracoGeometry>, JsValue> {
    if value.is_undefined() || value.is_null() {
        return Ok(Vec::new());
    }
    let number = |g: &JsValue, key: &str| {
        Reflect::get(g, &key.into())
            .ok()
            .and_then(|v| v.as_f64())
            .map(|v| v as usize)
    };
    Array::from(value)
        .iter()
        .map(|g| {
            let floats = |key: &str| -> Vec<f32> {
                Reflect::get(&g, &key.into())
                    .ok()
                    .and_then(|v| v.dyn_into::<Float32Array>().ok())
                    .map(|a| a.to_vec())
                    .unwrap_or_default()
            };
            let indices: Uint32Array = Reflect::get(&g, &"indices".into())?.dyn_into()?;
            Ok(decode::DracoGeometry {
                mesh: number(&g, "mesh").ok_or("Draco geometry without mesh")?,
                primitive: number(&g, "primitive").ok_or("Draco geometry without primitive")?,
                positions: floats("positions"),
                uvs: floats("uvs"),
                indices: indices.to_vec(),
            })
        })
        .collect()
}

/// Decodes a `.b3dm` / `.glb` tile: `{ anchor: Float64Array(3), primitives: [{ vertices: Float32Array (x, y, z, u,
/// v per vertex, relative to the anchor), indices: Uint32Array, image?: Uint8Array, mime?: string, color?:
/// Float32Array (the material's base color, when not white) }] }`.
/// `transform`: the tile's world transform (16 numbers, column-major).
/// `draco`: the decoded geometries of its Draco primitives (`dracoRequests`), if any.
#[wasm_bindgen(js_name = decodeTile, unchecked_return_type = "EncodedTile")]
pub fn decode_tile(
    bytes: &[u8],
    transform: &[f64],
    y_up: bool,
    #[wasm_bindgen(unchecked_param_type = "DracoGeometry[] | undefined")] draco: &JsValue,
) -> Result<Object, JsValue> {
    let transform = DMat4::from_cols_slice(transform.get(..16).ok_or("transform: 16 numbers expected")?);
    let mesh = decode::decode(bytes, &transform, y_up, &draco_geometries(draco)?)?;
    let out = Object::new();
    let set = |target: &Object, key: &str, value: &JsValue| Reflect::set(target, &JsValue::from_str(key), value);
    set(&out, "anchor", &Float64Array::from(&mesh.anchor.to_array()[..]))?;
    // One array per image, shared by the primitives that use it
    let images: Vec<Uint8Array> = mesh
        .images
        .iter()
        .map(|(bytes, _)| Uint8Array::from(&bytes[..]))
        .collect();
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
        if let (Some(image), Some((_, mime))) = (p.image.and_then(|i| images.get(i)), mesh.image(p)) {
            set(&primitive, "image", image)?;
            set(&primitive, "mime", &JsValue::from_str(mime))?;
        }
        if p.color.iter().any(|&c| c < 1.0) {
            set(&primitive, "color", &Float32Array::from(&p.color[..]))?;
        }
        primitives.push(&primitive);
    }
    set(&out, "primitives", &primitives)?;
    Ok(out)
}
