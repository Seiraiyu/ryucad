//! A loaded project and its rebuilt parts, kept fresh against the files.
//!
//! The files are the truth. Before every request the session checks each
//! file it depends on: an unchanged size and modification time is trusted
//! unless the time is too recent to be reliable; otherwise the content is
//! read again. A real change reloads only the parts it lies in (all of
//! them when the manifest changed). The session keeps every file's bytes,
//! so the activity log's snapshots cost no reading. Rebuilds are cached by
//! part revision, so only parts whose files changed are rebuilt.
//!
//! A daemon's session watches the folder ([`Session::with_watch`]): file
//! events name what may have changed, and only those paths are checked,
//! with every file checked at the start, when events overflow or fail,
//! and every `daemon.rescan_s` as a safety net. A folder that can't be
//! watched is checked whole on every request, as before.
//!
//! A request may name a Git revision instead of the working files: the
//! session then answers from a frozen session over that revision's files,
//! one of the few it keeps, which starts from the working files' results,
//! so only what differs is rebuilt.

use crate::activity::Files;
use crate::assembly::{Model, Pins, Rebuild, Store};
use crate::doc::assembly::Rev;
use crate::doc::id::Id;
use crate::doc::project::{Part, Project};
use crate::error::{Error, Result};
use crate::rebuild::PartResult;
use ryu_session::fresh::{Changes, Stamps, Watch, WatchStats, read_dir};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Set, a daemon doesn't watch its folder and checks every file on every
/// request (tests compare both ways; a filesystem whose events lie can be
/// told).
pub const NO_WATCH_VAR: &str = "RYUCAD_NO_WATCH";

/// How many revisions a session keeps loaded beside the working files.
pub const REVISIONS: usize = 4;

pub struct Session {
    project: Project,
    stamps: Stamps,
    /// Each part's rebuild, and its results by feature keyed by their
    /// inputs' hash, so a rebuild reuses every feature whose inputs haven't
    /// changed; the same for parts read at release labels.
    pub(crate) store: Store,
    /// Parts as release labels have them, loaded from the commit where
    /// each record became official, by part and commit.
    pinned_parts: BTreeMap<(Id, String), Part>,
    /// What each name of a feature's body was, by part and feature, from
    /// every build this session has seen: its kind and, for a sweep, where
    /// it came from. Names are only added, so a reference whose name has
    /// gone can still say what it named.
    known: BTreeMap<(Id, Id), BTreeMap<ryukei::name::Name, Known>>,
    /// Display meshes of rebuilt bodies.
    pub meshes: crate::mesh::Cache,
    /// What intersecting pairs of placed solids found, for `interference`.
    pub interference: crate::interference::Cache,
    /// Mass properties worked out, for `mass`.
    pub mass: crate::mass::Cache,
    /// Distances between placed solids, for `clearance`.
    pub clearance: crate::clearance::Cache,
    /// A revision's session: its files never change, and it never reads
    /// the disk.
    frozen: bool,
    /// The revisions requests have named, by the project folder's tree,
    /// most recently used first.
    revisions: Vec<(String, Session)>,
    /// The last merge status, by the key of what it was computed from.
    pub(crate) merge_status: Option<(String, serde_json::Value)>,
    /// The release branch as last read, by the key of the refs it was
    /// read from (see [`crate::release::history`]).
    pub(crate) releases: Option<(String, ryupdm::history::Releases)>,
    /// Bodies of released content as this build rebuilds them, by content
    /// hash, for `release_result_changed`.
    pub(crate) release_results: BTreeMap<String, BTreeMap<String, String>>,
    /// Parts read at commits for BOMs and where-used, by commit and part.
    pub(crate) structure: ryupdm::structure::Cache,
    /// A daemon's watch of the folder: `None` checks every file on
    /// every request.
    watching: Option<Watching>,
    /// Counts the times the files changed: what subscribers were told is
    /// still news while it hasn't moved.
    epoch: u64,
    /// The project's cycles of assemblies, by the epoch and the pins they
    /// were found from.
    cycles: Option<(u64, String, Arc<crate::assembly::Cycles>)>,
    /// Other worktrees' project files as last read, by their project
    /// folders: what a compare, diff or merge preview with a worktree
    /// reads, checked as this session's own files are.
    worktrees: BTreeMap<PathBuf, Stamps>,
    /// How many project files each worktree's working files change from
    /// its head, by the head and a digest of the files.
    pub(crate) worktree_changes: BTreeMap<String, usize>,
    /// The last merge previews, by the key of what each was made from,
    /// most recently made first.
    pub(crate) previews: Vec<(String, serde_json::Value)>,
}

/// How a revision names another worktree's working files:
/// `worktree:<path>`, the path absolute (its work tree or the project
/// folder in it).
pub const WORKTREE: &str = "worktree:";

/// The path a `worktree:<path>` revision names, if it is one.
pub fn worktree_path(rev: &str) -> Option<&Path> {
    rev.strip_prefix(WORKTREE).map(Path::new)
}

/// A daemon's watch of its project, or why it has none, and what it
/// counted.
struct Watching {
    watch: std::result::Result<Watch, String>,
    last_full: Instant,
    every: Duration,
    stats: WatchStats,
}

impl Session {
    /// Loads the project at `root`.
    pub fn open(root: &Path) -> Result<Session> {
        let mut s = Session {
            project: Project::load(root)?,
            stamps: Stamps::default(),
            store: Store::default(),
            pinned_parts: BTreeMap::new(),
            known: BTreeMap::new(),
            meshes: crate::mesh::Cache::default(),
            interference: crate::interference::Cache::default(),
            mass: crate::mass::Cache::default(),
            clearance: crate::clearance::Cache::default(),
            frozen: false,
            revisions: Vec::new(),
            merge_status: None,
            releases: None,
            release_results: BTreeMap::new(),
            structure: ryupdm::structure::Cache::new(),
            watching: None,
            epoch: 0,
            cycles: None,
            worktrees: BTreeMap::new(),
            worktree_changes: BTreeMap::new(),
            previews: Vec::new(),
        };
        s.scan()?;
        Ok(s)
    }

    /// The session, watching its folder for file events, with its markers
    /// in `markers` (outside the project): what a daemon's session does.
    /// A folder that can't be watched is checked whole on every request,
    /// and the reason (`watch_unavailable`) is logged and given in
    /// `daemon status`.
    pub fn with_watch(self, markers: &Path) -> Session {
        let watch = match std::env::var_os(NO_WATCH_VAR) {
            Some(_) => Err(format!("{NO_WATCH_VAR} is set")),
            None => Watch::start(self.root(), markers, tracked),
        };
        self.watched(watch)
    }

    /// The session with `watch`, or why it has none.
    fn watched(mut self, watch: std::result::Result<Watch, String>) -> Session {
        if let Err(why) = &watch {
            eprintln!(
                "ryucad: watch_unavailable: can't watch {} ({why}); checking every file on \
                 every request",
                self.root().display()
            );
        }
        let every = crate::config::user("daemon.rescan_s")
            .as_u64()
            .map_or(ryu_session::fresh::RESCAN, Duration::from_secs);
        self.watching = Some(Watching {
            watch,
            last_full: Instant::now(),
            every,
            stats: WatchStats::default(),
        });
        self
    }

    /// The session holding at most `max` bytes of parts' rebuilds and
    /// display meshes between requests (see [`crate::budget`]).
    pub fn with_budget(mut self, max: u64) -> Session {
        self.store.budget.max = max;
        self
    }

    /// Lets go of what the budget doesn't cover, once a request has
    /// answered: past it, the revisions' sessions let go of their parts'
    /// rebuilds first, then this one of its parts', least recently used
    /// first.
    pub fn trim(&mut self) {
        self.store.budget.unpin();
        if !self.store.budget.on() {
            return;
        }
        let meshes = self.meshes.bytes() as u64;
        if self.store.held(&self.project, meshes) > self.store.budget.max {
            for (_, r) in &mut self.revisions {
                let project = &r.project;
                r.store
                    .results
                    .retain(|id, _| !crate::assembly::is_leaf(project, id));
                r.store.caches.clear();
            }
        }
        self.store.trim(&self.project, meshes);
    }

