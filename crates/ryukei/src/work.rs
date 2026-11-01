//! Bounded work: a cancel flag and a deterministic budget for long kernel
//! calls (design 296, plan 15.A).
//!
//! A caller gives a long call a [`Work`]: an optional cancel flag, set
//! from another thread, and an optional budget in work units. A work unit
//! is one charge at an inner loop (an enclosure, a cell or a pair of cells
//! visited, a search box, a step of a march, a span certified, a triangle
//! tested in refinement, a bisection step of an edge), so the same call
//! spends the same units on every platform and any number of threads.
//!
//! Inside, each sequential task carries a [`Meter`], passed explicitly as
//! `&Meter`, never through thread-locals (rayon's work stealing would run
//! another task's closure on this thread). A charge past the budget, or
//! after the flag is set, halts the task: the charge answers
//! [`Halt::Stopped`], which travels as [`Failure::Uncertain`], and every
//! later charge on that meter halts too. Parallel work goes through
//! [`Meter::par_map`]: each task gets a fresh meter whose limit is the whole
//! budget, and their counts are added to the parent in order. [`run`] sums
//! the call's count and replaces whatever the call returned with
//! [`Error::BudgetExceeded`] when the count passed the budget, else with
//! [`Error::Cancelled`] when a task halted. So whether a budget is
//! exceeded doesn't depend on scheduling; a cancel is a person's act, and
//! is not deterministic.
//!
//! [`Meter::capped`] is the kernel's own limit on one stage that could
//! otherwise run without a natural end (a trace, a search, a fit,
//! `beside`): past its scope's units, charges answer [`Halt::Capped`],
//! which is `Uncertain` too but stops only that scope. Caps are part of
//! the result, deterministic, and never the caller's budget error, so a
//! caller can always tell a kernel limit from its own.
//!
//! Never charge inside a lazily built shared value (a `OnceLock`, such as a
//! NURBS's Bézier pieces): whichever task built it first would pay.

#![allow(dead_code)] // until Task 10: the entry points use `run` and `par_map`


use crate::Error;
use crate::intersect::Failure;
use rayon::prelude::*;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What a long kernel call may spend, and the flag that stops it. One
/// `Work` may be given to several calls in turn; [`Work::spent`] adds up
/// what they spent, and the budget applies to each call alone.
#[derive(Debug)]
pub struct Work<'a> {
    cancel: Option<&'a AtomicBool>,
    budget: u64,
    spent: AtomicU64,
}

impl Default for Work<'_> {
    fn default() -> Self {
        Self::unlimited()
    }
}

impl<'a> Work<'a> {
    /// No cancel and no budget: what the plain entry points use.
    pub const fn unlimited() -> Self {
        Work {
            cancel: None,
            budget: u64::MAX,
            spent: AtomicU64::new(0),
        }
    }

    /// Stops a call, at its next charge, once `flag` is set.
    pub fn with_cancel(mut self, flag: &'a AtomicBool) -> Self {
        self.cancel = Some(flag);
        self
    }

    /// Refuses a call that would spend more than `units`.
    pub fn with_budget(mut self, units: u64) -> Self {
        self.budget = units;
        self
    }

    /// The budget, if there is one.
    pub fn budget(&self) -> Option<u64> {
        (self.budget != u64::MAX).then_some(self.budget)
    }

    /// The units every call given this `Work` has spent so far.
    pub fn spent(&self) -> u64 {
        self.spent.load(Ordering::Relaxed)
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }
}

/// Why a charge was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Halt {
    /// The budget is spent or the call was cancelled: sticky, and
    /// [`run`] turns the call into the caller's error.
    Stopped,
    /// The innermost [`Meter::capped`] scope is spent: `Uncertain` for
    /// that scope only.
    Capped,
}

impl From<Halt> for Failure {
    fn from(_: Halt) -> Self {
        Failure::Uncertain
    }
}

/// A stage of the kernel that could run without a natural end, and its
/// cap in work units. Set in plan 15.A's Task 19: 16 times the largest
/// seen across the release test suites (`work-stats`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// One surface–surface trace, its retries included.
    Trace,
    /// One curve–surface search.
    Search,
    /// One branch or pcurve fitted and certified.
    Fit,
    /// One `beside`, `faces_apart` or `apart_within`.
    Beside,
}

impl Scope {
    pub(crate) const ALL: [Scope; 4] = [Scope::Trace, Scope::Search, Scope::Fit, Scope::Beside];

