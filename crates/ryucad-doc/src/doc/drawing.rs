//! Drawings: `parts/<part>/drawings/<id>.json`, 2D drawings of the part
//! (or assembly) whose folder holds them (design 417). The file holds
//! intent and placement only: every value the model decides (a curve, a
//! title block's part number) is worked out again from the model at the
//! revision asked for. Nothing a rebuild makes reads a drawing.
//!
//! ```json
//! {
//!   "format": 1,
//!   "kind": "ryucad.drawing",
//!   "name": "Bracket",
//!   "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b", "scale": "1:2"}},
//!   "views": {
//!     "v1": {"type": "base", "sheet": "s1", "at": ["120 mm", "160 mm"], "orient": "front"},
//!     "v2": {"type": "projected", "sheet": "s1", "from": "v1", "side": "top", "offset": "70 mm"},
//!     "v3": {"type": "projected", "sheet": "s1", "from": "v1", "side": "iso", "at": ["330 mm", "200 mm"]},
//!     "v4": {"type": "auxiliary", "sheet": "s1", "from": "v1", "normal_to": {"feature": "base", "name": "<hex>"}}
//!   }
//! }
//! ```
//!
//! Members are keyed by a kind letter and a number (`s1`, `v3`), a new one
//! one past the largest of its kind. Positions are sheet millimetres as
//! quantities (a bare number is millimetres), kept as written: they are
//! intent, merged per key. A base view with no `at`, or a projected or
//! auxiliary view with no `offset`, is placed by ryucad, and the place is
//! written back on the next `drawing.edit`. Format 1 is fixed when release
//! v0.12.0 ships: plans 9.B and 9.C add members to it before then
//! (sections, details, flat and exploded views, annotations).

use super::files::{Obj, header, invalid};
use super::geometry::GeomRef;
use super::id::Id;
use super::reference::Reference;
#[cfg(test)]
use crate::error::Code;
use crate::error::Result;
use crate::units::{Quantity, Unit};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fmt;

pub const KIND: &str = "ryucad.drawing";

/// The newest format this build reads.
pub const FORMAT: u64 = 1;

/// The folder of a part's drawings, inside its folder.
pub const DRAWINGS: &str = "drawings";

/// The tag of the piece of a part's revision (and of ryupdm's content
/// hash) holding its drawings.
pub const TAG: &str = "ryucad.drawings";

/// The largest number either side of a scale's colon.
pub const MAX_SCALE: u32 = 1000;

/// A drawing's ID: a part ID.
pub fn check_id(text: &str) -> Result<Id> {
    Id::parse(text, "drawing")
}

/// Where drawing `id` of a part lives, relative to its folder.
pub fn drawing_file(id: &Id) -> String {
    format!("{DRAWINGS}/{id}.json")
}

/// Which drafting standard a drawing follows: ASME (Y14.5, Y14.1,
/// third-angle projection, ANSI sheets) unless the project or the drawing
/// says ISO (question 337).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Standard {
    #[default]
    Asme,
    Iso,
}

impl Standard {
    pub fn as_str(self) -> &'static str {
        match self {
            Standard::Asme => "asme",
            Standard::Iso => "iso",
        }
    }

    pub fn parse(s: &str) -> Option<Standard> {
        match s {
            "asme" => Some(Standard::Asme),
            "iso" => Some(Standard::Iso),
            _ => None,
        }
    }

    /// The standard's own projection angle.
    pub fn projection(self) -> Angle {
        match self {
            Standard::Asme => Angle::Third,
            Standard::Iso => Angle::First,
        }
    }
}

/// First- or third-angle projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Angle {
    First,
    Third,
}

impl Angle {
    pub fn as_str(self) -> &'static str {
        match self {
            Angle::First => "first_angle",
            Angle::Third => "third_angle",
        }
    }

    pub fn parse(s: &str) -> Option<Angle> {
        match s {
            "first_angle" => Some(Angle::First),
            "third_angle" => Some(Angle::Third),
            _ => None,
        }
    }
}

/// A sheet's paper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Size {
    IsoA0,
    IsoA1,
    IsoA2,
    IsoA3,
    IsoA4,
    AnsiA,
    AnsiB,
    AnsiC,
    AnsiD,
    AnsiE,
}

/// Every size, as files write them.
pub const SIZES: [(&str, Size); 10] = [
    ("iso_a0", Size::IsoA0),
    ("iso_a1", Size::IsoA1),
    ("iso_a2", Size::IsoA2),
    ("iso_a3", Size::IsoA3),
    ("iso_a4", Size::IsoA4),
    ("ansi_a", Size::AnsiA),
    ("ansi_b", Size::AnsiB),
    ("ansi_c", Size::AnsiC),
    ("ansi_d", Size::AnsiD),
    ("ansi_e", Size::AnsiE),
];

impl Size {
    pub fn as_str(self) -> &'static str {
        SIZES.iter().find(|(_, s)| *s == self).expect("listed").0
    }

    pub fn parse(s: &str) -> Option<Size> {
        SIZES.iter().find(|(n, _)| *n == s).map(|(_, z)| *z)
    }

    /// Its long and short sides in millimetres (ISO 216; ANSI Y14.1's
    /// inches times 25.4).
    pub fn sides(self) -> [f64; 2] {
        match self {
            Size::IsoA0 => [1189.0, 841.0],
            Size::IsoA1 => [841.0, 594.0],
            Size::IsoA2 => [594.0, 420.0],
            Size::IsoA3 => [420.0, 297.0],
            Size::IsoA4 => [297.0, 210.0],
            Size::AnsiA => [279.4, 215.9],
            Size::AnsiB => [431.8, 279.4],
            Size::AnsiC => [558.8, 431.8],
            Size::AnsiD => [863.6, 558.8],
            Size::AnsiE => [1117.6, 863.6],
        }
    }

    /// The sheet's width and height, in millimetres.
    pub fn paper(self, o: Orientation) -> [f64; 2] {
        let [long, short] = self.sides();
        match o {
            Orientation::Landscape => [long, short],
            Orientation::Portrait => [short, long],
        }
    }

    /// The size a new sheet takes in `standard`: ANSI B or ISO A3.
    pub fn default_for(standard: Standard) -> Size {
        match standard {
            Standard::Asme => Size::AnsiB,
            Standard::Iso => Size::IsoA3,
        }
    }

    /// The built-in template for this size: `asme-<letter>` for an ANSI
    /// sheet, `iso-a<n>` for an ISO one.
    pub fn template(self) -> String {
        let s = self.as_str();
        match s.strip_prefix("ansi_") {
            Some(letter) => format!("asme-{letter}"),
            None => format!("iso-{}", &s[4..]),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Orientation {
    #[default]
    Landscape,
    Portrait,
}

impl Orientation {
    pub fn as_str(self) -> &'static str {
        match self {
            Orientation::Landscape => "landscape",
            Orientation::Portrait => "portrait",
        }
    }

    pub fn parse(s: &str) -> Option<Orientation> {
        match s {
            "landscape" => Some(Orientation::Landscape),
            "portrait" => Some(Orientation::Portrait),
            _ => None,
        }
    }
}

/// A scale `a:b`: `a` sheet millimetres stand for `b` of the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scale {
    pub sheet: u32,
    pub model: u32,
}