    /// The budget's state for `daemon status`: its bound, what it holds,
    /// how many parts' rebuilds it has let go, and how many it holds.
    pub fn budget_status(&self) -> Value {
        let held = self.store.held(&self.project, self.meshes.bytes() as u64);
        let mut v = self.store.budget.to_json(held);
        v["parts_held"] = json!(
            self.store
                .results
                .keys()
                .filter(|id| crate::assembly::is_leaf(&self.project, id))
                .count()
        );
        v["meshes_bytes"] = json!(self.meshes.bytes());
        v
    }

    /// What `part`'s rebuild was at its revision, if this session built
    /// it: its hash and how many of its features failed.
    pub fn summary(&self, part: &Id) -> Option<(&str, usize)> {
        let rev = &self.project.parts.get(part)?.revision;
        let (key, hash, failed) = self.store.summaries.get(part)?;
        (key.split('+').next() == Some(rev.as_str())).then_some((hash.as_str(), *failed))
    }

    /// The watch's state for `daemon status`, if this session watches.
    pub fn watch_status(&self) -> Option<Value> {
        let w = self.watching.as_ref()?;
        Some(w.stats.to_json(w.watch.as_ref().err().map(String::as_str)))
    }

    /// The session with `disk` beneath its caches: results it doesn't
    /// hold are read from it, and what it builds is written to it.
    pub fn with_disk(mut self, disk: Option<Arc<crate::cache::Disk>>) -> Session {
        self.meshes.disk = disk.clone();
        self.store.disk = disk;
        self
    }

    /// The disk cache beneath this session's, if any.
    pub fn disk(&self) -> Option<&Arc<crate::cache::Disk>> {
        self.store.disk.as_ref()
    }

    /// A session over `files`, the project as a revision has it, starting
    /// from what `from` has built and learnt.
    fn frozen(from: &Session, files: Files) -> Result<Session> {
        let root = from.root().to_path_buf();
        let project = Project::load_from(&root, &files)?;
        let stamps = Stamps::frozen(&root, files);
        Ok(Session {
            project,
            stamps,
            store: Store {
                results: BTreeMap::new(),
                caches: from.store.caches.clone(),
                pinned: from.store.pinned.clone(),
                pinned_caches: from.store.pinned_caches.clone(),
                disk: from.store.disk.clone(),
                // Keyed by what they were built from, so a revision
                // reuses those its files agree with.
                models: from.store.models.clone(),
                summaries: BTreeMap::new(),
                // Its owner lets its rebuilds go (see `trim`).
                budget: Default::default(),
            },
            pinned_parts: from.pinned_parts.clone(),
            known: from.known.clone(),
            meshes: crate::mesh::Cache::with_disk(from.store.disk.clone()),
            interference: from.interference.clone(),
            mass: from.mass.clone(),
            clearance: from.clearance.clone(),
            frozen: true,
            revisions: Vec::new(),
            merge_status: None,
            releases: None,
            release_results: BTreeMap::new(),
            structure: from.structure.clone(),
            watching: None,
            epoch: 0,
            cycles: None,
            worktrees: BTreeMap::new(),
            worktree_changes: BTreeMap::new(),
            previews: Vec::new(),
        })
    }

    /// The session to answer from for `rev`: this one for the working
    /// files (no revision, or `working`), else one over the project as that
    /// Git revision has it, with the commit it names.
    pub fn at(&mut self, rev: Option<&str>) -> Result<(Option<String>, &mut Session)> {
        let Some(rev) = rev.filter(|r| *r != "working") else {
            return Ok((None, self));
        };
        if worktree_path(rev).is_some() {
            return self.at_worktree(rev).map(|s| (None, s));
        }
        let repo = crate::git::repo(self.root())?;
        let (commit, tree) = repo.locate(rev)?;
        if tree.is_empty() {
            return Err(Error::new(
                crate::error::Code::NotFound,
                format!("{rev} has no project at {}", self.root().display()),
            )
            .with("rev", rev)
            .with("commit", &commit));
        }
        let i = match self.revisions.iter().position(|(t, _)| *t == tree) {
            Some(i) => i,
            None => {
                let s = Session::frozen(self, repo.files(&commit)?)?;
                self.revisions.push((tree, s));
                self.revisions.len() - 1
            }
        };
        let used = self.revisions.remove(i);
        self.revisions.insert(0, used);
        self.revisions.truncate(REVISIONS);
        Ok((Some(commit), &mut self.revisions[0].1))
    }

    /// The worktree a `worktree:<path>` revision names, and its project
    /// files now: checked as this session checks its own (a file read
    /// again only when its size or time moved), never through that
    /// worktree's daemon. `None` for any other revision; `worktree_not_found`
    /// for a path that isn't a worktree holding the project.
    pub fn worktree_files(
        &mut self,
        rev: &str,
    ) -> Result<Option<(crate::git::Worktree, Files, String)>> {
        let Some(path) = worktree_path(rev) else {
            return Ok(None);
        };
        if !path.is_absolute() {
            return Err(Error::new(
                crate::error::Code::InvalidParam,
                format!(
                    "a worktree is named by an absolute path, not {}",
                    path.display()
                ),
            )
            .with("rev", rev));
        }
        let repo = crate::git::repo(self.root())?;
        let w = repo.worktree(path)?;
        if w.current && !self.frozen {
            return Ok(Some((w, self.snapshot(), self.digest())));
        }
        let stamps = self.worktrees.entry(w.root.clone()).or_default();
        stamps.scan(&w.root, listed(&w.root)?)?;
        let files = stamps.snapshot(&w.root);
        let digest = stamps.digest();
        Ok(Some((w, files, digest)))
    }

    /// The session over another worktree's working files: kept with the
    /// revisions while its files stay as they are, and made again (from
    /// this session's results) when they change.
    fn at_worktree(&mut self, rev: &str) -> Result<&mut Session> {
        let (w, files, digest) = self.worktree_files(rev)?.expect("a worktree");
        if w.current {
            return Ok(self);
        }
        let mine = format!("{WORKTREE}{}\0", w.root.display());
        let key = format!("{mine}{digest}");
        self.revisions
            .retain(|(t, _)| !t.starts_with(&mine) || *t == key);
        let i = match self.revisions.iter().position(|(t, _)| *t == key) {
            Some(i) => i,
            None => {
                let s = Session::frozen(self, files)?;
                self.revisions.push((key, s));
                self.revisions.len() - 1
            }
        };
        let used = self.revisions.remove(i);
        self.revisions.insert(0, used);
        self.revisions.truncate(REVISIONS);
        Ok(&mut self.revisions[0].1)
    }

