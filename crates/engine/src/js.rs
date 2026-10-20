//! Building JavaScript values returned by the engine (their TypeScript types are declared next to the exports), and
//! request options web-sys does not expose yet, a timer, a clock

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

/// A three-number tuple
pub fn triple(a: f64, b: f64, c: f64) -> JsValue {
    Array::of3(&a.into(), &b.into(), &c.into()).into()
}

/// Headers added to the requests whose URL starts with a prefix: the prefix and its name and value pairs
pub type HeaderRule = (String, Vec<(String, String)>);

thread_local! {
    /// Headers added to the requests by URL prefix (`setRequestHeaders`), the shortest prefix first
    static REQUEST_HEADERS: std::cell::RefCell<Vec<HeaderRule>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Replaces the headers added to the requests by URL prefix
pub fn set_request_headers(rules: Vec<HeaderRule>) {
    REQUEST_HEADERS.with(|r| *r.borrow_mut() = rules);
}

/// Sets on `init` the headers of the rules matching `url`, with `extra` (a range); nothing when none applies
pub fn request_headers(init: &web_sys::RequestInit, url: &str, extra: &[(&str, &str)]) -> Result<(), JsValue> {
    let matching: Vec<(String, String)> = REQUEST_HEADERS.with(|r| {
        r.borrow()
            .iter()
            .filter(|(prefix, _)| url.starts_with(prefix.as_str()))
            .flat_map(|(_, headers)| headers.iter().cloned())
            .collect()
    });
    if matching.is_empty() && extra.is_empty() {
        return Ok(());
    }
    let headers = web_sys::Headers::new()?;
    for (name, value) in &matching {
        headers.set(name, value)?;
    }
    for (name, value) in extra {
        headers.set(name, value)?;
    }
    init.set_headers(&headers);
    Ok(())
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

thread_local! {
    /// The page's high-resolution clock, looked up once
    static PERFORMANCE: Option<web_sys::Performance> = web_sys::window().and_then(|w| w.performance());
}

/// Milliseconds on the page's monotonic, sub-millisecond clock (`performance.now()`), for time budgets within a frame:
/// `Date.now()` steps by whole milliseconds and follows the wall clock's adjustments
pub fn now() -> f64 {
    PERFORMANCE.with(|p| p.as_ref().map_or_else(js_sys::Date::now, web_sys::Performance::now))
}
