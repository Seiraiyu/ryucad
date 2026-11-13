//! Export: a part or an assembly, at the working files, a Git revision or
//! a release label, as a file of one of the [`Format`]s. With `out` the
//! daemon writes the file itself, atomically, streaming it, so no file
//! crosses the socket; without, its bytes follow the answer. Mesh formats
//! take ryukei's meshes at the export's chord and angle, each body meshed
//! once whatever the occurrences, through the session's mesh cache.
//!
//! A part is one product of its solids as shown, in model order (consumed
//! solids aren't); an assembly is a product placing each instance's
//! product at its solved pose, and each instance pattern's copies at
//! theirs, subassemblies products of their own; a part placed at two
//! revisions is two products. A product's identifier is its PDM number,
//! else the one product its import features (or, for an assembly, its
//! imported instances) all name, else its part ID; its name the `name`
//! property, else that product's name, else its ID; its description the
//! `description` property; its revision the release label it is exported
//! at. An imported assembly's instance of its own bodies (an import with
//! no usage, at the origin) writes them as the assembly's own bodies, and
//! a usage's identifier is the file's where an import made the instance,
//! else the instance's ID: a file exported as it was imported comes back
//! with its products and usages, so the import matches the export.
//!
//! The bytes are the same for the same model and options: the header's
//! file name and time stamp are the caller's, the time stamp else the
//! commit's time when exporting a commit (a Git revision, or a release
//! where it became official), else now (UTC, seconds). Exports are built
//! from the session's results each time and never cached.

use crate::assembly::{Model, Pin};
use crate::doc::assembly::{Pose, Rev};
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::import::ProductRef;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::proto::Answer;
use crate::rebuild::{Built, Output, PartResult};
use crate::session::Session;
use crate::units::{self, Field, Quantity};
use ryukei::geom::Rigid;
use ryukei::mesh::MeshParams;
use ryukei::topo::Body;
use ryupdm::props::Properties;
use ryustep::schema::Schema;
use ryustep::{LengthUnit, WriteError, WriteModel, WriteOptions};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What an export writes, whatever the format: the product exported
/// first, then every product it places, in the order a depth-first walk
/// meets them, each once.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub products: Vec<Product>,
}

/// A part at a revision, as an export describes it: its identifier, name,
/// description and revision (the release label it is read at, else
/// empty), its own solids in model order, and what it places.
#[derive(Clone, Debug)]
pub struct Product {
    pub part: Id,
    pub rev: Rev,
    pub id: String,
    pub name: String,
    pub description: String,
    pub revision: String,
    pub solids: Vec<Solid>,
    pub children: Vec<Child>,
}

/// A solid a product holds: the part that shows it (an imported
/// assembly's own bodies are its body part's), its founder and its body.
#[derive(Clone, Debug)]
pub struct Solid {
    pub part: Id,
    pub founder: Id,
    /// Its key in its product's part's looks ([`crate::appearance::looks`]):
    /// its founder, or for an assembly's own bodies `<instance>/<founder>`.
    pub key: String,
    pub built: Arc<Built>,
}

/// One placing of a product in another: by a rigid motion of its frame
/// into its parent's, in meters. `usage` is the identifier STEP writes
/// (the file's where an import made the instance, else the instance's
/// ID); `instance` is the step it adds to an occurrence's path, the
/// instance's ID or a copy's `<pattern>:<i>`.
#[derive(Clone, Debug)]
pub struct Child {
    pub product: usize,
    pub place: Rigid,
    pub usage: String,
    pub instance: String,
    /// The steps it adds to an occurrence's path: the instance's ID, or a
    /// pattern copy's `<pattern>:<i>` and its seed's.
    pub steps: Vec<Id>,
}

/// What `export` asks for: the part, the revision (`working`, a Git
/// revision, or `label:<L>`), the format (else `out`'s extension, else
/// STEP), the absolute path to write, `ap242` or `ap214`, the unit, the
/// header's time stamp and file name, a mesh format's chord, angle and
/// longest edge, STL's `ascii`, and USD's `brep` and `mesh` (both by
/// default) and the tolerance pipes and sweeps are written within, and
/// `skip_failing`: write what built, leaving out what a failing feature
/// would have made, rather than refuse; and STEP's `appearance` (styles,
/// written unless `false`).
#[derive(Clone, Debug, Default)]
pub struct Ask {
    pub part: String,
    pub rev: Option<String>,
    pub format: Option<String>,
    pub out: Option<String>,
    pub schema: Option<String>,
    pub unit: Option<String>,
    pub time_stamp: Option<String>,
    pub file_name: Option<String>,
    pub chord: Option<String>,
    pub angle: Option<String>,
    pub max_edge: Option<String>,
    pub ascii: bool,
    pub brep: Option<bool>,
    pub mesh: Option<bool>,
    pub pipe_tolerance: Option<String>,
    pub skip_failing: bool,
    /// STEP: write the appearance as styles (true by default; design 448).
    pub appearance: Option<bool>,
    /// STEP: write materials and properties (true by default; design 448,
    /// plan 24.C).
    pub properties: Option<bool>,
    /// DXF: the sheet metal solid (the part's one unless named), whether
    /// to write bend notes (true), the side face held still and allowance
    /// overrides, as `flat.get` takes them.
    pub solid: Option<String>,
    pub notes: Option<bool>,
    pub fixed: Option<String>,
    pub bends: Vec<Value>,
}

/// The formats `export` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Step,
    Stl,
    ThreeMf,
    Usda,
    Usdc,
    Usdz,
    Dxf,
}

impl Format {
    pub const ALL: [Format; 7] = [
        Format::Step,
        Format::Stl,
        Format::ThreeMf,
        Format::Usda,
        Format::Usdc,
        Format::Usdz,
        Format::Dxf,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Step => "step",
            Format::Stl => "stl",
            Format::ThreeMf => "3mf",
            Format::Usda => "usda",
            Format::Usdc => "usdc",
            Format::Usdz => "usdz",
            Format::Dxf => "dxf",
        }
    }

    /// Whether it is one of USD's encodings, which share their options.
    pub fn is_usd(self) -> bool {
        matches!(self, Format::Usda | Format::Usdc | Format::Usdz)
    }

    pub fn parse(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.as_str() == s)
    }

    /// The format a file name's extension names, in any case: `.step` or
    /// `.stp`, `.stl`, `.3mf`, `.usda`, `.usdc` or `.usd` (USD's binary
    /// encoding, as OpenUSD writes `.usd` by default), `.usdz`, `.dxf`.
    pub fn of_path(path: &str) -> Option<Format> {
        let ext = std::path::Path::new(path).extension()?.to_str()?;
        match ext.to_ascii_lowercase().as_str() {
            "step" | "stp" => Some(Format::Step),
            "stl" => Some(Format::Stl),
            "3mf" => Some(Format::ThreeMf),
            "usda" => Some(Format::Usda),
            "usdc" | "usd" => Some(Format::Usdc),
            "usdz" => Some(Format::Usdz),
            "dxf" => Some(Format::Dxf),
            _ => None,
        }
    }
}

/// What a format is written with, read from the ask and checked before
/// anything is built: an option another format takes is `invalid_param`.
enum Options {
    Step {
        schema: Schema,
        schema_name: &'static str,
        unit: LengthUnit,
        unit_name: &'static str,
    },
    Stl {
        unit: ryuformats::Unit,
        mesh: MeshOptions,
        ascii: bool,
    },
    ThreeMf {
        unit: ryuformats::Unit,
        mesh: MeshOptions,
    },
    Usd {
        unit: ryuformats::Unit,
        brep: bool,
        /// None without meshes.
        mesh: Option<MeshOptions>,
        /// None without B-rep.
        pipe_tolerance: Option<Quantity>,
    },
    Dxf(ryuformats::dxf::DxfOptions),
}

/// The tolerance pipes and sweeps are written within by default, as
/// STEP's writer does.
pub const DEFAULT_PIPE_TOLERANCE: &str = "0.0001 mm";

impl Options {
    /// `format`'s options from `ask`, a bare length in `default` units.
    fn read(format: Format, ask: &Ask, default: units::Unit) -> Result<Options> {
        let not_for = |param: &str, given: bool| -> Result<()> {
            if !given {
                return Ok(());
            }
            Err(Error::new(
                Code::InvalidParam,
                format!("{param} doesn't apply to {}", format.as_str()),
            )
            .with("param", param))
        };
        let unit = ask.unit.as_deref().unwrap_or("mm");
        if !format.is_usd() {
            not_for("brep", ask.brep.is_some())?;
            not_for("mesh", ask.mesh.is_some())?;
            not_for("pipe_tolerance", ask.pipe_tolerance.is_some())?;
        }
        if format != Format::Dxf {
            not_for("solid", ask.solid.is_some())?;
            not_for("notes", ask.notes.is_some())?;
            not_for("fixed", ask.fixed.is_some())?;
            not_for("bends", !ask.bends.is_empty())?;
        }
        match format {
            Format::Dxf => {
                not_for("schema", ask.schema.is_some())?;
                not_for("ascii", ask.ascii)?;
                not_for("chord", ask.chord.is_some())?;
                not_for("angle", ask.angle.is_some())?;
                not_for("max_edge", ask.max_edge.is_some())?;
                not_for("skip_failing", ask.skip_failing)?;
                let unit = ryuformats::dxf::DxfUnit::parse(unit)
                    .ok_or_else(|| invalid("unit", unit, "mm or in"))?;
                Ok(Options::Dxf(ryuformats::dxf::DxfOptions {
                    unit,
                    notes: ask.notes.unwrap_or(true),
                }))
            }
            Format::Usda | Format::Usdc | Format::Usdz => {
                not_for("schema", ask.schema.is_some())?;
                not_for("ascii", ask.ascii)?;
                let unit = ryuformats::Unit::parse(unit)
                    .ok_or_else(|| invalid("unit", unit, "mm, cm or m"))?;
                let (brep, mesh) = (ask.brep.unwrap_or(true), ask.mesh.unwrap_or(true));
                if !brep && !mesh {
                    return Err(Error::new(
                        Code::InvalidParam,
                        "a USD export writes B-rep, meshes or both: not neither",
                    )
                    .with("param", "mesh"));
                }
                let without = |param: &str, given: bool, what: &str| -> Result<()> {
                    if !given {
                        return Ok(());
                    }
                    Err(Error::new(
                        Code::InvalidParam,
                        format!("{param} doesn't apply to a USD export without {what}"),
                    )
                    .with("param", param))
                };
                if !mesh {
                    without("chord", ask.chord.is_some(), "meshes")?;
                    without("angle", ask.angle.is_some(), "meshes")?;
                    without("max_edge", ask.max_edge.is_some(), "meshes")?;
                }
                if !brep {
                    without("pipe_tolerance", ask.pipe_tolerance.is_some(), "B-rep")?;
                }
                let mesh = mesh
                    .then(|| {
                        MeshOptions::read(
                            ask.chord.as_deref(),
                            ask.angle.as_deref(),
                            ask.max_edge.as_deref(),
                            default,
                        )
                    })
                    .transpose()?;
                let pipe_tolerance = brep
                    .then(|| {
                        let text = ask
                            .pipe_tolerance
                            .as_deref()
                            .unwrap_or(DEFAULT_PIPE_TOLERANCE);
                        let q = literal("pipe_tolerance", text, Field::Length(default))?;
                        if q.meters() < ryukei::tol::LINEAR_RESOLUTION {
                            return Err(Error::new(
                                Code::InvalidParam,
                                format!(
                                    "pipe_tolerance {} is under ryukei's resolution, {} m",
                                    q.text(),
                                    ryukei::tol::LINEAR_RESOLUTION
                                ),
                            )
                            .with("param", "pipe_tolerance"));
                        }
                        Ok(q)
                    })
                    .transpose()?;
                Ok(Options::Usd {
                    unit,
                    brep,
                    mesh,
                    pipe_tolerance,
                })
            }
            Format::Step => {
                not_for("chord", ask.chord.is_some())?;
                not_for("angle", ask.angle.is_some())?;
                not_for("max_edge", ask.max_edge.is_some())?;
                not_for("ascii", ask.ascii)?;
                let (schema, schema_name) = match ask.schema.as_deref().unwrap_or("ap242") {
                    "ap242" => (Schema::Ap242, "ap242"),
                    "ap214" => (Schema::Ap214, "ap214"),
                    other => return Err(invalid("schema", other, "ap242 or ap214")),
                };
                let (unit, unit_name) = match unit {
                    "mm" => (LengthUnit::Millimeter, "mm"),
                    "m" => (LengthUnit::Meter, "m"),
                    other => return Err(invalid("unit", other, "mm or m")),
                };
                Ok(Options::Step {
                    schema,
                    schema_name,
                    unit,
                    unit_name,
                })
            }
            Format::Stl | Format::ThreeMf => {
                not_for("schema", ask.schema.is_some())?;
                if format == Format::ThreeMf {
                    not_for("ascii", ask.ascii)?;
                }
                let unit = ryuformats::Unit::parse(unit)
                    .ok_or_else(|| invalid("unit", unit, "mm, cm or m"))?;
                let mesh = MeshOptions::read(
                    ask.chord.as_deref(),
                    ask.angle.as_deref(),
                    ask.max_edge.as_deref(),
                    default,
                )?;
                Ok(match format {
                    Format::Stl => Options::Stl {
                        unit,
                        mesh,
                        ascii: ask.ascii,
                    },
                    _ => Options::ThreeMf { unit, mesh },
                })
            }
        }
    }
}

/// The format asked for: `format`, else the one `out`'s extension names,
/// else STEP.
fn format(ask: &Ask) -> Result<Format> {
    let names = || {
        Format::ALL
            .iter()
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match (ask.format.as_deref(), ask.out.as_deref()) {
        (Some(f), _) => Format::parse(f).ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                format!("format {f:?} isn't one export writes: expected {}", names()),
            )
            .with("param", "format")
        }),
        (None, Some(out)) => Format::of_path(out).ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                format!(
                    "{out} has no extension export knows: give the format ({})",
                    names()
                ),
            )
            .with("param", "format")
        }),
        (None, None) => Ok(Format::Step),
    }
}

/// The file `out` names, checked: absolute, in a folder that exists, and
/// none of the project's own files under `root` (the root files, anything
/// under `parts/`, `releases/`, `imports/` or `annotations/`, or any path
/// through a dot folder such as `.git`). An export is never a project
/// write: it is neither logged nor undoable, and a person in View mode may
/// ask for one, so it may never replace a project's file. Elsewhere in the
/// project folder (`<project>/p.step`) it may write.
pub(crate) fn out_path(out: &str, root: &std::path::Path) -> Result<std::path::PathBuf> {
    let path = std::path::PathBuf::from(out);
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("out {out:?} isn't an absolute path to a file: the daemon's folder isn't the caller's"),
        )
        .with("param", "out"));
    }
    let dir = path.parent().expect("an absolute file path has a parent");
    if !dir.is_dir() {
        return Err(Error::new(
            Code::NotFound,
            format!("{} isn't a folder to write {out} in", dir.display()),
        )
        .with("path", dir));
    }
    let file = path.file_name().expect("checked above");
    let root_c = root.canonicalize().ok();
    // Judged as spelled (the folder resolved, the name as given) ...
    let spelled = match (dir.canonicalize(), &root_c) {
        (Ok(d), Some(r)) => d.strip_prefix(r).ok().map(|p| p.join(file)),
        _ => dir.strip_prefix(root).ok().map(|p| p.join(file)),
    };
    // ... and, when `out` already names a file, as the file system
    // resolves it (#581): an 8.3 short name (`RYUCAD~1.JSO`) or a Unicode
    // case fold (`ryucad.jſon` on APFS) finds `ryucad.json`, and the
    // atomic rename would replace that entry. A new name aliases nothing.
    let resolved = match (path.canonicalize(), &root_c) {
        (Ok(p), Some(r)) => p.strip_prefix(r).ok().map(std::path::Path::to_path_buf),
        _ => None,
    };
    // Folded as the file systems that would write it there fold: ASCII
    // case (Windows and macOS find `RYUCAD.JSON` and `Parts/p` the same),
    // trailing dots and spaces (Windows drops them from a name), and an
    // NTFS stream (`ryucad.json::$DATA` is `ryucad.json`: no project file
    // has a colon in its name). Judged so on every platform, so one rule
    // holds wherever the daemon runs.
    let fold = |p: std::path::PathBuf| -> Vec<String> {
        p.components()
            .map(|c| {
                let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
                match s.split_once(':') {
                    Some((name, _)) => name.to_owned(),
                    None => s,
                }
            })
            .collect()
    };
    let refused = |parts: &[String]| {
        let folded = parts
            .iter()
            .map(|s| s.trim_end_matches(['.', ' ']))
            .collect::<Vec<_>>()
            .join("/");
        crate::session::tracked(&folded) || parts.iter().any(|s| s.starts_with('.'))
    };
    if let Some(rel) = [spelled, resolved]
        .into_iter()
        .flatten()
        .map(fold)
        .find(|r| refused(r))
        .map(|r| r.join("/"))
    {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "out \"{out}\" is the project's own {rel}: an export never writes over a project's files (write it elsewhere)"
            ),
        )
        .with("param", "out")
        .with("path", &path));
    }
    Ok(path)
}

/// The answer to an export of `encoding` (a format's name): `write`'s
/// bytes written to `out` atomically, with their count and BLAKE3, or
/// with no `out` following the answer.
///
/// A cancel stops an export up to here (#487); once it begins writing
/// `out` it is a write, as an import's is (#268): it finishes, the file
/// in place, and a cancel that came too late answers `ok` with
/// `cancel_ignored`. So a cancelled export never leaves its file.
pub(crate) fn deliver(
    job: &ryu_session::job::Job,
    mut result: Value,
    out: Option<&std::path::Path>,
    encoding: &str,
    write: impl FnOnce(&mut dyn std::io::Write) -> std::io::Result<()>,
) -> Result<Answer> {
    let Some(out) = out else {
        let mut bytes = Vec::new();
        write(&mut bytes)
            .map_err(|e| Error::new(Code::Internal, format!("writing {encoding}: {e}")))?;
        return Ok(Answer::bytes(result, encoding, bytes));
    };
    job.check()?;
    job.writing();
    let (bytes, blake3) = crate::doc::source::write_atomic_with(out, |f| {
        let mut w = Tally {
            inner: std::io::BufWriter::new(f),
            hasher: blake3::Hasher::new(),
            bytes: 0,
        };
        write(&mut w)?;
        std::io::Write::flush(&mut w.inner)?;
        Ok((w.bytes, w.hasher.finalize().to_hex().to_string()))
    })?;
    // A boundary once written: it counts, and a cancel no longer stops
    // the export, which answers with `cancel_ignored` when one came.
    job.check()?;
    result["out"] = json!(out);
    result["bytes"] = json!(bytes);
    result["blake3"] = json!(blake3);
    Ok(Answer::json(result))
}