    pub(crate) fn limit(self) -> u64 {
        match self {
            Scope::Trace => TRACE,
            Scope::Search => SEARCH,
            Scope::Fit => FIT,
            Scope::Beside => BESIDE,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Scope::Trace => "trace",
            Scope::Search => "search",
            Scope::Fit => "fit",
            Scope::Beside => "beside",
        }
    }
}

/// Caps: uncalibrated until Task 19.
const TRACE: u64 = u64::MAX;
const SEARCH: u64 = u64::MAX;
const FIT: u64 = u64::MAX;
const BESIDE: u64 = u64::MAX;

static FREE: Work<'static> = Work::unlimited();

/// One sequential task's count. Not `Sync`: parallel tasks each get their
/// own through [`Meter::par_map`].
#[derive(Debug)]
pub(crate) struct Meter<'a> {
    work: &'a Work<'a>,
    spent: Cell<u64>,
    halted: Cell<bool>,
    /// The count at which charges answer [`Halt::Capped`].
    cap: Cell<u64>,
}

impl<'a> Meter<'a> {
    pub(crate) fn new(work: &'a Work<'a>) -> Self {
        Meter {
            work,
            spent: Cell::new(0),
            halted: Cell::new(false),
            cap: Cell::new(u64::MAX),
        }
    }

    /// A meter with no budget and no cancel, for public functions that
    /// take none and for calls no long entry point makes. Every use says
    /// why with `// unmetered: …` (Task 18's test).
    pub(crate) fn free() -> Meter<'static> {
        Meter::new(&FREE)
    }

    pub(crate) fn spent(&self) -> u64 {
        self.spent.get()
    }

    pub(crate) fn halted(&self) -> bool {
        self.halted.get()
    }

    /// Spends `units`. Halts past the budget or once cancelled (and from
    /// then on), and answers `Capped` past the innermost cap.
    #[inline]
    pub(crate) fn charge(&self, units: u64) -> Result<(), Halt> {
        if self.halted.get() {
            return Err(Halt::Stopped);
        }
        let s = self.spent.get().saturating_add(units);
        self.spent.set(s);
        if s > self.work.budget || self.work.cancelled() {
            self.halted.set(true);
            return Err(Halt::Stopped);
        }
        if s > self.cap.get() {
            return Err(Halt::Capped);
        }
        Ok(())
    }

    /// Runs `f` under `scope`'s cap: once it has spent the scope's units,
    /// charges answer `Capped` until it returns.
    pub(crate) fn capped<R>(&self, scope: Scope, f: impl FnOnce() -> R) -> R {
        let before = self.spent.get();
        let r = self.capped_by(scope.limit(), f);
        stats::seen(scope, self.spent.get() - before);
        r
    }

    /// [`Meter::capped`] with an explicit number of units.
    pub(crate) fn capped_by<R>(&self, units: u64, f: impl FnOnce() -> R) -> R {
        let outer = self.cap.get();
        self.cap
            .set(outer.min(self.spent.get().saturating_add(units)));
        let r = f();
        self.cap.set(outer);
        r
    }

    /// `f` over `items` in parallel, each with a meter of its own, the
    /// results in order. The tasks' counts are added to this meter in
    /// order, and it halts if they pass the budget together.
    pub(crate) fn par_map<T, R>(
        &self,
        items: &[T],
        f: impl Fn(&Meter<'a>, &T) -> R + Sync,
    ) -> Vec<R>
    where
        T: Sync,
        R: Send,
    {
        let work = self.work;
        let halted = self.halted.get() || self.spent.get() > work.budget;
        let cap = self.cap.get();
        let room = if cap == u64::MAX {
            u64::MAX
        } else {
            cap.saturating_sub(self.spent.get())
        };
        let done: Vec<(R, u64, bool)> = items
            .par_iter()
            .map(|t| {
                let m = Meter::new(work);
                m.halted.set(halted);
                m.cap.set(room);
                let r = f(&m, t);
                (r, m.spent.get(), m.halted.get())
            })
            .collect();
        let mut out = Vec::with_capacity(done.len());
        for (r, s, h) in done {
            self.spent.set(self.spent.get().saturating_add(s));
            if h {
                self.halted.set(true);
            }
            out.push(r);
        }
        if self.spent.get() > work.budget {
            self.halted.set(true);
        }
        out
    }
}

