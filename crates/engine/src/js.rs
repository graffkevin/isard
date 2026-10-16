//! Building JavaScript values returned by the engine (their TypeScript types are declared next to the exports), and
//! request options web-sys does not expose yet, a timer

use js_sys::{Array, Object, Reflect};
use wasm_bindgen::JsValue;

/// A plain object from key-value pairs
pub fn object(entries: &[(&str, JsValue)]) -> JsValue {
    let out = Object::new();
    for (key, value) in entries {
        // Setting a property on a fresh plain object cannot fail
        let _ = Reflect::set(&out, &JsValue::from_str(key), value);
    }
    out.into()
}

/// A two-number tuple
pub fn pair(a: f64, b: f64) -> JsValue {
    Array::of2(&a.into(), &b.into()).into()
}

/// Sets the Fetch Priority of a request (`"high"`, `"low"`, `"auto"`). Besides ordering, an explicit priority keeps
/// Firefox from holding the request back: by default it throttles plain fetches for as long as another request of the
/// page is pending, ~10 s per slow server response (`network.http.throttle.enable`)
pub fn fetch_priority(init: &web_sys::RequestInit, priority: &str) {
    // Setting a property on a plain options object cannot fail; browsers without Fetch Priority ignore it
    let _ = Reflect::set(init, &JsValue::from_str("priority"), &JsValue::from_str(priority));
}

/// Resolves after `ms` milliseconds
pub async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