/// A writer that counts and hashes what passes through it.
struct Tally<W> {
    inner: W,
    hasher: blake3::Hasher,
    bytes: u64,
}

impl<W: std::io::Write> std::io::Write for Tally<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Writes `ask.part` in the format asked for: the answer, and the file
/// written to `out` or its bytes following the answer. `export_failed`
/// when a feature of anything it places fails, `export_unrepresentable`
/// for a face STEP can't describe, `export_mesh_faults` and
/// `export_too_large` for bodies a mesh format can't mesh.
pub fn export(session: &mut Session, ask: Ask) -> Result<Answer> {
    export_stamped(session, ask, env!("CARGO_PKG_VERSION"))
}

/// [`export`], with `version` where STL's header, 3MF's `Application`
/// and USD's layer data name ryucad's version: the golden hashes pin
/// every encoding's bytes with one version, the same in a release (which
/// stamps its own) and a development build. STEP's header names
/// ryustep's version, which this doesn't change.
pub fn export_stamped(session: &mut Session, ask: Ask, version: &str) -> Result<Answer> {
    let appearance = ask.appearance;
    let properties = ask.properties;
    let format = format(&ask)?;
    let options = Options::read(format, &ask, session.project().manifest.default_unit)?;
    let out = ask
        .out
        .as_deref()
        .map(|o| out_path(o, session.root()))
        .transpose()?;
    let part = crate::part::part_id(&ask.part)?;
    if let Options::Dxf(o) = options {
        return dxf(session, &ask, part, o, out.as_deref());
    }
    let (label, git) = match ask.rev.as_deref() {
        Some(r) if r.starts_with("label:") => {
            let l = &r["label:".len()..];
            let rev = Rev::from_json(&json!({"label": l}), "/rev")?;
            (Some(rev), None)
        }
        r => (None, r),
    };
    let (commit, s) = session.at(git)?;
    let rev = label.unwrap_or(Rev::Working);
    if rev == Rev::Working {
        s.project().part(&part)?;
    }
    let mut walk = Walk {
        built: s.nodes(&part, &rev)?,
        ..Walk::default()
    };
    walk.product(s, &part, &rev)?;
    // The commit whose files give an export's appearance: a label's
    // official commit (design 448); else the files `s` already reads.
    let looks_at = match &rev {
        Rev::Label(_) => walk.pin.as_ref().and_then(|p| p.commit.clone()),
        _ => None,
    };
    let failing: Vec<Value> = walk.failing.values().cloned().collect();
    if !failing.is_empty() && !ask.skip_failing {
        return Err(Error::new(
            Code::ExportFailed,
            format!(
                "{part} can't be exported: {} feature{} of what it places fail{} \
                 (skip_failing writes the rest)",
                failing.len(),
                if failing.len() == 1 { "" } else { "s" },
                if failing.len() == 1 { "s" } else { "" },
            ),
        )
        .with("part", &part)
        .with("failing", failing));
    }
    // The commit the model is read from: the revision's, or where the
    // release became official.
    let at = commit
        .clone()
        .or_else(|| walk.pin.as_ref().and_then(|p| p.commit.clone()));
    let time_stamp = match (ask.time_stamp, &at) {
        (Some(t), _) => t,
        (None, Some(c)) => utc(crate::git::repo(s.root())?.commit_time(c)?),
        (None, None) => utc(now()),
    };
    let products: Vec<Value> = walk
        .scene
        .products
        .iter()
        .map(|p| {
            let mut v = json!({
                "product": p.id,
                "name": p.name,
                "part": p.part,
                "rev": p.rev.to_json(),
                "bodies": p.solids.len(),
                "children": p.children.len(),
            });
            if !p.revision.is_empty() {
                v["revision"] = json!(p.revision);
            }
            v
        })
        .collect();
    let mut result = json!({
        "part": part,
        "rev": ask.rev.as_deref().unwrap_or("working"),
        "format": format.as_str(),
        "products": products,
    });
    if !failing.is_empty() {
        // What a failing feature would have made isn't written: a solid
        // it founds is missing, one it acts on is written as the last
        // feature of its chain that built left it.
        result["warnings"] = failing
            .iter()
            .map(|f| {
                json!({
                    "code": "export_skipped",
                    "part": f["part"],
                    "rev": f["rev"],
                    "feature": f["feature"],
                    "message": format!(
                        "{}/{} fails ({}): what it makes is left out of the export",
                        f["part"].as_str().unwrap_or_default(),
                        f["feature"].as_str().unwrap_or_default(),
                        f["code"].as_str().unwrap_or_default(),
                    ),
                })
            })
            .collect();
        result["skipped"] = json!(failing);
    }
    if let Some(c) = at {
        result["commit"] = json!(c);
    }
    match options {
        Options::Dxf(_) => unreachable!("written by dxf"),
        Options::Step {
            schema,
            schema_name,
            unit,
            unit_name,
        } => {
            let file_name = ask.file_name.or_else(|| {
                out.as_ref()
                    .and_then(|o| o.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
            });
            let opts = WriteOptions {
                schema,
                unit,
                file_name: file_name.unwrap_or_default(),
                time_stamp,
                ..WriteOptions::default()
            };
            let (model, passed) = step_parts(&walk)?;
            let (paints, unpainted, data, unwritten) = {
                let sess = match &looks_at {
                    Some(c) => s.at(Some(c))?.1,
                    None => &mut *s,
                };
                let (paints, unpainted) = if appearance == Some(false) {
                    (ryustep::Paints::default(), None)
                } else {
                    let (p, u) = step_paints(sess, &walk);
                    (p, Some(u))
                };
                let (data, unwritten) = if properties == Some(false) {
                    (ryustep::PartData::default(), None)
                } else {
                    let (d, u) = step_data(sess, &walk);
                    (d, Some(u))
                };
                (paints, unpainted, data, unwritten)
            };
            let report = ryustep::write_with_data(&model, &passed, &paints, &data, &opts)
                .map_err(|e| walk.unwritable(e))?;
            let off = if appearance == Some(false) {
                Some(appearance_left_out(s, &walk)?)
            } else {
                None
            };
            let (transfer, omitted) = step_transfer(
                s,
                &walk,
                &report,
                &model,
                unpainted.as_ref(),
                unwritten.as_ref(),
                off,
            )?;
            result["transfer"] = transfer;
            if !omitted.is_empty() {
                warn(&mut result, omitted);
            }
            let bytes = report.bytes;
            result["time_stamp"] = json!(opts.time_stamp);
            result["schema"] = json!(schema_name);
            result["unit"] = json!(unit_name);
            result["file_name"] = json!(opts.file_name);
            deliver(s.job(), result, out.as_deref(), "step", |w| {
                w.write_all(&bytes)
            })
        }
        Options::Stl { unit, mesh, ascii } => {
            let scene = meshed(s, &walk.scene, &mesh, &mut result)?;
            triangles(&mut result, &scene);
            result["unit"] = json!(unit.as_str());
            result["mesh"] = mesh.report();
            result["ascii"] = json!(ascii);
            let opts = ryuformats::stl::Stl {
                unit,
                ascii,
                header: format!("ryucad {version} {part} {}", unit.as_str()),
            };
            deliver(s.job(), result, out.as_deref(), "stl", |w| {
                ryuformats::stl::write(&scene, &opts, w)
            })
        }
        Options::ThreeMf { unit, mesh } => {
            let scene = meshed(s, &walk.scene, &mesh, &mut result)?;
            triangles(&mut result, &scene);
            result["unit"] = json!(unit.as_str());
            result["mesh"] = mesh.report();
            result["time_stamp"] = json!(time_stamp);
            let mut metadata = vec![
                ("Title".to_owned(), scene.products[0].name.clone()),
                ("Application".to_owned(), format!("ryucad {version}")),
                ("CreationDate".to_owned(), time_stamp),
                ("ryucad:chord".to_owned(), mesh.chord.text()),
                ("ryucad:angle".to_owned(), mesh.angle.text()),
            ];
            if let Some(e) = &mesh.max_edge {
                metadata.push(("ryucad:max_edge".to_owned(), e.text()));
            }
            let opts = ryuformats::threemf::ThreeMf { unit, metadata };
            deliver(s.job(), result, out.as_deref(), "3mf", |w| {
                ryuformats::threemf::write(&scene, &opts, w)
            })
        }
        Options::Usd {
            unit,
            brep,
            mesh,
            pipe_tolerance,
        } => {
            // A reference has no BrepArray here: it is written as its mesh,
            // so an export without meshes would write nothing of it. A sheet
            // has one (#624).
            let alone: Vec<String> = walk
                .scene
                .products
                .iter()
                .flat_map(|p| &p.solids)
                .filter(|x| x.built.reference)
                .map(|x| format!("{}/{}", x.part, x.founder))
                .collect();
            if mesh.is_none()
                && let Some(r) = alone.first()
            {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "{r} is a reference import, which USD holds only as a mesh: export with meshes, or convert it to a solid"
                    ),
                )
                .with("param", "mesh")
                .with("solid", r));
            }
            let meshes = mesh
                .as_ref()
                .map(|m| meshed(s, &walk.scene, m, &mut result))
                .transpose()?;
            if brep {
                warn(
                    &mut result,
                    alone
                        .iter()
                        .map(|r| {
                            json!({"code": "export_reference_mesh_only", "solid": r,
                                   "message": format!("{r} is a reference import: written as its mesh alone, with no BrepArray")})
                        })
                        .collect(),
                );
            }
            if let Some(scene) = &meshes {
                triangles(&mut result, scene);
            }
            result["unit"] = json!(unit.as_str());
            result["brep"] = json!(brep);
            result["time_stamp"] = json!(time_stamp);
            let mut data = vec![
                ("version".to_owned(), version.to_owned()),
                ("time_stamp".to_owned(), time_stamp),
            ];
            if let Some(c) = result.get("commit").and_then(Value::as_str) {
                data.push(("commit".to_owned(), c.to_owned()));
            }
            if let Some(m) = &mesh {
                result["mesh"] = m.report();
                data.push(("chord".to_owned(), m.chord.text()));
                data.push(("angle".to_owned(), m.angle.text()));
                if let Some(e) = &m.max_edge {
                    data.push(("max_edge".to_owned(), e.text()));
                }
            }
            if let Some(q) = &pipe_tolerance {
                result["pipe_tolerance"] = q.report();
                data.push(("pipe_tolerance".to_owned(), q.text()));
            }
            let model = usd_model(&walk.scene, meshes.as_ref());
            let opts = ryuformats::usd::stage::Options {
                unit,
                brep,
                mesh: mesh.is_some(),
                pipe_tolerance: pipe_tolerance.as_ref().map_or(0.0, Quantity::meters),
                layer_data: data,
            };
            let (layer, skipped) = ryuformats::usd::stage::layer_skipping(&model, &opts)
                .map_err(|e| walk.usd_unrepresentable(e))?;
            warn(
                &mut result,
                skipped.iter().map(|x| walk.sheet_brep_skipped(x)).collect(),
            );
            // A usdz's root layer is named by the part, not the file, so
            // where it is written changes no byte.
            let stem = part.to_string();
            deliver(
                s.job(),
                result,
                out.as_deref(),
                format.as_str(),
                |w| match format {
                    Format::Usdc => ryuformats::usd::usdc::write(&layer, w),
                    Format::Usdz => ryuformats::usd::usdz::write(&layer, &stem, w),
                    _ => ryuformats::usd::usda::write(&layer, w),
                },
            )
        }
    }
}

/// A sheet metal solid's flat pattern as DXF (design 346): `flat::of`'s
/// pattern written by `ryuformats::dxf`. An assembly is `invalid_param`,
/// as is a release label: a flat is its commit's.
fn dxf(
    session: &mut Session,
    ask: &Ask,
    part: Id,
    o: ryuformats::dxf::DxfOptions,
    out: Option<&std::path::Path>,
) -> Result<Answer> {
    if ask.rev.as_deref().is_some_and(|r| r.starts_with("label:")) {
        return Err(Error::new(
            Code::InvalidParam,
            "a DXF is of a commit: export the release's commit (--rev <commit>)",
        )
        .with("param", "rev"));
    }
    let (_, s) = session.at(ask.rev.as_deref())?;
    let p = s.project().part(&part)?;
    if crate::part::part_kind(p) == crate::doc::files::PartKind::Assembly {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{part} is an assembly: a DXF is one sheet metal solid's flat"),
        )
        .with("param", "part"));
    }
    let flat_options = crate::sheet_metal::flat::Options {
        fixed: ask.fixed.clone(),
        bends: ask.bends.clone(),
    };
    let (commit, f) = crate::sheet_metal::flat::of(
        session,
        part.as_str(),
        ask.solid.as_deref(),
        ask.rev.as_deref(),
        &flat_options,
    )?;
    let bytes = ryuformats::dxf::write(&f.pattern, &o)
        .map_err(|e| Error::new(Code::Internal, format!("writing DXF: {e}")))?;
    let mut result = json!({
        "part": part,
        "solid": f.solid,
        "format": "dxf",
        "unit": o.unit.as_str(),
        "notes": o.notes,
        "from": match &f.from { crate::sheet_metal::flat::From::Whole => "whole", crate::sheet_metal::flat::From::Unfold(_) => "unfold" },
        "loops": f.pattern.loops.len(),
        "bend_lines": f.pattern.bend_lines.len(),
    });
    if let Some(c) = commit {
        result["commit"] = json!(c);
    }
    let (_, s) = session.at(ask.rev.as_deref())?;
    deliver(s.job(), result, out, "dxf", |w| w.write_all(&bytes))
}

/// USD's model of a scene: each product's solids with their bodies (each
/// distinct body once) and, where meshes are written, `meshes`' meshes.
fn usd_model(scene: &Scene, meshes: Option<&ryuformats::Scene>) -> ryuformats::usd::stage::Model {
    let mut bodies: BTreeMap<&str, Arc<Body>> = BTreeMap::new();
    let products = scene
        .products
        .iter()
        .enumerate()
        .map(|(k, p)| ryuformats::usd::stage::Product {
            id: p.id.clone(),
            name: p.name.clone(),
            part: p.part.to_string(),
            label: p.rev.label().map(str::to_owned),
            solids: p
                .solids
                .iter()
                .enumerate()
                .map(|(i, x)| ryuformats::usd::stage::Solid {
                    brep: !x.built.reference,
                    name: x.founder.to_string(),
                    body: Arc::clone(
                        bodies
                            .entry(&x.built.hash)
                            .or_insert_with(|| Arc::new(x.built.body.clone())),
                    ),
                    mesh: meshes.map(|m| Arc::clone(&m.products[k].solids[i].mesh)),
                })
                .collect(),
            children: p
                .children
                .iter()
                .map(|c| ryuformats::Child {
                    product: c.product,
                    place: c.place,
                    name: c.instance.clone(),
                })
                .collect(),
        })
        .collect();
    ryuformats::usd::stage::Model { products }
}

/// Each product's own triangles in the answer.
fn triangles(result: &mut Value, scene: &ryuformats::Scene) {
    for (k, p) in scene.products.iter().enumerate() {
        let own: usize = p.solids.iter().map(|s| s.mesh.triangles.len()).sum();
        result["products"][k]["triangles"] = json!(own);
    }
}

/// The scene with every solid meshed at `opts`, each body once
/// whatever the occurrences, through the session's mesh cache.
/// `export_too_large` for a body past ryukei's triangle budget,
/// `export_mesh_faults` naming every face that wouldn't mesh.
/// `warnings` added to the answer's `warnings`.
fn warn(result: &mut Value, warnings: Vec<Value>) {
    if warnings.is_empty() {
        return;
    }
    if !result["warnings"].is_array() {
        result["warnings"] = json!([]);
    }
    result["warnings"]
        .as_array_mut()
        .expect("an array")
        .extend(warnings);
}

/// The scene meshed ([`meshed_with`]), a reference's holes warned of in
/// `result`.
fn meshed(
    s: &mut Session,
    scene: &Scene,
    opts: &MeshOptions,
    result: &mut Value,
) -> Result<ryuformats::Scene> {
    let (scene, warnings) = meshed_with(s, scene, opts)?;
    warn(result, warnings);
    Ok(scene)
}

/// The scene with every solid meshed, and `reference_faces_unmeshed`
/// warnings for references whose faces the mesh left out: a reference is
/// written with its holes, where a solid's faults refuse the export.
fn meshed_with(
    s: &mut Session,
    scene: &Scene,
    opts: &MeshOptions,
) -> Result<(ryuformats::Scene, Vec<Value>)> {
    let solids: Vec<(&Product, &Solid)> = scene
        .products
        .iter()
        .flat_map(|p| p.solids.iter().map(move |x| (p, x)))
        .collect();
    let builts: Vec<&Built> = solids.iter().map(|(_, x)| x.built.as_ref()).collect();
    let job = s.job().clone();
    let made = s.caches_mut().meshes_mut().export_all_for(
        &builts,
        &opts.params,
        crate::budget::mesh_budget(),
        &job,
    )?;
    let mut meshes: BTreeMap<&str, Arc<ryukei::mesh::BodyMesh>> = BTreeMap::new();
    let mut faults = Vec::new();
    let mut warnings = Vec::new();
    for ((p, x), m) in solids.iter().zip(&made) {
        let mesh = match &m.mesh {
            Ok(mesh) => mesh,
            Err(e) if e.details.get("triangles").is_some() => {
                return Err(Error::new(
                    Code::ExportTooLarge,
                    format!(
                        "{}/{} would take {} triangles at chord {} and angle {}: past ryukei's {}",
                        x.part,
                        x.founder,
                        e.details["triangles"],
                        opts.chord.text(),
                        opts.angle.text(),
                        opts.params.max_triangles
                    ),
                )
                .with("part", &x.part)
                .with("rev", p.rev.to_json())
                .with("solid", &x.founder)
                .with("triangles", &e.details["triangles"])
                .with("max_triangles", opts.params.max_triangles));
            }
            Err(e) => return Err(e.clone()),
        };
        if meshes.insert(&x.built.hash, Arc::clone(mesh)).is_none() {
            if x.built.reference {
                let display = crate::mesh::DisplayMesh {
                    params: opts.params,
                    mesh: Ok(Arc::clone(mesh)),
                };
                warnings.extend(crate::mesh::unmeshed_warning(
                    &x.part, &x.founder, &x.built, &display,
                ));
            } else {
                faults.extend(fault_report(x, &p.rev, mesh));
            }
        }
    }
    if !faults.is_empty() {
        return Err(Error::new(
            Code::ExportMeshFaults,
            format!(
                "{} face{} of what the export places can't be meshed to chord {} and angle {}",
                faults.len(),
                if faults.len() == 1 { "" } else { "s" },
                opts.chord.text(),
                opts.angle.text()
            ),
        )
        .with("faults", faults));
    }
    let products = scene
        .products
        .iter()
        .map(|p| ryuformats::Product {
            id: p.id.clone(),
            name: p.name.clone(),
            part: p.part.to_string(),
            solids: p
                .solids
                .iter()
                .map(|x| ryuformats::Solid {
                    name: x.founder.to_string(),
                    mesh: Arc::clone(&meshes[x.built.hash.as_str()]),
                    surface: x.built.body.kind() == ryukei::topo::BodyKind::Sheet,
                })
                .collect(),
            children: p
                .children
                .iter()
                .map(|c| ryuformats::Child {
                    product: c.product,
                    place: c.place,
                    name: c.instance.clone(),
                })
                .collect(),
        })
        .collect();
    Ok((ryuformats::Scene { products }, warnings))
}

