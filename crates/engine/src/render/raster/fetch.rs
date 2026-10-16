//! The downloads of a raster source: images decoded by the browser, altitudes as raw floats, tiles drawn by the
//! page's loader; and the timer of the retries

use super::TILE;
use super::data::{fill_no_data, to_ellipsoid};
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
/// alpha)
/// `reload`: bypass the HTTP cache. The Géoplateforme sends its error answers with a 21-day cache lifetime: a retry
/// served from the cache would read the same error for weeks; reloading also replaces the cached error.
pub async fn fetch_bitmap(
    url: &str,
    size: i32,
    signal: &web_sys::AbortSignal,
    reload: bool,
) -> Result<web_sys::ImageBitmap, Fetch> {
    let response = fetch_response(url, signal, reload)
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

/// Altitudes of a tile: a full grid of 256 × 256 little-endian f32. A server error may come as a 200 with an XML
/// exception: only a grid counts. No-data (outside the coverage) takes the lowest altitude of the tile: neither pit
/// nor wall at the coast
pub async fn fetch_heights(
    url: &str,
    tile: TileId,
    geoid: bool,
    signal: &web_sys::AbortSignal,
    reload: bool,
) -> Result<Vec<f32>, Fetch> {
    let response = fetch_response(url, signal, reload)
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
    if buffer.byte_length() as usize != (TILE * TILE * 4) as usize {
        return Err(Fetch::Transient);
    }
    // One copy, straight into the floats: a Float32Array reads the platform's byte order, little-endian in
    // WebAssembly, that of the grid
    let mut heights = vec![0f32; (TILE * TILE) as usize];
    js_sys::Float32Array::new(&buffer).copy_to(&mut heights);
    fill_no_data(&mut heights);
    if geoid {
        to_ellipsoid(tile, &mut heights);
    }
    Ok(heights)
}

async fn fetch_response(url: &str, signal: &web_sys::AbortSignal, reload: bool) -> Result<web_sys::Response, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_signal(Some(signal));
    // The basemap first: it covers the whole screen
    crate::js::fetch_priority(&init, "high");
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
