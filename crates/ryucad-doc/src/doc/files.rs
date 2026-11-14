//! The file kinds: the project manifest, a part's feature list, its
//! parameters and a feature. Each reads from and writes to a JSON value;
//! the file system is [`super::project`]'s business.

use super::assembly::Instance;
use super::blend::{Chamfer, Fillet};
use super::body::{Mirror, Offset, Shell, Split};
use super::datum::Datum;
use super::edit::{DeleteBlend, DeleteFace, ResizeBlend};
use super::geometry::GeomRef;
use super::hole::Hole;
use super::id::{Id, ProjectId};
use super::import::Import;
use super::mate::{InstancePattern, Mate};
use super::path::PathFeature;
use super::pattern::Pattern;
use super::reference::Reference;
use super::sheet_metal::{BaseFlange, Flange, Refold, Unfold};
use super::sketch::Sketch;
use super::solid::{Boolean, Combine};
use super::surface::{Extract, Fill, Sew, Thicken};
use super::sweep::{Extrude, Loft, Revolve, Sweep};
use super::tweak::{Draft, MoveFace, ReplaceFace};
use crate::error::{Code, Error, Result};
use crate::expr::{self, Env, Expr};
use crate::units::{Quantity, Unit};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The format version of the manifest, part and parameters files.
pub const FORMAT: u64 = 1;

/// The newest part file format: 2 adds `kind`, 3 an assembly's item
/// numbers, 4 item numbers of other projects' parts. A part file stays at
/// the lowest that holds it: a part at 1, an assembly without items at 2.
pub const PART_FORMAT: u64 = 4;

/// The largest item number.
pub const MAX_ITEM: u32 = 99_999;

/// An assembly's item numbers (the find numbers balloons cite), by the
/// part each numbers: its ID, or for another project's part
/// `<project ID>:<part>` ([`Id::foreign`]), so a local `bolt` and a
/// dependency's `bolt` have numbers of their own.
pub type Items = BTreeMap<Id, u32>;

/// The key of `part` at `rev` in an assembly's [`Items`].
pub fn item_key(part: &Id, rev: &super::assembly::Rev) -> Id {
    match rev.project() {
        Some(p) => Id::foreign(p, part),
        None => part.clone(),
    }
}

/// An item key as `items` writes it: a part ID, or `<project ID>:<part>`.
pub fn parse_item_key(s: &str) -> Result<Id> {
    if let Some((p, part)) = s.split_once(':') {
        let p = ProjectId::parse(p)?;
        return Ok(Id::foreign(&p, &Id::parse(part, "part")?));
    }
    Id::parse(s, "part")
}

/// The newest manifest format: 2 adds the project's ID, 3 its
/// dependencies and where it was forked from, 4 the drafting standard its
/// drawings follow (design 417). A manifest stays at the lowest that holds
/// it, so older builds still read projects that don't use PDM, other
/// projects or ISO drawings.
pub const MANIFEST_FORMAT: u64 = 4;

/// The newest feature format: 2 adds expressions, sketches and sweeps, 3
/// operations on solids, 4 an assembly's instances, mates and instance
/// patterns, 5 imports, 6 instances of other projects' parts, 7 datums and
/// the geometric references new with them (a whole feature, a sketch's
/// entity, a plane where an axis went), 8 sweeps along a path (ryucad 14),
/// 9 shells and offsets, 10 mirrors and splits, 11 base flanges and
/// flanges (ryucad 16), 12 surface sweeps and planar faces (ryucad 18),
/// 13 reference imports (ryucad 21), 14 unfolds and refolds (ryucad
/// 16.B), 15 fills, extracts, sews and thickens (ryucad 18), 16 face
/// tweaks (ryucad 17), 17 a flattened import's place (ryucad 21.D), 18
/// drafts (ryucad 17.B), 19 splits by a body or faces (ryucad 18.C), 20 direct edits (ryucad 22), 21
/// inserts, deletes of bodies and solid paths (ryucad 25), 22 a mate's
/// limits (ryucad 26), 23 a hole's
/// thread, fastener, fit and tolerance (ryucad 28), 24 an instance
/// pattern that follows a part's features or a sketch's points (ryucad
/// 25.C), 25 a hole's new geometry and cosmetic threads (ryucad 28.B). A
/// feature is written at the lowest format that holds it, so one with
/// only literals stays 1.
pub const FEATURE_FORMAT: u64 = {
    // The largest of every stage's (a later stage merging adds its own).
    let all = [
        super::sweep::SURFACE_FORMAT,
        super::import::MODE_FORMAT,
        super::sheet_metal::UNFOLD_FORMAT,
        super::surface::FORMAT,
        super::tweak::FORMAT,
        super::import::PLACE_FORMAT,
        super::tweak::DRAFT_FORMAT,
        super::body::TOOL_FORMAT,
        super::edit::FORMAT,
        super::hole::INTENT_FORMAT,
        super::hole::GEOMETRY_FORMAT,
        super::insert::FORMAT,
        super::mate::LIMITS_FORMAT,
        super::mate::DRIVEN_FORMAT,
    ];
    let (mut max, mut i) = (0, 0);
    while i < all.len() {
        if all[i] > max {
            max = all[i];
        }
        i += 1;
    }
    max
};

pub const PROJECT_KIND: &str = "ryucad.project";
pub const PART_KIND: &str = "ryucad.part";
pub const PARAMETERS_KIND: &str = "ryucad.parameters";
pub const FEATURE_KIND: &str = "ryucad.feature";

/// The project manifest, `ryucad.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub default_unit: Unit,
    /// The project's identity, from its first PDM write on.
    pub project: Option<ProjectId>,
    /// Other projects whose released parts it places, by project ID.
    pub dependencies: super::deps::Dependencies,
    /// The ID and commit `ryucad pdm new-id` gave it a new ID at.
    pub forked_from: Option<super::deps::ForkedFrom>,
    /// The standard its drawings follow unless one says another: ASME
    /// when none is written (question 337).
    pub drawing_standard: Option<super::drawing::Standard>,
}

/// A part's ordered feature list, `parts/<part>/part.json`, whether it is
/// an assembly, and an assembly's item numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartFile {
    pub kind: PartKind,
    pub features: Vec<Id>,
    /// Each placed part's item number, from 1 to [`MAX_ITEM`]. Labels,
    /// not identities: two parts sharing one, or a placed part without
    /// one, read, and are faults of the BOM (`ryupdm::items`).
    pub items: Items,
}

/// What a part file describes: a part, whose features make solids, or an
/// assembly, whose features place other parts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PartKind {
    #[default]
    Part,
    Assembly,
}

impl PartKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PartKind::Part => "part",
            PartKind::Assembly => "assembly",
        }
    }

    /// `part` or `assembly`.
    pub fn parse(s: &str) -> Result<PartKind> {
        match s {
            "part" => Ok(PartKind::Part),
            "assembly" => Ok(PartKind::Assembly),
            _ => Err(Error::new(
                Code::InvalidParam,
                format!("unknown part kind {s:?}; known: part, assembly"),
            )
            .with("kind", s)),
        }
    }
}

/// A part's named parameters, `parts/<part>/parameters.json`: each an
/// expression over the others.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParametersFile {
    pub parameters: BTreeMap<String, Expr>,
}

/// The primitive types of phase 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimitiveType {
    Box,
    Cylinder,
    Cone,
    Sphere,
    Torus,
}

/// What a feature that always founds its own solid does with its body.
static NEW: Combine = Combine::NEW;

/// A feature, `parts/<part>/features/<id>.json`: its `type` says which.
#[derive(Clone, Debug, PartialEq)]
pub enum Feature {
    /// Another part's solids, founded in this part's frame (ryucad 25).
    Insert(Box<super::insert::Insert>),
    /// Solids taken out of the part's result (ryucad 25).
    DeleteBody(Box<super::body::DeleteBody>),
    Primitive(Primitive),
    Sketch(Sketch),
    Extrude(Extrude),
    Revolve(Revolve),
    Boolean(Boolean),
    Fillet(Fillet),
    Chamfer(Chamfer),
    Hole(Box<Hole>),
    /// A thread on cylindrical faces, as data (ryucad 28.B).
    CosmeticThread(Box<super::thread::CosmeticThread>),
    Pattern(Pattern),
    Instance(Instance),
    Mate(Box<Mate>),
    InstancePattern(Box<InstancePattern>),
    Import(Import),
    Datum(Box<Datum>),
    /// A path or a helix.
    Path(Box<PathFeature>),
    /// A profile swept along a path.
    Sweep(Box<Sweep>),
    /// A skin through planar sections.
    Loft(Box<Loft>),
    /// A solid hollowed to a wall.
    Shell(Box<Shell>),
    /// A solid grown or shrunk, or its faces pushed and pulled.
    Offset(Box<Offset>),
    /// A solid's mirror image, a solid of its own or applied to others.
    Mirror(Box<Mirror>),
    /// A solid split by a plane.
    Split(Box<Split>),
    /// A profile's closed loops as plane faces of a sheet (ryucad 18).
    Planar(Box<super::sweep::Planar>),
    Fill(Box<Fill>),
    Extract(Box<Extract>),
    Sew(Box<Sew>),
    Thicken(Box<Thicken>),
    /// A sheet metal solid founded from a sketch, with its settings.
    BaseFlange(Box<BaseFlange>),
    /// Bends and flanges on straight edges of a sheet metal solid.
    Flange(Box<Flange>),
    /// A sheet metal solid laid flat.
    Unfold(Box<Unfold>),
    /// A sheet metal solid folded back by its unfold's flat map.
    Refold(Box<Refold>),
    /// Faces of a solid moved: translated, turned or offset (ryucad 17).
    MoveFace(Box<MoveFace>),
    /// Faces of a solid given another face's surface or a plane.
    ReplaceFace(Box<ReplaceFace>),
    /// Faces of a solid drafted about a neutral plane or a parting line
    /// (ryucad 17.B).
    Draft(Box<Draft>),
    /// Faces of a solid deleted, each gap healed (ryucad 22).
    DeleteFace(Box<DeleteFace>),
    /// Blends of a solid deleted, by size or by face, their supports
    /// grown back (ryucad 22.B).
    DeleteBlend(Box<DeleteBlend>),
    /// Blends of a solid made again at new sizes, keeping their names
    /// (ryucad 22.B).
    ResizeBlend(Box<ResizeBlend>),
}

/// A primitive feature: one solid of a type, sized by its params.
#[derive(Clone, Debug, PartialEq)]
pub struct Primitive {
    pub ty: PrimitiveType,
    pub params: BTreeMap<String, Quantity>,
    pub placement: Placement,
    /// A solid of its own, or what it does to others.
    pub combine: Combine,
}

/// Where a feature's local frame sits: an origin, an axis `z` and a
/// reference direction `x` (made perpendicular to `z` on rebuild).
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub origin: [Quantity; 3],
    pub z: [f64; 3],
    pub x: [f64; 3],
}

impl PrimitiveType {
    pub const ALL: [PrimitiveType; 5] = [
        PrimitiveType::Box,
        PrimitiveType::Cylinder,
        PrimitiveType::Cone,
        PrimitiveType::Sphere,
        PrimitiveType::Torus,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PrimitiveType::Box => "box",
            PrimitiveType::Cylinder => "cylinder",
            PrimitiveType::Cone => "cone",
            PrimitiveType::Sphere => "sphere",
            PrimitiveType::Torus => "torus",
        }
    }

    pub fn parse(s: &str) -> Result<PrimitiveType> {
        PrimitiveType::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| unknown_type(s))
    }

    /// The required parameters, each a length greater than zero.
    pub fn required(self) -> &'static [&'static str] {
        match self {
            PrimitiveType::Box => &["x", "y", "z"],
            PrimitiveType::Cylinder | PrimitiveType::Cone => &["radius", "height"],
            PrimitiveType::Sphere => &["radius"],
            PrimitiveType::Torus => &["major_radius", "minor_radius"],
        }
    }

    /// The optional parameters, each a length of zero or more.
    pub fn optional(self) -> &'static [&'static str] {
        match self {
            PrimitiveType::Cone => &["top_radius"],
            _ => &[],
        }
    }
}

impl Placement {
    /// Whether it is the frame it sits in: a zero origin (literals) and
    /// the axes as they are.
    pub fn is_identity(&self) -> bool {
        self.origin.iter().all(|q| q.is_literal() && q.si() == 0.0)
            && self.z == [0.0, 0.0, 1.0]
            && self.x == [1.0, 0.0, 0.0]
    }

    /// The world frame, with the origin written in `unit`.
    pub fn identity(unit: Unit) -> Placement {
        let zero = Quantity::parse("0", unit).expect("0 parses");
        Placement {
            origin: [zero.clone(), zero.clone(), zero],
            z: [0.0, 0.0, 1.0],
            x: [1.0, 0.0, 0.0],
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "origin": self.origin.iter().map(|q| q.text()).collect::<Vec<_>>(),
            "x": self.x.map(number),
            "z": self.z.map(number),
        })
    }

    /// Reads a placement object, its origin's expressions reading `env`;
    /// `at` is its JSON pointer for errors.
    pub fn from_json(v: &Value, default: Unit, at: &str, env: &Env) -> Result<Placement> {
        let mut o = Obj::new(v, at)?;
        let origin = o.take("origin")?;
        let origin = triple(&origin, &format!("{at}/origin"), |v, at| {
            Quantity::from_json_in(v, default, &|n| env.get(n)).map_err(|e| e.with("pointer", at))
        })?;
        let z = vector(&o.take("z")?, &format!("{at}/z"))?;
        let x = vector(&o.take("x")?, &format!("{at}/x"))?;
        o.finish()?;
        Ok(Placement { origin, z, x })
    }
}

impl Manifest {
    /// A manifest with no project ID.
    pub fn new(default_unit: Unit) -> Manifest {
        Manifest {
            default_unit,
            project: None,
            dependencies: super::deps::Dependencies::new(),
            forked_from: None,
            drawing_standard: None,
        }
    }

    /// The standard the project's drawings follow by default.
    pub fn standard(&self) -> super::drawing::Standard {
        self.drawing_standard.unwrap_or_default()
    }

    pub fn to_json(&self) -> Value {
        let mut v =
            json!({"default_unit": self.default_unit, "format": FORMAT, "kind": PROJECT_KIND});
        if let Some(id) = &self.project {
            v["project"] = json!(id);
            v["format"] = json!(2);
        }
        if !self.dependencies.is_empty() {
            v["dependencies"] = super::deps::to_json(&self.dependencies);
            v["format"] = json!(3);
        }
        if let Some(f) = &self.forked_from {
            v["forked_from"] = f.to_json();
            v["format"] = json!(3);
        }
        if let Some(s) = self.drawing_standard {
            v["drawing_standard"] = json!(s.as_str());
            v["format"] = json!(MANIFEST_FORMAT);
        }
        v
    }

