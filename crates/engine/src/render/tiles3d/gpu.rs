//! The meshes of a 3D Tiles layer on the GPU: uploads of the tiles decoded here or by a worker, compressed textures,
//! deletion. A failed upload deletes what it had created and closes the images left

use super::fetch::{get_array, js_get};
use super::state::Resident;
use crate::render::gl::{self, Objects, bytes};
use glow::HasContext;
use tiles3d::decode::Mesh;
use wasm_bindgen::{JsCast, JsValue};

pub struct GpuPrimitive {
    pub vao: glow::VertexArray,
    buffers: [glow::Buffer; 2],
    pub count: i32,
    pub texture: Option<glow::Texture>,
}

pub struct GpuMesh {
    pub anchor: glam::DVec3,
    pub primitives: Vec<GpuPrimitive>,
    bytes: usize,
    used: u64,
}

impl Resident for GpuMesh {
    fn bytes(&self) -> usize {
        self.bytes
    }

    fn used(&self) -> u64 {
        self.used
    }

    fn touch(&mut self, frame: u64) {
        self.used = frame;
    }
}

/// A vertex array of a primitive: vertices x, y, z, u, v (20 bytes) and indices, uploaded by `fill` into the bound
/// buffers (from the wasm heap or straight from JavaScript). Deleted again if `fill` fails
fn primitive_arrays(
    gl: &glow::Context,
    fill: impl FnOnce(u32) -> Result<(), JsValue>,
    fill_indices: impl FnOnce(u32) -> Result<(), JsValue>,
) -> Result<(glow::VertexArray, [glow::Buffer; 2]), JsValue> {
    let mut objects = Objects::new(gl);
    let vao = objects.vertex_array()?;
    let vbo = objects.buffer()?;
    let ibo = objects.buffer()?;
    // SAFETY: state setup of objects just created on the live context
    unsafe {
        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        fill(glow::ARRAY_BUFFER)?;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 20, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 20, 12);
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
        fill_indices(glow::ELEMENT_ARRAY_BUFFER)?;
        gl.bind_vertex_array(None);
    }
    objects.keep();
    Ok((vao, [vbo, ibo]))
}

/// Uploads a mesh decoded on this thread; its images are closed, uploaded or not
pub fn upload(
    gl: &glow::Context,
    mesh: &Mesh,
    images: Vec<Option<web_sys::ImageBitmap>>,
    frame: u64,
) -> Result<GpuMesh, String> {
    let mut gpu = GpuMesh {
        anchor: mesh.anchor,
        primitives: Vec::new(),
        bytes: 0,
        used: frame,
    };
    let mut images = images.into_iter();
    for p in &mesh.primitives {
        let image = images.next().flatten();
        let made = upload_primitive(gl, p, image.as_ref(), &mut gpu);
        if let Err(e) = made {
            if let Some(image) = image {
                image.close();
            }
            images.flatten().for_each(|i| i.close());
            delete_mesh(gl, &gpu);
            return Err(e);
        }
    }
    Ok(gpu)
}

