//! Representations (design 734's Links): the ryucad and ryueda projects
//! items link to, found on disk by their manifests, and (Tasks 9 and 10)
//! the lines their BOMs give. ryuplm reads them through their files,
//! never their crates: ryucad's through ryupdm's structure reader
//! (feature `cad`), ryueda's through the `ryueda` executable.

use crate::error::{Code, Error, Result};
use crate::id::{Id, ProjectId};
use crate::source::Source;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The products an item may link to, and their manifests.
pub const PRODUCTS: [(&str, &str); 2] = [("ryucad", "ryucad.json"), ("ryueda", "ryueda.json")];

/// The manifest of `product`, `None` for a product links can't name.
pub fn manifest_of(product: &str) -> Option<&'static str> {
    PRODUCTS
        .iter()
        .find(|(p, _)| *p == product)
        .map(|(_, m)| *m)
}

/// The project ID the manifest `manifest` in `src` gives (its `project`),
/// `None` when it has none or doesn't read.
pub fn project_id(src: &dyn Source, manifest: &str) -> Option<ProjectId> {
    let bytes = src.read(manifest).ok()??;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    ProjectId::parse(v.get("project")?.as_str()?).ok()
}

/// Whether `part` is a part of the product's project `src` holds: a
/// ryucad part or assembly (`parts/<id>/part.json`), a ryueda design
/// (`designs/<id>.json`) or library component
/// (`library/components/<id>.json`).
pub fn part_exists(src: &dyn Source, product: &str, part: &Id) -> bool {
    let paths = match product {
        "ryucad" => vec![format!("parts/{part}/part.json")],
        "ryueda" => vec![
            format!("designs/{part}.json"),
            format!("library/components/{part}.json"),
        ],
        _ => Vec::new(),
    };
    paths.iter().any(|p| matches!(src.read(p), Ok(Some(_))))
}

/// The `number` property the representation's properties file gives
/// (ryupdm's: `parts/<id>/properties.json` in ryucad,
/// `properties/<id>.json` in ryueda), `None` when it gives none.
pub fn number_of(src: &dyn Source, product: &str, part: &Id) -> Option<String> {
    let path = match product {
        "ryucad" => format!("parts/{part}/properties.json"),
        "ryueda" => format!("properties/{part}.json"),
        _ => return None,
    };
    let bytes = src.read(&path).ok()??;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let props = ryupdm::props::Properties::from_json_in(&v, product).ok()?;
    props.values.get("number")?.as_str().map(str::to_owned)
}

/// The representation projects found under some folders, by product and
/// project ID.
#[derive(Clone, Debug, Default)]
pub struct Index {
    found: BTreeMap<(String, ProjectId), PathBuf>,
    /// The folders scanned, in order.
    pub searched: Vec<PathBuf>,
}

impl Index {
    /// Every ryucad and ryueda project at or under `tops`, in order; a
    /// project ID found twice (two checkouts) keeps its first folder.
    pub fn scan(tops: &[PathBuf]) -> Index {
        let mut ix = Index::default();
        for t in tops {
            ix.searched.push(t.clone());
            for (product, manifest) in PRODUCTS {
                for dir in ryupdm::dependents::projects_under(t, manifest).unwrap_or_default() {
                    let dir = dir.canonicalize().unwrap_or(dir);
                    if let Some(pid) = project_id(&crate::source::Folder(&dir), manifest) {
                        ix.found.entry((product.to_owned(), pid)).or_insert(dir);
                    }
                }
            }
        }
        ix
    }

    /// Records `dir` as `product`'s project `project` (tests).
    pub fn insert(&mut self, product: &str, project: ProjectId, dir: PathBuf) {
        self.found.insert((product.to_owned(), project), dir);
    }

    /// The folder holding `product`'s project `project`.
    pub fn dir(&self, product: &str, project: &ProjectId) -> Option<&Path> {
        self.found
            .get(&(product.to_owned(), project.clone()))
            .map(PathBuf::as_path)
    }
}

/// Where representations are looked for: the catalogue's Git work tree
/// (its own folder outside one), then `folders`.
pub fn tops(here: &Path, folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match gix::discover(here)
        .ok()
        .and_then(|r| r.workdir().map(Path::to_path_buf))
    {
        Some(w) => out.push(w.canonicalize().unwrap_or(w)),
        None => out.push(here.to_path_buf()),
    }
    for f in folders {
        if !out.contains(f) {
            out.push(f.clone());
        }
    }
    out
}

