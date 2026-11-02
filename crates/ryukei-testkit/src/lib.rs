//! Test-only helpers for ryukei. Never a dependency of the kernel itself.
//!
//! - [`exact`]: exact rational reference results for the robust predicates.
//! - [`Lcg`]: a seeded, platform-independent input generator.
//! - [`Golden`]: bit-exact output hashing checked against committed hashes.
//! - [`euler_script`]: Euler operator sequences played from bytes.
//! - [`nurbs`]: seeded random NURBS curves and surfaces.
//! - [`mesh`]: a NURBS-topped block, and measures of meshes.
//! - [`sweep_bytes`]: sweeps from bytes, for fuzzing and its regressions.
//! - [`along_bytes`]: sweeps along paths and lofts from bytes, likewise.
//! - [`boolean_bytes`]: booleans from bytes, for fuzzing and its
//!   regressions.
//! - [`blend_bytes`]: blends from bytes, for fuzzing and its regressions.
//! - [`heal_bytes`]: shapes with a file's faults from bytes, for fuzzing
//!   healing and its regressions.
//! - [`offset_bytes`]: offsets and shells from bytes, for fuzzing and its
//!   regressions.
//! - [`sheet_bytes`]: sheets from bytes, for fuzzing and its regressions.
//! - [`tolerant_bytes`]: tolerant twins of bodies and booleans from bytes
//!   (design 299), for the tolerant tests and fuzzing.
//! - [`race`]: the bodies of the large-assembly STEP race.
//! - [`nurbs_race`]: the bodies of the NURBS race (design 94, plan 8.F).
//! - [`sheet_metal`]: sheet-metal parts extruded from bent strips.

/// The work budget the blend, sweep and offset fuzz targets give each
/// case, shared by every call it makes (the operation, its
/// self-intersection check and its mesh): 8 times the most any case of
/// theirs was measured to spend, rounded up to a power of two (plan 15.A,
/// Task 21: sweep's seed corpus at most 6,228,952 units on davids-m4, an
/// Apple M4, 2026-11-01; blend's 213,336 and offset's 3,925,012 on
/// cox-worker-4). A unit's cost varies about 275 times between cases (a
/// lofted spline's or a sweep's enclosures in the four-parameter tracer
/// take milliseconds each, #353), so the budget bounds a case in units and
/// [`FUZZ_DEADLINE`] bounds it in seconds.
pub const FUZZ_BUDGET: u64 = 1 << 26;

/// How long a fuzz case may run before it is cancelled: inside the
/// targets' timeouts (600 s for sweep and offset, 1,200 s for blend),
/// with room for what follows the cancel.
pub const FUZZ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(450);

/// One fuzz case's work: the calls it runs share one [`FUZZ_BUDGET`], and
/// a watchdog cancels them at [`FUZZ_DEADLINE`]. A cancel isn't
/// deterministic, but a fuzz case needn't be: a runaway ends as a typed
/// refusal (`Cancelled` or `BudgetExceeded`), never the target's timeout.
pub struct FuzzCase {
    cancel: std::sync::atomic::AtomicBool,
    spent: std::sync::atomic::AtomicU64,
}

impl FuzzCase {
    /// Runs one call under what is left of the case's budget, and counts
    /// what it spent against the case.
    pub fn run<T>(&self, call: impl FnOnce(&ryukei::work::Work) -> T) -> T {
        use std::sync::atomic::Ordering;
        let left = FUZZ_BUDGET.saturating_sub(self.spent.load(Ordering::Relaxed));
        let work = ryukei::work::Work::unlimited()
            .with_cancel(&self.cancel)
            .with_budget(left);
        let out = call(&work);
        self.spent.fetch_add(work.spent(), Ordering::Relaxed);
        out
    }

    /// The units the case's calls have spent so far.
    pub fn spent(&self) -> u64 {
        self.spent.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Runs one fuzz case with its watchdog; prints the units it spent when
/// `RYUKEI_FUZZ_UNITS` is set, for calibrating [`FUZZ_BUDGET`].
pub fn fuzz_case<R>(case: impl FnOnce(&FuzzCase) -> R) -> R {
    fuzz_case_within(FUZZ_DEADLINE, case)
}

/// [`fuzz_case`] with another deadline.
pub fn fuzz_case_within<R>(deadline: std::time::Duration, case: impl FnOnce(&FuzzCase) -> R) -> R {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{RecvTimeoutError, channel};
    let c = FuzzCase {
        cancel: AtomicBool::new(false),
        spent: AtomicU64::new(0),
    };
    let (done, wait) = channel::<()>();
    let out = std::thread::scope(|s| {
        let cancel = &c.cancel;
        s.spawn(move || {
            if wait.recv_timeout(deadline) == Err(RecvTimeoutError::Timeout) {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        let out = case(&c);
        drop(done);
        out
    });
    if std::env::var_os("RYUKEI_FUZZ_UNITS").is_some() {
        eprintln!("units {}", c.spent());
    }
    out
}

#[cfg(test)]
mod fuzz_case_tests {
    use super::*;
    use ryukei::geom::Rigid;
    use ryukei::mesh::{BodyMesh, MeshParams, tessellate_with};
    use ryukei::name::OpKey;
    use ryukei::primitive::cylinder;

    fn spend(w: &ryukei::work::Work) -> Result<BodyMesh, ryukei::Error> {
        let (b, _) = cylinder(OpKey(1), 0.01, 0.02, &Rigid::IDENTITY).expect("a rod");
        tessellate_with(&b, &MeshParams::new(1e-5, 0.2), w)
    }

    #[test]
    fn a_case_shares_one_budget_between_its_calls() {
        let one = fuzz_case(|c| {
            c.run(spend).expect("meshes");
            c.spent()
        });
        assert!(one > 0);
        let both = fuzz_case(|c| {
            c.run(spend).expect("meshes");
            c.run(spend).expect("meshes");
            c.spent()
        });
        assert_eq!(both, 2 * one);
        // The second call gets only what the first left.
        fuzz_case(|c| {
            c.spent
                .store(FUZZ_BUDGET - one + 1, std::sync::atomic::Ordering::Relaxed);
            assert!(matches!(
                c.run(spend),
                Err(ryukei::Error::BudgetExceeded { .. })
            ));
        });
    }

    #[test]
    fn a_case_past_its_deadline_is_cancelled() {
        let r = fuzz_case_within(std::time::Duration::ZERO, |c| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            c.run(spend)
        });
        assert!(matches!(r, Err(ryukei::Error::Cancelled)), "{r:?}");
    }
}

pub mod along_bytes;
pub mod blend_bytes;
pub mod boolean_bytes;
pub mod euler_script;
pub mod exact;
mod golden;
pub mod heal_bytes;
mod lcg;
pub mod mesh;
pub mod nurbs;
pub mod nurbs_race;
pub mod offset_bytes;
pub mod race;
pub mod sheet_bytes;
pub mod sheet_metal;
pub mod sweep_bytes;
pub mod tolerant_bytes;

pub use golden::{Golden, Section, unversioned};
pub use lcg::Lcg;