impl Scale {
    pub const ONE: Scale = Scale { sheet: 1, model: 1 };

    /// `"1:2"`: two whole numbers from 1 to [`MAX_SCALE`].
    pub fn parse(s: &str) -> Option<Scale> {
        let (a, b) = s.split_once(':')?;
        let n = |t: &str| {
            t.parse::<u32>()
                .ok()
                .filter(|v| (1..=MAX_SCALE).contains(v) && !t.starts_with('0'))
        };
        Some(Scale {
            sheet: n(a)?,
            model: n(b)?,
        })
    }

    /// Sheet millimetres per model metre.
    pub fn mm_per_m(self) -> f64 {
        1000.0 * f64::from(self.sheet) / f64::from(self.model)
    }
}

impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.sheet, self.model)
    }
}

/// A drawing member's key: a kind letter and a number from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub kind: char,
    pub number: u32,
}

impl Key {
    /// `s3` of kind `s`, or `None`.
    pub fn parse(text: &str, kind: char) -> Option<Key> {
        let digits = text.strip_prefix(kind)?;
        let number = digits.parse::<u32>().ok()?;
        (number >= 1 && !digits.starts_with('0')).then_some(Key { kind, number })
    }

    /// The first key of `kind` past every key of `keys`.
    pub fn next<'a>(kind: char, keys: impl IntoIterator<Item = &'a Key>) -> Key {
        let n = keys
            .into_iter()
            .filter(|k| k.kind == kind)
            .map(|k| k.number)
            .max()
            .unwrap_or(0);
        Key {
            kind,
            number: n + 1,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.kind, self.number)
    }
}

/// One sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub size: Size,
    pub orientation: Orientation,
    /// A built-in template's ID or a project template's; none for the
    /// size's built-in one.
    pub template: Option<String>,
    /// Its views' scale where they don't say one; none to fit them.
    pub scale: Option<Scale>,
}

/// Whether a view draws hidden lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Hidden {
    #[default]
    Off,
    Dashed,
}

/// How a view draws tangent edges (and silhouettes running along them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tangent {
    #[default]
    Thin,
    Off,
    Phantom,
}

/// Which way a base view looks.
#[derive(Clone, Debug, PartialEq)]
pub enum Orient {
    /// A standard view's name or a named view of the part drawn.
    Named(String),
    /// Along a planar face's outward normal, or a revolved face's axis.
    NormalTo(Reference),
}

/// Which side of its parent a projected view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
    Back,
    /// A diagonal (`iso`, or a corner of the cube such as
    /// `front_top_right`): an isometric view, placed by its own `at`.
    Diagonal(String),
}

/// The diagonal sides a projected view takes.
pub const DIAGONALS: [&str; 9] = [
    "iso",
    "front_top_left",
    "front_top_right",
    "front_bottom_left",
    "front_bottom_right",
    "back_top_left",
    "back_top_right",
    "back_bottom_left",
    "back_bottom_right",
];

impl Side {
    pub fn as_str(&self) -> &str {
        match self {
            Side::Top => "top",
            Side::Bottom => "bottom",
            Side::Left => "left",
            Side::Right => "right",
            Side::Back => "back",
            Side::Diagonal(d) => d,
        }
    }

    pub fn parse(s: &str) -> Option<Side> {
        Some(match s {
            "top" => Side::Top,
            "bottom" => Side::Bottom,
            "left" => Side::Left,
            "right" => Side::Right,
            "back" => Side::Back,
            d if DIAGONALS.contains(&d) => Side::Diagonal(d.to_owned()),
            _ => return None,
        })
    }
}

/// A point in the model a drawing names: a geometric reference that
/// stands for one (a vertex, a datum point, the origin), resolved in what
/// the view draws, or a point in model space.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelPoint {
    Ref(GeomRef),
    /// Lengths in the model, bare numbers millimetres.
    At([Quantity; 3]),
}

/// Where a section cuts. What lies on the side the plane's normal points
/// to is cut away, and the section is seen from there (`flip` turns it).
#[derive(Clone, Debug, PartialEq)]
pub enum Cut {
    /// A full section on a plane reference of what the view draws (an
    /// origin plane, a planar face, a datum plane), moved by `offset`
    /// along its normal.
    Plane {
        plane: GeomRef,
        offset: Option<Quantity>,
    },
    /// A full section on the plane through `origin` square to `normal`.
    At {
        origin: [Quantity; 3],
        normal: [f64; 3],
    },
    /// An offset section through two or more points in order, stepped
    /// square to its parent's view: the cut runs along each pair's line
    /// seen in the parent, and what lies to the cut's left (as its first
    /// segment runs, seen in the parent) is cut away.
    Through(Vec<ModelPoint>),
}

