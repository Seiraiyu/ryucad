//! JSON-RPC 2.0 framing, the same for every product: newline-delimited
//! request and response lines, every error carried as `RPC_ERROR` with the
//! product's error in `data`, and an answer's raw bytes right after its
//! response line.

use ryu_doc::error::{Code, Error, Result};
use serde_json::{Value, json};

/// What a request answers: its JSON result and, for some requests, raw
/// bytes sent right after the response line. A result with bytes says so
/// in `binary: {encoding, length}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub result: Value,
    pub binary: Option<Vec<u8>>,
}

impl Answer {
    pub fn json(result: Value) -> Answer {
        Answer {
            result,
            binary: None,
        }
    }

    pub fn bytes(mut result: Value, encoding: &str, bytes: Vec<u8>) -> Answer {
        result["binary"] = json!({"encoding": encoding, "length": bytes.len()});
        Answer {
            result,
            binary: Some(bytes),
        }
    }

    /// How many bytes follow a response line with this result.
    pub fn announced(result: &Value) -> Option<usize> {
        result
            .pointer("/binary/length")?
            .as_u64()
            .map(|n| n as usize)
    }
}

/// The JSON-RPC error code that carries every ryu error; the error's code
/// and details are in `data`.
pub const RPC_ERROR: i64 = -32000;

/// One JSON-RPC request line: `params` is left out when null.
pub fn request_line(id: u64, method: &str, params: Value) -> String {
    let mut v = json!({"jsonrpc": "2.0", "id": id, "method": method});
    if !params.is_null() {
        v["params"] = params;
    }
    v.to_string()
}

/// Parses one JSON-RPC request line into its id, and its method and
/// params for the product to read.
pub fn parse_request_line(line: &str) -> (Value, Result<(String, Option<Value>)>) {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return (
                Value::Null,
                Err(Error::new(Code::InvalidJson, format!("bad request: {e}"))),
            );
        }
    };
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let method = match v.get("method").and_then(Value::as_str) {
        Some(m) if v.get("jsonrpc") == Some(&json!("2.0")) => m.to_owned(),
        _ => {
            let e = Error::new(Code::Usage, "not a JSON-RPC 2.0 request");
            return (id, Err(e));
        }
    };
    (id, Ok((method, v.get("params").cloned())))
}

/// One JSON-RPC response line.
pub fn rpc_response(id: Value, result: &Result<Value>) -> String {
    match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": RPC_ERROR, "message": e.message, "data": e},
        }),
    }
    .to_string()
}

/// Parses one JSON-RPC response line back into a result.
pub fn parse_rpc_response(line: &str) -> Result<Value> {
    let bad = |why: &str| Error::new(Code::Daemon, format!("bad response from the daemon: {why}"));
    let mut v: Value = serde_json::from_str(line).map_err(|e| bad(&e.to_string()))?;
    if let Some(r) = v.get_mut("result") {
        return Ok(r.take());
    }
    let data = v
        .pointer_mut("/error/data")
        .map(Value::take)
        .ok_or_else(|| bad("no result or error"))?;
    Err(serde_json::from_value(data).map_err(|e| bad(&e.to_string()))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_leave_out_null_params() {
        let v: Value =
            serde_json::from_str(&request_line(3, "daemon.status", Value::Null)).unwrap();
        assert_eq!(
            v,
            json!({"jsonrpc": "2.0", "id": 3, "method": "daemon.status"})
        );
        let line = request_line(4, "a.b", json!({"x": 1}));
        let (id, call) = parse_request_line(&line);
        assert_eq!(id, json!(4));
        assert_eq!(call.unwrap(), ("a.b".to_owned(), Some(json!({"x": 1}))));
    }

    #[test]
    fn bad_request_lines() {
        let (id, call) = parse_request_line("{");
        assert_eq!(
            (id, call.unwrap_err().code),
            (Value::Null, Code::InvalidJson)
        );
        let (id, call) = parse_request_line(r#"{"id": 1, "method": "a"}"#);
        assert_eq!((id, call.unwrap_err().code), (json!(1), Code::Usage));
    }

    #[test]
    fn errors_travel_in_data() {
        let e = Error::new(Code::NotFound, "no such thing").with("part", "p");
        let line = rpc_response(json!(7), &Err(e.clone()));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["error"]["code"], RPC_ERROR);
        assert_eq!(parse_rpc_response(&line).unwrap_err(), e);
        let line = rpc_response(json!(8), &Ok(json!({"ok": 1})));
        assert_eq!(parse_rpc_response(&line).unwrap(), json!({"ok": 1}));
    }

    #[test]
    fn answers_announce_their_bytes() {
        let a = Answer::bytes(json!({}), "ryucad.mesh.1", vec![1, 2, 3]);
        assert_eq!(Answer::announced(&a.result), Some(3));
        assert_eq!(Answer::announced(&Answer::json(json!({})).result), None);
    }
}
