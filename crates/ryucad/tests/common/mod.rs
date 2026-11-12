//! Shared by ryucad's integration tests: requests answered by a session
//! in this process, and a project to send them to.

#![allow(dead_code)]

use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};

pub fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
}

pub fn must(s: &mut Session, method: &str, params: Value) -> Value {
    call(s, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// A project with an empty part `p`, in millimetres (`init`'s default).
pub fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    (dir, s)
}