/// One line a representation's BOM gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepLine {
    /// What tells it from the representation's other lines: ryupdm's row
    /// key for ryucad (`<part>` or `<project>:<part>`), ryueda's part,
    /// value and footprint joined by NUL.
    pub key: String,
    /// The child representation's project: `None` when ryueda names it by
    /// a library alias with no project ID, so no item can link it.
    pub project: Option<ProjectId>,
    pub part: Id,
    /// How many: ryucad's instance count, ryueda's populated occurrences.
    pub quantity: u64,
    /// ryucad's item number in the assembly.
    pub find: Option<String>,
    /// ryueda's designators.
    pub refdes: Option<String>,
}

/// The lines one representation gave, and the revision they were read
/// at: `working`, or a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read {
    pub lines: Vec<RepLine>,
    pub rev: String,
}

/// Which revision of the catalogue lines are read for.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum At {
    Working,
    /// A commit of the catalogue's repository, in hex.
    Commit(String),
}

impl At {
    pub fn text(&self) -> &str {
        match self {
            At::Working => "working",
            At::Commit(c) => c,
        }
    }
}

/// Where representation lines come from.
pub trait Reps {
    fn lines(&mut self, link: &crate::item::Link, at: &At) -> Result<Read>;
}

/// `item_link_unresolved` for `link`, saying why.
pub fn unresolved(link: &crate::item::Link, why: &str) -> Error {
    Error::new(
        Code::ItemLinkUnresolved,
        format!(
            "{} {}:{} can't be read: {why}",
            link.product, link.project, link.part
        ),
    )
    .with("product", &link.product)
    .with("project", &link.project)
    .with("part", &link.part)
}

/// Representation lines held in memory, by what a link names: for tests,
/// the golden and fuzzing.
#[derive(Clone, Debug, Default)]
pub struct Fixed(pub BTreeMap<(String, ProjectId, Id), Vec<RepLine>>);

impl Reps for Fixed {
    fn lines(&mut self, link: &crate::item::Link, at: &At) -> Result<Read> {
        let k = (
            link.product.clone(),
            link.project.clone(),
            link.part.clone(),
        );
        let lines = self
            .0
            .get(&k)
            .cloned()
            .ok_or_else(|| unresolved(link, "it isn't held"))?;
        Ok(Read {
            lines,
            rev: at.text().to_owned(),
        })
    }
}

/// The variable naming the `ryueda` executable; `ryueda` on PATH without it.
pub const RYUEDA_EXE: &str = "RYUEDA_EXE";

/// The lines of `ryueda bom`'s answer (`schema` 1) for a design of the
/// project `own`: `self/<id>` lines are `own`'s components,
/// `<project>:<id>` another project's, `<alias>/<id>` of no known
/// project. A line whose every occurrence is DNP (quantity 0) buys
/// nothing and is left out. Anything else is `item_link_unresolved`.
pub fn eda_answer(stdout: &[u8], own: &ProjectId) -> Result<Vec<RepLine>> {
    let bad = |why: String| {
        Error::new(
            Code::ItemLinkUnresolved,
            format!("ryueda's bom answer {why}"),
        )
    };
    let v: Value = serde_json::from_slice(stdout).map_err(|e| bad(format!("isn't JSON: {e}")))?;
    if v.get("ok") != Some(&Value::Bool(true)) {
        let msg = v
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("is a failure");
        return Err(bad(format!("is an error: {msg}"))
            .with("ryueda", v.get("error").cloned().unwrap_or(Value::Null)));
    }
    let r = &v["result"];
    if r.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err(bad(
            "isn't schema 1: upgrade ryuplm or ryueda so they agree".to_owned(),
        ));
    }
    let list = r
        .get("lines")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("has no lines".to_owned()))?;
    let mut out = Vec::new();
    for (i, l) in list.iter().enumerate() {
        let text = |k: &str| l.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
        let part_text = text("part");
        let quantity = l
            .get("quantity")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad(format!("line {i} has no quantity")))?;
        if quantity == 0 {
            continue;
        }
        let (project, id) = if let Some(id) = part_text.strip_prefix("self/") {
            (Some(own.clone()), id)
        } else if let Some((p, id)) = part_text.split_once(':') {
            let p =
                ProjectId::parse(p).map_err(|_| bad(format!("line {i} names project {p:?}")))?;
            (Some(p), id)
        } else if let Some((_, id)) = part_text.split_once('/') {
            (None, id)
        } else {
            return Err(bad(format!("line {i} names part {part_text:?}")));
        };
        let part =
            Id::parse(id, "part").map_err(|_| bad(format!("line {i} names part {part_text:?}")))?;
        out.push(RepLine {
            key: format!("{part_text}\0{}\0{}", text("value"), text("footprint")),
            project,
            part,
            quantity,
            find: None,
            refdes: Some(text("refdes")).filter(|s| !s.is_empty()),
        });
    }
    Ok(out)
}