/// What a view draws, and where from.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewKind {
    /// A part or assembly of the project (the drawing's own when `of` is
    /// none), at `working` or a release label.
    Base {
        of: Option<Id>,
        label: Option<String>,
        orient: Orient,
    },
    /// Its parent's source and scale, a quarter turn (or, for a diagonal,
    /// an isometric) from it.
    Projected {
        from: Key,
        side: Side,
        /// Along the row or column from the parent's centre, for a side
        /// that isn't a diagonal.
        offset: Option<Quantity>,
    },
    /// Along a straight edge or a planar face's normal of what its parent
    /// draws, aligned with its parent along it.
    Auxiliary {
        from: Key,
        normal_to: Reference,
        offset: Option<Quantity>,
    },
    /// Its parent's source cut by `cut`, seen square to the cut from the
    /// side cut away (`flip` turns it): placed along its parent's row or
    /// column by `offset` where it looks along the parent's plane, else by
    /// its own `at`. Its `label` is its letter.
    Section {
        from: Key,
        cut: Cut,
        flip: bool,
        /// Instance paths (`p1`, `p1/left`) an assembly's section leaves
        /// whole: fasteners, shafts.
        unsectioned: Vec<String>,
        offset: Option<Quantity>,
    },
    /// Its parent at its own scale (twice its parent's unless written),
    /// kept inside the circle of `radius` (in the model) about `centre`.
    /// Its `label` is its letter.
    Detail {
        from: Key,
        centre: ModelPoint,
        radius: Quantity,
    },
    /// A sheet metal solid's flat pattern (the drawing's own part's unless
    /// `of`; its one sheet metal solid unless `solid`), with its bend
    /// lines and, unless `notes` is false, their notes.
    Flat {
        of: Option<Id>,
        solid: Option<Id>,
        notes: bool,
    },
    /// An assembly's explode `explode`, `fraction` of the way (all of it
    /// unless written), seen from `orient`, its trails drawn when
    /// `trails`.
    Exploded {
        of: Option<Id>,
        explode: Id,
        fraction: f64,
        orient: Orient,
        trails: bool,
    },
}

/// Whether `s` is a section's or detail's letter: one or two capitals,
/// none of I, O and Q (ASME Y14.3 and ISO 128-3 leave them out, being
/// read as 1 and 0).
pub fn letter_ok(s: &str) -> bool {
    (1..=2).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_uppercase() && !matches!(c, 'I' | 'O' | 'Q'))
}

/// The first letter none of `used` is: A to Z less I, O, Q and (as
/// ASME Y14.3 advises) S, X and Z, then AA, AB and on.
pub fn next_letter<'a>(used: impl IntoIterator<Item = &'a str>) -> String {
    let used: Vec<&str> = used.into_iter().collect();
    let singles: Vec<char> = ('A'..='Z')
        .filter(|c| !matches!(c, 'I' | 'O' | 'Q' | 'S' | 'X' | 'Z'))
        .collect();
    let candidates = singles.iter().map(|c| c.to_string()).chain(
        singles
            .iter()
            .flat_map(|a| singles.iter().map(move |b| format!("{a}{b}"))),
    );
    for c in candidates {
        if !used.contains(&c.as_str()) {
            return c;
        }
    }
    "ZZ".to_owned()
}

/// One view.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub sheet: Key,
    /// Its centre on the sheet (a base or diagonal view's); none to be
    /// placed.
    pub at: Option<[Quantity; 2]>,
    pub scale: Option<Scale>,
    pub hidden: Hidden,
    pub tangent: Tangent,
    pub label: Option<String>,
    pub kind: ViewKind,
}

impl View {
    /// The view it derives from, if any.
    pub fn parent(&self) -> Option<Key> {
        match &self.kind {
            ViewKind::Base { .. } | ViewKind::Flat { .. } | ViewKind::Exploded { .. } => None,
            ViewKind::Projected { from, .. }
            | ViewKind::Auxiliary { from, .. }
            | ViewKind::Section { from, .. }
            | ViewKind::Detail { from, .. } => Some(*from),
        }
    }

    /// A section's or detail's letter: its label.
    pub fn letter(&self) -> Option<&str> {
        match self.kind {
            ViewKind::Section { .. } | ViewKind::Detail { .. } => self.label.as_deref(),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self.kind {
            ViewKind::Base { .. } => "base",
            ViewKind::Projected { .. } => "projected",
            ViewKind::Auxiliary { .. } => "auxiliary",
            ViewKind::Section { .. } => "section",
            ViewKind::Detail { .. } => "detail",
            ViewKind::Flat { .. } => "flat",
            ViewKind::Exploded { .. } => "exploded",
        }
    }
}

/// A drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawingFile {
    pub name: String,
    pub standard: Option<Standard>,
    pub projection: Option<Angle>,
    pub unit: Option<Unit>,
    /// Decimals a value shows; none for 2 in millimetres, 3 in inches.
    pub precision: Option<u32>,
    pub sheets: BTreeMap<Key, Sheet>,
    pub views: BTreeMap<Key, View>,
    /// Text the title block reads that no property gives (`drawn_by`).
    pub fields: BTreeMap<String, String>,
}

/// The most decimals a value shows.
pub const MAX_PRECISION: u32 = 6;

fn text_of(v: &Value, at: &str, what: &str) -> Result<String> {
    v.as_str()
        .filter(|s| !s.is_empty() && !s.contains('\n') && s.chars().count() <= 200)
        .map(str::to_owned)
        .ok_or_else(|| {
            invalid(
                at,
                &format!("{what} is one line of text, 1 to 200 characters"),
            )
        })
}

fn word<T>(v: &Value, at: &str, parse: fn(&str) -> Option<T>, words: &str) -> Result<T> {
    v.as_str()
        .and_then(parse)
        .ok_or_else(|| invalid(at, &format!("expected one of {words}")))
}

/// A length on the sheet: a quantity, bare numbers in millimetres, no
/// parameters.
fn length(v: &Value, at: &str) -> Result<Quantity> {
    Quantity::from_json(v, Unit::Mm).map_err(|e| e.with("pointer", at))
}

fn position(v: &Value, at: &str) -> Result<[Quantity; 2]> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or_else(|| invalid(at, "a position is two lengths on the sheet"))?;
    Ok([
        length(&a[0], &format!("{at}/0"))?,
        length(&a[1], &format!("{at}/1"))?,
    ])
}

