//! The meshes of a 3D Tiles layer on the GPU: uploads of the tiles decoded here or by a worker, compressed textures,
//! deletion. A failed upload deletes what it had created and closes the images left

use super::fetch::{get_array, js_get};
use super::state::Resident;
use crate::render::gl::{self, Objects, bytes};
use crate::render::points::{GpuPoints, tile_vertices};
use glow::HasContext;
use tiles3d::decode::Mesh;
use wasm_bindgen::{JsCast, JsValue};

/// Location of the feature of each vertex (`a_feature` of the picking pass); a primitive without features gives none
pub const FEATURE_ATTRIBUTE: u32 = 7;

/// A glTF primitive on the GPU: its vertex array and buffers (vertices, indices), drawn as indexed triangles
pub struct GpuPrimitive {
    /// Its attributes and index buffer
    pub vao: glow::VertexArray,
    buffers: [glow::Buffer; 2],
    /// The feature of each vertex (at `FEATURE_ATTRIBUTE`), when the tile has features
    pub features: Option<glow::Buffer>,
    /// Index count
    pub count: i32,
    /// One of the mesh's `textures`
    pub texture: Option<glow::Texture>,
    /// Base color factor of its material (the models multiply their color by it; the 3D Tiles leave it)
    pub color: [f32; 4],
}

/// A tile's or a model's content on the GPU: its primitives and points, relative to its anchor
pub struct GpuMesh {
    /// The origin of the mesh's vertices (ECEF metres, or the model's frame): kept in f64, the matrices made
    /// relative to the camera
    pub anchor: glam::DVec3,
    /// Its triangles, one draw each
    pub primitives: Vec<GpuPrimitive>,
    /// Its points (`.pnts`, glTF points)
    pub points: Vec<GpuPoints>,
    /// The properties of its features, a JavaScript array of one object per feature (`GpuPrimitive::features`
    /// index it)
    pub properties: Option<JsValue>,
    /// Features of the tile: those of its properties, at least one past the highest feature of its vertices
    pub feature_count: u32,
    /// Its textures, each once however many primitives share it
    textures: Vec<glow::Texture>,
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

/// The buffers of a primitive's vertex array: vertices, indices, features
type Arrays = (glow::VertexArray, [glow::Buffer; 2], Option<glow::Buffer>);

/// A vertex array of a primitive: vertices x, y, z, u, v (20 bytes), indices, and its features if `fill_features`
/// is given, uploaded into the bound buffers (from the wasm heap or straight from JavaScript). Deleted again if a fill
/// fails
fn primitive_arrays(
    gl: &glow::Context,
    fill: impl FnOnce(u32) -> Result<(), JsValue>,
    fill_indices: impl FnOnce(u32) -> Result<(), JsValue>,
    fill_features: Option<impl FnOnce(u32) -> Result<(), JsValue>>,
) -> Result<Arrays, JsValue> {
    let mut objects = Objects::new(gl);
    let vao = objects.vertex_array()?;
    let vbo = objects.buffer()?;
    let ibo = objects.buffer()?;
    let features = match fill_features {
        Some(_) => Some(objects.buffer()?),
        None => None,
    };

    // SAFETY: state setup of objects just created on the live context
    unsafe {
        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        fill(glow::ARRAY_BUFFER)?;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 20, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 20, 12);
        if let (Some(buffer), Some(fill_features)) = (features, fill_features) {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            fill_features(glow::ARRAY_BUFFER)?;
            gl.enable_vertex_attrib_array(FEATURE_ATTRIBUTE);
            gl.vertex_attrib_pointer_f32(FEATURE_ATTRIBUTE, 1, glow::FLOAT, false, 4, 0);
        }
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
        fill_indices(glow::ELEMENT_ARRAY_BUFFER)?;
        gl.bind_vertex_array(None);
    }
    objects.keep();

    Ok((vao, [vbo, ibo], features))
}

/// The properties of a tile's features as JavaScript: an array of one object per feature
pub fn feature_properties(properties: &tiles3d::decode::Properties) -> JsValue {
    use js_sys::{Array, Object, Reflect};
    use tiles3d::decode::Property;
    let out = Array::new();

    for feature in &properties.features {
        let object = Object::new();

        for (name, value) in properties.names.iter().zip(feature) {
            let value = match value {
                Property::Null => JsValue::NULL,
                Property::Bool(b) => JsValue::from_bool(*b),
                Property::Number(n) => JsValue::from_f64(*n),
                Property::Text(s) => JsValue::from_str(s),
            };
            // Setting a property on a fresh plain object cannot fail
            let _ = Reflect::set(&object, &JsValue::from_str(name), &value);
        }
        out.push(&object);
    }

    out.into()
}

/// One past the highest feature of a primitive's vertices
fn feature_bound(features: impl IntoIterator<Item = f32>) -> u32 {
    features
        .into_iter()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .fold(0.0f32, f32::max)
        .min(16_000_000.0) as u32
        + 1
}

