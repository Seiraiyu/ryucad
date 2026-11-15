//! A project: a folder with `ryueda.json`, its own library under
//! `library/`, and library projects beside it that its manifest names by
//! alias. A library project is a project whose manifest says
//! `project_kind: library`; it holds a library and no design.

use crate::design::{self, Connectivity};
use crate::doc;
use crate::error::{Code, Error, Result};
use crate::fault::{Fault, Severity};
use crate::footprint::Footprint;
use crate::id::{Id, ProjectId};
use crate::libcomp::{LibComp, LibRef};
use crate::library::{self, Library, Loaded, ObjKind};
use crate::source::{Source, join, plain};
use crate::symbol::Symbol;
use crate::units::Unit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "ryueda.json";
pub const KIND: &str = "ryueda.project";
pub const FORMAT: u64 = 1;
/// A manifest with properties or schematic settings.
pub const FORMAT_2: u64 = 2;
/// A manifest naming the project it was forked from (`pdm new-id`;
/// design 629); 10.C's library pins and URLs are format 3 too.
pub const FORMAT_3: u64 = 3;
/// The alias of a project's own library, never written.
pub const SELF: &str = "self";
/// ERC's severities and waivers, beside the manifest.
pub const ERC: &str = "erc.json";
/// The schematic lint's preferences, beside the manifest.
pub const LINT: &str = "lint.json";
/// The project's style, beside the manifest (plan 2.E).
pub const STYLE: &str = crate::style::FILE;
/// The project's sourcing (design 347), beside the manifest.
pub const PARTS: &str = "parts.json";
/// The Git attributes file, beside the manifest.
pub const ATTRIBUTES: &str = ".gitattributes";
/// Which commit each pinned label of a library in another repository
/// resolved to (design 629): ryupdm's lock, this kind.
pub const LOCK: &str = "ryueda.lock";
pub const LOCK_KIND: &str = "ryueda.lock";
/// The files at a project's root that it is made of.
pub const ROOT_FILES: [&str; 8] = [
    MANIFEST,
    ATTRIBUTES,
    ERC,
    LINT,
    PARTS,
    STYLE,
    crate::pdm::PDM_CONFIG,
    LOCK,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    /// Designs and a library of its own.
    #[default]
    Design,
    /// Only a library, for other projects to name.
    Library,
}

impl ProjectKind {
    fn is_design(&self) -> bool {
        *self == ProjectKind::Design
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// 128 random bits, written at `init`: the same form as a ryucad
    /// manifest's, so ryupdm can name ryueda projects.
    pub project: ProjectId,
    /// The unit of lengths an agent types as bare numbers.
    pub default_unit: Unit,
    #[serde(default, skip_serializing_if = "ProjectKind::is_design")]
    pub project_kind: ProjectKind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub libraries: BTreeMap<Id, Alias>,
    /// The outermost level of properties (format 2).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: crate::props::Properties,
    /// How schematics are drawn (format 2): the sheet's paper.
    #[serde(default, skip_serializing_if = "Schematic::is_default")]
    pub schematic: Schematic,
    /// The project this one was copied from, once `pdm new-id` gave it an
    /// ID of its own (format 3): its records naming that ID are the
    /// upstream's history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ForkedFrom>,
}

/// The project a fork's ID replaced, and the commit it was at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkedFrom {
    pub project: ProjectId,
    pub commit: String,
}

/// A project's schematic settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schematic {
    #[serde(default)]
    pub sheet: Sheet,
    /// The library project (an alias in `libraries`) whose `style.json`
    /// is the organisation's style (plan 2.E).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_from: Option<Id>,
}

impl Schematic {
    fn is_default(&self) -> bool {
        *self == Schematic::default()
    }
}

/// The paper a schematic's sheets are laid out to fit: `size` the
/// largest, and with `fit` each sheet on the smallest of `sizes` (up to
/// `size`) that holds it (plan 2.A).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sheet {
    #[serde(default)]
    pub size: PaperSize,
    #[serde(default)]
    pub orientation: Orientation,
    /// Each sheet on the smallest paper of `sizes` that holds it; off,
    /// every sheet is `size`.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub fit: bool,
    /// The papers a sheet may take, up to `size`.
    #[serde(default = "default_sizes", skip_serializing_if = "is_default_sizes")]
    pub sizes: Vec<PaperSize>,
}

fn yes() -> bool {
    true
}

fn is_yes(v: &bool) -> bool {
    *v
}

fn default_sizes() -> Vec<PaperSize> {
    vec![PaperSize::A4, PaperSize::A3]
}

fn is_default_sizes(v: &[PaperSize]) -> bool {
    v == default_sizes().as_slice()
}

impl Default for Sheet {
    fn default() -> Sheet {
        Sheet {
            size: PaperSize::default(),
            orientation: Orientation::default(),
            fit: true,
            sizes: default_sizes(),
        }
    }
}

impl PaperSize {
    /// Every size, as files name them.
    pub const ALL: [PaperSize; 10] = [
        PaperSize::A4,
        PaperSize::A3,
        PaperSize::A2,
        PaperSize::A1,
        PaperSize::A0,
        PaperSize::AnsiA,
        PaperSize::AnsiB,
        PaperSize::AnsiC,
        PaperSize::AnsiD,
        PaperSize::AnsiE,
    ];

    /// Its long side, for ordering the series.
    fn long(self) -> i64 {
        let s = Sheet {
            size: self,
            ..Sheet::default()
        };
        s.size_nm().0
    }

    /// Its name in files (`a4`, `ansi_b`).
    pub fn as_str(self) -> &'static str {
        match self {
            PaperSize::A4 => "a4",
            PaperSize::A3 => "a3",
            PaperSize::A2 => "a2",
            PaperSize::A1 => "a1",
            PaperSize::A0 => "a0",
            PaperSize::AnsiA => "ansi_a",
            PaperSize::AnsiB => "ansi_b",
            PaperSize::AnsiC => "ansi_c",
            PaperSize::AnsiD => "ansi_d",
            PaperSize::AnsiE => "ansi_e",
        }
    }

    /// The size a file's name gives.
    pub fn parse(name: &str) -> Option<PaperSize> {
        PaperSize::ALL.into_iter().find(|p| p.as_str() == name)
    }
}

/// ISO A4 to A0 and ANSI A to E.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperSize {
    A4,
    #[default]
    A3,
    A2,
    A1,
    A0,
    AnsiA,
    AnsiB,
    AnsiC,
    AnsiD,
    AnsiE,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    #[default]
    Landscape,
    Portrait,
}

impl Sheet {
    /// The papers a sheet may take, smallest first, the project's size
    /// last; none when `fit` is off.
    pub fn series(&self) -> Vec<PaperSize> {
        if !self.fit {
            return Vec::new();
        }
        let top = self.size.long();
        let mut out: Vec<PaperSize> = self
            .sizes
            .iter()
            .copied()
            .filter(|s| s.long() <= top && *s != self.size)
            .collect();
        out.sort_by_key(|s| s.long());
        out.dedup();
        out.push(self.size);
        out
    }

    /// The paper's width and height in nanometres.
    pub fn size_nm(&self) -> (i64, i64) {
        const MM: i64 = 1_000_000;
        const IN: i64 = 25_400_000;
        let (long, short) = match self.size {
            PaperSize::A4 => (297 * MM, 210 * MM),
            PaperSize::A3 => (420 * MM, 297 * MM),
            PaperSize::A2 => (594 * MM, 420 * MM),
            PaperSize::A1 => (841 * MM, 594 * MM),
            PaperSize::A0 => (1189 * MM, 841 * MM),
            PaperSize::AnsiA => (11 * IN, 17 * IN / 2),
            PaperSize::AnsiB => (17 * IN, 11 * IN),
            PaperSize::AnsiC => (22 * IN, 17 * IN),
            PaperSize::AnsiD => (34 * IN, 22 * IN),
            PaperSize::AnsiE => (44 * IN, 34 * IN),
        };
        match self.orientation {
            Orientation::Landscape => (long, short),
            Orientation::Portrait => (short, long),
        }
    }
}

/// A library project: by its folder relative to this one, in this
/// repository; or, with `url`, in another repository at `path` inside it
/// (`""` for its root). `labels` pins components (design 629).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alias {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The library project's ID: written with `url`, and once one of its
    /// components is pinned (format 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectId>,
    /// Each pinned component's label (format 3).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<Id, String>,
}

impl Alias {
    /// A library project in this repository, nothing pinned.
    pub fn local(path: &str) -> Alias {
        Alias {
            path: path.to_owned(),
            url: None,
            project: None,
            labels: BTreeMap::new(),
        }
    }

    /// Whether it is in another repository.
    pub fn is_foreign(&self) -> bool {
        self.url.is_some()
    }

    /// Whether it needs format 3.
    fn is_format_3(&self) -> bool {
        self.url.is_some() || self.project.is_some() || !self.labels.is_empty()
    }
}

