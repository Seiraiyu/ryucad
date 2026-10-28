//! A daemon's memory budget (`session.max_memory`): how much of parts'
//! rebuilds it holds, least recently used first out.
//!
//! What a part's rebuild holds is counted by its results' encoded sizes
//! (their disk cache entries', else the codec's lengths, measured once
//! per body) times [`RESIDENT`]: a body takes about three times its
//! encoding in memory. Past the
//! budget, whole parts' rebuilds go (their features' results and their
//! caches), least recently used first, but never what an assembly's model
//! shows of them: each placed part's model (the solids it shows), which
//! drawing, picking and interference read, is kept, and counted. A part
//! whose rebuild went is rebuilt when it is next asked for, from the disk
//! cache (or, where the cache has trimmed it, from its inputs): the same
//! bits. Assemblies, hashes, names, reports' summaries, poses and solves
//! are small and always kept. Parts a request asks for itself are kept
//! until it answers.

use crate::doc::id::Id;
use crate::rebuild::{Built, Cache, Made, Output, PartResult};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// What a result takes in memory for each byte of its encoding, as
/// measured over design 192's benchmark project: 1.9 GB of encoded bodies
/// held in 5.2 GB, once origins and names shared their values.
pub const RESIDENT: u64 = 3;

/// The default budget: a quarter of the machine's memory, from 512 MiB to
/// 8 GiB (512 MiB where the machine doesn't say).
pub fn default_max() -> u64 {
    const MIB: u64 = 1 << 20;
    physical_memory().map_or(512 * MIB, |m| (m / 4).clamp(512 * MIB, 8192 * MIB))
}

/// The machine's physical memory in bytes, where the system says.
pub fn physical_memory() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kib: u64 = s
            .lines()
            .find(|l| l.starts_with("MemTotal:"))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        Some(kib * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        let mut bytes: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: `hw.memsize` is a u64, and `len` says how much room
        // `bytes` gives it.
        let ok = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&raw mut bytes).cast(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (ok == 0).then_some(bytes)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // SAFETY: the struct is plain data, sized as the call requires.
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut m) };
        (ok != 0).then_some(m.ullTotalPhys)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// What a part's rebuild holds: every body it and its cache hold, and of
/// those the bodies its model shows.
#[derive(Clone, Copy, Debug, Default)]
struct Held {
    all: u64,
    shown: u64,
}

/// The budget and what it counts. `max` 0 is no budget: nothing is
/// measured or let go (sessions in a CLI process, which answer once).
#[derive(Clone, Debug, Default)]
pub struct Budget {
    pub max: u64,
    clock: u64,
    used: BTreeMap<Id, u64>,
    held: BTreeMap<Id, Held>,
    /// Bodies' and journals' encoded sizes, by body hash.
    sizes: HashMap<String, u64>,
    /// Parts asked for by the request in progress.
    pinned: BTreeSet<Id>,
    /// Parts' rebuilds let go so far.
    pub evictions: u64,
    /// Whether what must be held has passed the budget, and been warned of.
    pub too_small: bool,
    /// What is held, once worked out, until something changes it: a
    /// request that builds nothing doesn't count 10,000 parts again.
    held_now: std::cell::Cell<Option<u64>>,
}

impl Budget {
    /// Whether there is a budget to keep.
    pub fn on(&self) -> bool {
        self.max > 0
    }

    /// What is held, if worked out since it last changed.
    pub fn cached(&self) -> Option<u64> {
        self.held_now.get()
    }

    /// Notes what is held, until it changes.
    pub fn cache(&self, held: u64) {
        self.held_now.set(Some(held));
    }

    /// What is held may have changed.
    pub fn changed(&self) {
        self.held_now.set(None);
    }

    /// Notes that `part` was just used.
    pub fn touch(&mut self, part: &Id) {
        if self.on() {
            self.clock += 1;
            self.used.insert(part.clone(), self.clock);
        }
    }

    /// Keeps `part`'s rebuild until the request in progress answers.
    pub fn pin(&mut self, part: &Id) {
        if self.on() {
            self.pinned.insert(part.clone());
        }
    }

    /// The request answered: nothing is pinned.
    pub fn unpin(&mut self) {
        self.pinned.clear();
    }

    fn size(&mut self, b: &Built) -> u64 {
        if let Some(n) = self.sizes.get(&b.hash) {
            return *n;
        }
        let n = RESIDENT
            * (ryukei::codec::encode_body(&b.body).len()
                + ryukei::codec::encode_journal(&b.journal).len()) as u64;
        self.sizes.insert(b.hash.clone(), n);
        n
    }

    /// What `m` holds in memory: its disk entry's size where it has one,
    /// else its bodies', encoded.
    fn made_size(&mut self, m: &Made) -> u64 {
        match m.encoded.get() {
            Some(n) => RESIDENT * n,
            None => m.solids.iter().map(|(_, b)| self.size(b)).sum(),
        }
    }