    pub fn from_json(v: &Value) -> Result<Manifest> {
        let mut o = header(v, PROJECT_KIND, MANIFEST_FORMAT)?;
        let project = match o.take_opt("project") {
            None => None,
            Some(p) => Some(
                p.as_str()
                    .ok_or_else(|| invalid("/project", "project must be a string"))
                    .and_then(ProjectId::parse)
                    .map_err(|e| e.with("pointer", "/project"))?,
            ),
        };
        let dependencies = match o.take_opt("dependencies") {
            None => super::deps::Dependencies::new(),
            Some(d) => super::deps::from_json(&d)?,
        };
        let forked_from = match o.take_opt("forked_from") {
            None => None,
            Some(f) => Some(super::deps::ForkedFrom::from_json(&f)?),
        };
        let drawing_standard = match o.take_opt("drawing_standard") {
            None => None,
            Some(d) => Some(
                d.as_str()
                    .and_then(super::drawing::Standard::parse)
                    .ok_or_else(|| invalid("/drawing_standard", "expected one of asme, iso"))?,
            ),
        };
        let unit = o.take("default_unit")?;
        let default_unit = unit.as_str().and_then(Unit::parse).ok_or_else(|| {
            invalid(
                "/default_unit",
                "default_unit must be one of m, cm, mm, um, in, ft",
            )
        })?;
        o.finish()?;
        Ok(Manifest {
            default_unit,
            project,
            dependencies,
            forked_from,
            drawing_standard,
        })
    }
}

impl PartFile {
    /// A part (not an assembly) listing `features`.
    pub fn new(features: Vec<Id>) -> PartFile {
        PartFile {
            kind: PartKind::Part,
            features,
            items: Items::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"features": self.features, "format": FORMAT, "kind": PART_KIND});
        // `kind` names the file kind; a part's own kind is `part_kind`.
        if self.kind == PartKind::Assembly {
            v["part_kind"] = json!(self.kind.as_str());
            v["format"] = json!(2);
        }
        if !self.items.is_empty() {
            v["items"] = json!(self.items);
            let foreign = self.items.keys().any(|k| k.foreign_of().is_some());
            v["format"] = json!(if foreign { PART_FORMAT } else { 3 });
        }
        v
    }

    pub fn from_json(v: &Value) -> Result<PartFile> {
        let mut o = header(v, PART_KIND, PART_FORMAT)?;
        let kind = match o.take_opt("part_kind") {
            None => PartKind::Part,
            Some(k) => match k.as_str() {
                Some("assembly") => PartKind::Assembly,
                _ => {
                    return Err(invalid(
                        "/part_kind",
                        "part_kind must be \"assembly\", or left out for a part",
                    ));
                }
            },
        };
        let list = o.take("features")?;
        let items = list
            .as_array()
            .ok_or_else(|| invalid("/features", "features must be an array of feature IDs"))?;
        let mut features: Vec<Id> = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let at = format!("/features/{i}");
            let s = item
                .as_str()
                .ok_or_else(|| invalid(&at, "a feature ID must be a string"))?;
            let id = Id::parse(s, "feature").map_err(|e| e.with("pointer", &at))?;
            if features.contains(&id) {
                return Err(
                    Error::new(Code::DuplicateId, format!("feature {id} is listed twice"))
                        .with("id", &id)
                        .with("pointer", &at),
                );
            }
            features.push(id);
        }
        let mut items = Items::new();
        if let Some(list) = o.take_opt("items") {
            if kind != PartKind::Assembly {
                return Err(invalid("/items", "only an assembly has item numbers"));
            }
            let map = list.as_object().ok_or_else(|| {
                invalid(
                    "/items",
                    "items must be an object of item numbers by part ID",
                )
            })?;
            for (k, n) in map {
                let at = format!("/items/{k}");
                let part = parse_item_key(k).map_err(|e| e.with("pointer", &at))?;
                let n = n
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| (1..=MAX_ITEM).contains(n))
                    .ok_or_else(|| {
                        invalid(
                            &at,
                            &format!("an item number is a whole number from 1 to {MAX_ITEM}"),
                        )
                    })?;
                items.insert(part, n);
            }
        }
        o.finish()?;
        Ok(PartFile {
            kind,
            features,
            items,
        })
    }
}

impl ParametersFile {
    pub fn to_json(&self) -> Value {
        let params: Map<String, Value> = self
            .parameters
            .iter()
            .map(|(k, e)| (k.clone(), Value::String(e.text().to_owned())))
            .collect();
        json!({"format": FORMAT, "kind": PARAMETERS_KIND, "parameters": params})
    }

    /// Reads a parameters file: valid names, and expressions that parse. It
    /// doesn't evaluate them ([`Env::new`] does).
    pub fn from_json(v: &Value) -> Result<ParametersFile> {
        let mut o = header(v, PARAMETERS_KIND, FORMAT)?;
        let list = o.take("parameters")?;
        let map = list
            .as_object()
            .ok_or_else(|| invalid("/parameters", "parameters must be an object"))?;
        let mut parameters = BTreeMap::new();
        for (k, v) in map {
            let at = format!("/parameters/{k}");
            let name = expr::parameter_name(k).map_err(|e| e.with("pointer", &at))?;
            parameters.insert(name, expression(v).map_err(|e| e.with("pointer", &at))?);
        }
        o.finish()?;
        Ok(ParametersFile { parameters })
    }

    /// Their values.
    pub fn env(&self) -> Env {
        Env::new(&self.parameters)
    }
}

/// An expression from JSON: a string, or a number (a plain number).
pub fn expression(v: &Value) -> Result<Expr> {
    match v {
        Value::String(s) => Expr::parse(s),
        Value::Number(n) => Expr::parse(&n.to_string()),
        _ => Err(Error::new(
            Code::InvalidQuantity,
            format!("expected an expression string such as \"2 * wall\", got {v}"),
        )
        .with("value", v)),
    }
}

/// The expression strings in a feature's JSON: its params, the
/// coordinates of its origin, a sketch's dimensions, and a sweep's length
/// or angle.
fn expression_strings(v: &mut Value) -> Vec<&mut String> {
    let mut out = Vec::new();
    let Some(o) = v.as_object_mut() else {
        return out;
    };
    for (k, field) in o.iter_mut() {
        match (k.as_str(), field) {
            ("params", Value::Object(m)) => out.extend(m.values_mut().filter_map(|x| match x {
                Value::String(s) => Some(s),
                _ => None,
            })),
            ("placement", Value::Object(p)) => {
                if let Some(Value::Array(a)) = p.get_mut("origin") {
                    out.extend(a.iter_mut().filter_map(|x| match x {
                        Value::String(s) => Some(s),
                        _ => None,
                    }));
                }
            }
            ("constraints", c) => out.extend(Sketch::expression_strings(c)),
            (k @ ("fraction" | "at"), x) => out.extend(super::datum::expression_strings(k, x)),
            ("twist" | "scale", Value::String(s)) => out.push(s),
            ("start_tangent" | "end_tangent", Value::Object(t)) => {
                if let Some(Value::String(s)) = t.get_mut("weight") {
                    out.push(s);
                }
            }
            (k @ ("radius" | "pitch" | "turns" | "height" | "taper"), x) => {
                out.extend(super::path::expression_strings(k, x));
            }
            ("groups", g) => out.extend(super::blend::expression_strings(g)),
            (k @ ("allowance" | "relief" | "ends" | "setback" | "length" | "bends"), x) => {
                out.extend(super::sheet_metal::expression_strings(k, x));
            }
            ("thickness", Value::String(s)) => out.push(s),
            ("faces", g) => out.extend(super::body::expression_strings(g)),
            ("diameter", Value::String(s)) => out.push(s),
            ("distance", Value::String(s)) => out.push(s),
            ("limits", l @ Value::Object(_)) => out.extend(super::mate::limit_strings(l)),
            ("a" | "b" | "axis" | "direction" | "direction2", c @ Value::Object(_)) => {
                out.extend(super::mate::expression_strings(c));
            }
            // A base flange's bent section's depth is a plain length; a
            // hole's is an object (and "through_all" a word).
            ("depth", Value::String(s)) => {
                if s != "through_all" && s != "through_next" {
                    out.push(s);
                }
            }
            (
                k @ ("counterbore" | "countersink" | "counterdrill" | "spotface" | "near" | "far"
                | "depth" | "bottom" | "thread"),
                x,
            ) => {
                out.extend(super::hole::expression_strings(k, x));
            }
            // A hole's tolerance is an object; a fill's or a sew's a string.
            ("tolerance", x @ Value::Object(_)) => {
                out.extend(super::hole::expression_strings("tolerance", x));
            }
            (k @ ("spacing" | "spacing2" | "count" | "count2"), x) => {
                out.extend(super::pattern::expression_strings(k, x));
            }
            ("extent", Value::Object(e)) => {
                for (k, x) in e.iter_mut() {
                    if let ("length" | "offset", Value::String(s)) = (k.as_str(), x) {
                        out.push(s);
                    }
                }
            }
            // "full" is a word, not a parameter.
            // A delete_blend's size.
            ("below", Value::String(s)) => out.push(s),
            ("tolerance" | "front" | "back", Value::String(s)) => out.push(s),
            ("angle", Value::String(s)) if s != "full" => out.push(s),
            _ => {}
        }
    }
    out
}

/// Each expression field of a feature's JSON as it is written (the
/// strings [`feature_names`] reads), as its JSON pointer and text, in
/// pointer order: what an equations table lists and edits.
pub fn expression_fields(v: &Value) -> Vec<(String, String)> {
    pointers(v, expression_strings)
}

/// The strings `strings` finds in `v`, each by its JSON pointer, in
/// pointer order.
pub fn pointers(v: &Value, strings: fn(&mut Value) -> Vec<&mut String>) -> Vec<(String, String)> {
    // Each string found is marked by its place in the list, then found
    // again by a walk that knows where it is.
    const MARK: char = '\u{1}';
    let mut marked = v.clone();
    let mut texts = Vec::new();
    for (i, s) in strings(&mut marked).into_iter().enumerate() {
        texts.push(std::mem::replace(s, format!("{MARK}{i}")));
    }
    fn walk(v: &Value, at: &str, texts: &[String], out: &mut Vec<(String, String)>) {
        match v {
            Value::String(s) => {
                if let Some(i) = s
                    .strip_prefix(MARK)
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|i| *i < texts.len())
                {
                    out.push((at.to_owned(), texts[i].clone()));
                }
            }
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    walk(x, &format!("{at}/{i}"), texts, out);
                }
            }
            Value::Object(o) => {
                for (k, x) in o {
                    let k = k.replace('~', "~0").replace('/', "~1");
                    walk(x, &format!("{at}/{k}"), texts, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(&marked, "", &texts, &mut out);
    out.sort();
    out
}

/// The lowest part file format that holds `v`: 4 with an item number of
/// another project's part, 3 with item numbers, 2 for an assembly, else
/// 1.
pub fn part_format(v: &Value) -> u64 {
    let foreign = v
        .get("items")
        .and_then(Value::as_object)
        .is_some_and(|m| m.keys().any(|k| k.contains(':')));
    if foreign {
        PART_FORMAT
    } else if v.get("items").is_some() {
        3
    } else if v.get("part_kind").is_some() {
        2
    } else {
        FORMAT
    }
}

/// The lowest manifest format that holds `v`: 4 with a drawing standard,
/// 3 with dependencies or a fork's origin, 2 with a project ID, else 1.
pub fn manifest_format(v: &Value) -> u64 {
    if v.get("drawing_standard").is_some() {
        MANIFEST_FORMAT
    } else if v.get("dependencies").is_some() || v.get("forked_from").is_some() {
        3
    } else if v.get("project").is_some() {
        2
    } else {
        FORMAT
    }
}

/// The lowest format that holds a feature's JSON, judged from the JSON
/// alone, as [`Feature::format`] judges a loaded feature: instances of
/// other projects' parts are new in format 6, imports in format 5, instances, mates and instance patterns in format 4, operations on solids, booleans, fillets, chamfers,
/// holes and patterns are new in format 3, sketches
/// and sweeps in format 2, and a primitive needs 2 only for an
/// expression.
pub fn feature_format(v: &Value) -> u64 {
    let ty = v.get("type").and_then(Value::as_str);
    // Highest first: a cosmetic thread and a hole's new geometry are 25,
    // a following instance pattern 24, a hole with intent 23, whatever
    // else they hold.
    if ty == Some("cosmetic_thread") {
        return super::thread::FORMAT;
    }
    if ty == Some("hole") && super::hole::Hole::json_has_geometry(v) {
        return super::hole::GEOMETRY_FORMAT;
    }
    if ty == Some("instance_pattern")
        && matches!(
            v.get("form").and_then(Value::as_str),
            Some("feature" | "points")
        )
    {
        return super::mate::DRIVEN_FORMAT;
    }
    if ty == Some("hole")
        && ["thread", "fastener", "fit", "tolerance"]
            .iter()
            .any(|k| v.get(*k).is_some())
    {
        return super::hole::INTENT_FORMAT;
    }
    // A mate with limits that names a solid path needs 22.
    if v.get("type").and_then(Value::as_str) == Some("mate") && v.get("limits").is_some() {
        return super::mate::LIMITS_FORMAT;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("insert" | "delete_body")
    ) || solid_paths(v)
    {
        return super::insert::FORMAT;
    }
    let retrims = v.get("topology").and_then(Value::as_str) == Some("change")
        || v.get("blends").and_then(Value::as_str) == Some("rebuild");
    if ty.is_some_and(|t| super::edit::TYPES.contains(&t))
        || (ty.is_some_and(|t| super::tweak::TYPES.contains(&t)) && retrims)
    {
        return super::edit::FORMAT;
    }
    if v.get("type").and_then(Value::as_str) == Some("import") && v.get("place").is_some() {
        return super::import::PLACE_FORMAT;
    }
    if v.get("type").and_then(Value::as_str) == Some("draft") {
        return super::tweak::DRAFT_FORMAT;
    }
    if v.get("type").and_then(Value::as_str) == Some("split") && v.get("tool").is_some() {
        return super::body::TOOL_FORMAT;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("fill" | "extract" | "sew" | "thicken")
    ) {
        return super::surface::FORMAT;
    }
    if v.get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| super::tweak::TYPES.contains(&t))
    {
        return super::tweak::FORMAT;
    }
    if v.get("type").and_then(Value::as_str) == Some("import")
        && v.get("mode").and_then(Value::as_str) == Some("reference")
    {
        return super::import::MODE_FORMAT;
    }
    if v.get("surface") == Some(&Value::Bool(true))
        || v.get("type").and_then(Value::as_str) == Some("planar")
    {
        return super::sweep::SURFACE_FORMAT;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("unfold" | "refold")
    ) {
        return super::sheet_metal::UNFOLD_FORMAT;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("base_flange" | "flange")
    ) {
        return super::sheet_metal::FORMAT;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("sweep" | "loft")
    ) {
        return super::sweep::FORMAT;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("mirror" | "split")
    ) {
        return super::body::MIRROR_FORMAT;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("shell" | "offset")
    ) {
        return super::body::FORMAT;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("plane" | "axis" | "point" | "path" | "helix")
    ) || new_geometry(v)
    {
        return super::datum::FORMAT;
    }
    if v.get("type").and_then(Value::as_str) == Some("instance") && v.get("project").is_some() {
        return 6;
    }
    if v.get("type").and_then(Value::as_str) == Some("import")
        || (v.get("type").and_then(Value::as_str) == Some("instance") && v.get("import").is_some())
    {
        return 5;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("instance" | "mate" | "instance_pattern")
    ) {
        return 4;
    }
    let new = |o: &Value| o.as_str().is_none_or(|o| o == "new");
    if v.get("targets").is_some()
        || v.get("operation").is_some_and(|o| !new(o))
        || matches!(
            v.get("type").and_then(Value::as_str),
            Some("boolean" | "fillet" | "chamfer" | "hole" | "pattern")
        )
    {
        return 3;
    }
    if matches!(
        v.get("type").and_then(Value::as_str),
        Some("sketch" | "extrude" | "revolve")
    ) {
        return 2;
    }
    let mut v = v.clone();
    let literal = expression_strings(&mut v)
        .into_iter()
        .all(|s| Expr::parse(s).is_ok_and(|e| e.is_literal()));
    if literal { 1 } else { 2 }
}

/// Whether a feature's JSON names a solid by a path (a `/` in a solid
/// field) or holds a reference with `solid`: new in format 21.
fn solid_paths(v: &Value) -> bool {
    fn walk(v: &Value) -> bool {
        match v {
            Value::Object(m) => {
                (m.contains_key("solid") && m.contains_key("feature") && m.contains_key("name"))
                    || m.values().any(walk)
            }
            Value::Array(a) => a.iter().any(walk),
            _ => false,
        }
    }
    let path = |k: &str| match v.get(k) {
        Some(Value::String(s)) => s.contains('/'),
        Some(Value::Array(a)) => a
            .iter()
            .any(|x| x.as_str().is_some_and(|s| s.contains('/'))),
        _ => false,
    };
    ["targets", "target", "tools", "solid", "solids"]
        .into_iter()
        .any(path)
        || v.pointer("/tool/body")
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains('/'))
        || walk(v)
}