/// Runs a long call under `work`: `f` with a meter of its own, then the
/// call's count added to `work`, then `BudgetExceeded` if it passed the
/// budget, else `Cancelled` if a task halted, else what `f` returned.
pub(crate) fn run<T>(
    work: &Work<'_>,
    f: impl FnOnce(&Meter<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    let m = Meter::new(work);
    let r = f(&m);
    let spent = m.spent.get();
    let _ = work
        .spent
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |s| {
            Some(s.saturating_add(spent))
        });
    if spent > work.budget {
        return Err(Error::BudgetExceeded {
            spent,
            budget: work.budget,
        });
    }
    if m.halted.get() {
        return Err(Error::Cancelled);
    }
    r
}

/// The largest units each cap's scope spent in this process, for
/// calibrating the caps (Task 19). Built only with the `work-stats`
/// feature; without it, nothing is recorded.
pub mod stats {
    use super::Scope;

    #[cfg(not(feature = "work-stats"))]
    pub(crate) fn seen(_: Scope, _: u64) {}

    #[cfg(feature = "work-stats")]
    pub(crate) use on::seen;

    #[cfg(feature = "work-stats")]
    pub use on::report;

    #[cfg(feature = "work-stats")]
    mod on {
        use super::Scope;
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};

        static MAX: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

        fn index(s: Scope) -> usize {
            Scope::ALL.iter().position(|&x| x == s).expect("a scope")
        }

        /// Notes that one scope of `s` spent `units`; a new largest goes
        /// to the file `RYUKEI_WORK_STATS` names, as `<scope> <units>`.
        pub(crate) fn seen(s: Scope, units: u64) {
            let before = MAX[index(s)].fetch_max(units, Ordering::Relaxed);
            if units > before
                && let Some(path) = std::env::var_os("RYUKEI_WORK_STATS")
                && let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
            {
                let _ = writeln!(f, "{} {units}", s.name());
            }
        }

