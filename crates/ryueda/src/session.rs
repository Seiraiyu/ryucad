//! A loaded project, kept fresh against the files. Before every request
//! the session checks each file it depends on (the manifest, this
//! project's library, and each alias's library project), as ryucad's
//! does, and loads the project again when any changed. It keeps every
//! file's bytes, so the activity log's snapshots cost no reading.

use crate::error::{Code, Error, Result};
use crate::project::{MANIFEST, Manifest, Project, resolve};
use ryu_session::fresh::{Files, Stamps};
use std::path::{Path, PathBuf};

/// How many revisions a session keeps read, most recently used first.
pub const REVISIONS: usize = 4;

/// How many scenes a session keeps drawn (plan 3.B), most recently used
/// first.
pub const SCENES: usize = 8;

/// How many revisions' laid-out blocks a session keeps (plan 13.A), most
/// recently used first: as many as [`REVISIONS`].
pub const LAID: usize = REVISIONS;

/// A revision's block as it lays out ([`crate::compare::lay_then`]).
pub type LaidThen = std::sync::Arc<Result<Option<crate::schem::Laid>>>;

pub struct Session {
    root: PathBuf,
    project: Project,
    stamps: Stamps,
    /// What a long request's lock-free work made, taken by its answer.
    pub(crate) prepared: Option<Result<serde_json::Value>>,
    /// This machine's catalogues, opened once and kept while their
    /// indexes stand.
    catalogues: Option<crate::parts::Catalogues>,
    /// The last [`REVISIONS`] revisions read ([`Session::at`]), by what
    /// they read, most recently used first.
    pub(crate) revisions: Vec<(String, std::sync::Arc<Project>)>,
    /// The last `merge.status` answer, by the key it was read at
    /// ([`crate::validate::key`]).
    pub(crate) merge_status: Option<(String, serde_json::Value)>,
    /// The last [`SCENES`] scenes drawn, by their ask and the revisions
    /// they were drawn from, most recently used first.
    pub(crate) scenes: Vec<(String, std::sync::Arc<String>)>,
    /// The last [`LAID`] revision lay-outs, by the revision's key, the
    /// block and the layer, most recently used first.
    pub(crate) laid: Vec<(String, LaidThen)>,
    /// The release branch as last read and the refs it was read at
    /// ([`crate::pdm::history`]): read again only when they move.
    pub(crate) pdm: Option<(String, ryupdm::history::Releases)>,
    /// Resolves the manifest's pins (design 629): [`crate::pin::Pins`],
    /// which keeps what it read by commit.
    pub(crate) pins: Box<dyn crate::pin::Resolve + Send>,
    /// The refs the pinned libraries' release branches were read at
    /// ([`pins_key`]): when they move, the project is read again.
    pins_key: String,
    /// What a long request's lock-free fetches got
    /// ([`crate::libdeps::prepare`]), taken by its answer.
    pub(crate) fetched: Option<crate::libdeps::Prefetched>,
    /// The last [`BOARDS`] boards' shapes built, by the board and the
    /// revisions they were built from, most recently used first.
    pub(crate) boards: Vec<(String, std::sync::Arc<crate::board::shapes::BoardShapes>)>,
    /// The last [`BOARDS`] boards' checks, by the same keys as their
    /// shapes, most recently used first.
    pub(crate) checks: Vec<(String, std::sync::Arc<crate::board::drc::BoardCheck>)>,
}

/// How many boards' shapes a session keeps.
const BOARDS: usize = 4;

/// The refs every same-repository pinned library's release branch is
/// read at, joined: when they move, a label may have become official or
/// obsolete, and the project is read again. Empty, without opening the
/// repository, for a project that pins nothing in it.
fn pins_key(root: &Path, m: &Manifest) -> String {
    let mut pinned = m
        .libraries
        .values()
        .filter(|a| a.url.is_none() && !a.labels.is_empty())
        .peekable();
    if pinned.peek().is_none() {
        return String::new();
    }
    let Ok(repo) = ryu_git::repo::repo::<crate::Eda>(root) else {
        return String::new();
    };
    let mut out = String::new();
    for a in pinned {
        let lib = path_of(root, &a.path);
        if let Ok(c) =
            ryupdm::config::Config::read_in(&crate::source::Folder(&lib), crate::pdm::NAME)
            && let Ok(k) = ryupdm::release::refs_key(repo.gix(), &c)
        {
            out.push_str(&k);
        }
        out.push('\n');
    }
    out
}