/// Uploads a mesh decoded on this thread, with its images decoded (one per `Mesh::images`, a texture each, shared by
/// the primitives using it); they are closed, uploaded or not
pub fn upload(
    gl: &glow::Context,
    mesh: &Mesh,
    images: Vec<Option<web_sys::ImageBitmap>>,
    frame: u64,
) -> Result<GpuMesh, String> {
    let mut gpu = GpuMesh {
        anchor: mesh.anchor,
        primitives: Vec::new(),
        points: Vec::new(),
        properties: None,
        feature_count: 0,
        textures: Vec::new(),
        bytes: 0,
        used: frame,
    };
    let mut textures = Vec::with_capacity(images.len());
    let mut images = images.into_iter();

    while let Some(image) = images.next() {
        let Some(bitmap) = image else {
            textures.push(None);
            continue;
        };

        match texture(gl, &bitmap) {
            Ok((texture, size)) => {
                gpu.textures.push(texture);
                gpu.bytes += size;
                textures.push(Some(texture));
            }
            Err(e) => {
                images.flatten().for_each(|i| i.close());
                delete_mesh(gl, &gpu);

                return Err(e);
            }
        }
    }
    for p in &mesh.primitives {
        let texture = p.image.and_then(|i| textures.get(i).copied().flatten());

        if let Err(e) = upload_primitive(gl, p, texture, &mut gpu) {
            delete_mesh(gl, &gpu);

            return Err(e);
        }
    }
    if let Some(properties) = &mesh.properties {
        gpu.properties = Some(feature_properties(properties));
        gpu.feature_count = gpu.feature_count.max(properties.features.len() as u32);
    }
    for p in &mesh.points {
        match GpuPoints::upload(gl, &tile_vertices(p)) {
            Ok(points) => {
                gpu.bytes += points.bytes;
                gpu.points.push(points);
            }
            Err(e) => {
                delete_mesh(gl, &gpu);

                return Err(format!("{e:?}"));
            }
        }
    }

    Ok(gpu)
}

fn upload_primitive(
    gl: &glow::Context,
    p: &tiles3d::decode::Primitive,
    texture: Option<glow::Texture>,
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
    let (vao, buffers, features_buffer) = primitive_arrays(
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
        p.features.as_ref().map(|features| {
            |target| {
                // SAFETY: upload into the buffer bound by `primitive_arrays`
                unsafe { gl.buffer_data_u8_slice(target, bytes(features), glow::STATIC_DRAW) };

                Ok(())
            }
        }),
    )
    .map_err(|e| format!("{e:?}"))?;

    if let Some(features) = &p.features {
        gpu.feature_count = gpu.feature_count.max(feature_bound(features.iter().copied()));
        gpu.bytes += features.len() * 4;
    }
    gpu.primitives.push(GpuPrimitive {
        vao,
        buffers,
        features: features_buffer,
        count: p.indices.len() as i32,
        texture,
        color: p.color,
    });
    gpu.bytes += vertices.len() * 4 + p.indices.len() * 4;

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
        points: Vec::new(),
        properties: None,
        feature_count: 0,
        textures: Vec::new(),
        bytes: 0,
        used: frame,
    };
    let properties = js_get(tile, "properties");

    if let Ok(list) = properties.clone().dyn_into::<js_sys::Array>() {
        gpu.feature_count = list.length();
        gpu.properties = Some(properties);
    }

    // The bitmaps and transcoded textures met so far and their GPU textures: the same object, the same texture
    let mut shared: Vec<(JsValue, Option<glow::Texture>)> = Vec::new();

    for p in get_array(tile, "primitives").iter() {
        if let Err(e) = upload_decoded_primitive(gl, raw, &p, &mut gpu, &mut shared) {
            delete_mesh(gl, &gpu);

            return Err(e);
        }
    }
    for vertices in get_array(tile, "points").iter() {
        let uploaded = vertices
            .dyn_into::<js_sys::Uint8Array>()
            .map_err(|_| JsValue::from_str("decoded tile: points of no Uint8Array"))
            .and_then(|v| GpuPoints::upload_js(gl, raw, &v));

        match uploaded {
            Ok(points) => {
                gpu.bytes += points.bytes;
                gpu.points.push(points);
            }
            Err(e) => {
                delete_mesh(gl, &gpu);

                return Err(e);
            }
        }
    }

    Ok(gpu)
}