/// A model point: three lengths (bare numbers millimetres) or a
/// geometric reference.
fn model_point(v: &Value, at: &str) -> Result<ModelPoint> {
    match v {
        Value::Array(a) if a.len() == 3 => Ok(ModelPoint::At([
            length(&a[0], &format!("{at}/0"))?,
            length(&a[1], &format!("{at}/1"))?,
            length(&a[2], &format!("{at}/2"))?,
        ])),
        Value::Array(_) => Err(invalid(at, "a point in the model is three lengths")),
        _ => Ok(ModelPoint::Ref(GeomRef::from_json(v, at)?)),
    }
}

impl ModelPoint {
    fn to_json(&self) -> Value {
        match self {
            ModelPoint::Ref(g) => g.to_json(),
            ModelPoint::At(q) => json!([q[0].text(), q[1].text(), q[2].text()]),
        }
    }
}

impl Cut {
    fn to_json(&self) -> Value {
        match self {
            Cut::Plane { plane, offset } => {
                let mut v = json!({"plane": plane.to_json()});
                if let Some(o) = offset {
                    v["offset"] = json!(o.text());
                }
                v
            }
            Cut::At { origin, normal } => json!({
                "origin": [origin[0].text(), origin[1].text(), origin[2].text()],
                "normal": normal,
            }),
            Cut::Through(points) => {
                json!({"through": points.iter().map(ModelPoint::to_json).collect::<Vec<_>>()})
            }
        }
    }

    fn from_json(v: &Value, at: &str) -> Result<Cut> {
        let mut o = Obj::new(v, at)?;
        let p = |k: &str| format!("{at}/{k}");
        let cut = if let Some(plane) = o.take_opt("plane") {
            let plane = GeomRef::from_json(&plane, &p("plane"))?;
            let offset = match o.take_opt("offset") {
                None => None,
                Some(x) => Some(length(&x, &p("offset"))?),
            };
            Cut::Plane { plane, offset }
        } else if let Some(through) = o.take_opt("through") {
            let a = through
                .as_array()
                .filter(|a| (2..=32).contains(&a.len()))
                .ok_or_else(|| {
                    invalid(
                        &p("through"),
                        "an offset section runs through 2 to 32 points",
                    )
                })?;
            Cut::Through(
                a.iter()
                    .enumerate()
                    .map(|(i, x)| model_point(x, &format!("{at}/through/{i}")))
                    .collect::<Result<_>>()?,
            )
        } else {
            let origin = o.take("origin")?;
            let origin = match model_point(&origin, &p("origin"))? {
                ModelPoint::At(q) => q,
                ModelPoint::Ref(_) => {
                    return Err(invalid(&p("origin"), "a cut's origin is three lengths"));
                }
            };
            let n = o.take("normal")?;
            let normal: [f64; 3] = n
                .as_array()
                .filter(|a| a.len() == 3)
                .and_then(|a| {
                    let f: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
                    (f.len() == 3).then(|| [f[0], f[1], f[2]])
                })
                .filter(|n| n.iter().all(|x| x.is_finite()) && n.iter().any(|x| *x != 0.0))
                .ok_or_else(|| {
                    invalid(&p("normal"), "a cut's normal is three numbers, not all 0")
                })?;
            Cut::At { origin, normal }
        };
        o.finish()?;
        Ok(cut)
    }
}

fn key_of(v: &Value, at: &str, kind: char, what: &str) -> Result<Key> {
    v.as_str()
        .and_then(|s| Key::parse(s, kind))
        .ok_or_else(|| invalid(at, &format!("{what} is a key such as {kind}1")))
}

fn scale_of(v: &Value, at: &str) -> Result<Scale> {
    v.as_str().and_then(Scale::parse).ok_or_else(|| {
        invalid(
            at,
            "a scale is two whole numbers from 1 to 1000, such as \"1:2\"",
        )
    })
}

fn orient_json(o: &Orient) -> Value {
    match o {
        Orient::Named(n) => json!(n),
        Orient::NormalTo(r) => json!({"normal_to": r.to_json()}),
    }
}

fn orient_of(v: Value, at: &str) -> Result<Orient> {
    match v {
        Value::String(s) if !s.is_empty() => Ok(Orient::Named(s)),
        Value::Object(m) if m.len() == 1 && m.contains_key("normal_to") => Ok(Orient::NormalTo(
            Reference::from_json(&m["normal_to"], &format!("{at}/normal_to"))?,
        )),
        _ => Err(invalid(
            at,
            "a view's orient is a standard or named view, or {\"normal_to\": a face}",
        )),
    }
}

fn id_of(v: &Value, at: &str, what: &str) -> Result<Id> {
    v.as_str()
        .and_then(|s| Id::parse(s, what).ok())
        .ok_or_else(|| invalid(at, &format!("a {what}'s ID")))
}

impl Sheet {
    fn to_json(&self) -> Value {
        let mut v = json!({"name": self.name, "size": self.size.as_str()});
        if self.orientation != Orientation::Landscape {
            v["orientation"] = json!(self.orientation.as_str());
        }
        if let Some(t) = &self.template {
            v["template"] = json!(t);
        }
        if let Some(s) = self.scale {
            v["scale"] = json!(s.to_string());
        }
        v
    }

    fn from_json(v: &Value, at: &str) -> Result<Sheet> {
        let mut o = Obj::new(v, at)?;
        let name = text_of(&o.take("name")?, &format!("{at}/name"), "a sheet's name")?;
        let size = word(
            &o.take("size")?,
            &format!("{at}/size"),
            Size::parse,
            "iso_a0 to iso_a4, ansi_a to ansi_e",
        )?;
        let orientation = match o.take_opt("orientation") {
            None => Orientation::Landscape,
            Some(x) => word(
                &x,
                &format!("{at}/orientation"),
                Orientation::parse,
                "landscape, portrait",
            )?,
        };
        let template = match o.take_opt("template") {
            None => None,
            Some(t) => Some(
                t.as_str()
                    .and_then(|s| Id::parse(s, "template").ok())
                    .map(|i| i.as_str().to_owned())
                    .ok_or_else(|| invalid(&format!("{at}/template"), "a template's ID"))?,
            ),
        };
        let scale = match o.take_opt("scale") {
            None => None,
            Some(s) => Some(scale_of(&s, &format!("{at}/scale"))?),
        };
        o.finish()?;
        Ok(Sheet {
            name,
            size,
            orientation,
            template,
            scale,
        })
    }
}

