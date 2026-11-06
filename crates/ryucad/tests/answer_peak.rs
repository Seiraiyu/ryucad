//! A whole project's rebuild answer is written as text, never held as a
//! `Value` tree (#533): over a generated 1,000-part project, the bytes the
//! answer holds at its peak, counted by a global allocator, are within a
//! small multiple of its JSON, and its response line is the line the
//! `Value` gives. Release only (the project's first rebuild takes minutes
//! in a debug build).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use ryu_session::job::Job;
use ryu_session::rpc::write_raw_response;
use ryucad::bench::scale::{Spec, write};
use ryucad::proto::{Raw, Request, rpc_response};
use ryucad::session::Session;
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

fn mib(n: usize) -> f64 {
    n as f64 / (1 << 20) as f64
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the first rebuild takes minutes"
)]
fn a_whole_projects_rebuild_is_never_a_value_tree() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: this is the binary's one test, so no other thread reads the
    // environment.
    unsafe {
        std::env::set_var("RYUCAD_CONFIG_DIR", dir.path().join("config"));
        std::env::set_var("RYUCAD_CACHE_DIR", dir.path().join("cache"));
        std::env::set_var("RYUCAD_RUNTIME_DIR", dir.path().join("run"));
    }
    let root = dir.path().join("p");
    let spec = Spec {
        races: false,
        ..Spec::parts(1000)
    };
    let summary = write(&root, &spec, &[]);
    let mut slot = Some(Session::open(&root).unwrap());
    let mut ask = || {
        let req = Request::from_rpc("rebuild", Some(json!({}))).unwrap();
        ryucad::server::handle_job_raw(&mut slot, None, None, &root, req, &Job::default())
    };
    // Everything built once; the second answer builds nothing, so what
    // it holds is the answer's.
    drop(ask());
    let id = json!(1);
    let (text, raw) = held(|| {
        let a = ask().unwrap();
        let raw = a.raw.expect("a rebuild of the project answers as text");
        write_raw_response(&mut std::io::sink(), &id, &raw).unwrap();
        let Raw::Result(text) = raw else {
            panic!("the project rebuilds: {raw:?}")
        };
        text
    });
    // What answering through a `Value` held: the tree, then the line.
    let ((v, value_line), tree) = held(|| {
        let v: Value = serde_json::from_str(&text).unwrap();
        let line = rpc_response(id.clone(), &Ok(v.clone()));
        (v, line)
    });
    let len = text.len();
    println!(
        "{} parts: the answer {:.1} MiB; held {:.1} MiB as text ({:.1}x), {:.1} MiB through a \
         Value ({:.1}x)",
        summary.parts,
        mib(len),
        mib(raw),
        raw as f64 / len as f64,
        mib(tree),
        tree as f64 / len as f64,
    );
    // The bytes the client reads are the `Value`'s.
    let mut out = Vec::new();
    write_raw_response(&mut out, &id, &Raw::Result(text.clone())).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), value_line);
    assert_eq!(
        v["parts"].as_array().unwrap().len(),
        summary.parts + summary.assemblies
    );
    // The text, with room for a `String`'s doubling (the old block and the
    // new one while it moves) and one part's report: never a tree's 18 times.
    assert!(raw < 4 * len, "held {raw} bytes for an answer of {len}");
}