        /// Each scope's largest spend so far in this process.
        pub fn report() -> Vec<(&'static str, u64)> {
            Scope::ALL
                .iter()
                .map(|&s| (s.name(), MAX[index(s)].load(Ordering::Relaxed)))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(threads: usize) -> rayon::ThreadPool {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
    }

    #[test]
    fn charges_add_up_and_a_budget_halts_past_it() {
        let w = Work::unlimited().with_budget(3);
        let m = Meter::new(&w);
        assert_eq!(m.charge(2), Ok(()));
        assert_eq!(m.charge(1), Ok(()));
        assert_eq!(m.charge(1), Err(Halt::Stopped));
        // Sticky, and nothing more is added.
        assert_eq!(m.charge(5), Err(Halt::Stopped));
        assert_eq!(m.spent(), 4);
        assert!(m.halted());
    }

    #[test]
    fn a_cancel_halts_at_the_next_charge() {
        let flag = AtomicBool::new(false);
        let w = Work::unlimited().with_cancel(&flag);
        let m = Meter::new(&w);
        assert_eq!(m.charge(1), Ok(()));
        flag.store(true, Ordering::Relaxed);
        assert_eq!(m.charge(1), Err(Halt::Stopped));
        assert!(m.halted());
    }

    #[test]
    fn a_cap_answers_capped_inside_its_scope_only() {
        let w = Work::unlimited();
        let m = Meter::new(&w);
        let inside = m.capped_by(2, || [m.charge(1), m.charge(1), m.charge(1)]);
        assert_eq!(inside, [Ok(()), Ok(()), Err(Halt::Capped)]);
        assert_eq!(m.charge(1), Ok(()));
        assert!(!m.halted());
        // Nested caps take the tighter.
        let r = m.capped_by(10, || m.capped_by(1, || [m.charge(1), m.charge(1)]));
        assert_eq!(r, [Ok(()), Err(Halt::Capped)]);
    }

    #[test]
    fn a_halt_converts_to_uncertain() {
        assert_eq!(Failure::from(Halt::Stopped), Failure::Uncertain);
        assert_eq!(Failure::from(Halt::Capped), Failure::Uncertain);
    }

    /// 64 tasks, the `k`th charging `k` units: 2,016 in all.
    fn tasks(m: &Meter<'_>) -> Vec<bool> {
        let items: Vec<u64> = (0..64).collect();
        m.par_map(&items, |m, &k| (0..k).all(|_| m.charge(1).is_ok()))
    }

    #[test]
    fn par_map_sums_its_tasks_alike_on_any_thread_count() {
        let mut seen = Vec::new();
        for threads in [1, 2, 8] {
            seen.push(pool(threads).install(|| {
                let w = Work::unlimited().with_budget(100);
                let m = Meter::new(&w);
                let r = tasks(&m);
                (r, m.spent(), m.halted())
            }));
        }
        // Each task alone is within 100 units, so none halts; together
        // they pass it, so the parent does.
        assert!(seen[0].0.iter().all(|&ok| ok));
        assert_eq!(seen[0].1, 2016);
        assert!(seen[0].2);
        assert!(seen.iter().all(|s| *s == seen[0]));
    }

    #[test]
    fn a_task_past_the_whole_budget_halts_alone() {
        let w = Work::unlimited().with_budget(40);
        let m = Meter::new(&w);
        let r = tasks(&m);
        // Tasks of more than 40 units stop at their 41st.
        assert!(r[..41].iter().all(|&ok| ok));
        assert!(r[41..].iter().all(|&ok| !ok));
        assert_eq!(m.spent(), (0..41u64).sum::<u64>() + 23 * 41);
    }

    #[test]
    fn a_halted_meter_makes_halted_tasks() {
        let w = Work::unlimited().with_budget(1);
        let m = Meter::new(&w);
        assert!(m.charge(2).is_err());
        let r = tasks(&m);
        assert!(r[1..].iter().all(|&ok| !ok));
        assert_eq!(m.spent(), 2);
    }

    #[test]
    fn tasks_inherit_what_is_left_of_a_cap() {
        let w = Work::unlimited();
        let m = Meter::new(&w);
        let r = m.capped_by(10, || {
            m.charge(5).unwrap();
            m.par_map(&[4u64, 5, 6], |m, &k| (0..k).all(|_| m.charge(1).is_ok()))
        });
        // Five units of the cap were left for each.
        assert_eq!(r, [true, true, false]);
        assert!(!m.halted());
    }

    #[test]
    fn run_says_budget_exceeded_before_cancelled() {
        let flag = AtomicBool::new(true);
        let w = Work::unlimited().with_budget(1).with_cancel(&flag);
        let r: Result<(), Error> = run(&w, |m| {
            let _ = m.charge(5);
            Ok(())
        });
        assert_eq!(
            r,
            Err(Error::BudgetExceeded {
                spent: 5,
                budget: 1
            })
        );
        let w = Work::unlimited().with_cancel(&flag);
        let r: Result<(), Error> = run(&w, |m| {
            let _ = m.charge(1);
            Ok(())
        });
        assert_eq!(r, Err(Error::Cancelled));
    }

    #[test]
    fn run_keeps_what_the_call_returned_when_nothing_halted() {
        let w = Work::unlimited().with_budget(10);
        assert_eq!(run(&w, |m| m.charge(3).map(|_| 7).map_err(|_| Error::Degenerate)), Ok(7));
        assert_eq!(run(&w, |m| m.charge(10).map(|_| 8).map_err(|_| Error::Degenerate)), Ok(8));
        // Calls add up on the work; each is judged alone.
        assert_eq!(w.spent(), 13);
        assert_eq!(w.budget(), Some(10));
        assert_eq!(Work::unlimited().budget(), None);
    }

    #[test]
    fn a_free_meter_never_halts() {
        let m = Meter::free();
        assert_eq!(m.charge(u64::MAX), Ok(()));
        assert_eq!(m.charge(1), Ok(()));
    }

    #[test]
    fn the_errors_say_what_stopped() {
        assert_eq!(Error::Cancelled.to_string(), "cancelled");
        assert_eq!(
            Error::BudgetExceeded {
                spent: 12,
                budget: 10
            }
            .to_string(),
            "work budget exceeded: 12 units spent of a budget of 10"
        );
    }

    #[cfg(feature = "work-stats")]
    #[test]
    fn stats_keep_each_scopes_largest_spend() {
        let w = Work::unlimited();
        let m = Meter::new(&w);
        m.capped(Scope::Fit, || m.charge(7).unwrap());
        m.capped(Scope::Fit, || m.charge(3).unwrap());
        let fit = stats::report()
            .into_iter()
            .find(|(n, _)| *n == "fit")
            .unwrap();
        assert!(fit.1 >= 7);
    }
}
