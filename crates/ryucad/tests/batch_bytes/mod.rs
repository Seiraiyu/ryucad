//! A batch from bytes (design 603, plan 31.C): up to eight ops from a
//! menu over a small project, run as one `batch` over one copy of it and
//! one by one over another. Both succeed and leave the same files, or
//! both refuse with the same code at the same op and the batch writes
//! nothing. Never an internal error; any panic is a bug. The fuzz target
//! `batch` runs it on libFuzzer's bytes, `tests/batch_props.rs` on
//! seeded ones.

use ryucad::error::Code;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params))?)
}

fn scratch(what: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "ryucad-batch-bytes-{}-{what}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
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

fn on_disk(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, rel: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(root.join(rel)).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let r = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if e.file_type().unwrap().is_dir() {
                walk(root, &r, out);
            } else {
                out.insert(r, std::fs::read(e.path()).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

/// Part `a` (parameter `w`, box `b`, cylinder `c`) and assembly `asm`
/// placing it as `i1`, made once.
fn template() -> &'static Path {
    static T: OnceLock<PathBuf> = OnceLock::new();
    T.get_or_init(|| {
        let root = scratch("template");
        std::fs::create_dir_all(&root).unwrap();
        ryucad::server::init(&root, None).unwrap();
        let mut s = Session::open(&root).unwrap();
        for (m, p) in [
            ("part.create", json!({"part": "a"})),
            ("param.set", json!({"part": "a", "parameters": {"w": "20 mm"}})),
            ("feature.add", json!({"part": "a", "type": "box", "id": "b", "params": {"x": "w", "y": 10, "z": 2}})),
            ("feature.add", json!({"part": "a", "type": "cylinder", "id": "c", "params": {"radius": 1, "height": 4}})),
            ("part.create", json!({"part": "asm", "kind": "assembly"})),
            ("feature.add", json!({"part": "asm", "type": "instance", "id": "i1", "of": "a"})),
        ] {
            call(&mut s, m, p).unwrap_or_else(|e| panic!("{m}: {e:?}"));
        }
        root
    })
}

/// Op `b % 12` with argument `x`.
fn op(b: u8, x: u8) -> (&'static str, Value) {
    let n = u32::from(x % 5) + 1;
    let pick = |xs: &[&'static str]| xs[usize::from(x) % xs.len()];
    match b % 12 {
        0 => ("part.create", json!({"part": pick(&["q0", "q1", "a"])})),
        1 => ("part.delete", json!({"part": pick(&["a", "q0", "asm"])})),
        2 => (
            "feature.add",
            json!({"part": "a", "type": "sphere", "id": pick(&["s0", "s1", "b"]),
                                     "params": {"radius": n}}),
        ),
        3 => (
            "feature.edit",
            json!({"part": "a", "id": "b", "params": {"x": n * 5}}),
        ),
        4 => (
            "feature.delete",
            json!({"part": "a", "id": pick(&["b", "c", "s0"])}),
        ),
        5 => (
            "param.set",
            json!({"part": "a", "parameters": {"w": format!("{n} mm")}}),
        ),
        6 => (
            "materials.set",
            json!({"name": "m", "density": format!("{n}000 kg/m^3")}),
        ),
        7 => (
            "part.ensure",
            json!({"part": "a", "params": {"w": format!("{n} mm")}, "features": [
                {"id": "b", "type": "box", "params": {"x": "w", "y": 10, "z": 2}},
                {"id": "c", "type": "cylinder", "params": {"radius": 1, "height": n}}]}),
        ),
        8 => (
            "part.ensure",
            json!({"part": pick(&["q0", "q1", "a"]), "features": [
                {"id": "c", "type": "cylinder", "params": {"radius": n, "height": 4}}]}),
        ),
        9 => (
            "part.ensure",
            json!({"part": "asm", "kind": "assembly", "features": [
                {"id": "i1", "type": "instance", "of": "a", "placement": {"origin": [n * 10, 0, 0]}}]}),
        ),
        10 => (
            "feature.add",
            json!({"part": "asm", "type": "instance", "id": pick(&["j0", "j1"]), "of": "a"}),
        ),
        _ => (
            "feature.move",
            json!({"part": "a", "id": "c", "first": x.is_multiple_of(2)}),
        ),
    }
}

pub fn check(data: &[u8]) {
    let ops: Vec<(&str, Value)> = data
        .as_chunks::<2>()
        .0
        .iter()
        .take(8)
        .map(|&[a, b]| op(a, b))
        .collect();
    if ops.is_empty() {
        return;
    }
    let (a_root, b_root) = (scratch("a"), scratch("b"));
    copy(template(), &a_root);
    copy(template(), &b_root);
    let mut a = Session::open(&a_root).unwrap();
    let mut b = Session::open(&b_root).unwrap();
    let before = on_disk(&a_root);
    let batch: Vec<Value> = ops
        .iter()
        .map(|(m, p)| json!({"method": m, "params": p}))
        .collect();
    let got = call(&mut a, "batch", json!({"ops": batch, "rebuild": false}));
    let mut alone = Ok(());
    for (i, (m, p)) in ops.iter().enumerate() {
        if let Err(e) = call(&mut b, m, p.clone()) {
            alone = Err((i, e));
            break;
        }
    }
    match (got, alone) {
        (Ok(_), Ok(())) => assert!(
            on_disk(&a_root) == on_disk(&b_root),
            "{ops:?}: the files differ"
        ),
        (Err(e), Err((i, f))) => {
            assert_ne!(e.code, Code::Internal, "{ops:?}: {e:?}");
            assert_eq!(e.code, f.code, "{ops:?}: batch {e:?}, alone {f:?}");
            assert_eq!(e.details["op"], json!(i), "{ops:?}: {e:?}");
            assert!(on_disk(&a_root) == before, "{ops:?}: a refused batch wrote");
        }
        (x, y) => panic!("{ops:?}: batch {x:?}, one by one {y:?}"),
    }
    let _ = std::fs::remove_dir_all(&a_root);
    let _ = std::fs::remove_dir_all(&b_root);
}
