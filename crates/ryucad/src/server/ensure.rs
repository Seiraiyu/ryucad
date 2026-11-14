//! `part.ensure` (design 603, plan 31.C): a part made exactly as a
//! script declares it, writing only what differs, with what became of
//! the part, each feature, its parameters and its properties.

use crate::error::{Code, Error, Result};
use crate::live::Live;
use crate::session::Session;
use serde_json::{Map, Value, json};

/// What `part.ensure` is given.
#[derive(Clone, Debug)]
pub(crate) struct Ask {
    pub part: String,
    pub kind: Option<String>,
    pub params: Option<Map<String, Value>>,
    pub features: Vec<Value>,
    pub props: Option<Map<String, Value>>,
    pub expected: Option<String>,
}

impl Ask {
    /// The request's params again, as a batch's op holds them.
    pub(crate) fn to_params(&self) -> Value {
        let mut v = json!({"part": self.part, "features": self.features});
        if let Some(k) = &self.kind {
            v["kind"] = json!(k);
        }
        if let Some(p) = &self.params {
            v["params"] = json!(p);
        }
        if let Some(p) = &self.props {
            v["props"] = json!(p);
        }
        if let Some(r) = &self.expected {
            v["expected_revision"] = json!(r);
        }
        v
    }
}

/// `part.ensure`.
pub(crate) fn run(
    _session: &mut Session,
    _live: Option<&mut Live>,
    _me: Option<&str>,
    _ask: Ask,
) -> Result<Value> {
    // Tasks 5-9 fill this in.
    Err(Error::new(Code::Usage, "part.ensure isn't answered yet"))
}
