//! A cancel stops a heal promptly (#485): healing NIST CTC 05's solid
//! spent about 20 s comparing its coincident spline faces' pcurves in
//! `intersect2::crossings`, which never looked at the cancel, so a daemon
//! asked to cancel a cold import's rebuild stayed busy that long.
//!
//! Release only (a debug heal takes minutes): `cargo test -p ryustep
//! --release --test cancel`.

use ryukei::name::OpKey;
use ryukei::work::Work;
use ryustep::{ImportError, ReadOptions};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The longest a heal may run on once cancelled. After the fix the worst
/// of 38 cancels was 0.13 s on cox-worker-3 (Intel Core Ultra 5 250K
/// Plus, three rayon threads, load about 2); before it, 20 s.
const STOPS_WITHIN: Duration = Duration::from_secs(2);

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_cancelled_heal_stops_within_a_couple_of_seconds() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/corpus/nist_ctc_05_asme1_rd.stp"
    );
    let bytes = std::fs::read(path).unwrap();
    let opened = ryustep::open(&bytes).unwrap();
    let opts = ReadOptions {
        key: OpKey::from_bytes(b"cancel"),
        heal: Default::default(),
    };
    // Its first representation is the solid, healed once to time it.
    let k = 0;
    let t = Instant::now();
    assert!(opened.body(k, &opts).result.is_ok());
    let whole = t.elapsed();
    for fraction in [0.2, 0.4, 0.6, 0.8] {
        let at = whole.mul_f64(fraction);
        let flag = AtomicBool::new(false);
        let (late, result) = std::thread::scope(|s| {
            let h = s.spawn(|| {
                let b = opened.body_with(k, &opts, &Work::unlimited().with_cancel(&flag));
                (Instant::now(), b.result)
            });
            std::thread::sleep(at);
            let sent = Instant::now();
            flag.store(true, Ordering::SeqCst);
            let (done, result) = h.join().unwrap();
            (done.saturating_duration_since(sent), result)
        });
        println!(
            "cancelled at {:.2} of {:.2} s: stopped {:.3} s later",
            at.as_secs_f64(),
            whole.as_secs_f64(),
            late.as_secs_f64()
        );
        match result {
            Err(ImportError::Work(ryukei::Error::Cancelled)) => {}
            // Finished before the cancel landed (a fast machine near the end).
            Ok(_) if late <= STOPS_WITHIN => continue,
            other => panic!("cancelled at {at:?}: {:?}", other.map(|_| "a body")),
        }
        assert!(
            late <= STOPS_WITHIN,
            "cancelled {:.2} s in, the heal ran on {:.2} s",
            at.as_secs_f64(),
            late.as_secs_f64()
        );
    }
}
