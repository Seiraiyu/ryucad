//! #400: the window's open never takes its daemon past its memory budget.
//! A window over design 192's benchmark project at 1,000 parts (10,000
//! occurrences, no race bodies), against a real daemon given the least
//! `session.max_memory`; once it is idle, the thumbnail's own request is
//! sent as a window before #400's fix sent it (the daemon must refuse it,
//! `render_too_large`, rather than mesh every occurrence). The daemon's
//! peak resident memory (`VmHWM`) is read from `/proc`: Linux, release
//! builds only.
//!
//! Measured on cox-worker-2 (AMD Ryzen 7 9700X, 8 cores, 32 GB, load 9 to
//! 13), each run under a 6G memory cgroup: before the fix the render took
//! the daemon to the cap and it was OOM-killed; after it, three runs
//! peaked at 859 to 868 MiB once open and 1,171 to 1,187 MiB after the
//! refused render. The bound, `2 × budget + 256 MiB`, is the coxswain's.
//! Since #407 a render counts the meshes it makes against its quarter and
//! a refused one leaves none in the mesh cache: on cox-worker-1 (Intel
//! Xeon E5-2650, 8 threads, load 6 to 7) the peak was 696 MiB once open
//! and 942 MiB after the refused render. That is still past
//! `budget + budget / 4 + 256 MiB` (896 MiB), and machines differ by about
//! 160 MiB on the open alone, so the bound stays.

#![cfg(all(target_os = "linux", not(debug_assertions)))]

mod common;

use common::ryucad;
use ryucad::bench::scale::{Spec, write};
use ryucad_app::core::workspace::Workspace;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(900);
/// `session.max_memory`'s least value.
const BUDGET: u64 = 512 << 20;

/// A process's peak resident memory, in bytes.
fn peak(pid: u64) -> u64 {
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let line = s.lines().find(|l| l.starts_with("VmHWM:")).unwrap();
    let kib: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kib * 1024
}

#[test]
fn opening_ten_thousand_occurrences_stays_within_the_budget() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    let spec = Spec {
        races: false,
        ..Spec::parts(1000)
    };
    let made = write(&root, &spec, &[]);
    // In process, so no daemon starts before the budget is set.
    let set = common::bin(&ryucad().exe)
        .args([
            "--no-daemon",
            "config",
            "set",
            "--user",
            "session.max_memory",
        ])
        .arg(BUDGET.to_string())
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(set.status.success(), "{set:?}");
    let t = Instant::now();
    let mut ws = common::open_editing(&root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    let drawn = |ws: &Workspace| {
        ws.part("root").is_some_and(|r| {
            r.occurrences.len() == made.occurrences
                && r.solids.iter().all(|s| {
                    ws.model
                        .meshes
                        .contains_key(&("root".to_owned(), s.id.clone()))
                })
        })
    };
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && drawn(ws)));
    // This window asks for no thumbnail of a model it draws coarse-first.
    assert!(!ws.model.thumbnailed());
    let status = ryucad::client::status(&root).unwrap();
    let pid = status["pid"].as_u64().unwrap();
    assert_eq!(status["session"]["budget"]["max_bytes"], BUDGET, "{status}");
    let opened = peak(pid);
    // The thumbnail's own request, as a window before the fix sent it.
    let answer = ryucad()
        .connect(&root)
        .unwrap()
        .call_answer(&ryucad_app::core::thumbs::request())
        .unwrap();
    let after = peak(pid);
    let code = answer.as_ref().err().map(|e| e.code);
    println!(
        "the daemon over {} occurrences: peak {} MiB once open ({:.1} s), {} MiB after the \
         thumbnail's render ({code:?}); budget {} MiB",
        made.occurrences,
        opened >> 20,
        t.elapsed().as_secs_f64(),
        after >> 20,
        BUDGET >> 20,
    );
    assert_eq!(code, Some(ryucad::error::Code::RenderTooLarge));
    // What the budget holds, a render's quarter and the meshes it made
    // before it stopped, and the process's own (code, allocator, a
    // request's JSON).
    let bound = 2 * BUDGET + (256 << 20);
    assert!(
        after <= bound,
        "peak {} MiB, bound {} MiB",
        after >> 20,
        bound >> 20
    );
}