impl Manifest {
    pub fn new(project: ProjectId, default_unit: Unit, kind: ProjectKind) -> Manifest {
        Manifest {
            project,
            default_unit,
            project_kind: kind,
            libraries: BTreeMap::new(),
            properties: BTreeMap::new(),
            schematic: Schematic::default(),
            forked_from: None,
        }
    }

    /// Reads a manifest. An alias named `self` is refused: it would stand
    /// in for the project's own library, so writes would land in another
    /// project, outside the activity log.
    pub fn read(bytes: &[u8]) -> Result<Manifest> {
        let m: Manifest = doc::read(bytes, MANIFEST, KIND, FORMAT_3)?;
        if let Some(f) = crate::props::faults(&m.properties, "/properties").first() {
            return Err(f.error.clone().with("path", MANIFEST));
        }
        if m.libraries.keys().any(|a| a.as_str() == SELF) {
            return Err(Error::new(
                Code::InvalidDocument,
                format!("{MANIFEST}: no library may be named {SELF}, the project's own"),
            )
            .with("path", MANIFEST)
            .with("pointer", format!("/libraries/{SELF}")));
        }
        for (alias, a) in &m.libraries {
            let at = format!("/libraries/{alias}");
            if a.url.is_some() && a.project.is_none() {
                return Err(Error::new(
                    Code::InvalidDocument,
                    format!(
                        "{MANIFEST}: library {alias} is in another repository and names no project"
                    ),
                )
                .with("path", MANIFEST)
                .with("pointer", format!("{at}/project")));
            }
            if a.url.as_deref().is_some_and(|u| u.trim().is_empty()) {
                return Err(Error::new(
                    Code::InvalidDocument,
                    format!("{MANIFEST}: library {alias}'s url is empty"),
                )
                .with("path", MANIFEST)
                .with("pointer", format!("{at}/url")));
            }
            if a.url.is_some()
                && !a.path.is_empty()
                && (!path_ok(&a.path) || a.path.split('/').any(|s| s == ".."))
            {
                return Err(Error::new(
                    Code::InvalidDocument,
                    format!(
                        "{MANIFEST}: library {alias}'s path {:?} isn't folder names joined by /",
                        a.path
                    ),
                )
                .with("path", MANIFEST)
                .with("pointer", format!("{at}/path")));
            }
            if let Some((c, _)) = a.labels.iter().find(|(_, l)| l.trim().is_empty()) {
                return Err(Error::new(
                    Code::InvalidDocument,
                    format!("{MANIFEST}: {alias}/{c} is pinned to no label"),
                )
                .with("path", MANIFEST)
                .with("pointer", format!("{at}/labels/{c}")));
            }
        }
        Ok(m)
    }

    /// Its canonical text: format 2 only when it holds what format 1
    /// can't.
    pub fn to_text(&self) -> String {
        let format =
            if self.forked_from.is_some() || self.libraries.values().any(Alias::is_format_3) {
                FORMAT_3
            } else if !self.properties.is_empty() || self.schematic != Schematic::default() {
                FORMAT_2
            } else {
                FORMAT
            };
        doc::to_string(self, KIND, format)
    }
}

/// Whether `path` is written as an alias's path must be: relative, with
/// `/` between names, none of them empty or `.`, nothing a platform would
/// read as a root or drive (`/x`, `C:`, `\\`).
pub fn path_ok(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && path.split('/').all(|s| !s.is_empty() && s != ".")
}

/// Where each alias's library project is, relative to the project (as the
/// manifest writes it), or why it can't be used: a path written wrong, no
/// project there, not a library, one naming libraries of its own, or one
/// outside the repository (outside a repository, outside the project's
/// parent folder), through a symlink or not.
pub fn resolve(root: &Path, m: &Manifest) -> BTreeMap<Id, Result<String>> {
    let canonical = root.canonicalize().map(plain);
    let fence = canonical.as_ref().ok().and_then(|r| boundary(r));
    m.libraries
        .iter()
        .map(|(alias, a)| {
            let found = match &a.url {
                Some(url) => Err(Error::new(
                    Code::LibraryForeignWorking,
                    format!(
                        "library {alias} is in another repository ({url}): only its pinned components are read; pin them with `ryueda lib pin`"
                    ),
                )),
                None => resolve_one(
                    root,
                    &a.path,
                    a.project.as_ref(),
                    canonical.as_deref().ok(),
                    fence.as_deref(),
                ),
            };
            (
                alias.clone(),
                found.map_err(|e| e.with("alias", alias).with("path", &a.path)),
            )
        })
        .collect()
}

/// The folder library projects must be in: the repository holding `root`,
/// else `root`'s parent.
fn boundary(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .find(|d| d.join(".git").exists())
        .or(root.parent())
        .map(Path::to_path_buf)
}

fn missing(why: &str) -> Error {
    Error::new(Code::LibraryMissing, why.to_owned())
}

fn resolve_one(
    root: &Path,
    path: &str,
    expected: Option<&ProjectId>,
    canonical: Option<&Path>,
    fence: Option<&Path>,
) -> Result<String> {
    if !path_ok(path) {
        return Err(missing(&format!(
            "library path {path:?} isn't a relative path of folder names joined by /"
        )));
    }
    let at = path.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    let found = at
        .canonicalize()
        .map(plain)
        .map_err(|_| missing(&format!("no folder at {path}")))?;
    let (Some(canonical), Some(fence)) = (canonical, fence) else {
        return Err(missing("the project's own folder can't be found"));
    };
    if found == canonical || !found.starts_with(fence) {
        return Err(missing(&format!(
            "{path} isn't a library project beside this one, inside {}",
            fence.display()
        )));
    }
    let bytes = std::fs::read(found.join(MANIFEST))
        .map_err(|_| missing(&format!("{path} holds no ryueda project ({MANIFEST})")))?;
    let m = Manifest::read(&bytes)
        .map_err(|e| missing(&format!("{path}/{MANIFEST}: {}", e.message)))?;
    if m.project_kind != ProjectKind::Library {
        return Err(missing(&format!(
            "{path} is a design project, not a library project"
        )));
    }
    if let Some(want) = expected
        && m.project != *want
    {
        return Err(Error::new(
            Code::DependencyProjectMismatch,
            format!(
                "{path} holds project {}, not {want}, which the manifest names",
                m.project
            ),
        )
        .with("project", want)
        .with("found", &m.project));
    }
    if !m.libraries.is_empty() {
        return Err(Error::new(
            Code::LibraryNested,
            format!("{path} names libraries of its own; a library project can't"),
        ));
    }
    Ok(path.to_owned())
}

/// A loaded project.
#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    /// A hash of the files that define the project as a whole: the
    /// manifest, `erc.json`, the designs, rails and classes. Writes to
    /// them are guarded by it.
    pub revision: String,
    /// `self` and every alias, each its library or why it can't be used.
    pub libraries: BTreeMap<String, Result<Library>>,
    /// Each pinned component, by (alias, component), as its label has it
    /// or why it can't be read: a library of its own (design 629).
    pub pinned: BTreeMap<(String, Id), Result<std::sync::Arc<crate::pin::Pinned>>>,
    /// Designs, blocks, rails and classes.
    pub conn: Connectivity,
    /// `erc.json`, read (empty when there is none) with its faults.
    pub erc: Loaded<crate::erc::Config>,
    /// `lint.json`, read (the defaults when there is none) with its faults.
    pub lint: Loaded<ryusch::lint::Prefs>,
    /// `parts.json`, read (the defaults when there is none) with its faults.
    pub parts: Loaded<crate::sourcing::Config>,
    /// `style.json`, read (the empty style when there is none) with its
    /// faults.
    pub style: Loaded<crate::style::Style>,
    /// The organisation's style, when the manifest names one.
    pub org_style: Option<OrgStyle>,
}

/// The organisation's style: the library project `schematic.style_from`
/// names, and its `style.json` as read.
#[derive(Clone, Debug)]
pub struct OrgStyle {
    pub alias: String,
    /// The file, relative to this project (`../org/style.json`).
    pub path: String,
    pub style: Loaded<crate::style::Style>,
}

fn read_style(bytes: &[u8], path: &str) -> Result<crate::style::Style> {
    doc::read::<crate::style::Style>(bytes, path, crate::style::KIND, crate::style::FORMAT).map_err(
        |e| {
            if e.code == Code::FormatTooNew {
                e
            } else {
                Error::new(Code::StyleInvalid, e.message.clone()).with("path", path)
            }
        },
    )
}

/// The nearest folder at or above `start` holding a `ryueda.json`.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    let start = plain(start.canonicalize().map_err(|e| Error::io(start, &e))?);
    start
        .ancestors()
        .find(|d| d.join(MANIFEST).is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            Error::new(
                Code::ProjectNotFound,
                format!(
                    "no {MANIFEST} in {} or any folder above it; run `ryueda init` or pass -C <dir>",
                    start.display()
                ),
            )
            .with("path", &start)
        })
}