/// Each face of `x`'s mesh ryukei couldn't mesh: its part, revision,
/// solid and reference (`<part>/<solid>/<name>`), ryukei's fault and, for
/// a fault on an edge, the edge's reference.
fn fault_report(x: &Solid, rev: &Rev, mesh: &ryukei::mesh::BodyMesh) -> Vec<Value> {
    use ryukei::mesh::MeshFaultKind as K;
    let body = &x.built.body;
    let at = |name: ryukei::name::Name| format!("{}/{}/{name}", x.part, x.founder);
    mesh.faults
        .iter()
        .map(|f| {
            let (fault, edge) = match f.kind {
                K::EdgeOffSurface { edge } => ("edge_off_surface", Some(edge)),
                K::BadDomain => ("bad_domain", None),
                K::RefinementStalled => ("refinement_stalled", None),
                K::BoundaryNotResolved { edge } => ("boundary_not_resolved", Some(edge)),
            };
            let mut v = json!({
                "part": x.part,
                "rev": rev.to_json(),
                "solid": x.founder,
                "fault": fault,
            });
            if let Some(face) = body.face(f.face) {
                v["face"] = json!(at(face.name));
            }
            if let Some(e) = edge.and_then(|e| body.edge(e)) {
                v["edge"] = json!(at(e.name));
            }
            v
        })
        .collect()
}

fn invalid(param: &str, value: &str, expected: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("{param} {value:?} isn't one this export takes: expected {expected}"),
    )
    .with("param", param)
}

/// A mesh format's tolerances, as asked for and checked: the chord (the
/// most the mesh may be from the exact faces), the angle (the most the
/// normal may turn across a triangle) and the longest edge, if any. Each
/// a literal quantity: exports read no parameters.
#[derive(Clone, Debug)]
pub struct MeshOptions {
    pub params: MeshParams,
    pub chord: Quantity,
    pub angle: Quantity,
    pub max_edge: Option<Quantity>,
}

/// The chord and angle a mesh export takes by default.
pub const DEFAULT_CHORD: &str = "0.01 mm";
pub const DEFAULT_ANGLE: &str = "15 deg";

impl MeshOptions {
    /// The options from their texts (the defaults where left out), a bare
    /// length in `default` units and a bare angle in degrees.
    pub fn read(
        chord: Option<&str>,
        angle: Option<&str>,
        max_edge: Option<&str>,
        default: units::Unit,
    ) -> Result<MeshOptions> {
        let chord = literal(
            "chord",
            chord.unwrap_or(DEFAULT_CHORD),
            Field::Length(default),
        )?;
        let angle = literal("angle", angle.unwrap_or(DEFAULT_ANGLE), Field::Angle)?;
        let max_edge = max_edge
            .map(|t| literal("max_edge", t, Field::Length(default)))
            .transpose()?;
        let floor = 10.0 * ryukei::tol::LINEAR_RESOLUTION;
        if chord.meters() < floor {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "chord {} is under ryukei's least, {floor} m (0.0001 mm)",
                    chord.text()
                ),
            )
            .with("param", "chord"));
        }
        let a = angle.radians();
        if a <= 0.0 || a > core::f64::consts::FRAC_PI_2 {
            return Err(Error::new(
                Code::InvalidParam,
                format!("angle {} isn't in (0, 90 deg]", angle.text()),
            )
            .with("param", "angle"));
        }
        if let Some(e) = &max_edge
            && e.meters() < chord.meters()
        {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "max_edge {} is shorter than the chord, {}",
                    e.text(),
                    chord.text()
                ),
            )
            .with("param", "max_edge"));
        }
        let params = MeshParams {
            max_edge: max_edge.as_ref().map(Quantity::meters),
            ..MeshParams::new(chord.meters(), a)
        };
        Ok(MeshOptions {
            params,
            chord,
            angle,
            max_edge,
        })
    }

    /// How the answer reports them.
    pub fn report(&self) -> Value {
        let mut v = json!({"chord": self.chord.report(), "angle": self.angle.report()});
        if let Some(e) = &self.max_edge {
            v["max_edge"] = e.report();
        }
        v
    }
}

/// `text` as a literal quantity for `field`, else `invalid_param` naming
/// `param`.
fn literal(param: &str, text: &str, field: Field) -> Result<Quantity> {
    let bad = |why: String| {
        Error::new(Code::InvalidParam, format!("{param} {text:?}: {why}"))
            .with("param", param)
            .with("value", text)
    };
    let q =
        Quantity::eval_as(text, field, &crate::expr::no_parameters).map_err(|e| bad(e.message))?;
    if !q.is_literal() {
        return Err(bad(
            "an export takes a literal such as \"0.01 mm\", not an expression".into(),
        ));
    }
    Ok(q)
}

/// STEP's model of a scene: each product's solids as its bodies, named
/// by their founders (#412), each child's usage as its identifier.
fn step_model(scene: &Scene) -> WriteModel {
    let products = scene
        .products
        .iter()
        .map(|p| ryustep::Product {
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            revision: p.revision.clone(),
            // A reference is passed through as its file has it.
            bodies: p
                .solids
                .iter()
                .filter(|s| !s.built.reference)
                .map(|s| s.built.body.clone())
                .collect(),
            children: p
                .children
                .iter()
                .map(|c| ryustep::Child {
                    product: c.product,
                    place: c.place,
                    id: c.usage.clone(),
                })
                .collect(),
            names: p
                .solids
                .iter()
                .filter(|s| !s.built.reference)
                .map(|s| s.founder.to_string())
                .collect(),
        })
        .collect();
    WriteModel { products }
}

/// Each reference solid's own representation, cut out of its imported
/// file (as step-hunt cuts a body) and named by its founder, beside its
/// product: a reference is written as the file had it, never healed or
/// seamed (design 364). Each file is parsed once.
fn passed_references(walk: &Walk) -> Result<Vec<Cut>> {
    let mut parsed: BTreeMap<String, Arc<ryustep::part21::File>> = BTreeMap::new();
    let mut passed = Vec::new();
    for (k, p) in walk.scene.products.iter().enumerate() {
        for sol in p.solids.iter().filter(|s| s.built.reference) {
            let Some((part, _, _)) = walk.built.get(&(sol.part.clone(), p.rev.clone())) else {
                return Err(Error::new(
                    Code::Internal,
                    format!("export: {}/{} wasn't rebuilt", sol.part, sol.founder),
                ));
            };
            let Some(Ok(crate::doc::files::Feature::Import(i))) = part.features.get(&sol.founder)
            else {
                return Err(Error::new(
                    Code::Internal,
                    format!(
                        "export: {}/{} is a reference but no import",
                        sol.part, sol.founder
                    ),
                ));
            };
            let Some(f) = part.imports.get(&i.file.path) else {
                return Err(Error::new(
                    Code::ImportFileMissing,
                    format!("export: {} is missing", i.file.path),
                ));
            };
            let unreadable = |e: &ryustep::StepError| crate::import::unreadable(&i.file.path, e);
            let file = match parsed.get(&f.blake3) {
                Some(x) => x.clone(),
                None => {
                    let x = Arc::new(ryustep::part21::parse(&f.bytes).map_err(|e| unreadable(&e))?);
                    parsed.insert(f.blake3.clone(), x.clone());
                    x
                }
            };
            let opened = crate::import::opened(f).map_err(|e| unreadable(&e))?;
            let n = crate::import::select(&sol.founder, i, &opened)?;
            let h = &opened.held[n];
            let instances = ryustep::part21::closure::cut(
                &file,
                h.representation,
                h.context,
                &h.items,
                sol.founder.as_str(),
            )
            .map_err(|e| Error::new(Code::Internal, format!("export: {e}")))?;
            let place = match &i.place {
                Some(p) => Some(crate::assembly::pose_rigid(&p.pose)?),
                None => None,
            };
            passed.push(Cut {
                product: k,
                founder: sol.founder.to_string(),
                place,
                instances,
            });
        }
    }
    Ok(passed)
}

/// A reference's representation cut out of its file: the product it
/// belongs to, its founder, where a flattened import placed it, and the
/// instances.
struct Cut {
    product: usize,
    founder: String,
    place: Option<ryukei::geom::Rigid>,
    instances: Vec<ryustep::part21::Instance>,
}

/// STEP's model of a scene and the reference representations passed
/// through: each product's healed solids as its bodies, named by their
/// founders (#412), each child's usage as its identifier, and each
/// reference cut out of its imported file (as step-hunt cuts a body)
/// beside its product. A reference placed by a flattened import stands in
/// a child product of its own (named by its founder), placed by a usage,
/// since a reader takes as a product's own only representations related
/// to it without a transformation (design 364, ryucad 21.D). A healed
/// solid placed so is written as its body already stands.
fn step_parts(walk: &Walk) -> Result<(WriteModel, Vec<ryustep::Passed>)> {
    let mut model = step_model(&walk.scene);
    let mut passed = Vec::new();
    for c in passed_references(walk)? {
        match c.place {
            None => passed.push(ryustep::Passed {
                product: c.product,
                instances: c.instances,
            }),
            Some(r) => {
                let child = model.products.len();
                model.products.push(ryustep::Product {
                    id: c.founder.clone(),
                    name: c.founder.clone(),
                    description: String::new(),
                    revision: String::new(),
                    bodies: Vec::new(),
                    children: Vec::new(),
                    names: Vec::new(),
                });
                model.products[c.product].children.push(ryustep::Child {
                    product: child,
                    place: r,
                    id: c.founder,
                });
                passed.push(ryustep::Passed {
                    product: child,
                    instances: c.instances,
                });
            }
        }
    }
    Ok((model, passed))
}

/// The scene being built, walked from the part exported.
#[derive(Default)]
struct Walk {
    scene: Scene,
    /// Each product's index, by part and revision.
    index: BTreeMap<(Id, Rev), usize>,
    /// Failing features, by part, revision and feature.
    failing: BTreeMap<(Id, String, Id), Value>,
    /// How the part exported resolved, at a release label.
    pin: Option<Pin>,
    /// Every part and revision the export reaches, rebuilt at once
    /// ([`Session::nodes`]).
    built: BTreeMap<(Id, Rev), (Part, PartResult, Option<Pin>)>,
}

impl Walk {
    /// `part` at `rev` rebuilt, its failing features noted.
    fn node(&mut self, s: &mut Session, part: &Id, rev: &Rev) -> Result<(Part, PartResult)> {
        let at = (part.clone(), rev.clone());
        let (p, result, pin) = match self.built.get(&at) {
            Some(n) => n.clone(),
            None => s.node(part, rev)?,
        };
        if self.scene.products.is_empty() && self.pin.is_none() {
            self.pin = pin;
        }
        for (fid, r) in &result.features {
            if let Err(e) = r {
                self.failing
                    .entry((part.clone(), rev.text(), fid.clone()))
                    .or_insert_with(|| {
                        json!({
                            "part": part,
                            "rev": rev.to_json(),
                            "feature": fid,
                            "code": e.code,
                            "message": e.message,
                        })
                    });
            }
        }
        Ok((p, result))
    }

    /// The index of `part` at `rev`'s product, written first if it isn't
    /// yet, with the products it places.
    fn product(&mut self, s: &mut Session, part: &Id, rev: &Rev) -> Result<usize> {
        if let Some(&k) = self.index.get(&(part.clone(), rev.clone())) {
            return Ok(k);
        }
        let (p, result) = self.node(s, part, rev)?;
        let k = self.scene.products.len();
        self.index.insert((part.clone(), rev.clone()), k);
        let props = properties_of(s, part, rev, self.pin.as_ref().filter(|_| k == 0));
        // Another project's part is named by its dependency.
        let fallback = match rev {
            Rev::Foreign { project, .. } => {
                format!("{}:{part}", crate::deps::name_of(s.project(), project))
            }
            _ => part.to_string(),
        };
        let (id, name, description) = identity(&fallback, &p, &props);
        let revision = rev.label().unwrap_or_default().to_owned();
        self.scene.products.push(Product {
            part: part.clone(),
            rev: rev.clone(),
            id,
            name,
            description,
            revision,
            solids: Vec::new(),
            children: Vec::new(),
        });
        let mut mine = Vec::new();
        let mut children = Vec::new();
        if p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly) {
            for (fid, r) in &result.features {
                match r {
                    Ok(Output::Instance(placed)) => {
                        let import = match p.features.get(fid) {
                            Some(Ok(Feature::Instance(i))) => i.import.clone(),
                            _ => None,
                        };
                        let own = import.as_ref().is_some_and(|u| u.usage.is_none())
                            && crate::assembly::rigid_pose(&placed.pose) == Pose::IDENTITY;
                        if own {
                            let (cp, cr) = self.node(s, &placed.part, &placed.rev)?;
                            if !cp.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly) {
                                mine.extend(solids(&cp, &placed.rev, &cr).into_iter().map(
                                    |mut x| {
                                        x.key = format!("{fid}/{}", x.founder);
                                        x
                                    },
                                ));
                                continue;
                            }
                        }
                        let product = self.product(s, &placed.part, &placed.rev)?;
                        let usage = import
                            .and_then(|u| u.usage)
                            .unwrap_or_else(|| fid.to_string());
                        children.push(Child {
                            product,
                            place: placed.pose,
                            usage,
                            instance: fid.to_string(),
                            steps: vec![fid.clone()],
                        });
                    }
                    Ok(Output::Copies(c)) => {
                        for copy in &c.copies {
                            let product = self.product(s, &copy.placed.part, &copy.placed.rev)?;
                            let id = Id::copy(fid, copy.index).to_string();
                            children.push(Child {
                                product,
                                place: copy.placed.pose,
                                usage: id.clone(),
                                instance: id,
                                steps: vec![Id::copy(fid, copy.index), copy.seed.clone()],
                            });
                        }
                    }
                    _ => {}
                }
            }
        } else {
            mine = solids(&p, rev, &result);
        }
        let made = &mut self.scene.products[k];
        made.solids = mine;
        made.children = children;
        Ok(k)
    }

    /// `export_unrepresentable` for a face ryustep couldn't write, naming
    /// its part and solid.
    fn unwritable(&self, e: WriteError) -> Error {
        let WriteError::NotRepresentable { face } = e else {
            return Error::new(Code::Internal, format!("writing STEP: {e}"));
        };
        let mut err = Error::new(
            Code::ExportUnrepresentable,
            format!(
                "face {face} can't be described in STEP: its loops can't be cut open along seams"
            ),
        )
        .with("face", face.to_string());
        let owner = self
            .scene
            .products
            .iter()
            .flat_map(|p| &p.solids)
            .find(|s| has_face(&s.built.body, face));
        if let Some(Solid { part, founder, .. }) = owner {
            err = err
                .with("part", part)
                .with("solid", founder)
                .with("entity", format!("{part}/{founder}/{face}"));
        }
        err
    }
}

impl Walk {
    /// `export_unrepresentable` for a face USD's BrepArray can't describe,
    /// naming its part, revision, solid and why.
    fn usd_unrepresentable(&self, e: ryuformats::usd::stage::StageError) -> Error {
        use ryuformats::usd::stage::StageError;
        let StageError::Brep {
            product,
            solid,
            error,
        } = e
        else {
            return Error::new(Code::Internal, format!("writing USD: {e}"));
        };
        let p = &self.scene.products[product];
        let x = &p.solids[solid];
        Error::new(
            Code::ExportUnrepresentable,
            format!(
                "face {} of {}/{} can't be described as an omniSolid BrepArray ({})",
                error.face, x.part, x.founder, error.reason
            ),
        )
        .with("part", &x.part)
        .with("rev", p.rev.to_json())
        .with("solid", &x.founder)
        .with("face", error.face.to_string())
        .with("entity", format!("{}/{}/{}", x.part, x.founder, error.face))
        .with("reason", error.reason)
    }

    /// The warning for a sheet the stage wrote as its mesh alone, its
    /// BrepArray refused (#624).
    fn sheet_brep_skipped(&self, x: &ryuformats::usd::stage::Skipped) -> Value {
        let s = &self.scene.products[x.product].solids[x.solid];
        sheet_brep_skipped(&format!("{}/{}", s.part, s.founder), &x.error)
    }
}

/// `export_sheet_brep_skipped` for sheet `solid` (`<part>/<founder>`),
/// whose BrepArray the writer refused with `error`: written as its mesh.
fn sheet_brep_skipped(solid: &str, error: &ryuformats::usd::brep::BrepError) -> Value {
    json!({"code": "export_sheet_brep_skipped", "solid": solid,
    "face": error.face.to_string(), "reason": error.reason,
    "message": format!(
        "{solid} is a sheet whose face {} can't be written as a BrepArray ({}): written as its mesh alone",
        error.face, error.reason
    )})
}

fn has_face(b: &Body, name: ryukei::name::Name) -> bool {
    b.faces().any(|f| b.face(f).expect("live").name == name)
}

/// A part's solids as it shows them, in model order.
fn solids(p: &Part, rev: &Rev, result: &PartResult) -> Vec<Solid> {
    let m = Model::of_part(p, rev, result);
    m.occurrences
        .into_iter()
        .flat_map(|o| o.solids)
        .map(|s| Solid {
            part: p.id.clone(),
            key: s.founder.to_string(),
            founder: s.founder,
            built: s.built,
        })
        .collect()
}

/// The product an imported part or assembly came from: the one product
/// every import feature of a part names, or every imported instance of
/// an assembly is used in.
pub(crate) fn imported(p: &Part) -> Option<ProductRef> {
    let mut named: BTreeSet<Option<ProductRef>> = BTreeSet::new();
    for fid in p.order() {
        match p.features.get(fid) {
            Some(Ok(Feature::Import(i))) => {
                named.insert(i.product.clone());
            }
            Some(Ok(Feature::Instance(i))) => {
                if let Some(u) = &i.import {
                    named.insert(Some(u.product.clone()));
                }
            }
            _ => {}
        }
    }
    match named.len() {
        1 => named.into_iter().next().flatten(),
        _ => None,
    }
}