impl View {
    fn to_json(&self) -> Value {
        let mut v = json!({"sheet": self.sheet.to_string(), "type": self.type_name()});
        if let Some([x, y]) = &self.at {
            v["at"] = json!([x.text(), y.text()]);
        }
        if let Some(s) = self.scale {
            v["scale"] = json!(s.to_string());
        }
        if self.hidden == Hidden::Dashed {
            v["hidden"] = json!("dashed");
        }
        match self.tangent {
            Tangent::Thin => {}
            Tangent::Off => v["tangent"] = json!("off"),
            Tangent::Phantom => v["tangent"] = json!("phantom"),
        }
        if let Some(l) = &self.label {
            v["label"] = json!(l);
        }
        match &self.kind {
            ViewKind::Base { of, label, orient } => {
                if let Some(of) = of {
                    v["of"] = json!(of);
                }
                if let Some(l) = label {
                    v["rev"] = json!({"label": l});
                }
                v["orient"] = orient_json(orient);
            }
            ViewKind::Projected { from, side, offset } => {
                v["from"] = json!(from.to_string());
                v["side"] = json!(side.as_str());
                if let Some(o) = offset {
                    v["offset"] = json!(o.text());
                }
            }
            ViewKind::Auxiliary {
                from,
                normal_to,
                offset,
            } => {
                v["from"] = json!(from.to_string());
                v["normal_to"] = normal_to.to_json();
                if let Some(o) = offset {
                    v["offset"] = json!(o.text());
                }
            }
            ViewKind::Section {
                from,
                cut,
                flip,
                unsectioned,
                offset,
            } => {
                v["from"] = json!(from.to_string());
                v["cut"] = cut.to_json();
                if *flip {
                    v["flip"] = json!(true);
                }
                if !unsectioned.is_empty() {
                    v["unsectioned"] = json!(unsectioned);
                }
                if let Some(o) = offset {
                    v["offset"] = json!(o.text());
                }
            }
            ViewKind::Detail {
                from,
                centre,
                radius,
            } => {
                v["from"] = json!(from.to_string());
                v["centre"] = centre.to_json();
                v["radius"] = json!(radius.text());
            }
            ViewKind::Flat { of, solid, notes } => {
                if let Some(of) = of {
                    v["of"] = json!(of);
                }
                if let Some(s) = solid {
                    v["solid"] = json!(s);
                }
                if !notes {
                    v["notes"] = json!(false);
                }
            }
            ViewKind::Exploded {
                of,
                explode,
                fraction,
                orient,
                trails,
            } => {
                if let Some(of) = of {
                    v["of"] = json!(of);
                }
                v["explode"] = json!(explode);
                if *fraction != 1.0 {
                    v["fraction"] = json!(fraction);
                }
                v["orient"] = orient_json(orient);
                if *trails {
                    v["trails"] = json!(true);
                }
            }
        }
        v
    }