/// The executable [`RYUEDA_EXE`] names, else `ryueda` on PATH.
pub fn ryueda_exe() -> PathBuf {
    std::env::var_os(RYUEDA_EXE)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ryueda"))
}

/// `exe -C <dir> --no-daemon bom <part> [--rev <rev>]`, read by
/// [`eda_answer`].
pub fn eda_lines_with(
    exe: &Path,
    dir: &Path,
    own: &ProjectId,
    part: &Id,
    rev: Option<&str>,
) -> Result<Vec<RepLine>> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("-C")
        .arg(dir)
        .arg("--no-daemon")
        .arg("bom")
        .arg(part.as_str());
    if let Some(r) = rev {
        cmd.arg("--rev").arg(r);
    }
    let out = cmd
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            Error::new(
                Code::ItemLinkUnresolved,
                format!(
                    "{} couldn't be run ({e}): install ryueda, or set {RYUEDA_EXE} to it",
                    exe.display()
                ),
            )
            .with("exe", exe.display().to_string())
        })?;
    eda_answer(&out.stdout, own)
}

/// The direct children of ryucad's `part` (in `src`, loaded as if at
/// `root`) as lines: ryupdm's indented BOM one level deep, each row its
/// part, project (`own` for the project's own parts), instance count and
/// item number. A plain part has none.
#[cfg(feature = "cad")]
pub fn cad_lines(
    src: &dyn Source,
    root: &Path,
    own: &ProjectId,
    part: &Id,
) -> Result<Vec<RepLine>> {
    use ryucad_doc::doc::assembly::Rev;
    let project = ryucad_doc::doc::project::Project::load_from(root, src)?;
    let mut cache = ryupdm::structure::Cache::new();
    let mut reader = ryupdm::structure::Reader::new(src, &project, None, None, &mut cache);
    let tree = ryupdm::bom::tree(&mut reader, part, &Rev::Working, Some(1))?;
    Ok(tree
        .children
        .iter()
        .map(|c| RepLine {
            key: c.key().to_string(),
            project: Some(c.project().cloned().unwrap_or_else(|| own.clone())),
            part: c.part.clone(),
            quantity: c.count,
            find: c.item.map(|n| n.to_string()),
            refdes: None,
        })
        .collect())
}

/// The commit `rev` names in `repo`, in hex (ryupdm's own rule).
pub fn commit_hex(repo: &gix::Repository, rev: &str) -> Result<String> {
    let id = repo.rev_parse_single(rev).map_err(|e| {
        Error::new(Code::RevNotFound, format!("no revision {rev}: {e}")).with("rev", rev)
    })?;
    Ok(id
        .object()
        .map_err(ryupdm::history::git_error)?
        .peel_to_commit()
        .map_err(ryupdm::history::git_error)?
        .id
        .to_string())
}

/// Representation lines read from disk: projects found by an [`Index`],
/// ryucad's in process, ryueda's through its executable, each link and
/// revision read once.
pub struct Disk {
    pub index: Index,
    /// The catalogue's repository and work tree: a representation inside
    /// it is read at the catalogue's commit (ryucad's, behind `cad`;
    /// ryueda's through `--rev`).
    repo: Option<gix::Repository>,
    workdir: Option<PathBuf>,
    memo: BTreeMap<(String, ProjectId, Id, At), std::result::Result<Read, Error>>,
}