/// Whether a feature's JSON holds a geometric reference whose form is new
/// in format 7: `{"feature"}` or `{"feature", "entity"}` anywhere, a
/// plane as a pattern's direction, or a word as an up-to target.
fn new_geometry(v: &Value) -> bool {
    fn walk(v: &Value) -> bool {
        match v {
            Value::Object(m) => {
                let keys: Vec<&str> = m.keys().map(String::as_str).collect();
                matches!(keys.as_slice(), ["feature"] | ["entity", "feature"])
                    || m.values().any(walk)
            }
            Value::Array(a) => a.iter().any(walk),
            _ => false,
        }
    }
    let plane = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .is_some_and(|s| matches!(s, "xy" | "yz" | "zx"))
    };
    let word = |t: Option<&Value>| t.is_some_and(Value::is_string);
    walk(v)
        || (v.get("type").and_then(Value::as_str) == Some("pattern")
            && (plane("direction") || plane("direction2")))
        || word(v.pointer("/extent/target"))
        || word(v.pointer("/depth/up_to/target"))
}

/// What a feature does to its part's solids, read from its JSON alone, so
/// a feature that doesn't load (a merge conflict, a parameter that doesn't
/// evaluate) still holds its place in its solids' chains: what follows it
/// there fails through it, as it would if it had loaded and failed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Acts {
    /// Whether it founds a solid of its own.
    pub founds: bool,
    /// The solids whose chains it joins.
    pub acts_on: Vec<Id>,
    /// The bodies it consumes: a boolean's tools, a sew's sheets, a
    /// thicken's sheet.
    pub consumes: Vec<Id>,
    /// Whether it is an insert: it founds a family of solids, one per
    /// solid its source shows, keyed `<insert>/<source key>`.
    pub inserts: bool,
    /// The solids (keys; an insert's ID for every solid it founded) a
    /// `delete_body` ends.
    pub deletes: Vec<Id>,
}

/// [`Acts`] from a feature's JSON, as [`Feature::targets`] and
/// [`Feature::combine`] read a loaded one. Names that aren't IDs are left
/// out.
pub fn feature_acts(v: &Value) -> Acts {
    let ids = |v: Option<&Value>| -> Vec<Id> {
        let one = |v: &Value| v.as_str().and_then(|s| Id::solid_key(s, "feature").ok());
        match v {
            Some(Value::Array(a)) => a.iter().filter_map(one).collect(),
            Some(v) => one(v).into_iter().collect(),
            None => Vec::new(),
        }
    };
    let mut out = Acts::default();
    match v.get("type").and_then(Value::as_str) {
        Some("import" | "planar" | "fill" | "extract") => out.founds = true,
        Some("insert") => out.inserts = true,
        Some("delete_body") => out.deletes = ids(v.get("solids")),
        Some("sew") => {
            out.founds = true;
            out.consumes = ids(v.get("sheets"));
        }
        Some("thicken") => {
            if v.get("operation")
                .and_then(Value::as_str)
                .is_none_or(|o| o == "new")
            {
                out.founds = true;
            } else {
                out.acts_on = ids(v.get("targets"));
            }
            out.consumes = ids(v.pointer("/from/sheet"));
        }
        Some(
            "box" | "cylinder" | "cone" | "sphere" | "torus" | "extrude" | "revolve" | "sweep"
            | "loft",
        ) => {
            if v.get("operation")
                .is_none_or(|o| o.as_str().is_none_or(|o| o == "new"))
            {
                out.founds = true;
            } else {
                out.acts_on = ids(v.get("targets"));
            }
        }
        Some("boolean") => {
            out.acts_on = ids(v.get("target"));
            out.consumes = ids(v.get("tools"));
        }
        Some("hole" | "pattern") => out.acts_on = ids(v.get("targets")),
        Some("fillet" | "chamfer") => {
            let groups = v.get("groups").and_then(Value::as_array);
            for g in groups.into_iter().flatten() {
                let edges = g.get("edges").and_then(Value::as_array);
                let named = edges.into_iter().flatten().chain(g.get("face"));
                for id in named.flat_map(|r| {
                    let f = r.get("feature").and_then(Value::as_str);
                    let key = match (f, r.get("solid").and_then(Value::as_str)) {
                        (Some(f), Some(s)) => Id::solid_key(&format!("{f}/{s}"), "feature").ok(),
                        (Some(f), None) => Id::parse(f, "feature").ok(),
                        _ => None,
                    };
                    key.into_iter()
                }) {
                    if !out.acts_on.contains(&id) {
                        out.acts_on.push(id);
                    }
                }
            }
        }
        Some("mirror") => {
            if v.get("operation").and_then(Value::as_str) == Some("new") {
                out.founds = true;
            } else {
                out.acts_on = ids(v.get("targets"));
            }
        }
        Some("split") => {
            out.founds = v.get("keep").and_then(Value::as_str) != Some("behind");
            out.acts_on = ids(v.get("target"));
        }
        Some("base_flange") => out.founds = true,
        Some("flange" | "unfold" | "refold") => out.acts_on = ids(v.get("target")),
        Some("shell" | "offset") => out.acts_on = ids(v.get("target")),
        Some(
            "move_face" | "replace_face" | "draft" | "delete_face" | "delete_blend"
            | "resize_blend",
        ) => out.acts_on = ids(v.get("target")),
        _ => {}
    }
    out
}

/// The parameters a feature's JSON names, read from its text alone, so a
/// feature that doesn't evaluate still says what it reads.
pub fn feature_names(v: &Value) -> std::collections::BTreeSet<String> {
    let mut v = v.clone();
    expression_strings(&mut v)
        .into_iter()
        .filter_map(|s| Expr::parse(s).ok())
        .flat_map(|e| e.names())
        .collect()
}

/// Renames a parameter in a feature's JSON, in place. Returns whether
/// anything changed; strings that don't parse are left as they are.
pub fn rename_in_feature(v: &mut Value, from: &str, to: &str) -> bool {
    let mut changed = false;
    for s in expression_strings(v) {
        if let Ok(e) = Expr::parse(s)
            && e.names().contains(from)
        {
            *s = e.rename(from, to).text().to_owned();
            changed = true;
        }
    }
    changed
}

/// The message for a `type` no feature has.
fn unknown_type(s: &str) -> Error {
    Error::new(
        Code::UnknownFeatureType,
        format!("unknown feature type {s:?}; known: box, cylinder, cone, sphere, torus, sketch, extrude, revolve, boolean, fillet, chamfer, hole, pattern, instance, mate, instance_pattern, import, insert, delete_body, plane, axis, point, path, helix, sweep, loft, planar, shell, offset, mirror, split, base_flange, flange, unfold, refold, fill, extract, sew, thicken, move_face, replace_face, draft, delete_face, delete_blend, resize_blend, cosmetic_thread"),
    )
    .with("type", s)
}