fn upload_primitive(
    gl: &glow::Context,
    p: &tiles3d::decode::Primitive,
    image: Option<&web_sys::ImageBitmap>,
    gpu: &mut GpuMesh,
) -> Result<(), String> {
    let vertices: Vec<f32> = p
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(p.uvs.as_chunks::<2>().0)
        .flat_map(|(&[x, y, z], &[u, v])| [x, y, z, u, v])
        .collect();
    let (vao, buffers) = primitive_arrays(
        gl,
        |target| {
            // SAFETY: upload into the buffer bound by `primitive_arrays`
            unsafe { gl.buffer_data_u8_slice(target, bytes(&vertices), glow::STATIC_DRAW) };
            Ok(())
        },
        |target| {
            // SAFETY: upload into the buffer bound by `primitive_arrays`
            unsafe { gl.buffer_data_u8_slice(target, bytes(&p.indices), glow::STATIC_DRAW) };
            Ok(())
        },
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut primitive = GpuPrimitive {
        vao,
        buffers,
        count: p.indices.len() as i32,
        texture: None,
    };
    gpu.bytes += vertices.len() * 4 + p.indices.len() * 4;
    if let Some(bitmap) = image {
        match texture(gl, bitmap) {
            Ok((texture, size)) => {
                primitive.texture = Some(texture);
                gpu.bytes += size;
            }
            Err(e) => {
                delete_primitive(gl, &primitive);
                return Err(e);
            }
        }
    }
    gpu.primitives.push(primitive);
    Ok(())
}

/// Uploads a tile decoded by a worker, straight from its typed arrays (no copy into the wasm heap). On a failure
/// nothing is left on the GPU; the caller closes the tile's images
pub fn upload_decoded(
    gl: &glow::Context,
    raw: &web_sys::WebGl2RenderingContext,
    tile: &JsValue,
    frame: u64,
) -> Result<GpuMesh, JsValue> {
    let anchor: js_sys::Float64Array = js_get(tile, "anchor").dyn_into()?;
    if anchor.length() < 3 {
        return Err("decoded tile: an anchor of 3 numbers expected".into());
    }
    let anchor = glam::DVec3::new(anchor.get_index(0), anchor.get_index(1), anchor.get_index(2));
    let mut gpu = GpuMesh {
        anchor,
        primitives: Vec::new(),
        bytes: 0,
        used: frame,
    };
    for p in get_array(tile, "primitives").iter() {
        if let Err(e) = upload_decoded_primitive(gl, raw, &p, &mut gpu) {
            delete_mesh(gl, &gpu);
            return Err(e);
        }
    }
    Ok(gpu)
}

fn upload_decoded_primitive(
    gl: &glow::Context,
    raw: &web_sys::WebGl2RenderingContext,
    p: &JsValue,
    gpu: &mut GpuMesh,
) -> Result<(), JsValue> {
    let vertices: js_sys::Float32Array = js_get(p, "vertices").dyn_into()?;
    let indices: js_sys::Uint32Array = js_get(p, "indices").dyn_into()?;
    // glow binds, the raw context uploads into the bound buffers (the same WebGL state)
    let (vao, buffers) = primitive_arrays(
        gl,
        |target| {
            raw.buffer_data_with_array_buffer_view(target, &vertices, glow::STATIC_DRAW);
            Ok(())
        },
        |target| {
            raw.buffer_data_with_array_buffer_view(target, &indices, glow::STATIC_DRAW);
            Ok(())
        },
    )?;
    let mut primitive = GpuPrimitive {
        vao,
        buffers,
        count: indices.length() as i32,
        texture: None,
    };
    gpu.bytes += (vertices.byte_length() + indices.byte_length()) as usize;
    let made = if let Ok(bitmap) = js_get(p, "bitmap").dyn_into::<web_sys::ImageBitmap>() {
        texture(gl, &bitmap).map(Some).map_err(JsValue::from)
    } else if js_get(p, "texture").is_object() {
        compressed_texture(gl, raw, &js_get(p, "texture"))
    } else {
        Ok(None)
    };
    match made {
        Ok(texture) => {
            if let Some((texture, size)) = texture {
                primitive.texture = Some(texture);
                gpu.bytes += size;
            }
            gpu.primitives.push(primitive);
            Ok(())
        }
        Err(e) => {
            delete_primitive(gl, &primitive);
            Err(e)
        }
    }
}

/// Bytes of a mip level of `width` × `height` in `format` (RGBA8 or the block formats the workers transcode to), None
/// for another format or a side that is not positive
fn level_bytes(format: u32, width: i32, height: i32) -> Option<usize> {
    let (w, h) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    let block = match format {
        glow::RGBA8 => return w.checked_mul(h)?.checked_mul(4),
        // BC1 (S3TC RGB / RGBA), ETC2 RGB, ETC1
        0x83f0 | 0x83f1 | 0x9274 | 0x8d64 => 8,
        // BC2, BC3, BC7, ETC2 RGBA, ASTC 4×4
        0x83f2 | 0x83f3 | 0x8e8c | 0x9278 | 0x93b0 => 16,
        _ => return None,
    };
    w.div_ceil(4).checked_mul(h.div_ceil(4))?.checked_mul(block)
}

/// Whether mip levels (width, height, bytes) make a texture in `format`: each level half the one before (at least 1),
/// with exactly the bytes its size needs. A wrong one would leave the texture incomplete (drawn black) without an
/// error
fn levels_fit(format: u32, levels: &[(i32, i32, usize)]) -> bool {
    let Some(&(w0, h0, _)) = levels.first() else {
        return false;
    };
    levels.len() <= 32
        && levels.iter().enumerate().all(|(k, &(w, h, length))| {
            w == (w0 >> k).max(1) && h == (h0 >> k).max(1) && level_bytes(format, w, h) == Some(length)
        })
}

/// Texture transcoded by a worker (KTX2): its mip levels uploaded as they are, compressed or RGBA8; None (drawn
/// untextured) if they do not make a texture (`levels_fit`); deleted again if a level fails
fn compressed_texture(
    gl: &glow::Context,
    raw: &web_sys::WebGl2RenderingContext,
    texture: &JsValue,
) -> Result<Option<(glow::Texture, usize)>, JsValue> {
    let format = js_get(texture, "format")
        .as_f64()
        .ok_or("compressed texture without format")? as u32;
    let mut levels = Vec::new();
    let mut sizes = Vec::new();
    for info in get_array(texture, "levels").iter() {
        let width = js_get(&info, "width").as_f64().unwrap_or(0.0) as i32;
        let height = js_get(&info, "height").as_f64().unwrap_or(0.0) as i32;
        let data: js_sys::Uint8Array = js_get(&info, "data").dyn_into()?;
        sizes.push((width, height, data.byte_length() as usize));
        levels.push(data);
    }
    if !levels_fit(format, &sizes) {
        return Ok(None);
    }
    let mut objects = Objects::new(gl);
    let handle = objects.texture()?;
    let bytes = sizes.iter().map(|&(_, _, length)| length).sum();
    // SAFETY: setup of a texture just created on the live context; glow binds, the raw context uploads into the bound
    // texture
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(handle));
        for (level, (data, &(width, height, _))) in levels.iter().zip(&sizes).enumerate() {
            if format == glow::RGBA8 {
                raw.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_array_buffer_view(
                    glow::TEXTURE_2D,
                    level as i32,
                    glow::RGBA8 as i32,
                    width,
                    height,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    Some(data),
                )?;
            } else {
                raw.compressed_tex_image_2d_with_array_buffer_view(
                    glow::TEXTURE_2D,
                    level as i32,
                    format,
                    width,
                    height,
                    0,
                    data,
                );
            }
        }
        let count = levels.len() as i32;
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, (count - 1).max(0));
        let min = if count > 1 {
            glow::LINEAR_MIPMAP_LINEAR
        } else {
            glow::LINEAR
        };
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    }
    objects.keep();
    Ok(Some((handle, bytes)))
}

