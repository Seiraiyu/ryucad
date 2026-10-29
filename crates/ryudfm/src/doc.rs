//! Reading and writing ryudfm's files: canonical JSON with a `kind` and a
//! `format`, the rest of each file the shape its module's types give it.

use crate::canonical;
use crate::error::{Code, Error, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// Reads the file at `path` (relative to the project) as a `kind` of at
/// most format `newest`.
pub fn read<T: DeserializeOwned>(bytes: &[u8], path: &str, kind: &str, newest: u64) -> Result<T> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::new(Code::InvalidJson, format!("{path} is not UTF-8")))
        .map_err(|e| e.with("path", path))?;
    let v = canonical::parse(text).map_err(|e| e.with("path", path))?;
    from_value(v, path, kind, newest)
}

/// [`read`], from JSON already parsed.
pub fn from_value<T: DeserializeOwned>(v: Value, path: &str, kind: &str, newest: u64) -> Result<T> {
    let invalid = |msg: String| {
        Error::new(Code::InvalidDocument, format!("{path}: {msg}")).with("path", path)
    };
    let Value::Object(mut o) = v else {
        return Err(invalid("a file is a JSON object".into()));
    };
    match o.remove("kind") {
        Some(Value::String(k)) if k == kind => {}
        k => {
            return Err(invalid(format!(
                "expected kind {kind:?}, found {}",
                k.unwrap_or(Value::Null)
            )));
        }
    }
    let format = o
        .remove("format")
        .and_then(|f| f.as_u64())
        .filter(|f| *f >= 1)
        .ok_or_else(|| invalid("format must be a positive integer".into()))?;
    if format > newest {
        return Err(Error::new(
            Code::FormatTooNew,
            format!("{path} has format {format}; this ryudfm reads up to {newest}. Upgrade ryudfm"),
        )
        .with("path", path)
        .with("format", format)
        .with("supported", newest));
    }
    serde_json::from_value(Value::Object(o)).map_err(|e| invalid(e.to_string()))
}

/// The canonical text of `t` as a `kind` of `format`.
pub fn to_string<T: Serialize>(t: &T, kind: &str, format: u64) -> String {
    canonical::to_string(&to_value(t, kind, format))
}

/// `t`'s JSON with its `kind` and `format`.
pub fn to_value<T: Serialize>(t: &T, kind: &str, format: u64) -> Value {
    let mut o = match serde_json::to_value(t).expect("file types serialize") {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    o.insert("kind".into(), Value::from(kind));
    o.insert("format".into(), Value::from(format));
    Value::Object(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct T {
        name: String,
    }

    #[test]
    fn headers_are_checked() {
        let t = T { name: "x".into() };
        let text = to_string(&t, "ryudfm.t", 1);
        assert_eq!(
            text,
            "{\n  \"format\": 1,\n  \"kind\": \"ryudfm.t\",\n  \"name\": \"x\"\n}\n"
        );
        assert_eq!(
            read::<T>(text.as_bytes(), "t.json", "ryudfm.t", 1).unwrap(),
            t
        );
        let e = read::<T>(text.as_bytes(), "t.json", "ryudfm.u", 1).unwrap_err();
        assert_eq!(
            (e.code, &e.details["path"]),
            (Code::InvalidDocument, &json!("t.json"))
        );
        let newer = to_string(&t, "ryudfm.t", 2);
        let e = read::<T>(newer.as_bytes(), "t.json", "ryudfm.t", 1).unwrap_err();
        assert_eq!(e.code, Code::FormatTooNew);
        let extra = br#"{"format": 1, "kind": "ryudfm.t", "name": "x", "colour": "red"}"#;
        let e = read::<T>(extra, "t.json", "ryudfm.t", 1).unwrap_err();
        assert!(e.message.contains("colour"), "{}", e.message);
        assert_eq!(
            read::<T>(b"{", "t.json", "ryudfm.t", 1).unwrap_err().code,
            Code::InvalidJson
        );
    }
}