impl Feature {
    /// The `type` the file gives.
    pub fn type_name(&self) -> &'static str {
        match self {
            Feature::Primitive(p) => p.ty.as_str(),
            Feature::Sketch(_) => "sketch",
            Feature::Extrude(_) => "extrude",
            Feature::Revolve(_) => "revolve",
            Feature::Boolean(_) => "boolean",
            Feature::Fillet(_) => "fillet",
            Feature::Chamfer(_) => "chamfer",
            Feature::Hole(_) => "hole",
            Feature::CosmeticThread(_) => "cosmetic_thread",
            Feature::Pattern(_) => "pattern",
            Feature::Instance(_) => "instance",
            Feature::Insert(_) => "insert",
            Feature::DeleteBody(_) => "delete_body",
            Feature::Mate(_) => "mate",
            Feature::InstancePattern(_) => "instance_pattern",
            Feature::Import(_) => "import",
            Feature::Datum(d) => d.type_name(),
            Feature::Path(p) => p.type_name(),
            Feature::Sweep(_) => "sweep",
            Feature::Loft(_) => "loft",
            Feature::Shell(_) => "shell",
            Feature::Offset(_) => "offset",
            Feature::Mirror(_) => "mirror",
            Feature::Split(_) => "split",
            Feature::BaseFlange(_) => "base_flange",
            Feature::Flange(_) => "flange",
            Feature::Unfold(_) => "unfold",
            Feature::Refold(_) => "refold",
            Feature::Planar(_) => "planar",
            Feature::Fill(_) => "fill",
            Feature::Extract(_) => "extract",
            Feature::Sew(_) => "sew",
            Feature::Thicken(_) => "thicken",
            Feature::MoveFace(_) => "move_face",
            Feature::ReplaceFace(_) => "replace_face",
            Feature::Draft(_) => "draft",
            Feature::DeleteFace(_) => "delete_face",
            Feature::DeleteBlend(_) => "delete_blend",
            Feature::ResizeBlend(_) => "resize_blend",
        }
    }

    /// Whether it founds a sheet: a surface sweep or a planar face.
    pub fn surface(&self) -> bool {
        match self {
            Feature::Extrude(e) => e.surface,
            Feature::Revolve(r) => r.surface,
            Feature::Sweep(w) => w.surface,
            Feature::Loft(l) => l.surface,
            Feature::Planar(_) => true,
            _ => false,
        }
    }

    /// Whether it may found a sheet: a surface sweep, a planar face, a
    /// fill, an extract, or a sew that doesn't make a solid. A part
    /// holding none holds no sheet, so what its features act on needs no
    /// rebuild to judge (`server::check_references`).
    pub fn may_make_sheet(&self) -> bool {
        self.surface()
            || matches!(self, Feature::Fill(_) | Feature::Extract(_))
            || matches!(self, Feature::Sew(s) if !s.solid)
    }

    /// A face tweak's topology and blends (ryucad 22); `None` for any
    /// other feature.
    pub fn retrim(&self) -> Option<super::edit::Retrim> {
        match self {
            Feature::MoveFace(m) => Some(m.retrim),
            Feature::ReplaceFace(r) => Some(r.retrim),
            Feature::Draft(d) => Some(d.retrim),
            _ => None,
        }
    }

    /// The lowest format that holds this feature.
    pub fn format(&self) -> u64 {
        // Highest first, as `feature_format`.
        if let Feature::CosmeticThread(_) = self {
            return super::thread::FORMAT;
        }
        if let Feature::Hole(h) = self
            && h.has_geometry()
        {
            return super::hole::GEOMETRY_FORMAT;
        }
        if let Feature::InstancePattern(p) = self
            && p.form.driven()
        {
            return super::mate::DRIVEN_FORMAT;
        }
        if let Feature::Hole(h) = self
            && h.has_intent()
        {
            return super::hole::INTENT_FORMAT;
        }
        if let Feature::Mate(m) = self
            && m.limits.is_some()
        {
            return super::mate::LIMITS_FORMAT;
        }
        if matches!(self, Feature::Insert(_) | Feature::DeleteBody(_))
            || solid_paths(&self.fields())
        {
            return super::insert::FORMAT;
        }
        // A tweak that may change topology or remake its blends is
        // ryucad 22's, whatever ryucad 17 wrote it at.
        if self.retrim().is_some_and(|r| !r.is_keep())
            || matches!(
                self,
                Feature::DeleteFace(_) | Feature::DeleteBlend(_) | Feature::ResizeBlend(_)
            )
        {
            return super::edit::FORMAT;
        }
        if let Feature::Split(s) = self {
            return s.format();
        }
        if matches!(
            self,
            Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_)
        ) {
            return super::surface::FORMAT;
        }
        if matches!(self, Feature::Draft(_)) {
            return super::tweak::DRAFT_FORMAT;
        }
        if matches!(self, Feature::MoveFace(_) | Feature::ReplaceFace(_)) {
            return super::tweak::FORMAT;
        }
        if matches!(self, Feature::Unfold(_) | Feature::Refold(_)) {
            return super::sheet_metal::UNFOLD_FORMAT;
        }
        if self.surface() {
            return super::sweep::SURFACE_FORMAT;
        }
        if matches!(self, Feature::BaseFlange(_) | Feature::Flange(_)) {
            return super::sheet_metal::FORMAT;
        }
        if matches!(self, Feature::Mirror(_)) {
            return super::body::MIRROR_FORMAT;
        }
        if matches!(self, Feature::Sweep(_) | Feature::Loft(_)) {
            return super::sweep::FORMAT;
        }
        if matches!(self, Feature::Shell(_) | Feature::Offset(_)) {
            return super::body::FORMAT;
        }
        if matches!(self, Feature::Datum(_) | Feature::Path(_)) || new_geometry(&self.fields()) {
            return super::datum::FORMAT;
        }
        if self.combine().is_some_and(|c| !c.is_new()) {
            return 3;
        }
        match self {
            Feature::Primitive(p) => p.format(),
            Feature::Sketch(s) => s.format(),
            // New in format 2.
            Feature::Extrude(_) | Feature::Revolve(_) => 2,
            Feature::Boolean(_)
            | Feature::Fillet(_)
            | Feature::Chamfer(_)
            | Feature::Hole(_)
            | Feature::Pattern(_) => 3,
            // An instance of another project's part says so in format 6,
            // an imported one in format 5.
            Feature::Instance(i) if i.rev.project().is_some() => 6,
            Feature::Instance(i) if i.import.is_some() => 5,
            Feature::Instance(_) | Feature::Mate(_) | Feature::InstancePattern(_) => 4,
            Feature::Import(i) => i.format(),
            Feature::Insert(_) | Feature::DeleteBody(_) => super::insert::FORMAT,
            Feature::CosmeticThread(_) => super::thread::FORMAT,
            Feature::Datum(_) => super::datum::FORMAT,
            Feature::Path(_) => super::path::FORMAT,
            Feature::Sweep(_) | Feature::Loft(_) => super::sweep::FORMAT,
            Feature::Shell(_) | Feature::Offset(_) => super::body::FORMAT,
            Feature::Mirror(_) | Feature::Split(_) => super::body::MIRROR_FORMAT,
            Feature::Planar(_) => super::sweep::SURFACE_FORMAT,
            Feature::BaseFlange(_) | Feature::Flange(_) => super::sheet_metal::FORMAT,
            Feature::Unfold(_) | Feature::Refold(_) => super::sheet_metal::UNFOLD_FORMAT,
            Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_) => {
                super::surface::FORMAT
            }
            Feature::MoveFace(_) | Feature::ReplaceFace(_) => super::tweak::FORMAT,
            Feature::Draft(_) => super::tweak::DRAFT_FORMAT,
            Feature::DeleteFace(_) => super::edit::FORMAT,
            Feature::DeleteBlend(_) | Feature::ResizeBlend(_) => super::edit::FORMAT,
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = self.fields();
        v["format"] = json!(self.format());
        v["kind"] = json!(FEATURE_KIND);
        v["type"] = json!(self.type_name());
        v
    }

    /// Its fields in a feature file, besides the header.
    fn fields(&self) -> Value {
        match self {
            Feature::Primitive(p) => p.fields(),
            Feature::Sketch(s) => s.fields(),
            Feature::Extrude(e) => e.fields(),
            Feature::Revolve(r) => r.fields(),
            Feature::Boolean(b) => b.fields(),
            Feature::Fillet(f) => f.fields(),
            Feature::Chamfer(c) => c.fields(),
            Feature::Hole(h) => h.fields(),
            Feature::CosmeticThread(c) => c.fields(),
            Feature::Pattern(p) => p.fields(),
            Feature::Instance(i) => i.fields(),
            Feature::Insert(i) => i.fields(),
            Feature::DeleteBody(d) => d.fields(),
            Feature::Mate(m) => m.fields(),
            Feature::InstancePattern(p) => p.fields(),
            Feature::Import(i) => i.fields(),
            Feature::Datum(d) => d.fields(),
            Feature::Path(p) => p.fields(),
            Feature::Sweep(w) => w.fields(),
            Feature::Loft(l) => l.fields(),
            Feature::Shell(s) => s.fields(),
            Feature::Offset(o) => o.fields(),
            Feature::Mirror(m) => m.fields(),
            Feature::Split(s) => s.fields(),
            Feature::Planar(p) => p.fields(),
            Feature::BaseFlange(b) => b.fields(),
            Feature::Flange(f) => f.fields(),
            Feature::Unfold(u) => u.fields(),
            Feature::Refold(r) => r.fields(),
            Feature::Fill(x) => x.fields(),
            Feature::Extract(x) => x.fields(),
            Feature::Sew(x) => x.fields(),
            Feature::Thicken(x) => x.fields(),
            Feature::MoveFace(m) => m.fields(),
            Feature::ReplaceFace(r) => r.fields(),
            Feature::Draft(d) => d.fields(),
            Feature::DeleteFace(x) => x.fields(),
            Feature::DeleteBlend(x) => x.fields(),
            Feature::ResizeBlend(x) => x.fields(),
        }
    }

    /// Reads and validates a feature, its expressions reading `env`. Bare
    /// numbers take `default`.
    pub fn from_json(v: &Value, default: Unit, env: &Env) -> Result<Feature> {
        let mut o = header(v, FEATURE_KIND, FEATURE_FORMAT)?;
        let ty = o.take("type")?;
        let ty = ty
            .as_str()
            .ok_or_else(|| invalid("/type", "type must be a string"))?;
        let f = match ty {
            "sketch" => Feature::Sketch(Sketch::from_fields(&mut o, default, env)?),
            "extrude" => Feature::Extrude(Extrude::from_fields(&mut o, default, env)?),
            "revolve" => Feature::Revolve(Revolve::from_fields(&mut o, env)?),
            "sweep" => Feature::Sweep(Box::new(Sweep::from_fields(&mut o, env)?)),
            "loft" => Feature::Loft(Box::new(Loft::from_fields(&mut o, env)?)),
            "planar" => Feature::Planar(Box::new(super::sweep::Planar::from_fields(&mut o)?)),
            "fill" => Feature::Fill(Box::new(Fill::from_fields(&mut o, default, env)?)),
            "extract" => Feature::Extract(Box::new(Extract::from_fields(&mut o)?)),
            "sew" => Feature::Sew(Box::new(Sew::from_fields(&mut o, default, env)?)),
            "thicken" => Feature::Thicken(Box::new(Thicken::from_fields(&mut o, default, env)?)),
            "boolean" => Feature::Boolean(Boolean::from_fields(&mut o)?),
            "fillet" => Feature::Fillet(Fillet::from_fields(&mut o, default, env)?),
            "chamfer" => Feature::Chamfer(Chamfer::from_fields(&mut o, default, env)?),
            "shell" => Feature::Shell(Box::new(Shell::from_fields(&mut o, default, env)?)),
            "offset" => Feature::Offset(Box::new(Offset::from_fields(&mut o, default, env)?)),
            "move_face" => {
                Feature::MoveFace(Box::new(MoveFace::from_fields(&mut o, default, env)?))
            }
            "replace_face" => Feature::ReplaceFace(Box::new(ReplaceFace::from_fields(&mut o)?)),
            "draft" => Feature::Draft(Box::new(Draft::from_fields(&mut o, env)?)),
            "delete_face" => Feature::DeleteFace(Box::new(DeleteFace::from_fields(&mut o)?)),
            "delete_blend" => {
                Feature::DeleteBlend(Box::new(DeleteBlend::from_fields(&mut o, default, env)?))
            }
            "resize_blend" => {
                Feature::ResizeBlend(Box::new(ResizeBlend::from_fields(&mut o, default, env)?))
            }
            "mirror" => Feature::Mirror(Box::new(Mirror::from_fields(&mut o)?)),
            "split" => Feature::Split(Box::new(Split::from_fields(&mut o)?)),
            "base_flange" => {
                Feature::BaseFlange(Box::new(BaseFlange::from_fields(&mut o, default, env)?))
            }
            "flange" => Feature::Flange(Box::new(Flange::from_fields(&mut o, default, env)?)),
            "unfold" => Feature::Unfold(Box::new(Unfold::from_fields(&mut o, default, env)?)),
            "refold" => Feature::Refold(Box::new(Refold::from_fields(&mut o)?)),
            "hole" => Feature::Hole(Box::new(Hole::from_fields(&mut o, default, env)?)),
            "cosmetic_thread" => Feature::CosmeticThread(Box::new(
                super::thread::CosmeticThread::from_fields(&mut o, default, env)?,
            )),
            "pattern" => Feature::Pattern(Pattern::from_fields(&mut o, default, env)?),
            "instance" => Feature::Instance(Instance::from_fields(&mut o)?),
            "insert" => Feature::Insert(Box::new(super::insert::Insert::from_fields(
                &mut o, default, env,
            )?)),
            "import" => Feature::Import(Import::from_fields(&mut o)?),
            "delete_body" => {
                Feature::DeleteBody(Box::new(super::body::DeleteBody::from_fields(&mut o)?))
            }
            "plane" | "axis" | "point" => {
                Feature::Datum(Box::new(Datum::from_fields(ty, &mut o, default, env)?))
            }
            "path" | "helix" => Feature::Path(Box::new(PathFeature::from_fields(
                ty, &mut o, default, env,
            )?)),
            "mate" => Feature::Mate(Box::new(Mate::from_fields(&mut o, default, env)?)),
            "instance_pattern" => Feature::InstancePattern(Box::new(InstancePattern::from_fields(
                &mut o, default, env,
            )?)),
            _ => {
                let ty = PrimitiveType::parse(ty).map_err(|e| e.with("pointer", "/type"))?;
                Feature::Primitive(Primitive::from_fields(ty, &mut o, default, env)?)
            }
        };
        o.finish()?;
        Ok(f)
    }

    /// The primitive, if it is one.
    pub fn primitive(&self) -> Option<&Primitive> {
        match self {
            Feature::Primitive(p) => Some(p),
            _ => None,
        }
    }

    /// What a body-making feature does with its body; `None` for a
    /// feature that makes none.
    pub fn combine(&self) -> Option<&Combine> {
        match self {
            Feature::Primitive(p) => Some(&p.combine),
            Feature::Extrude(e) => Some(&e.combine),
            Feature::Revolve(r) => Some(&r.combine),
            Feature::Sweep(w) => Some(&w.combine),
            Feature::Loft(l) => Some(&l.combine),
            // An import always founds its own solid, a planar face, a
            // fill and an extract their own sheet.
            Feature::Import(_) | Feature::Planar(_) | Feature::Fill(_) | Feature::Extract(_) => {
                Some(&NEW)
            }
            // A sew founds the body it sews.
            Feature::Sew(_) => Some(&NEW),
            Feature::Thicken(t) => Some(&t.combine),
            // A base flange always founds its own solid.
            Feature::BaseFlange(_) => Some(&NEW),
            Feature::Mirror(m) => Some(&m.combine),
            Feature::Sketch(_)
            | Feature::Boolean(_)
            | Feature::Fillet(_)
            | Feature::Chamfer(_)
            | Feature::Hole(_)
            | Feature::CosmeticThread(_)
            | Feature::Pattern(_)
            | Feature::Instance(_)
            | Feature::Insert(_)
            | Feature::DeleteBody(_)
            | Feature::Mate(_)
            | Feature::InstancePattern(_)
            | Feature::Datum(_)
            | Feature::Path(_)
            | Feature::Shell(_)
            | Feature::Offset(_)
            | Feature::Split(_)
            | Feature::Flange(_)
            | Feature::Unfold(_)
            | Feature::Refold(_)
            | Feature::MoveFace(_)
            | Feature::ReplaceFace(_)
            | Feature::Draft(_)
            | Feature::DeleteFace(_)
            | Feature::DeleteBlend(_)
            | Feature::ResizeBlend(_) => None,
        }
    }

    /// The solids it acts on, by their founders, in the order it names
    /// them: an operation's, a hole's or a pattern's targets, a boolean's
    /// target then tools, a sew's sheets, a thicken's targets then its sheet, a shell's or offset's target, or the solids a
    /// fillet's or chamfer's references name, each once (one, unless the
    /// file mixes them).
    pub fn targets(&self) -> Vec<&Id> {
        match self {
            Feature::Boolean(b) => b.solids().collect(),
            Feature::Hole(h) => h.targets.iter().collect(),
            Feature::Pattern(p) => p.targets.iter().collect(),
            Feature::Fillet(_) | Feature::Chamfer(_) => {
                let mut out: Vec<&Id> = Vec::new();
                for (_, r) in self.references() {
                    if !out.contains(&&r.feature) {
                        out.push(&r.feature);
                    }
                }
                out
            }
            Feature::Shell(s) => vec![&s.target],
            Feature::Offset(o) => vec![&o.target],
            Feature::Split(s) => vec![&s.target],
            Feature::Flange(f) => vec![&f.target],
            Feature::Unfold(u) => vec![&u.target],
            Feature::Refold(r) => vec![&r.target],
            // The sheets a sew consumes, as a boolean's tools are its.
            Feature::Sew(s) => s.sheets.iter().collect(),
            // A thicken's targets, then the sheet it consumes.
            Feature::Thicken(t) => t.combine.targets.iter().chain(t.sheet()).collect(),
            Feature::MoveFace(m) => vec![&m.target],
            Feature::ReplaceFace(r) => vec![&r.target],
            Feature::Draft(d) => vec![&d.target],
            Feature::DeleteFace(x) => vec![&x.target],
            Feature::DeleteBlend(x) => vec![&x.target],
            Feature::ResizeBlend(x) => vec![&x.target],
            f => f
                .combine()
                .map_or(Vec::new(), |c| c.targets.iter().collect()),
        }
    }

    /// Whether it is an insert, founding a family of solids.
    pub fn inserts(&self) -> bool {
        matches!(self, Feature::Insert(_))
    }

    /// The insert, if it is one.
    pub fn insert(&self) -> Option<&super::insert::Insert> {
        match self {
            Feature::Insert(i) => Some(i),
            _ => None,
        }
    }

    /// The solids a `delete_body` ends, by key; none for anything else.
    pub fn deletes(&self) -> &[Id] {
        match self {
            Feature::DeleteBody(d) => &d.solids,
            _ => &[],
        }
    }

    /// Whether it founds a solid of its own, named by its ID: a body-making
    /// feature whose operation is `new` (an import always), or a split
    /// keeping both sides, which also acts on its target.
    pub fn founds(&self) -> bool {
        match self {
            Feature::Split(s) => s.keep == super::body::Keep::Both,
            f => f.combine().is_some_and(Combine::is_new),
        }
    }

    /// The bodies it consumes, by founder: a boolean's tools, a sew's
    /// sheets, a thicken's sheet. Their chains end here: nothing acts on
    /// them or refers to them after it.
    pub fn consumes(&self) -> Vec<&Id> {
        match self {
            Feature::Boolean(b) => b.tools.iter().collect(),
            Feature::Sew(s) => s.sheets.iter().collect(),
            Feature::Thicken(t) => t.sheet().into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// The body it reads whole without changing it, as it sees it: the
    /// solid or sheet a mirror copies, the body a split cuts with; `None`
    /// for anything else.
    pub fn source(&self) -> Option<&Id> {
        match self {
            Feature::Mirror(m) => Some(&m.solid),
            Feature::Split(s) => s.tool_body(),
            _ => None,
        }
    }

    /// The features a pattern repeats, or the instances an instance
    /// pattern copies; none for anything else.
    pub fn seeds(&self) -> &[Id] {
        match self {
            Feature::Pattern(p) => &p.features,
            Feature::InstancePattern(p) => &p.instances,
            _ => &[],
        }
    }

    /// The connectors it reads, each with its role (`a`, `b`,
    /// `direction`, `direction2`, `axis`): a mate's and an instance
    /// pattern's.
    pub fn connectors(&self) -> Vec<(&'static str, &super::mate::Connector)> {
        match self {
            Feature::Mate(m) => vec![("a", &m.a), ("b", &m.b)],
            Feature::InstancePattern(p) => p.connectors(),
            _ => Vec::new(),
        }
    }

    /// The mate, if it is one.
    pub fn mate(&self) -> Option<&Mate> {
        match self {
            Feature::Mate(m) => Some(m),
            _ => None,
        }
    }

    /// The instance pattern, if it is one.
    pub fn instance_pattern(&self) -> Option<&InstancePattern> {
        match self {
            Feature::InstancePattern(p) => Some(p),
            _ => None,
        }
    }

    /// The pattern, if it is one.
    pub fn pattern(&self) -> Option<&Pattern> {
        match self {
            Feature::Pattern(p) => Some(p),
            _ => None,
        }
    }

    /// The hole, if it is one.
    pub fn hole(&self) -> Option<&Hole> {
        match self {
            Feature::Hole(h) => Some(h),
            _ => None,
        }
    }

    /// The boolean, if it is one.
    pub fn boolean(&self) -> Option<&Boolean> {
        match self {
            Feature::Boolean(b) => Some(b),
            _ => None,
        }
    }

    /// The import, if it is one.
    pub fn import(&self) -> Option<&Import> {
        match self {
            Feature::Import(i) => Some(i),
            _ => None,
        }
    }

    /// The instance, if it is one.
    pub fn instance(&self) -> Option<&Instance> {
        match self {
            Feature::Instance(i) => Some(i),
            _ => None,
        }
    }

    /// The sketch, if it is one.
    pub fn sketch(&self) -> Option<&Sketch> {
        match self {
            Feature::Sketch(s) => Some(s),
            _ => None,
        }
    }

    /// Its references to other features' faces, edges and vertices, each
    /// with its role in the feature (`plane`, `x`, `target`, `axis`,
    /// `entities.<id>`, `groups.<i>.edges.<j>`, `groups.<i>.face`,
    /// `depth`, `direction`, `direction2`, `plane`, a shell's `open.<i>`
    /// a shell's or offset's `faces.<i>.faces.<j>`, and a tweak's
    /// `faces.<i>`, `groups.<i>.faces.<j>`, `direction`, `axis` and
    /// `groups.<i>.with`), in a fixed order.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        match self {
            Feature::Sketch(s) => s.references(),
            Feature::Fillet(f) => f.references(),
            Feature::Chamfer(c) => c.references(),
            Feature::Shell(s) => s.references(),
            // A plane that is a face, or the faces it cuts with.
            Feature::Split(s) => s
                .references()
                .into_iter()
                .chain(
                    self.geometry()
                        .into_iter()
                        .filter_map(|(k, g)| Some((k, g.reference()?))),
                )
                .collect(),
            Feature::Offset(o) => o.references(),
            Feature::CosmeticThread(c) => c.references(),
            Feature::Flange(f) => f.references(),
            Feature::Unfold(u) => u.references(),
            Feature::Refold(r) => r.references(),
            Feature::Extract(x) => x.references(),
            Feature::Thicken(x) => x.references(),
            // A fill's edge sides, then the edges its curve sides name.
            Feature::Fill(x) => x
                .references()
                .into_iter()
                .chain(
                    x.geometry()
                        .into_iter()
                        .filter_map(|(k, g)| Some((k, g.reference()?))),
                )
                .collect(),
            Feature::MoveFace(m) => m.references(),
            Feature::ReplaceFace(r) => r.references(),
            Feature::Draft(d) => d.references(),
            Feature::DeleteFace(x) => x.references(),
            Feature::DeleteBlend(x) => x.references(),
            Feature::ResizeBlend(x) => x.references(),
            _ => self
                .geometry()
                .into_iter()
                .filter_map(|(k, g)| Some((k, g.reference()?)))
                .collect(),
        }
    }

    /// Its geometric references, each with its role (`plane`, `origin`,
    /// `x`, `entities.<id>`, `target`, `axis`, `depth`, `direction`,
    /// `direction2`, a datum's fields), in a fixed order. A fillet's or
    /// chamfer's edges and faces, and a sketch's face, are
    /// [`Feature::references`] alone.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        match self {
            Feature::Sketch(s) => s.geometry(),
            Feature::Hole(h) => h.geometry(),
            Feature::Pattern(p) => p.geometry(),
            Feature::Datum(d) => d.geometry(),
            Feature::Path(p) => p.geometry(),
            Feature::Sweep(w) => w.geometry(),
            Feature::Loft(l) => l.geometry(),
            Feature::Extrude(e) => match &e.extent {
                super::sweep::Extent::UpTo { target, .. } => vec![("target".to_owned(), target)],
                _ => Vec::new(),
            },
            Feature::Revolve(r) => match &r.axis {
                super::sweep::RevolveAxis::Ref(g) => vec![("axis".to_owned(), g)],
                _ => Vec::new(),
            },
            Feature::Mirror(m) => vec![("plane".to_owned(), &m.plane)],
            Feature::Split(s) => s
                .plane()
                .map(|p| ("plane".to_owned(), p))
                .into_iter()
                .collect(),
            Feature::Planar(p) => p.geometry(),
            Feature::Fill(x) => x.geometry(),
            Feature::MoveFace(m) => m.geometry(),
            Feature::ReplaceFace(r) => r.geometry(),
            Feature::Draft(d) => d.geometry(),
            _ => Vec::new(),
        }
    }

    /// The features it reads through its references, each once, in the
    /// order it names them: an entity's solid, a datum, a sketch.
    pub fn geometry_reads(&self) -> Vec<&Id> {
        let mut out: Vec<&Id> = Vec::new();
        let named = self
            .geometry()
            .into_iter()
            .filter_map(|(_, g)| g.feature())
            .chain(self.references().into_iter().map(|(_, r)| &r.feature));
        for id in named {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }

    /// The sketches it sweeps, skins or drills at: an extrude's,
    /// revolve's or hole's, a sweep's whose profile is a sketch's, a
    /// loft's sections' sketches in order.
    pub fn profiles(&self) -> Vec<&Id> {
        match self {
            Feature::Extrude(e) => vec![&e.sketch],
            Feature::Revolve(r) => vec![&r.sketch],
            Feature::Hole(h) => vec![&h.sketch],
            Feature::Sweep(w) => w.profile.sketch().into_iter().collect(),
            Feature::Loft(l) => l.sketches(),
            Feature::Planar(p) => p.profile.sketch().into_iter().collect(),
            Feature::BaseFlange(b) => vec![&b.sketch],
            _ => Vec::new(),
        }
    }

    /// The loft, if it is one.
    pub fn loft(&self) -> Option<&Loft> {
        match self {
            Feature::Loft(l) => Some(l),
            _ => None,
        }
    }

    /// The sweep, if it is one.
    pub fn sweep(&self) -> Option<&Sweep> {
        match self {
            Feature::Sweep(w) => Some(w),
            _ => None,
        }
    }

    /// The base flange, if it is one.
    pub fn base_flange(&self) -> Option<&BaseFlange> {
        match self {
            Feature::BaseFlange(b) => Some(b),
            _ => None,
        }
    }

    /// The edge flange, if it is one.
    pub fn flange(&self) -> Option<&Flange> {
        match self {
            Feature::Flange(f) => Some(f),
            _ => None,
        }
    }

    /// The unfold, if it is one.
    pub fn unfold(&self) -> Option<&Unfold> {
        match self {
            Feature::Unfold(u) => Some(u),
            _ => None,
        }
    }

    /// The refold, if it is one.
    pub fn refold(&self) -> Option<&Refold> {
        match self {
            Feature::Refold(r) => Some(r),
            _ => None,
        }
    }
}