/// A product's identifier, name and description: the PDM number, name
/// and description, else the imported product's identifier and name, else
/// `fallback` (the part's ID, or `<dependency>:<part>` for another
/// project's).
fn identity(fallback: &str, p: &Part, props: &Properties) -> (String, String, String) {
    let text = |key: &str| {
        props
            .values
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned)
    };
    let from = imported(p);
    let id = text("number")
        .or_else(|| from.as_ref().map(|f| f.id.clone()))
        .unwrap_or_else(|| fallback.to_owned());
    let name = text("name")
        .or_else(|| from.as_ref().map(|f| f.name.clone()))
        .unwrap_or_else(|| fallback.to_owned());
    (id, name, text("description").unwrap_or_default())
}

/// The properties STEP export writes as the product itself.
const WRITTEN_PROPERTIES: [&str; 3] = ["number", "name", "description"];

/// What an export's styles leave out (design 448), counted for its
/// transfer report.
#[derive(Debug, Default, PartialEq)]
struct Unpainted {
    /// Faces drawn otherwise only in an assembly: STEP's occurrence
    /// styles are per body.
    occurrence_faces: usize,
    /// Looks with an opacity and no colour: STEP's fill needs a colour.
    uncoloured: usize,
    /// Reference solids with a look: passed through as their file has
    /// them.
    references: usize,
    /// Products of another project's parts: their appearance is theirs.
    foreign: usize,
}

impl Unpainted {
    fn count(&self) -> usize {
        self.occurrence_faces + self.uncoloured + self.references + self.foreign
    }

    fn summary(&self) -> String {
        [
            (
                self.occurrence_faces,
                "occurrence face(s) coloured otherwise only in an assembly (STEP styles an occurrence's body)",
            ),
            (
                self.uncoloured,
                "look(s) with an opacity and no colour (STEP's fill needs a colour)",
            ),
            (
                self.references,
                "reference solid(s) with a look (written as their file has them)",
            ),
            (
                self.foreign,
                "product(s) of another project, whose appearance isn't read",
            ),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{n} {w}"))
        .collect::<Vec<_>>()
        .join("; ")
    }
}

/// A look's colour and opacity as STEP's paint; `None` with no colour.
fn paint(e: &crate::appearance::Effective) -> Option<ryustep::Paint> {
    let c = e.colour.as_deref()?.strip_prefix('#')?;
    let byte = |k: usize| u8::from_str_radix(c.get(k..k + 2)?, 16).ok();
    Some(ryustep::Paint {
        colour: [byte(0)?, byte(2)?, byte(4)?],
        opacity: e.opacity,
    })
}

/// Whether two looks draw alike, wherever their values came from.
fn alike(a: &crate::appearance::Effective, b: &crate::appearance::Effective) -> bool {
    a.colour == b.colour && a.opacity == b.opacity
}

/// The paints of `walk`'s scene (design 448): each product's healed
/// bodies' looks as its part draws them (`looks` of that part in `sess`,
/// the session of the commit the export reads), and each occurrence of
/// the root whose look differs from its product's as an override; with
/// what STEP can't carry.
fn step_paints(sess: &mut Session, walk: &Walk) -> (ryustep::Paints, Unpainted) {
    use crate::appearance::SolidLook;
    let scene = &walk.scene;
    let mut looks: BTreeMap<Id, BTreeMap<String, SolidLook>> = BTreeMap::new();
    let mut un = Unpainted::default();
    for p in &scene.products {
        if matches!(p.rev, Rev::Foreign { .. }) {
            un.foreign += 1;
            continue;
        }
        if !looks.contains_key(&p.part) {
            // A part the commit doesn't build has no look to write.
            let l = crate::appearance::looks(sess, &p.part).unwrap_or_default();
            looks.insert(p.part.clone(), l);
        }
    }
    let plain = SolidLook::default();
    let mut out = ryustep::Paints::default();
    for (k, p) in scene.products.iter().enumerate() {
        let foreign = matches!(p.rev, Rev::Foreign { .. });
        let mut body = 0;
        for sol in &p.solids {
            let l = look_in(&looks, &plain, scene, k, sol);
            if sol.built.reference {
                if !l.is_plain() {
                    un.references += 1;
                }
                continue;
            }
            let j = body;
            body += 1;
            if foreign {
                continue;
            }
            let base = paint(&l.base);
            if base.is_none() && l.base.opacity < 1.0 {
                un.uncoloured += 1;
            }
            let mut faces = Vec::new();
            for (name, e) in &l.faces {
                match (crate::doc::reference::parse_name(name), paint(e)) {
                    (Some(n), Some(pt)) => faces.push((n, pt)),
                    _ => un.uncoloured += 1,
                }
            }
            if base.is_some() || !faces.is_empty() {
                out.bodies.push(ryustep::BodyPaint {
                    product: k,
                    body: j,
                    base,
                    faces,
                });
            }
        }
    }
    occurrence_paints(scene, &looks, &mut out, &mut un);
    (out, un)
}

/// What an export's materials and properties leave out (design 448,
/// plan 24.C), counted for its transfer report.
#[derive(Debug, Default, PartialEq)]
struct Unwritten {
    /// Values STEP can't hold: neither text, number nor boolean.
    values: usize,
    /// Materials of another project's parts, written without a density:
    /// their library is theirs.
    foreign_densities: usize,
}

/// A declared quantity's text as a length or an angle in SI; any other
/// dimension, or text that doesn't read, as the text.
fn quantity_value(text: &str, dim: crate::units::Dim, unit: units::Unit) -> ryustep::DataValue {
    use crate::units::Dim;
    use ryustep::DataValue;
    let si = crate::units::Quantity::parse(text, unit)
        .ok()
        .filter(|q| q.dim() == dim)
        .map(|q| q.si())
        .or_else(|| {
            crate::expr::Expr::parse(text)
                .and_then(|e| e.eval(&crate::expr::no_parameters))
                .ok()
                .filter(|v| v.dim == dim)
                .map(|v| v.x)
        });
    match si {
        Some(x) if dim == Dim::LENGTH => DataValue::Length(x),
        Some(x) if dim == Dim::ANGLE => DataValue::Angle(x),
        _ => DataValue::Text(text.to_owned()),
    }
}

/// Each product's material, with its library's density, and its set
/// properties but those STEP's product carries, from the files `sess`
/// reads (the export's, or a label's official commit's).
fn step_data(sess: &mut Session, walk: &Walk) -> (ryustep::PartData, Unwritten) {
    use ryupdm::config::Type;
    use ryustep::{AttributeData, DataValue, MaterialData, ProductData};
    let unit = sess.project().manifest.default_unit;
    let config = ryupdm::config::Config::read(&sess.files()).unwrap_or_default();
    let lib = crate::materials::library(sess).unwrap_or_default();
    let mut out = ryustep::PartData::default();
    let mut u = Unwritten::default();
    for (k, p) in walk.scene.products.iter().enumerate() {
        let props = properties_of(sess, &p.part, &p.rev, walk.pin.as_ref().filter(|_| k == 0));
        let foreign = matches!(p.rev, Rev::Foreign { .. });
        let material = props
            .values
            .get("material")
            .and_then(Value::as_str)
            .map(|name| MaterialData {
                name: name.to_owned(),
                density: (!foreign)
                    .then(|| lib.materials.get(name).map(|m| m.density.si()))
                    .flatten(),
            });
        if foreign && material.is_some() {
            u.foreign_densities += 1;
        }
        let mut attributes = Vec::new();
        for (key, v) in &props.values {
            if WRITTEN_PROPERTIES.contains(&key.as_str()) || key == "material" || v.is_null() {
                continue;
            }
            let value = match (config.decl(key).map(|d| d.ty), v) {
                (Some(Type::Quantity(dim)), Value::String(t)) => quantity_value(t, dim, unit),
                (_, Value::String(t)) => DataValue::Text(t.clone()),
                (_, Value::Bool(b)) => DataValue::Boolean(*b),
                (_, Value::Number(n)) => match n.as_i64() {
                    Some(i) => DataValue::Integer(i),
                    None => DataValue::Number(n.as_f64().unwrap_or_default()),
                },
                _ => {
                    u.values += 1;
                    continue;
                }
            };
            attributes.push(AttributeData {
                name: key.clone(),
                value,
            });
        }
        if material.is_some() || !attributes.is_empty() {
            out.products.push(ProductData {
                product: k,
                material,
                attributes,
            });
        }
    }
    (out, u)
}

/// Solid `sol` of product `k`'s look as its part draws it; `plain` for
/// none.
fn look_in<'a>(
    looks: &'a BTreeMap<Id, BTreeMap<String, crate::appearance::SolidLook>>,
    plain: &'a crate::appearance::SolidLook,
    scene: &Scene,
    k: usize,
    sol: &Solid,
) -> &'a crate::appearance::SolidLook {
    looks
        .get(&scene.products[k].part)
        .and_then(|m| m.get(&sol.key))
        .unwrap_or(plain)
}

/// Each occurrence of the scene's root whose solid is drawn otherwise
/// than its product draws it (the root's looks, by
/// `<path>/<key>`): an override on that body in its usages; each face
/// drawn otherwise than a reader composes it (the product's face style,
/// else the occurrence's) is omitted.
fn occurrence_paints(
    scene: &Scene,
    looks: &BTreeMap<Id, BTreeMap<String, crate::appearance::SolidLook>>,
    out: &mut ryustep::Paints,
    un: &mut Unpainted,
) {
    let Some(root_looks) = scene.products.first().and_then(|r| looks.get(&r.part)) else {
        return;
    };
    let plain = crate::appearance::SolidLook::default();
    // Each product to walk, the usages that reach it, and its path's steps.
    type Walking = (usize, Vec<(usize, usize)>, Vec<Id>);
    let mut stack: Vec<Walking> = vec![(0, Vec::new(), Vec::new())];
    while let Some((k, path, steps)) = stack.pop() {
        for (n, c) in scene.products[k].children.iter().enumerate() {
            let mut path = path.clone();
            path.push((k, n));
            let mut steps = steps.clone();
            steps.extend(c.steps.iter().cloned());
            let child = &scene.products[c.product];
            let foreign = matches!(child.rev, Rev::Foreign { .. });
            let mut body = 0;
            for sol in &child.solids {
                if sol.built.reference {
                    continue;
                }
                let j = body;
                body += 1;
                let key = format!("{}/{}", crate::assembly::path_text(&steps), sol.key);
                let Some(actual) = root_looks.get(&key) else {
                    continue;
                };
                let written = if foreign {
                    &plain
                } else {
                    look_in(looks, &plain, scene, c.product, sol)
                };
                let over = !alike(&actual.base, &written.base);
                if over {
                    match paint(&actual.base) {
                        Some(pt) => out.occurrences.push(ryustep::OccurrencePaint {
                            path: path.clone(),
                            body: j,
                            paint: pt,
                        }),
                        None => un.uncoloured += 1,
                    }
                }
                let names: BTreeSet<&String> =
                    actual.faces.keys().chain(written.faces.keys()).collect();
                for name in names {
                    let under = if over { &actual.base } else { &written.base };
                    let reader = written.faces.get(name).unwrap_or(under);
                    if !alike(actual.face(name), reader) {
                        un.occurrence_faces += 1;
                    }
                }
            }
            stack.push((c.product, path, steps));
        }
    }
    out.occurrences
        .sort_by(|a, b| (&a.path, a.body).cmp(&(&b.path, b.body)));
}

/// How many solids of the parts an export with `appearance: false`
/// writes have a look: what it leaves out.
fn appearance_left_out(s: &mut Session, walk: &Walk) -> Result<usize> {
    let mut seen = BTreeSet::new();
    let mut n = 0;
    for p in &walk.scene.products {
        if matches!(p.rev, Rev::Foreign { .. }) || !seen.insert(p.part.clone()) {
            continue;
        }
        n += crate::appearance::looks(s, &p.part)
            .map(|l| l.values().filter(|x| !x.is_plain()).count())
            .unwrap_or(0);
    }
    Ok(n)
}

/// What a STEP export wrote and left out (design 448): `written`, the
/// writer's instances by category (at most eight names each, the
/// largest first); `omitted`, what the written parts hold that STEP
/// export doesn't write; `approximated`, each body written within a
/// tolerance. With the `export_metadata_omitted` warnings to raise.
fn step_transfer(
    s: &mut Session,
    walk: &Walk,
    w: &ryustep::Written,
    model: &WriteModel,
    unpainted: Option<&Unpainted>,
    unwritten: Option<&Unwritten>,
    off: Option<usize>,
) -> Result<(Value, Vec<Value>)> {
    use crate::doc::files::Feature;
    use crate::transfer::SUMMARY_NAMES;
    use ryustep::census::Category;
    let written: Vec<Value> = Category::ALL
        .iter()
        .filter_map(|&c| {
            let mut names: Vec<(&str, u64)> = w
                .counts
                .iter()
                .filter(|x| x.0 == c)
                .map(|x| (x.1.as_str(), x.2))
                .collect();
            if names.is_empty() {
                return None;
            }
            names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let count: u64 = names.iter().map(|x| x.1).sum();
            let top: Vec<Value> = names
                .iter()
                .take(SUMMARY_NAMES)
                .map(|(n, k)| json!({"name": n, "count": k}))
                .collect();
            let mut v = json!({"category": c.as_str(), "count": count, "names": top});
            if names.len() > SUMMARY_NAMES {
                v["more"] = json!(names.len() - SUMMARY_NAMES);
            }
            Some(v)
        })
        .collect();
    let approximated: Vec<Value> = w
        .approximated
        .iter()
        .map(|a| {
            let p = &model.products[a.product];
            json!({"product": p.id, "body": p.names.get(a.body).cloned().unwrap_or_default(),
                   "faces": a.faces, "bound_m": a.bound})
        })
        .collect();
    let mut seen: BTreeSet<(Id, Rev)> = BTreeSet::new();
    let (mut features, mut sketches, mut datums) = (0usize, 0usize, 0usize);
    let mut properties: BTreeSet<(Id, String)> = BTreeSet::new();
    for p in &walk.scene.products {
        if !seen.insert((p.part.clone(), p.rev.clone())) {
            continue;
        }
        if let Some((part, _, _)) = walk.built.get(&(p.part.clone(), p.rev.clone())) {
            for f in part.features.values().flatten() {
                features += 1;
                match f {
                    Feature::Sketch(_) => sketches += 1,
                    Feature::Datum(_) => datums += 1,
                    _ => {}
                }
            }
        }
        let props = properties_of(s, &p.part, &p.rev, walk.pin.as_ref());
        for (k, v) in &props.values {
            if !WRITTEN_PROPERTIES.contains(&k.as_str()) && !v.is_null() {
                properties.insert((p.part.clone(), k.clone()));
            }
        }
    }
    let written_parts: BTreeSet<&str> = seen.iter().map(|(p, _)| p.as_str()).collect();
    let annotations = crate::annotation::readable(&crate::annotation::read_all(&s.snapshot()))
        .values()
        .filter(|a| a.status == crate::doc::annotation::Status::Open)
        .filter(|a| a.pins.iter().any(|pin| written_parts.contains(pin.part())))
        .count();
    let mut omitted = vec![json!({
        "what": "history",
        "count": features,
        "summary": format!(
            "feature history and parameters: {features} feature(s) in {} part(s), written as their shapes",
            seen.len()
        ),
    })];
    for (what, n, words) in [
        ("sketches", sketches, "sketch(es), not written"),
        ("datums", datums, "datum feature(s), not written"),
        (
            "annotations",
            annotations,
            "open annotation(s) on the parts, not written",
        ),
    ] {
        if n > 0 {
            omitted.push(json!({"what": what, "count": n, "summary": format!("{n} {words}")}));
        }
    }
    match unwritten {
        // properties: false — every property it would have written.
        None if !properties.is_empty() => omitted.push(json!({
            "what": "properties",
            "count": properties.len(),
            "summary": format!(
                "{} part propert(ies) and material(s), not written (properties: false)",
                properties.len()
            ),
        })),
        None => {}
        Some(u) => {
            if u.values > 0 {
                omitted.push(json!({"what": "properties", "count": u.values,
                    "summary": format!("{} propert(ies) whose values STEP can't hold (neither text, number nor boolean), not written", u.values)}));
            }
            if u.foreign_densities > 0 {
                omitted.push(json!({"what": "properties", "count": u.foreign_densities,
                    "summary": format!("{} material(s) of another project's parts, written without their density", u.foreign_densities)}));
            }
        }
    }
    if let Some(u) = unpainted.filter(|u| u.count() > 0) {
        omitted.push(json!({"what": "appearance", "count": u.count(), "summary": u.summary()}));
    }
    if let Some(n) = off.filter(|n| *n > 0) {
        omitted.push(json!({"what": "appearance", "count": n,
            "summary": format!("{n} solid(s) with a colour or opacity, not written (appearance: false)")}));
    }
    let warnings: Vec<Value> = omitted
        .iter()
        .filter(|o| matches!(o["what"].as_str(), Some("appearance" | "properties")))
        .map(|o| {
            json!({"code": "export_metadata_omitted", "what": o["what"], "count": o["count"],
                   "message": format!("export_metadata_omitted: {}", o["summary"].as_str().unwrap_or_default())})
        })
        .collect();
    Ok((
        json!({"written": written, "omitted": omitted, "approximated": approximated}),
        warnings,
    ))
}