/// Uploads one primitive a worker decoded into `gpu`; its texture made once per image, shared (`shared`) by the
/// primitives using the same one. Nothing of it is left on a failure
fn upload_decoded_primitive(
    gl: &glow::Context,
    raw: &web_sys::WebGl2RenderingContext,
    p: &JsValue,
    gpu: &mut GpuMesh,
    shared: &mut Vec<(JsValue, Option<glow::Texture>)>,
) -> Result<(), JsValue> {
    let primitive = upload_decoded_buffers(gl, raw, p, gpu)?;
    let (bitmap, compressed) = (js_get(p, "bitmap"), js_get(p, "texture"));
    let source = if bitmap.is_object() { bitmap } else { compressed };

    if !source.is_object() {
        gpu.primitives.push(primitive);

        return Ok(());
    }
    if let Some((_, texture)) = shared.iter().find(|(s, _)| js_sys::Object::is(s, &source)) {
        gpu.primitives.push(GpuPrimitive {
            texture: *texture,
            ..primitive
        });

        return Ok(());
    }

    match decoded_texture(gl, raw, &source, gpu) {
        Ok(texture) => {
            shared.push((source, texture));
            gpu.primitives.push(GpuPrimitive { texture, ..primitive });

            Ok(())
        }
        Err(e) => {
            delete_primitive(gl, &primitive);

            Err(e)
        }
    }
}

/// The buffers of a primitive a worker decoded (vertices, indices, the feature of each vertex if any), counted in
/// `gpu`'s memory and features; no texture yet
fn upload_decoded_buffers(
    gl: &glow::Context,
    raw: &web_sys::WebGl2RenderingContext,
    p: &JsValue,
    gpu: &mut GpuMesh,
) -> Result<GpuPrimitive, JsValue> {
    let vertices: js_sys::Float32Array = js_get(p, "vertices").dyn_into()?;
    let indices: js_sys::Uint32Array = js_get(p, "indices").dyn_into()?;
    let features = js_get(p, "features").dyn_into::<js_sys::Float32Array>().ok();
    // glow binds, the raw context uploads into the bound buffers (the same WebGL state)
    let (vao, buffers, features_buffer) = primitive_arrays(
        gl,
        |target| {
            raw.buffer_data_with_array_buffer_view(target, &vertices, glow::STATIC_DRAW);

            Ok(())
        },
        |target| {
            raw.buffer_data_with_array_buffer_view(target, &indices, glow::STATIC_DRAW);

            Ok(())
        },
        features.as_ref().map(|features| {
            |target| {
                raw.buffer_data_with_array_buffer_view(target, features, glow::STATIC_DRAW);

                Ok(())
            }
        }),
    )?;

    if let Some(features) = &features {
        gpu.feature_count = gpu.feature_count.max(feature_bound(features.to_vec()));
        gpu.bytes += features.byte_length() as usize;
    }
    gpu.bytes += (vertices.byte_length() + indices.byte_length()) as usize;

    Ok(GpuPrimitive {
        vao,
        buffers,
        features: features_buffer,
        count: indices.length() as i32,
        texture: None,
        color: color_of(p),
    })
}

/// The texture of a decoded image (an `ImageBitmap`) or of a transcoded one (KTX2 levels), owned by `gpu` and counted
/// in its memory; None (drawn untextured) when transcoded levels do not make a texture
fn decoded_texture(
    gl: &glow::Context,
    raw: &web_sys::WebGl2RenderingContext,
    source: &JsValue,
    gpu: &mut GpuMesh,
) -> Result<Option<glow::Texture>, JsValue> {
    let made = match source.clone().dyn_into::<web_sys::ImageBitmap>() {
        Ok(bitmap) => texture(gl, &bitmap).map(Some).map_err(JsValue::from),
        Err(_) => compressed_texture(gl, raw, source),
    }?;

    Ok(made.map(|(texture, size)| {
        gpu.textures.push(texture);
        gpu.bytes += size;

        texture
    }))
}

/// The base color of a primitive a worker decoded: white when it has none
fn color_of(p: &JsValue) -> [f32; 4] {
    let mut color = [1.0; 4];

    if let Ok(given) = js_get(p, "color").dyn_into::<js_sys::Float32Array>()
        && given.length() == 4
    {
        given.copy_to(&mut color);
    }

    color
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

/// Deletes the arrays of a primitive (its texture is the mesh's)
fn delete_primitive(gl: &glow::Context, p: &GpuPrimitive) {
    // SAFETY: objects owned by the primitive, not used afterwards
    unsafe {
        gl.delete_vertex_array(p.vao);

        for b in p.buffers.into_iter().chain(p.features) {
            gl.delete_buffer(b);
        }
    }
}

/// Deletes everything of the mesh (buffers, vertex arrays, textures); it is not used afterwards
pub fn delete_mesh(gl: &glow::Context, mesh: &GpuMesh) {
    for p in &mesh.primitives {
        delete_primitive(gl, p);
    }
    for p in &mesh.points {
        p.delete(gl);
    }

    for &t in &mesh.textures {
        // SAFETY: textures owned by the mesh, not used afterwards
        unsafe { gl.delete_texture(t) };
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