impl Primitive {
    fn format(&self) -> u64 {
        let literal = self.params.values().all(Quantity::is_literal)
            && self.placement.origin.iter().all(Quantity::is_literal);
        if literal { 1 } else { 2 }
    }

    /// Its fields in a feature file, besides the header.
    fn fields(&self) -> Value {
        let params: Map<String, Value> = self
            .params
            .iter()
            .map(|(k, q)| (k.clone(), Value::String(q.text())))
            .collect();
        let mut v = json!({"params": params, "placement": self.placement.to_json()});
        self.combine.write(&mut v);
        v
    }

    fn from_fields(ty: PrimitiveType, o: &mut Obj, default: Unit, env: &Env) -> Result<Primitive> {
        let params = o.take("params")?;
        let params = Primitive::params_from_json(ty, &params, default, env)?;
        let placement = Placement::from_json(&o.take("placement")?, default, "/placement", env)?;
        Ok(Primitive {
            ty,
            params,
            placement,
            combine: Combine::from_fields(o)?,
        })
    }

    /// Validates a params object for `ty`: every required parameter present
    /// and positive, optional ones non-negative, no others.
    pub fn params_from_json(
        ty: PrimitiveType,
        v: &Value,
        default: Unit,
        env: &Env,
    ) -> Result<BTreeMap<String, Quantity>> {
        let map = v
            .as_object()
            .ok_or_else(|| invalid("/params", "params must be an object"))?;
        let mut params = BTreeMap::new();
        for (k, v) in map {
            let at = format!("/params/{k}");
            let required = ty.required().contains(&k.as_str());
            if !required && !ty.optional().contains(&k.as_str()) {
                return Err(param_error(ty, k, &at, "is not a parameter of"));
            }
            let q = Quantity::from_json_in(v, default, &|n| env.get(n))
                .map_err(|e| e.with("pointer", &at))?;
            let m = q.meters();
            if (required && m <= 0.0) || m < 0.0 {
                let want = if required {
                    "must be greater than zero for"
                } else {
                    "must not be negative for"
                };
                return Err(param_error(ty, k, &at, want).with("value", q.text()));
            }
            params.insert(k.clone(), q);
        }
        if let Some(missing) = ty.required().iter().find(|k| !params.contains_key(**k)) {
            return Err(param_error(ty, missing, "/params", "is required for"));
        }
        Ok(params)
    }

    /// A parameter in meters; 0 for an absent optional one.
    pub fn meters(&self, name: &str) -> f64 {
        self.params.get(name).map_or(0.0, Quantity::meters)
    }
}

fn param_error(ty: PrimitiveType, name: &str, at: &str, what: &str) -> Error {
    let known = [ty.required(), ty.optional()].concat().join(", ");
    Error::new(
        Code::InvalidParam,
        format!(
            "{name} {what} a {}; its parameters are {known}",
            ty.as_str()
        ),
    )
    .with("param", name)
    .with("pointer", at)
}

/// A JSON number for `x`: an integer when `x` is one, so `[0, 0, 1]`
/// stays `[0, 0, 1]`.
pub fn number(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        json!(x as i64)
    } else {
        json!(x)
    }
}

#[doc(hidden)]
pub use ryu_doc::files::{Obj, invalid};

/// Checks `kind` and `format` (at most `newest`), and returns the
/// remaining fields: [`ryu_doc::files::header_in`] naming ryucad. Each
/// later format adds its migration where the kind is read.
#[doc(hidden)]
pub fn header<'a>(v: &'a Value, kind: &str, newest: u64) -> Result<Obj<'a>> {
    ryu_doc::files::header_in(v, kind, newest, "ryucad")
}

fn triple<T>(v: &Value, at: &str, item: impl Fn(&Value, &str) -> Result<T>) -> Result<[T; 3]> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or_else(|| invalid(at, "expected an array of three"))?;
    Ok([
        item(&a[0], &format!("{at}/0"))?,
        item(&a[1], &format!("{at}/1"))?,
        item(&a[2], &format!("{at}/2"))?,
    ])
}