/// `rel` (relative to the project, with `/`) as a path under `root`.
pub fn path_of(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s))
}

/// Every file the project's state depends on, with its metadata, as
/// paths under `root` (an alias's files by way of `..`).
fn listed(root: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    Project::tracked(root)
        .into_iter()
        .map(|(rel, m)| (path_of(root, &rel), m))
        .collect()
}

impl Session {
    /// The scene kept under `key`, now the most recently used.
    pub(crate) fn scene(&mut self, key: &str) -> Option<std::sync::Arc<String>> {
        let i = self.scenes.iter().position(|(k, _)| k == key)?;
        let hit = self.scenes.remove(i);
        let text = hit.1.clone();
        self.scenes.insert(0, hit);
        Some(text)
    }

    /// Keeps a scene drawn under `key`, forgetting the least recently
    /// used past [`SCENES`].
    pub(crate) fn keep_scene(&mut self, key: String, text: std::sync::Arc<String>) {
        self.scenes.insert(0, (key, text));
        self.scenes.truncate(SCENES);
    }

    /// Loads the project at `root`.
    pub fn open(root: &Path) -> Result<Session> {
        Session::open_with_pins(root, Box::new(crate::pin::Pins::default()))
    }

    /// Loads the project at `root`, its pins resolved by `pins`.
    pub(crate) fn open_with_pins(
        root: &Path,
        pins: Box<dyn crate::pin::Resolve + Send>,
    ) -> Result<Session> {
        let mut stamps = Stamps::default();
        stamps.scan(root, listed(root))?;
        let project = load(root, &stamps, &*pins)?;
        let pins_key = pins_key(root, &project.manifest);
        Ok(Session {
            root: root.to_path_buf(),
            project,
            stamps,
            prepared: None,
            catalogues: None,
            revisions: Vec::new(),
            merge_status: None,
            scenes: Vec::new(),
            laid: Vec::new(),
            pdm: None,
            pins,
            pins_key,
            fetched: None,
            boards: Vec::new(),
            checks: Vec::new(),
        })
    }

    /// What a board's shapes and check are kept by: the board's, the
    /// project's, its libraries' and its blocks' revisions.
    fn board_key(&self, board: &crate::id::Id) -> Result<String> {
        let p = &self.project;
        let b = p.boards.get(board).ok_or_else(|| {
            Error::new(Code::BoardNotFound, format!("there is no board {board}"))
                .with("board", board)
        })?;
        let mut h = blake3::Hasher::new();
        h.update(b"ryueda.board-shapes.1");
        crate::design::piece(&mut h, board.as_str().as_bytes());
        crate::design::piece(&mut h, b.revision.as_bytes());
        for (k, v) in crate::server::revisions(p) {
            if k == "project" || k.starts_with("lib/") || k.starts_with("block/") {
                crate::design::piece(&mut h, k.as_bytes());
                crate::design::piece(&mut h, v.to_string().as_bytes());
            }
        }
        Ok(h.finalize().to_hex().to_string())
    }

    /// The board's check, built once per the same revisions as its shapes.
    pub fn board_check(
        &mut self,
        board: &crate::id::Id,
    ) -> Result<std::sync::Arc<crate::board::drc::BoardCheck>> {
        let key = self.board_key(board)?;
        if let Some(i) = self.checks.iter().position(|(k, _)| *k == key) {
            let hit = self.checks.remove(i);
            let c = hit.1.clone();
            self.checks.insert(0, hit);
            return Ok(c);
        }
        let shapes = self.board_shapes(board)?;
        let p = &self.project;
        let built = std::sync::Arc::new(crate::board::drc::check_shapes(
            p,
            &p.boards[board],
            shapes,
        )?);
        self.checks.insert(0, (key, built.clone()));
        self.checks.truncate(BOARDS);
        Ok(built)
    }

