//! The downloads of a raster source: images decoded by the browser, altitudes as raw floats, tiles and altitudes drawn
//! by the page's loader; and the timer of the retries

use super::TILE;
use super::data::{apply_mask, fill_no_data, to_ellipsoid};
use geo::tiles::TileId;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// The content of an arrived tile
pub enum Pixels {
    Image(web_sys::ImageBitmap),
    /// 256 × 256 altitudes, rows from the north
    Heights(Vec<f32>),
}

impl Pixels {
    /// Frees the decoded image, outside the wasm heap
    pub fn close(&self) {
        if let Self::Image(bitmap) = self {
            bitmap.close();
        }
    }
}

pub enum Fetch {
    /// No tile at this place (404, 204): final
    Missing,
    /// Network error, rate limit, server error, decoding error: worth a retry
    Transient,
}

/// Fetches an image and decodes it off the main thread, without premultiplied alpha (the shader blends straight
/// alpha). `priority`: the Fetch Priority (`"high"`, `"low"`).
/// `reload`: bypass the HTTP cache. The Géoplateforme sends its error answers with a 21-day cache lifetime: a retry
/// served from the cache would read the same error for weeks; reloading also replaces the cached error.
pub async fn fetch_bitmap(
    url: &str,
    size: i32,
    signal: &web_sys::AbortSignal,
    reload: bool,
    priority: &str,
) -> Result<web_sys::ImageBitmap, Fetch> {
    let response = fetch_response(url, signal, reload, priority)
        .await
        .map_err(|_| Fetch::Transient)?;
    match response.status() {
        204 | 404 | 410 => return Err(Fetch::Missing),
        200..=299 => {}
        _ => return Err(Fetch::Transient),
    }
    decode(&response, size).await.map_err(|_| Fetch::Transient)
}

/// A tile drawn by the page's loader: an image of the pool's size, `null` for no tile here
pub async fn draw_tile(
    loader: &js_sys::Function,
    tile: TileId,
    signal: &web_sys::AbortSignal,
) -> Result<web_sys::ImageBitmap, Fetch> {
    let promise = loader
        .call4(
            &JsValue::NULL,
            &tile.z.into(),
            &tile.x.into(),
            &tile.y.into(),
            signal.as_ref(),
        )
        .map_err(|_| Fetch::Transient)?;
    let value = JsFuture::from(js_sys::Promise::resolve(&promise))
        .await
        .map_err(|_| Fetch::Transient)?;
    if value.is_null() || value.is_undefined() {
        return Err(Fetch::Missing);
    }
    value.dyn_into().map_err(|_| Fetch::Transient)
}

/// Altitudes of a tile drawn by the page's loader (decoded from an encoding the engine does not read, such as the
/// RGB of Terrarium PNGs): 256 × 256 floats, rows from the north, `null` for no tile here. No-data and the geoid as for
/// a fetched grid
pub async fn draw_heights(
    loader: &js_sys::Function,
    tile: TileId,
    geoid: bool,
    no_data: f32,
    signal: &web_sys::AbortSignal,
) -> Result<Vec<f32>, Fetch> {
    let promise = loader
        .call4(
            &JsValue::NULL,
            &tile.z.into(),
            &tile.x.into(),
            &tile.y.into(),
            signal.as_ref(),
        )
        .map_err(|_| Fetch::Transient)?;
    let value = JsFuture::from(js_sys::Promise::resolve(&promise))
        .await
        .map_err(|_| Fetch::Transient)?;
    if value.is_null() || value.is_undefined() {
        return Err(Fetch::Missing);
    }
    let grid: js_sys::Float32Array = value.dyn_into().map_err(|_| Fetch::Transient)?;
    if grid.length() != (TILE * TILE) as u32 {
        return Err(Fetch::Transient);
    }
    let mut heights = grid.to_vec();
    fill_no_data(&mut heights, no_data);
    if geoid {
        to_ellipsoid(tile, &mut heights);
    }
    Ok(heights)
}

/// Altitudes of a tile: a full grid of 256 × 256 little-endian f32, maybe followed by the validity mask of an `ArcGIS`
/// image service (1 bit per pixel). A server error may come as a 200 with an XML exception: only a grid counts.
/// No-data (outside the coverage, under `no_data`) takes the lowest altitude of the tile: neither pit nor wall at the
/// coast
pub async fn fetch_heights(
    url: &str,
    tile: TileId,
    geoid: bool,
    no_data: f32,
    signal: &web_sys::AbortSignal,
    reload: bool,
    priority: &str,
) -> Result<Vec<f32>, Fetch> {
    let response = fetch_response(url, signal, reload, priority)
        .await
        .map_err(|_| Fetch::Transient)?;
    match response.status() {
        204 | 404 | 410 => return Err(Fetch::Missing),
        200..=299 => {}
        _ => return Err(Fetch::Transient),
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|_| Fetch::Transient)?)
        .await
        .map_err(|_| Fetch::Transient)?;
    let buffer: js_sys::ArrayBuffer = buffer.dyn_into().map_err(|_| Fetch::Transient)?;
    let (grid, mask) = ((TILE * TILE * 4) as u32, (TILE * TILE / 8) as u32);
    let length = buffer.byte_length();
    if length != grid && length != grid + mask {
        return Err(Fetch::Transient);
    }
    // One copy, straight into the floats: a Float32Array reads the platform's byte order, little-endian in
    // WebAssembly, that of the grid
    let mut heights = vec![0f32; (TILE * TILE) as usize];
    js_sys::Float32Array::new_with_byte_offset_and_length(&buffer, 0, grid / 4).copy_to(&mut heights);
    if length > grid {
        apply_mask(
            &mut heights,
            &js_sys::Uint8Array::new_with_byte_offset(&buffer, grid).to_vec(),
        );
    }
    fill_no_data(&mut heights, no_data);
    if geoid {
        to_ellipsoid(tile, &mut heights);
    }
    Ok(heights)
}

async fn fetch_response(
    url: &str,
    signal: &web_sys::AbortSignal,
    reload: bool,
    priority: &str,
) -> Result<web_sys::Response, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_signal(Some(signal));
    // The tiles at the screen center first; always explicit (see `fetch_priority`)
    crate::js::fetch_priority(&init, priority);
    if reload {
        init.set_cache(web_sys::RequestCache::Reload);
    }
    JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await?
        .dyn_into()
}

async fn decode(response: &web_sys::Response, size: i32) -> Result<web_sys::ImageBitmap, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let blob: web_sys::Blob = JsFuture::from(response.blob()?).await?.dyn_into()?;
    let options = web_sys::ImageBitmapOptions::new();
    options.set_premultiply_alpha(web_sys::PremultiplyAlpha::None);
    options.set_color_space_conversion(web_sys::ColorSpaceConversion::None);
    // Every slot has the pool's size: any other size (512 px tiles, an error image) is resized, never half-copied
    options.set_resize_width(size as u32);
    options.set_resize_height(size as u32);
    let bitmap =
        JsFuture::from(window.create_image_bitmap_with_blob_and_image_bitmap_options(&blob, &options)?).await?;
    bitmap.dyn_into()
}