impl Disk {
    /// A reader over `index` for the catalogue at `here`.
    pub fn new(index: Index, here: &Path) -> Disk {
        let repo = gix::discover(here).ok();
        let workdir = repo
            .as_ref()
            .and_then(|r| r.workdir().map(Path::to_path_buf))
            .map(|w| w.canonicalize().unwrap_or(w));
        Disk {
            index,
            repo,
            workdir,
            memo: BTreeMap::new(),
        }
    }

    /// `dir`'s place in the catalogue's repository (`/`-separated, ending
    /// in `/` unless the root), when it lies in its work tree.
    fn prefix(&self, dir: &Path) -> Option<String> {
        let rel = dir.strip_prefix(self.workdir.as_ref()?).ok()?;
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some(if parts.is_empty() {
            String::new()
        } else {
            format!("{}/", parts.join("/"))
        })
    }

    fn read(&self, link: &crate::item::Link, at: &At) -> Result<Read> {
        let dir = self
            .index
            .dir(&link.product, &link.project)
            .ok_or_else(|| {
                unresolved(link, "no folder searched holds the project")
                    .with("searched", &self.index.searched)
            })?
            .to_path_buf();
        // Decision 3: at the catalogue's commit inside its work tree, else
        // the working files, and the lines say which.
        let commit = match at {
            At::Commit(c) if self.prefix(&dir).is_some() => Some(c.clone()),
            _ => None,
        };
        // Whatever a product's reader refuses with (its own not-found, a
        // file that doesn't read, ryueda's answer) is this link's
        // `item_link_unresolved`, the reader's error kept as its cause.
        let lines = self
            .product_lines(&dir, link, commit.as_deref())
            .map_err(|e| unresolved(link, &e.message).with("cause", &e))?;
        Ok(Read {
            lines,
            rev: commit.unwrap_or_else(|| "working".to_owned()),
        })
    }

    /// The lines of `link`'s representation in `dir`, at `commit` when
    /// given (the catalogue's commit, the representation being in its work
    /// tree), else its working files.
    fn product_lines(
        &self,
        dir: &Path,
        link: &crate::item::Link,
        commit: Option<&str>,
    ) -> Result<Vec<RepLine>> {
        match link.product.as_str() {
            "ryueda" => {
                // Only a design has a BOM; a library component is a leaf.
                // Looked for at the revision read, so a design added or
                // deleted since is judged as it was.
                let design = format!("designs/{}.json", link.part);
                let there = match (commit, &self.repo) {
                    (Some(c), Some(repo)) => {
                        let prefix = self.prefix(dir).unwrap_or_default();
                        ryupdm::history::file_at(repo, c, &prefix, &design)?.is_some()
                    }
                    _ => crate::source::Folder(dir).read(&design)?.is_some(),
                };
                if there {
                    eda_lines_with(&ryueda_exe(), dir, &link.project, &link.part, commit)
                } else {
                    Ok(Vec::new())
                }
            }
            "ryucad" => self.cad(dir, link, commit),
            other => Err(Error::new(
                Code::InvalidParam,
                format!("{other} isn't a product links read"),
            )),
        }
    }

    #[cfg(feature = "cad")]
    fn cad(
        &self,
        dir: &Path,
        link: &crate::item::Link,
        commit: Option<&str>,
    ) -> Result<Vec<RepLine>> {
        match (commit, &self.repo) {
            (Some(c), Some(repo)) => {
                let prefix = self.prefix(dir).unwrap_or_default();
                let files = ryupdm::history::files_for::<ryupdm::cad::Cad>(repo, c, &prefix)?;
                cad_lines(&files, Path::new(""), &link.project, &link.part)
            }
            _ => cad_lines(&crate::source::Folder(dir), dir, &link.project, &link.part),
        }
    }

    #[cfg(not(feature = "cad"))]
    fn cad(
        &self,
        _dir: &Path,
        _link: &crate::item::Link,
        _commit: Option<&str>,
    ) -> Result<Vec<RepLine>> {
        Err(Error::new(
            Code::ItemLinkUnresolved,
            "this ryuplm was built without ryucad's reader (feature cad)",
        ))
    }
}