    fn from_json(v: &Value, at: &str) -> Result<View> {
        let mut o = Obj::new(v, at)?;
        let p = |k: &str| format!("{at}/{k}");
        let sheet = key_of(&o.take("sheet")?, &p("sheet"), 's', "a view's sheet")?;
        let ty = o.take("type")?;
        let pos = match o.take_opt("at") {
            None => None,
            Some(a) => Some(position(&a, &p("at"))?),
        };
        let scale = match o.take_opt("scale") {
            None => None,
            Some(s) => Some(scale_of(&s, &p("scale"))?),
        };
        let hidden = match o.take_opt("hidden") {
            None => Hidden::Off,
            Some(h) => word(
                &h,
                &p("hidden"),
                |s| match s {
                    "off" => Some(Hidden::Off),
                    "dashed" => Some(Hidden::Dashed),
                    _ => None,
                },
                "off, dashed",
            )?,
        };
        let tangent = match o.take_opt("tangent") {
            None => Tangent::Thin,
            Some(t) => word(
                &t,
                &p("tangent"),
                |s| match s {
                    "thin" => Some(Tangent::Thin),
                    "off" => Some(Tangent::Off),
                    "phantom" => Some(Tangent::Phantom),
                    _ => None,
                },
                "thin, off, phantom",
            )?,
        };
        let label = match o.take_opt("label") {
            None => None,
            Some(l) => Some(text_of(&l, &p("label"), "a view's label")?),
        };
        let kind = match ty.as_str() {
            Some("base") => {
                let of = match o.take_opt("of") {
                    None => None,
                    Some(x) => Some(
                        x.as_str()
                            .and_then(|s| Id::parse(s, "part").ok())
                            .ok_or_else(|| invalid(&p("of"), "a part's ID"))?,
                    ),
                };
                let label = match o.take_opt("rev") {
                    None => None,
                    Some(r) => {
                        let mut r = Obj::new(&r, &p("rev"))?;
                        let l = r.take("label")?;
                        r.finish()?;
                        Some(
                            l.as_str()
                                .filter(|s| super::assembly::label_text(s).is_ok())
                                .map(str::to_owned)
                                .ok_or_else(|| {
                                    invalid(
                                        &p("rev/label"),
                                        "a label of 1 to 12 capitals and digits",
                                    )
                                })?,
                        )
                    }
                };
                let orient = orient_of(o.take("orient")?, &p("orient"))?;
                ViewKind::Base { of, label, orient }
            }
            Some("projected") => {
                let from = key_of(&o.take("from")?, &p("from"), 'v', "a view's parent")?;
                let side = word(
                    &o.take("side")?,
                    &p("side"),
                    Side::parse,
                    "top, bottom, left, right, back, iso, or a corner such as front_top_right",
                )?;
                let offset = match o.take_opt("offset") {
                    None => None,
                    Some(x) => Some(length(&x, &p("offset"))?),
                };
                match (&side, &offset, &pos) {
                    (Side::Diagonal(_), Some(_), _) => {
                        return Err(invalid(
                            &p("offset"),
                            "an isometric view is placed by its at, not an offset",
                        ));
                    }
                    (Side::Diagonal(_), None, _) | (_, _, None) => {}
                    (_, _, Some(_)) => {
                        return Err(invalid(
                            &p("at"),
                            "a projected view lies on its parent's row or column: it takes an offset, not an at",
                        ));
                    }
                }
                ViewKind::Projected { from, side, offset }
            }
            Some("auxiliary") => {
                let from = key_of(&o.take("from")?, &p("from"), 'v', "a view's parent")?;
                let normal_to = Reference::from_json(&o.take("normal_to")?, &p("normal_to"))?;
                let offset = match o.take_opt("offset") {
                    None => None,
                    Some(x) => Some(length(&x, &p("offset"))?),
                };
                if pos.is_some() {
                    return Err(invalid(
                        &p("at"),
                        "an auxiliary view lies along its direction from its parent: it takes an offset, not an at",
                    ));
                }
                ViewKind::Auxiliary {
                    from,
                    normal_to,
                    offset,
                }
            }
            Some("section") => {
                let from = key_of(&o.take("from")?, &p("from"), 'v', "a view's parent")?;
                let cut = Cut::from_json(&o.take("cut")?, &p("cut"))?;
                let flip = match o.take_opt("flip") {
                    None => false,
                    Some(f) => f
                        .as_bool()
                        .ok_or_else(|| invalid(&p("flip"), "flip is true or false"))?,
                };
                let unsectioned = match o.take_opt("unsectioned") {
                    None => Vec::new(),
                    Some(u) => {
                        let a = u.as_array().ok_or_else(|| {
                            invalid(&p("unsectioned"), "a list of instance paths")
                        })?;
                        let mut out: Vec<String> = Vec::new();
                        for (i, x) in a.iter().enumerate() {
                            let at = format!("{at}/unsectioned/{i}");
                            let path = x
                                .as_str()
                                .filter(|t| {
                                    t.split('/')
                                        .all(|step| super::id::parse_step(step, "").is_ok())
                                })
                                .ok_or_else(|| invalid(&at, "an instance path such as p1/left"))?;
                            if out.iter().any(|o| o == path) {
                                return Err(invalid(&at, "an instance path listed twice"));
                            }
                            out.push(path.to_owned());
                        }
                        out
                    }
                };
                let offset = match o.take_opt("offset") {
                    None => None,
                    Some(x) => Some(length(&x, &p("offset"))?),
                };
                if offset.is_some() && pos.is_some() {
                    return Err(invalid(
                        &p("offset"),
                        "a section is placed by its offset or its at, not both",
                    ));
                }
                ViewKind::Section {
                    from,
                    cut,
                    flip,
                    unsectioned,
                    offset,
                }
            }
            Some("detail") => {
                let from = key_of(&o.take("from")?, &p("from"), 'v', "a view's parent")?;
                let centre = model_point(&o.take("centre")?, &p("centre"))?;
                let radius = length(&o.take("radius")?, &p("radius"))?;
                if radius.meters().is_nan() || radius.meters() <= 0.0 {
                    return Err(invalid(&p("radius"), "a detail's radius is more than 0"));
                }
                ViewKind::Detail {
                    from,
                    centre,
                    radius,
                }
            }
            Some("flat") => {
                let of = match o.take_opt("of") {
                    None => None,
                    Some(x) => Some(id_of(&x, &p("of"), "part")?),
                };
                let solid = match o.take_opt("solid") {
                    None => None,
                    Some(x) => Some(id_of(&x, &p("solid"), "solid")?),
                };
                let notes = match o.take_opt("notes") {
                    None => true,
                    Some(n) => n
                        .as_bool()
                        .ok_or_else(|| invalid(&p("notes"), "notes is true or false"))?,
                };
                ViewKind::Flat { of, solid, notes }
            }
            Some("exploded") => {
                let of = match o.take_opt("of") {
                    None => None,
                    Some(x) => Some(id_of(&x, &p("of"), "part")?),
                };
                let explode = id_of(&o.take("explode")?, &p("explode"), "explode")?;
                let fraction = match o.take_opt("fraction") {
                    None => 1.0,
                    Some(f) => f
                        .as_f64()
                        .filter(|f| (0.0..=1.0).contains(f))
                        .ok_or_else(|| invalid(&p("fraction"), "a fraction from 0 to 1"))?,
                };
                let orient = orient_of(o.take("orient")?, &p("orient"))?;
                let trails = match o.take_opt("trails") {
                    None => false,
                    Some(t) => t
                        .as_bool()
                        .ok_or_else(|| invalid(&p("trails"), "trails is true or false"))?,
                };
                ViewKind::Exploded {
                    of,
                    explode,
                    fraction,
                    orient,
                    trails,
                }
            }
            _ => {
                return Err(invalid(
                    &p("type"),
                    "expected one of base, projected, auxiliary, section, detail, flat, exploded",
                ));
            }
        };
        if matches!(kind, ViewKind::Section { .. } | ViewKind::Detail { .. }) {
            match &label {
                Some(l) if letter_ok(l) => {}
                _ => {
                    return Err(invalid(
                        &p("label"),
                        "a section's or detail's label is its letter: one or two capitals, not I, O or Q",
                    ));
                }
            }
        }
        o.finish()?;
        Ok(View {
            sheet,
            at: pos,
            scale,
            hidden,
            tangent,
            label,
            kind,
        })
    }
}