    /// The board's shapes, built once per the board's, the project's, its
    /// libraries' and its blocks' revisions.
    pub fn board_shapes(
        &mut self,
        board: &crate::id::Id,
    ) -> Result<std::sync::Arc<crate::board::shapes::BoardShapes>> {
        let key = self.board_key(board)?;
        let p = &self.project;
        let b = &p.boards[board];
        if let Some(i) = self.boards.iter().position(|(k, _)| *k == key) {
            let hit = self.boards.remove(i);
            let shapes = hit.1.clone();
            self.boards.insert(0, hit);
            return Ok(shapes);
        }
        let built = std::sync::Arc::new(crate::board::shapes::build(p, b)?);
        self.boards.insert(0, (key, built.clone()));
        self.boards.truncate(BOARDS);
        Ok(built)
    }

    /// [`Session::at`], with the key it is kept by.
    pub fn at_keyed(&mut self, rev: &str) -> Result<(String, String, std::sync::Arc<Project>)> {
        let (commit, key) = crate::git::key(&self.root, rev)?;
        if let Some(i) = self.revisions.iter().position(|(k, _)| *k == key) {
            let hit = self.revisions.remove(i);
            let p = hit.1.clone();
            self.revisions.insert(0, hit);
            return Ok((commit, key, p));
        }
        let r = crate::git::revision(&self.root, &commit)?;
        let p = std::sync::Arc::new(Project::load_from(
            &self.root,
            &r.files,
            &r.aliases,
            &*self.pins,
        )?);
        self.revisions.insert(0, (key.clone(), p.clone()));
        self.revisions.truncate(REVISIONS);
        Ok((commit, key, p))
    }

    /// The project as `rev` has it, and the commit it names: one of the
    /// last [`REVISIONS`] read, kept by what they read (the project's tree
    /// and its aliases'), else read from Git's objects. Never the disk.
    pub fn at(&mut self, rev: &str) -> Result<(String, std::sync::Arc<Project>)> {
        self.at_keyed(rev).map(|(c, _, p)| (c, p))
    }

    /// Block `b` of the revision `key` names (`then`, from
    /// [`Session::at_keyed`]) laid out with `layer`'s designators: kept,
    /// else laid out and kept, forgetting the least recently used past
    /// [`LAID`].
    pub fn laid_at(
        &mut self,
        key: &str,
        then: &Project,
        b: &crate::id::Id,
        layer: Option<&crate::schem::Layer>,
    ) -> LaidThen {
        let k = format!(
            "{key}\u{0}{b}\u{0}{}",
            layer.map_or_else(String::new, |l| format!("{}/{:?}", l.design, l.path))
        );
        if let Some(i) = self.laid.iter().position(|(x, _)| *x == k) {
            let hit = self.laid.remove(i);
            let l = hit.1.clone();
            self.laid.insert(0, hit);
            return l;
        }
        let l = std::sync::Arc::new(crate::compare::lay_then(then, b, layer));
        self.laid.insert(0, (k, l.clone()));
        self.laid.truncate(LAID);
        l
    }

    /// The files `rev` holds, as a session holds them, and its commit:
    /// what `diff` compares. Never the disk.
    pub fn files_at(&self, rev: &str) -> Result<(String, Files)> {
        let r = crate::git::revision(&self.root, rev)?;
        Ok((r.commit, r.files))
    }

    /// This machine's catalogues as of now: those opened before, unless
    /// synced again since.
    pub fn catalogues(&mut self) -> Result<crate::parts::Catalogues> {
        let now = crate::parts::now();
        let c = match &self.catalogues {
            Some(c) => c.refreshed(now),
            None => crate::parts::Catalogues::here()?,
        };
        self.catalogues = Some(c.clone());
        Ok(c)
    }