/// A reference's text: `lib/<alias>/<kind>/<id>`.
pub fn ref_text(alias: &str, kind: ObjKind, id: &Id) -> String {
    format!("lib/{alias}/{}/{id}", kind.as_str())
}

impl Project {
    /// Loads the project at `root` from `src`, with its aliases resolved
    /// as [`resolve`] found them.
    pub fn load_from(
        root: &Path,
        src: &dyn Source,
        aliases: &BTreeMap<Id, Result<String>>,
        pins: &dyn crate::pin::Resolve,
    ) -> Result<Project> {
        let bytes = src.read(MANIFEST)?.ok_or_else(|| {
            Error::new(
                Code::ProjectNotFound,
                format!("{} has no {MANIFEST}", root.display()),
            )
            .with("path", root)
        })?;
        let manifest = Manifest::read(&bytes)?;
        let erc = src.read(ERC)?;
        let lint = src.read(LINT)?;
        let parts = src.read(PARTS)?;
        let style = src.read(STYLE)?;
        let mut conn = design::load(src)?;
        let mut h = blake3::Hasher::new();
        h.update(b"ryueda.project.1");
        design::piece(&mut h, &bytes);
        match &erc {
            Some(e) => design::piece(&mut h, e),
            None => {
                h.update(b"-");
            }
        }
        h.update(&conn.top);
        // Only a project with lint preferences hashes them, so others'
        // revisions are as they were.
        if let Some(l) = &lint {
            h.update(b"ryueda.lint");
            design::piece(&mut h, l);
        }
        // Likewise its sourcing.
        if let Some(b) = &parts {
            h.update(b"ryueda.parts");
            design::piece(&mut h, b);
        }
        // And its style (plan 2.E).
        if let Some(b) = &style {
            h.update(b"ryueda.style");
            design::piece(&mut h, b);
        }
        let revision = h.finalize().to_hex()[..32].to_owned();
        let lint = match &lint {
            None => Loaded {
                item: Ok(ryusch::lint::Prefs::default()),
                faults: Vec::new(),
            },
            Some(bytes) => match doc::read::<ryusch::lint::Prefs>(
                bytes,
                LINT,
                crate::lint::KIND,
                crate::lint::FORMAT,
            ) {
                Ok(mut c) => {
                    crate::lint::upgrade(&mut c);
                    Loaded {
                        faults: crate::lint::check(&c)
                            .into_iter()
                            .map(|f| f.with("path", LINT))
                            .collect(),
                        item: Ok(c),
                    }
                }
                Err(e) => Loaded {
                    item: Err(e),
                    faults: Vec::new(),
                },
            },
        };
        let parts = match &parts {
            None => Loaded {
                item: Ok(crate::sourcing::Config::default()),
                faults: Vec::new(),
            },
            Some(bytes) => match doc::read::<crate::sourcing::Config>(
                bytes,
                PARTS,
                crate::sourcing::KIND,
                crate::sourcing::FORMAT,
            ) {
                Ok(c) => Loaded {
                    faults: c
                        .check()
                        .into_iter()
                        .map(|f| f.with("path", PARTS))
                        .collect(),
                    item: Ok(c),
                },
                Err(e) => Loaded {
                    item: Err(e),
                    faults: Vec::new(),
                },
            },
        };
        let erc = match &erc {
            None => Loaded {
                item: Ok(crate::erc::Config::default()),
                faults: Vec::new(),
            },
            Some(bytes) => match doc::read::<crate::erc::Config>(
                bytes,
                ERC,
                crate::erc::KIND,
                crate::erc::FORMAT,
            ) {
                Ok(c) => Loaded {
                    faults: c.check().into_iter().map(|f| f.with("path", ERC)).collect(),
                    item: Ok(c),
                },
                Err(e) => Loaded {
                    item: Err(e),
                    faults: Vec::new(),
                },
            },
        };
        let mut libraries = BTreeMap::new();
        libraries.insert(SELF.to_owned(), library::load(src, SELF, ""));
        for alias in manifest.libraries.keys() {
            let lib = match aliases.get(alias) {
                Some(Ok(base)) => library::load(src, alias.as_str(), base),
                Some(Err(e)) => Err(e.clone()),
                None => {
                    Err(missing(&format!("library {alias} isn't resolved")).with("alias", alias))
                }
            };
            libraries.insert(alias.to_string(), lib);
        }
        let lock = src
            .read(LOCK)?
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|t| crate::canonical::parse(&t).ok())
            .and_then(|v| ryu_doc::lock::Lock::from_json_as(&v, LOCK_KIND).ok());
        let mut pinned = BTreeMap::new();
        for (alias, a) in &manifest.libraries {
            for (c, label) in &a.labels {
                pinned.insert(
                    (alias.to_string(), c.clone()),
                    pins.pin(root, alias, a, c, label, lock.as_ref()),
                );
            }
        }
        let org_style = manifest.schematic.style_from.as_ref().map(|alias| {
            let base = libraries
                .get(alias.as_str())
                .and_then(|l| l.as_ref().ok())
                .map(|l| l.base.clone());
            let path = join(&base.clone().unwrap_or_default(), STYLE);
            let item = match base {
                None => Err(Error::new(
                    Code::StyleInvalid,
                    format!(
                        "schematic.style_from names {alias}, which isn't a usable library alias"
                    ),
                )
                .with("path", MANIFEST)
                .with("pointer", "/schematic/style_from")),
                Some(_) => match src.read(&path) {
                    Ok(Some(b)) => read_style(&b, &path),
                    Ok(None) => Err(Error::new(
                        Code::StyleInvalid,
                        format!("{alias}'s project has no {STYLE}"),
                    )
                    .with("path", &path)),
                    Err(e) => Err(e),
                },
            };
            OrgStyle {
                alias: alias.to_string(),
                path,
                style: Loaded {
                    item,
                    faults: Vec::new(),
                },
            }
        });
        let style = Loaded {
            item: match &style {
                None => Ok(crate::style::Style::default()),
                Some(b) => read_style(b, STYLE),
            },
            faults: Vec::new(),
        };
        let mut p = Project {
            root: root.to_path_buf(),
            manifest,
            revision,
            libraries,
            pinned,
            conn: Connectivity::default(),
            erc,
            lint,
            parts,
            style,
            org_style,
        };
        let builtin = crate::style::builtin();
        let org_palette: Vec<String> = p
            .org_style
            .as_ref()
            .and_then(|o| o.style.item.as_ref().ok())
            .map(|s| s.palette.keys().cloned().collect())
            .unwrap_or_default();
        let own_palette: Vec<String> = p
            .style
            .item
            .as_ref()
            .map(|s| s.palette.keys().cloned().collect())
            .unwrap_or_default();
        let known = |n: &str| {
            builtin.palette.contains_key(n)
                || org_palette.iter().any(|x| x == n)
                || own_palette.iter().any(|x| x == n)
        };
        if let Some(o) = &mut p.org_style
            && let Ok(s) = &o.style.item
        {
            o.style.faults = s
                .check(crate::style::Layer::Organisation, &known)
                .into_iter()
                .map(|f| f.with("path", &o.path))
                .collect();
        }
        if let Ok(s) = &p.style.item {
            p.style.faults = s
                .check(crate::style::Layer::Project, &known)
                .into_iter()
                .map(|f| f.with("path", STYLE))
                .collect();
        }
        p.judge_components();
        conn.judge(&|r| p.libcomp(SELF, r).map(drop));
        if p.manifest.project_kind == ProjectKind::Library {
            let held = [
                (crate::conn::DESIGNS, conn.designs.is_empty()),
                (crate::conn::BLOCKS, conn.blocks.is_empty()),
                (crate::conn::RAILS, conn.rails.is_empty()),
            ];
            for (folder, empty) in held {
                if !empty {
                    conn.faults.push(
                        Fault::error(
                            Code::InvalidDocument,
                            format!("a library project holds no {folder}"),
                        )
                        .with("path", folder),
                    );
                }
            }
        }
        p.conn = conn;
        Ok(p)
    }

    /// Loads the project in its folder.
    pub fn load(root: &Path) -> Result<Project> {
        let bytes =
            std::fs::read(root.join(MANIFEST)).map_err(|e| Error::io(&root.join(MANIFEST), &e))?;
        let aliases = resolve(root, &Manifest::read(&bytes)?);
        Project::load_from(
            root,
            &crate::source::Folder(root),
            &aliases,
            &crate::pin::Pins::default(),
        )
    }

    /// The library `alias` names, or why it can't be used.
    /// The standards that judge library `alias`'s symbols, and where they
    /// come from (the alias whose file they are, or `built_in`): the
    /// library's own `standards.json`, else the organisation's when its
    /// style pins them (plan 2.E), else this project's own, else the
    /// built-in standards, whose resistor shape follows `lint.json`'s
    /// `prefer.resistor`. A file that doesn't read, or fails its checks,
    /// judges nothing: its fault is the answer (`standards_invalid`).
    pub fn standards_for(&self, alias: &str) -> Result<(crate::standards::Standards, String)> {
        self.standards_with(alias, &crate::lint::prefs(self))
    }

    /// [`Project::standards_for`] under lint preferences `prefs` (what
    /// `lint configure --dry-run` tries).
    pub fn standards_with(
        &self,
        alias: &str,
        prefs: &ryusch::lint::Prefs,
    ) -> Result<(crate::standards::Standards, String)> {
        use crate::standards::{Shapes, Standards};
        let own = |a: &str| -> Result<Option<Standards>> {
            let Some(st) = &self.library(a)?.standards else {
                return Ok(None);
            };
            let s = st.item.as_ref().map_err(|e| e.clone())?;
            if let Some(f) = st.faults.iter().find(|f| f.is_error()) {
                return Err(f.error.clone());
            }
            Ok(Some(s.clone()))
        };
        if let Some(s) = own(alias)? {
            return Ok((s, alias.to_owned()));
        }
        // The organisation's pinned standards take the project's own place
        // (plan 2.E; plan 2.D's question 444).
        if let Some(o) = &self.org_style
            && o.style.item.as_ref().is_ok_and(|s| s.standards)
            && let Some(s) = own(&o.alias)?
        {
            return Ok((s, o.alias.clone()));
        }
        if alias != SELF
            && let Some(s) = own(SELF)?
        {
            return Ok((s, SELF.to_owned()));
        }
        let mut s = Standards::default();
        if prefs.prefer.resistor == ryusch::lint::ResistorStyle::Iec {
            s.shapes = Shapes::Iec;
        }
        Ok((s, "built_in".to_owned()))
    }

    pub fn library(&self, alias: &str) -> Result<&Library> {
        match self.libraries.get(alias) {
            Some(Ok(l)) => Ok(l),
            Some(Err(e)) => Err(e.clone()),
            None => Err(Error::new(
                Code::LibraryMissing,
                format!("the project names no library {alias}"),
            )
            .with("alias", alias)),
        }
    }

    /// The library a reference from a file in library `from` reaches: its
    /// own when it names none or `self` (as its own project reads it). A
    /// library project's files name only their own library.
    fn reached(&self, from: &str, r: &LibRef) -> Result<&Library> {
        let named = r.library.as_ref().map(Id::as_str).filter(|l| *l != SELF);
        let to = named.unwrap_or(from);
        let nested = from != SELF || self.manifest.project_kind == ProjectKind::Library;
        if nested && named.is_some() {
            return Err(Error::new(
                Code::LibraryNested,
                format!("a library project's files name only their own library, not {to}"),
            )
            .with("library", to));
        }
        self.library(to)
    }

    fn object<'a, T>(
        &self,
        lib: &'a Library,
        map: &'a BTreeMap<Id, Loaded<T>>,
        kind: ObjKind,
        id: &Id,
    ) -> Result<&'a T> {
        let at = ref_text(&lib.alias, kind, id);
        match map.get(id) {
            Some(Loaded { item: Ok(t), .. }) => Ok(t),
            Some(Loaded { item: Err(e), .. }) => Err(e.clone().with("reference", &at)),
            None => Err(
                Error::new(Code::NotFound, format!("{at} isn't in the library"))
                    .with("reference", &at),
            ),
        }
    }

    /// The symbol a file in library `from` names.
    pub fn symbol(&self, from: &str, r: &LibRef) -> Result<&Symbol> {
        let lib = self.reached(from, r)?;
        self.object(lib, &lib.symbols, ObjKind::Symbol, &r.id)
    }

    /// The library component a file in library `from` names (a design's
    /// files are `self`'s): a pinned one as its label has it.
    pub fn libcomp(&self, from: &str, r: &LibRef) -> Result<&LibComp> {
        let lib = if from == SELF {
            self.home(r)?
        } else {
            self.reached(from, r)?
        };
        self.object(lib, &lib.components, ObjKind::Libcomp, &r.id)
    }

    /// A pinned placement's own library, as its label has it (or why it
    /// can't be read); `None` when `of` isn't pinned.
    fn pinned_library(&self, of: &LibRef) -> Option<Result<&Library>> {
        let alias = of.library.as_ref().filter(|a| a.as_str() != SELF)?;
        let p = self.pinned.get(&(alias.to_string(), of.id.clone()))?;
        Some(
            p.as_ref()
                .map(|p| &p.library)
                .map_err(|e| e.clone().with("alias", alias).with("component", &of.id)),
        )
    }

    /// The library a placement `of` (a design's reference) reads from: a
    /// pinned component's own, as its label has it; else the alias's at
    /// `working`, or `self`.
    pub fn home(&self, of: &LibRef) -> Result<&Library> {
        match self.pinned_library(of) {
            Some(lib) => lib,
            None => self.reached(SELF, of),
        }
    }

    /// The symbol placed component `of` names (`r`, from its library).
    pub fn placed_symbol(&self, of: &LibRef, r: &LibRef) -> Result<&Symbol> {
        match self.pinned_library(of) {
            Some(lib) => {
                let lib = lib?;
                self.object(lib, &lib.symbols, ObjKind::Symbol, &r.id)
            }
            None => self.symbol(of.library.as_ref().map_or(SELF, Id::as_str), r),
        }
    }

    /// A footprint placed component `of` lists (`r`, from its library).
    pub fn placed_footprint(&self, of: &LibRef, r: &LibRef) -> Result<&Footprint> {
        match self.pinned_library(of) {
            Some(lib) => {
                let lib = lib?;
                self.object(lib, &lib.footprints, ObjKind::Footprint, &r.id)
            }
            None => self.footprint(of.library.as_ref().map_or(SELF, Id::as_str), r),
        }
    }

    /// The footprint a file in library `from` names.
    pub fn footprint(&self, from: &str, r: &LibRef) -> Result<&Footprint> {
        let lib = self.reached(from, r)?;
        self.object(lib, &lib.footprints, ObjKind::Footprint, &r.id)
    }

    /// Gives every library component the faults its checks find against
    /// the symbol and footprints it names.
    fn judge_components(&mut self) {
        let mut found: Vec<(String, Id, Vec<Fault>)> = Vec::new();
        for (alias, lib) in &self.libraries {
            let Ok(lib) = lib else { continue };
            for (id, c) in &lib.components {
                let Ok(c) = &c.item else { continue };
                let symbol = self.symbol(alias, &c.symbol);
                let footprints: Vec<Result<&Footprint>> = c
                    .footprints
                    .iter()
                    .map(|u| self.footprint(alias, &u.footprint))
                    .collect();
                let path = lib.path(ObjKind::Libcomp, id);
                let faults = c
                    .check(symbol, &footprints)
                    .into_iter()
                    .chain(c.check_data())
                    .map(|f| f.with("path", &path))
                    .collect();
                found.push((alias.clone(), id.clone(), faults));
            }
        }
        for (alias, id, faults) in found {
            if let Some(Ok(lib)) = self.libraries.get_mut(&alias) {
                lib.components.get_mut(&id).expect("judged").faults = faults;
            }
        }
    }

    /// Every fault of every library: unusable aliases, files that don't
    /// read, and what checks find, in library and path order.
    pub fn faults(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        for (alias, lib) in &self.libraries {
            let lib = match lib {
                Ok(l) => l,
                // A library in another repository has no working library:
                // the placements that read it report it themselves.
                Err(e) if e.code == Code::LibraryForeignWorking => continue,
                Err(e) => {
                    out.push(Fault {
                        severity: Severity::Error,
                        error: e.clone().with("alias", alias),
                    });
                    continue;
                }
            };
            out.extend(lib.faults.iter().cloned());
            if let Some(st) = &lib.standards {
                if let Err(e) = &st.item {
                    out.push(Fault {
                        severity: Severity::Error,
                        error: e.clone(),
                    });
                }
                out.extend(st.faults.iter().cloned());
            }
            for kind in [ObjKind::Symbol, ObjKind::Footprint, ObjKind::Libcomp] {
                for id in lib.ids(kind) {
                    let (item, faults) = match kind {
                        ObjKind::Symbol => {
                            let l = &lib.symbols[id];
                            (l.item.as_ref().err(), &l.faults)
                        }
                        ObjKind::Footprint => {
                            let l = &lib.footprints[id];
                            (l.item.as_ref().err(), &l.faults)
                        }
                        ObjKind::Libcomp => {
                            let l = &lib.components[id];
                            (l.item.as_ref().err(), &l.faults)
                        }
                    };
                    if let Some(e) = item {
                        out.push(Fault {
                            severity: Severity::Error,
                            error: e.clone(),
                        });
                    }
                    out.extend(faults.iter().cloned());
                }
            }
        }
        if let Err(e) = &self.erc.item {
            out.push(Fault {
                severity: Severity::Error,
                error: e.clone().with("path", ERC),
            });
        }
        out.extend(self.erc.faults.iter().cloned());
        if let Err(e) = &self.lint.item {
            out.push(Fault {
                severity: Severity::Error,
                error: e.clone().with("path", LINT),
            });
        }
        out.extend(self.lint.faults.iter().cloned());
        if let Err(e) = &self.parts.item {
            out.push(Fault {
                severity: Severity::Error,
                error: e.clone().with("path", PARTS),
            });
        }
        out.extend(self.parts.faults.iter().cloned());
        if let Err(e) = &self.style.item {
            out.push(Fault {
                severity: Severity::Error,
                error: e.clone().with("path", STYLE),
            });
        }
        out.extend(self.style.faults.iter().cloned());
        if let Some(o) = &self.org_style {
            if let Err(e) = &o.style.item {
                let e = if e.details.get("path").is_some() {
                    e.clone()
                } else {
                    e.clone().with("path", &o.path)
                };
                out.push(Fault {
                    severity: Severity::Error,
                    error: e,
                });
            }
            out.extend(o.style.faults.iter().cloned());
        }
        let layers = crate::cascade::Layers::of(self);
        for (lock, ptr) in layers.broken_locks() {
            out.push(
                Fault::error(
                    Code::StyleLocked,
                    format!("the organisation's style locks {lock}; this project's style sets it"),
                )
                .with("path", STYLE)
                .with("pointer", ptr)
                .with("lock", &lock)
                .with("organisation", layers.org_path().unwrap_or_default()),
            );
        }
        out.extend(self.conn.faults());
        out
    }

    /// The files a session tracks, relative to the project: the manifest,
    /// and the library folders of this project and each alias it can use.
    pub fn tracked(root: &Path) -> Vec<(String, std::fs::Metadata)> {
        let mut out = Vec::new();
        let manifest = root.join(MANIFEST);
        let Ok(meta) = std::fs::metadata(&manifest) else {
            return out;
        };
        out.push((MANIFEST.to_owned(), meta));
        for file in [ERC, LINT, PARTS, STYLE, crate::pdm::PDM_CONFIG, LOCK] {
            if let Ok(meta) = std::fs::metadata(root.join(file))
                && meta.is_file()
            {
                out.push((file.to_owned(), meta));
            }
        }
        for folder in [
            crate::conn::DESIGNS,
            crate::conn::RAILS,
            crate::conn::CLASSES,
        ] {
            out.extend(files_of(root, folder));
        }
        // PDM: properties, and each part's release records.
        out.extend(files_of(root, crate::pdm::PROPERTIES));
        for (part, meta) in listed(root, crate::pdm::RELEASES) {
            if meta.is_dir() {
                out.extend(files_of(root, &join(crate::pdm::RELEASES, &part)));
            }
        }
        for (block, meta) in listed(root, crate::conn::BLOCKS) {
            let dir = join(crate::conn::BLOCKS, &block);
            if !meta.is_dir() {
                out.push((dir, meta));
                continue;
            }
            for (name, meta) in listed(root, &dir) {
                let at = join(&dir, &name);
                if meta.is_dir() {
                    out.extend(files_of(root, &at));
                } else {
                    out.push((at, meta));
                }
            }
        }
        let mut bases = vec![String::new()];
        if let Ok(m) = std::fs::read(&manifest)
            .map_err(drop)
            .and_then(|b| Manifest::read(&b).map_err(drop))
        {
            bases.extend(resolve(root, &m).into_values().flatten());
        }
        for base in bases {
            // `.gitattributes`, which an import writes beside a library's
            // first model, so undo takes it back too.
            let attributes = join(&base, crate::kicad::import::GITATTRIBUTES);
            if let Ok(meta) = std::fs::metadata(crate::session::path_of(root, &attributes))
                && meta.is_file()
            {
                out.push((attributes, meta));
            }
            if !base.is_empty() {
                let at = join(&base, MANIFEST);
                if let Ok(meta) = std::fs::metadata(root.join(&at)) {
                    out.push((at, meta));
                }
                // An organisation's style beside it (plan 2.E), so its
                // edits reload a session.
                let at = join(&base, STYLE);
                if let Ok(meta) = std::fs::metadata(crate::session::path_of(root, &at))
                    && meta.is_file()
                {
                    out.push((at, meta));
                }
            }
            let dir = join(&base, library::DIR);
            for (name, meta) in listed(root, &dir) {
                if meta.is_dir() {
                    let sub = join(&dir, &name);
                    out.extend(
                        listed(root, &sub)
                            .into_iter()
                            .filter(|(_, m)| m.is_file())
                            .map(|(f, m)| (join(&sub, &f), m)),
                    );
                } else if meta.is_file() {
                    out.push((join(&dir, &name), meta));
                }
            }
        }
        out
    }
}