    /// Measures what `part`'s rebuild `result` and its `cache` hold, and
    /// of that what `shown` (the results its model shows) does.
    pub fn measure(&mut self, part: &Id, result: &PartResult, cache: &Cache, shown: &[&Made]) {
        if !self.on() {
            return;
        }
        let mut seen = BTreeSet::new();
        let mut all = 0;
        let made: Vec<Arc<Made>> = result
            .features
            .iter()
            .map(|(_, r)| r)
            .chain(cache.features.values().map(|(_, r)| r))
            .filter_map(|r| match r {
                Ok(Output::Solids(m)) => Some(Arc::clone(m)),
                _ => None,
            })
            .collect();
        for m in made {
            if seen.insert(Arc::as_ptr(&m)) {
                all += self.made_size(&m);
            }
        }
        let tools: Vec<Arc<Built>> = cache.tools.values().map(|(_, b)| Arc::clone(b)).collect();
        for b in tools {
            all += self.size(&b);
        }
        let shown = shown.iter().map(|m| self.made_size(m)).sum();
        self.held.insert(part.clone(), Held { all, shown });
        self.changed();
    }

    /// Forgets parts the project no longer has.
    pub fn retain(&mut self, live: impl Fn(&Id) -> bool) {
        self.held.retain(|id, _| live(id));
        self.used.retain(|id, _| live(id));
        self.changed();
    }

    /// What is held: the rebuilds of the parts in `results`, the shown
    /// bodies of the other parts whose models are kept (`models`), and
    /// `extra` (display meshes).
    pub fn total<'a>(
        &self,
        results: impl Iterator<Item = &'a Id>,
        models: impl Iterator<Item = &'a Id>,
        extra: u64,
    ) -> u64 {
        let results: BTreeSet<&Id> = results.collect();
        let rebuilt: u64 = results
            .iter()
            .filter_map(|id| self.held.get(*id))
            .map(|h| h.all)
            .sum();
        let shown: u64 = models
            .filter(|id| !results.contains(id))
            .filter_map(|id| self.held.get(id))
            .map(|h| h.shown)
            .sum();
        rebuilt + shown + extra
    }

    /// The parts among `candidates` (parts whose rebuilds may go) to let
    /// go, least recently used first, so what is held (`total`) comes
    /// within the budget; and notes whether even all of them don't.
    pub fn evict(&mut self, candidates: impl Iterator<Item = Id>, mut total: u64) -> Vec<Id> {
        if !self.on() || total <= self.max {
            return Vec::new();
        }
        let mut order: Vec<(u64, Id)> = candidates
            .filter(|id| !self.pinned.contains(id))
            .map(|id| (self.used.get(&id).copied().unwrap_or(0), id))
            .collect();
        order.sort();
        let mut out = Vec::new();
        for (_, id) in order {
            if total <= self.max {
                break;
            }
            let h = self.held.get(&id).copied().unwrap_or_default();
            total = total.saturating_sub(h.all - h.shown.min(h.all));
            out.push(id);
        }
        self.evictions += out.len() as u64;
        if !out.is_empty() {
            self.changed();
        }
        if total > self.max && !self.too_small {
            self.too_small = true;
            eprintln!(
                "ryucad: memory_budget_too_small: what must be held ({} MiB) is more than \
                 session.max_memory ({} MiB)",
                total >> 20,
                self.max >> 20
            );
        }
        out
    }

    pub fn to_json(&self, held: u64) -> serde_json::Value {
        let mut v = serde_json::json!({
            "max_bytes": self.max,
            "held_bytes": held,
            "evictions": self.evictions,
        });
        if self.too_small {
            v["code"] = serde_json::json!(crate::error::Code::MemoryBudgetTooSmall);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    #[test]
    fn the_least_recently_used_go_first_and_pinned_parts_stay() {
        let mut b = Budget {
            max: 100,
            ..Budget::default()
        };
        for (p, all, shown) in [("a", 60, 10), ("b", 60, 20), ("c", 60, 0)] {
            b.held.insert(id(p), Held { all, shown });
            b.touch(&id(p));
        }
        b.touch(&id("a"));
        b.pin(&id("b"));
        let parts = [id("a"), id("b"), id("c")];
        let total = b.total(parts.iter(), parts.iter(), 0);
        assert_eq!(total, 180);
        // c is the least recently used; b is pinned; then a.
        assert_eq!(b.evict(parts.iter().cloned(), total), [id("c"), id("a")]);
        assert!(!b.too_small);
        // What is left: b whole and a's shown bodies.
        let left = [id("b")];
        assert_eq!(b.total(left.iter(), parts.iter(), 0), 70);
        // A budget smaller than what must be held says so, once.
        b.max = 10;
        b.unpin();
        assert_eq!(b.evict(left.iter().cloned(), 200), [id("b")]);
        assert!(b.too_small);
    }

    #[test]
    fn no_budget_measures_and_evicts_nothing() {
        let mut b = Budget::default();
        b.touch(&id("a"));
        assert!(b.used.is_empty());
        assert!(b.evict([id("a")].into_iter(), u64::MAX).is_empty());
        assert!(default_max() >= 512 << 20 && default_max() <= 8192 << 20);
    }
}