fn vector(v: &Value, at: &str) -> Result<[f64; 3]> {
    triple(v, at, |v, at| {
        v.as_f64().ok_or_else(|| invalid(at, "expected a number"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::canonical;

    #[test]
    fn shells_and_offsets_are_format_9_and_act_on_their_target() {
        let n = "0123456789abcdef0123456789abcdef";
        let base = Id::parse("base", "feature").unwrap();
        let v = json!({"format": 9, "kind": FEATURE_KIND, "type": "shell", "target": "base",
                       "thickness": "2 mm", "open": [{"feature": "base", "name": n}]});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!((f.type_name(), f.format()), ("shell", 9));
        assert_eq!(feature_format(&v), 9);
        assert_eq!(f.to_json(), v);
        assert_eq!(f.targets(), [&base]);
        assert!(f.combine().is_none());
        assert_eq!(f.references().len(), 1);
        assert_eq!(
            feature_acts(&v),
            Acts {
                founds: false,
                acts_on: vec![base.clone()],
                consumes: vec![],
                ..Default::default()
            }
        );
        let v = json!({"format": 9, "kind": FEATURE_KIND, "type": "offset", "target": "base",
                       "distance": "wall"});
        assert_eq!(feature_format(&v), 9);
        assert_eq!(
            expression_fields(&v),
            [("/distance".to_owned(), "wall".to_owned())]
        );
        let s = json!({"format": 9, "kind": FEATURE_KIND, "type": "shell", "target": "base",
                       "thickness": "t",
                       "faces": [{"thickness": "2 * t", "faces": [{"feature": "base", "name": n}]}]});
        assert_eq!(
            expression_fields(&s),
            [
                ("/faces/0/thickness".to_owned(), "2 * t".to_owned()),
                ("/thickness".to_owned(), "t".to_owned())
            ]
        );
    }

    #[test]
    fn sheet_metal_features_are_format_11_and_say_what_they_do_to_solids() {
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        let v = json!({"format": 11, "kind": FEATURE_KIND, "type": "base_flange", "sketch": "s",
                       "thickness": "1.5 mm", "radius": "1 mm",
                       "allowance": {"k_factor": "0.44"}});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!((f.type_name(), f.format()), ("base_flange", 11));
        assert_eq!(feature_format(&v), 11);
        assert_eq!(f.to_json(), v);
        assert!(f.founds() && f.targets().is_empty());
        assert_eq!(f.profiles(), [&id("s")]);
        assert_eq!(
            feature_acts(&v),
            Acts {
                founds: true,
                ..Acts::default()
            }
        );
        let n = "0123456789abcdef0123456789abcdef";
        let m = "fedcba9876543210fedcba9876543210";
        let v = json!({"format": 11, "kind": FEATURE_KIND, "type": "flange", "target": "plate",
                       "edges": [{"feature": "plate", "name": n}], "angle": "90 deg",
                       "length": "20 mm",
                       "ends": [{"vertex": {"feature": "plate", "name": m}, "setback": "5 mm"}]});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!((f.type_name(), f.format()), ("flange", 11));
        assert!(!f.founds() && f.combine().is_none());
        assert_eq!(f.targets(), [&id("plate")]);
        let roles: Vec<String> = f.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["edges.0", "ends.0.vertex"]);
        assert_eq!(f.geometry_reads(), [&id("plate")]);
        assert_eq!(
            feature_acts(&v),
            Acts {
                founds: false,
                acts_on: vec![id("plate")],
                consumes: vec![],
                ..Default::default()
            }
        );
        // Every quantity is an expression an equations table lists.
        let fields: Vec<String> = expression_fields(&v).into_iter().map(|(p, _)| p).collect();
        assert_eq!(fields, ["/angle", "/ends/0/setback", "/length"]);
        let b = json!({"format": 11, "kind": FEATURE_KIND, "type": "base_flange", "sketch": "s",
                       "thickness": "t", "radius": "r", "depth": "40 mm",
                       "allowance": {"deduction": "2.4 mm"},
                       "relief": {"kind": "obround", "width": "1 mm", "depth": "1.5 mm"}});
        let fields: Vec<String> = expression_fields(&b).into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            fields,
            [
                "/allowance/deduction",
                "/depth",
                "/radius",
                "/relief/depth",
                "/relief/width",
                "/thickness"
            ]
        );
        assert_eq!(
            feature_names(&b).into_iter().collect::<Vec<_>>(),
            ["r", "t"]
        );
    }

    #[test]
    fn unfolds_and_refolds_are_format_14_and_act_on_their_target() {
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        let n = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": 14, "kind": FEATURE_KIND, "type": "unfold", "target": "tray",
                       "fixed": {"feature": "plate", "name": n},
                       "allowance": {"k_factor": "0.4"},
                       "bends": [{"bend": {"feature": "plate", "name": n}, "deduction": "2 mm"}]});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!((f.type_name(), f.format()), ("unfold", 14));
        assert_eq!(feature_format(&v), 14);
        assert_eq!(f.to_json(), v);
        assert!(!f.founds() && f.combine().is_none());
        assert_eq!(f.targets(), [&id("tray")]);
        assert_eq!(f.geometry_reads(), [&id("plate")]);
        assert_eq!(
            feature_acts(&v),
            Acts {
                founds: false,
                acts_on: vec![id("tray")],
                consumes: vec![],
                ..Default::default()
            }
        );
        let fields: Vec<String> = expression_fields(&v).into_iter().map(|(p, _)| p).collect();
        assert_eq!(fields, ["/allowance/k_factor", "/bends/0/deduction"]);
        let v = json!({"format": 14, "kind": FEATURE_KIND, "type": "refold", "target": "tray",
                       "unfold": "u1"});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!((f.type_name(), f.format()), ("refold", 14));
        assert_eq!(f.to_json(), v);
        assert_eq!(f.targets(), [&id("tray")]);
        assert_eq!(f.refold().map(|r| &r.unfold), Some(&id("u1")));
        assert_eq!(
            feature_acts(&v),
            Acts {
                founds: false,
                acts_on: vec![id("tray")],
                consumes: vec![],
                ..Default::default()
            }
        );
        const { assert!(FEATURE_FORMAT >= super::super::sheet_metal::UNFOLD_FORMAT) };
        const { assert!(FEATURE_FORMAT >= super::super::tweak::FORMAT) };
    }

    #[test]
    fn a_split_by_a_tool_is_format_19_and_reads_its_tool() {
        use super::super::body::{MIRROR_FORMAT, TOOL_FORMAT};
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        let header = |mut v: Value| {
            v["kind"] = json!(FEATURE_KIND);
            v["format"] = json!(TOOL_FORMAT);
            v["type"] = json!("split");
            v
        };
        let read = |v: &Value| Feature::from_json(v, Unit::Mm, &Env::empty()).unwrap();
        let plane = read(&header(json!({"target": "h", "plane": "xy"})));
        assert_eq!(plane.format(), MIRROR_FORMAT);
        assert_eq!(plane.geometry().len(), 1);
        assert!(plane.source().is_none());
        let v = header(json!({"target": "h", "tool": {"body": "parting"}}));
        let body = read(&v);
        assert_eq!(
            (body.format(), feature_format(&v)),
            (TOOL_FORMAT, TOOL_FORMAT)
        );
        assert_eq!(body.to_json(), v);
        assert!(body.geometry().is_empty());
        assert_eq!(body.source(), Some(&id("parting")));
        assert_eq!(body.targets(), vec![&id("h")]);
        let faces = read(&header(json!({"target": "h", "tool": {"faces": [
            {"feature": "base", "name": "0123456789abcdef0123456789abcdef"}]}})));
        assert_eq!(faces.references()[0].0, "tool.faces.0");
        assert_eq!(faces.geometry_reads(), vec![&id("base")]);
        const { assert!(FEATURE_FORMAT >= TOOL_FORMAT) };
    }

    #[test]
    fn mirrors_and_splits_are_format_10_and_say_what_they_do_to_solids() {
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        let v = json!({"format": 10, "kind": FEATURE_KIND, "type": "mirror", "solid": "half",
                       "plane": {"feature": "mid"}, "operation": "join", "targets": ["half"]});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!((f.type_name(), f.format()), ("mirror", 10));
        assert_eq!(feature_format(&v), 10);
        assert_eq!(f.to_json(), v);
        assert_eq!(f.targets(), [&id("half")]);
        assert_eq!(f.source(), Some(&id("half")));
        assert!(!f.founds());
        assert_eq!(f.geometry_reads(), [&id("mid")]);
        assert_eq!(
            feature_acts(&v),
            Acts {
                founds: false,
                acts_on: vec![id("half")],
                consumes: vec![],
                ..Default::default()
            }
        );
        // A new copy founds a solid and acts on nothing.
        let v = json!({"format": 10, "kind": FEATURE_KIND, "type": "mirror", "solid": "half",
                       "plane": "yz", "operation": "new"});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert!(f.founds() && f.targets().is_empty());
        assert_eq!(
            feature_acts(&v),
            Acts {
                founds: true,
                ..Acts::default()
            }
        );
        // A split acts on its target and, keeping both sides, founds.
        let v = json!({"format": 10, "kind": FEATURE_KIND, "type": "split", "target": "h",
                       "plane": {"feature": "parting"}});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!((f.type_name(), f.format()), ("split", 10));
        assert!(f.founds());
        assert_eq!(f.targets(), [&id("h")]);
        assert!(f.combine().is_none() && f.source().is_none());
        assert_eq!(
            feature_acts(&v),
            Acts {
                founds: true,
                acts_on: vec![id("h")],
                consumes: vec![],
                ..Default::default()
            }
        );
        let mut v = v;
        v["keep"] = json!("behind");
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert!(!f.founds());
        assert!(!feature_acts(&v).founds);
        // A plane that is a face of a solid is a reference, as any.
        let n = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": 10, "kind": FEATURE_KIND, "type": "split", "target": "h",
                       "plane": {"feature": "h", "name": n}});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(f.references().len(), 1);
        assert_eq!(f.references()[0].0, "plane");
    }
    #[test]
    fn what_a_feature_does_to_solids_is_read_from_its_text() {
        use serde_json::json;
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        let acts = |v: Value| feature_acts(&v);
        assert!(acts(json!({"type": "box", "params": {"x": "w"}})).founds);
        assert!(acts(json!({"type": "extrude", "operation": "new"})).founds);
        let cut = acts(json!({"type": "cylinder", "operation": "cut", "targets": ["b", "C"]}));
        assert_eq!((cut.founds, cut.acts_on), (false, vec![id("b")]));
        let b = acts(json!({"type": "boolean", "target": "a", "tools": ["t"]}));
        assert_eq!((b.acts_on, b.consumes), (vec![id("a")], vec![id("t")]));
        let f = acts(json!({"type": "chamfer", "groups": [
            {"edges": [{"feature": "a", "name": "x"}, {"feature": "a", "name": "y"}],
             "face": {"feature": "b", "name": "z"}}]}));
        assert_eq!(f.acts_on, [id("a"), id("b")]);
        assert_eq!(acts(json!({"type": "sketch"})), Acts::default());
    }

    /// The primitive a feature holds.
    fn prim(f: &Feature) -> &Primitive {
        f.primitive().expect("a primitive")
    }

    fn cylinder_json() -> Value {
        json!({
            "format": 1,
            "kind": "ryucad.feature",
            "params": {"height": "40 mm", "radius": 12.5},
            "placement": {"origin": ["0 mm", "0", "10 mm"], "x": [1, 0, 0], "z": [0, 0, 1]},
            "type": "cylinder"
        })
    }

    #[test]
    fn manifest_round_trip() {
        let mut m = Manifest::new(Unit::In);
        let v = m.to_json();
        assert_eq!(
            v,
            json!({"default_unit": "in", "format": 1, "kind": "ryucad.project"})
        );
        assert_eq!(Manifest::from_json(&v).unwrap(), m);
        // A project ID needs format 2.
        m.project = Some(ProjectId::from_bytes([7; 16]));
        let v = m.to_json();
        assert_eq!(
            v,
            json!({"default_unit": "in", "format": 2, "kind": "ryucad.project",
                   "project": "07".repeat(16)})
        );
        assert_eq!(Manifest::from_json(&v).unwrap(), m);
        let bad = json!({"default_unit": "in", "format": 2, "kind": "ryucad.project",
                         "project": "x"});
        let e = Manifest::from_json(&bad).unwrap_err();
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidId, &json!("/project"))
        );
        // Dependencies and a fork's origin need format 3.
        let lib = ProjectId::from_bytes([8; 16]);
        m.dependencies.insert(
            lib.clone(),
            crate::doc::deps::Dependency {
                name: "fasteners".into(),
                url: "https://github.com/acme/fasteners.git".into(),
                path: "".into(),
            },
        );
        let v = m.to_json();
        assert_eq!(v["format"], 3);
        assert_eq!(manifest_format(&v), 3);
        assert_eq!(
            v["dependencies"][lib.as_str()],
            json!({"name": "fasteners", "path": "", "url": "https://github.com/acme/fasteners.git"})
        );
        assert_eq!(Manifest::from_json(&v).unwrap(), m);
        m.dependencies.clear();
        m.forked_from = Some(crate::doc::deps::ForkedFrom {
            project: lib,
            commit: "0".repeat(40),
        });
        let v = m.to_json();
        assert_eq!((v["format"].as_u64(), manifest_format(&v)), (Some(3), 3));
        assert_eq!(Manifest::from_json(&v).unwrap(), m);
        m.forked_from = None;
        assert_eq!(manifest_format(&m.to_json()), 2);
    }

    #[test]
    fn part_round_trip() {
        let p = PartFile::new(vec![
            Id::parse("b", "feature").unwrap(),
            Id::parse("a", "feature").unwrap(),
        ]);
        let v = p.to_json();
        assert_eq!(v["features"], json!(["b", "a"]));
        assert_eq!(v["format"], 1);
        assert!(v.get("part_kind").is_none());
        assert_eq!(part_format(&v), 1);
        assert_eq!(PartFile::from_json(&v).unwrap(), p);
    }

    #[test]
    fn assemblies_are_part_format_2() {
        let p = PartFile {
            kind: PartKind::Assembly,
            features: vec![Id::parse("i1", "feature").unwrap()],
            items: Items::new(),
        };
        let v = p.to_json();
        assert_eq!(v["part_kind"], "assembly");
        assert_eq!(v["format"], 2);
        assert_eq!(part_format(&v), 2);
        assert_eq!(PartFile::from_json(&v).unwrap(), p);
        let bad = json!({"features": [], "format": 2, "kind": "ryucad.part", "part_kind": "part"});
        let e = PartFile::from_json(&bad).unwrap_err();
        assert_eq!(e.details["pointer"], "/part_kind");
        let newer = json!({"features": [], "format": 5, "kind": "ryucad.part"});
        assert_eq!(
            PartFile::from_json(&newer).unwrap_err().code,
            Code::FormatTooNew
        );
    }

    #[test]
    fn item_numbers_are_part_format_3() {
        let id = |s: &str| Id::parse(s, "part").unwrap();
        let p = PartFile {
            kind: PartKind::Assembly,
            features: vec![Id::parse("i1", "feature").unwrap()],
            items: [(id("bolt"), 3), (id("plate"), 1)].into_iter().collect(),
        };
        let v = p.to_json();
        assert_eq!(v["items"], json!({"bolt": 3, "plate": 1}));
        assert_eq!(v["format"], 3);
        assert_eq!(part_format(&v), 3);
        assert_eq!(PartFile::from_json(&v).unwrap(), p);
        // Another project's part has a key of its own, in format 4.
        let lib = ProjectId::from_bytes([7; 16]);
        let mut foreign = p.clone();
        foreign.items.insert(Id::foreign(&lib, &id("bolt")), 2);
        let v4 = foreign.to_json();
        assert_eq!(v4["items"][format!("{lib}:bolt")], 2);
        assert_eq!((v4["format"].as_u64(), part_format(&v4)), (Some(4), 4));
        assert_eq!(PartFile::from_json(&v4).unwrap(), foreign);
        assert_eq!(
            Id::foreign(&lib, &id("bolt")).foreign_of(),
            Some((lib.clone(), id("bolt")))
        );
        assert_eq!(id("bolt").foreign_of(), None);
        let rev = crate::doc::assembly::Rev::Foreign {
            project: lib,
            label: "A".into(),
        };
        assert_eq!(
            item_key(&id("bolt"), &rev),
            Id::foreign(rev.project().unwrap(), &id("bolt"))
        );
        // Shared numbers read: they are the BOM's fault to report.
        let shared = json!({"features": [], "format": 3, "items": {"a": 2, "b": 2},
                            "kind": "ryucad.part", "part_kind": "assembly"});
        assert_eq!(PartFile::from_json(&shared).unwrap().items.len(), 2);
        let bad = |items: Value, part_kind: Option<&str>| {
            let mut v = json!({"features": [], "format": 3, "items": items, "kind": "ryucad.part"});
            if let Some(k) = part_kind {
                v["part_kind"] = json!(k);
            }
            PartFile::from_json(&v).unwrap_err().details["pointer"].clone()
        };
        assert_eq!(bad(json!({"a": 1}), None), "/items");
        assert_eq!(bad(json!([1]), Some("assembly")), "/items");
        assert_eq!(bad(json!({"a": 0}), Some("assembly")), "/items/a");
        assert_eq!(bad(json!({"a": 100000}), Some("assembly")), "/items/a");
        assert_eq!(bad(json!({"a": 1.5}), Some("assembly")), "/items/a");
        assert_eq!(bad(json!({"A!": 1}), Some("assembly")), "/items/A!");
        // An assembly with no items stays at format 2.
        let empty = PartFile {
            items: Items::new(),
            ..p
        };
        assert_eq!(empty.to_json()["format"], 2);
    }

    #[test]
    fn feature_reads_and_writes_explicit_units() {
        let f = Feature::from_json(&cylinder_json(), Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(prim(&f).ty, PrimitiveType::Cylinder);
        assert_eq!(prim(&f).meters("radius"), 0.0125);
        assert_eq!(
            canonical::to_string(&f.to_json()),
            "{\n  \"format\": 1,\n  \"kind\": \"ryucad.feature\",\n  \"params\": {\n    \"height\": \"40 mm\",\n    \"radius\": \"12.5 mm\"\n  },\n  \"placement\": {\n    \"origin\": [\n      \"0 mm\",\n      \"0 mm\",\n      \"10 mm\"\n    ],\n    \"x\": [\n      1,\n      0,\n      0\n    ],\n    \"z\": [\n      0,\n      0,\n      1\n    ]\n  },\n  \"type\": \"cylinder\"\n}\n"
        );
        // Written once, it reads back identically in any default unit.
        assert_eq!(
            Feature::from_json(&f.to_json(), Unit::Ft, &Env::empty()).unwrap(),
            f
        );
    }

    #[test]
    fn errors_point_at_the_field() {
        let cases: [(&str, Value, Code, &str); 10] = [
            ("/type", json!("prism"), Code::UnknownFeatureType, "/type"),
            (
                "/params/radius",
                json!("-1 mm"),
                Code::InvalidParam,
                "/params/radius",
            ),
            (
                "/params/radius",
                json!("1 parsec"),
                Code::InvalidQuantity,
                "/params/radius",
            ),
            (
                "/params/width",
                json!("1 mm"),
                Code::InvalidParam,
                "/params/width",
            ),
            (
                "/placement/z",
                json!([0, 1]),
                Code::InvalidDocument,
                "/placement/z",
            ),
            (
                "/placement/x/2",
                json!("1"),
                Code::InvalidDocument,
                "/placement/x/2",
            ),
            (
                "/placement/origin/1",
                json!(true),
                Code::InvalidQuantity,
                "/placement/origin/1",
            ),
            ("/extra", json!(1), Code::InvalidDocument, "/extra"),
            ("/format", json!(FEATURE_FORMAT + 1), Code::FormatTooNew, ""),
            (
                "/kind",
                json!("ryucad.part"),
                Code::InvalidDocument,
                "/kind",
            ),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = cylinder_json();
            *v.pointer_mut_or_insert(path) = value;
            let e = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap_err();
            assert_eq!(e.code, code, "{path}: {e}");
            if !pointer.is_empty() {
                assert_eq!(e.details["pointer"], pointer, "{path}");
            }
        }
    }

    #[test]
    fn missing_params() {
        let mut v = cylinder_json();
        v["params"].as_object_mut().unwrap().remove("height");
        let e = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["param"], "height");
        v.as_object_mut().unwrap().remove("placement");
        v["params"]["height"] = json!(1);
        let e = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap_err();
        assert_eq!(e.details["pointer"], "/placement");
    }

    #[test]
    fn cone_top_radius_is_optional_and_may_be_zero() {
        let mut v = cylinder_json();
        v["type"] = json!("cone");
        assert!(Feature::from_json(&v, Unit::Mm, &Env::empty()).is_ok());
        v["params"]["top_radius"] = json!(0);
        assert_eq!(
            prim(&Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap()).meters("top_radius"),
            0.0
        );
        v["params"]["radius"] = json!(0);
        assert_eq!(
            Feature::from_json(&v, Unit::Mm, &Env::empty())
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
    }

    fn wall() -> Env {
        ParametersFile::from_json(&json!({
            "format": 1,
            "kind": "ryucad.parameters",
            "parameters": {"hole": "wall*2", "n": 4, "wall": "3mm"}
        }))
        .unwrap()
        .env()
    }

    #[test]
    fn parameters_round_trip() {
        let v = json!({
            "format": 1,
            "kind": "ryucad.parameters",
            "parameters": {"hole": "wall*2", "n": 4, "wall": "3mm"}
        });
        let p = ParametersFile::from_json(&v).unwrap();
        assert_eq!(
            p.to_json()["parameters"],
            json!({"hole": "wall * 2", "n": "4", "wall": "3 mm"})
        );
        assert_eq!(ParametersFile::from_json(&p.to_json()).unwrap(), p);
        assert_eq!(p.env().get("hole").unwrap().x, 0.006);
    }

    #[test]
    fn parameter_files_are_checked() {
        let base = |params: Value| {
            ParametersFile::from_json(&json!({
                "format": 1, "kind": "ryucad.parameters", "parameters": params
            }))
        };
        let e = base(json!({"Wall": "1 mm"})).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidId, Some("/parameters/Wall"))
        );
        let e = base(json!({"mm": "1 mm"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidId);
        let e = base(json!({"a": "1 +"})).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidQuantity, Some("/parameters/a"))
        );
        let e = base(json!({"a": true})).unwrap_err();
        assert_eq!(e.code, Code::InvalidQuantity);
        assert_eq!(base(json!([])).unwrap_err().code, Code::InvalidDocument);
        // An unknown name or a cycle is the env's business, not the file's.
        assert!(base(json!({"a": "b"})).is_ok());
    }

    #[test]
    fn features_read_parameters() {
        let mut v = cylinder_json();
        v["params"]["radius"] = json!("hole/2+1mm");
        let f = Feature::from_json(&v, Unit::Mm, &wall()).unwrap();
        assert_eq!(prim(&f).meters("radius"), 0.004);
        assert_eq!(prim(&f).params["radius"].text(), "hole / 2 + 1 mm");
        assert_eq!(f.format(), 2);
        assert_eq!(f.to_json()["format"], 2);
        // Written at format 2, it reads back the same.
        assert_eq!(
            Feature::from_json(&f.to_json(), Unit::Ft, &wall()).unwrap(),
            f
        );
        v["placement"]["origin"][2] = json!("-wall");
        let f = Feature::from_json(&v, Unit::Mm, &wall()).unwrap();
        assert_eq!(prim(&f).placement.origin[2].meters(), -0.003);
        for (radius, code) in [
            ("gap", Code::UnknownParameter),
            ("n", Code::UnitMismatch),
            ("wall - hole", Code::InvalidParam),
        ] {
            v["params"]["radius"] = json!(radius);
            let e = Feature::from_json(&v, Unit::Mm, &wall()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some("/params/radius")),
                "{radius}"
            );
        }
        // A literal-only feature stays at format 1.
        let f = Feature::from_json(&cylinder_json(), Unit::Mm, &wall()).unwrap();
        assert_eq!(f.format(), 1);
        // The JSON alone says the same.
        assert_eq!(feature_format(&f.to_json()), 1);
        assert_eq!(feature_format(&v), 2);
        let sketch = json!({"type": "sketch", "entities": {}, "constraints": {}});
        assert_eq!(feature_format(&sketch), 2);
    }

    #[test]
    fn operations_are_format_3() {
        let mut v = cylinder_json();
        v["operation"] = json!("cut");
        v["targets"] = json!(["b"]);
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(f.targets(), [&Id::parse("b", "feature").unwrap()]);
        assert_eq!(f.format(), 3);
        assert_eq!(f.to_json()["format"], 3);
        assert_eq!(feature_format(&f.to_json()), 3);
        assert_eq!(
            Feature::from_json(&f.to_json(), Unit::Ft, &Env::empty()).unwrap(),
            f
        );
        // `new` is the default, and writes nothing.
        v["operation"] = json!("new");
        v.as_object_mut().unwrap().remove("targets");
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert!(f.combine().unwrap().is_new() && f.targets().is_empty());
        assert_eq!(f.format(), 1);
        assert!(f.to_json().get("operation").is_none());
        assert_eq!(feature_format(&v), 1);
    }

    #[test]
    fn blends_are_format_3() {
        let name = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": 3, "kind": "ryucad.feature", "type": "fillet",
                       "groups": [{"radius": "r", "edges": [{"feature": "b", "name": name}]}]});
        let env = ParametersFile::from_json(&json!({
            "format": 1, "kind": "ryucad.parameters", "parameters": {"r": "2 mm"}
        }))
        .unwrap()
        .env();
        let f = Feature::from_json(&v, Unit::Mm, &env).unwrap();
        assert_eq!((f.type_name(), f.format()), ("fillet", 3));
        assert_eq!(f.targets(), [&Id::parse("b", "feature").unwrap()]);
        assert_eq!(f.references()[0].0, "groups.0.edges.0");
        assert_eq!(f.to_json(), v);
        assert_eq!(feature_format(&v), 3);
        assert_eq!(feature_names(&v).into_iter().collect::<Vec<_>>(), ["r"]);
        let mut w = v.clone();
        assert!(rename_in_feature(&mut w, "r", "round"));
        assert_eq!(w["groups"][0]["radius"], "round");
        // A chamfer's references name its solids once each, faces too.
        let v = json!({"format": 3, "kind": "ryucad.feature", "type": "chamfer",
                       "groups": [{"form": "distances", "d1": "1 mm", "d2": "2 mm",
                                   "face": {"feature": "c", "name": name},
                                   "edges": [{"feature": "b", "name": name},
                                             {"feature": "b", "name": name}]}]});
        let f = Feature::from_json(&v, Unit::Mm, &env).unwrap();
        let ids: Vec<&str> = f.targets().into_iter().map(Id::as_str).collect();
        assert_eq!(ids, ["b", "c"]);
        assert!(f.combine().is_none());
    }

    #[test]
    fn names_are_read_from_the_text() {
        let mut v = cylinder_json();
        v["params"]["radius"] = json!("wall*2");
        v["params"]["height"] = json!("1 +");
        v["placement"]["origin"][0] = json!("x0 + pi * 1 mm");
        assert_eq!(
            feature_names(&v).into_iter().collect::<Vec<_>>(),
            ["wall", "x0"]
        );
        assert!(rename_in_feature(&mut v, "wall", "skin"));
        assert_eq!(v["params"]["radius"], "skin * 2");
        assert_eq!(v["params"]["height"], "1 +");
        assert!(!rename_in_feature(&mut v, "wall", "skin"));
    }

    #[test]
    fn duplicate_features_in_a_part() {
        let v = json!({"features": ["a", "a"], "format": 1, "kind": "ryucad.part"});
        let e = PartFile::from_json(&v).unwrap_err();
        assert_eq!(e.code, Code::DuplicateId);
        assert_eq!(e.details["pointer"], "/features/1");
    }

    #[test]
    fn migration_seam() {
        // Format 1 passes through; anything newer is refused before any
        // other field is read, and format 0 never existed.
        let mut v = Manifest::new(Unit::Mm).to_json();
        assert!(Manifest::from_json(&v).is_ok());
        v["format"] = json!(MANIFEST_FORMAT + 1);
        v["default_unit"] = json!("parsec");
        let e = Manifest::from_json(&v).unwrap_err();
        assert_eq!(e.code, Code::FormatTooNew);
        assert_eq!(e.details["supported"], MANIFEST_FORMAT);
        v["format"] = json!(0);
        assert_eq!(
            Manifest::from_json(&v).unwrap_err().code,
            Code::InvalidDocument
        );
    }

    /// `pointer_mut`, creating the last key if it is missing.
    trait PointerInsert {
        fn pointer_mut_or_insert(&mut self, path: &str) -> &mut Value;
    }

    impl PointerInsert for Value {
        fn pointer_mut_or_insert(&mut self, path: &str) -> &mut Value {
            if self.pointer(path).is_none() {
                let (parent, key) = path.rsplit_once('/').unwrap();
                self.pointer_mut(parent).unwrap()[key] = Value::Null;
            }
            self.pointer_mut(path).unwrap()
        }
    }

    #[test]
    fn expression_fields_are_found_by_pointer() {
        let v = json!({
            "format": 2, "kind": "ryucad.feature", "type": "box",
            "params": {"x": "w * 2", "y": "10 mm", "z": 4},
            "placement": {"origin": ["0 mm", "gap", "0 mm"], "x": [1, 0, 0], "z": [0, 0, 1]},
        });
        assert_eq!(
            expression_fields(&v),
            [
                ("/params/x".to_owned(), "w * 2".to_owned()),
                ("/params/y".to_owned(), "10 mm".to_owned()),
                ("/placement/origin/0".to_owned(), "0 mm".to_owned()),
                ("/placement/origin/1".to_owned(), "gap".to_owned()),
                ("/placement/origin/2".to_owned(), "0 mm".to_owned()),
            ]
        );
        // A sketch's dimensions, by constraint.
        let s = json!({"format": 2, "kind": "ryucad.feature", "type": "sketch", "plane": "xy",
                       "constraints": {"d1": {"type": "length", "a": "l1", "value": "w / 2"}},
                       "entities": {}});
        assert_eq!(
            expression_fields(&s),
            [("/constraints/d1/value".to_owned(), "w / 2".to_owned())]
        );
    }

    #[test]
    fn fills_and_extracts_are_format_15_and_found_sheets() {
        use super::super::surface::FORMAT;
        const A: &str = "0123456789abcdef0123456789abcdef";
        let fill = json!({"format": FORMAT, "kind": FEATURE_KIND, "type": "fill",
                          "sides": [{"edge": {"feature": "w", "name": A}, "tangent": true},
                                    {"curve": {"feature": "s", "entity": "l1"}},
                                    {"curve": {"feature": "p1"}}],
                          "tolerance": "0.001 mm", "angle": "1 deg"});
        let f = Feature::from_json(&fill, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(
            (f.type_name(), f.format(), feature_format(&fill)),
            ("fill", FORMAT, FORMAT)
        );
        assert!(f.founds() && f.may_make_sheet() && f.targets().is_empty());
        assert_eq!(f.to_json(), fill);
        let reads: Vec<&str> = f.geometry_reads().into_iter().map(Id::as_str).collect();
        assert_eq!(reads, ["s", "p1", "w"]);
        let a = feature_acts(&fill);
        assert!(a.founds && a.acts_on.is_empty());
        let ex = json!({"format": FORMAT, "kind": FEATURE_KIND, "type": "extract",
                        "faces": [{"feature": "base", "name": A}]});
        let f = Feature::from_json(&ex, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(
            (f.type_name(), f.format(), feature_format(&ex)),
            ("extract", FORMAT, FORMAT)
        );
        assert!(f.founds() && f.may_make_sheet() && f.targets().is_empty());
        assert_eq!(f.to_json(), ex);
        assert!(feature_acts(&ex).founds);
        // The newest format is the largest stage's, at least this one.
        const { assert!(FEATURE_FORMAT >= FORMAT) };
        // A fill's tolerance and angle are expressions an equations table lists.
        let fields: Vec<String> = expression_fields(&fill)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(fields, ["/angle", "/tolerance"]);
    }

    #[test]
    fn sews_and_thickens_are_format_15_and_consume_their_sheets() {
        use super::super::surface::FORMAT;
        const A: &str = "0123456789abcdef0123456789abcdef";
        let ids = |v: Vec<&Id>| -> Vec<String> { v.into_iter().map(|i| i.to_string()).collect() };
        let sew = json!({"format": FORMAT, "kind": FEATURE_KIND, "type": "sew",
                         "sheets": ["a", "b"], "tolerance": "0.01 mm", "solid": false});
        let f = Feature::from_json(&sew, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(
            (f.type_name(), f.format(), feature_format(&sew)),
            ("sew", FORMAT, FORMAT)
        );
        assert!(f.founds() && f.may_make_sheet());
        assert_eq!(
            (ids(f.targets()), ids(f.consumes())),
            (vec!["a".into(), "b".into()], vec!["a".into(), "b".into()])
        );
        assert_eq!(f.to_json(), sew);
        let a = feature_acts(&sew);
        assert!(a.founds && a.acts_on.is_empty());
        assert_eq!(
            a.consumes,
            [
                Id::parse("a", "feature").unwrap(),
                Id::parse("b", "feature").unwrap()
            ]
        );
        let th = json!({"format": FORMAT, "kind": FEATURE_KIND, "type": "thicken",
                        "from": {"sheet": "skin"}, "front": "2 mm", "back": "0 mm",
                        "operation": "join", "targets": ["base"]});
        let f = Feature::from_json(&th, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!((f.type_name(), f.format()), ("thicken", FORMAT));
        assert!(!f.founds() && !f.may_make_sheet());
        assert_eq!(ids(f.targets()), ["base", "skin"]);
        assert_eq!(ids(f.consumes()), ["skin"]);
        let a = feature_acts(&th);
        assert_eq!((a.founds, a.acts_on.len(), a.consumes.len()), (false, 1, 1));
        // From faces: it consumes nothing, and reads the faces' body.
        let th = json!({"format": FORMAT, "kind": FEATURE_KIND, "type": "thicken",
                        "from": {"faces": [{"feature": "base", "name": A}]},
                        "front": "2 mm", "back": "0 mm", "operation": "new"});
        let f = Feature::from_json(&th, Unit::Mm, &Env::empty()).unwrap();
        assert!(f.founds() && f.consumes().is_empty() && f.targets().is_empty());
        assert_eq!(ids(f.geometry_reads()), ["base"]);
        let fields: Vec<String> = expression_fields(&th).into_iter().map(|(p, _)| p).collect();
        assert_eq!(fields, ["/back", "/front"]);
        // A boolean consumes its tools, as before.
        let b = json!({"format": 3, "kind": FEATURE_KIND, "type": "boolean", "op": "unite",
                       "target": "a", "tools": ["t"]});
        let f = Feature::from_json(&b, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(ids(f.consumes()), ["t"]);
    }

    #[test]
    fn a_blend_deletes_size_is_an_expression() {
        let mut v = json!({"type": "delete_blend", "target": "b", "below": "r * 2",
                           "tolerance": "tol"});
        let mut found: Vec<String> = expression_strings(&mut v)
            .into_iter()
            .map(|s| s.clone())
            .collect();
        found.sort();
        assert_eq!(found, ["r * 2", "tol"]);
    }

    #[test]
    fn blend_edits_load_at_the_edit_format_and_act_on_their_target() {
        let edit = super::super::edit::FORMAT;
        let face = json!({"feature": "base", "name": format!("{:032x}", 9)});
        let d = json!({"format": edit, "kind": "ryucad.feature", "type": "delete_blend",
                       "target": "base", "below": "2 mm"});
        let r = json!({"format": edit, "kind": "ryucad.feature", "type": "resize_blend",
                       "target": "base", "groups": [{"radius": "3 mm", "faces": [face]}]});
        let shell = json!({"type": "shell", "target": "base"});
        for (v, ty, roles) in [
            (d, "delete_blend", Vec::<&str>::new()),
            (r, "resize_blend", vec!["groups.0.faces.0"]),
        ] {
            let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
            assert_eq!(f.type_name(), ty);
            assert_eq!((f.format(), feature_format(&v)), (edit, edit));
            assert_eq!(f.to_json(), v);
            assert_eq!(f.targets(), [&Id::parse("base", "feature").unwrap()]);
            let got: Vec<String> = f.references().into_iter().map(|(k, _)| k).collect();
            assert_eq!(got, roles);
            assert_eq!(feature_acts(&v).acts_on, feature_acts(&shell).acts_on);
            assert!(f.retrim().is_none());
        }
    }

    #[test]
    fn a_delete_face_loads_at_its_format_and_acts_on_its_target() {
        let edit = super::super::edit::FORMAT;
        let face = json!({"feature": "base", "name": format!("{:032x}", 9)});
        let v = json!({"format": edit, "kind": "ryucad.feature", "type": "delete_face",
                       "target": "plate", "seeds": [face], "heal": "grow"});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(f.type_name(), "delete_face");
        assert_eq!((f.format(), feature_format(&v)), (edit, edit));
        assert_eq!(f.to_json(), v);
        assert_eq!(f.targets(), [&Id::parse("plate", "feature").unwrap()]);
        let roles: Vec<String> = f.references().into_iter().map(|(k, _)| k).collect();
        assert_eq!(roles, ["seeds.0"]);
        // It acts on its target as a shell does.
        let shell = json!({"type": "shell", "target": "plate"});
        assert_eq!(feature_acts(&v).acts_on, feature_acts(&shell).acts_on);
        assert!(f.retrim().is_none());
        const { assert!(FEATURE_FORMAT >= super::super::edit::FORMAT) };
    }

    #[test]
    fn tweaks_load_at_their_format_and_act_on_their_target() {
        let n = "0123456789abcdef0123456789abcdef";
        let p = [("d".to_owned(), crate::expr::Expr::parse("2 mm").unwrap())].into();
        let env = Env::new(&p);
        let v = json!({"format": super::super::tweak::FORMAT, "kind": "ryucad.feature",
                       "type": "move_face", "target": "plate", "by": "offset",
                       "distance": "d", "faces": [{"feature": "plate", "name": n}],
                       "groups": [{"distance": "d / 2", "faces": [{"feature": "boss", "name": n}]}]});
        let f = Feature::from_json(&v, Unit::Mm, &env).unwrap();
        assert_eq!(f.type_name(), "move_face");
        assert_eq!(f.format(), super::super::tweak::FORMAT);
        assert_eq!(feature_format(&v), super::super::tweak::FORMAT);
        assert_eq!(f.to_json(), v);
        assert_eq!(f.targets(), [&Id::parse("plate", "feature").unwrap()]);
        assert!(f.combine().is_none() && !f.founds());
        assert_eq!(
            feature_acts(&v).acts_on,
            [Id::parse("plate", "feature").unwrap()]
        );
        let roles: Vec<String> = f.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["faces.0", "groups.0.faces.0"]);
        // Its distances are expressions an equations table lists.
        let fields: Vec<String> = expression_fields(&v).into_iter().map(|(p, _)| p).collect();
        assert_eq!(fields, ["/distance", "/groups/0/distance"]);
        assert!(feature_names(&v).contains("d"));
        let v = json!({"format": super::super::tweak::FORMAT, "kind": "ryucad.feature",
                       "type": "replace_face", "target": "base",
                       "groups": [{"with": {"feature": "lid"}, "faces": [{"feature": "base", "name": n}]}]});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(f.to_json(), v);
        let reads: Vec<&str> = f.geometry_reads().into_iter().map(Id::as_str).collect();
        assert_eq!(reads, ["lid", "base"]);
        // One past the newest is too new.
        const { assert!(FEATURE_FORMAT >= super::super::tweak::FORMAT) };
    }

    #[test]
    fn a_draft_is_a_feature_at_its_own_format() {
        let n = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": super::super::tweak::DRAFT_FORMAT, "kind": "ryucad.feature",
                       "type": "draft", "target": "base", "angle": "2 deg", "neutral": "xy",
                       "faces": [{"feature": "base", "name": n}]});
        let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!(f.type_name(), "draft");
        assert_eq!(f.format(), super::super::tweak::DRAFT_FORMAT);
        assert_eq!(feature_format(&v), super::super::tweak::DRAFT_FORMAT);
        assert_eq!(f.to_json(), v);
        assert_eq!(f.targets(), [&Id::parse("base", "feature").unwrap()]);
        let roles: Vec<String> = f.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["faces.0"]);
        let geo: Vec<String> = f.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(geo, ["neutral"]);
        assert!(f.combine().is_none());
        // What a file that doesn't load still says it acts on.
        assert_eq!(
            feature_acts(&v).acts_on,
            vec![Id::parse("base", "feature").unwrap()]
        );
        // The format reads the constant; a newer one is refused.
        const { assert!(FEATURE_FORMAT >= super::super::tweak::DRAFT_FORMAT) };
        let mut newer = v.clone();
        newer["format"] = json!(FEATURE_FORMAT + 1);
        assert_eq!(
            Feature::from_json(&newer, Unit::Mm, &Env::empty())
                .unwrap_err()
                .code,
            Code::FormatTooNew
        );
    }

    #[test]
    fn hole_geometry_is_format_25_and_old_holes_keep_theirs() {
        use crate::units::Unit;
        let env = crate::expr::Env::empty();
        let base = json!({"kind": "ryucad.feature", "type": "hole", "sketch": "s",
                          "form": "simple", "diameter": "5 mm", "depth": "through_all",
                          "targets": ["b"]});
        for (k, x) in [
            ("depth", json!("through_next")),
            ("near", json!({"diameter": "7 mm"})),
            (
                "far",
                json!({"countersink": {"diameter": "8 mm", "angle": "90 deg"}}),
            ),
        ] {
            let mut v = base.clone();
            v[k] = x;
            assert_eq!(
                feature_format(&v),
                super::super::hole::GEOMETRY_FORMAT,
                "{k}"
            );
            v["format"] = json!(25);
            let f = Feature::from_json(&v, Unit::Mm, &env).unwrap();
            assert_eq!(f.format(), 25, "{k}");
        }
        let mut v = base.clone();
        v["form"] = json!("spotface");
        v["spotface"] = json!({"diameter": "12 mm"});
        assert_eq!(feature_format(&v), 25);
        // A plain hole and a tapped one keep 3 and 23.
        assert_eq!(feature_format(&base), 3);
        let mut t = base.clone();
        t.as_object_mut().unwrap().remove("diameter");
        t["thread"] = json!({"standard": "iso_metric", "size": "M6"});
        assert_eq!(feature_format(&t), 23);
        // Geometry wins over intent: highest first.
        t["near"] = json!({"diameter": "7 mm"});
        assert_eq!(feature_format(&t), 25);
    }

    #[test]
    fn a_holes_new_geometry_holds_expressions_and_through_next_is_none() {
        let v = json!({"format": 25, "kind": FEATURE_KIND, "type": "hole", "sketch": "s",
            "form": "counterdrill", "diameter": "d", "depth": "through_next",
            "counterdrill": {"diameter": "d * 2", "depth": "1 mm"},
            "near": {"diameter": "d + 3 mm"},
            "far": {"countersink": {"diameter": "e", "angle": "90 deg"}},
            "targets": ["b"]});
        let at: Vec<String> = expression_fields(&v).into_iter().map(|(p, _)| p).collect();
        for p in [
            "/counterdrill/diameter",
            "/near/diameter",
            "/far/countersink/diameter",
        ] {
            assert!(at.iter().any(|x| x == p), "{p} in {at:?}");
        }
        assert!(!at.iter().any(|x| x == "/depth"), "{at:?}");
        assert!(feature_names(&v).contains("e"));
        // A cosmetic thread's depth is a length.
        let c = json!({"format": 25, "kind": FEATURE_KIND, "type": "cosmetic_thread",
            "faces": [{"feature": "shaft", "name": "0123456789abcdef0123456789abcdef"}],
            "thread": {"standard": "iso_metric", "size": "M10"},
            "from": {"feature": "shaft", "name": "fedcba9876543210fedcba9876543210"},
            "depth": "l"});
        assert!(feature_names(&c).contains("l"));
    }

    #[test]
    fn a_hole_with_intent_is_format_23_and_one_without_keeps_its_own() {
        const { assert!(FEATURE_FORMAT >= super::super::hole::INTENT_FORMAT) };
        const { assert!(FEATURE_FORMAT >= super::super::hole::GEOMETRY_FORMAT) };
        let plain = json!({"format": 3, "kind": FEATURE_KIND, "type": "hole", "sketch": "s",
                           "form": "simple", "diameter": "5 mm", "depth": "through_all",
                           "targets": ["b"]});
        let f = Feature::from_json(&plain, Unit::Mm, &Env::empty()).unwrap();
        assert_eq!((f.format(), feature_format(&plain)), (3, 3));
        assert_eq!(
            canonical::to_string(&f.to_json()),
            canonical::to_string(&plain)
        );
        for (k, x) in [
            ("thread", json!({"standard": "iso_metric", "size": "M6"})),
            ("fastener", json!({"standard": "house_cap", "size": "M5"})),
            ("fit", json!("H7")),
            ("tolerance", json!({"symmetric": "0.01 mm"})),
        ] {
            let mut v = plain.clone();
            v[k] = x;
            v["format"] = json!(super::super::hole::INTENT_FORMAT);
            let f = Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap();
            assert_eq!((f.format(), feature_format(&v)), (23, 23), "{k}");
        }
    }

    #[test]
    fn solid_paths_are_format_21_and_nothing_else_moves() {
        use super::super::insert::FORMAT;
        assert_eq!(FORMAT, 21);
        // ryucad 26's mate limits are 22, so the newest is past it.
        const { assert!(FEATURE_FORMAT >= FORMAT) };
        let n = "0123456789abcdef0123456789abcdef";
        for (v, want) in [
            (
                json!({"type": "box", "operation": "cut", "targets": ["board/u1"]}),
                FORMAT,
            ),
            (
                json!({"type": "boolean", "op": "subtract", "target": "a", "tools": ["b/c"]}),
                FORMAT,
            ),
            (json!({"type": "shell", "target": "board/u1"}), FORMAT),
            (json!({"type": "mirror", "solid": "board/u1"}), FORMAT),
            (
                json!({"type": "fillet", "groups": [{"edges": [{"feature": "board", "solid": "u1", "name": n}]}]}),
                FORMAT,
            ),
            (
                json!({"type": "sketch", "plane": {"face": {"feature": "board", "solid": "u1", "name": n}}}),
                FORMAT,
            ),
            (
                json!({"type": "insert", "part": "pcb", "rev": "working"}),
                FORMAT,
            ),
            (json!({"type": "delete_body", "solids": ["a"]}), FORMAT),
            // The higher of two formats wins.
            (
                json!({"type": "mate", "a": {"feature": "board", "solid": "u1", "name": n},
                       "limits": {"angle": {"min": "0 deg"}}}),
                super::super::mate::LIMITS_FORMAT,
            ),
            // Founders' keys stay where they were.
            (
                json!({"type": "box", "operation": "cut", "targets": ["a"]}),
                3,
            ),
            (
                json!({"type": "shell", "target": "a"}),
                super::super::body::FORMAT,
            ),
            (
                json!({"type": "mirror", "solid": "a"}),
                super::super::body::MIRROR_FORMAT,
            ),
        ] {
            assert_eq!(feature_format(&v), want, "{v}");
        }
    }
}
