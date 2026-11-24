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
//! budget, and their counts are added to the parent in order. Under a
//! finite cap a map answers as its tasks run one after another in order
//! would: each task's room is what the tasks before it left (design 669),
//! so a rung's cap bounds its pairs' sum. [`run`] sums
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

use crate::Error;
use crate::geom::nurbs::memo::Enclosures;
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
    projections: AtomicU64,
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
            projections: AtomicU64::new(0),
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

    /// How many points the calls given this `Work` found the feet of by
    /// a global projection onto a surface, where they count them: a fit's
    /// samples and an offset's traced closing edges (plan 15.C, #408). A
    /// count, not units: no unit is charged for it, since one projection's
    /// cost depends on the surface (seconds on a spline's offset), so it
    /// changes no budget, cap or result. A test can watch it while a call
    /// runs and cancel it once it passes a bound.
    pub fn projections(&self) -> u64 {
        self.projections.load(Ordering::Relaxed)
    }

    /// Whether the cancel flag is set: for callers that loop over kernel
    /// calls and stop between them.
    pub fn cancelled(&self) -> bool {
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
    /// One face pair met in a file's union of its solids
    /// ([`crate::boolean::unite_solids_with`]): `face_face`, its traces
    /// and searches included (#550).
    Union,
    /// One rung of the local re-trim's ladder: its working body, table,
    /// walk and checks (#647, #659).
    Rung,
    /// One deleted set's whole re-trim ladder (design 919, plan 23.B):
    /// every rung and build of it, their own caps nested inside.
    Gap,
}

impl Scope {
    #[cfg_attr(not(feature = "work-stats"), allow(dead_code))]
    pub(crate) const ALL: [Scope; 7] = [
        Scope::Trace,
        Scope::Search,
        Scope::Fit,
        Scope::Beside,
        Scope::Union,
        Scope::Rung,
        Scope::Gap,
    ];

    pub(crate) fn limit(self) -> u64 {
        match self {
            Scope::Trace => TRACE,
            Scope::Search => SEARCH,
            Scope::Fit => FIT,
            Scope::Beside => BESIDE,
            Scope::Union => UNION,
            Scope::Rung => RUNG,
            Scope::Gap => GAP,
        }
    }

    #[cfg_attr(not(feature = "work-stats"), allow(dead_code))]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Scope::Trace => "trace",
            Scope::Search => "search",
            Scope::Fit => "fit",
            Scope::Beside => "beside",
            Scope::Union => "union",
            Scope::Rung => "rung",
            Scope::Gap => "gap",
        }
    }
}

/// Caps, each 16 times the largest scope seen across ryukei's, ryustep's
/// and ryucad's release suites and the pinned STEP corpus, rounded up to
/// a power of two (plan 15.A's rule). Plan 15.C weighted enclosure charges
/// by their sheet's cost (#353) and measured again on cox-worker-4 (AMD
/// Opteron 6380, 2026-11-04, at 198515ab): largest trace 2,866,615
/// (`heal_props`), search 467,324 (`general_boolean_props`), fit 1,467,251
/// (`intersect_props`) and beside 577,683 (`spline_offsets`, whose offsets'
/// enclosures now cost 8). Plan 15.A's were trace 2,866,615, search
/// 65,515, fit 1,005,613 and beside 191,434, under caps of `1 << 26`,
/// `1 << 21`, `1 << 24` and `1 << 23`.
const TRACE: u64 = 1 << 26;
const SEARCH: u64 = 1 << 23;
const FIT: u64 = 1 << 25;
const BESIDE: u64 = 1 << 24;
/// A face pair of a file's union: 16 times the largest pair of a union
/// that builds across the release suites and the pinned STEP corpus,
/// rounded up to a power of two (plan 16.B's Task 8, on cox-worker-2,
/// AMD Ryzen 7 9700X, 2026-11-11: largest 968,826 units, a 119-face
/// union of a pinned Voron assembly in ryucad's `corpus` test; every
/// union past 1M units in those runs failed, and #550's inductor's
/// refused pair spent 23,520,833). Unions only: every other boolean's
/// pairs keep the trace's own cap.
const UNION: u64 = 1 << 24;
/// One rung of the re-trim: 16 times the largest rung that builds across
/// the release suites and the pinned STEP corpus, rounded up to a power
/// of two (plan 16.C's Task 11, on cox-worker-2, AMD Ryzen 7 9700X,
/// 2026-11-11: largest 1,009,883 units, NIST FTC 08's pocket floor, a
/// 33-face region at rung 0, in a pinned edition; `retrim`,
/// `delete_faces`, `blend_edits`, `tweaks`, `drafts`, `edit_props`,
/// `work`, `fuzz_regressions`, `tweak_props` and ryustep's `defeature`
/// with its pinned bodies). Every rung past it refused: FTC 07's spent
/// 95,573,843. Under design 669's ordered sum it bounds the rung's
/// parallel pairs together, not each alone.
const RUNG: u64 = 1 << 24;
/// One deleted set's ladder (design 919, plan 23.B): four times the
/// largest ladder that builds across ryukei's suites, `edit_props`,
/// ryustep's committed and pinned corpora and ryucad's `direct_edits`,
/// rounded up to a power of two (census VIII on cox-worker-2, 2026-11-18,
/// main `ede45371`: largest 9,871,261 units, the bent rod's rim of plan
/// 19.A, whose rung-0 first build spends 9,606,924 refusing `Uncertain`
/// before its narrowed retry builds). Every refused set spent less: up to
/// 27,459,995 (linkrods #11), so today the gap refuses nothing sooner:
/// the owner chose the rule's figure (2026-11-18) over a margin under
/// four. A work bound, not a tolerance.
const GAP: u64 = 1 << 26;

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
    /// The innermost [`Meter::capped`] scope, read only by `work-stats`.
    #[cfg_attr(not(feature = "work-stats"), allow(dead_code))]
    scope: Cell<Option<Scope>>,
    /// The operation's enclosures, shared by every meter of the call.
    enclosures: &'a Enclosures,
    /// Inside a re-trim rung's first build (plan 20.B): no tube contact
    /// after a failed trace there, so every body built today keeps its
    /// first build's answer.
    first_build: Cell<bool>,
}