impl Reps for Disk {
    fn lines(&mut self, link: &crate::item::Link, at: &At) -> Result<Read> {
        let key = (
            link.product.clone(),
            link.project.clone(),
            link.part.clone(),
            at.clone(),
        );
        if let Some(r) = self.memo.get(&key) {
            return r.clone();
        }
        let r = self.read(link, at);
        self.memo.insert(key, r.clone());
        r
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    pub(crate) fn pid(n: u8) -> ProjectId {
        ProjectId::from_bytes([n; 16])
    }

    /// A ryucad project at `dir` with ID `project`: `parts` as plain
    /// boxes, `assemblies` placing `(part, count)` instances.
    pub(crate) fn cad_project(
        dir: &Path,
        project: &ProjectId,
        parts: &[&str],
        assemblies: &[(&str, &[(&str, usize)])],
    ) {
        let put = |rel: &str, v: serde_json::Value| {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, crate::canonical::to_string(&v)).unwrap();
        };
        put(
            "ryucad.json",
            serde_json::json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project", "project": project}),
        );
        for part in parts {
            put(
                &format!("parts/{part}/part.json"),
                serde_json::json!({"features": ["b"], "format": 1, "kind": "ryucad.part"}),
            );
            put(
                &format!("parts/{part}/features/b.json"),
                serde_json::json!({"format": 1, "kind": "ryucad.feature", "type": "box", "x": "1", "y": "1", "z": "1"}),
            );
        }
        for (asm, children) in assemblies {
            let mut ids = Vec::new();
            for (child, count) in *children {
                for k in 0..*count {
                    let f = format!("{child}{k}");
                    put(
                        &format!("parts/{asm}/features/{f}.json"),
                        serde_json::json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                            "part": child, "rev": "working",
                            "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
                    );
                    ids.push(f);
                }
            }
            put(
                &format!("parts/{asm}/part.json"),
                serde_json::json!({"features": ids, "format": 2, "kind": "ryucad.part", "part_kind": "assembly"}),
            );
        }
    }

