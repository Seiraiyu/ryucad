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
}

/// What `export` asks for: the part, the revision (`working`, a Git
/// revision, or `label:<L>`), the format (else `out`'s extension, else
/// STEP), the absolute path to write, `ap242` or `ap214`, the unit, and
/// the header's time stamp and file name.
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
}

/// The formats `export` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Step,
    Stl,
    ThreeMf,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Step, Format::Stl, Format::ThreeMf];

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Step => "step",
            Format::Stl => "stl",
            Format::ThreeMf => "3mf",
        }
    }

    pub fn parse(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.as_str() == s)
    }

    /// The format a file name's extension names, in any case: `.step` or
    /// `.stp`, `.stl`, `.3mf`.
    pub fn of_path(path: &str) -> Option<Format> {
        let ext = std::path::Path::new(path).extension()?.to_str()?;
        match ext.to_ascii_lowercase().as_str() {
            "step" | "stp" => Some(Format::Step),
            "stl" => Some(Format::Stl),
            "3mf" => Some(Format::ThreeMf),
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
}

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
        match format {
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

/// The file `out` names, checked: absolute, in a folder that exists.
fn out_path(out: &str) -> Result<std::path::PathBuf> {
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
    Ok(path)
}

/// The answer to an export of `encoding` (a format's name): `write`'s
/// bytes written to `out` atomically, with their count and BLAKE3, or
/// with no `out` following the answer.
fn deliver(
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
    let format = format(&ask)?;
    let options = Options::read(format, &ask, session.project().manifest.default_unit)?;
    let out = ask.out.as_deref().map(out_path).transpose()?;
    let part = crate::server::part_id(&ask.part)?;
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
    if !walk.failing.is_empty() {
        let failing: Vec<&Value> = walk.failing.values().collect();
        return Err(Error::new(
            Code::ExportFailed,
            format!(
                "{part} can't be exported: {} feature{} of what it places fail{}",
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
    if let Some(c) = at {
        result["commit"] = json!(c);
    }
    match options {
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
            let bytes =
                ryustep::write(&step_model(&walk.scene), &opts).map_err(|e| walk.unwritable(e))?;
            result["time_stamp"] = json!(opts.time_stamp);
            result["schema"] = json!(schema_name);
            result["unit"] = json!(unit_name);
            result["file_name"] = json!(opts.file_name);
            deliver(result, out.as_deref(), "step", |w| w.write_all(&bytes))
        }
        Options::Stl { unit, mesh, ascii } => {
            let scene = meshed(s, &walk.scene, &mesh)?;
            triangles(&mut result, &scene);
            result["unit"] = json!(unit.as_str());
            result["mesh"] = mesh.report();
            result["ascii"] = json!(ascii);
            let opts = ryuformats::stl::Stl {
                unit,
                ascii,
                header: format!(
                    "ryucad {} {part} {}",
                    env!("CARGO_PKG_VERSION"),
                    unit.as_str()
                ),
            };
            deliver(result, out.as_deref(), "stl", |w| {
                ryuformats::stl::write(&scene, &opts, w)
            })
        }
        Options::ThreeMf { unit, mesh } => {
            let scene = meshed(s, &walk.scene, &mesh)?;
            triangles(&mut result, &scene);
            result["unit"] = json!(unit.as_str());
            result["mesh"] = mesh.report();
            result["time_stamp"] = json!(time_stamp);
            let mut metadata = vec![
                ("Title".to_owned(), scene.products[0].name.clone()),
                (
                    "Application".to_owned(),
                    format!("ryucad {}", env!("CARGO_PKG_VERSION")),
                ),
                ("CreationDate".to_owned(), time_stamp),
                ("ryucad:chord".to_owned(), mesh.chord.text()),
                ("ryucad:angle".to_owned(), mesh.angle.text()),
            ];
            if let Some(e) = &mesh.max_edge {
                metadata.push(("ryucad:max_edge".to_owned(), e.text()));
            }
            let opts = ryuformats::threemf::ThreeMf { unit, metadata };
            deliver(result, out.as_deref(), "3mf", |w| {
                ryuformats::threemf::write(&scene, &opts, w)
            })
        }
    }
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
fn meshed(s: &mut Session, scene: &Scene, opts: &MeshOptions) -> Result<ryuformats::Scene> {
    let solids: Vec<(&Product, &Solid)> = scene
        .products
        .iter()
        .flat_map(|p| p.solids.iter().map(move |x| (p, x)))
        .collect();
    let builts: Vec<&Built> = solids.iter().map(|(_, x)| x.built.as_ref()).collect();
    let made = s
        .meshes
        .export_all(&builts, &opts.params, crate::server::mesh_budget());
    let mut meshes: BTreeMap<&str, Arc<ryukei::mesh::BodyMesh>> = BTreeMap::new();
    let mut faults = Vec::new();
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
            faults.extend(fault_report(x, &p.rev, mesh));
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
    Ok(ryuformats::Scene { products })
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

/// STEP's model of a scene: each product's solids as its bodies, each
/// child's usage as its identifier.
fn step_model(scene: &Scene) -> WriteModel {
    let products = scene
        .products
        .iter()
        .map(|p| ryustep::Product {
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            revision: p.revision.clone(),
            bodies: p.solids.iter().map(|s| s.built.body.clone()).collect(),
            children: p
                .children
                .iter()
                .map(|c| ryustep::Child {
                    product: c.product,
                    place: c.place,
                    id: c.usage.clone(),
                })
                .collect(),
        })
        .collect();
    WriteModel { products }
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
        let props = properties(s, part, rev, self.pin.as_ref().filter(|_| k == 0));
        let (id, name, description) = identity(part, &p, &props);
        let revision = match rev {
            Rev::Label(l) => l.clone(),
            Rev::Working => String::new(),
        };
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
                                mine.extend(solids(&cp, &placed.rev, &cr));
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
            founder: s.founder,
            built: s.built,
        })
        .collect()
}

/// The product an imported part or assembly came from: the one product
/// every import feature of a part names, or every imported instance of
/// an assembly is used in.
fn imported(p: &Part) -> Option<ProductRef> {
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
/// the part's ID.
fn identity(part: &Id, p: &Part, props: &Properties) -> (String, String, String) {
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
        .unwrap_or_else(|| part.to_string());
    let name = text("name")
        .or_else(|| from.as_ref().map(|f| f.name.clone()))
        .unwrap_or_else(|| part.to_string());
    (id, name, text("description").unwrap_or_default())
}

/// `part`'s properties at `rev`: the working files', or for an official
/// release (`pin`, for the part exported; else looked up) its commit's.
fn properties(s: &mut Session, part: &Id, rev: &Rev, pin: Option<&Pin>) -> Properties {
    let commit = match (rev, pin) {
        (Rev::Working, _) => None,
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
    use crate::server::tests::{call, fresh};

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
        let product = |id: &str, name: &str, bodies, children| ryustep::Product {
            id: id.into(),
            name: name.into(),
            bodies,
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

    #[test]
    fn an_imported_file_exports_as_it_came() {
        let original = frame();
        let (_src, path) = outside(&original, "frame.step");
        let (dir, mut s) = fresh();
        call(&mut s, "import", json!({"path": path})).unwrap();
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
            json!({"path": out, "update": "imports/frame.step"}),
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
        let v = call(&mut s2, "import", json!({"path": out})).unwrap();
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
}