impl<'a> Meter<'a> {
    /// A meter whose table of enclosures remembers nothing.
    pub(crate) fn new(work: &'a Work<'a>) -> Self {
        Meter::with_enclosures(work, &crate::geom::nurbs::memo::NONE)
    }

    /// A meter reading and filling `enclosures`.
    pub(crate) fn with_enclosures(work: &'a Work<'a>, enclosures: &'a Enclosures) -> Self {
        Meter {
            work,
            spent: Cell::new(0),
            halted: Cell::new(false),
            cap: Cell::new(u64::MAX),
            scope: Cell::new(None),
            enclosures,
            first_build: Cell::new(false),
        }
    }

    /// The operation's table of enclosures: what it gives is what
    /// computing again gives, so no result depends on it.
    pub(crate) fn enclosures(&self) -> &'a Enclosures {
        self.enclosures
    }

    /// A meter with no budget and no cancel, for public functions that
    /// take none and for calls no long entry point makes. Every use says
    /// why with `// unmetered: …` (Task 18's test).
    pub(crate) fn free() -> Meter<'static> {
        Meter::new(&FREE)
    }

    #[allow(dead_code)] // the tests read it
    pub(crate) fn spent(&self) -> u64 {
        self.spent.get()
    }

    pub(crate) fn halted(&self) -> bool {
        self.halted.get()
    }

    /// Whether the innermost cap is spent: charges now answer
    /// [`Halt::Capped`] (plan 23.B: a ladder reads it between builds,
    /// where the innermost cap is its gap's). Never under no cap.
    pub(crate) fn exhausted(&self) -> bool {
        self.spent.get() > self.cap.get()
    }

    /// What halted this meter (plan 23.C): `None` while it runs; the
    /// caller's cancel; or its budget, with what was spent.
    pub(crate) fn stop_cause(&self) -> Option<Error> {
        if !self.halted.get() {
            return None;
        }
        if self.work.cancelled() {
            return Some(Error::Cancelled);
        }
        Some(Error::BudgetExceeded {
            spent: self.spent.get(),
            budget: self.work.budget,
        })
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

    /// Halts once cancelled, charging nothing: for work that must not
    /// count (debug builds' input checks), so units are the same in every
    /// build while a cancel still stops it.
    /// Counts one global projection ([`Work::projections`]).
    pub(crate) fn projected(&self) {
        self.work.projections.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn poll(&self) -> Result<(), Halt> {
        if self.halted.get() {
            return Err(Halt::Stopped);
        }
        if self.work.cancelled() {
            self.halted.set(true);
            return Err(Halt::Stopped);
        }
        Ok(())
    }

    /// Runs `f` as a re-trim rung's first build (plan 20.B).
    pub(crate) fn first_build<R>(&self, f: impl FnOnce() -> R) -> R {
        let outer = self.first_build.replace(true);
        let r = f();
        self.first_build.set(outer);
        r
    }

    /// Whether this is inside a re-trim rung's first build.
    pub(crate) fn in_first_build(&self) -> bool {
        self.first_build.get()
    }

    /// Runs `f` under `scope`'s cap: once it has spent the scope's units,
    /// charges answer `Capped` until it returns.
    pub(crate) fn capped<R>(&self, scope: Scope, f: impl FnOnce() -> R) -> R {
        self.capped_with(scope, scope.limit(), f)
    }

    /// [`Meter::capped`] at `units` rather than the scope's own limit:
    /// the tests' small gaps, and `delete::grow`'s `GAP`.
    pub(crate) fn capped_with<R>(&self, scope: Scope, units: u64, f: impl FnOnce() -> R) -> R {
        let before = self.spent.get();
        let outer = self.scope.replace(Some(scope));
        let r = self.capped_by(units, f);
        self.scope.set(outer);
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
    /// order, and it halts if they pass the budget together. Under a
    /// finite cap the tasks get, and are charged, exactly what running
    /// them one after another in order would give them ([`in_order`],
    /// design 669); under none each gets everything, as a budget gives
    /// each task the whole budget.
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
        let enclosures = self.enclosures;
        let halted = self.halted.get() || self.spent.get() > work.budget;
        let scope = self.scope.get();
        let cap = self.cap.get();
        let first_build = self.first_build.get();
        // One task on a fresh meter given `room`: its result, count and halt.
        let one = |t: &T, room: u64| {
            let m = Meter::with_enclosures(work, enclosures);
            m.halted.set(halted);
            m.cap.set(room);
            m.scope.set(scope);
            m.first_build.set(first_build);
            let r = f(&m, t);
            (r, m.spent.get(), m.halted.get())
        };
        let done: Vec<(R, u64, bool)> = if cap == u64::MAX {
            items.par_iter().map(|t| one(t, u64::MAX)).collect()
        } else {
            in_order(items, cap.saturating_sub(self.spent.get()), scope, &one)
        };
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

/// A capped parallel map's tasks as running them one after another in
/// item order would give them (design 669): task `i`'s room is `room`
/// less what tasks `0..i` spent. Tasks run in waves of the pool's threads,
/// each given the room left when its wave starts. A task whose count stays
/// within its in-order room, or whose wave gave it exactly that room, ran
/// as the sequential run would (its charges don't depend on its limit
/// until they pass it), and keeps its result. The first that passed its
/// room runs again alone with exactly that room, and every later task with
/// what is then left: none, so each answers at its first charge. Only
/// discarded speculative work, at most one wave's room, depends on the
/// thread count, and it is never counted.
fn in_order<T, R>(
    items: &[T],
    room: u64,
    scope: Option<Scope>,
    one: &(impl Fn(&T, u64) -> (R, u64, bool) + Sync),
) -> Vec<(R, u64, bool)>
where
    T: Sync,
    R: Send,
{
    let wave = rayon::current_num_threads().max(1);
    let mut out: Vec<(R, u64, bool)> = Vec::with_capacity(items.len());
    let mut used: u64 = 0;
    let mut start = 0;
    'waves: while start < items.len() {
        let given = room.saturating_sub(used);
        let end = (start + wave).min(items.len());
        let ran: Vec<(R, u64, bool)> = items[start..end]
            .par_iter()
            .map(|t| one(t, given))
            .collect();
        for (k, r) in ran.into_iter().enumerate() {
            let mine = room.saturating_sub(used);
            // The first task whose count crosses the room: noted for
            // `work-stats` (plan 20.A, Task 2), however many threads ran.
            if used <= room && r.1 > mine {
                stats::overran(scope, room, used.saturating_add(r.1));
            }
            if r.1 <= mine || given == mine {
                used = used.saturating_add(r.1);
                out.push(r);
                continue;
            }
            let i = start + k;
            let again = one(&items[i], mine);
            used = used.saturating_add(again.1);
            out.push(again);
            if room.saturating_sub(used) == 0 {
                // No room for any later task: each answers alike whatever
                // ran before it, so they run together.
                out.extend(
                    items[i + 1..]
                        .par_iter()
                        .map(|t| one(t, 0))
                        .collect::<Vec<_>>(),
                );
                return out;
            }
            // A task run again with its room passes it again (the charge
            // that passed it is the same), so this isn't reached; were it,
            // the tasks after it go on in order.
            start = i + 1;
            continue 'waves;
        }
        start = end;
    }
    out
}

/// Runs a long call under `work`: `f` with a meter of its own, then the
/// call's count added to `work`, then `BudgetExceeded` if it passed the
/// budget, else `Cancelled` if a task halted, else what `f` returned.
pub(crate) fn run<T>(
    work: &Work<'_>,
    f: impl FnOnce(&Meter<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    let table = Enclosures::new();
    let m = Meter::with_enclosures(work, &table);
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

/// [`run`] for a call that settles its own budget (plan 23.C's partial
/// delete keeps what it built and reports the rest): what it spent is
/// counted, a cancel still answers `Cancelled`, and past the budget its
/// answer stands. A refusal past the budget is the budget's, as in
/// [`run`]: the call never got to keep anything.
pub(crate) fn run_keeping<T>(
    work: &Work<'_>,
    f: impl FnOnce(&Meter<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    let table = Enclosures::new();
    let m = Meter::with_enclosures(work, &table);
    let r = f(&m);
    let spent = m.spent.get();
    let _ = work
        .spent
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |s| {
            Some(s.saturating_add(spent))
        });
    if m.halted.get() && work.cancelled() {
        return Err(Error::Cancelled);
    }
    if r.is_err() && spent > work.budget {
        return Err(Error::BudgetExceeded {
            spent,
            budget: work.budget,
        });
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

    #[cfg(not(feature = "work-stats"))]
    pub(crate) fn overran(_: Option<Scope>, _: u64, _: u64) {}

    #[cfg(feature = "work-stats")]
    pub(crate) use on::overran;

    #[cfg(feature = "work-stats")]
    pub use on::overruns;

    #[cfg(feature = "work-stats")]
    pub use on::report;

    #[cfg(feature = "work-stats")]
    mod on {
        use super::Scope;
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};

        static MAX: [AtomicU64; Scope::ALL.len()] = [const { AtomicU64::new(0) }; Scope::ALL.len()];

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
                // One write of a whole line: appends that small don't
                // interleave between processes or threads.
                let _ = f.write_all(format!("{} {units}\n", s.name()).as_bytes());
            }
        }

        static OVERRUNS: AtomicU64 = AtomicU64::new(0);

        /// Notes a capped parallel map whose tasks together passed the
        /// room it started with (design 669): `overran <scope> <room>
        /// <sum> <thread>` to the file `RYUKEI_WORK_STATS` names. The
        /// thread is libtest's test name where the map ran on it.
        pub(crate) fn overran(s: Option<Scope>, room: u64, sum: u64) {
            OVERRUNS.fetch_add(1, Ordering::Relaxed);
            if let Some(path) = std::env::var_os("RYUKEI_WORK_STATS")
                && let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
            {
                let scope = s.map_or("none", Scope::name);
                let thread = std::thread::current();
                let name = thread.name().unwrap_or("?");
                let _ = f.write_all(format!("overran {scope} {room} {sum} {name}\n").as_bytes());
            }
        }

        /// How many capped maps have overrun in this process.
        pub fn overruns() -> u64 {
            OVERRUNS.load(Ordering::Relaxed)
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

    /// Design 669: a capped map whose tasks together pass the room it
    /// started with is noted, with the innermost scope's name, for the
    /// ordered sum's census (plan 20.A, Task 2).
    #[cfg(feature = "work-stats")]
    #[test]
    fn a_capped_map_past_its_room_is_noted() {
        let before = stats::overruns();
        let w = Work::unlimited();
        let m = Meter::new(&w);
        m.capped(Scope::Rung, || {
            m.capped_by(10, || {
                m.par_map(&[6u64, 6], |m, &k| (0..k).all(|_| m.charge(1).is_ok()))
            })
        });
        assert!(stats::overruns() > before);
    }

    #[test]
    fn tasks_share_what_is_left_of_a_cap_in_order() {
        let w = Work::unlimited();
        let m = Meter::new(&w);
        let r = m.capped_by(10, || {
            m.charge(5).unwrap();
            m.par_map(&[4u64, 5, 6], |m, &k| (0..k).all(|_| m.charge(1).is_ok()))
        });
        // Five units were left: four for the first; one for the second,
        // which stops at its second charge; none for the third, which
        // stops at its first (design 669).
        assert_eq!(r, [true, false, false]);
        assert_eq!(m.spent(), 5 + 4 + 2 + 1);
        assert!(!m.halted());
    }

    /// The tasks `items` one after another on one meter under `cap`: what
    /// a capped map must answer on any thread count (design 669).
    fn sequential(cap: u64, items: &[u64]) -> (Vec<bool>, u64) {
        let w = Work::unlimited();
        let m = Meter::new(&w);
        let r = m.capped_by(cap, || {
            items
                .iter()
                .map(|&k| (0..k).all(|_| m.charge(1).is_ok()))
                .collect()
        });
        (r, m.spent())
    }

    #[test]
    fn a_capped_map_answers_as_its_tasks_in_order_on_any_thread_count() {
        let up: Vec<u64> = (0..64).collect();
        let down: Vec<u64> = (0..64).rev().collect();
        for items in [&up, &down] {
            let want = sequential(1000, items);
            for threads in [1, 2, 8] {
                let got = pool(threads).install(|| {
                    let w = Work::unlimited();
                    let m = Meter::new(&w);
                    let r = m.capped_by(1000, || {
                        m.par_map(items, |m, &k| (0..k).all(|_| m.charge(1).is_ok()))
                    });
                    assert!(!m.halted());
                    (r, m.spent())
                });
                assert_eq!(got, want, "{threads} threads");
            }
        }
        // In item order: tasks 0 to 44 spend 990 units; task 45 has 10
        // left and stops at its 11th charge; the 18 after it at their
        // first.
        let (r, spent) = sequential(1000, &up);
        assert!(r[..45].iter().all(|&ok| ok));
        assert!(r[45..].iter().all(|&ok| !ok));
        assert_eq!(spent, 990 + 11 + 18);
    }

    /// Plan 23.C: what stopped a meter, and a run that keeps its answer
    /// past the budget (counting what it spent) but not past a cancel.
    #[test]
    fn run_keeping_keeps_the_answer_past_a_budget_only() {
        let w = Work::unlimited().with_budget(3);
        let r = run_keeping(&w, |m| {
            let _ = m.charge(5);
            Ok(m.stop_cause())
        });
        assert_eq!(
            r,
            Ok(Some(Error::BudgetExceeded {
                spent: 5,
                budget: 3
            }))
        );
        assert_eq!(w.spent(), 5);
        let flag = AtomicBool::new(true);
        let w = Work::unlimited().with_cancel(&flag);
        let r = run_keeping(&w, |m| {
            let _ = m.charge(1);
            Ok(m.stop_cause())
        });
        assert_eq!(r, Err(Error::Cancelled));
        let w = Work::unlimited();
        assert_eq!(run_keeping(&w, |m| Ok(m.stop_cause())), Ok(None));
    }

    /// Plan 23.D: a call that refuses past its budget (a partial delete's
    /// recognition halted before anything was built) answers
    /// `BudgetExceeded`, as `run` does; one that refuses within it keeps
    /// its own refusal.
    #[test]
    fn run_keeping_answers_the_budget_for_a_refusal_past_it() {
        let w = Work::unlimited().with_budget(3);
        let r: Result<(), Error> = run_keeping(&w, |m| {
            let _ = m.charge(5);
            Err(Error::NonFinite)
        });
        assert_eq!(r, Err(Error::BudgetExceeded { spent: 5, budget: 3 }));
        let w = Work::unlimited().with_budget(3);
        let r: Result<(), Error> = run_keeping(&w, |m| {
            let _ = m.charge(2);
            Err(Error::NonFinite)
        });
        assert_eq!(r, Err(Error::NonFinite));
    }

    /// STC 07's shape (#658): a rung of pairs, each able to spend a
    /// trace's cap, that together pass the rung's. The rung's map spends
    /// its cap and at most one charge a task past it, on any thread count.
    #[test]
    fn a_rungs_pairs_spend_its_cap_between_them() {
        const STEP: u64 = 1024;
        // 16 pairs of 4M units: four times the rung's cap.
        let pairs = vec![1u64 << 22; 16];
        for threads in [1, 8] {
            let (ok, spent) = pool(threads).install(|| {
                let w = Work::unlimited();
                let m = Meter::new(&w);
                let r = m.capped(Scope::Rung, || {
                    m.par_map(&pairs, |m, &k| {
                        m.capped(Scope::Trace, || {
                            (0..k / STEP).all(|_| m.charge(STEP).is_ok())
                        })
                    })
                });
                (r.iter().filter(|&&x| x).count(), m.spent())
            });
            // Four pairs fill the cap exactly; the twelve after it each
            // stop at their first charge.
            assert_eq!(ok, 4, "{threads} threads");
            assert_eq!(spent, RUNG + 12 * STEP, "{threads} threads");
        }
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
        assert_eq!(
            run(&w, |m| m
                .charge(3)
                .map(|_| 7)
                .map_err(|_| Error::Degenerate)),
            Ok(7)
        );
        assert_eq!(
            run(&w, |m| m
                .charge(10)
                .map(|_| 8)
                .map_err(|_| Error::Degenerate)),
            Ok(8)
        );
        // Calls add up on the work; each is judged alone.
        assert_eq!(w.spent(), 13);
        assert_eq!(w.budget(), Some(10));
        assert_eq!(Work::unlimited().budget(), None);
    }

    #[test]
    fn a_free_meter_never_halts() {
        // unmetered: a test
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

    #[test]
    fn a_rung_is_a_scope_of_its_own() {
        assert!(Scope::ALL.contains(&Scope::Rung));
        assert_eq!(Scope::Rung.name(), "rung");
        assert_eq!(Scope::Rung.limit(), RUNG);
        // A rung's cap binds before the trace's inside it.
        let w = Work::unlimited();
        let m = Meter::new(&w);
        let r = m.capped_by(10, || {
            m.capped(Scope::Trace, || {
                (0..20).map(|_| m.charge(1)).collect::<Vec<_>>()
            })
        });
        assert_eq!(r.iter().filter(|x| x.is_ok()).count(), 10);
    }

    /// Design 919, plan 23.B: a deleted set's ladder is a scope of its
    /// own, `GAP` units, and the rungs inside it share it: a rung's cap
    /// never reaches past the gap's.
    #[test]
    fn a_gap_is_a_scope_its_rungs_share() {
        assert!(Scope::ALL.contains(&Scope::Gap));
        assert_eq!(Scope::Gap.name(), "gap");
        assert_eq!(Scope::Gap.limit(), GAP);
        assert!(GAP.is_power_of_two() && GAP >= 4 * 9_871_261, "{GAP}");
        let w = Work::unlimited();
        let m = Meter::new(&w);
        // A gap of 10 units: two rungs of 6 each get 10 between them.
        let r = m.capped_with(Scope::Gap, 10, || {
            let a = m.capped(Scope::Rung, || {
                (0..6).map(|_| m.charge(1)).collect::<Vec<_>>()
            });
            let b = m.capped(Scope::Rung, || {
                (0..6).map(|_| m.charge(1)).collect::<Vec<_>>()
            });
            (a, b)
        });
        assert!(r.0.iter().all(Result::is_ok));
        assert_eq!(r.1.iter().filter(|x| x.is_ok()).count(), 4);
        assert_eq!(r.1[4], Err(Halt::Capped));
        // Outside it, charges go on: a cap is never the caller's budget.
        assert_eq!(m.charge(1), Ok(()));
        assert!(!m.halted());
        // `capped` is `capped_with` at the scope's own limit.
        let w = Work::unlimited();
        let m = Meter::new(&w);
        let n = m.capped(Scope::Fit, || {
            m.charge(FIT).unwrap();
            m.charge(1)
        });
        assert_eq!(n, Err(Halt::Capped));
    }

    /// Plan 23.B: a meter is exhausted once its innermost cap is spent,
    /// never by its budget alone or under no cap, and no longer once the
    /// cap's scope has returned.
    #[test]
    fn a_meter_is_exhausted_past_its_innermost_cap_only() {
        let w = Work::unlimited();
        let m = Meter::new(&w);
        assert!(!m.exhausted());
        m.charge(1_000).unwrap();
        assert!(!m.exhausted(), "no cap: never exhausted");
        let inside = m.capped_with(Scope::Gap, 3, || {
            let a = m.exhausted();
            let _ = m.charge(3);
            let b = m.exhausted();
            let _ = m.charge(1);
            (a, b, m.exhausted())
        });
        // At the cap it is spent but not past it; one more is past.
        assert_eq!(inside, (false, false, true));
        assert!(!m.exhausted(), "the scope returned");
        // A budget's halt is not a cap.
        let w = Work::unlimited().with_budget(2);
        let m = Meter::new(&w);
        let _ = m.charge(5);
        assert!(m.halted() && !m.exhausted());
    }
}