    pub fn root(&self) -> &Path {
        &self.project.root
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Brings the session up to date with the files. Returns whether the
    /// project was reloaded.
    pub fn refresh(&mut self) -> Result<bool> {
        Ok(!self.refresh_changes()?.is_empty())
    }

    /// [`Session::refresh`], returning the files that changed (relative,
    /// with `/`) and how: `created`, `modified` or `deleted`.
    pub fn refresh_changes(&mut self) -> Result<Vec<(String, &'static str)>> {
        if self.frozen {
            return Ok(Vec::new());
        }
        let changes = self.scan()?;
        if !changes.is_empty() {
            self.epoch += 1;
            let paths: Vec<String> = changes.iter().map(|(p, _)| p.clone()).collect();
            let held = Held {
                root: &self.project.root.clone(),
                stamps: &self.stamps,
            };
            self.project.reload_from(&held, &paths)?;
            let live = &self.project.parts;
            // An assembly's key is its revision, then its children's.
            self.store.results.retain(|id, (key, _)| {
                live.get(id)
                    .is_some_and(|p| key.split('+').next() == Some(p.revision.as_str()))
            });
            self.store.caches.retain(|id, _| live.contains_key(id));
            self.store.budget.changed();
            let current = |id: &Id, key: &str| {
                live.get(id)
                    .is_some_and(|p| key.split('+').next() == Some(p.revision.as_str()))
            };
            self.store.models.retain(|id, (key, _)| current(id, key));
            self.store
                .summaries
                .retain(|id, (key, ..)| current(id, key));
            self.store.budget.retain(|id| live.contains_key(id));
        }
        Ok(changes)
    }

    /// How many times the files have changed under this session.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Each assembly's key for subscribers, for those that build: its
    /// revision, its rebuild's hash and the pins of the labels it places,
    /// all from one rebuild.
    pub(crate) fn assembly_keys(&mut self) -> BTreeMap<Id, String> {
        let assemblies: Vec<Id> = self
            .project
            .parts
            .iter()
            .filter(|(_, p)| {
                p.file
                    .as_ref()
                    .is_ok_and(|f| f.kind == crate::doc::files::PartKind::Assembly)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut out = BTreeMap::new();
        let _ = self.rebuild(|r| {
            for id in assemblies {
                let rev = r.project().parts[&id].revision.clone();
                let Ok(res) = r.result(&id) else { continue };
                let pins: Vec<String> = res
                    .features
                    .iter()
                    .filter_map(|(_, o)| o.as_ref().ok()?.instance()?.model.pin.as_ref())
                    .map(|p| format!("{p:?}"))
                    .collect();
                out.insert(id.clone(), format!("{rev}+{}+{}", res.hash, pins.join(",")));
            }
            Ok(())
        });
        out
    }

    /// Takes in what this session just wrote. Every write goes through here
    /// before it answers, so the project, the stamps and the snapshot
    /// agree with the files.
    pub fn reload(&mut self) -> Result<()> {
        self.refresh_changes().map(drop)
    }

    /// Every file the project's state depends on, as last read: what the
    /// activity log keeps of a change.
    pub fn snapshot(&self) -> Files {
        self.stamps.snapshot(self.root())
    }

    /// A hash of every file the project's state depends on, as last read.
    pub fn digest(&self) -> String {
        self.stamps.digest()
    }

    /// The rebuild of `part`, from the cache when nothing it reads has
    /// changed: its files and, for an assembly, the models of what it
    /// places, rebuilt first.
    pub fn rebuilt(&mut self, part: &Id) -> Result<&PartResult> {
        self.project.part(part)?;
        self.store.budget.pin(part);
        self.rebuild(|r| r.result(part).map(|_| ()))?;
        Ok(&self.store.results[part].1)
    }

    /// The rebuild of `part`, a proposed edit of one of the project's
    /// parts, over this session's results for everything else; kept
    /// nowhere, so the session is as it was.
    pub fn rebuilt_as(&mut self, part: Part) -> Result<PartResult> {
        let pins = self.pins()?;
        let id = part.id.clone();
        let mut project = self.project.clone();
        project.parts.insert(id.clone(), part);
        let mut store = self.store.clone();
        // Its revision is the files', which the edit hasn't changed.
        store.results.remove(&id);
        let mut r = Rebuild::new(&project, &pins, store);
        Ok(r.result(&id)?.clone())
    }

    /// The model of `part` at `rev`: what an instance of it places.
    pub fn model(&mut self, part: &Id, rev: &Rev) -> Result<Arc<Model>> {
        let mut out = None;
        self.rebuild(|r| {
            out = Some(r.model(part, rev));
            Ok(())
        })?;
        out.expect("asked")
    }

    /// A session over the project's files with `writes` laid over them
    /// (bytes, or `None` for a file deleted), starting from what this one
    /// has built and learnt: what a write would leave, rebuilt without
    /// writing anything.
    pub(crate) fn proposed(&self, writes: &BTreeMap<String, Option<Vec<u8>>>) -> Result<Session> {
        let mut files = self.snapshot();
        for (path, bytes) in writes {
            match bytes {
                Some(b) => files.insert(path.clone(), b.as_slice().into()),
                None => files.remove(path),
            };
        }
        self.over(files)
    }

    /// A session over `files` (a merge made in memory, say), starting from
    /// what this one has built and learnt, its parts' results taken where
    /// their revisions agree.
    pub(crate) fn over(&self, files: Files) -> Result<Session> {
        let mut s = Session::frozen(self, files)?;
        // Its parts' results are this session's while their revisions
        // are.
        for (id, (key, r)) in &self.store.results {
            if s.project
                .parts
                .get(id)
                .is_some_and(|p| key.split('+').next() == Some(&p.revision))
            {
                s.store.results.insert(id.clone(), (key.clone(), r.clone()));
            }
        }
        Ok(s)
    }

    /// Rebuilds those of `parts` that aren't assemblies side by side,
    /// keeping each as [`Session::rebuilt`] would: an import of many parts
    /// heals their bodies in parallel.
    pub fn rebuild_parts(&mut self, parts: &[Id]) -> Result<()> {
        self.rebuild(|r| {
            r.build_parts(parts);
            Ok(())
        })
    }

    /// `part` at `rev` as rebuilt, every part it places first: its files,
    /// its rebuild, and for a release label how the label resolved. A
    /// label no instance places is looked up too.
    pub fn node(
        &mut self,
        part: &Id,
        rev: &Rev,
    ) -> Result<(Part, PartResult, Option<crate::assembly::Pin>)> {
        self.store.budget.pin(part);
        let extra = match rev {
            Rev::Working => Vec::new(),
            Rev::Label(l) => vec![(part.clone(), l.clone())],
        };
        let mut out = None;
        self.rebuild_with(extra, |r| {
            out = Some(r.node(part, rev));
            Ok(())
        })?;
        out.expect("asked")
    }

    /// [`Session::node`] for `part` at `rev` and for every part and
    /// revision it places, at any depth, all in one rebuild: what an
    /// export of an assembly of thousands of parts walks.
    #[allow(clippy::type_complexity)]
    pub fn nodes(
        &mut self,
        part: &Id,
        rev: &Rev,
    ) -> Result<BTreeMap<(Id, Rev), (Part, PartResult, Option<crate::assembly::Pin>)>> {
        let extra = match rev {
            Rev::Working => Vec::new(),
            Rev::Label(l) => vec![(part.clone(), l.clone())],
        };
        let mut out = BTreeMap::new();
        self.rebuild_with(extra, |r| {
            let mut todo = vec![(part.clone(), rev.clone())];
            while let Some(at) = todo.pop() {
                if out.contains_key(&at) {
                    continue;
                }
                let node = r.node(&at.0, &at.1)?;
                for (_, f) in &node.1.features {
                    match f {
                        Ok(crate::rebuild::Output::Instance(p)) => {
                            todo.push((p.part.clone(), p.rev.clone()));
                        }
                        Ok(crate::rebuild::Output::Copies(c)) => todo.extend(
                            c.copies
                                .iter()
                                .map(|c| (c.placed.part.clone(), c.placed.rev.clone())),
                        ),
                        _ => {}
                    }
                }
                out.insert(at, node);
            }
            Ok(())
        })?;
        Ok(out)
    }

    /// Runs `f` over an [`assembly::Rebuild`](crate::assembly::Rebuild)
    /// holding this session's results and caches, and keeps what it
    /// built: each part it rebuilt with its errors explained and its names
    /// learnt.
    fn rebuild(&mut self, f: impl FnOnce(&mut Rebuild) -> Result<()>) -> Result<()> {
        self.rebuild_with(Vec::new(), f)
    }

    /// [`Session::rebuild`], with the release labels `extra` looked up
    /// beside those the project's instances reach.
    fn rebuild_with(
        &mut self,
        extra: Vec<(Id, String)>,
        f: impl FnOnce(&mut Rebuild) -> Result<()>,
    ) -> Result<()> {
        let pins = if extra.is_empty() {
            self.pins()?
        } else {
            let project = self.project.clone();
            crate::assembly::pins_from(&project, extra, |part, label| {
                crate::release::pinned(self, part, label)
            })
        };
        // Found again only when the files or the labels' pins move.
        let key: String = pins
            .iter()
            .map(|((part, label), p)| match p {
                Ok(p) => format!("{part}@{label}:{:?};", p.pin.commit),
                Err(e) => format!("{part}@{label}:{:?};", e.code),
            })
            .collect();
        let cycles = match &self.cycles {
            Some((epoch, k, c)) if *epoch == self.epoch && *k == key => Arc::clone(c),
            _ => {
                let c = Arc::new(crate::assembly::Cycles::of(&self.project, &pins));
                self.cycles = Some((self.epoch, key, Arc::clone(&c)));
                c
            }
        };
        let store = std::mem::take(&mut self.store);
        let mut r = Rebuild::with_cycles(&self.project, &pins, store, cycles);
        let out = f(&mut r);
        let fresh = std::mem::take(&mut r.fresh);
        let learned = std::mem::take(&mut r.learned);
        self.store = r.store;
        self.keep_fresh(fresh, learned);
        out
    }

    /// Learns the names of the parts just rebuilt, and explains their
    /// failures against these results.
    fn keep_fresh(&mut self, fresh: Vec<Id>, learned: Vec<Name>) {
        // Parts whose rebuilds the budget let go learnt their names first.
        self.learn_names(learned);
        for id in fresh {
            let (key, result) = self.store.results.remove(&id).expect("rebuilt");
            self.learn(&id, &result);
            self.store.results.insert(id.clone(), (key, result));
            // Explained against this rebuild, not the one before: which
            // feature took a name away is read from these results.
            let explained: Vec<(usize, Error)> = self.store.results[&id]
                .1
                .features
                .iter()
                .enumerate()
                .filter_map(|(i, (_, r))| Some((i, self.explain(&id, r.as_ref().err()?.clone()))))
                .collect();
            let result = &mut self.store.results.get_mut(&id).expect("just put").1;
            for (i, e) in explained {
                result.features[i].1 = Err(e);
            }
        }
    }

    /// Every release label the project's instances reach, looked up.
    pub(crate) fn pins(&mut self) -> Result<Pins> {
        let mut labels = false;
        for p in self.project.parts.values() {
            labels |= p.features.values().any(|f| {
                matches!(f, Ok(crate::doc::files::Feature::Instance(i)) if i.rev != Rev::Working)
            });
        }
        if !labels {
            return Ok(Pins::new());
        }
        let project = self.project.clone();
        Ok(crate::assembly::pins(&project, |part, label| {
            crate::release::pinned(self, part, label)
        }))
    }

    /// `part` as the project's files at `commit` have it, loaded once.
    pub(crate) fn pinned_part(
        &mut self,
        repo: &crate::git::Repo,
        part: &Id,
        commit: &str,
    ) -> Result<Part> {
        let at = (part.clone(), commit.to_owned());
        if let Some(p) = self.pinned_parts.get(&at) {
            return Ok(p.clone());
        }
        let mine = format!("parts/{part}/");
        let all = repo.files(commit)?;
        // The imported files its import features name, with its own.
        let named: std::collections::BTreeSet<String> = all
            .iter()
            .filter(|(path, _)| path.starts_with(&mine))
            .filter_map(|(_, bytes)| serde_json::from_slice::<Value>(bytes).ok())
            .filter(|v| v["type"] == "import")
            .filter_map(|v| v["file"]["path"].as_str().map(str::to_owned))
            .collect();
        let files: Files = all
            .into_iter()
            .filter(|(path, _)| {
                path == crate::doc::project::MANIFEST
                    || path.starts_with(&mine)
                    || named.contains(path)
            })
            .collect();
        let mut project = Project::load_from(self.root(), &files)?;
        let p = project.parts.remove(part).ok_or_else(|| {
            Error::new(
                crate::error::Code::NotFound,
                format!("{part} isn't in the project at {commit}"),
            )
            .with("part", part)
            .with("commit", commit)
        })?;
        self.pinned_parts.insert(at, p.clone());
        Ok(p)
    }

    /// The cached rebuild of `part`, to make it wrong in tests.
    #[cfg(test)]
    pub(crate) fn cached_mut(&mut self, part: &Id) -> Option<&mut PartResult> {
        self.store.results.get_mut(part).map(|(_, r)| r)
    }

    /// Adds to `reference_unresolved` what its name was when it last
    /// resolved (`was`: `{kind, from}`), if this session saw it, and
    /// which feature of its solid's chain took it away (`removed_by`):
    /// one that dropped it after the feature before it had it, or the
    /// feature that made it (`was.from.feature`) when that one no longer
    /// does (its sketch drawn again with new IDs).
    pub fn explain(&self, part: &Id, e: Error) -> Error {
        let order = self
            .project
            .parts
            .get(part)
            .map(|p| p.order().to_vec())
            .unwrap_or_default();
        self.explain_in(part, &order, e)
    }

    /// [`Session::explain`] for the part's list as `order` has it: as a
    /// write would leave it.
    pub fn explain_in(&self, part: &Id, order: &[Id], mut e: Error) -> Error {
        if e.code != crate::error::Code::ReferenceUnresolved {
            return e;
        }
        if e.details["connector"].is_string() {
            self.connector_candidates(part, &mut e);
            return e;
        }
        let dependency = e.details["dependency"]
            .as_str()
            .and_then(|d| Id::parse(d, "feature").ok());
        let name = e.details["reference"]
            .as_str()
            .and_then(|r| r.rsplit('/').next())
            .map(str::to_owned);
        let name = name.as_deref();
        let was = dependency
            .as_ref()
            .zip(name.and_then(crate::doc::reference::parse_name))
            .and_then(|(d, n)| self.known.get(&(part.clone(), d.clone()))?.get(&n))
            .map(Known::to_json);
        if let Some(w) = &was {
            e.details["was"] = w.clone();
        }
        let reader = e.details["feature"]
            .as_str()
            .and_then(|f| Id::parse(f, "feature").ok());
        let maker = was
            .as_ref()
            .and_then(|w| w["from"]["feature"].as_str())
            .and_then(|f| Id::parse(f, "feature").ok());
        let lost = dependency
            .zip(name.and_then(crate::doc::reference::parse_name))
            .zip(reader);
        let removed = lost.as_ref().and_then(|((solid, name), reader)| {
            self.removed_by(part, order, solid, *name, reader, maker.as_ref())
        });
        if let Some(by) = removed {
            e.details["removed_by"] = serde_json::json!(by);
        }
        // What the name became, written as the reference was (issue #271).
        let candidates = lost
            .and_then(|((solid, name), reader)| {
                let p = self.project.parts.get(part)?;
                let last = crate::solid::Solids::of_order(p, order)
                    .view(&solid, &reader)?
                    .clone();
                Some(self.candidates(part, order, &solid, name, &last))
            })
            .unwrap_or_default();
        let prefix = e.details["reference"]
            .as_str()
            .and_then(|r| r.rsplit_once('/'))
            .map(|(p, _)| p.to_owned());
        if let (Some(prefix), false) = (prefix, candidates.is_empty()) {
            e.details["candidates"] = serde_json::json!(
                candidates
                    .iter()
                    .map(|n| format!("{prefix}/{n}"))
                    .collect::<Vec<_>>()
            );
        }
        e
    }

    /// A mate's connector whose entity is gone from the part an occurrence
    /// places at `working`: what it became in that solid as the part shows
    /// it, written as `<asm>/<path>/<solid>/<name>` (issue #271).
    fn connector_candidates(&self, asm: &Id, e: &mut Error) {
        if e.details["rev"] != "working" {
            return;
        }
        let (Some(text), Some(placed), Some(solid)) = (
            e.details["connector"].as_str(),
            e.details["part"]
                .as_str()
                .and_then(|p| Id::parse(p, "part").ok()),
            e.details["solid"]
                .as_str()
                .and_then(|s| Id::parse(s, "feature").ok()),
        ) else {
            return;
        };
        let Some((prefix, name)) = text.rsplit_once('/') else {
            return;
        };
        let (Some(name), Some(p)) = (
            crate::doc::reference::parse_name(name),
            self.project.parts.get(&placed),
        ) else {
            return;
        };
        let order = p.order().to_vec();
        let Some(tip) = crate::solid::Solids::of_order(p, &order)
            .chain(&solid)
            .and_then(|c| c.last().cloned())
        else {
            return;
        };
        let candidates = self.candidates(&placed, &order, &solid, name, &tip);
        if !candidates.is_empty() {
            e.details["candidates"] = serde_json::json!(
                candidates
                    .iter()
                    .map(|n| format!("{asm}/{prefix}/{n}"))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// What `name` of `solid` became through the journals of its chain up
    /// to and including `last`, in the cached rebuild.
    fn candidates(
        &self,
        part: &Id,
        order: &[Id],
        solid: &Id,
        name: ryukei::name::Name,
        last: &Id,
    ) -> Vec<ryukei::name::Name> {
        let (Some(p), Some((_, result))) =
            (self.project.parts.get(part), self.store.results.get(part))
        else {
            return Vec::new();
        };
        crate::rebuild::successors(p, order, result, solid, last, name)
    }

    /// The feature of `solid`'s chain, before `reader`, that left the
    /// solid without `name` after the one before it had it, or `maker`
    /// itself when its own result lacks it, in the cached rebuild.
    fn removed_by(
        &self,
        part: &Id,
        order: &[Id],
        solid: &Id,
        name: ryukei::name::Name,
        reader: &Id,
        maker: Option<&Id>,
    ) -> Option<Id> {
        let p = self.project.parts.get(part)?;
        let result = &self.store.results.get(part)?.1;
        let solids = crate::solid::Solids::of_order(p, order);
        let last = solids.view(solid, reader)?;
        let chain = solids.chain(solid)?;
        let has = |f: &Id| {
            result
                .output(f)
                .and_then(|o| o.solid(solid))
                .map(|b| b.body.by_name(name).is_some())
        };
        let mut had = false;
        for f in chain {
            match has(f) {
                Some(true) => had = true,
                Some(false) if had || maker == Some(f) => return Some(f.clone()),
                _ => {}
            }
            if f == last {
                break;
            }
        }
        None
    }

    /// The feature of `solid`'s whole chain that left it without the
    /// entity `name` after the feature before it had it, or the feature
    /// that made it (as this session last saw the name) when its own
    /// result lacks it, in the cached rebuild: what took away a face an
    /// appearance entry names.
    pub fn removed_from(&self, part: &Id, solid: &Id, name: &str) -> Option<Id> {
        let n = crate::doc::reference::parse_name(name)?;
        let maker = self
            .known
            .get(&(part.clone(), solid.clone()))
            .and_then(|k| k.get(&n))
            .and_then(|w| w.from.as_ref()?["feature"].as_str())
            .and_then(|f| Id::parse(f, "feature").ok());
        let p = self.project.parts.get(part)?;
        let result = &self.store.results.get(part)?.1;
        let solids = crate::solid::Solids::of(p);
        let mut had = false;
        for f in solids.chain(solid)? {
            match result
                .output(f)
                .and_then(|o| o.solid(solid))
                .map(|b| b.body.by_name(n).is_some())
            {
                Some(true) => had = true,
                Some(false) if had || maker.as_ref() == Some(f) => return Some(f.clone()),
                _ => {}
            }
        }
        None
    }

    /// Takes what the revisions' sessions have learnt of names, keeping
    /// what this session knows of a name both saw: a merge's sides built
    /// names its result has lost.
    pub(crate) fn learn_from_revisions(&mut self) {
        for (_, r) in &self.revisions {
            for (key, names) in &r.known {
                let known = self.known.entry(key.clone()).or_default();
                for (name, was) in names {
                    known.entry(*name).or_insert_with(|| was.clone());
                }
            }
        }
    }

    /// Notes what every name of the part's solids is, by solid.
    fn learn(&mut self, part: &Id, result: &PartResult) {
        self.learn_names(names_of(part, result));
    }

    fn learn_names(&mut self, names: Vec<Name>) {
        for (key, name, was) in names {
            self.known.entry(key).or_default().insert(name, was);
        }
    }

    /// Checks the files against their stamps, reading only those whose
    /// size or time moved or whose time is too recent to trust: those file
    /// events named when the session watches (every file when they can't
    /// say, and every `daemon.rescan_s`), else every file. Returns what
    /// changed, sorted.
    fn scan(&mut self) -> Result<Vec<(String, &'static str)>> {
        let root = self.project.root.clone();
        let Some(Watching {
            watch: Ok(watch),
            last_full,
            every,
            stats,
        }) = &mut self.watching
        else {
            // Every file is read anyway: drop what this process noted.
            let _ = crate::doc::source::take_written(&[root.clone()]);
            return self.stamps.scan(&root, listed(&root)?);
        };
        let named = match watch.changes() {
            Changes::All(why) => {
                stats.full += 1;
                stats.last_full = Some(why);
                *last_full = Instant::now();
                return self.stamps.scan(&root, listed(&root)?);
            }
            Changes::Paths(p) => p,
        };
        stats.narrowed += 1;
        let mut files = Vec::new();
        let mut under = Vec::new();
        for rel in &named {
            files.extend(listed_at(&root, rel)?);
            under.push(rel.split('/').fold(root.clone(), |p, s| p.join(s)));
        }
        let mut changes = self.stamps.scan_under(&root, files, &under)?;
        if last_full.elapsed() >= *every {
            // The safety net: what events missed, which a platform whose
            // events lie would show here, not as stale answers.
            *last_full = Instant::now();
            stats.full += 1;
            stats.last_full = Some("the time came to check every file");
            let missed = self.stamps.scan(&root, listed(&root)?)?;
            if !missed.is_empty() {
                if stats.missed == 0 {
                    eprintln!(
                        "ryucad: file events missed {} change(s) in {}, first {}; a full \
                         check found them",
                        missed.len(),
                        root.display(),
                        missed[0].0
                    );
                }
                stats.missed += missed.len() as u64;
                changes.extend(missed);
                changes.sort();
                changes.dedup();
            }
        }
        Ok(changes)
    }
}

/// What a name was, as a session learnt it: its kind and, where known,
/// where it came from, shared with its body's origins. Held as JSON, each
/// took about 1.6 KB; a project of 10,000 parts has millions.
#[derive(Clone, Debug)]
pub struct Known {
    kind: &'static str,
    from: Option<Arc<Value>>,
}

impl Known {
    /// `{kind, from}`, as `reference_unresolved`'s `was` says it.
    pub fn to_json(&self) -> Value {
        let mut was = json!({ "kind": self.kind });
        if let Some(from) = &self.from {
            was["from"] = (**from).clone();
        }
        was
    }
}

/// A name of a part's solid as learnt: the part and solid, the name, and
/// what it was.
pub type Name = ((Id, Id), ryukei::name::Name, Known);

/// Every name the solids of `result` (a rebuild of `part`) have, as a
/// session learns them.
pub(crate) fn names_of(part: &Id, result: &PartResult) -> Vec<Name> {
    let mut out = Vec::new();
    let solids = result
        .features
        .iter()
        .filter_map(|(_, r)| r.as_ref().ok())
        .flat_map(|o| o.solids());
    for (solid, b) in solids {
        let body = &b.body;
        let names = body
            .faces()
            .map(|f| (body.face(f).expect("live").name, "face"))
            .chain(
                body.edges()
                    .map(|e| (body.edge(e).expect("live").name, "edge")),
            )
            .chain(
                body.vertices()
                    .map(|v| (body.vertex(v).expect("live").name, "vertex")),
            );
        for (name, kind) in names {
            let from = b.origins.shared(&name.to_string()).cloned();
            out.push(((part.clone(), solid.clone()), name, Known { kind, from }));
        }
    }
    out
}

/// Whether an event's path (relative, with `/`) can matter to a project's
/// state: the root files, and anything under `parts/`, `releases/` and
/// `imports/` ([`listed`] decides which files there do).
fn tracked(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or_default();
    crate::doc::project::ROOT_FILES.contains(&rel)
        || first == "parts"
        || first == crate::doc::project::RELEASES
        || first == crate::doc::project::IMPORTS
}

/// The project's folders as they are on disk, and its files' bytes as
/// the scan just read them: a part that changed is loaded again without
/// opening every file it has (issue #25: thousands of opens per write).
struct Held<'a> {
    root: &'a Path,
    stamps: &'a Stamps,
}

impl crate::doc::source::Source for Held<'_> {
    fn list(&self, dir: &str) -> Result<Vec<(String, crate::doc::source::Kind)>> {
        crate::doc::source::Folder(self.root).list(dir)
    }

    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let path = rel
            .split('/')
            .filter(|s| !s.is_empty())
            .fold(self.root.to_path_buf(), |p, s| p.join(s));
        match self.stamps.bytes(&path) {
            Some(b) => Ok(Some(b.to_vec())),
            None => crate::doc::source::Folder(self.root).read(rel),
        }
    }
}

/// The files as last read, without copying them: what the session reads
/// on its own while polling (release states, for `pdm.changed`).
impl crate::doc::source::Source for Session {
    fn list(&self, dir: &str) -> Result<Vec<(String, crate::doc::source::Kind)>> {
        use crate::doc::source::Kind;
        let at = dir
            .split('/')
            .filter(|s| !s.is_empty())
            .fold(self.root().to_path_buf(), |p, s| p.join(s));
        let mut out: Vec<(String, Kind)> = Vec::new();
        for path in self.stamps.paths_from(&at) {
            let Ok(rest) = path.strip_prefix(&at) else {
                break;
            };
            let mut parts = rest.components();
            let Some(name) = parts.next() else {
                continue;
            };
            let kind = if parts.next().is_some() {
                Kind::Dir
            } else {
                Kind::File
            };
            let entry = (name.as_os_str().to_string_lossy().into_owned(), kind);
            // A folder's paths sort together, so its entry is the last.
            if out.last() != Some(&entry) {
                out.push(entry);
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let path = rel
            .split('/')
            .filter(|s| !s.is_empty())
            .fold(self.root().to_path_buf(), |p, s| p.join(s));
        Ok(self.stamps.bytes(&path).map(<[u8]>::to_vec))
    }

    fn read_shared(&self, rel: &str) -> Result<Option<Arc<[u8]>>> {
        let path = rel
            .split('/')
            .filter(|s| !s.is_empty())
            .fold(self.root().to_path_buf(), |p, s| p.join(s));
        Ok(self.stamps.shared(&path))
    }
}

/// The files a project's state depends on: the manifest, the attributes
/// that register ryucad with Git, ryupdm's settings, everything under
/// `parts/` two and three levels down, ryupdm's release records, two
/// levels under `releases/`, and the files under `imports/`.
pub fn files(root: &Path) -> Result<Vec<PathBuf>> {
    Ok(listed(root)?.into_iter().map(|(p, _)| p).collect())
}

/// [`files`], each with its metadata. A directory listing says what each
/// entry is (and on Windows gives its size and time) without opening it,
/// so checking a project costs at most one `stat` per file.
fn listed(root: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    let mut out = Vec::new();
    for name in crate::doc::project::ROOT_FILES {
        let path = root.join(name);
        if let Ok(m) = fs::metadata(&path)
            && m.is_file()
        {
            out.push((path, m));
        }
    }
    out.extend(parts_in(&root.join("parts"))?);
    out.extend(releases_in(&root.join(crate::doc::project::RELEASES))?);
    out.extend(files_in(&root.join(crate::doc::project::IMPORTS))?);
    Ok(out)
}

/// The files directly in `dir`.
fn files_in(dir: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    Ok(read_dir(dir)?
        .into_iter()
        .filter(|(_, m)| m.is_file())
        .collect())
}

/// The files of every part in `parts/`.
fn parts_in(dir: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    let mut out = Vec::new();
    for (part, m) in read_dir(dir)? {
        if m.is_dir() {
            out.extend(part_in(&part)?);
        }
    }
    Ok(out)
}

/// A part's files: those in its folder and in the folders in it.
fn part_in(dir: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    let mut out = Vec::new();
    for (entry, m) in read_dir(dir)? {
        if m.is_dir() {
            out.extend(files_in(&entry)?);
        } else if m.is_file() {
            out.push((entry, m));
        }
    }
    Ok(out)
}

/// The release records in `releases/`, a folder per part.
fn releases_in(dir: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    let mut out = Vec::new();
    for (part, m) in read_dir(dir)? {
        if m.is_dir() {
            out.extend(files_in(&part)?);
        }
    }
    Ok(out)
}

/// The files [`listed`] gives at or under `rel` (relative, with `/`: a
/// path a file event named, a file or a folder, there or gone).
fn listed_at(root: &Path, rel: &str) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    use crate::doc::project::{IMPORTS, RELEASES, ROOT_FILES};
    let steps: Vec<&str> = rel.split('/').collect();
    let path = steps.iter().fold(root.to_path_buf(), |p, s| p.join(s));
    let meta = fs::metadata(&path).ok();
    let (file, dir) = (
        meta.as_ref().is_some_and(fs::Metadata::is_file),
        meta.as_ref().is_some_and(fs::Metadata::is_dir),
    );
    let one = || {
        Ok(meta
            .clone()
            .map(|m| vec![(path.clone(), m)])
            .unwrap_or_default())
    };
    match steps.as_slice() {
        [name] if ROOT_FILES.contains(name) && file => one(),
        ["parts"] => parts_in(&path),
        ["parts", _] if dir => part_in(&path),
        ["parts", _, _] if dir => files_in(&path),
        ["parts", _, _] | ["parts", _, _, _] if file => one(),
        [r] if *r == RELEASES => releases_in(&path),
        [r, _] if *r == RELEASES && dir => files_in(&path),
        [r, _, _] if *r == RELEASES && file => one(),
        [i] if *i == IMPORTS => files_in(&path),
        [i, _] if *i == IMPORTS && file => one(),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{Feature, Manifest, PartFile, Placement, Primitive, PrimitiveType};
    use crate::doc::project::{MANIFEST, feature_path, part_path, write_json};
    use crate::error::Code;
    use crate::rebuild::rebuild_part;
    use crate::units::{Quantity, Unit};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    fn sphere(r: &str) -> serde_json::Value {
        Feature::Primitive(Primitive {
            ty: PrimitiveType::Sphere,
            params: [("radius".to_owned(), Quantity::parse(r, Unit::Mm).unwrap())].into(),
            placement: Placement::identity(Unit::Mm),
            combine: crate::doc::solid::Combine::NEW,
        })
        .to_json()
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_json(&root.join(MANIFEST), &Manifest::new(Unit::Mm).to_json()).unwrap();
        write_json(
            &part_path(root, &id("p")),
            &PartFile::new(vec![id("a")]).to_json(),
        )
        .unwrap();
        write_json(&feature_path(root, &id("p"), &id("a")), &sphere("1 mm")).unwrap();
        dir
    }

    fn radius(s: &mut Session) -> f64 {
        let p = s.project().part(&id("p")).unwrap();
        p.features[&id("a")]
            .as_ref()
            .unwrap()
            .primitive()
            .unwrap()
            .meters("radius")
    }

    #[test]
    fn snapshots_and_dry_runs_share_what_they_dont_change() {
        let bytes = crate::import::tests::occt("box_ap214.stp");
        let (_dir, s) = crate::import::tests::with_import(&bytes, Some(("part 2", "part 2")));
        let path = "imports/f.step";
        let snap = s.snapshot();
        assert!(Arc::ptr_eq(&snap[path], &s.snapshot()[path]), "not copied");
        // A dry run over one changed feature file holds the very bytes.
        let writes = BTreeMap::from([("parts/p/features/x.json".to_owned(), Some(b"{}".to_vec()))]);
        let proposed = s.proposed(&writes).unwrap();
        assert!(Arc::ptr_eq(&snap[path], &proposed.snapshot()[path]));
        let file = &proposed.project().imports[path];
        assert!(
            Arc::ptr_eq(&snap[path], &file.bytes),
            "loaded without a copy"
        );
    }

    #[test]
    fn sees_external_edits_even_within_the_same_tick() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        assert!(!s.refresh().unwrap());
        // Same size, written immediately: only the content hash can tell.
        write_json(
            &feature_path(dir.path(), &id("p"), &id("a")),
            &sphere("2 mm"),
        )
        .unwrap();
        assert!(s.refresh().unwrap());
        assert_eq!(radius(&mut s), 0.002);
        assert!(!s.refresh().unwrap());
    }

    /// Changes every kind of file a project has, and some it doesn't, and
    /// what a session should say changed.
    fn change_everything(root: &Path) -> Vec<(String, &'static str)> {
        write_json(&feature_path(root, &id("p"), &id("a")), &sphere("2 mm")).unwrap();
        write_json(&feature_path(root, &id("p"), &id("b")), &sphere("1 mm")).unwrap();
        fs::remove_file(root.join("parts/p/part.json")).unwrap();
        fs::write(root.join("ryupdm.json"), "{}").unwrap();
        fs::write(root.join("parts/p/properties.json"), "{}").unwrap();
        // Release records, two levels down; nothing else under releases/.
        fs::create_dir_all(root.join("releases/p/deeper")).unwrap();
        fs::write(root.join("releases/p/A.json"), "{}").unwrap();
        fs::write(root.join("releases/p/deeper/x.json"), "{}").unwrap();
        fs::write(root.join("releases/top.json"), "{}").unwrap();
        fs::write(root.join("notes.txt"), "not the project's").unwrap();
        vec![
            ("parts/p/features/a.json".to_owned(), "modified"),
            ("parts/p/features/b.json".to_owned(), "created"),
            ("parts/p/part.json".to_owned(), "deleted"),
            ("parts/p/properties.json".to_owned(), "created"),
            ("releases/p/A.json".to_owned(), "created"),
            ("ryupdm.json".to_owned(), "created"),
        ]
    }

    #[test]
    fn a_watched_session_sees_what_a_scan_does() {
        let dir = project();
        let marks = tempfile::tempdir().unwrap();
        let mut s = Session::open(dir.path())
            .unwrap()
            .with_watch(&marks.path().join("m"));
        // The first check is whole, as the watch just started.
        assert!(s.refresh_changes().unwrap().is_empty());
        let want = change_everything(dir.path());
        assert_eq!(s.refresh_changes().unwrap(), want);
        let path = feature_path(dir.path(), &id("p"), &id("a"));
        for k in 0..50 {
            write_json(&path, &sphere(&format!("{} mm", 3 + k))).unwrap();
            assert_eq!(
                s.refresh_changes().unwrap(),
                [("parts/p/features/a.json".to_owned(), "modified")],
                "{k}"
            );
            assert_eq!(radius(&mut s), (3 + k) as f64 * 1e-3);
        }
        // A part folder gone and back, as a checkout swaps it.
        let part = dir.path().join("parts/p");
        let kept = tempfile::tempdir().unwrap();
        fs::rename(&part, kept.path().join("p")).unwrap();
        let gone = s.refresh_changes().unwrap();
        assert!(gone.iter().all(|(_, c)| *c == "deleted"), "{gone:?}");
        fs::rename(kept.path().join("p"), &part).unwrap();
        let back = s.refresh_changes().unwrap();
        assert_eq!(back.len(), gone.len(), "{back:?}");
        let w = s.watch_status().unwrap();
        assert_eq!(
            (w["watching"].clone(), w["full_checks"].clone()),
            (json!(true), json!(1))
        );
        assert_eq!(w["missed"], 0);
        assert!(w["narrowed_checks"].as_u64().unwrap() >= 53, "{w}");
    }

    #[test]
    fn a_session_told_not_to_watch_checks_every_file() {
        let dir = project();
        let mut s = Session::open(dir.path())
            .unwrap()
            .watched(Err(format!("{NO_WATCH_VAR} is set")));
        let want = change_everything(dir.path());
        assert_eq!(s.refresh_changes().unwrap(), want);
        let w = s.watch_status().unwrap();
        assert_eq!(w["watching"], false);
        assert_eq!(w["unavailable"], "RYUCAD_NO_WATCH is set");
    }

    #[test]
    fn says_which_files_changed() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        let want = change_everything(dir.path());
        assert_eq!(s.refresh_changes().unwrap(), want);
        assert!(s.snapshot().contains_key("releases/p/A.json"));
        // Read in place, the session lists and reads as its snapshot does.
        use crate::doc::source::Source;
        let snap = s.snapshot();
        for dir in ["", "parts", "parts/p", "releases", "releases/p", "nowhere"] {
            assert_eq!(s.list(dir).unwrap(), snap.list(dir).unwrap(), "{dir}");
        }
        assert_eq!(s.read("releases/p/A.json").unwrap(), Some(b"{}".to_vec()));
        assert_eq!(s.read("parts/p/part.json").unwrap(), None);
    }

    #[test]
    fn touching_without_changing_reloads_nothing() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        let path = feature_path(dir.path(), &id("p"), &id("a"));
        let text = fs::read(&path).unwrap();
        fs::write(&path, &text).unwrap();
        assert!(!s.refresh().unwrap());
    }

    #[test]
    fn a_part_reloaded_from_held_bytes_is_as_loaded_from_disk() {
        let dir = project();
        let root = dir.path();
        let mut s = Session::open(root).unwrap();
        let same = |s: &Session| {
            assert_eq!(
                format!("{:?}", s.project()),
                format!("{:?}", Project::load(root).unwrap())
            );
        };
        // An edit, a new feature, a broken one and a stray in one part.
        write_json(&feature_path(root, &id("p"), &id("a")), &sphere("3 mm")).unwrap();
        write_json(&feature_path(root, &id("p"), &id("z")), &sphere("4 mm")).unwrap();
        fs::write(feature_path(root, &id("p"), &id("y")), "{ nope").unwrap();
        fs::write(root.join("parts/p/notes.txt"), "hi").unwrap();
        assert!(s.refresh().unwrap());
        same(&s);
        // The manifest, which every part reads.
        write_json(
            &root.join(crate::doc::project::MANIFEST),
            &crate::doc::files::Manifest::new(Unit::In).to_json(),
        )
        .unwrap();
        assert!(s.refresh().unwrap());
        same(&s);
    }

    #[test]
    fn sees_files_added_and_removed() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        write_json(
            &part_path(dir.path(), &id("q")),
            &PartFile::new(vec![]).to_json(),
        )
        .unwrap();
        assert!(s.refresh().unwrap());
        assert!(s.project().parts.contains_key(&id("q")));
        fs::remove_dir_all(dir.path().join("parts/q")).unwrap();
        assert!(s.refresh().unwrap());
        assert!(!s.project().parts.contains_key(&id("q")));
    }

    #[test]
    fn rebuilds_are_cached_by_revision() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        let h1 = s.rebuilt(&id("p")).unwrap().hash.clone();
        let ptr1 = s.rebuilt(&id("p")).unwrap() as *const PartResult;
        let ptr2 = s.rebuilt(&id("p")).unwrap() as *const PartResult;
        assert_eq!(ptr1, ptr2);
        write_json(
            &feature_path(dir.path(), &id("p"), &id("a")),
            &sphere("3 mm"),
        )
        .unwrap();
        s.refresh().unwrap();
        assert_ne!(s.rebuilt(&id("p")).unwrap().hash, h1);
    }