/// Mipmapped texture from a decoded image (closed after the upload); returns it with its GPU size
fn texture(gl: &glow::Context, bitmap: &web_sys::ImageBitmap) -> Result<(glow::Texture, usize), String> {
    // SAFETY: creation on the live context
    let texture = match unsafe { gl.create_texture() } {
        Ok(texture) => texture,
        Err(e) => {
            bitmap.close();
            return Err(e);
        }
    };
    // SAFETY: setup of a texture just created on the live context
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl::image_bitmap_2d(gl, bitmap);
        // Mipmaps: no shimmering when the mesh is seen from afar
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR_MIPMAP_LINEAR as i32,
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    }
    let bytes = (bitmap.width() * bitmap.height()) as usize * 4 * 4 / 3;
    bitmap.close();
    Ok((texture, bytes))
}

fn delete_primitive(gl: &glow::Context, p: &GpuPrimitive) {
    // SAFETY: objects owned by the primitive, not used afterwards
    unsafe {
        gl.delete_vertex_array(p.vao);
        for b in p.buffers {
            gl.delete_buffer(b);
        }
        if let Some(t) = p.texture {
            gl.delete_texture(t);
        }
    }
}

pub fn delete_mesh(gl: &glow::Context, mesh: &GpuMesh) {
    for p in &mesh.primitives {
        delete_primitive(gl, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_levels_must_have_the_size_of_their_format() {
        // BC7 (16 bytes per 4 × 4 block): 8 × 8, 4 × 4, 2 × 2, 1 × 1
        let good = [(8, 8, 64), (4, 4, 16), (2, 2, 16), (1, 1, 16)];
        assert!(levels_fit(0x8e8c, &good));
        // BC1 (8 bytes a block) does not fit the same bytes; RGBA8 has its own
        assert!(!levels_fit(0x83f0, &good));
        assert!(levels_fit(glow::RGBA8, &[(3, 2, 24), (1, 1, 4)]));
        assert_eq!(level_bytes(0x93b0, 5, 5), Some(64));
        assert_eq!(level_bytes(0x9274, 1, 1), Some(8));
        // Short data, a level of the wrong size, no level, an unknown format, sides not positive
        assert!(!levels_fit(0x8e8c, &[(8, 8, 63)]));
        assert!(!levels_fit(0x8e8c, &[(8, 8, 64), (8, 8, 64)]));
        assert!(!levels_fit(0x8e8c, &[]));
        assert!(!levels_fit(0x1234, &[(4, 4, 16)]));
        assert!(!levels_fit(glow::RGBA8, &[(0, 4, 0)]));
        assert_eq!(level_bytes(glow::RGBA8, -1, 4), None);
    }
}
