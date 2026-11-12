//! The unit tests' harness: a fresh project and its session, a request
//! by method name (in process or as a daemon's participant), and the
//! fixtures many modules' tests share. It was `server::tests`' until
//! ryucad 27.B, which re-exports it there while ryucad 27 runs.

use crate::doc::files::PartFile;
use crate::doc::id::Id;
use crate::doc::project::write_json;
use crate::error::Result;
use crate::live::Live;
use crate::proto::Request;
use crate::server::{handle, handle_live, handle_safely, init};
use crate::session::Session;
use serde_json::{Value, json};

thread_local! {
    /// A render budget for this thread's renders in place of
    /// `session.max_memory`'s quarter (#400's test).
    pub(crate) static RENDER_BUDGET: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// A fresh project and its session.
pub(crate) fn fresh() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path(), None).unwrap();
    let s = Session::open(dir.path()).unwrap();
    (dir, s)
}

pub(crate) fn call(s: &mut Session, method: &str, params: Value) -> Result<Value> {
    handle(s, Request::from_rpc(method, Some(params))?)
}

/// The pixels of a render of `s`'s project, RGB.
pub(crate) fn render_pixels(dir: &std::path::Path, s: Session, params: Value) -> Vec<[u8; 3]> {
    let req = Request::from_rpc("render", Some(params)).unwrap();
    let a = handle_safely(&mut Some(s), None, None, dir, req).unwrap();
    let png = a.binary.unwrap();
    let mut r = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    buf[..info.buffer_size()]
        .chunks(4)
        .map(|p| [p[0], p[1], p[2]])
        .collect()
}

/// The box `b` of part `p` with a datum plane `d1` 3 mm above its
/// top, an axis `a1` along y and a point `q1` at (1, 2, 3) mm, and the
/// top's reference.
pub(crate) fn with_datums() -> (tempfile::TempDir, Session, String) {
    let (dir, mut s, body) = crate::sketch::tests::with_box();
    let top = format!(
        "p/b/{}",
        crate::sketch::tests::face_along(&body, [0.0, 0.0, 1.0])
    );
    for f in [
        json!({"type": "plane", "id": "d1", "by": "offset", "from": top, "distance": 3}),
        json!({"type": "axis", "id": "a1", "by": "of", "axis": "y"}),
        json!({"type": "point", "id": "q1", "by": "coordinates", "coordinates": [1, 2, 3]}),
    ] {
        let mut f = f;
        f["part"] = json!("p");
        call(&mut s, "feature.add", f).unwrap();
    }
    (dir, s, top)
}

/// Part `p` with its box, and a helix `h` about z (radius 8 mm, 10
/// turns of 2 mm) and a path `w` along it.
pub(crate) fn with_helix() -> (tempfile::TempDir, Session) {
    let (dir, mut s, _) = crate::sketch::tests::with_box();
    for f in [
        json!({"type": "helix", "id": "h", "axis": "z", "radius": 8, "pitch": 2,
               "turns": 10}),
        json!({"type": "path", "id": "w", "segments": ["h"]}),
    ] {
        let mut f = f;
        f["part"] = json!("p");
        call(&mut s, "feature.add", f).unwrap();
    }
    (dir, s)
}

pub(crate) fn with_part() -> (tempfile::TempDir, Session) {
    let (dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    (dir, s)
}

/// Part `p` with parameter `width` and the rectangle sketch `s`,
/// written as a file.
pub(crate) fn with_sketch() -> (tempfile::TempDir, Session) {
    let (dir, mut s) = with_part();
    call(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"width": "20 mm"}}),
    )
    .unwrap();
    let root = dir.path();
    write_json(
        &root.join("parts/p/features/s.json"),
        &crate::doc::fixtures::rectangle(),
    )
    .unwrap();
    write_json(
        &root.join("parts/p/part.json"),
        &PartFile::new(vec![Id::parse("s", "feature").unwrap()]).to_json(),
    )
    .unwrap();
    (dir, s)
}

/// `call` with the daemon's live session, as participant `me`.
pub(crate) fn call_live(
    s: &mut Session,
    live: &mut Live,
    me: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value> {
    handle_live(s, Some(live), me, Request::from_rpc(method, Some(params))?)
}

/// The notifications queued for `sub` so far, as (method, params).
pub(crate) fn drain(sub: &crate::notify::Subscriber) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    // `next` blocks when empty, so close a clone's view by sending a
    // marker first.
    loop {
        let line = sub.next().unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["method"] == "marker" {
            return out;
        }
        out.push((
            v["method"].as_str().unwrap().to_owned(),
            v["params"].clone(),
        ));
    }
}
