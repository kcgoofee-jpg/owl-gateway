//! Standard UTF-8 JSON encoding and decoding.
use serde::Serialize;
use serde_json::Value;
pub fn dumps<T: Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).expect("JSON-compatible value")
}
pub fn loads(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}
pub fn wire<T: Serialize + ?Sized>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).expect("JSON-compatible value")
}
