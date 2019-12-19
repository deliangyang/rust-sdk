use delegate::delegate;
use serde_json::{map::Map, value::Index, Result, Value};
use std::default::Default;

#[derive(Debug, Clone)]
pub struct UploadResponse(pub(super) Value);

impl UploadResponse {
    pub fn key(&self) -> Option<&str> {
        self.get("key").and_then(|k| k.as_str())
    }

    pub fn hash(&self) -> Option<&str> {
        self.get("hash").and_then(|k| k.as_str())
    }

    pub fn into_value(self) -> Value {
        self.0
    }

    pub fn to_string(&self) -> Result<String> {
        serde_json::to_string(&self.0)
    }

    delegate! {
        target self.0 {
            pub fn get<I: Index>(&self, index:I) -> Option<&Value>;
            pub fn is_object(&self) -> bool;
            pub fn as_object(&self) -> Option<&Map<String,Value>>;
            pub fn is_array(&self) -> bool;
            pub fn as_array(&self) -> Option<&Vec<Value>>;
            pub fn is_string(&self) -> bool;
            pub fn as_str(&self) -> Option<&str>;
            pub fn is_number(&self) -> bool;
            pub fn is_i64(&self) -> bool;
            pub fn is_u64(&self) -> bool;
            pub fn is_f64(&self) -> bool;
            pub fn as_i64(&self) -> Option<i64>;
            pub fn as_u64(&self) -> Option<u64>;
            pub fn as_f64(&self) -> Option<f64>;
            pub fn is_boolean(&self) -> bool;
            pub fn as_bool(&self) -> Option<bool>;
            pub fn is_null(&self) -> bool;
            pub fn as_null(&self) -> Option<()>;
        }
    }
}

impl Default for UploadResponse {
    fn default() -> Self {
        UploadResponse(Default::default())
    }
}

impl From<Value> for UploadResponse {
    fn from(v: Value) -> Self {
        UploadResponse(v)
    }
}