/// `part`'s properties at `rev`: the working files', or for an official
/// release (`pin`, for the part exported; else looked up) its commit's.
fn properties_of(s: &mut Session, part: &Id, rev: &Rev, pin: Option<&Pin>) -> Properties {
    let commit = match (rev, pin) {
        (Rev::Working, _) => None,
        // Another project's properties are read where its files are.
        (Rev::Foreign { project, label }, _) => {
            return crate::deps::properties(s, project, part, label).unwrap_or_default();
        }
        (Rev::Label(_), Some(p)) => p.commit.clone(),
        (Rev::Label(l), None) => crate::release::pinned(s, part, l)
            .ok()
            .and_then(|p| p.pin.commit),
    };
    let read = match commit {
        Some(c) => crate::git::repo(s.root())
            .and_then(|r| r.files(&c))
            .ok()
            .and_then(|files| ryupdm::props::read(&files, part).ok()),
        None => ryupdm::props::read(&*s, part).ok(),
    };
    read.unwrap_or_default()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `secs` since the Unix epoch as an ISO 8601 date and time in UTC, to the
/// second (Hinnant's civil-from-days).
pub fn utc(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::assembly::tests::{place_in, project, released};
    use crate::mate::tests::add_in;
    use crate::testing::{call, fresh};

    /// The answer to an export with `params`.
    pub(crate) fn answered(s: &mut Session, params: Value) -> Result<Answer> {
        let req = crate::proto::Request::from_rpc("export", Some(params))?;
        crate::server::answer_live(s, None, None, req)
    }

    /// The answer to an export with `params`, and the file's bytes.
    pub(crate) fn exported(s: &mut Session, params: Value) -> Result<(Value, Vec<u8>)> {
        let a = answered(s, params)?;
        Ok((a.result, a.binary.expect("an export answers with bytes")))
    }

    fn opened(bytes: &[u8]) -> ryustep::Opened {
        ryustep::open(bytes).unwrap()
    }

    /// The header's time stamp.
    fn time_stamp(bytes: &[u8]) -> String {
        let o = opened(bytes);
        let name = o.header.entities.iter().find(|e| &*e.name == "FILE_NAME");
        match &name.unwrap().params[1] {
            ryustep::part21::Param::Str(t) => t.to_string(),
            p => panic!("{p:?}"),
        }
    }

    fn add(s: &mut Session, part: &str, f: Value) {
        let mut f = f;
        f["part"] = json!(part);
        call(s, "feature.add", f).unwrap();
    }

    /// A STEP export says what it wrote by category, what it left out
    /// (always the history; the appearance when asked not to write it)
    /// and what it approximated (design 448).
    #[test]
    fn a_step_export_reports_what_it_wrote_and_left_out() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        add(
            &mut s,
            "p",
            json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
        );
        let opts = json!({"part": "p", "time_stamp": "2026-10-27T00:00:00", "file_name": "p.step"});
        let (plain, before) = exported(&mut s, opts.clone()).unwrap();
        let t = &plain["transfer"];
        let written: Vec<&str> = t["written"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["category"].as_str().unwrap())
            .collect();
        for c in ["geometry", "topology", "product_structure", "units"] {
            assert!(written.contains(&c), "{c}: {written:?}");
        }
        assert!(!written.contains(&"unknown"), "{written:?}");
        assert_eq!(
            t["omitted"],
            json!([{"what": "history", "count": 1,
                    "summary": "feature history and parameters: 1 feature(s) in 1 part(s), written as their shapes"}])
        );
        assert_eq!(t["approximated"], json!([]));
        assert!(plain.get("warnings").is_none(), "{plain}");
        call(
            &mut s,
            "appearance.set",
            json!({"entities": ["p"], "colour": "#ff0000"}),
        )
        .unwrap();
        // Written now: the bytes grow by the styles, the geometry's stay.
        let (coloured, after) = exported(&mut s, opts.clone()).unwrap();
        assert_ne!(after, before);
        let plain_file = ryustep::part21::parse(&before).unwrap();
        let styled = ryustep::part21::parse(&after).unwrap();
        assert_eq!(
            format!("{:?}", &styled.instances[..plain_file.instances.len()]),
            format!("{:?}", plain_file.instances)
        );
        let what = |v: &Value| -> Vec<String> {
            v["transfer"]["omitted"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["what"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(what(&coloured), ["history"]);
        assert!(coloured.get("warnings").is_none(), "{coloured}");
        // Asked to leave it out: the plain bytes, the omission said.
        let mut off = opts;
        off["appearance"] = json!(false);
        let (left_out, bytes) = exported(&mut s, off).unwrap();
        assert_eq!(bytes, before, "the bytes don't change");
        assert_eq!(what(&left_out), ["history", "appearance"]);
        let w = left_out["warnings"].as_array().unwrap();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(
            (&w[0]["code"], &w[0]["what"]),
            (&json!("export_metadata_omitted"), &json!("appearance"))
        );
    }

    #[test]
    fn a_reference_exports_as_its_files_own_representation() {
        use crate::doc::import::Mode;
        let bytes = crate::import::tests::occt("box_ap214.stp");
        let (_d, mut s) = crate::import::tests::with_import_as(
            &bytes,
            Some(("part 2", "part 2")),
            Some(Mode::Reference),
        );
        let (_, out) = exported(
            &mut s,
            json!({"part": "p", "time_stamp": "2026-11-06T00:00:00"}),
        )
        .unwrap();
        // The file's faces' instances, renumbered: the same reference read
        // back under the same key, so the same names and body.
        let o = opened(&out);
        assert_eq!(o.held.len(), 1);
        let key = crate::rebuild::op_key(
            &Id::parse("p", "part").unwrap(),
            &Id::parse("i", "feature").unwrap(),
        );
        let ro = ryustep::ReadOptions {
            key,
            heal: Default::default(),
        };
        let again = o.reference(0, &ro).result.unwrap().0;
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        assert_eq!(
            blake3::hash(again.dump().as_bytes()).to_hex()[..32],
            r["parts"][0]["solids"][0]["hash"]
        );
        // No seamed body was written for it.
        assert!(!String::from_utf8_lossy(&out).contains("ryukei:seam"));
    }

    #[test]
    fn a_reference_exports_as_meshes_with_usd_saying_so() {
        use crate::doc::import::Mode;
        let bytes = crate::import::tests::occt("box_ap214.stp");
        let (_d, mut s) = crate::import::tests::with_import_as(
            &bytes,
            Some(("part 2", "part 2")),
            Some(Mode::Reference),
        );
        for format in ["stl", "3mf", "usda"] {
            let (v, out) = exported(&mut s, json!({"part": "p", "format": format})).unwrap();
            assert!(!out.is_empty(), "{format}: {v}");
        }
        // USD writes it as a mesh alone, and says so.
        let (v, out) = exported(&mut s, json!({"part": "p", "format": "usda"})).unwrap();
        let w = v["warnings"].as_array().unwrap();
        assert!(
            w.iter().any(|w| w["code"] == "export_reference_mesh_only"),
            "{v}"
        );
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("BrepArray"), "a BrepArray of a reference");
        assert!(text.contains("ryucad:faceName"));
        // Without meshes there is nothing to write for it.
        let e = answered(
            &mut s,
            json!({"part": "p", "format": "usda", "mesh": false}),
        )
        .err()
        .unwrap();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["solid"], "p/i");
    }

    #[test]
    fn a_sheet_is_written_to_usd_with_its_breparray() {
        use crate::testing::{call, fresh};
        let (_d, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
                {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
                {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}]}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "w", "sketch": "s", "surface": true,
                   "extent": {"type": "blind", "length": "20 mm"}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 5, "y": 5, "z": 5},
                   "placement": {"origin": [20, 0, 0]}}),
        )
        .unwrap();
        let (v, out) = exported(&mut s, json!({"part": "p", "format": "usda"})).unwrap();
        assert!(
            v["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|w| w["code"] != "export_sheet_brep_skipped"),
            "{v}"
        );
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text.matches("BrepArray").count(),
            2,
            "the wall's and the box's"
        );
        // Without meshes a sheet is still written: as its BrepArray.
        let (_, out) = exported(
            &mut s,
            json!({"part": "p", "format": "usda", "mesh": false}),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            (
                text.matches("BrepArray").count(),
                text.matches("def Mesh").count()
            ),
            (2, 0)
        );
    }

    #[test]
    fn a_sheet_whose_breparray_is_refused_is_warned_of_by_face() {
        let e = ryuformats::usd::brep::BrepError {
            face: ryukei::name::Name::from_u128(0xab),
            reason: "domain",
        };
        let w = sheet_brep_skipped("p/w", &e);
        assert_eq!(
            (
                w["code"].as_str(),
                w["solid"].as_str(),
                w["reason"].as_str()
            ),
            (
                Some("export_sheet_brep_skipped"),
                Some("p/w"),
                Some("domain")
            )
        );
        assert_eq!(w["face"], json!(e.face.to_string()));
        assert!(
            w["message"]
                .as_str()
                .unwrap()
                .ends_with("written as its mesh alone")
        );
    }

    #[test]
    fn a_references_mesh_faults_are_warnings() {
        use crate::doc::import::Mode;
        let bytes = crate::import::tests::occt("box_ap214.stp");
        let (_d, mut s) = crate::import::tests::with_import_as(
            &bytes,
            Some(("part 2", "part 2")),
            Some(Mode::Reference),
        );
        let p = Id::parse("p", "part").unwrap();
        let r = s.rebuilt(&p).unwrap();
        let b = r
            .features
            .iter()
            .find_map(|(_, o)| o.as_ref().ok()?.solid(&Id::parse("i", "feature").unwrap()))
            .unwrap()
            .clone();
        let params = ryukei::mesh::MeshParams::new(1e-4, 0.3);
        let mut m = ryukei::mesh::tessellate(&b.body, &params).unwrap();
        m.faults.push(ryukei::mesh::FaceFault {
            face: b.body.faces().next().unwrap(),
            kind: ryukei::mesh::MeshFaultKind::BadDomain,
        });
        let display = crate::mesh::DisplayMesh {
            params,
            mesh: Ok(Arc::new(m.clone())),
        };
        let w =
            crate::mesh::unmeshed_warning(&p, &Id::parse("i", "feature").unwrap(), &b, &display)
                .unwrap();
        assert_eq!(w["code"], "reference_faces_unmeshed");
        // A solid's faults stay `export_mesh_faults`.
        let solid = Solid {
            part: p.clone(),
            key: "i".into(),
            founder: Id::parse("i", "feature").unwrap(),
            built: Arc::new(crate::rebuild::Built {
                reference: false,
                ..b.clone()
            }),
        };
        assert_eq!(fault_report(&solid, &Rev::Working, &m).len(), 1);
    }

    #[test]
    fn mesh_options_are_literals_in_range() {
        let mm = units::Unit::Mm;
        let o = MeshOptions::read(None, None, None, mm).unwrap();
        assert_eq!(
            (o.params.chord, o.params.angle),
            (0.01 * 1e-3, 15.0 * units::DEG)
        );
        assert_eq!(
            (o.params.max_edge, o.params.max_triangles),
            (None, 50_000_000)
        );
        assert_eq!(
            o.report(),
            json!({"chord": {"value": "0.01 mm", "m": 0.01 * 1e-3},
                   "angle": {"value": "15 deg", "rad": 15.0 * units::DEG}})
        );
        // A bare length takes the project's unit, a bare angle degrees.
        let o = MeshOptions::read(Some("0.5"), Some("10"), Some("2 cm"), units::Unit::Cm).unwrap();
        assert_eq!(o.params.chord, 0.5 * 0.01);
        assert_eq!(o.params.angle, 10.0 * units::DEG);
        assert_eq!(o.params.max_edge, Some(2.0 * 0.01));
        assert_eq!(o.report()["max_edge"]["value"], "2 cm");
        for (chord, angle, edge, param) in [
            (Some("0.00001 mm"), None, None, "chord"),
            (Some("-1 mm"), None, None, "chord"),
            (Some("2 * 0.01 mm"), None, None, "chord"),
            (Some("wall"), None, None, "chord"),
            (Some("5 deg"), None, None, "chord"),
            (None, Some("0 deg"), None, "angle"),
            (None, Some("91 deg"), None, "angle"),
            (None, Some("1 mm"), None, "angle"),
            (None, None, Some("0.001 mm"), "max_edge"),
        ] {
            let e = MeshOptions::read(chord, angle, edge, mm).unwrap_err();
            assert_eq!(
                (e.code, &e.details["param"]),
                (Code::InvalidParam, &json!(param)),
                "{chord:?} {angle:?} {edge:?}: {}",
                e.message
            );
        }
        // Ninety degrees is the most.
        assert!(MeshOptions::read(None, Some("90 deg"), None, mm).is_ok());
    }

    #[test]
    fn times_are_utc_to_the_second() {
        assert_eq!(utc(0), "1970-01-01T00:00:00");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00");
        assert_eq!(utc(1_761_573_470), "2025-10-27T13:57:50");
        assert_eq!(utc(-1), "1969-12-31T23:59:59");
    }

    #[test]
    fn a_sheet_reads_back_as_its_founder() {
        // #675: a sheet's SHELL_BASED_SURFACE_MODEL carries its founder, as
        // a solid's MANIFOLD_SOLID_BREP does (#412).
        let (_d, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        add(
            &mut s,
            "p",
            json!({"type": "box", "id": "h", "params": {"x": 5, "y": 5, "z": 5},
                   "placement": {"origin": [20, 0, 0]}}),
        );
        add(
            &mut s,
            "p",
            json!({"type": "sketch", "id": "s", "plane": "xy"}),
        );
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
                {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
                {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}]}),
        )
        .unwrap();
        add(
            &mut s,
            "p",
            json!({"type": "extrude", "id": "w", "sketch": "s", "surface": true,
                   "extent": {"type": "blind", "length": "20 mm"}}),
        );
        let (_, bytes) = exported(&mut s, json!({"part": "p"})).unwrap();
        let mut names: Vec<String> = opened(&bytes).held.iter().map(|h| h.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["h", "w"]);
        // Imported again, the sheet keeps its ID as the solid does.
        let (_out, out) = outside(&bytes, "p.step");
        let (_dir2, mut s2) = fresh();
        let v = call(&mut s2, "import", json!({"solid": true, "path": out})).unwrap();
        let mut ids: Vec<&str> = v["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, ["h", "w"], "{v}");
    }

    #[test]
    fn a_part_is_one_product_of_the_solids_it_shows() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        for f in [
            json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
            json!({"type": "box", "id": "lug", "params": {"x": 4, "y": 4, "z": 4},
                   "placement": {"origin": [10, 3, 0]}}),
            json!({"type": "boolean", "id": "u", "op": "unite", "target": "a", "tools": ["lug"]}),
            json!({"type": "sphere", "id": "ball", "params": {"radius": 3},
                   "placement": {"origin": [30, 0, 0]}}),
        ] {
            add(&mut s, "p", f);
        }
        let (v, bytes) = exported(
            &mut s,
            json!({"part": "p", "time_stamp": "2026-10-27T00:00:00", "file_name": "p.step"}),
        )
        .unwrap();
        assert_eq!(
            (&v["schema"], &v["unit"], &v["rev"], &v["file_name"]),
            (
                &json!("ap242"),
                &json!("mm"),
                &json!("working"),
                &json!("p.step")
            )
        );
        assert_eq!(
            v["products"],
            json!([{"product": "p", "name": "p", "part": "p", "rev": "working",
                    "bodies": 2, "children": 0}])
        );
        assert_eq!(time_stamp(&bytes), "2026-10-27T00:00:00");
        // Each body named by its solid's founder (#412).
        let names: Vec<String> = opened(&bytes).held.iter().map(|h| h.name.clone()).collect();
        assert_eq!(names, ["a", "ball"]);
        // Imported again, each solid keeps its ID.
        let (_out, out) = outside(&bytes, "p.step");
        let (_dir2, mut s2) = fresh();
        let v = call(&mut s2, "import", json!({"solid": true, "path": out})).unwrap();
        let ids: Vec<&Value> = v["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| &f["id"])
            .collect();
        assert_eq!(ids, [&json!("a"), &json!("ball")], "{v}");
        // The united block and the ball, read back as two bodies of p; the
        // lug the boolean consumed isn't one.
        let opts = ryustep::ReadOptions {
            key: ryukei::name::OpKey(1),
            heal: Default::default(),
        };
        let read = ryustep::read(&bytes, &opts).unwrap();
        assert_eq!(read.bodies.len(), 2);
        assert!(read.bodies.iter().all(|b| b.result.is_ok()));
        let d = &read.products.definitions[0];
        assert_eq!((&*d.product_id, &*d.name, &*d.revision), ("p", "p", ""));
        // The same model and options, the same bytes.
        let (_, again) = exported(
            &mut s,
            json!({"part": "p", "time_stamp": "2026-10-27T00:00:00", "file_name": "p.step"}),
        )
        .unwrap();
        assert_eq!(again, bytes);
    }

    #[test]
    fn pdm_names_the_product() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        add(
            &mut s,
            "p",
            json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
        );
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {
                "number": "PN-7", "name": "Spacer", "description": "A spacer block"}}),
        )
        .unwrap();
        let (v, bytes) = exported(&mut s, json!({"part": "p", "unit": "m"})).unwrap();
        assert_eq!(v["products"][0]["product"], "PN-7");
        let d = &opened(&bytes).products.definitions[0];
        assert_eq!(
            (&*d.product_id, &*d.name, &*d.description),
            ("PN-7", "Spacer", "A spacer block")
        );
    }

    #[test]
    fn an_assembly_places_its_parts_where_it_solved_them() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(
            r,
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.0, 0.005],
        );
        add_in(
            r,
            "frame",
            "row",
            "instance_pattern",
            json!({"form": "linear", "instances": ["b1"], "direction": "x",
                   "spacing": "20 mm", "count": 3}),
        );
        s.refresh().unwrap();
        let (v, bytes) = exported(&mut s, json!({"part": "frame", "unit": "m"})).unwrap();
        let made: Vec<(&str, u64, u64)> = v["products"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["product"].as_str().unwrap(),
                    p["bodies"].as_u64().unwrap(),
                    p["children"].as_u64().unwrap(),
                )
            })
            .collect();
        // The bolt is one product, placed three times.
        assert_eq!(made, [("frame", 0, 4), ("plate", 1, 0), ("bolt", 1, 0)]);
        let o = opened(&bytes);
        let p = &o.products;
        assert_eq!(p.roots.len(), 1);
        let root = p.roots[0];
        let ids: Vec<&str> = p.definitions[root]
            .children
            .iter()
            .map(|c| &*c.id)
            .collect();
        assert_eq!(ids, ["base", "b1", "row:1", "row:2"]);
        // Each where the rebuild put it, bit for bit in meters.
        let frame = s.model(&Id::parse("frame", "part").unwrap(), &Rev::Working);
        let frame = frame.unwrap();
        for (n, at) in [
            (1, [0.01, 0.0, 0.005]),
            (2, [0.03, 0.0, 0.005]),
            (3, [0.05, 0.0, 0.005]),
        ] {
            let t = o.placement(root, n).unwrap().translation_part();
            assert_eq!([t.x, t.y, t.z], at, "{n}");
        }
        assert_eq!(frame.occurrences.len(), 4);
    }

    #[test]
    fn releases_and_revisions_export_as_their_commits_have_them() {
        let (dir, mut s) = released();
        let r = dir.path();
        // HEAD holds release A of the bolt; the working bolt is thicker.
        let (v, at_label) = exported(&mut s, json!({"part": "bolt", "rev": "label:A"})).unwrap();
        let commit = v["commit"].as_str().unwrap().to_owned();
        assert_eq!(v["products"][0]["revision"], "A");
        let d = &opened(&at_label).products.definitions[0];
        assert_eq!((&*d.product_id, &*d.revision), ("100", "A"));
        // The time stamp is the commit's.
        let repo = crate::git::repo(r).unwrap();
        let when = utc(repo.commit_time(&commit).unwrap());
        assert_eq!(time_stamp(&at_label), when);
        // HEAD's bolt is release A's, with no revision written.
        let (v, at_head) = exported(&mut s, json!({"part": "bolt", "rev": "HEAD"})).unwrap();
        assert_eq!(v["commit"], json!(commit));
        let text = String::from_utf8(at_label.clone()).unwrap();
        let unlabelled = text.replace(
            "PRODUCT_DEFINITION_FORMATION('A'",
            "PRODUCT_DEFINITION_FORMATION(''",
        );
        assert_eq!(unlabelled.as_bytes(), at_head);
        let (_, working) = exported(&mut s, json!({"part": "bolt", "time_stamp": when})).unwrap();
        assert_ne!(working, at_head);
        // An assembly placing release A writes that product with its
        // revision, beside the working bolt: two products.
        place_in(r, "frame", "a", "bolt", json!({"label": "A"}), [0.0; 3]);
        place_in(r, "frame", "w", "bolt", json!("working"), [0.02, 0.0, 0.0]);
        s.refresh().unwrap();
        let (v, _) = exported(&mut s, json!({"part": "frame"})).unwrap();
        let revs: Vec<(&Value, &Value)> = v["products"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (&p["product"], &p["rev"]))
            .collect();
        assert_eq!(
            revs,
            [
                (&json!("frame"), &json!("working")),
                (&json!("100"), &json!({"label": "A"})),
                (&json!("100"), &json!("working")),
            ]
        );
        let e = exported(&mut s, json!({"part": "bolt", "rev": "label:B"})).unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
    }

    #[test]
    fn what_fails_anywhere_refuses_the_export() {
        let (dir, mut s) = project();
        add(
            &mut s,
            "bolt",
            json!({"type": "box", "id": "miss", "operation": "cut", "targets": ["shank"],
                   "params": {"x": 1, "y": 1, "z": 1}, "placement": {"origin": [100, 100, 100]}}),
        );
        let e = exported(&mut s, json!({"part": "bolt"})).unwrap_err();
        assert_eq!(e.code, Code::ExportFailed);
        assert_eq!(
            e.details["failing"],
            json!([{"part": "bolt", "rev": "working", "feature": "miss",
                    "code": "operation_no_effect",
                    "message": e.details["failing"][0]["message"]}])
        );
        place_in(dir.path(), "frame", "b", "bolt", json!("working"), [0.0; 3]);
        s.refresh().unwrap();
        let e = exported(&mut s, json!({"part": "frame"})).unwrap_err();
        assert_eq!(e.code, Code::ExportFailed);
        assert_eq!(e.details["failing"][0]["feature"], "miss");
        // Skipping what fails writes the rest: the bolt as its chain
        // built it, and the frame placing it, with what was left out.
        for part in ["bolt", "frame"] {
            let (v, bytes) = exported(&mut s, json!({"part": part, "skip_failing": true})).unwrap();
            assert_eq!(v["skipped"][0]["feature"], "miss");
            assert_eq!(v["skipped"][0]["code"], "operation_no_effect");
            assert_eq!(v["warnings"][0]["code"], "export_skipped");
            assert_eq!(v["warnings"][0]["feature"], "miss");
            let bolt = v["products"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["part"] == "bolt")
                .unwrap();
            assert_eq!(bolt["bodies"], 1);
            assert!(!opened(&bytes).held.is_empty());
        }
        let (v, _) = exported(
            &mut s,
            json!({"part": "frame", "format": "stl", "skip_failing": true}),
        )
        .unwrap();
        assert_eq!(v["warnings"].as_array().unwrap().len(), 1);
        assert!(v["products"][0]["triangles"].as_u64().is_some());
    }

    #[test]
    fn skipping_what_fails_leaves_out_a_solid_that_never_built() {
        let (_dir, mut s) = project();
        // A second solid whose founder fails: a box past the size box.
        add(
            &mut s,
            "bolt",
            json!({"type": "box", "id": "lump", "params": {"x": 20000, "y": 1, "z": 1},
                   "placement": {"origin": [490000, 0, 0]}}),
        );
        let e = exported(&mut s, json!({"part": "bolt"})).unwrap_err();
        assert_eq!(e.code, Code::ExportFailed);
        assert_eq!(e.details["failing"][0]["feature"], "lump");
        let (v, bytes) = exported(&mut s, json!({"part": "bolt", "skip_failing": true})).unwrap();
        assert_eq!(v["products"][0]["bodies"], 1);
        assert_eq!(v["skipped"][0]["feature"], "lump");
        assert_eq!(opened(&bytes).held.len(), 1);
        // Nothing failing: no skipped list and no warnings.
        call(
            &mut s,
            "feature.delete",
            json!({"part": "bolt", "id": "lump"}),
        )
        .unwrap();
        let (v, _) = exported(&mut s, json!({"part": "bolt", "skip_failing": true})).unwrap();
        assert!(v.get("skipped").is_none() && v.get("warnings").is_none());
    }

    /// An export cancelled at any boundary either answers `cancelled`
    /// and leaves no file, or (cancelled once it began writing) writes
    /// the file whole and answers `ok` with `cancel_ignored`: never
    /// `cancelled` with the file written (#487).
    #[test]
    fn a_cancelled_export_leaves_no_file_or_finishes_it() {
        use ryu_session::job::Job;
        let (dir, s) = project();
        let root = dir.path().to_path_buf();
        let mut slot = Some(s);
        let to = tempfile::tempdir().unwrap();
        let mut outcomes = Vec::new();
        for n in 1.. {
            let path = to.path().join(format!("bolt-{n}.stl"));
            let req = crate::proto::Request::from_rpc(
                "export",
                Some(json!({"part": "bolt", "out": path, "chord": "0.05 mm"})),
            )
            .unwrap();
            // A fresh mesh cache each time, so every boundary is reached.
            *slot.as_mut().unwrap().caches_mut().meshes_mut() = Default::default();
            match crate::server::handle_job(
                &mut slot,
                None,
                None,
                &root,
                req,
                &Job::cancelled_at(n),
            ) {
                Err(e) => {
                    assert_eq!(e.code, Code::Cancelled, "{e:?}");
                    assert!(
                        !path.exists(),
                        "cancelled at {n}, yet {} is there",
                        path.display()
                    );
                    let left: Vec<_> = std::fs::read_dir(to.path()).unwrap().collect();
                    // Cancels come before any write: nothing, not even a
                    // temporary file, is there yet.
                    assert!(left.is_empty(), "left: {left:?}");
                    outcomes.push("cancelled");
                }
                Ok(a) => {
                    let bytes = std::fs::read(&path).unwrap();
                    assert_eq!(a.result["bytes"], json!(bytes.len()));
                    if a.result.get("cancel_ignored").is_none() {
                        break;
                    }
                    outcomes.push("ignored");
                }
            }
            assert!(n < 1000, "the export never ran out of boundaries");
        }
        // The cancel stops it before it writes, and is ignored once it does.
        assert_eq!(outcomes.first(), Some(&"cancelled"), "{outcomes:?}");
        assert_eq!(outcomes.last(), Some(&"ignored"), "{outcomes:?}");
    }

    #[test]
    fn the_daemon_writes_out() {
        let (_dir, mut s) = project();
        let to = tempfile::tempdir().unwrap();
        let path = to.path().join("bolt.step");
        let stamp = "2026-10-28T00:00:00";
        let a = answered(
            &mut s,
            json!({"part": "bolt", "out": path, "time_stamp": stamp}),
        )
        .unwrap();
        // Written whole, nothing after the answer.
        assert!(a.binary.is_none());
        let bytes = std::fs::read(&path).unwrap();
        assert!(!to.path().join(".bolt.step.tmp").exists());
        let v = &a.result;
        assert_eq!(
            (&v["out"], &v["bytes"], &v["blake3"], &v["format"]),
            (
                &json!(path),
                &json!(bytes.len()),
                &json!(blake3::hash(&bytes).to_hex().to_string()),
                &json!("step")
            )
        );
        // The bytes an answer carries, the header naming the file.
        assert_eq!(v["file_name"], "bolt.step");
        let (_, back) = exported(
            &mut s,
            json!({"part": "bolt", "time_stamp": stamp, "file_name": "bolt.step"}),
        )
        .unwrap();
        assert_eq!(back, bytes);
        // The format from the extension, in any case, or as given.
        let stp = to.path().join("BOLT.STP");
        let v = answered(
            &mut s,
            json!({"part": "bolt", "out": stp, "time_stamp": stamp}),
        )
        .unwrap()
        .result;
        assert_eq!(v["format"], "step");
        let any = to.path().join("bolt.out");
        let v = answered(
            &mut s,
            json!({"part": "bolt", "out": any, "format": "step", "time_stamp": stamp}),
        )
        .unwrap()
        .result;
        assert_eq!(v["format"], "step");
        // Refused before anything is written.
        let missing = to.path().join("nowhere/bolt.step");
        for (params, code, param) in [
            (
                json!({"part": "bolt", "out": "bolt.step"}),
                Code::InvalidParam,
                json!("out"),
            ),
            (
                json!({"part": "bolt", "out": any}),
                Code::InvalidParam,
                json!("format"),
            ),
            (
                json!({"part": "bolt", "format": "obj"}),
                Code::InvalidParam,
                json!("format"),
            ),
            (
                json!({"part": "bolt", "out": missing}),
                Code::NotFound,
                Value::Null,
            ),
        ] {
            let e = answered(&mut s, params.clone()).unwrap_err();
            assert_eq!((e.code, &e.details["param"]), (code, &param), "{params}");
        }
        assert!(!missing.parent().unwrap().exists());
    }

    /// A binary STL's triangles, each its three vertices.
    fn stl_triangles(bytes: &[u8]) -> Vec<[[f32; 3]; 3]> {
        let n = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 84 + 50 * n);
        let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        (0..n)
            .map(|t| [0, 1, 2].map(|v| [0, 1, 2].map(|k| f(84 + 50 * t + 12 + 12 * v + 4 * k))))
            .collect()
    }

    /// An L base flange `bl` in part `p`, 20 mm deep: runs of 40 and 30 mm
    /// on the sketch, 1.5 mm thick, 1 mm inside, K 0.44.
    fn l_bracket(s: &mut Session) {
        add(s, "p", json!({"type": "sketch", "id": "l", "plane": "xy"}));
        let mut ops = Vec::new();
        for (i, at) in [[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]]
            .into_iter()
            .enumerate()
        {
            ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
            ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
        }
        for i in 1..=2 {
            ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                            "start": format!("p{i}"), "end": format!("p{}", i + 1)}));
        }
        crate::server::handle(
            s,
            crate::proto::Request::from_rpc(
                "sketch.edit",
                Some(json!({"part": "p", "sketch": "l", "ops": ops})),
            )
            .unwrap(),
        )
        .unwrap();
        add(
            s,
            "p",
            json!({"type": "base_flange", "id": "bl", "sketch": "l", "thickness": "1.5 mm",
                           "radius": "1 mm", "allowance": {"k_factor": "0.44"}, "depth": "20 mm"}),
        );
    }

    #[test]
    fn a_sheet_metal_part_exports_its_flat_as_dxf() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        l_bracket(&mut s);
        let (v, bytes) = exported(&mut s, json!({"part": "p", "format": "dxf"})).unwrap();
        assert_eq!(
            (
                v["format"].as_str(),
                v["solid"].as_str(),
                v["unit"].as_str()
            ),
            (Some("dxf"), Some("bl"), Some("mm"))
        );
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains("AC1015") && text.contains("BEND_NOTES") && text.contains("K0.44"));
        // The same flat as flat.get's, in inches without notes, by extension.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("l.DXF");
        let v = answered(
            &mut s,
            json!({"part": "p", "out": out, "unit": "in", "notes": false}),
        )
        .unwrap()
        .result;
        assert_eq!(v["format"], "dxf");
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(
            !text.contains("K0.44") && text.contains("$INSUNITS\n 70\n1\n"),
            "{text}"
        );
        // Twice, the same bytes.
        assert_eq!(
            exported(&mut s, json!({"part": "p", "format": "dxf"}))
                .unwrap()
                .1,
            bytes
        );
    }

    #[test]
    fn a_dxf_refuses_what_it_cant_write() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        l_bracket(&mut s);
        for (params, param) in [
            (json!({"part": "p", "format": "dxf", "unit": "cm"}), "unit"),
            (
                json!({"part": "p", "format": "dxf", "chord": "0.1 mm"}),
                "chord",
            ),
            (
                json!({"part": "p", "format": "stl", "notes": false}),
                "notes",
            ),
            (
                json!({"part": "p", "format": "step", "solid": "bl"}),
                "solid",
            ),
            (
                json!({"part": "p", "format": "dxf", "rev": "label:A"}),
                "rev",
            ),
        ] {
            let e = exported(&mut s, params.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["param"].as_str()),
                (Code::InvalidParam, Some(param)),
                "{params}: {e}"
            );
        }
    }

    #[test]
    fn a_part_exports_as_stl() {
        let (_dir, mut s) = project();
        let (v, bytes) = exported(&mut s, json!({"part": "plate", "format": "stl"})).unwrap();
        assert_eq!(
            (
                &v["format"],
                &v["unit"],
                &v["ascii"],
                &v["mesh"]["chord"]["value"]
            ),
            (
                &json!("stl"),
                &json!("mm"),
                &json!(false),
                &json!("0.01 mm")
            )
        );
        assert!(v.get("schema").is_none() && v.get("file_name").is_none());
        assert_eq!(v["products"][0]["triangles"], 12);
        assert!(bytes.starts_with(b"ryucad 0.0.0 plate mm "));
        let t = stl_triangles(&bytes);
        assert_eq!(t.len(), 12);
        let most = t
            .iter()
            .flatten()
            .fold([0f32; 3], |m, p| [0, 1, 2].map(|k| m[k].max(p[k])));
        assert_eq!(most, [100.0, 50.0, 5.0]);
        // In centimetres, ASCII, one block named for its solid.
        let (_, ascii) = exported(
            &mut s,
            json!({"part": "plate", "format": "stl", "unit": "cm", "ascii": true}),
        )
        .unwrap();
        let text = String::from_utf8(ascii).unwrap();
        assert!(text.starts_with("solid plate/slab\n"), "{text}");
        assert!(text.contains("vertex 10 5 0.5\n"), "{text}");
        // The same model and options, the same bytes.
        let (_, again) = exported(&mut s, json!({"part": "plate", "format": "stl"})).unwrap();
        assert_eq!(again, bytes);
    }

    #[test]
    fn an_assembly_exports_every_occurrence_as_one_stl() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(
            r,
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.0, 0.005],
        );
        add_in(
            r,
            "frame",
            "row",
            "instance_pattern",
            json!({"form": "linear", "instances": ["b1"], "direction": "x",
                   "spacing": "20 mm", "count": 3}),
        );
        s.refresh().unwrap();
        let (v, bytes) = exported(
            &mut s,
            json!({"part": "frame", "format": "stl", "chord": "0.05 mm"}),
        )
        .unwrap();
        let own: Vec<u64> = v["products"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["triangles"].as_u64().unwrap())
            .collect();
        // The frame holds no solid of its own; the bolt, meshed once, is
        // placed three times.
        assert_eq!(own[0], 0);
        assert_eq!(stl_triangles(&bytes).len() as u64, own[1] + 3 * own[2]);
        let (_, ascii) = exported(
            &mut s,
            json!({"part": "frame", "format": "stl", "chord": "0.05 mm", "ascii": true}),
        )
        .unwrap();
        let text = String::from_utf8(ascii).unwrap();
        let solids: Vec<&str> = text.lines().filter(|l| l.starts_with("solid ")).collect();
        assert_eq!(
            solids,
            [
                "solid frame/base/slab",
                "solid frame/b1/shank",
                "solid frame/row:1/shank",
                "solid frame/row:2/shank"
            ]
        );
    }

    /// A 3MF package's model.
    fn model_of(bytes: &[u8]) -> String {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut model = String::new();
        std::io::Read::read_to_string(&mut z.by_name("3D/3dmodel.model").unwrap(), &mut model)
            .unwrap();
        model
    }

    #[test]
    fn an_assembly_exports_as_3mf_objects() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(
            r,
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.0, 0.005],
        );
        place_in(
            r,
            "frame",
            "b2",
            "bolt",
            json!("working"),
            [0.03, 0.0, 0.005],
        );
        s.refresh().unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "bolt", "properties": {"number": "B-6", "name": "Bolt"}}),
        )
        .unwrap();
        let stamp = "2026-10-28T00:00:00";
        let (v, bytes) = exported(
            &mut s,
            json!({"part": "frame", "format": "3mf", "chord": "0.05 mm", "time_stamp": stamp}),
        )
        .unwrap();
        assert_eq!(
            (&v["format"], &v["unit"], &v["time_stamp"]),
            (&json!("3mf"), &json!("mm"), &json!(stamp))
        );
        let model = model_of(&bytes);
        assert!(model.contains("<model unit=\"millimeter\""), "{model}");
        for meta in [
            "<metadata name=\"Title\">frame</metadata>",
            "<metadata name=\"CreationDate\">2026-10-28T00:00:00</metadata>",
            "<metadata name=\"ryucad:chord\">0.05 mm</metadata>",
            "<metadata name=\"ryucad:angle\">15 deg</metadata>",
        ] {
            assert!(model.contains(meta), "{meta}");
        }
        // The plate, the bolt once (its PDM number and name), then the
        // frame placing the bolt twice, 20 mm apart.
        let objects: Vec<&str> = model
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("<object"))
            .collect();
        assert_eq!(
            objects,
            [
                "<object id=\"1\" type=\"model\" name=\"plate\" partnumber=\"plate\">",
                "<object id=\"2\" type=\"model\" name=\"Bolt\" partnumber=\"B-6\">",
                "<object id=\"3\" type=\"model\" name=\"frame\" partnumber=\"frame\">",
            ]
        );
        assert!(
            model.contains("<component objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 30 0 5\"/>")
        );
        assert!(model.contains("<item objectid=\"3\"/>"));
        // The same model and options, the same bytes.
        let (_, again) = exported(
            &mut s,
            json!({"part": "frame", "format": "3mf", "chord": "0.05 mm", "time_stamp": stamp}),
        )
        .unwrap();
        assert_eq!(again, bytes);
        let e = answered(
            &mut s,
            json!({"part": "frame", "format": "3mf", "ascii": true}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::InvalidParam, &json!("ascii"))
        );
    }

    #[test]
    fn a_part_exports_as_usda() {
        let (_dir, mut s) = project();
        let stamp = "2026-10-29T00:00:00";
        let ask = json!({"part": "bolt", "format": "usda", "time_stamp": stamp});
        let (v, bytes) = exported(&mut s, ask.clone()).unwrap();
        assert_eq!(
            (
                &v["format"],
                &v["unit"],
                &v["brep"],
                &v["mesh"]["chord"]["value"],
                &v["pipe_tolerance"]["value"],
                &v["time_stamp"],
            ),
            (
                &json!("usda"),
                &json!("mm"),
                &json!(true),
                &json!("0.01 mm"),
                &json!("0.0001 mm"),
                &json!(stamp)
            )
        );
        assert!(v["products"][0]["triangles"].as_u64().unwrap() > 0);
        let text = String::from_utf8(bytes.clone()).unwrap();
        // A part alone: no prototypes, its solid with its B-rep and mesh.
        for want in [
            "#usda 1.0\n",
            "defaultPrim = \"bolt\"",
            "metersPerUnit = 0.001",
            "upAxis = \"Z\"",
            "string time_stamp = \"2026-10-29T00:00:00\"",
            "string chord = \"0.01 mm\"",
            "string pipe_tolerance = \"0.0001 mm\"",
            "def Xform \"bolt\" (",
            "kind = \"component\"",
            "string reference = \"bolt/shank\"",
            "def BrepArray \"brep\" (",
            "uniform double[] brep:surface:cylinder:radius = [3]",
            "def Mesh \"mesh\"",
        ] {
            assert!(text.contains(want), "{want}");
        }
        assert!(!text.contains("Prototypes"));
        // The faces' names are the body's.
        let bolt = Id::parse("bolt", "part").unwrap();
        let mut walk = Walk::default();
        walk.product(&mut s, &bolt, &Rev::Working).unwrap();
        let body = &walk.scene.products[0].solids[0].built.body;
        for f in body.faces() {
            assert!(text.contains(&body.face(f).unwrap().name.to_string()));
        }
        // The same model and options, the same bytes.
        let (_, again) = exported(&mut s, ask).unwrap();
        assert_eq!(again, bytes);
        // Meshes only: the faces' names go on the mesh.
        let (v, bytes) = exported(
            &mut s,
            json!({"part": "bolt", "format": "usda", "brep": false}),
        )
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("BrepArray") && !text.contains("omni_solid"));
        assert!(text.contains("custom uniform string[] ryucad:faceName"));
        assert_eq!((&v["brep"], v.get("pipe_tolerance")), (&json!(false), None));
        // B-rep only.
        let (v, bytes) = exported(
            &mut s,
            json!({"part": "bolt", "format": "usda", "mesh": false, "unit": "m"}),
        )
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("def Mesh") && text.contains("metersPerUnit = 1\n"));
        assert!(v.get("mesh").is_none() && v["products"][0].get("triangles").is_none());
    }

    #[test]
    fn a_part_exports_as_usdc_and_usdz() {
        let (dir, mut s) = project();
        let stamp = "2026-10-29T00:00:00";
        let ask = |format: &str| json!({"part": "bolt", "format": format, "time_stamp": stamp});
        let (v, usdc) = exported(&mut s, ask("usdc")).unwrap();
        assert_eq!((&v["format"], &v["brep"]), (&json!("usdc"), &json!(true)));
        assert!(v["products"][0]["triangles"].as_u64().unwrap() > 0);
        assert_eq!(&usdc[..8], b"PXR-USDC");
        // The same layer as the usda: smaller, the same every time.
        let (_, usda) = exported(&mut s, ask("usda")).unwrap();
        assert!(
            usdc.len() < usda.len(),
            "{} against {}",
            usdc.len(),
            usda.len()
        );
        assert_eq!(exported(&mut s, ask("usdc")).unwrap().1, usdc);
        // A usdz is the usdc, stored as `<part>.usdc` at a 64-byte offset:
        // a local header (30 bytes), the name and padding.
        let (v, usdz) = exported(&mut s, ask("usdz")).unwrap();
        assert_eq!(v["format"], "usdz");
        assert_eq!(&usdz[..4], b"PK\x03\x04");
        assert_eq!(&usdz[8..10], &[0, 0], "stored");
        assert_eq!(&usdz[30..39], b"bolt.usdc");
        assert_eq!(&usdz[64..64 + usdc.len()], &usdc[..]);
        // Written to a file whatever its name: `.usd` is usdc, and the
        // usdz holds `bolt.usdc` wherever it goes.
        let out = dir.path().join("elsewhere.usd");
        let a = answered(
            &mut s,
            json!({"part": "bolt", "out": out, "time_stamp": stamp}),
        )
        .unwrap();
        assert_eq!(a.result["format"], "usdc");
        assert_eq!(std::fs::read(&out).unwrap(), usdc);
        let out = dir.path().join("other.usdz");
        answered(
            &mut s,
            json!({"part": "bolt", "out": out, "time_stamp": stamp}),
        )
        .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), usdz);
    }

    #[test]
    fn an_assembly_exports_as_instanced_usda() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(
            r,
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.0, 0.005],
        );
        place_in(
            r,
            "frame",
            "b2",
            "bolt",
            json!("working"),
            [0.03, 0.0, 0.005],
        );
        s.refresh().unwrap();
        let (_, bytes) = exported(
            &mut s,
            json!({"part": "frame", "format": "usda", "mesh": false}),
        )
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        for want in [
            "class \"Prototypes\"\n",
            "class Xform \"plate\" (",
            "class Xform \"bolt\" (",
            "def Xform \"frame\" (",
            "kind = \"assembly\"",
            "def Xform \"b2\" (",
            "instanceable = true",
            "prepend references = </Prototypes/bolt>",
            "matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (30, 0, 5, 1) )",
            "string instance = \"b1\"",
        ] {
            assert!(text.contains(want), "{want}");
        }
        // Each part defined once, however many times it is placed.
        assert_eq!(text.matches("def BrepArray").count(), 2);
    }

    #[test]
    fn usd_options_are_checked() {
        let (_dir, mut s) = project();
        for (params, param) in [
            (
                json!({"part": "bolt", "format": "usda", "ascii": true}),
                "ascii",
            ),
            (
                json!({"part": "bolt", "format": "usda", "schema": "ap242"}),
                "schema",
            ),
            (
                json!({"part": "bolt", "format": "usda", "brep": false, "mesh": false}),
                "mesh",
            ),
            (
                json!({"part": "bolt", "format": "usda", "mesh": false, "chord": "1 mm"}),
                "chord",
            ),
            (
                json!({"part": "bolt", "format": "usda", "brep": false, "pipe_tolerance": "0.001 mm"}),
                "pipe_tolerance",
            ),
            (
                json!({"part": "bolt", "format": "usda", "pipe_tolerance": "0 mm"}),
                "pipe_tolerance",
            ),
            (
                json!({"part": "bolt", "format": "usda", "unit": "in"}),
                "unit",
            ),
            (
                json!({"part": "bolt", "format": "stl", "brep": false}),
                "brep",
            ),
            (json!({"part": "bolt", "mesh": true}), "mesh"),
            (
                json!({"part": "bolt", "format": "3mf", "pipe_tolerance": "0.01 mm"}),
                "pipe_tolerance",
            ),
        ] {
            let e = answered(&mut s, params.clone()).unwrap_err();
            assert_eq!(
                (e.code, &e.details["param"]),
                (Code::InvalidParam, &json!(param)),
                "{params}"
            );
        }
        assert_eq!(Format::of_path("/x/a.USDA"), Some(Format::Usda));
        assert_eq!(Format::of_path("/x/a.usdc"), Some(Format::Usdc));
        assert_eq!(Format::of_path("/x/a.Usd"), Some(Format::Usdc));
        assert_eq!(Format::of_path("/x/a.usdz"), Some(Format::Usdz));
        // The binary encodings take USD's options, and refuse the rest.
        for (params, param) in [
            (
                json!({"part": "bolt", "format": "usdc", "ascii": true}),
                "ascii",
            ),
            (
                json!({"part": "bolt", "format": "usdz", "schema": "ap242"}),
                "schema",
            ),
            (
                json!({"part": "bolt", "format": "usdc", "brep": false, "mesh": false}),
                "mesh",
            ),
        ] {
            let e = answered(&mut s, params.clone()).unwrap_err();
            assert_eq!(
                (e.code, &e.details["param"]),
                (Code::InvalidParam, &json!(param)),
                "{params}"
            );
        }
    }

    #[test]
    fn a_face_usd_cant_describe_is_named() {
        let (_dir, mut s) = project();
        let bolt = Id::parse("bolt", "part").unwrap();
        let mut walk = Walk::default();
        walk.product(&mut s, &bolt, &Rev::Working).unwrap();
        let body = &walk.scene.products[0].solids[0].built.body;
        let face = body.face(body.faces().next().unwrap()).unwrap().name;
        let e = walk.usd_unrepresentable(ryuformats::usd::stage::StageError::Brep {
            product: 0,
            solid: 0,
            error: ryuformats::usd::brep::BrepError {
                face,
                reason: "seams",
            },
        });
        assert_eq!(e.code, Code::ExportUnrepresentable);
        assert_eq!(
            (
                &e.details["entity"],
                &e.details["reason"],
                &e.details["solid"]
            ),
            (
                &json!(format!("bolt/shank/{face}")),
                &json!("seams"),
                &json!("shank")
            )
        );
    }

    #[test]
    fn mesh_options_belong_to_mesh_formats() {
        let (_dir, mut s) = project();
        for (params, param) in [
            (json!({"part": "bolt", "chord": "0.1 mm"}), "chord"),
            (json!({"part": "bolt", "ascii": true}), "ascii"),
            (json!({"part": "bolt", "unit": "cm"}), "unit"),
            (
                json!({"part": "bolt", "format": "stl", "schema": "ap242"}),
                "schema",
            ),
            (
                json!({"part": "bolt", "format": "stl", "unit": "in"}),
                "unit",
            ),
            (
                json!({"part": "bolt", "format": "stl", "angle": "0 deg"}),
                "angle",
            ),
        ] {
            let e = answered(&mut s, params.clone()).unwrap_err();
            assert_eq!(
                (e.code, &e.details["param"]),
                (Code::InvalidParam, &json!(param)),
                "{params}"
            );
        }
    }

    #[test]
    fn a_body_past_the_budget_is_too_large() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "dome"})).unwrap();
        add(
            &mut s,
            "dome",
            json!({"type": "sphere", "id": "ball", "params": {"radius": "100 m"}}),
        );
        let e = answered(
            &mut s,
            json!({"part": "dome", "format": "stl", "chord": "0.001 mm"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ExportTooLarge, "{e:?}");
        assert_eq!(
            (&e.details["part"], &e.details["solid"]),
            (&json!("dome"), &json!("ball"))
        );
        assert!(e.details["triangles"].as_u64().unwrap() > 50_000_000);
    }

    #[test]
    fn faces_that_wont_mesh_are_named() {
        let (_dir, mut s) = project();
        let bolt = Id::parse("bolt", "part").unwrap();
        let mut walk = Walk::default();
        walk.product(&mut s, &bolt, &Rev::Working).unwrap();
        let solid = &walk.scene.products[0].solids[0];
        let body = &solid.built.body;
        let mut mesh = ryukei::mesh::tessellate(body, &MeshParams::new(1e-4, 0.5)).unwrap();
        let face = body.faces().next().unwrap();
        let edge = body.edges().next().unwrap();
        mesh.faults = vec![
            ryukei::mesh::FaceFault {
                face,
                kind: ryukei::mesh::MeshFaultKind::BadDomain,
            },
            ryukei::mesh::FaceFault {
                face,
                kind: ryukei::mesh::MeshFaultKind::EdgeOffSurface { edge },
            },
        ];
        let f = fault_report(solid, &Rev::Working, &mesh);
        let name = |n: ryukei::name::Name| format!("bolt/shank/{n}");
        assert_eq!(
            f,
            [
                json!({"part": "bolt", "rev": "working", "solid": "shank",
                       "fault": "bad_domain", "face": name(body.face(face).unwrap().name)}),
                json!({"part": "bolt", "rev": "working", "solid": "shank",
                       "fault": "edge_off_surface", "face": name(body.face(face).unwrap().name),
                       "edge": name(body.edge(edge).unwrap().name)}),
            ]
        );
    }

    #[test]
    fn options_and_parts_are_checked() {
        let (_dir, mut s) = project();
        for (params, code, param) in [
            (
                json!({"part": "bolt", "schema": "ap203"}),
                Code::InvalidParam,
                "schema",
            ),
            (
                json!({"part": "bolt", "unit": "in"}),
                Code::InvalidParam,
                "unit",
            ),
        ] {
            let e = exported(&mut s, params).unwrap_err();
            assert_eq!((e.code, &e.details["param"]), (code, &json!(param)));
        }
        let e = exported(&mut s, json!({"part": "nut"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = exported(&mut s, json!({"part": "bolt", "rev": "HEAD"})).unwrap_err();
        assert_eq!(e.code, Code::NotAGitRepo);
        let (v, bytes) = exported(&mut s, json!({"part": "bolt", "schema": "ap214"})).unwrap();
        assert_eq!(v["schema"], "ap214");
        assert_eq!(opened(&bytes).schema, Schema::Ap214);
    }

    /// A frame (`F-1`) with a plate of its own, a sub-assembly (`S-1`) of
    /// two pins (`P-1`) and one pin more, every usage `1`: one root.
    fn frame() -> Vec<u8> {
        use ryukei::geom::{Rigid, Vector3};
        use ryukei::name::OpKey;
        use ryukei::primitive;
        let at = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
        let k = OpKey(1);
        let plate = primitive::block(k, 0.1, 0.1, 0.01, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let pin = primitive::cylinder(k, 0.002, 0.02, &Rigid::IDENTITY)
            .unwrap()
            .0;
        // Each body named as ryucad names its import feature (#412).
        let product = |id: &str, name: &str, bodies, children| ryustep::Product {
            id: id.into(),
            name: name.into(),
            bodies,
            names: vec!["body".into()],
            children,
            ..Default::default()
        };
        let child = |product, place| ryustep::Child {
            product,
            place,
            id: "1".into(),
        };
        crate::import::tests::written(vec![
            product(
                "F-1",
                "Frame",
                vec![plate],
                vec![child(1, at(0.0, 0.0, 0.01)), child(2, at(0.05, 0.05, 0.01))],
            ),
            product(
                "S-1",
                "Sub",
                vec![],
                vec![child(2, at(0.01, 0.0, 0.0)), child(2, at(0.02, 0.0, 0.0))],
            ),
            product("P-1", "Pin", vec![pin], vec![]),
        ])
    }

    /// `bytes` written to `name` outside any project.
    fn outside(bytes: &[u8], name: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        (dir, path.to_string_lossy().into_owned())
    }

    /// Each definition's product, own bodies, and usages (id, child's
    /// product, the placement's bits), in definition order.
    #[allow(clippy::type_complexity)]
    fn tree(bytes: &[u8]) -> Vec<(String, usize, Vec<(String, String, Vec<u64>)>)> {
        let o = opened(bytes);
        let defs = &o.products.definitions;
        (0..defs.len())
            .map(|k| {
                let d = &defs[k];
                let bodies = o.held.iter().filter(|h| h.definition == Some(k)).count();
                let usages = d
                    .children
                    .iter()
                    .enumerate()
                    .map(|(n, c)| {
                        let r = o.placement(k, n).unwrap();
                        let t = r.translation_part();
                        let bits = r
                            .rotation_matrix()
                            .iter()
                            .flatten()
                            .chain([t.x, t.y, t.z].iter())
                            .map(|x| x.to_bits())
                            .collect();
                        (
                            c.id.to_string(),
                            defs[c.definition].product_id.to_string(),
                            bits,
                        )
                    })
                    .collect();
                (format!("{} {}", d.product_id, d.name), bodies, usages)
            })
            .collect()
    }

    /// Every face, edge and vertex name of every part's solids, by part.
    fn names(s: &mut Session) -> BTreeMap<String, BTreeSet<String>> {
        let parts: Vec<Id> = s.project().parts.keys().cloned().collect();
        let mut out = BTreeMap::new();
        for p in parts {
            let v = call(s, "inspect", json!({"part": p})).unwrap();
            let mut set = BTreeSet::new();
            for b in v["bodies"].as_array().into_iter().flatten() {
                for kind in ["faces", "edges", "vertices"] {
                    for e in b[kind].as_array().into_iter().flatten() {
                        set.insert(e["name"].as_str().unwrap().to_owned());
                    }
                }
            }
            out.insert(p.to_string(), set);
        }
        out
    }

    /// [`frame`], painted as ryucad paints: the plate red with one face
    /// blue, the pin translucent, the sub-assembly's second pin green in
    /// that occurrence.
    /// `painted_frame`'s model, and the plate's face it paints.
    fn painted_frame_model() -> (ryustep::WriteModel, ryukei::name::Name) {
        use ryukei::geom::{Rigid, Vector3};
        use ryukei::name::OpKey;
        use ryukei::primitive;
        let at = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
        let k = OpKey(1);
        let plate = primitive::block(k, 0.1, 0.1, 0.01, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let pin = primitive::cylinder(k, 0.002, 0.02, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let face = plate
            .faces()
            .map(|f| plate.face(f).unwrap().name)
            .min()
            .unwrap();
        let product = |id: &str, name: &str, bodies, children| ryustep::Product {
            id: id.into(),
            name: name.into(),
            bodies,
            names: vec!["body".into()],
            children,
            ..Default::default()
        };
        let child = |product, place| ryustep::Child {
            product,
            place,
            id: "1".into(),
        };
        let model = ryustep::WriteModel {
            products: vec![
                product(
                    "F-1",
                    "Frame",
                    vec![plate],
                    vec![child(1, at(0.0, 0.0, 0.01)), child(2, at(0.05, 0.05, 0.01))],
                ),
                product(
                    "S-1",
                    "Sub",
                    vec![],
                    vec![child(2, at(0.01, 0.0, 0.0)), child(2, at(0.02, 0.0, 0.0))],
                ),
                product("P-1", "Pin", vec![pin], vec![]),
            ],
        };
        (model, face)
    }

    fn painted_frame() -> Vec<u8> {
        let (model, face) = painted_frame_model();
        let p = |c: [u8; 3], o: f64| ryustep::Paint {
            colour: c,
            opacity: o,
        };
        let paints = ryustep::Paints {
            bodies: vec![
                ryustep::BodyPaint {
                    product: 0,
                    body: 0,
                    base: Some(p([255, 0, 0], 1.0)),
                    faces: vec![(face, p([0, 0, 255], 1.0))],
                },
                ryustep::BodyPaint {
                    product: 2,
                    body: 0,
                    base: Some(p([0, 0, 255], 0.5)),
                    faces: Vec::new(),
                },
            ],
            occurrences: vec![ryustep::OccurrencePaint {
                path: vec![(0, 0), (1, 1)],
                body: 0,
                paint: p([0, 255, 0], 1.0),
            }],
        };
        let opts = ryustep::WriteOptions {
            unit: ryustep::LengthUnit::Meter,
            file_name: "frame.step".into(),
            time_stamp: "2026-10-27T00:00:00".into(),
            ..Default::default()
        };
        ryustep::write_styled(&model, &[], &paints, &opts)
            .unwrap()
            .bytes
    }

    /// A painted file imported and exported with its header is the file,
    /// byte for byte: its styles came in as the file layer and went out
    /// as the same styles (design 448's fixpoint).
    #[test]
    fn a_painted_file_exports_as_it_came() {
        let original = painted_frame();
        let (_src, path) = outside(&original, "frame.step");
        let (_dir, mut s) = fresh();
        call(&mut s, "import", json!({"solid": true, "path": path})).unwrap();
        let params = json!({"part": "f-1", "unit": "m", "time_stamp": "2026-10-27T00:00:00",
                            "file_name": "frame.step"});
        let (_, bytes) = exported(&mut s, params).unwrap();
        assert!(bytes == original, "{}", first_difference(&original, &bytes));
    }

    /// Where two files' text first differs, line by line.
    fn first_difference(original: &[u8], bytes: &[u8]) -> String {
        let (a, b) = (
            String::from_utf8_lossy(original).into_owned(),
            String::from_utf8_lossy(bytes).into_owned(),
        );
        let first = a.lines().zip(b.lines()).position(|(x, y)| x != y);
        format!(
            "differs from line {first:?}:\n{}\n---\n{}",
            a.lines().nth(first.unwrap_or(0)).unwrap_or(""),
            b.lines().nth(first.unwrap_or(0)).unwrap_or("")
        )
    }

    #[test]
    fn an_imported_file_exports_as_it_came() {
        let original = frame();
        let (_src, path) = outside(&original, "frame.step");
        let (dir, mut s) = fresh();
        call(&mut s, "import", json!({"solid": true, "path": path})).unwrap();
        // f-1 places its plate's part (f-1-body), s-1 and p-1. Exported
        // with the header the file had, it is the file, byte for byte.
        let export = |s: &mut Session, time_stamp: &str| {
            let params = json!({"part": "f-1", "unit": "m", "time_stamp": time_stamp,
                                "file_name": "frame.step"});
            exported(s, params).unwrap().1
        };
        assert_eq!(export(&mut s, "2026-10-27T00:00:00"), original);
        // At another time, the same products, bodies and usages, each
        // where it was: the plate is the frame's own body again, not a part
        // placed in it.
        let bytes = export(&mut s, "2026-10-28T00:00:00");
        assert_eq!(tree(&bytes), tree(&original));
        // Updating the import with the export changes nothing but the
        // file's hash, and renames nothing.
        let before = crate::session::files(dir.path()).unwrap();
        let read = |p: &std::path::Path| std::fs::read(p).unwrap();
        let old: BTreeMap<_, _> = before.iter().map(|p| (p.clone(), read(p))).collect();
        let names_before = names(&mut s);
        let (_out, out) = outside(&bytes, "export.step");
        let v = call(
            &mut s,
            "import",
            json!({"solid": true, "path": out, "update": "imports/frame.step"}),
        )
        .unwrap();
        assert_ne!(v["file"]["blake3"], v["file"]["from"]);
        for key in ["vanished", "kept", "failing"] {
            assert!(v.get(key).is_none(), "{key}: {v}");
        }
        let after = crate::session::files(dir.path()).unwrap();
        assert_eq!(after, before);
        for p in &after {
            let (a, b) = (&old[p], read(p));
            if *a == b || p.ends_with("frame.step") {
                continue;
            }
            // An import feature, only its file's hash new.
            let mut a: Value = serde_json::from_slice(a).unwrap();
            let b: Value = serde_json::from_slice(&b).unwrap();
            assert_eq!(a["type"], "import", "{}", p.display());
            a["file"]["blake3"] = b["file"]["blake3"].clone();
            assert_eq!(a, b, "{}", p.display());
        }
        assert_eq!(names(&mut s), names_before);
        // Imported into another project, it makes the same parts.
        let (_dir2, mut s2) = fresh();
        let v = call(&mut s2, "import", json!({"solid": true, "path": out})).unwrap();
        let made: Vec<(&Value, &Value)> = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (&p["part"], &p["kind"]))
            .collect();
        assert_eq!(
            made,
            [
                (&json!("f-1"), &json!("assembly")),
                (&json!("f-1-body"), &json!("part")),
                (&json!("s-1"), &json!("assembly")),
                (&json!("p-1"), &json!("part")),
            ]
        );
    }

    #[test]
    fn a_face_step_cant_describe_is_named() {
        let (_dir, mut s) = project();
        let bolt = Id::parse("bolt", "part").unwrap();
        let mut walk = Walk::default();
        walk.product(&mut s, &bolt, &Rev::Working).unwrap();
        let body = &walk.scene.products[0].solids[0].built.body;
        let face = body.face(body.faces().next().unwrap()).unwrap().name;
        let e = walk.unwritable(WriteError::NotRepresentable { face });
        assert_eq!(e.code, Code::ExportUnrepresentable);
        assert_eq!(e.details["entity"], format!("bolt/shank/{face}"));
    }

    /// A part's looks are written as styles: the part's colour on each
    /// body, a face's own, a solid's opacity (design 448).
    #[test]
    fn a_parts_looks_are_written_as_styles() {
        let (_dir, mut s) = crate::appearance::tests::project();
        let set = |s: &mut Session, e: &str, v: Value| {
            let mut v = v;
            v["entities"] = json!([e]);
            call(s, "appearance.set", v).unwrap();
        };
        set(&mut s, "plate", json!({"colour": "#d03030"}));
        set(&mut s, "plate/boss", json!({"opacity": 0.5}));
        let top = crate::appearance::tests::face_names(&mut s, "plate", "base")[0].clone();
        set(
            &mut s,
            &format!("plate/base/{top}"),
            json!({"colour": "#2050c0"}),
        );
        let (v, bytes) = exported(
            &mut s,
            json!({"part": "plate", "time_stamp": "2026-11-11T00:00:00"}),
        )
        .unwrap();
        let o = ryustep::open(&bytes).unwrap();
        let held = |n: &str| o.held.iter().position(|h| h.name == n).unwrap();
        let base = o.body_styles(held("base"));
        assert_eq!(base.body.unwrap().colour, Some([0xd0, 0x30, 0x30]));
        assert_eq!(base.faces.len(), 1);
        assert_eq!(
            base.faces.values().next().unwrap().colour,
            Some([0x20, 0x50, 0xc0])
        );
        let boss = o.body_styles(held("boss")).body.unwrap();
        assert_eq!(
            (boss.colour, boss.opacity),
            (Some([0xd0, 0x30, 0x30]), Some(0.5))
        );
        let written: Vec<&str> = v["transfer"]["written"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["category"].as_str().unwrap())
            .collect();
        assert!(written.contains(&"appearance"), "{written:?}");
        assert!(v.get("warnings").is_none(), "{v}");
    }

    /// An occurrence drawn otherwise than its part is an override in its
    /// usages; a face drawn otherwise only in an assembly is omitted.
    #[test]
    fn occurrences_drawn_otherwise_are_overrides() {
        let (_dir, mut s) = crate::appearance::tests::project();
        let set = |s: &mut Session, e: &str, v: Value| {
            let mut v = v;
            v["entities"] = json!([e]);
            call(s, "appearance.set", v).unwrap();
        };
        set(&mut s, "bolt", json!({"colour": "#101010"}));
        set(
            &mut s,
            "pair/left",
            json!({"colour": "#202020", "opacity": 0.5}),
        );
        set(&mut s, "frame/p1", json!({"colour": "#303030"}));
        set(&mut s, "frame/p2/right/shank", json!({"opacity": 0.25}));
        let (v, bytes) = exported(
            &mut s,
            json!({"part": "frame", "time_stamp": "2026-11-11T00:00:00"}),
        )
        .unwrap();
        let o = ryustep::open(&bytes).unwrap();
        let defs = &o.products.definitions;
        let bolt = o
            .held
            .iter()
            .position(|h| h.definition.is_some_and(|d| &*defs[d].product_id == "bolt"))
            .unwrap();
        assert_eq!(o.body_styles(bolt).body.unwrap().colour, Some([0x10; 3]));
        // Each occurrence by its usages' ids, outermost first.
        // Each occurrence style's usage ids, colour and opacity.
        type Got = (Vec<String>, Option<[u8; 3]>, Option<f64>);
        let mut got: Vec<Got> = o
            .occurrence_styles()
            .iter()
            .map(|x| {
                let ids = x
                    .path
                    .iter()
                    .map(|&(k, n)| defs[k].children[n].id.to_string())
                    .collect();
                (ids, x.style.colour, x.style.opacity)
            })
            .collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            [
                (vec!["p1".into(), "left".into()], Some([0x30; 3]), Some(0.5)),
                (vec!["p1".into(), "right".into()], Some([0x30; 3]), None),
                (vec!["p2".into(), "left".into()], Some([0x20; 3]), Some(0.5)),
                (
                    vec!["p2".into(), "right".into()],
                    Some([0x10; 3]),
                    Some(0.25)
                ),
            ]
        );
        assert!(v.get("warnings").is_none(), "{v}");
        // A face of the bolt coloured in the bolt: in an occurrence drawn
        // otherwise, that face can't be said per occurrence.
        let face = crate::appearance::tests::face_names(&mut s, "bolt", "shank")[0].clone();
        set(
            &mut s,
            &format!("bolt/shank/{face}"),
            json!({"colour": "#0000ff"}),
        );
        let (v, _) = exported(&mut s, json!({"part": "frame"})).unwrap();
        let omitted = v["transfer"]["omitted"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["what"] == "appearance")
            .cloned()
            .unwrap_or_else(|| panic!("{v}"));
        assert_eq!(omitted["count"], 4, "{omitted}");
        let w = v["warnings"].as_array().unwrap();
        assert!(
            w.iter()
                .any(|w| w["code"] == "export_metadata_omitted" && w["what"] == "appearance")
        );
    }

    /// A label's export is its official commit's: its appearance too,
    /// whatever the working files say now (design 448, question 438).
    #[test]
    fn an_export_at_a_label_writes_the_release_commits_appearance() {
        use crate::git::tests::git;
        let (dir, mut s) = crate::assembly::tests::project();
        let r = dir.path();
        git(r, &["init", "-q", "-b", "main"]);
        call(
            &mut s,
            "props.set",
            json!({"part": "bolt", "properties": {"number": "100"}}),
        )
        .unwrap();
        call(
            &mut s,
            "appearance.set",
            json!({"entities": ["bolt"], "colour": "#ff0000"}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "bolt"})).unwrap();
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "bolt A, red"]);
        call(
            &mut s,
            "appearance.set",
            json!({"entities": ["bolt"], "colour": "#0000ff"}),
        )
        .unwrap();
        let colour = |bytes: &[u8]| {
            let o = ryustep::open(bytes).unwrap();
            o.body_styles(0).body.and_then(|x| x.colour)
        };
        let opts = |rev: Option<&str>| {
            let mut v = json!({"part": "bolt", "time_stamp": "2026-11-11T00:00:00"});
            if let Some(r) = rev {
                v["rev"] = json!(r);
            }
            v
        };
        let (_, at_label) = exported(&mut s, opts(Some("label:A"))).unwrap();
        assert_eq!(colour(&at_label), Some([255, 0, 0]));
        let (_, working) = exported(&mut s, opts(None)).unwrap();
        assert_eq!(colour(&working), Some([0, 0, 255]));
        // Twice at the label, the same bytes: what ryupdm's packages need.
        let (_, again) = exported(&mut s, opts(Some("label:A"))).unwrap();
        assert_eq!(again, at_label);
    }

    /// A part's material (with the library's density) and its set
    /// properties go out as STEP's material and user-defined attributes
    /// (design 448, plan 24.C).
    #[test]
    fn a_parts_material_and_properties_are_exported() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        add(
            &mut s,
            "p",
            json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
        );
        call(
            &mut s,
            "pdm.configure",
            json!({"properties": {
                "finish": {"type": "string"},
                "count": {"type": "integer"},
                "wall": {"type": "quantity", "dim": "length"},
                "rohs": {"type": "boolean"}
            }}),
        )
        .unwrap();
        call(
            &mut s,
            "materials.set",
            json!({"name": "Steel", "density": "7.85 g/cm^3"}),
        )
        .unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"number": "100", "material": "Steel", "finish": "red",
                                                "count": 4, "wall": "2 mm", "rohs": true}}),
        )
        .unwrap();
        let (v, bytes) = exported(
            &mut s,
            json!({"part": "p", "unit": "mm", "time_stamp": "2026-11-11T00:00:00"}),
        )
        .unwrap();
        let o = ryustep::open(&bytes).unwrap();
        let k = o
            .products
            .definitions
            .iter()
            .position(|d| &*d.product_id == "100")
            .unwrap();
        let p = &o.properties().products[&k];
        let m = p.material.as_ref().unwrap();
        assert_eq!((m.name.as_str(), m.density), ("Steel", Some(7850.0)));
        let got: Vec<(&str, &ryustep::properties::Value)> = p
            .attributes
            .iter()
            .map(|a| (a.name.as_str(), &a.value))
            .collect();
        use ryustep::properties::Value as V;
        assert_eq!(got[0], ("count", &V::Integer(4)));
        assert_eq!(got[1], ("finish", &V::Text("red".into())));
        assert_eq!(got[2], ("rohs", &V::Boolean(true)));
        let V::Measure { value, unit } = got[3].1 else {
            panic!("{got:?}")
        };
        assert_eq!((got[3].0, *value, unit.name.as_str()), ("wall", 2.0, "mm"));
        assert_eq!(
            got.len(),
            4,
            "number is the product's id, never an attribute"
        );
        let written: Vec<&str> = v["transfer"]["written"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["category"].as_str().unwrap())
            .collect();
        assert!(
            written.contains(&"materials") && written.contains(&"properties"),
            "{written:?}"
        );
        assert!(
            v["transfer"]["omitted"]
                .as_array()
                .unwrap()
                .iter()
                .all(|o| o["what"] != "properties"),
            "{v}"
        );
    }

    #[test]
    fn properties_false_writes_none_and_says_so() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        add(
            &mut s,
            "p",
            json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
        );
        let opts = json!({"part": "p", "time_stamp": "2026-11-11T00:00:00", "file_name": "p.step"});
        let (_, before) = exported(&mut s, opts.clone()).unwrap();
        call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"finish": {"type": "string"}}}),
        )
        .unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"finish": "red", "material": "Steel"}}),
        )
        .unwrap();
        let (with, after) = exported(&mut s, opts.clone()).unwrap();
        assert_ne!(after, before);
        assert!(with.get("warnings").is_none(), "{with}");
        let mut off = opts;
        off["properties"] = json!(false);
        let (left_out, bytes) = exported(&mut s, off).unwrap();
        assert_eq!(bytes, before, "the plain bytes");
        let omitted: Vec<&Value> = left_out["transfer"]["omitted"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| o["what"] == "properties")
            .collect();
        assert_eq!(omitted.len(), 1, "{left_out}");
        assert_eq!(omitted[0]["count"], 2, "finish and material");
        assert!(
            omitted[0]["summary"]
                .as_str()
                .unwrap()
                .contains("properties: false")
        );
        let w = left_out["warnings"].as_array().unwrap();
        assert!(
            w.iter()
                .any(|w| w["code"] == "export_metadata_omitted" && w["what"] == "properties")
        );
    }

    /// A label's export is its official commit's: its properties and its
    /// library's density too (design 448, plan 24.C).
    #[test]
    fn an_export_at_a_label_writes_the_release_commits_properties() {
        use crate::git::tests::git;
        let (dir, mut s) = crate::assembly::tests::project();
        let r = dir.path();
        git(r, &["init", "-q", "-b", "main"]);
        call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"finish": {"type": "string"}}}),
        )
        .unwrap();
        call(
            &mut s,
            "materials.set",
            json!({"name": "Steel", "density": "7850 kg/m^3"}),
        )
        .unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "bolt", "properties": {"number": "100", "material": "Steel", "finish": "red"}}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "bolt"})).unwrap();
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "bolt A"]);
        call(
            &mut s,
            "props.set",
            json!({"part": "bolt", "properties": {"finish": "blue"}}),
        )
        .unwrap();
        call(
            &mut s,
            "materials.set",
            json!({"name": "Steel", "density": "8000 kg/m^3"}),
        )
        .unwrap();
        let read = |bytes: &[u8]| {
            let o = ryustep::open(bytes).unwrap();
            let k = o
                .products
                .definitions
                .iter()
                .position(|d| &*d.product_id == "100")
                .unwrap();
            let p = o.properties().products[&k].clone();
            let finish = p
                .attributes
                .iter()
                .find(|a| a.name == "finish")
                .unwrap()
                .value
                .clone();
            (p.material.unwrap().density, finish)
        };
        let opts = |rev: Option<&str>| {
            let mut v = json!({"part": "bolt", "time_stamp": "2026-11-11T00:00:00"});
            if let Some(r) = rev {
                v["rev"] = json!(r);
            }
            v
        };
        use ryustep::properties::Value as V;
        let (_, at_label) = exported(&mut s, opts(Some("label:A"))).unwrap();
        assert_eq!(read(&at_label), (Some(7850.0), V::Text("red".into())));
        let (_, working) = exported(&mut s, opts(None)).unwrap();
        assert_eq!(read(&working), (Some(8000.0), V::Text("blue".into())));
        let (_, again) = exported(&mut s, opts(Some("label:A"))).unwrap();
        assert_eq!(again, at_label, "what ryupdm's packages need");
    }

    /// `painted_frame`'s model with a material and two attributes on its
    /// root, as ryucad writes them: what an import maps and an export
    /// writes again.
    fn attributed_frame() -> Vec<u8> {
        // The same model `painted_frame` writes (factor its model into a
        // `painted_frame_model()` both call), without paints.
        let (model, _) = painted_frame_model();
        let data = ryustep::PartData {
            products: vec![ryustep::ProductData {
                product: 0,
                material: Some(ryustep::MaterialData {
                    name: "Steel".into(),
                    density: Some(7850.0),
                }),
                attributes: vec![
                    ryustep::AttributeData {
                        name: "count".into(),
                        value: ryustep::DataValue::Integer(4),
                    },
                    ryustep::AttributeData {
                        name: "finish".into(),
                        value: ryustep::DataValue::Text("red".into()),
                    },
                ],
            }],
        };
        let opts = ryustep::WriteOptions {
            unit: ryustep::LengthUnit::Meter,
            file_name: "frame.step".into(),
            time_stamp: "2026-10-27T00:00:00".into(),
            ..Default::default()
        };
        ryustep::write_with_data(&model, &[], &ryustep::Paints::default(), &data, &opts)
            .unwrap()
            .bytes
    }

    /// A file with a material and attributes imported into a project
    /// declaring them, and exported with its header, is the file, byte
    /// for byte (design 448's fixpoint, with metadata).
    #[test]
    fn an_attributed_file_exports_as_it_came() {
        let original = attributed_frame();
        let (_src, path) = outside(&original, "frame.step");
        let (_dir, mut s) = fresh();
        call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"finish": {"type": "string"}, "count": {"type": "integer"}}}),
        )
        .unwrap();
        call(
            &mut s,
            "materials.set",
            json!({"name": "Steel", "density": "7850 kg/m^3"}),
        )
        .unwrap();
        call(&mut s, "import", json!({"solid": true, "path": path})).unwrap();
        let params = json!({"part": "f-1", "unit": "m", "time_stamp": "2026-10-27T00:00:00",
                            "file_name": "frame.step"});
        let (_, bytes) = exported(&mut s, params).unwrap();
        assert!(bytes == original, "{}", first_difference(&original, &bytes));
    }
}