    #[test]
    fn projects_are_found_by_manifest_and_id() {
        let d = tempfile::tempdir().unwrap();
        let top = d.path().canonicalize().unwrap();
        cad_project(&top.join("mech"), &pid(3), &["bolt"], &[]);
        cad_project(&top.join("copy"), &pid(3), &["bolt"], &[]);
        fs::create_dir_all(top.join("eda")).unwrap();
        fs::write(
            top.join("eda/ryueda.json"),
            format!(r#"{{"kind":"ryueda.project","project":"{}"}}"#, pid(4)),
        )
        .unwrap();
        fs::create_dir_all(top.join("eda/designs")).unwrap();
        fs::write(top.join("eda/designs/main.json"), "{}").unwrap();
        fs::create_dir_all(top.join("eda/properties")).unwrap();
        fs::write(
            top.join("eda/properties/main.json"),
            r#"{"format":1,"kind":"ryupdm.properties","number":"PCB-1"}"#,
        )
        .unwrap();
        let ix = Index::scan(std::slice::from_ref(&top));
        // The first folder in sorted order wins: copy before mech.
        assert_eq!(ix.dir("ryucad", &pid(3)), Some(top.join("copy").as_path()));
        assert_eq!(ix.dir("ryueda", &pid(4)), Some(top.join("eda").as_path()));
        assert_eq!(ix.dir("ryueda", &pid(3)), None);
        assert_eq!(ix.searched, std::slice::from_ref(&top));
        let id = |s: &str| Id::parse(s, "part").unwrap();
        let mech = crate::source::Folder(&top.join("mech"));
        assert!(part_exists(&mech, "ryucad", &id("bolt")));
        assert!(!part_exists(&mech, "ryucad", &id("nut")));
        let eda = crate::source::Folder(&top.join("eda"));
        assert!(part_exists(&eda, "ryueda", &id("main")));
        assert_eq!(
            number_of(&eda, "ryueda", &id("main")).as_deref(),
            Some("PCB-1")
        );
        assert_eq!(number_of(&mech, "ryucad", &id("bolt")), None);
        assert_eq!(manifest_of("ryudfm"), None);
    }

    /// What `ryueda bom` prints: a design placing a component of its own
    /// library, one of a library project by ID, one by an alias with no
    /// project, and one whose every occurrence is DNP.
    pub(crate) fn eda_stdout(lib: &ProjectId) -> String {
        serde_json::json!({"ok": true, "warnings": [], "result": {
        "schema": 1, "design": "main", "rev": "working", "columns": [], "findings": [],
        "lines": [
            {"part": "self/r10k", "revision": "working", "quantity": 3, "value": "10k",
             "footprint": "R0603", "refdes": "R1,R2,R3"},
            {"part": format!("{lib}:mcu"), "revision": "A", "quantity": 1, "value": "RP2040",
             "footprint": "QFN56", "refdes": "U1"},
            {"part": "passives/c100n", "revision": "working", "quantity": 2, "value": "100n",
             "footprint": "C0402", "refdes": "C1,C2"},
            {"part": "self/tp", "revision": "working", "quantity": 0, "value": "",
             "footprint": "TP", "refdes": ""}
        ]}})
        .to_string()
    }

    #[test]
    fn ryuedas_answer_reads_as_lines() {
        let id = |s: &str| Id::parse(s, "part").unwrap();
        let lines = eda_answer(eda_stdout(&pid(5)).as_bytes(), &pid(4)).unwrap();
        assert_eq!(
            lines,
            [
                RepLine {
                    key: "self/r10k\u{0}10k\u{0}R0603".into(),
                    project: Some(pid(4)),
                    part: id("r10k"),
                    quantity: 3,
                    find: None,
                    refdes: Some("R1,R2,R3".into()),
                },
                RepLine {
                    key: format!("{}:mcu\u{0}RP2040\u{0}QFN56", pid(5)),
                    project: Some(pid(5)),
                    part: id("mcu"),
                    quantity: 1,
                    find: None,
                    refdes: Some("U1".into()),
                },
                RepLine {
                    key: "passives/c100n\u{0}100n\u{0}C0402".into(),
                    project: None,
                    part: id("c100n"),
                    quantity: 2,
                    find: None,
                    refdes: Some("C1,C2".into()),
                },
            ]
        );
        for bad in [
            "not json".to_owned(),
            r#"{"ok": false, "error": {"code": "not_found", "message": "no design"}}"#.to_owned(),
            r#"{"ok": true, "result": {"schema": 2, "lines": []}}"#.to_owned(),
        ] {
            let e = eda_answer(bad.as_bytes(), &pid(4)).unwrap_err();
            assert_eq!(e.code, Code::ItemLinkUnresolved, "{bad}");
        }
    }

    #[test]
    fn lines_held_in_memory_and_a_missing_executable() {
        let link = crate::item::Link {
            product: "ryueda".into(),
            project: pid(4),
            part: Id::parse("main", "part").unwrap(),
        };
        let mut fixed = Fixed::default();
        let e = fixed.lines(&link, &At::Working).unwrap_err();
        assert_eq!(e.code, Code::ItemLinkUnresolved);
        fixed.0.insert(
            ("ryueda".into(), pid(4), link.part.clone()),
            eda_answer(eda_stdout(&pid(5)).as_bytes(), &pid(4)).unwrap(),
        );
        let r = fixed.lines(&link, &At::Commit("abc".into())).unwrap();
        assert_eq!((r.lines.len(), r.rev.as_str()), (3, "abc"));
        // An executable that isn't there is a typed error, never a panic.
        let e = eda_lines_with(
            std::path::Path::new("/no/such/ryueda"),
            std::path::Path::new("."),
            &pid(4),
            &link.part,
            None,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ItemLinkUnresolved);
        assert!(e.message.contains("RYUEDA_EXE"), "{}", e.message);
    }

    #[cfg(feature = "cad")]
    #[test]
    fn ryucad_lines_are_the_assemblys_children() {
        let d = tempfile::tempdir().unwrap();
        let top = d.path().canonicalize().unwrap();
        cad_project(
            &top.join("mech"),
            &pid(3),
            &["bolt", "plate"],
            &[("frame", &[("bolt", 4), ("plate", 1)])],
        );
        let src = crate::source::Folder(&top.join("mech"));
        let id = |s: &str| Id::parse(s, "part").unwrap();
        let lines = cad_lines(&src, &top.join("mech"), &pid(3), &id("frame")).unwrap();
        let got: Vec<(String, Option<ProjectId>, u64)> = lines
            .iter()
            .map(|l| (l.part.to_string(), l.project.clone(), l.quantity))
            .collect();
        assert_eq!(
            got,
            [
                ("bolt".into(), Some(pid(3)), 4),
                ("plate".into(), Some(pid(3)), 1)
            ]
        );
        // A plain part has no lines.
        assert!(
            cad_lines(&src, &top.join("mech"), &pid(3), &id("bolt"))
                .unwrap()
                .is_empty()
        );
        // Through the disk reader, found by the index, read once.
        let mut disk = Disk::new(Index::scan(std::slice::from_ref(&top)), &top);
        let link = crate::item::Link {
            product: "ryucad".into(),
            project: pid(3),
            part: id("frame"),
        };
        let r = disk.lines(&link, &At::Working).unwrap();
        assert_eq!((r.lines.len(), r.rev.as_str()), (2, "working"));
        let gone = crate::item::Link {
            product: "ryucad".into(),
            project: pid(9),
            part: id("frame"),
        };
        assert_eq!(
            disk.lines(&gone, &At::Working).unwrap_err().code,
            Code::ItemLinkUnresolved
        );
        // A ryueda component (no designs/<id>.json) is a leaf: no executable run.
        std::fs::create_dir_all(top.join("lib")).unwrap();
        std::fs::write(
            top.join("lib/ryueda.json"),
            format!(r#"{{"project":"{}"}}"#, pid(6)),
        )
        .unwrap();
        let mut disk = Disk::new(Index::scan(std::slice::from_ref(&top)), &top);
        let comp = crate::item::Link {
            product: "ryueda".into(),
            project: pid(6),
            part: id("r10k"),
        };
        assert!(disk.lines(&comp, &At::Working).unwrap().lines.is_empty());
    }

    #[cfg(not(feature = "cad"))]
    #[test]
    fn without_cad_ryucad_links_are_unresolved() {
        let d = tempfile::tempdir().unwrap();
        let top = d.path().canonicalize().unwrap();
        cad_project(
            &top.join("mech"),
            &pid(3),
            &["bolt"],
            &[("frame", &[("bolt", 1)])],
        );
        let mut disk = Disk::new(Index::scan(std::slice::from_ref(&top)), &top);
        let link = crate::item::Link {
            product: "ryucad".into(),
            project: pid(3),
            part: Id::parse("frame", "part").unwrap(),
        };
        let e = disk.lines(&link, &At::Working).unwrap_err();
        assert!(e.message.contains("feature cad"), "{}", e.message);
    }

    #[test]
    fn a_ryueda_design_is_looked_for_at_the_commit_read() {
        // A design committed, then deleted from the work tree: read at the
        // commit, it has a BOM (so the executable is run, and here fails:
        // unresolved), never the working tree's "no design, a leaf".
        let d = tempfile::tempdir().unwrap();
        let top = d.path().canonicalize().unwrap();
        let eda = top.join("eda");
        fs::create_dir_all(eda.join("designs")).unwrap();
        fs::write(
            eda.join("ryueda.json"),
            format!(r#"{{"project":"{}"}}"#, pid(4)),
        )
        .unwrap();
        fs::write(eda.join("designs/main.json"), "{}").unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .current_dir(&top)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "a design"]);
        let repo = gix::discover(&top).unwrap();
        let commit = commit_hex(&repo, "HEAD").unwrap();
        fs::remove_file(eda.join("designs/main.json")).unwrap();
        // SAFETY: no other test reads RYUEDA_EXE.
        unsafe { std::env::set_var(RYUEDA_EXE, "/no/such/ryueda") };
        let link = crate::item::Link {
            product: "ryueda".into(),
            project: pid(4),
            part: Id::parse("main", "part").unwrap(),
        };
        let mut disk = Disk::new(Index::scan(std::slice::from_ref(&top)), &top);
        assert!(disk.lines(&link, &At::Working).unwrap().lines.is_empty());
        let e = disk.lines(&link, &At::Commit(commit)).unwrap_err();
        assert_eq!(e.code, Code::ItemLinkUnresolved);
        assert_eq!(e.details["product"], "ryueda");
    }
}