impl DrawingFile {
    /// A drawing of one sheet `size` and no views.
    pub fn new(name: &str, size: Size) -> DrawingFile {
        let mut sheets = BTreeMap::new();
        sheets.insert(
            Key {
                kind: 's',
                number: 1,
            },
            Sheet {
                name: "Sheet 1".into(),
                size,
                orientation: Orientation::Landscape,
                template: None,
                scale: None,
            },
        );
        DrawingFile {
            name: name.to_owned(),
            standard: None,
            projection: None,
            unit: None,
            precision: None,
            sheets,
            views: BTreeMap::new(),
            fields: BTreeMap::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let members = |m: Vec<(String, Value)>| Value::Object(m.into_iter().collect::<Map<_, _>>());
        let mut v = json!({
            "format": FORMAT,
            "kind": KIND,
            "name": self.name,
            "sheets": members(self.sheets.iter().map(|(k, s)| (k.to_string(), s.to_json())).collect()),
            "views": members(self.views.iter().map(|(k, w)| (k.to_string(), w.to_json())).collect()),
        });
        if let Some(s) = self.standard {
            v["standard"] = json!(s.as_str());
        }
        if let Some(p) = self.projection {
            v["projection"] = json!(p.as_str());
        }
        if let Some(u) = self.unit {
            v["unit"] = json!(u);
        }
        if let Some(p) = self.precision {
            v["precision"] = json!(p);
        }
        if !self.fields.is_empty() {
            v["fields"] = json!(self.fields);
        }
        v
    }

    /// Reads a drawing: its sheets (at least one) and views by key, each
    /// view on a sheet it has, deriving from a view it has, with no cycle.
    pub fn from_json(v: &Value) -> Result<DrawingFile> {
        let mut o = header(v, KIND, FORMAT)?;
        let name = text_of(&o.take("name")?, "/name", "a drawing's name")?;
        let standard = match o.take_opt("standard") {
            None => None,
            Some(s) => Some(word(&s, "/standard", Standard::parse, "asme, iso")?),
        };
        let projection = match o.take_opt("projection") {
            None => None,
            Some(s) => Some(word(
                &s,
                "/projection",
                Angle::parse,
                "first_angle, third_angle",
            )?),
        };
        let unit = match o.take_opt("unit") {
            None => None,
            Some(u) => Some(word(&u, "/unit", Unit::parse, "m, cm, mm, um, in, ft")?),
        };
        let precision = match o.take_opt("precision") {
            None => None,
            Some(p) => Some(
                p.as_u64()
                    .filter(|p| *p <= u64::from(MAX_PRECISION))
                    .map(|p| p as u32)
                    .ok_or_else(|| invalid("/precision", "a precision is 0 to 6 decimals"))?,
            ),
        };
        let members = |v: Value, at: &str, kind: char| -> Result<Vec<(Key, Value)>> {
            let Value::Object(m) = v else {
                return Err(invalid(at, "an object of members by key"));
            };
            m.into_iter()
                .map(|(k, v)| {
                    Key::parse(&k, kind).map(|key| (key, v)).ok_or_else(|| {
                        invalid(&format!("{at}/{k}"), &format!("a key such as {kind}1"))
                    })
                })
                .collect()
        };
        let mut sheets = BTreeMap::new();
        for (k, s) in members(o.take("sheets")?, "/sheets", 's')? {
            sheets.insert(k, Sheet::from_json(&s, &format!("/sheets/{k}"))?);
        }
        if sheets.is_empty() {
            return Err(invalid("/sheets", "a drawing has one sheet or more"));
        }
        let mut views = BTreeMap::new();
        for (k, w) in members(o.take("views")?, "/views", 'v')? {
            views.insert(k, View::from_json(&w, &format!("/views/{k}"))?);
        }
        let fields = match o.take_opt("fields") {
            None => BTreeMap::new(),
            Some(Value::Object(m)) => {
                let mut out = BTreeMap::new();
                for (k, v) in m {
                    let at = format!("/fields/{k}");
                    if !field_name_ok(&k) {
                        return Err(invalid(&at, "a field's name is [a-z_][a-z0-9_]{0,62}"));
                    }
                    out.insert(k, text_of(&v, &at, "a field")?);
                }
                out
            }
            Some(_) => return Err(invalid("/fields", "an object of text by name")),
        };
        o.finish()?;
        let d = DrawingFile {
            name,
            standard,
            projection,
            unit,
            precision,
            sheets,
            views,
            fields,
        };
        d.check()?;
        Ok(d)
    }

    /// What a drawing's members must say of each other: each view's sheet
    /// and parent exist, and no view derives from itself.
    pub fn check(&self) -> Result<()> {
        let mut letters: BTreeMap<&str, Key> = BTreeMap::new();
        for (k, w) in &self.views {
            if let Some(l) = w.letter()
                && let Some(first) = letters.insert(l, *k)
            {
                return Err(invalid(
                    &format!("/views/{k}/label"),
                    &format!("view {k} takes letter {l}, which view {first} has"),
                ));
            }
        }
        for (k, w) in &self.views {
            if !self.sheets.contains_key(&w.sheet) {
                return Err(invalid(
                    &format!("/views/{k}/sheet"),
                    &format!("view {k} is on sheet {}, which the drawing hasn't", w.sheet),
                ));
            }
            if let Some(from) = w.parent()
                && !self.views.contains_key(&from)
            {
                return Err(invalid(
                    &format!("/views/{k}/from"),
                    &format!("view {k} derives from view {from}, which the drawing hasn't"),
                ));
            }
            // Walk up: a chain longer than the views is a cycle.
            let mut at = *k;
            for _ in 0..=self.views.len() {
                match self.views.get(&at).and_then(View::parent) {
                    Some(up) if up == *k => {
                        return Err(invalid(
                            &format!("/views/{k}/from"),
                            &format!("view {k} derives from itself"),
                        ));
                    }
                    Some(up) => at = up,
                    None => break,
                }
            }
        }
        Ok(())
    }

    /// The standard it follows: its own, else the project's.
    pub fn standard_in(&self, project: Standard) -> Standard {
        self.standard.unwrap_or(project)
    }

    /// The base view each view's source comes from.
    pub fn root_of(&self, mut k: Key) -> Key {
        for _ in 0..=self.views.len() {
            match self.views.get(&k).and_then(View::parent) {
                Some(up) => k = up,
                None => break,
            }
        }
        k
    }

    /// Every entity reference it holds, with its pointer.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = Vec::new();
        for (k, w) in &self.views {
            match &w.kind {
                ViewKind::Base {
                    orient: Orient::NormalTo(r),
                    ..
                } => out.push((format!("/views/{k}/orient/normal_to"), r)),
                ViewKind::Auxiliary { normal_to, .. } => {
                    out.push((format!("/views/{k}/normal_to"), normal_to));
                }
                ViewKind::Exploded {
                    orient: Orient::NormalTo(r),
                    ..
                } => out.push((format!("/views/{k}/orient/normal_to"), r)),
                ViewKind::Section { cut, .. } => match cut {
                    Cut::Plane { plane, .. } => {
                        if let Some(r) = plane.reference() {
                            out.push((format!("/views/{k}/cut/plane"), r));
                        }
                    }
                    Cut::Through(points) => {
                        for (i, pt) in points.iter().enumerate() {
                            if let ModelPoint::Ref(g) = pt
                                && let Some(r) = g.reference()
                            {
                                out.push((format!("/views/{k}/cut/through/{i}"), r));
                            }
                        }
                    }
                    Cut::At { .. } => {}
                },
                ViewKind::Detail {
                    centre: ModelPoint::Ref(g),
                    ..
                } => {
                    if let Some(r) = g.reference() {
                        out.push((format!("/views/{k}/centre"), r));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The project templates its sheets name (those that aren't built in
    /// are looked for under `templates/`).
    pub fn templates(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .sheets
            .values()
            .filter_map(|s| s.template.as_deref())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Whether `s` names a field: `[a-z_][a-z0-9_]{0,62}`.
pub fn field_name_ok(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some('a'..='z' | '_'))
        && s.len() <= 63
        && c.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bracket() -> Value {
        json!({
            "format": 1,
            "kind": "ryucad.drawing",
            "name": "Bracket",
            "standard": "iso",
            "sheets": {"s1": {"name": "Sheet 1", "size": "iso_a3", "scale": "1:2"}},
            "views": {
                "v1": {"type": "base", "sheet": "s1", "at": ["120 mm", 160], "orient": "front", "hidden": "dashed"},
                "v2": {"type": "projected", "sheet": "s1", "from": "v1", "side": "top", "offset": "70 mm"},
                "v10": {"type": "projected", "sheet": "s1", "from": "v1", "side": "iso", "at": ["330 mm", "200 mm"]},
                "v3": {"type": "auxiliary", "sheet": "s1", "from": "v1",
                       "normal_to": {"feature": "base", "name": "0123456789abcdef0123456789abcdef"}},
                "v4": {"type": "base", "sheet": "s1", "of": "pin", "rev": {"label": "B"},
                       "orient": {"normal_to": {"feature": "pin", "name": "0123456789abcdef0123456789abcdef"}}}
            },
            "fields": {"drawn_by": "agent"}
        })
    }

    #[test]
    fn a_drawing_reads_and_writes_itself() {
        let d = DrawingFile::from_json(&bracket()).unwrap();
        assert_eq!(d.views.len(), 5);
        assert_eq!(d.standard, Some(Standard::Iso));
        assert_eq!(
            d.sheets[&Key::parse("s1", 's').unwrap()].scale,
            Scale::parse("1:2")
        );
        let v1 = &d.views[&Key::parse("v1", 'v').unwrap()];
        assert_eq!(v1.at.as_ref().unwrap()[1].text(), "160 mm");
        // Keys order by their numbers: v10 after v4.
        let keys: Vec<String> = d.views.keys().map(Key::to_string).collect();
        assert_eq!(keys, ["v1", "v2", "v3", "v4", "v10"]);
        let back = DrawingFile::from_json(&d.to_json()).unwrap();
        assert_eq!(back, d);
        assert_eq!(d.references().len(), 2);
        assert_eq!(Key::next('v', d.views.keys()).to_string(), "v11");
    }

    #[test]
    fn faults_are_refused_at_their_pointers() {
        let at = |f: &dyn Fn(&mut Value)| {
            let mut v = bracket();
            f(&mut v);
            let e = DrawingFile::from_json(&v).unwrap_err();
            e.details
                .get("pointer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        assert_eq!(
            at(&|v| v["views"]["v2"]["sheet"] = json!("s9")),
            "/views/v2/sheet"
        );
        assert_eq!(
            at(&|v| v["views"]["v2"]["from"] = json!("v2")),
            "/views/v2/from"
        );
        assert_eq!(
            at(&|v| v["views"]["v2"]["from"] = json!("v7")),
            "/views/v2/from"
        );
        assert_eq!(
            at(&|v| v["sheets"]["s1"]["scale"] = json!("0:2")),
            "/sheets/s1/scale"
        );
        assert_eq!(
            at(&|v| v["sheets"]["s1"]["size"] = json!("a3")),
            "/sheets/s1/size"
        );
        assert_eq!(
            at(&|v| v["views"]["v2"]["at"] = json!([1, 2])),
            "/views/v2/at"
        );
        assert_eq!(
            at(&|v| v["views"]["v10"]["offset"] = json!(5)),
            "/views/v10/offset"
        );
        assert_eq!(
            at(&|v| v["views"]["v1"]["at"] = json!(["1 deg", 2])),
            "/views/v1/at/0"
        );
        assert_eq!(at(&|v| v["precision"] = json!(9)), "/precision");
        assert_eq!(at(&|v| v["fields"]["Bad"] = json!("x")), "/fields/Bad");
        // Two views deriving from each other.
        assert_eq!(
            at(&|v| {
                v["views"]["v1"] =
                    json!({"type": "projected", "sheet": "s1", "from": "v2", "side": "left"});
            }),
            "/views/v1/from"
        );
        let mut v = bracket();
        v["views"]["v1"]["colour"] = json!("red");
        assert_eq!(
            DrawingFile::from_json(&v).unwrap_err().code,
            Code::InvalidDocument
        );
        v = bracket();
        v["format"] = json!(2);
        assert_eq!(
            DrawingFile::from_json(&v).unwrap_err().code,
            Code::FormatTooNew
        );
    }

    #[test]
    fn sizes_scales_and_templates() {
        assert_eq!(Size::AnsiB.paper(Orientation::Landscape), [431.8, 279.4]);
        assert_eq!(Size::IsoA4.paper(Orientation::Portrait), [210.0, 297.0]);
        assert_eq!(Size::AnsiB.template(), "asme-b");
        assert_eq!(Size::IsoA3.template(), "iso-a3");
        assert_eq!(Scale::parse("2:1").unwrap().mm_per_m(), 2000.0);
        assert!(Scale::parse("1:01").is_none() && Scale::parse("1:1001").is_none());
        assert_eq!(Standard::Iso.projection(), Angle::First);
    }
}