    /// A hash of every file the project's state depends on, as last read.
    pub fn digest(&self) -> String {
        self.stamps.digest()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Brings the session up to date with the files, returning those that
    /// changed (relative, with `/`) and how.
    pub fn refresh_changes(&mut self) -> Result<Vec<(String, &'static str)>> {
        let changes = self.stamps.scan(&self.root, listed(&self.root))?;
        if !changes.is_empty() {
            self.project = load(&self.root, &self.stamps, &*self.pins)?;
            self.pins_key = pins_key(&self.root, &self.project.manifest);
        } else {
            let key = pins_key(&self.root, &self.project.manifest);
            if key != self.pins_key {
                self.project = load(&self.root, &self.stamps, &*self.pins)?;
                self.pins_key = key;
            }
        }
        Ok(changes)
    }

    /// Reads the project again over the same files: what resolves its
    /// pins may have changed (a fetch into the dependency cache).
    pub(crate) fn reload_pins(&mut self) -> Result<()> {
        self.project = load(&self.root, &self.stamps, &*self.pins)?;
        self.pins_key = pins_key(&self.root, &self.project.manifest);
        Ok(())
    }

    /// Takes in what this session just wrote.
    pub fn reload(&mut self) -> Result<()> {
        self.refresh_changes().map(drop)
    }

    /// Takes in the files this session just wrote at `written`, as
    /// `project`: the project already read over them in memory. Reads it
    /// again only if anything else changed too.
    pub fn adopt(&mut self, project: Project, written: &[String]) -> Result<()> {
        let changes = self.stamps.scan(&self.root, listed(&self.root))?;
        if changes.iter().all(|(p, _)| written.contains(p)) {
            self.project = project;
        } else {
            self.project = load(&self.root, &self.stamps, &*self.pins)?;
        }
        self.pins_key = pins_key(&self.root, &self.project.manifest);
        Ok(())
    }

    /// `ryueda.lock` as the working files hold it, if it reads.
    pub fn lock(&self) -> Option<ryu_doc::lock::Lock> {
        self.files()
            .get(crate::project::LOCK)
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|t| crate::canonical::parse(t).ok())
            .and_then(|v| ryu_doc::lock::Lock::from_json_as(&v, crate::project::LOCK_KIND).ok())
    }

    /// Every file the project's state depends on, as last read.
    pub fn snapshot(&self) -> Files {
        Files::clone(&self.stamps.snapshot())
    }

    /// [`Self::snapshot`], shared: a pointer, where the caller only reads
    /// it (a write's files before and after, for the activity log).
    pub fn files(&self) -> std::sync::Arc<Files> {
        self.stamps.snapshot()
    }
}

/// The project from the stamps' bytes, its aliases resolved on disk.
fn load(root: &Path, stamps: &Stamps, pins: &dyn crate::pin::Resolve) -> Result<Project> {
    let files = stamps.snapshot();
    let bytes = files.get(MANIFEST).ok_or_else(|| {
        Error::new(
            Code::ProjectNotFound,
            format!("{} has no {MANIFEST}", root.display()),
        )
        .with("path", root)
    })?;
    let aliases = resolve(root, &Manifest::read(bytes)?);
    Project::load_from(root, &files, &aliases, pins)
}

