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

/// What code outside `server/` may name of `crate::server`: the request
/// lifecycle's entry points. The daemon, the CLI, a worktree's requests
/// and the requests annotations and redlines replay run through them, and
/// unit tests start projects with `init` and `handle`. Everything else
/// `server` used to lend lives in `part`, `save`, `geometry`, `budget`,
/// `scene`, `diff` and `testing` (design 541, "The server split").
const SERVER_ENTRY: &[&str] = &[
    "answer_live",
    "dispatch",
    "handle",
    "handle_job",
    "handle_job_raw",
    "handle_live",
    "handle_safely",
    "init",
    "mode_refusal",
    "prepare",
    "prepare_job",
    "publish",
    "settle",
    "tick",
];

/// The names `code` takes from the server module: after `crate::server::`
/// or `ryucad::server::`, and after a bare `server::` (a module imported
/// as `server`), but not another crate's `…::server::`. Comment lines
/// don't count.
fn server_names(code: &str) -> Vec<String> {
    let code = code
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = code[at..].find("server::") {
        let start = at + i;
        at = start + "server::".len();
        let before = &code[..start];
        let ours = before.ends_with("crate::") || before.ends_with("ryucad::");
        if (before.ends_with("::") && !ours) || before.ends_with(word) {
            continue;
        }
        let rest = &code[at..];
        let names: Vec<&str> = match rest.strip_prefix('{') {
            Some(group) => group[..group.find('}').unwrap_or(group.len())]
                .split(',')
                .map(|n| n.split("::").next().unwrap_or("").split(" as ").next().unwrap_or(""))
                .map(str::trim)
                .collect(),
            None => vec![&rest[..rest.find(|c: char| !word(c)).unwrap_or(rest.len())]],
        };
        out.extend(
            names
                .into_iter()
                .filter(|n| !n.is_empty() && *n != "self")
                .map(str::to_owned),
        );
    }
    out
}

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn the_scan_reads_paths_groups_and_bare_imports() {
    // Spliced, so this file's own text names nothing.
    let s = "server";
    let code = format!(
        "use crate::{s}::{{handle, part_id as id}};\n\
         let x = crate::{s}::tests::call;\n\
         // crate::{s}::save\n\
         use ryu_session::{s}::Serve;\n\
         use ryucad::{{daemon, {s}}};\n\
         {s}::init(p);\n\
         let a = ryucad::{s}::meshes;\n"
    );
    assert_eq!(server_names(&code), ["handle", "part_id", "tests", "init", "meshes"]);
}

/// Design 541: the dependency points one way. Nothing outside `server/`
/// names more of it than its entry points; a helper it used to hold is
/// named at its new home (`crate::part::part_id`, `crate::save::save`,
/// `crate::scene::render_scene`, `crate::testing::call`, …).
#[test]
fn nothing_outside_the_server_names_more_than_its_entry_points() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();
    let mut found = Vec::new();
    for f in files.iter().filter(|f| !f.starts_with(src.join("server"))) {
        let text = std::fs::read_to_string(f).unwrap();
        for n in server_names(&text) {
            if !SERVER_ENTRY.contains(&n.as_str()) {
                let at = f.strip_prefix(&src).unwrap().display();
                found.push(format!("{at}: server::{n}"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "outside server/, name these at their homes (crate::part, save, geometry, budget, \
         scene, diff; the tests' harness is crate::testing): {found:#?}"
    );
}
