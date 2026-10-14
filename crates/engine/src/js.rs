//! Building JavaScript values returned by the engine (their TypeScript types are declared next to the exports)

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