/// Every file the project's state depends on, read now: what undo checks
/// a revert against.
pub fn current(root: &Path) -> Result<Files> {
    listed(root)
        .into_iter()
        .map(|(path, _)| {
            let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, &e))?;
            Ok((crate::source::relative(root, &path), bytes.into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_boards_check_is_kept_until_a_revision_moves() {
        let (dir, p) = crate::netlist::tests::project(&[]);
        let design = crate::netlist::one_design(&p, None).unwrap().to_string();
        let root = dir.path().join("board");
        let mut s = Session::open(&root).unwrap();
        crate::board::cmd::create(
            &mut s,
            "B",
            &design,
            Some(&["30".into(), "20".into()]),
            None,
            None,
            Some("b"),
            None,
        )
        .unwrap();
        let id = crate::id::Id::parse("b", "board").unwrap();
        let a = s.board_check(&id).unwrap();
        let b = s.board_check(&id).unwrap();
        assert!(std::sync::Arc::ptr_eq(&a, &b), "kept");
        crate::board::cmd::edit(&mut s, "b", Some("C"), None, None, None, None, None).unwrap();
        let c = s.board_check(&id).unwrap();
        assert!(
            !std::sync::Arc::ptr_eq(&a, &c),
            "built again after the board moved"
        );
    }

    #[test]
    fn a_session_resolves_pins_and_sees_a_new_official_label() {
        let (d, board) = crate::pin::tests::released_library_and_board();
        let mut s = Session::open(&board).unwrap();
        let key = (
            String::from("passives"),
            crate::id::Id::parse("r_100k", "c").unwrap(),
        );
        assert!(
            s.project().pinned[&key].is_ok(),
            "{:?}",
            s.project().pinned[&key]
        );
        assert!(!s.project().pinned[&key].as_ref().unwrap().obsolete);
        // Obsolete A on the library's release branch: no file the session
        // watches changes, and the next request sees it.
        crate::pin::tests::obsolete(d.path(), "lib", "r_100k", "A");
        s.refresh_changes().unwrap();
        assert!(s.project().pinned[&key].as_ref().unwrap().obsolete);
    }
    use super::*;
    use crate::project::ProjectKind;
    use crate::project::tests::{library_in, manifest};

    #[test]
    fn a_revisions_lay_out_is_kept() {
        use crate::git::tests::git;
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        git(dir.path(), &["init", "-q"]);
        manifest(&board, ProjectKind::Design, &[]);
        crate::project::tests::design_in(&board);
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "c0"]);
        let mut s = Session::open(&board).unwrap();
        let (commit, key, then) = s.at_keyed("HEAD").unwrap();
        assert_eq!(commit.len(), 40);
        let b = crate::id::Id::parse("top", "block").unwrap();
        let one = s.laid_at(&key, &then, &b, None);
        assert!(matches!(&*one, Ok(Some(_))), "top lays out at HEAD");
        let two = s.laid_at(&key, &then, &b, None);
        assert!(std::sync::Arc::ptr_eq(&one, &two), "laid out once");
        assert_eq!(s.laid.len(), 1);
        // A block the revision hasn't: kept as none.
        let none = crate::id::Id::parse("nope", "block").unwrap();
        assert!(matches!(&*s.laid_at(&key, &then, &none, None), Ok(None)));
    }

    #[test]
    fn revisions_are_read_once_and_kept_four_at_a_time() {
        use crate::git::tests::git;
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        git(dir.path(), &["init", "-q"]);
        manifest(&board, ProjectKind::Design, &[]);
        crate::project::tests::design_in(&board);
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "c0"]);
        for i in 1..=5 {
            std::fs::write(
                board.join("rails/x.json"),
                format!("{{\"kind\": \"x{i}\"}}\n"),
            )
            .unwrap();
            git(dir.path(), &["add", "."]);
            git(dir.path(), &["commit", "-q", "-m", &format!("c{i}")]);
        }
        let mut s = Session::open(&board).unwrap();
        let (c0, p0) = s.at("HEAD~5").unwrap();
        let (again, p0b) = s.at(&c0).unwrap();
        assert_eq!(again, c0);
        assert!(std::sync::Arc::ptr_eq(&p0, &p0b), "read once");
        for i in 0..5 {
            s.at(&format!("HEAD~{i}")).unwrap();
        }
        assert_eq!(s.revisions.len(), REVISIONS);
        let (_, p0c) = s.at(&c0).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&p0, &p0c), "the oldest was dropped");
        // A revision never touches the working project.
        assert_eq!(
            s.project().revision,
            Project::load(&board).unwrap().revision
        );
    }

    #[test]
    fn sessions_see_changes_in_aliases_too() {
        let dir = tempfile::tempdir().unwrap();
        let (board, lib) = (dir.path().join("board"), dir.path().join("lib"));
        manifest(&lib, ProjectKind::Library, &[]);
        library_in(&lib);
        manifest(&board, ProjectKind::Design, &[("passives", "../lib")]);
        let mut s = Session::open(&board).unwrap();
        let before = s.project().library("passives").unwrap().revision.clone();
        assert!(s.snapshot().contains_key("../lib/library/symbols/r.json"));
        assert!(s.refresh_changes().unwrap().is_empty());
        std::fs::remove_file(lib.join("library/symbols/r.json")).unwrap();
        let changes = s.refresh_changes().unwrap();
        assert_eq!(
            changes,
            [("../lib/library/symbols/r.json".to_owned(), "deleted")]
        );
        assert_ne!(s.project().library("passives").unwrap().revision, before);
        assert_eq!(current(&board).unwrap(), s.snapshot());
    }

    #[test]
    fn sessions_see_blocks_change() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        crate::project::tests::design_in(&board);
        let mut s = Session::open(&board).unwrap();
        let top = crate::design::tests::id("top");
        let before = s.project().conn.blocks[&top].revision.clone();
        std::fs::remove_file(board.join("blocks/top/nets/sda.json")).unwrap();
        let changes = s.refresh_changes().unwrap();
        assert_eq!(
            changes,
            [("blocks/top/nets/sda.json".to_owned(), "deleted")]
        );
        assert_ne!(s.project().conn.blocks[&top].revision, before);
    }
}