/// The files of the folder at `rel` (relative to `root`), as paths
/// relative to `root`.
fn files_of(root: &Path, rel: &str) -> Vec<(String, std::fs::Metadata)> {
    listed(root, rel)
        .into_iter()
        .filter(|(_, m)| m.is_file())
        .map(|(f, m)| (join(rel, &f), m))
        .collect()
}

/// The entries of the folder at `rel` (relative to `root`), with their
/// metadata, sorted; none when it isn't there.
fn listed(root: &Path, rel: &str) -> Vec<(String, std::fs::Metadata)> {
    let at = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    let Ok(entries) = std::fs::read_dir(at) else {
        return Vec::new();
    };
    let mut out: Vec<(String, std::fs::Metadata)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = std::fs::metadata(e.path()).ok()?;
            Some((e.file_name().to_string_lossy().into_owned(), meta))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_paper_series_is_the_sizes_up_to_the_projects() {
        let s: Sheet = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(s.fit);
        assert_eq!(s.series(), vec![PaperSize::A4, PaperSize::A3]);
        let s: Sheet =
            serde_json::from_value(serde_json::json!({"size": "a2", "sizes": ["a4", "a3"]}))
                .unwrap();
        assert_eq!(
            s.series(),
            vec![PaperSize::A4, PaperSize::A3, PaperSize::A2]
        );
        let s: Sheet =
            serde_json::from_value(serde_json::json!({"size": "a4", "sizes": ["a3", "a4"]}))
                .unwrap();
        assert_eq!(
            s.series(),
            vec![PaperSize::A4],
            "none past the project's size"
        );
        let off: Sheet = serde_json::from_value(serde_json::json!({"fit": false})).unwrap();
        assert!(off.series().is_empty());
        assert_eq!(
            serde_json::to_value(Sheet::default()).unwrap(),
            serde_json::json!({"size": "a3", "orientation": "landscape"}),
            "fit and sizes at their defaults aren't written"
        );
    }
    use crate::source::write_atomic;

    pub fn pid(n: u8) -> ProjectId {
        ProjectId::from_bytes([n; 16])
    }

    /// Writes a project's manifest in `dir`.
    pub fn manifest(dir: &Path, kind: ProjectKind, libraries: &[(&str, &str)]) {
        let mut m = Manifest::new(pid(1), Unit::Mm, kind);
        for (alias, path) in libraries {
            m.libraries
                .insert(Id::parse(alias, "alias").unwrap(), Alias::local(path));
        }
        write_atomic(&dir.join(MANIFEST), m.to_text()).unwrap();
    }

    #[test]
    fn an_alias_with_pins_or_a_url_is_format_3_and_one_without_as_before() {
        let mut m = Manifest::new(pid(1), Unit::Mm, ProjectKind::Design);
        m.libraries.insert(
            Id::parse("passives", "a").unwrap(),
            Alias::local("../passives"),
        );
        let before = m.to_text();
        assert!(before.contains("\"format\": 1"), "{before}");
        assert!(
            before.contains(
                "\"libraries\": {\n    \"passives\": {\n      \"path\": \"../passives\"\n    }"
            ),
            "{before}"
        );
        let a = m
            .libraries
            .get_mut(&Id::parse("passives", "a").unwrap())
            .unwrap();
        a.labels
            .insert(Id::parse("r_4k7", "c").unwrap(), "B".into());
        a.project = Some(pid(2));
        let text = m.to_text();
        assert!(text.contains("\"format\": 3"), "{text}");
        assert_eq!(Manifest::read(text.as_bytes()).unwrap(), m);
        // Pinned to nothing: refused.
        let mut bad = m.clone();
        bad.libraries
            .get_mut(&Id::parse("passives", "a").unwrap())
            .unwrap()
            .labels
            .insert(Id::parse("c_1u", "c").unwrap(), " ".into());
        let e = Manifest::read(bad.to_text().as_bytes()).unwrap_err();
        assert_eq!(e.details["pointer"], "/libraries/passives/labels/c_1u");
    }

    #[test]
    fn a_url_alias_names_its_project() {
        let mut m = Manifest::new(pid(1), Unit::Mm, ProjectKind::Design);
        let mut a = Alias::local("lib");
        a.url = Some("file:///libs".into());
        m.libraries.insert(Id::parse("passives", "a").unwrap(), a);
        let e = Manifest::read(m.to_text().as_bytes()).unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
        assert_eq!(e.details["pointer"], "/libraries/passives/project");
        let a = m
            .libraries
            .get_mut(&Id::parse("passives", "a").unwrap())
            .unwrap();
        a.project = Some(pid(2));
        a.path = "../up".into();
        let e = Manifest::read(m.to_text().as_bytes()).unwrap_err();
        assert_eq!(e.details["pointer"], "/libraries/passives/path");
    }

    #[test]
    fn a_url_alias_has_no_working_library_and_is_no_fault() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        let mut m = Manifest::read(&std::fs::read(board.join(MANIFEST)).unwrap()).unwrap();
        let mut a = Alias::local("");
        a.url = Some("file:///libs".into());
        a.project = Some(pid(2));
        m.libraries.insert(Id::parse("passives", "a").unwrap(), a);
        write_atomic(&board.join(MANIFEST), m.to_text()).unwrap();
        let p = Project::load(&board).unwrap();
        assert_eq!(
            p.library("passives").unwrap_err().code,
            Code::LibraryForeignWorking
        );
        assert_eq!(p.faults(), []);
    }

    #[test]
    fn an_alias_naming_a_project_must_find_it() {
        let dir = tempfile::tempdir().unwrap();
        let (board, lib) = (dir.path().join("board"), dir.path().join("lib"));
        manifest(&lib, ProjectKind::Library, &[]);
        manifest(&board, ProjectKind::Design, &[("passives", "../lib")]);
        let mut m = Manifest::read(&std::fs::read(board.join(MANIFEST)).unwrap()).unwrap();
        let found = resolve(&board, &m);
        assert!(found[&Id::parse("passives", "a").unwrap()].is_ok());
        m.libraries
            .get_mut(&Id::parse("passives", "a").unwrap())
            .unwrap()
            .project = Some(pid(2));
        let e = resolve(&board, &m)
            .remove(&Id::parse("passives", "a").unwrap())
            .unwrap()
            .unwrap_err();
        assert_eq!(e.code, Code::DependencyProjectMismatch);
    }

    /// The test library's files written under `dir`.
    pub fn library_in(dir: &Path) {
        use crate::library::tests::{STEP, files};
        for (p, b) in files(STEP) {
            write_atomic(&dir.join(p), b).unwrap();
        }
    }

    #[test]
    fn a_fork_names_where_it_came_from_at_format_3() {
        let mut m = Manifest::new(pid(7), Unit::Mm, ProjectKind::Design);
        assert!(m.to_text().contains("\"format\": 1"));
        m.forked_from = Some(ForkedFrom {
            project: pid(6),
            commit: "a".repeat(40),
        });
        let text = m.to_text();
        assert!(text.contains("\"format\": 3"), "{text}");
        assert!(
            text.contains(&format!(
                "\"forked_from\": {{\n    \"commit\": \"{}\",\n    \"project\": \"{}\"",
                "a".repeat(40),
                "06".repeat(16)
            )),
            "{text}"
        );
        assert_eq!(Manifest::read(text.as_bytes()).unwrap(), m);
        // An older ryueda refuses it as too new rather than misreading it.
        let e = crate::doc::read::<serde_json::Value>(text.as_bytes(), MANIFEST, KIND, FORMAT_2)
            .unwrap_err();
        assert_eq!(e.code, crate::error::Code::FormatTooNew);
    }

    #[test]
    fn manifests_read_and_write_canonically() {
        let m = Manifest::new(pid(7), Unit::Mm, ProjectKind::Design);
        let text = m.to_text();
        assert_eq!(
            text,
            format!(
                "{{\n  \"default_unit\": \"mm\",\n  \"format\": 1,\n  \"kind\": \"ryueda.project\",\n  \"project\": \"{}\"\n}}\n",
                "07".repeat(16)
            )
        );
        assert_eq!(Manifest::read(text.as_bytes()).unwrap(), m);
        let lib = Manifest::new(pid(7), Unit::In, ProjectKind::Library).to_text();
        assert!(lib.contains("\"project_kind\": \"library\""));
        // Properties or a sheet other than the default are format 2.
        let mut two = m.clone();
        two.properties.insert("company".into(), "Seiraiyu".into());
        two.schematic.sheet.size = PaperSize::A4;
        let text2 = two.to_text();
        assert!(text2.contains("\"format\": 2") && text2.contains("\"size\": \"a4\""));
        assert_eq!(Manifest::read(text2.as_bytes()).unwrap(), two);
        assert_eq!(two.schematic.sheet.size_nm(), (297_000_000, 210_000_000));
        let mut bad_prop = two.clone();
        bad_prop.properties.insert("refdes".into(), "x".into());
        assert_eq!(
            Manifest::read(bad_prop.to_text().as_bytes())
                .unwrap_err()
                .code,
            Code::InvalidDocument
        );
        let bad = text.replace("\"project\": \"07", "\"project\": \"zz");
        assert_eq!(
            Manifest::read(bad.as_bytes()).unwrap_err().code,
            Code::InvalidDocument
        );
    }

    #[test]
    fn alias_paths_are_relative_folder_names() {
        for ok in ["../passives", "libs/passives", "../../a/b"] {
            assert!(path_ok(ok), "{ok}");
        }
        for bad in ["", "/abs", "a//b", "./a", "a/", "C:/x", "..\\x", "a/./b"] {
            assert!(!path_ok(bad), "{bad}");
        }
    }

    /// A repository holding a design project `board`, a library project
    /// `passives` beside it, and what else a test adds.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        manifest(&dir.path().join("passives"), ProjectKind::Library, &[]);
        library_in(&dir.path().join("passives"));
        dir
    }

    #[test]
    fn aliases_reach_library_projects_beside_the_project() {
        let dir = repo();
        let board = dir.path().join("board");
        let outside = tempfile::tempdir().unwrap();
        manifest(&outside.path().join("lib"), ProjectKind::Library, &[]);
        manifest(&dir.path().join("design"), ProjectKind::Design, &[]);
        manifest(
            &dir.path().join("nested"),
            ProjectKind::Library,
            &[("x", "../passives")],
        );
        let far = format!("../{}", "../".repeat(board.components().count() + 2));
        let far = format!(
            "{far}{}",
            outside
                .path()
                .join("lib")
                .to_string_lossy()
                .trim_start_matches('/')
        );
        manifest(
            &board,
            ProjectKind::Design,
            &[
                ("passives", "../passives"),
                ("none", "../none"),
                ("design", "../design"),
                ("nested", "../nested"),
                ("bad", "/abs"),
                ("far", &far),
            ],
        );
        let p = Project::load(&board).unwrap();
        let code = |a: &str| p.library(a).map(|_| ()).map_err(|e| e.code);
        assert_eq!(code("passives"), Ok(()));
        assert_eq!(code("self"), Ok(()));
        for a in ["none", "design", "bad", "far"] {
            assert_eq!(code(a), Err(Code::LibraryMissing), "{a}");
        }
        assert_eq!(code("nested"), Err(Code::LibraryNested));
        assert_eq!(code("other"), Err(Code::LibraryMissing));
        let lib = p.library("passives").unwrap();
        assert_eq!(lib.base, "../passives");
        assert!(lib.components.values().all(Loaded::is_sound));
        // Every fault, an alias's included, comes out once.
        assert_eq!(p.faults().len(), 5);
        let tracked: Vec<String> = Project::tracked(&board)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert!(
            tracked.contains(&"../passives/library/symbols/r.json".to_owned()),
            "{tracked:?}"
        );
        assert!(tracked.contains(&"../passives/ryueda.json".to_owned()));
        assert_eq!(tracked[0], MANIFEST);
    }

    #[test]
    fn components_resolve_across_libraries_one_step_only() {
        let dir = repo();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[("passives", "../passives")]);
        // The board's own component, on the alias's symbol and footprint.
        let c = r#"{"format": 1, "kind": "ryueda.libcomp", "prefix": "R", "symbol": {"library": "passives", "id": "r"},
            "footprints": [{"footprint": {"library": "passives", "id": "r_0402_1005metric"}}, {"footprint": {"id": "nothing"}}]}"#;
        write_atomic(&board.join("library/components/mine.json"), c).unwrap();
        // The library project's component naming a library of its own.
        let n = r#"{"format": 1, "kind": "ryueda.libcomp", "prefix": "R", "symbol": {"library": "other", "id": "r"}}"#;
        write_atomic(
            &dir.path().join("passives/library/components/nested.json"),
            n,
        )
        .unwrap();
        let p = Project::load(&board).unwrap();
        let mine = &p.library("self").unwrap().components[&Id::parse("mine", "c").unwrap()];
        let codes: Vec<(Code, &str)> = mine
            .faults
            .iter()
            .map(|f| (f.error.code, f.error.details["pointer"].as_str().unwrap()))
            .collect();
        assert_eq!(codes, [(Code::NotFound, "/footprints/1/footprint")]);
        assert_eq!(
            mine.faults[0].error.details["path"],
            "library/components/mine.json"
        );
        let nested = &p.library("passives").unwrap().components[&Id::parse("nested", "c").unwrap()];
        assert_eq!(nested.faults[0].error.code, Code::LibraryNested);
        assert_eq!(
            nested.faults[0].error.details["path"],
            "../passives/library/components/nested.json"
        );
    }

    /// `self` in a library project's file is that file's own library, as
    /// the library project reads it, never the library of a project that
    /// names it by an alias.
    #[test]
    fn self_in_an_aliased_library_is_its_own() {
        let dir = repo();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[("passives", "../passives")]);
        // The board's own library has a symbol `r` but no footprint, so
        // reaching it instead of the library project's would be `not_found`.
        let drawing = r#"{"format": 1, "kind": "ryueda.symbol", "name": "R"}"#;
        write_atomic(&board.join("library/symbols/r.json"), drawing).unwrap();
        let c = r#"{"format": 1, "kind": "ryueda.libcomp", "prefix": "R", "symbol": {"library": "self", "id": "r"},
            "footprints": [{"footprint": {"library": "self", "id": "r_0402_1005metric"}}]}"#;
        let path = dir.path().join("passives/library/components/explicit.json");
        write_atomic(&path, c).unwrap();
        let alone = Project::load(&dir.path().join("passives")).unwrap();
        let p = Project::load(&board).unwrap();
        let id = Id::parse("explicit", "c").unwrap();
        let faults = |p: &Project, alias: &str| {
            p.library(alias).unwrap().components[&id]
                .faults
                .iter()
                .map(|f| f.error.code)
                .collect::<Vec<_>>()
        };
        assert_eq!(faults(&alone, SELF), faults(&p, "passives"));
        assert!(
            !faults(&p, "passives").contains(&Code::NotFound),
            "{:?}",
            faults(&p, "passives")
        );
    }

    #[test]
    fn manifests_name_no_alias_self() {
        let mut m = Manifest::new(pid(7), Unit::Mm, ProjectKind::Design);
        m.libraries.insert(
            Id::parse(SELF, "alias").unwrap(),
            Alias::local("../passives"),
        );
        let e = Manifest::read(m.to_text().as_bytes()).unwrap_err();
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidDocument, &serde_json::json!("/libraries/self"))
        );
    }

    #[test]
    fn a_library_is_judged_by_its_own_standards_else_the_projects() {
        use crate::standards::Shapes;
        let dir = tempfile::tempdir().unwrap();
        let (board, passives) = (dir.path().join("board"), dir.path().join("passives"));
        manifest(&board, ProjectKind::Design, &[("passives", "../passives")]);
        manifest(&passives, ProjectKind::Library, &[]);
        let p = Project::load(&board).unwrap();
        assert_eq!(p.standards_for(SELF).unwrap().1, "built_in");
        assert_eq!(p.standards_for("passives").unwrap().1, "built_in");
        let iec = "{\"format\": 1, \"kind\": \"ryueda.standards\", \"shapes\": \"iec\"}\n";
        write_atomic(&board.join("library/standards.json"), iec).unwrap();
        let p = Project::load(&board).unwrap();
        let (s, from) = p.standards_for("passives").unwrap();
        assert_eq!((s.shapes, from.as_str()), (Shapes::Iec, SELF));
        let ansi = "{\"format\": 1, \"kind\": \"ryueda.standards\", \"shapes\": \"ansi\"}\n";
        write_atomic(&passives.join("library/standards.json"), ansi).unwrap();
        let p = Project::load(&board).unwrap();
        let (s, from) = p.standards_for("passives").unwrap();
        assert_eq!((s.shapes, from.as_str()), (Shapes::Ansi, "passives"));
        // A file that doesn't pass its checks judges nothing, and is a
        // fault of the project.
        write_atomic(
            &board.join("library/standards.json"),
            "{\"format\": 1, \"kind\": \"ryueda.standards\", \"grid_nm\": 0}\n",
        )
        .unwrap();
        let p = Project::load(&board).unwrap();
        assert_eq!(
            p.standards_for(SELF).unwrap_err().code,
            Code::StandardsInvalid
        );
        assert!(
            p.faults()
                .iter()
                .any(|f| f.error.code == Code::StandardsInvalid)
        );
    }

    /// The test library's files with a second component, `c_100n` (a copy
    /// of `r_100k`), each footprint named `name` when given.
    pub(crate) fn pinned_files(name: Option<&str>) -> BTreeMap<String, Vec<u8>> {
        let mut f = crate::library::tests::files(crate::library::tests::STEP);
        let r = f["library/components/r_100k.json"].clone();
        f.insert("library/components/c_100n.json".into(), r);
        if let Some(name) = name {
            for (p, b) in f.iter_mut() {
                if p.starts_with("library/footprints/") {
                    let mut v: serde_json::Value = serde_json::from_slice(b).unwrap();
                    v["name"] = name.into();
                    *b = crate::canonical::to_string(&v).into_bytes();
                }
            }
        }
        f
    }

    /// A pin as a resolver would read it from `files`.
    pub(crate) fn pinned_from(
        files: &BTreeMap<String, Vec<u8>>,
        c: &str,
        label: &str,
    ) -> std::sync::Arc<crate::pin::Pinned> {
        let id = |s: &str| Id::parse(s, "id").unwrap();
        let lib = crate::pin::from_files(files, &id("passives"), label, &id(c)).unwrap();
        std::sync::Arc::new(crate::pin::Pinned {
            alias: id("passives"),
            component: id(c),
            label: label.into(),
            project: pid(2),
            commit: "a".repeat(40),
            content: "c".repeat(64),
            obsolete: false,
            library: lib,
        })
    }

    /// A resolver over fixed pins of library `passives` (project 2), with
    /// the releases they come from.
    #[derive(Clone)]
    pub(crate) struct Fake {
        pub pins: BTreeMap<(String, String), std::sync::Arc<crate::pin::Pinned>>,
        pub releases: ryupdm::history::Releases,
    }

    impl Fake {
        pub(crate) fn new() -> Fake {
            Fake {
                pins: BTreeMap::new(),
                releases: ryupdm::history::Releases::default(),
            }
        }

        fn record(&mut self, c: &str, label: &str, kind: ryupdm::record::Kind) {
            let id = Id::parse(c, "id").unwrap();
            let n = self.releases.official.len();
            self.releases.official.insert(
                ryupdm::record::path(&id, label, kind),
                ryupdm::history::Official {
                    commit: format!("{:040x}", n + 1),
                    blob: gix::ObjectId::null(gix::hash::Kind::Sha1),
                    shallow: false,
                },
            );
        }

        /// `c` released at `label`, readable as a pin.
        pub(crate) fn release(&mut self, c: &str, label: &str) {
            self.record(c, label, ryupdm::record::Kind::Release);
            self.pins.insert(
                (c.into(), label.into()),
                pinned_from(&pinned_files(Some(&format!("at {label}"))), c, label),
            );
        }

        /// `c`'s `label` made obsolete.
        pub(crate) fn obsolete(&mut self, c: &str, label: &str) {
            self.record(c, label, ryupdm::record::Kind::Obsolete);
            if let Some(p) = self.pins.get_mut(&(c.into(), label.into())) {
                std::sync::Arc::make_mut(p).obsolete = true;
            }
        }
    }

    impl crate::pin::Resolve for Fake {
        fn pin(
            &self,
            _: &Path,
            alias: &Id,
            _: &Alias,
            c: &Id,
            label: &str,
            _: Option<&ryu_doc::lock::Lock>,
        ) -> Result<std::sync::Arc<crate::pin::Pinned>> {
            self.pins
                .get(&(c.to_string(), label.to_owned()))
                .cloned()
                .ok_or_else(|| Error::new(Code::LabelNotFound, format!("{alias}/{c}@{label}")))
        }

        fn releases_of(
            &self,
            _: &Path,
            _: &Alias,
        ) -> Result<(
            ryupdm::config::Config,
            std::sync::Arc<ryupdm::history::Releases>,
        )> {
            Ok((
                ryupdm::config::Config::default(),
                std::sync::Arc::new(self.releases.clone()),
            ))
        }

        fn file_at(&self, _: &Path, _: &Alias, _: &str, rel: &str) -> Result<Option<Vec<u8>>> {
            Ok(pinned_files(None).get(rel).cloned())
        }
    }

    /// The breakout (with its own library) in a design project whose
    /// manifest names library `passives` (project 2) at `../passives`,
    /// pinning `r_100k` at A and `c_100n` at B, and whose `r1` places
    /// `passives/r_100k`; and a resolver releasing `r_100k` at A and
    /// `c_100n` at A and B.
    pub(crate) fn pinned_board() -> (tempfile::TempDir, PathBuf, Fake) {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        let mut m = Manifest::read(&std::fs::read(board.join(MANIFEST)).unwrap()).unwrap();
        let mut a = Alias::local("../passives");
        a.project = Some(pid(2));
        a.labels
            .insert(Id::parse("r_100k", "c").unwrap(), "A".into());
        a.labels
            .insert(Id::parse("c_100n", "c").unwrap(), "B".into());
        m.libraries.insert(Id::parse("passives", "a").unwrap(), a);
        write_atomic(&board.join(MANIFEST), m.to_text()).unwrap();
        let r1 = board.join("blocks/top/components/r1.json");
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&r1).unwrap()).unwrap();
        v["of"] = serde_json::json!({"library": "passives", "id": "r_100k"});
        write_atomic(&r1, crate::canonical::to_string(&v)).unwrap();
        let mut fake = Fake::new();
        fake.release("r_100k", "A");
        fake.release("c_100n", "A");
        fake.release("c_100n", "B");
        (dir, board, fake)
    }

    #[test]
    fn pinned_components_read_their_own_objects() {
        let mut fake = Fake::new();
        fake.release("r_100k", "A");
        fake.release("c_100n", "B");
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        let mut m = Manifest::read(&std::fs::read(board.join(MANIFEST)).unwrap()).unwrap();
        let mut a = Alias::local("../passives");
        a.project = Some(pid(2));
        a.labels
            .insert(Id::parse("r_100k", "c").unwrap(), "A".into());
        a.labels
            .insert(Id::parse("c_100n", "c").unwrap(), "B".into());
        m.libraries.insert(Id::parse("passives", "a").unwrap(), a);
        write_atomic(&board.join(MANIFEST), m.to_text()).unwrap();
        let aliases = resolve(&board, &m);
        let p =
            Project::load_from(&board, &crate::source::Folder(&board), &aliases, &fake).unwrap();
        let of = |c: &str| LibRef {
            library: Some(Id::parse("passives", "a").unwrap()),
            id: Id::parse(c, "c").unwrap(),
        };
        // Each sees its own footprint, though they share its ID.
        for (c, name) in [("r_100k", "at A"), ("c_100n", "at B")] {
            let lc = p.libcomp(SELF, &of(c)).unwrap();
            let fp = p
                .placed_footprint(&of(c), &lc.footprints[0].footprint)
                .unwrap();
            assert_eq!(fp.name, name);
            assert!(p.placed_symbol(&of(c), &lc.symbol).is_ok());
            assert_eq!(
                p.home(&of(c)).unwrap().base,
                format!("@passives/{}", &name[3..])
            );
        }
        // ../passives isn't there: the working library is missing, the
        // pins read regardless, and a pin that doesn't resolve says why.
        assert!(p.library("passives").is_err());
        let e = p.home(&of("c_1u")).unwrap_err();
        assert_eq!(e.code, Code::LibraryMissing);
        let mut m2 = m.clone();
        m2.libraries
            .get_mut(&Id::parse("passives", "a").unwrap())
            .unwrap()
            .labels
            .insert(Id::parse("c_1u", "c").unwrap(), "Z".into());
        write_atomic(&board.join(MANIFEST), m2.to_text()).unwrap();
        let p =
            Project::load_from(&board, &crate::source::Folder(&board), &aliases, &fake).unwrap();
        assert_eq!(p.home(&of("c_1u")).unwrap_err().code, Code::LabelNotFound);
    }

    /// Writes the test design's files (`design::tests::files`) in `dir`.
    pub fn design_in(dir: &Path) {
        for (p, b) in crate::design::tests::files() {
            write_atomic(&dir.join(p), b).unwrap();
        }
    }

    #[test]
    fn a_project_reads_its_connectivity_against_its_library() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        let p = Project::load(&board).unwrap();
        assert_eq!(p.faults(), []);
        assert_eq!(p.conn.blocks.len(), 2);
        let tracked: Vec<String> = Project::tracked(&board)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        for f in [
            "blocks/top/block.json",
            "blocks/top/components/r1.json",
            "blocks/top/instances/ch_a.json",
            "designs/d.json",
            "rails/gnd.json",
            "classes/power.json",
        ] {
            assert!(tracked.contains(&f.to_owned()), "{f}: {tracked:?}");
        }
        // A component's library component is this project's library's.
        let c =
            br#"{"format": 1, "kind": "ryueda.component", "of": {"id": "r_1k"}, "refdes": "R9"}"#;
        write_atomic(&board.join("blocks/top/components/r9.json"), c).unwrap();
        let q = Project::load(&board).unwrap();
        let faults = q.faults();
        let codes: Vec<(Code, &str)> = faults
            .iter()
            .map(|f| (f.error.code, f.error.details["path"].as_str().unwrap()))
            .collect();
        assert_eq!(codes, [(Code::NotFound, "blocks/top/components/r9.json")]);
        // The project's revision is its manifest's, erc.json's and the
        // top-level files'; a block's files are the block's.
        assert_eq!(q.revision, p.revision);
        assert_ne!(
            q.conn.blocks[&Id::parse("top", "b").unwrap()].revision,
            p.conn.blocks[&Id::parse("top", "b").unwrap()].revision
        );
        write_atomic(&board.join(ERC), "{}").unwrap();
        assert_ne!(Project::load(&board).unwrap().revision, p.revision);
    }

    #[test]
    fn a_library_project_holds_no_design() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        manifest(&lib, ProjectKind::Library, &[]);
        library_in(&lib);
        design_in(&lib);
        let p = Project::load(&lib).unwrap();
        let faults = p.faults();
        let paths: Vec<&str> = faults
            .iter()
            .filter(|f| f.error.code == Code::InvalidDocument)
            .map(|f| f.error.details["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["designs", "blocks", "rails"]);
    }

    #[test]
    fn roots_are_found_above() {
        let dir = repo();
        let lib = dir.path().join("passives");
        assert_eq!(
            find_root(&lib.join("library/symbols")).unwrap(),
            plain(lib.canonicalize().unwrap())
        );
        assert_eq!(
            find_root(dir.path()).unwrap_err().code,
            Code::ProjectNotFound
        );
    }

    #[test]
    fn a_project_reads_its_style_and_its_organisations() {
        let d = tempfile::tempdir().unwrap();
        let org = d.path().join("org");
        std::fs::create_dir_all(&org).unwrap();
        tests::manifest(&org, ProjectKind::Library, &[]);
        std::fs::write(
            org.join(crate::style::FILE),
            r##"{"kind": "ryueda.style", "format": 1, "palette": {"brand": {"light": "#0055aa", "dark": "#66aaff"}}, "locks": ["colour"]}"##,
        )
        .unwrap();
        let me = d.path().join("me");
        std::fs::create_dir_all(&me).unwrap();
        tests::manifest(&me, ProjectKind::Design, &[("org", "../org")]);
        let mut m = Manifest::read(&std::fs::read(me.join(MANIFEST)).unwrap()).unwrap();
        m.schematic.style_from = Some(Id::parse("org", "alias").unwrap());
        std::fs::write(me.join(MANIFEST), m.to_text()).unwrap();
        std::fs::write(
            me.join(crate::style::FILE),
            r##"{"kind": "ryueda.style", "format": 1, "rules": [{"select": {"tag": "hv"}, "set": {"fill": "brand"}}]}"##,
        )
        .unwrap();
        let p = Project::load(&me).unwrap();
        let o = p.org_style.as_ref().expect("an organisation style");
        assert_eq!(o.alias, "org");
        assert_eq!(o.path, "../org/style.json");
        assert!(
            o.style
                .item
                .as_ref()
                .unwrap()
                .locks
                .contains(&"colour".to_owned())
        );
        assert_eq!(p.style.item.as_ref().unwrap().rules.len(), 1);
        assert!(p.faults().iter().all(|f| !f.is_error()), "{:?}", p.faults());
        // The project's style is in its revision.
        let before = p.revision.clone();
        std::fs::remove_file(me.join(crate::style::FILE)).unwrap();
        assert_ne!(Project::load(&me).unwrap().revision, before);
        // A style_from naming no alias is a fault.
        m.schematic.style_from = Some(Id::parse("nope", "alias").unwrap());
        std::fs::write(me.join(MANIFEST), m.to_text()).unwrap();
        assert!(
            Project::load(&me)
                .unwrap()
                .faults()
                .iter()
                .any(|f| f.error.code == Code::StyleInvalid)
        );
    }
}