    #[test]
    fn only_what_a_change_touches_is_rebuilt() {
        use crate::server::tests::{call, fresh};
        use serde_json::json;
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "2 mm", "w": "20 mm"}}),
        )
        .unwrap();
        for f in [
            json!({"type": "box", "id": "b", "params": {"x": "w", "y": 10, "z": 5}}),
            json!({"type": "sphere", "id": "c", "params": {"radius": "r"}}),
            json!({"type": "sketch", "id": "s"}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": crate::sketch::tests::rectangle_ops()}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"length": "w"}}),
        )
        .unwrap();
        let built = |s: &mut Session| -> Vec<String> {
            let full = rebuild_part(s.project().part(&id("p")).unwrap()).hash;
            let r = s.rebuilt(&id("p")).unwrap();
            // Reusing changes nothing.
            assert_eq!(r.hash, full);
            r.built.iter().map(|i| i.to_string()).collect()
        };
        built(&mut s);
        let edit = |s: &mut Session, method: &str, params: serde_json::Value| {
            let mut params = params;
            params["part"] = json!("p");
            call(s, method, params).unwrap();
            built(s)
        };
        assert_eq!(
            edit(
                &mut s,
                "feature.edit",
                json!({"id": "c", "params": {"radius": 3}})
            ),
            ["c"]
        );
        // w is read by the box and the extrude, not the sketch.
        assert_eq!(
            edit(&mut s, "param.set", json!({"parameters": {"w": "25 mm"}})),
            ["b", "e"]
        );
        // The sketch changes, and so does the extrude made from it.
        assert_eq!(
            edit(
                &mut s,
                "sketch.edit",
                json!({"sketch": "s", "ops": [{"op": "set", "id": "width", "value": "30 mm"}]}),
            ),
            ["s", "e"]
        );
        // Nothing changes: nothing is built.
        assert_eq!(
            edit(&mut s, "param.set", json!({"parameters": {"r": "3 mm"}})),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_change_in_one_part_leaves_the_others() {
        let dir = project();
        let root = dir.path();
        write_json(
            &part_path(root, &id("q")),
            &PartFile::new(vec![id("a")]).to_json(),
        )
        .unwrap();
        write_json(&feature_path(root, &id("q"), &id("a")), &sphere("1 mm")).unwrap();
        let mut s = Session::open(root).unwrap();
        let p = s.rebuilt(&id("p")).unwrap() as *const PartResult;
        write_json(&feature_path(root, &id("q"), &id("a")), &sphere("4 mm")).unwrap();
        assert!(s.refresh().unwrap());
        assert_eq!(s.rebuilt(&id("p")).unwrap() as *const PartResult, p);
        let fresh = Project::load(root).unwrap();
        assert_eq!(format!("{:?}", s.project()), format!("{fresh:?}"));
        // What the session wrote itself is in its snapshot once it reloads.
        write_json(&feature_path(root, &id("p"), &id("a")), &sphere("5 mm")).unwrap();
        s.reload().unwrap();
        assert_eq!(
            s.snapshot()["parts/p/features/a.json"][..],
            fs::read(feature_path(root, &id("p"), &id("a"))).unwrap()
        );
        assert_eq!(s.snapshot(), crate::activity::snapshot(root).unwrap());
    }

    #[test]
    fn any_revision_rebuilds_from_its_own_files() {
        use crate::git::tests::git;
        use crate::server::tests::call;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        crate::server::init(&root, None).unwrap();
        let mut s = Session::open(&root).unwrap();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let sphere = |s: &mut Session, r: u32| {
            call(
                s,
                "feature.add",
                json!({"part": "p", "type": "sphere", "id": format!("s{r}"), "params": {"radius": r}}),
            )
            .unwrap();
        };
        let mut hashes = Vec::new();
        for r in 1..=6 {
            sphere(&mut s, r);
            git(&root, &["add", "."]);
            git(&root, &["commit", "-q", "-m", &format!("sphere {r}")]);
            hashes.push(
                call(&mut s, "rebuild", json!({"part": "p"})).unwrap()["parts"][0]["hash"].clone(),
            );
        }
        // Uncommitted: one more.
        sphere(&mut s, 7);
        let at = |s: &mut Session, rev: &str| call(s, "rebuild", json!({"part": "p", "rev": rev}));
        let head = at(&mut s, "HEAD~2").unwrap();
        assert_eq!(head["parts"][0]["hash"], hashes[3]);
        let commit = head["commit"].as_str().unwrap().to_owned();
        assert_eq!(commit.len(), 40);
        assert_eq!(at(&mut s, &commit).unwrap()["parts"][0]["hash"], hashes[3]);
        assert_eq!(
            at(&mut s, "working").unwrap(),
            call(&mut s, "rebuild", json!({"part": "p"})).unwrap()
        );
        // A revision starts from the working files' results: HEAD differs
        // from them only by the uncommitted sphere, so nothing is built.
        let (_, h) = s.at(Some("HEAD")).unwrap();
        assert!(h.rebuilt(&id("p")).unwrap().built.is_empty());
        // Inspect and meshes read the revision too; s5 didn't exist then.
        let v = call(&mut s, "inspect", json!({"part": "p", "rev": "HEAD~3"})).unwrap();
        assert_eq!(v["bodies"].as_array().unwrap().len(), 3);
        assert_eq!(
            v["commit"],
            call(&mut s, "rebuild", json!({"rev": "HEAD~3"})).unwrap()["commit"]
        );
        let m = call(
            &mut s,
            "mesh.get",
            json!({"part": "p", "rev": "HEAD~3", "quality": "coarse"}),
        )
        .unwrap();
        assert_eq!(m["bodies"].as_array().unwrap().len(), 3);
        let e = call(
            &mut s,
            "inspect",
            json!({"part": "p", "feature": "s5", "rev": "HEAD~3"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["commit"]),
            (Code::NotFound, &v["commit"])
        );
        // A few revisions are kept, most recently used first.
        for r in ["HEAD~5", "HEAD~4", "HEAD~3", "HEAD~2", "HEAD~1"] {
            at(&mut s, r).unwrap();
        }
        assert_eq!(s.revisions.len(), REVISIONS);
        // A revision's session never reads the disk.
        fs::remove_file(root.join("parts/p/features/s1.json")).unwrap();
        assert_eq!(at(&mut s, "HEAD~1").unwrap()["parts"][0]["hash"], hashes[4]);
        let e = at(&mut s, "nowhere").unwrap_err();
        assert_eq!(e.code, Code::RevNotFound);
        let (outside, mut t) = crate::server::tests::fresh();
        let e = call(&mut t, "rebuild", json!({"rev": "HEAD"})).unwrap_err();
        assert_eq!(e.code, Code::NotAGitRepo);
        drop(outside);
    }

    #[test]
    fn another_worktrees_working_files_read_as_a_revision() {
        use crate::git::tests::git;
        use crate::server::tests::call;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        crate::server::init(&root, None).unwrap();
        let mut s = Session::open(&root).unwrap();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let sphere = json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}});
        call(&mut s, "feature.add", sphere).unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "a sphere"]);
        let wt = dir.path().join("arm");
        git(
            &root,
            &["worktree", "add", "-q", "-b", "arm", wt.to_str().unwrap()],
        );
        let wt = wt.canonicalize().unwrap();
        // Another agent's uncommitted edit, in its own worktree.
        let mut other = Session::open(&wt).unwrap();
        let edit = |o: &mut Session, r: u32| {
            call(
                o,
                "feature.edit",
                json!({"part": "p", "id": "s", "params": {"radius": r}}),
            )
            .unwrap();
            call(o, "rebuild", json!({"part": "p"})).unwrap()["parts"][0]["hash"].clone()
        };
        let two = edit(&mut other, 2);
        let rev = format!("{WORKTREE}{}", wt.display());
        let at = |s: &mut Session, rev: &str| call(s, "rebuild", json!({"part": "p", "rev": rev}));
        let v = at(&mut s, &rev).unwrap();
        assert_eq!(v["parts"][0]["hash"], two);
        assert_eq!(v.get("commit"), None);
        // The diff names it, with its branch and head.
        let d = call(&mut s, "diff", json!({"from": "working", "to": rev})).unwrap();
        assert_eq!(
            d["to"]["worktree"],
            json!(crate::doc::project::plain(wt.clone()))
        );
        assert_eq!(d["to"]["branch"], "arm");
        assert_eq!(d["to"]["head"].as_str().unwrap().len(), 40);
        assert_eq!(d["parts"]["p"]["features"]["s"]["status"], "changed");
        // It follows the other worktree's files, keeping one session for it.
        let three = edit(&mut other, 3);
        assert_eq!(at(&mut s, &rev).unwrap()["parts"][0]["hash"], three);
        let kept = s
            .revisions
            .iter()
            .filter(|(k, _)| k.starts_with(WORKTREE))
            .count();
        assert_eq!(kept, 1);
        // The project folder or the work tree names it; this worktree's
        // own is the working files.
        assert_eq!(
            at(&mut s, &format!("{WORKTREE}{}", root.display())).unwrap(),
            call(&mut s, "rebuild", json!({"part": "p"})).unwrap()
        );
        let e = at(&mut s, &format!("{WORKTREE}{}", dir.path().display())).unwrap_err();
        assert_eq!(e.code, Code::WorktreeNotFound);
        let e = at(&mut s, "worktree:arm").unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn a_vanished_manifest_is_an_error() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        fs::remove_file(dir.path().join(MANIFEST)).unwrap();
        assert!(s.refresh().is_err());
    }
}
