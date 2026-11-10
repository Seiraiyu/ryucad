//! Drawings from bytes (design 417, plan 9.D), over a project made once:
//! part `p`, a 40 × 30 × 10 mm box `b` drilled through by a ⌀8 cylinder
//! `h` and its vertical edges filleted (`f`), and drawing `d` of it with
//! the standard views and a dimension `d1` of its length.
//!
//! An even first byte reads the rest as a drawing file: one that doesn't
//! read is a typed error; one that reads writes back canonically and
//! reads back equal, and written as drawing `z` builds (`drawing.get` with
//! every item) to a typed refusal or a drawing that builds the same again
//! and exports as PDF, SVG and DXF and renders as PNG. An odd first byte
//! reads the rest as `drawing.edit`'s ops on `d`: refused by a typed
//! error, or the file written is canonical, reads back, and builds and
//! exports likewise. Never an internal error; any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::doc::canonical;
use ryucad::doc::drawing::DrawingFile;
use ryucad::error::Code;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params))?)
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

/// A folder of its own under the system's temporary folder.
fn scratch(what: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "ryucad-fuzz-drawing-{}-{what}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The project every input starts from, made once.
fn template() -> &'static Path {
    static T: OnceLock<PathBuf> = OnceLock::new();
    T.get_or_init(|| {
        let root = scratch("template");
        std::fs::create_dir_all(&root).unwrap();
        ryucad::server::init(&root, None).unwrap();
        let mut s = Session::open(&root).unwrap();
        let mut must = |m: &str, p: Value| call(&mut s, m, p).unwrap();
        must("part.create", json!({"part": "p"}));
        must(
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 10}}),
        );
        must(
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "h", "operation": "cut", "targets": ["b"],
                   "params": {"radius": 4, "height": 10},
                   "placement": {"origin": [20, 15, 0]}}),
        );
        must(
            "drawing.create",
            json!({"part": "p", "drawing": "d", "name": "Fuzzed", "views": "standard"}),
        );
        root
    })
}

/// Builds drawing `id` of `p`: a typed refusal, or the same build twice
/// and every format written.
fn built(s: &mut Session, id: &str, out: &Path) {
    let get = |s: &mut Session| {
        call(s, "drawing.get", json!({"part": "p", "drawing": id, "items": true}))
    };
    let first = match get(s) {
        Ok(v) => v,
        Err(e) => {
            assert_ne!(e.code, Code::Internal, "{e}");
            return;
        }
    };
    assert_eq!(get(s).unwrap(), first, "two builds differ");
    for (format, file) in [("pdf", "d.pdf"), ("svg", "d.svg"), ("dxf", "d.dxf")] {
        let f = out.join(file);
        match call(
            s,
            "drawing.export",
            json!({"part": "p", "drawing": id, "format": format, "out": f.to_str().unwrap()}),
        ) {
            Ok(_) => assert!(std::fs::metadata(&f).unwrap().len() > 0, "{format} is empty"),
            Err(e) => assert_ne!(e.code, Code::Internal, "{format}: {e}"),
        }
    }
    if let Err(e) = call(s, "drawing.render", json!({"part": "p", "drawing": id, "scale": 1.0})) {
        assert_ne!(e.code, Code::Internal, "png: {e}");
    }
}

fn file_text(root: &Path, id: &str) -> String {
    std::fs::read_to_string(root.join(format!("parts/p/drawings/{id}.json"))).unwrap()
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let root = scratch("project");
    let out = scratch("out");
    copy(template(), &root);
    std::fs::create_dir_all(&out).unwrap();
    if mode % 2 == 0 {
        // A drawing file: read, written back canonically and read again.
        if let Ok(f) = DrawingFile::from_json(&v) {
            let written = f.to_json();
            let again = DrawingFile::from_json(&written).expect("its own file reads back");
            assert_eq!(again.to_json(), written, "a file reads back otherwise");
            let path = root.join("parts/p/drawings/z.json");
            ryucad::doc::project::write_json(&path, &written).unwrap();
            let mut s = Session::open(&root).unwrap();
            built(&mut s, "z", &out);
        }
    } else {
        let mut s = Session::open(&root).unwrap();
        match call(
            &mut s,
            "drawing.edit",
            json!({"part": "p", "drawing": "d", "ops": v}),
        ) {
            Ok(_) => {
                let text = file_text(&root, "d");
                let read: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(text, canonical::to_string(&read), "an edit wrote a file not canonical");
                let f = DrawingFile::from_json(&read).expect("an edit's file reads back");
                assert_eq!(f.to_json(), read);
                built(&mut s, "d", &out);
            }
            Err(e) => {
                assert_ne!(e.code, Code::Internal, "{e}");
                // Refused: nothing written.
                assert_eq!(file_text(&root, "d"), file_text(template(), "d"));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&out);
});
