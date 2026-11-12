//! `lib check`'s report is written as text, never held as a `Value` tree
//! (#711): over a generated library of 2,000 symbols with findings on
//! every pin, the bytes the check holds at its peak, counted by a global
//! allocator, are within a small multiple of its JSON, where the same
//! report as a `Value` holds many times it; and the text reads as the
//! report it always was.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use ryu_session::rpc::Raw;
use serde_json::{Value, json};

/// Counts the bytes allocated now, and the most since the last reset.
struct Counting;

static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let n = NOW.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(n, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        NOW.fetch_sub(l.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        // A move holds both blocks for a moment: count them both.
        let q = unsafe { System.alloc(Layout::from_size_align_unchecked(new, l.align())) };
        if !q.is_null() {
            let n = NOW.fetch_add(new, Ordering::Relaxed) + new;
            PEAK.fetch_max(n, Ordering::Relaxed);
            unsafe {
                std::ptr::copy_nonoverlapping(p, q, l.size().min(new));
                self.dealloc(p, l);
            }
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Runs `f`, giving its result and the most bytes it held above what was
/// held before it.
fn held<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = NOW.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let t = f();
    (t, PEAK.load(Ordering::SeqCst).saturating_sub(base))
}

fn put(dir: &Path, rel: &str, kind: &str, mut v: Value) {
    v["kind"] = json!(kind);
    v["format"] = json!(1);
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, ryueda::canonical::to_string(&v)).unwrap();
}

/// A project whose own library holds `n` symbols of 24 pins each, every
/// pin off the grid and the inputs on the right.
fn library(dir: &Path, n: usize) {
    put(
        dir,
        "ryueda.json",
        "ryueda.project",
        json!({"project": "0123456789abcdef0123456789abcdef", "default_unit": "mm"}),
    );
    for s in 0..n {
        let mut pins = serde_json::Map::new();
        for i in 0..24i64 {
            pins.insert(
                (i + 1).to_string(),
                json!({"name": format!("IN{i}"), "type": "input",
                       "at": [7_620_001, 30_480_000 - i * 2_540_000 + 7], "side": "left",
                       "length_nm": 2_540_000}),
            );
        }
        put(
            dir,
            &format!("library/symbols/s{s:04}.json"),
            "ryueda.symbol",
            json!({"name": format!("S{s}"), "units": 1, "pins": pins}),
        );
    }
}

#[test]
fn a_library_checks_report_is_held_as_text() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path(), 2_000);
    let p = ryueda::project::Project::load(dir.path()).unwrap();
    let listing = ryueda::symlint::Listing::default();
    let (answer, peak) = held(|| ryueda::symlint::library_check(&p, None, None, &listing).unwrap());
    let text = match answer.raw {
        Some(Raw::Result(t)) => t,
        Some(Raw::Error { details, .. }) => details,
        None => panic!("a report as text"),
    };
    let (tree, tree_peak) = held(|| serde_json::from_str::<Value>(&text).unwrap());
    let report = tree.get("report").unwrap_or(&tree);
    let findings: usize = report["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["findings"].as_array().unwrap().len())
        .sum();
    println!(
        "{} bytes of report, {findings} findings: {peak} bytes held at the check's peak, {tree_peak} as a Value",
        text.len()
    );
    assert!(findings >= 2_000 * 24, "{findings}");
    assert!(
        peak < 3 * text.len(),
        "{peak} held for {} bytes",
        text.len()
    );
    assert!(
        tree_peak > 4 * text.len(),
        "{tree_peak}: the tree this avoids"
    );
}
