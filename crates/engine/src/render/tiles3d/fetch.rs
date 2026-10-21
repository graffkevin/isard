//! The downloads of a 3D Tiles layer: files fetched here, images decoded by the browser, tiles handed to the worker
//! pool; reading the objects the workers deliver

use tiles3d::decode::Mesh;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

pub fn js_get(target: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(target, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

pub fn get_array(target: &JsValue, key: &str) -> js_sys::Array {
    js_get(target, key).dyn_into().unwrap_or_default()
}

/// Asks the worker pool for a tile; None if the loader cannot take it. `priority`: the Fetch Priority for the
/// worker's download (`"high"`, `"low"`), a sixth argument a loader may ignore
pub fn load_in_worker(
    loader: &JsValue,
    id: u32,
    url: &str,
    transform: &glam::DMat4,
    (y_up, reload): (bool, bool),
    priority: &str,
) -> Option<js_sys::Promise> {
    let load: js_sys::Function = js_get(loader, "load").dyn_into().ok()?;
    let args = js_sys::Array::of5(
        &JsValue::from(id),
        &JsValue::from_str(url),
        &js_sys::Float64Array::from(&transform.to_cols_array()[..]),
        &JsValue::from_bool(y_up),
        &JsValue::from_bool(reload),
    );

    args.push(&JsValue::from_str(priority));

    load.apply(loader, &args).ok()?.dyn_into().ok()
}

pub fn close(images: Vec<Option<web_sys::ImageBitmap>>) {
    for image in images.into_iter().flatten() {
        image.close();
    }
}

/// Decodes the images of a mesh off the main thread (`createImageBitmap`), each once (`Mesh::images`)
pub async fn decode_images(mesh: &Mesh) -> Vec<Option<web_sys::ImageBitmap>> {
    let mut out = Vec::new();

    for (bytes, mime) in &mesh.images {
        out.push(decode_image(bytes, mime).await.ok());
    }

    out
}

async fn decode_image(bytes: &[u8], mime: &str) -> Result<web_sys::ImageBitmap, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();

    options.set_type(mime);

    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)?;
    let bitmap_options = web_sys::ImageBitmapOptions::new();

    bitmap_options.set_premultiply_alpha(web_sys::PremultiplyAlpha::None);

    let bitmap =
        JsFuture::from(window.create_image_bitmap_with_blob_and_image_bitmap_options(&blob, &bitmap_options)?).await?;

    bitmap.dyn_into()
}

/// Fetches a file; `priority`: the Fetch Priority (`"high"` for the root tileset and the first tiles of a frame,
/// `"low"` for the rest: a tileset server can be slow, ~15 s per cold file in Lille, and must not hold the basemap
/// back)
pub async fn fetch_bytes(
    url: &str,
    signal: Option<&web_sys::AbortSignal>,
    reload: bool,
    priority: &str,
) -> Result<Vec<u8>, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();

    init.set_signal(signal);
    crate::js::fetch_priority(&init, priority);
    crate::js::request_headers(&init, url, &[])?;
    if reload {
        init.set_cache(web_sys::RequestCache::Reload);
    }

    let response: web_sys::Response = JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await?
        .dyn_into()?;

    if !response.ok() {
        return Err(JsValue::from(response.status()));
    }

    let buffer = JsFuture::from(response.array_buffer()?).await?;

    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// Fetches `length` bytes of a file from `start` (an HTTP range request, as point clouds are read). A server that
/// ignores the range (200 for more than asked) is an error, its body not read: a whole point cloud is gigabytes
pub async fn fetch_range(url: &str, start: u64, length: u64, priority: &str) -> Result<Vec<u8>, String> {
    let error = |e: JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let controller = web_sys::AbortController::new().map_err(error)?;
    let init = web_sys::RequestInit::new();

    init.set_signal(Some(&controller.signal()));
    crate::js::fetch_priority(&init, priority);

    let end = start.saturating_add(length.max(1) - 1);

    crate::js::request_headers(&init, url, &[("Range", &format!("bytes={start}-{end}"))]).map_err(error)?;

    let response: web_sys::Response = JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await
        .map_err(error)?
        .dyn_into()
        .map_err(error)?;

    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    if response.status() != 206 {
        let size = response.headers().get("content-length").ok().flatten();

        if size.and_then(|s| s.parse::<u64>().ok()).is_none_or(|s| s > length) {
            controller.abort();

            return Err("the server ignores range requests (HTTP 206 expected)".into());
        }
    }

    let buffer = JsFuture::from(response.array_buffer().map_err(error)?)
        .await
        .map_err(error)?;
    let bytes = js_sys::Uint8Array::new(&buffer);
    // A whole small file (200) or the range: its part from `start`
    let offset = if response.status() == 206 {
        0
    } else {
        start.min(u64::from(bytes.length())) as u32
    };
    let end = (u64::from(offset) + length).min(u64::from(bytes.length())) as u32;

    Ok(bytes.subarray(offset, end).to_vec())
}
