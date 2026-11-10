//! A drawing's annotations (design 417, plan 9.C): dimensions, hole
//! callouts, centre marks and lines, notes, balloons and BOM tables, GD&T
//! frames and datum features, and surface finish, by key in a drawing's
//! `annotations`.
//!
//! ```json
//! "annotations": {
//!   "d1": {"type": "linear", "view": "v1", "a": "base/<hex>", "b": "base/<hex>",
//!          "along": "horizontal", "offset": "10 mm", "tolerance": {"symmetric": "0.1 mm"}},
//!   "d2": {"type": "diameter", "view": "v1", "of": "base/<hex>", "tolerance": {"fit": "H7"}},
//!   "n1": {"type": "note", "sheet": "s1", "at": ["20 mm", "30 mm"], "text": "BREAK ALL EDGES"},
//!   "g1": {"type": "feature_control_frame", "view": "v1", "on": "base/<hex>",
//!          "characteristic": "position", "rows": [{"tolerance": "0.1 mm", "diameter": true,
//!          "condition": "m", "datums": ["A", "B"]}]}
//! }
//! ```
//!
//! Entities are named relative to what their view draws, by stable name
//! only, as features name them: `<solid>/<name>` in a part's view,
//! `<instance>/…/<solid>/<name>` in an assembly's, `<datum>` for a datum
//! of the view's part; a balloon names an occurrence (`<instance>/…`).
//! Keys take a letter by type: `d` dimensions and hole callouts, `c`
//! centre marks and lines, `n` notes, `b` balloons, `t` BOM tables, `g`
//! GD&T frames and datum features, `f` surface finish. Every value the
//! model decides is worked out again when the drawing is built; the file
//! holds intent and place only. An annotation on a view writes its `at`
//! relative to the view's centre (it moves with the view), a note or BOM
//! table on a sheet its `at` on the sheet.

use super::drawing::Key;
use super::drawing_template::parse_fields;
use super::files::{Obj, invalid};
use super::id::{Id, parse_step};
use crate::error::Result;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// Which way a linear or ordinate dimension measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Along {
    Horizontal,
    Vertical,
    Aligned,
}

impl Along {
    pub fn as_str(self) -> &'static str {
        match self {
            Along::Horizontal => "horizontal",
            Along::Vertical => "vertical",
            Along::Aligned => "aligned",
        }
    }

    pub fn parse(s: &str) -> Option<Along> {
        match s {
            "horizontal" => Some(Along::Horizontal),
            "vertical" => Some(Along::Vertical),
            "aligned" => Some(Along::Aligned),
            _ => None,
        }
    }
}

/// A dimension's tolerance.
#[derive(Clone, Debug, PartialEq)]
pub enum Tolerance {
    /// ± one value.
    Symmetric(Quantity),
    /// An upper and a lower deviation, shown stacked after the value.
    Deviation(Quantity, Quantity),
    /// An upper and a lower deviation, shown as the two limits.
    Limits(Quantity, Quantity),
    /// An ISO 286 class (`H7`) or a fit (`H7/g6`), its deviations worked
    /// out from the nominal size.
    Fit(String),
    /// Basic (theoretically exact): boxed.
    Basic,
    /// For reference only: in parentheses.
    Reference,
}

/// What a dimension shows besides its value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    /// Its dimension line's distance from its feature on the sheet,
    /// signed: above or right of it positive (for an aligned dimension,
    /// to the left of its direction from `a` to `b`); none to be placed.
    pub offset: Option<Quantity>,
    /// Text with `<>` once standing for the value.
    pub text: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub tolerance: Option<Tolerance>,
    /// Decimals its value shows; none for the drawing's.
    pub precision: Option<u32>,
}

/// The 14 geometric characteristics of ASME Y14.5 and ISO 1101.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Characteristic {
    Straightness,
    Flatness,
    Circularity,
    Cylindricity,
    ProfileLine,
    ProfileSurface,
    Angularity,
    Perpendicularity,
    Parallelism,
    Position,
    Concentricity,
    Symmetry,
    CircularRunout,
    TotalRunout,
}

/// Whether a characteristic names datums.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Datums {
    Never,
    Always,
    Either,
}

pub const CHARACTERISTICS: [(&str, Characteristic); 14] = [
    ("straightness", Characteristic::Straightness),
    ("flatness", Characteristic::Flatness),
    ("circularity", Characteristic::Circularity),
    ("cylindricity", Characteristic::Cylindricity),
    ("profile_line", Characteristic::ProfileLine),
    ("profile_surface", Characteristic::ProfileSurface),
    ("angularity", Characteristic::Angularity),
    ("perpendicularity", Characteristic::Perpendicularity),
    ("parallelism", Characteristic::Parallelism),
    ("position", Characteristic::Position),
    ("concentricity", Characteristic::Concentricity),
    ("symmetry", Characteristic::Symmetry),
    ("circular_runout", Characteristic::CircularRunout),
    ("total_runout", Characteristic::TotalRunout),
];

impl Characteristic {
    pub fn as_str(self) -> &'static str {
        CHARACTERISTICS
            .iter()
            .find(|(_, c)| *c == self)
            .expect("listed")
            .0
    }

    pub fn parse(s: &str) -> Option<Characteristic> {
        CHARACTERISTICS
            .iter()
            .find(|(n, _)| *n == s)
            .map(|(_, c)| *c)
    }

    /// Form tolerances name no datum; orientation, location (but
    /// position) and runout name one at least; profiles and position may
    /// go either way.
    pub fn datums(self) -> Datums {
        use Characteristic::*;
        match self {
            Straightness | Flatness | Circularity | Cylindricity => Datums::Never,
            Angularity | Perpendicularity | Parallelism | Concentricity | Symmetry
            | CircularRunout | TotalRunout => Datums::Always,
            ProfileLine | ProfileSurface | Position => Datums::Either,
        }
    }
}

/// A material condition or modifier, in a circle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Condition {
    /// Maximum material.
    M,
    /// Least material.
    L,
    /// ISO's envelope requirement.
    E,
    /// Projected tolerance zone.
    P,
    /// Free state.
    F,
    /// Tangent plane.
    T,
}

impl Condition {
    pub fn letter(self) -> char {
        match self {
            Condition::M => 'M',
            Condition::L => 'L',
            Condition::E => 'E',
            Condition::P => 'P',
            Condition::F => 'F',
            Condition::T => 'T',
        }
    }

    pub fn parse(s: &str) -> Option<Condition> {
        match s {
            "m" => Some(Condition::M),
            "l" => Some(Condition::L),
            "e" => Some(Condition::E),
            "p" => Some(Condition::P),
            "f" => Some(Condition::F),
            "t" => Some(Condition::T),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Condition::M => "m",
            Condition::L => "l",
            Condition::E => "e",
            Condition::P => "p",
            Condition::F => "f",
            Condition::T => "t",
        }
    }
}

/// One row of a feature control frame (two for a composite frame).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub tolerance: Quantity,
    /// A cylindrical zone: ⌀ before the value.
    pub diameter: bool,
    pub condition: Option<Condition>,
    /// Datum letters in order of precedence, each with its own condition
    /// (`m` or `l`).
    pub datums: Vec<(String, Option<Condition>)>,
}

/// A surface finish symbol's process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Process {
    /// Any process: the bare check mark.
    #[default]
    Any,
    /// Material removed (machined): a bar across it.
    Removal,
    /// No material removed: a circle in it.
    NoRemoval,
}

impl Process {
    pub fn as_str(self) -> &'static str {
        match self {
            Process::Any => "any",
            Process::Removal => "removal",
            Process::NoRemoval => "no_removal",
        }
    }

    pub fn parse(s: &str) -> Option<Process> {
        match s {
            "any" => Some(Process::Any),
            "removal" => Some(Process::Removal),
            "no_removal" => Some(Process::NoRemoval),
            _ => None,
        }
    }
}

/// What an annotation is.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Between `a` and `b`, or along one straight edge `a`.
    Linear {
        a: String,
        b: Option<String>,
        along: Along,
    },
    /// Between two straight edges, at `vertex` where one is named.
    Angular {
        a: String,
        b: String,
        vertex: Option<String>,
    },
    Radius {
        of: String,
    },
    Diameter {
        of: String,
    },
    /// `of`'s distance from `datum`, horizontally or vertically.
    Ordinate {
        datum: String,
        of: String,
        along: Along,
    },
    /// A chamfer's face: its sizes from its feature.
    Chamfer {
        of: String,
    },
    /// A hole's face: its sizes, form and count from its feature.
    HoleCallout {
        of: String,
    },
    CentreMark {
        of: String,
    },
    /// On a cylindrical face seen side on, or between two circular edges.
    CentreLine {
        of: String,
        to: Option<String>,
    },
    /// Text (fields `{name}` filled as the title block's), optionally
    /// with a leader to an entity.
    Note {
        text: String,
        leader: Option<String>,
    },
    /// An occurrence's item number in a circle.
    Balloon {
        of: String,
    },
    /// The view's assembly's BOM (or the drawing's own part's).
    BomTable {
        flat: bool,
        depth: Option<u32>,
        columns: Vec<String>,
    },
    FeatureControlFrame {
        characteristic: Characteristic,
        rows: Vec<Row>,
        /// The entity it controls, by a leader.
        on: Option<String>,
        /// The dimension it is attached to, under its text.
        to: Option<Key>,
    },
    DatumFeature {
        letter: String,
        on: String,
    },
    SurfaceFinish {
        on: String,
        ra: Option<Quantity>,
        process: Process,
    },
}

/// The columns a BOM table shows when it doesn't say.
pub const BOM_COLUMNS: [&str; 4] = ["item", "number", "name", "quantity"];

/// One annotation.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    /// The view it annotates (its sheet the view's).
    pub view: Option<Key>,
    /// The sheet a note or BOM table on no view stands on.
    pub sheet: Option<Key>,
    /// Its place: relative to its view's centre on a view, on the sheet
    /// otherwise; none to be placed.
    pub at: Option<[Quantity; 2]>,
    /// A dimension's or callout's style (none for other kinds).
    pub style: Option<Style>,
    pub kind: Kind,
}

/// Every type, its key letter and whether it is a dimension.
pub const TYPES: [(&str, char, bool); 15] = [
    ("linear", 'd', true),
    ("angular", 'd', true),
    ("radius", 'd', true),
    ("diameter", 'd', true),
    ("ordinate", 'd', true),
    ("chamfer", 'd', true),
    ("hole_callout", 'd', true),
    ("centre_mark", 'c', false),
    ("centre_line", 'c', false),
    ("note", 'n', false),
    ("balloon", 'b', false),
    ("bom_table", 't', false),
    ("feature_control_frame", 'g', false),
    ("datum_feature", 'g', false),
    ("surface_finish", 'f', false),
];

/// The key letters annotations take.
pub const LETTERS: [char; 7] = ['d', 'c', 'n', 'b', 't', 'g', 'f'];

/// The key letter of type `ty`.
pub fn letter_of(ty: &str) -> Option<char> {
    TYPES.iter().find(|(n, _, _)| *n == ty).map(|t| t.1)
}

/// Whether `s` is a datum feature's letter: one or two capitals, not I,
/// O or Q.
pub fn datum_letter_ok(s: &str) -> bool {
    super::drawing::letter_ok(s)
}

/// Whether `s` names an entity as an annotation writes it: a datum's ID,
/// or instance steps, a solid's founder and a stable name of 32 hex
/// digits.
pub fn entity_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    match parts.as_slice() {
        [one] => Id::parse(one, "datum").is_ok(),
        [steps @ .., solid, name] => {
            name.len() == 32
                && name
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && Id::parse(solid, "solid").is_ok()
                && steps.iter().all(|x| parse_step(x, "").is_ok())
        }
        _ => false,
    }
}

/// Whether `s` names an occurrence: instance steps.
pub fn occurrence_ok(s: &str) -> bool {
    !s.is_empty() && s.split('/').all(|x| parse_step(x, "").is_ok())
}

fn entity(v: &Value, at: &str) -> Result<String> {
    v.as_str()
        .filter(|s| entity_ok(s))
        .map(str::to_owned)
        .ok_or_else(|| {
            invalid(
                at,
                "an entity: <solid>/<name> (in an assembly <instance>/…/<solid>/<name>) or a datum's ID",
            )
        })
}

fn length(v: &Value, at: &str) -> Result<Quantity> {
    Quantity::from_json(v, Unit::Mm).map_err(|e| e.with("pointer", at))
}

fn angle(v: &Value, at: &str) -> Result<Quantity> {
    Quantity::from_json_as(v, Field::Angle, &crate::expr::no_parameters)
        .map_err(|e| e.with("pointer", at))
}

/// A tolerance value: a length, or for an angular dimension an angle.
fn amount(v: &Value, at: &str, angular: bool) -> Result<Quantity> {
    if angular { angle(v, at) } else { length(v, at) }
}

fn short_text(v: &Value, at: &str, what: &str) -> Result<String> {
    v.as_str()
        .filter(|s| !s.is_empty() && !s.contains('\n') && s.chars().count() <= 100)
        .map(str::to_owned)
        .ok_or_else(|| invalid(at, &format!("{what} is one line of 1 to 100 characters")))
}

fn flag(v: &Value, at: &str) -> Result<bool> {
    v.as_bool().ok_or_else(|| invalid(at, "true or false"))
}

impl Tolerance {
    pub fn type_name(&self) -> &'static str {
        match self {
            Tolerance::Symmetric(_) => "symmetric",
            Tolerance::Deviation(..) => "deviation",
            Tolerance::Limits(..) => "limits",
            Tolerance::Fit(_) => "fit",
            Tolerance::Basic => "basic",
            Tolerance::Reference => "reference",
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Tolerance::Symmetric(q) => json!({"symmetric": q.text()}),
            Tolerance::Deviation(u, l) => json!({"deviation": [u.text(), l.text()]}),
            Tolerance::Limits(u, l) => json!({"limits": [u.text(), l.text()]}),
            Tolerance::Fit(f) => json!({"fit": f}),
            Tolerance::Basic => json!({"basic": true}),
            Tolerance::Reference => json!({"reference": true}),
        }
    }

    fn from_json(v: &Value, at: &str, angular: bool) -> Result<Tolerance> {
        let m = v
            .as_object()
            .filter(|m| m.len() == 1)
            .ok_or_else(|| {
                invalid(
                    at,
                    "a tolerance is one of {symmetric}, {deviation}, {limits}, {fit}, {basic}, {reference}",
                )
            })?;
        let (k, x) = m.iter().next().expect("one");
        let p = format!("{at}/{k}");
        let pair = |x: &Value| -> Result<(Quantity, Quantity)> {
            let a = x
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| invalid(&p, "an upper and a lower deviation"))?;
            let (u, l) = (
                amount(&a[0], &format!("{p}/0"), angular)?,
                amount(&a[1], &format!("{p}/1"), angular)?,
            );
            if u.si() < l.si() {
                return Err(invalid(
                    &p,
                    "the upper deviation is the first, and no less than the lower",
                ));
            }
            Ok((u, l))
        };
        Ok(match k.as_str() {
            "symmetric" => {
                let q = amount(x, &p, angular)?;
                if q.si().is_nan() || q.si() <= 0.0 {
                    return Err(invalid(&p, "a symmetric tolerance is more than 0"));
                }
                Tolerance::Symmetric(q)
            }
            "deviation" => {
                let (u, l) = pair(x)?;
                Tolerance::Deviation(u, l)
            }
            "limits" => {
                let (u, l) = pair(x)?;
                Tolerance::Limits(u, l)
            }
            "fit" if !angular => Tolerance::Fit(
                x.as_str()
                    .filter(|s| fit_text_ok(s))
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        invalid(&p, "an ISO 286 class such as H7, or a fit such as H7/g6")
                    })?,
            ),
            "fit" => return Err(invalid(&p, "an angle takes no ISO 286 fit")),
            "basic" if x == &json!(true) => Tolerance::Basic,
            "reference" if x == &json!(true) => Tolerance::Reference,
            _ => {
                return Err(invalid(
                    &p,
                    "a tolerance is one of {symmetric}, {deviation}, {limits}, {fit}, {basic: true}, {reference: true}",
                ));
            }
        })
    }
}

/// Whether `s` reads as an ISO 286 class or fit: letters and a grade
/// (`H7`, `js6`, `ZC11`, `IT01` is not a class), a hole's class before a
/// shaft's for a fit. Whether the standard defines it at a size is the
/// build's to say.
pub fn fit_text_ok(s: &str) -> bool {
    // A class's letters: a fundamental deviation's.
    const LETTERS: [&str; 28] = [
        "a", "b", "c", "cd", "d", "e", "ef", "f", "fg", "g", "h", "js", "j", "k", "m", "n", "p",
        "r", "s", "t", "u", "v", "x", "y", "z", "za", "zb", "zc",
    ];
    let class = |c: &str| -> Option<bool> {
        let letters: String = c.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let grade = &c[letters.len()..];
        let grade_ok = matches!(grade, "01" | "0")
            || grade
                .parse::<u8>()
                .is_ok_and(|g| (1..=18).contains(&g) && !grade.starts_with('0'));
        let upper = letters.chars().all(|c| c.is_ascii_uppercase());
        let lower = letters.chars().all(|c| c.is_ascii_lowercase());
        let known = LETTERS.contains(&letters.to_ascii_lowercase().as_str());
        (known && grade_ok && (upper || lower)).then_some(upper)
    };
    match s.split_once('/') {
        None => class(s).is_some(),
        Some((h, sh)) => class(h) == Some(true) && class(sh) == Some(false),
    }
}

impl Style {
    fn write(&self, v: &mut Value) {
        if let Some(o) = &self.offset {
            v["offset"] = json!(o.text());
        }
        if let Some(t) = &self.text {
            v["text"] = json!(t);
        }
        if let Some(t) = &self.prefix {
            v["prefix"] = json!(t);
        }
        if let Some(t) = &self.suffix {
            v["suffix"] = json!(t);
        }
        if let Some(t) = &self.tolerance {
            v["tolerance"] = t.to_json();
        }
        if let Some(p) = self.precision {
            v["precision"] = json!(p);
        }
    }

    fn read(o: &mut Obj, at: &str, angular: bool, tolerated: bool) -> Result<Style> {
        let p = |k: &str| format!("{at}/{k}");
        let offset = match o.take_opt("offset") {
            None => None,
            Some(x) => Some(length(&x, &p("offset"))?),
        };
        let text = match o.take_opt("text") {
            None => None,
            Some(x) => {
                let t = short_text(&x, &p("text"), "a dimension's text")?;
                if t.matches("<>").count() != 1 {
                    return Err(invalid(
                        &p("text"),
                        "a dimension's text holds <> once, standing for its value",
                    ));
                }
                Some(t)
            }
        };
        let prefix = match o.take_opt("prefix") {
            None => None,
            Some(x) => Some(short_text(&x, &p("prefix"), "a prefix")?),
        };
        let suffix = match o.take_opt("suffix") {
            None => None,
            Some(x) => Some(short_text(&x, &p("suffix"), "a suffix")?),
        };
        let tolerance = match o.take_opt("tolerance") {
            None => None,
            Some(_) if !tolerated => {
                return Err(invalid(
                    &p("tolerance"),
                    "a chamfer's or hole callout's sizes are its feature's: it takes no tolerance",
                ));
            }
            Some(x) => Some(Tolerance::from_json(&x, &p("tolerance"), angular)?),
        };
        let precision = match o.take_opt("precision") {
            None => None,
            Some(x) => Some(
                x.as_u64()
                    .filter(|n| *n <= u64::from(super::drawing::MAX_PRECISION))
                    .map(|n| n as u32)
                    .ok_or_else(|| invalid(&p("precision"), "a precision is 0 to 6 decimals"))?,
            ),
        };
        Ok(Style {
            offset,
            text,
            prefix,
            suffix,
            tolerance,
            precision,
        })
    }
}

impl Row {
    fn to_json(&self) -> Value {
        let mut v = json!({"tolerance": self.tolerance.text()});
        if self.diameter {
            v["diameter"] = json!(true);
        }
        if let Some(c) = self.condition {
            v["condition"] = json!(c.as_str());
        }
        if !self.datums.is_empty() {
            v["datums"] = json!(
                self.datums
                    .iter()
                    .map(|(l, c)| match c {
                        None => json!(l),
                        Some(c) => json!({"datum": l, "condition": c.as_str()}),
                    })
                    .collect::<Vec<_>>()
            );
        }
        v
    }

    fn from_json(v: &Value, at: &str) -> Result<Row> {
        let mut o = Obj::new(v, at)?;
        let p = |k: &str| format!("{at}/{k}");
        let tolerance = length(&o.take("tolerance")?, &p("tolerance"))?;
        if tolerance.si().is_nan() || tolerance.si() < 0.0 {
            return Err(invalid(&p("tolerance"), "a tolerance zone is 0 or more"));
        }
        let diameter = match o.take_opt("diameter") {
            None => false,
            Some(x) => flag(&x, &p("diameter"))?,
        };
        let condition = match o.take_opt("condition") {
            None => None,
            Some(x) => Some(
                x.as_str()
                    .and_then(Condition::parse)
                    .ok_or_else(|| invalid(&p("condition"), "one of m, l, e, p, f, t"))?,
            ),
        };
        let mut datums = Vec::new();
        if let Some(d) = o.take_opt("datums") {
            let a = d
                .as_array()
                .filter(|a| a.len() <= 3)
                .ok_or_else(|| invalid(&p("datums"), "up to three datum letters"))?;
            for (i, x) in a.iter().enumerate() {
                let at = format!("{at}/datums/{i}");
                let (l, c) = match x {
                    Value::String(s) => (s.clone(), None),
                    Value::Object(_) => {
                        let mut d = Obj::new(x, &at)?;
                        let l = d.take("datum")?;
                        let c = d.take("condition")?;
                        d.finish()?;
                        let c = c
                            .as_str()
                            .and_then(Condition::parse)
                            .filter(|c| matches!(c, Condition::M | Condition::L))
                            .ok_or_else(|| {
                                invalid(&format!("{at}/condition"), "a datum's condition is m or l")
                            })?;
                        (l.as_str().unwrap_or_default().to_owned(), Some(c))
                    }
                    _ => (String::new(), None),
                };
                if !datum_letter_ok(&l) {
                    return Err(invalid(
                        &at,
                        "a datum letter: one or two capitals, not I, O or Q",
                    ));
                }
                if datums.iter().any(|(m, _): &(String, _)| *m == l) {
                    return Err(invalid(&at, "a datum named twice in one frame"));
                }
                datums.push((l, c));
            }
        }
        o.finish()?;
        Ok(Row {
            tolerance,
            diameter,
            condition,
            datums,
        })
    }
}

impl Annotation {
    /// Its type as files write it.
    pub fn type_name(&self) -> &'static str {
        match &self.kind {
            Kind::Linear { .. } => "linear",
            Kind::Angular { .. } => "angular",
            Kind::Radius { .. } => "radius",
            Kind::Diameter { .. } => "diameter",
            Kind::Ordinate { .. } => "ordinate",
            Kind::Chamfer { .. } => "chamfer",
            Kind::HoleCallout { .. } => "hole_callout",
            Kind::CentreMark { .. } => "centre_mark",
            Kind::CentreLine { .. } => "centre_line",
            Kind::Note { .. } => "note",
            Kind::Balloon { .. } => "balloon",
            Kind::BomTable { .. } => "bom_table",
            Kind::FeatureControlFrame { .. } => "feature_control_frame",
            Kind::DatumFeature { .. } => "datum_feature",
            Kind::SurfaceFinish { .. } => "surface_finish",
        }
    }

    /// Whether it is a dimension a frame may attach to (one with a value).
    pub fn is_dimension(&self) -> bool {
        matches!(
            self.kind,
            Kind::Linear { .. }
                | Kind::Angular { .. }
                | Kind::Radius { .. }
                | Kind::Diameter { .. }
                | Kind::Ordinate { .. }
        )
    }

    /// The entities it names, each with its role (`a`, `of`, `on`, …).
    pub fn entities(&self) -> Vec<(&'static str, &str)> {
        let mut out: Vec<(&'static str, &str)> = Vec::new();
        match &self.kind {
            Kind::Linear { a, b, .. } => {
                out.push(("a", a));
                if let Some(b) = b {
                    out.push(("b", b));
                }
            }
            Kind::Angular { a, b, vertex } => {
                out.push(("a", a));
                out.push(("b", b));
                if let Some(v) = vertex {
                    out.push(("vertex", v));
                }
            }
            Kind::Ordinate { datum, of, .. } => {
                out.push(("datum", datum));
                out.push(("of", of));
            }
            Kind::Radius { of }
            | Kind::Diameter { of }
            | Kind::Chamfer { of }
            | Kind::HoleCallout { of }
            | Kind::CentreMark { of } => out.push(("of", of)),
            Kind::CentreLine { of, to } => {
                out.push(("of", of));
                if let Some(t) = to {
                    out.push(("to", t));
                }
            }
            Kind::Note { leader, .. } => {
                if let Some(l) = leader {
                    out.push(("leader", l));
                }
            }
            Kind::FeatureControlFrame { on, .. } => {
                if let Some(o) = on {
                    out.push(("on", o));
                }
            }
            Kind::DatumFeature { on, .. } | Kind::SurfaceFinish { on, .. } => out.push(("on", on)),
            Kind::Balloon { .. } | Kind::BomTable { .. } => {}
        }
        out
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"type": self.type_name()});
        if let Some(k) = self.view {
            v["view"] = json!(k.to_string());
        }
        if let Some(k) = self.sheet {
            v["sheet"] = json!(k.to_string());
        }
        if let Some([x, y]) = &self.at {
            v["at"] = json!([x.text(), y.text()]);
        }
        match &self.kind {
            Kind::Linear { a, b, along } => {
                v["a"] = json!(a);
                if let Some(b) = b {
                    v["b"] = json!(b);
                }
                v["along"] = json!(along.as_str());
            }
            Kind::Angular { a, b, vertex } => {
                v["a"] = json!(a);
                v["b"] = json!(b);
                if let Some(x) = vertex {
                    v["vertex"] = json!(x);
                }
            }
            Kind::Radius { of }
            | Kind::Diameter { of }
            | Kind::Chamfer { of }
            | Kind::HoleCallout { of }
            | Kind::CentreMark { of }
            | Kind::Balloon { of } => v["of"] = json!(of),
            Kind::Ordinate { datum, of, along } => {
                v["datum"] = json!(datum);
                v["of"] = json!(of);
                v["along"] = json!(along.as_str());
            }
            Kind::CentreLine { of, to } => {
                v["of"] = json!(of);
                if let Some(t) = to {
                    v["to"] = json!(t);
                }
            }
            Kind::Note { text, leader } => {
                v["text"] = json!(text);
                if let Some(l) = leader {
                    v["leader"] = json!(l);
                }
            }
            Kind::BomTable {
                flat,
                depth,
                columns,
            } => {
                if *flat {
                    v["flat"] = json!(true);
                }
                if let Some(d) = depth {
                    v["depth"] = json!(d);
                }
                if columns.as_slice() != BOM_COLUMNS {
                    v["columns"] = json!(columns);
                }
            }
            Kind::FeatureControlFrame {
                characteristic,
                rows,
                on,
                to,
            } => {
                v["characteristic"] = json!(characteristic.as_str());
                v["rows"] = json!(rows.iter().map(Row::to_json).collect::<Vec<_>>());
                if let Some(o) = on {
                    v["on"] = json!(o);
                }
                if let Some(t) = to {
                    v["to"] = json!(t.to_string());
                }
            }
            Kind::DatumFeature { letter, on } => {
                v["letter"] = json!(letter);
                v["on"] = json!(on);
            }
            Kind::SurfaceFinish { on, ra, process } => {
                v["on"] = json!(on);
                if let Some(r) = ra {
                    v["ra"] = json!(r.text());
                }
                if *process != Process::Any {
                    v["process"] = json!(process.as_str());
                }
            }
        }
        if let Some(s) = &self.style {
            s.write(&mut v);
        }
        v
    }

    /// Reads annotation `key` at `at`: its type must take the key's
    /// letter.
    pub fn from_json(v: &Value, at: &str, key: Key) -> Result<Annotation> {
        let mut o = Obj::new(v, at)?;
        let p = |k: &str| format!("{at}/{k}");
        let ty = o.take("type")?;
        let ty = ty.as_str().unwrap_or_default().to_owned();
        let Some((_, letter, _)) = TYPES.iter().find(|(n, _, _)| *n == ty) else {
            return Err(invalid(
                &p("type"),
                &format!(
                    "expected one of {}",
                    TYPES.iter().map(|t| t.0).collect::<Vec<_>>().join(", ")
                ),
            ));
        };
        if *letter != key.kind {
            return Err(invalid(
                at,
                &format!("a {ty} takes a key such as {letter}1, not {key}"),
            ));
        }
        let key_of = |x: &Value, at: &str, kind: char, what: &str| -> Result<Key> {
            x.as_str()
                .and_then(|s| Key::parse(s, kind))
                .ok_or_else(|| invalid(at, &format!("{what} is a key such as {kind}1")))
        };
        let view = match o.take_opt("view") {
            None => None,
            Some(x) => Some(key_of(&x, &p("view"), 'v', "a view")?),
        };
        let sheet = match o.take_opt("sheet") {
            None => None,
            Some(x) => Some(key_of(&x, &p("sheet"), 's', "a sheet")?),
        };
        let pos = match o.take_opt("at") {
            None => None,
            Some(x) => {
                let a = x
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .ok_or_else(|| invalid(&p("at"), "a place is two lengths on the sheet"))?;
                Some([length(&a[0], &p("at/0"))?, length(&a[1], &p("at/1"))?])
            }
        };
        let take_entity = |o: &mut Obj, k: &str| -> Result<String> { entity(&o.take(k)?, &p(k)) };
        let opt_entity = |o: &mut Obj, k: &str| -> Result<Option<String>> {
            match o.take_opt(k) {
                None => Ok(None),
                Some(x) => Ok(Some(entity(&x, &p(k))?)),
            }
        };
        let along_of = |o: &mut Obj, aligned: bool| -> Result<Along> {
            match o.take_opt("along") {
                None => Ok(if aligned {
                    Along::Aligned
                } else {
                    Along::Horizontal
                }),
                Some(x) => x
                    .as_str()
                    .and_then(Along::parse)
                    .filter(|a| aligned || *a != Along::Aligned)
                    .ok_or_else(|| {
                        invalid(
                            &p("along"),
                            if aligned {
                                "horizontal, vertical or aligned"
                            } else {
                                "an ordinate dimension measures horizontal or vertical"
                            },
                        )
                    }),
            }
        };
        let kind = match ty.as_str() {
            "linear" => Kind::Linear {
                a: take_entity(&mut o, "a")?,
                b: opt_entity(&mut o, "b")?,
                along: along_of(&mut o, true)?,
            },
            "angular" => Kind::Angular {
                a: take_entity(&mut o, "a")?,
                b: take_entity(&mut o, "b")?,
                vertex: opt_entity(&mut o, "vertex")?,
            },
            "radius" => Kind::Radius {
                of: take_entity(&mut o, "of")?,
            },
            "diameter" => Kind::Diameter {
                of: take_entity(&mut o, "of")?,
            },
            "ordinate" => Kind::Ordinate {
                datum: take_entity(&mut o, "datum")?,
                of: take_entity(&mut o, "of")?,
                along: along_of(&mut o, false)?,
            },
            "chamfer" => Kind::Chamfer {
                of: take_entity(&mut o, "of")?,
            },
            "hole_callout" => Kind::HoleCallout {
                of: take_entity(&mut o, "of")?,
            },
            "centre_mark" => Kind::CentreMark {
                of: take_entity(&mut o, "of")?,
            },
            "centre_line" => Kind::CentreLine {
                of: take_entity(&mut o, "of")?,
                to: opt_entity(&mut o, "to")?,
            },
            "note" => {
                let t = o.take("text")?;
                let text = t
                    .as_str()
                    .filter(|s| {
                        !s.is_empty()
                            && s.chars().count() <= 1000
                            && s.lines().count() <= 20
                            && !s.chars().any(|c| c.is_control() && c != '\n')
                    })
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        invalid(
                            &p("text"),
                            "a note is 1 to 1000 characters on up to 20 lines",
                        )
                    })?;
                if let Err(e) = parse_fields(&text) {
                    return Err(invalid(&p("text"), &format!("a note's fields: {e}")));
                }
                Kind::Note {
                    text,
                    leader: opt_entity(&mut o, "leader")?,
                }
            }
            "balloon" => {
                let x = o.take("of")?;
                Kind::Balloon {
                    of: x
                        .as_str()
                        .filter(|s| occurrence_ok(s))
                        .map(str::to_owned)
                        .ok_or_else(|| invalid(&p("of"), "an occurrence: <instance>/…"))?,
                }
            }
            "bom_table" => {
                let flat = match o.take_opt("flat") {
                    None => false,
                    Some(x) => flag(&x, &p("flat"))?,
                };
                let depth = match o.take_opt("depth") {
                    None => None,
                    Some(x) => Some(
                        x.as_u64()
                            .filter(|d| (1..=32).contains(d))
                            .map(|d| d as u32)
                            .ok_or_else(|| invalid(&p("depth"), "a depth of 1 to 32 levels"))?,
                    ),
                };
                let columns = match o.take_opt("columns") {
                    None => BOM_COLUMNS.iter().map(|c| (*c).to_owned()).collect(),
                    Some(x) => {
                        let a = x
                            .as_array()
                            .filter(|a| !a.is_empty() && a.len() <= 20)
                            .ok_or_else(|| invalid(&p("columns"), "1 to 20 column names"))?;
                        let mut out: Vec<String> = Vec::new();
                        for (i, c) in a.iter().enumerate() {
                            let c = c
                                .as_str()
                                .filter(|s| super::drawing::field_name_ok(s))
                                .ok_or_else(|| {
                                    invalid(&format!("{at}/columns/{i}"), "a BOM column's name")
                                })?;
                            if out.iter().any(|o| o == c) {
                                return Err(invalid(
                                    &format!("{at}/columns/{i}"),
                                    "a column named twice",
                                ));
                            }
                            out.push(c.to_owned());
                        }
                        out
                    }
                };
                if flat && depth.is_some() {
                    return Err(invalid(&p("depth"), "a flat BOM has no depth"));
                }
                Kind::BomTable {
                    flat,
                    depth,
                    columns,
                }
            }
            "feature_control_frame" => {
                let c = o.take("characteristic")?;
                let characteristic =
                    c.as_str().and_then(Characteristic::parse).ok_or_else(|| {
                        invalid(
                            &p("characteristic"),
                            &format!(
                                "one of {}",
                                CHARACTERISTICS
                                    .iter()
                                    .map(|c| c.0)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        )
                    })?;
                let r = o.take("rows")?;
                let a = r
                    .as_array()
                    .filter(|a| (1..=2).contains(&a.len()))
                    .ok_or_else(|| invalid(&p("rows"), "one row, or two for a composite frame"))?;
                let rows: Vec<Row> = a
                    .iter()
                    .enumerate()
                    .map(|(i, x)| Row::from_json(x, &format!("{at}/rows/{i}")))
                    .collect::<Result<_>>()?;
                for (i, r) in rows.iter().enumerate() {
                    let ptr = format!("{at}/rows/{i}/datums");
                    match (characteristic.datums(), r.datums.is_empty()) {
                        (Datums::Never, false) => {
                            return Err(invalid(
                                &ptr,
                                &format!("{} names no datum", characteristic.as_str()),
                            ));
                        }
                        (Datums::Always, true) => {
                            return Err(invalid(
                                &ptr,
                                &format!("{} names a datum at least", characteristic.as_str()),
                            ));
                        }
                        _ => {}
                    }
                }
                let on = opt_entity(&mut o, "on")?;
                let to = match o.take_opt("to") {
                    None => None,
                    Some(x) => Some(key_of(&x, &p("to"), 'd', "the dimension a frame is under")?),
                };
                if on.is_some() == to.is_some() {
                    return Err(invalid(
                        at,
                        "a frame is on an entity (on) or under a dimension (to): one of them",
                    ));
                }
                Kind::FeatureControlFrame {
                    characteristic,
                    rows,
                    on,
                    to,
                }
            }
            "datum_feature" => {
                let l = o.take("letter")?;
                let letter = l
                    .as_str()
                    .filter(|s| datum_letter_ok(s))
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        invalid(
                            &p("letter"),
                            "a datum letter: one or two capitals, not I, O or Q",
                        )
                    })?;
                Kind::DatumFeature {
                    letter,
                    on: take_entity(&mut o, "on")?,
                }
            }
            "surface_finish" => {
                let on = take_entity(&mut o, "on")?;
                let ra = match o.take_opt("ra") {
                    None => None,
                    Some(x) => {
                        let q = Quantity::from_json(&x, Unit::Um)
                            .map_err(|e| e.with("pointer", p("ra")))?;
                        if q.si().is_nan() || q.si() <= 0.0 {
                            return Err(invalid(&p("ra"), "a roughness is more than 0"));
                        }
                        Some(q)
                    }
                };
                let process = match o.take_opt("process") {
                    None => Process::Any,
                    Some(x) => x
                        .as_str()
                        .and_then(Process::parse)
                        .ok_or_else(|| invalid(&p("process"), "any, removal or no_removal"))?,
                };
                Kind::SurfaceFinish { on, ra, process }
            }
            _ => unreachable!("listed"),
        };
        let dimension = TYPES.iter().find(|t| t.0 == ty).is_some_and(|t| t.2);
        let style = if dimension {
            let tolerated = !matches!(kind, Kind::Chamfer { .. } | Kind::HoleCallout { .. });
            Some(Style::read(
                &mut o,
                at,
                matches!(kind, Kind::Angular { .. }),
                tolerated,
            )?)
        } else {
            None
        };
        // An ISO 286 fit is a feature of size's: a diameter's, a width's,
        // never a radius's, whose deviations would be read at the radius as
        // if it were the nominal size.
        if matches!(kind, Kind::Radius { .. })
            && matches!(
                style.as_ref().and_then(|s| s.tolerance.as_ref()),
                Some(Tolerance::Fit(_))
            )
        {
            return Err(invalid(
                &p("tolerance"),
                "a radius takes no ISO 286 fit: fits are for features of size (dimension the diameter)",
            ));
        }
        o.finish()?;
        let on_sheet = matches!(kind, Kind::Note { .. } | Kind::BomTable { .. });
        match (view, sheet) {
            (Some(_), Some(_)) => {
                return Err(invalid(
                    &p("sheet"),
                    "an annotation on a view stands on the view's sheet: view or sheet, not both",
                ));
            }
            (None, Some(_)) if !on_sheet => {
                return Err(invalid(&p("view"), &format!("a {ty} annotates a view")));
            }
            (None, None) if !on_sheet => {
                return Err(invalid(&p("view"), &format!("a {ty} annotates a view")));
            }
            (None, None) => {
                return Err(invalid(
                    &p("sheet"),
                    &format!("a {ty} stands on a view or a sheet"),
                ));
            }
            _ => {}
        }
        if matches!(
            kind,
            Kind::Note {
                leader: Some(_),
                ..
            }
        ) && view.is_none()
        {
            return Err(invalid(
                &p("leader"),
                "a note's leader names an entity of a view: it annotates the view",
            ));
        }
        Ok(Annotation {
            view,
            sheet,
            at: pos,
            style,
            kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    const N: &str = "0123456789abcdef0123456789abcdef";

    fn k(s: &str) -> Key {
        Key::parse(s, s.chars().next().unwrap()).unwrap()
    }

    #[test]
    fn every_type_reads_and_writes_itself() {
        let e = format!("base/{N}");
        let all = vec![
            (
                "d1",
                json!({"type": "linear", "view": "v1", "a": e, "b": e, "along": "horizontal",
                "offset": "10 mm", "at": ["0 mm", "12 mm"], "tolerance": {"symmetric": "0.1 mm"}, "precision": 3}),
            ),
            (
                "d2",
                json!({"type": "angular", "view": "v1", "a": e, "b": e,
                "tolerance": {"deviation": ["0.5 deg", "0 deg"]}}),
            ),
            (
                "d3",
                json!({"type": "diameter", "view": "v1", "of": e, "tolerance": {"fit": "H7/g6"}, "text": "4× <>"}),
            ),
            (
                "d4",
                json!({"type": "radius", "view": "v1", "of": e, "prefix": "TYP", "tolerance": {"reference": true}}),
            ),
            (
                "d5",
                json!({"type": "ordinate", "view": "v1", "datum": e, "of": "x1", "along": "vertical"}),
            ),
            ("d6", json!({"type": "chamfer", "view": "v1", "of": e})),
            (
                "d7",
                json!({"type": "hole_callout", "view": "v1", "of": format!("p1/rig:2/base/{N}")}),
            ),
            ("c1", json!({"type": "centre_mark", "view": "v1", "of": e})),
            (
                "c2",
                json!({"type": "centre_line", "view": "v1", "of": e, "to": e}),
            ),
            (
                "n1",
                json!({"type": "note", "sheet": "s1", "at": ["20 mm", "30 mm"], "text": "ALL {material}\nBREAK EDGES"}),
            ),
            (
                "b1",
                json!({"type": "balloon", "view": "v1", "of": "p1/left"}),
            ),
            (
                "t1",
                json!({"type": "bom_table", "sheet": "s1", "flat": true, "columns": ["item", "name"]}),
            ),
            (
                "g1",
                json!({"type": "feature_control_frame", "view": "v1", "on": e, "characteristic": "position",
                "rows": [{"tolerance": "0.1 mm", "diameter": true, "condition": "m",
                          "datums": ["A", {"datum": "B", "condition": "m"}]},
                         {"tolerance": "0.05 mm", "datums": ["A"]}]}),
            ),
            (
                "g2",
                json!({"type": "datum_feature", "view": "v1", "letter": "A", "on": e}),
            ),
            (
                "f1",
                json!({"type": "surface_finish", "view": "v1", "on": e, "ra": "1.6 um", "process": "removal"}),
            ),
        ];
        for (key, v) in all {
            let a = Annotation::from_json(&v, "/annotations/x", k(key))
                .unwrap_or_else(|e| panic!("{key}: {e}"));
            assert_eq!(a.to_json(), v, "{key}");
        }
    }

    #[test]
    fn faults_are_refused_at_their_pointers() {
        let e = format!("base/{N}");
        let cases = [
            (
                "d1",
                json!({"type": "linear", "view": "v1", "a": "base/zz"}),
                "/x/a",
            ),
            ("c1", json!({"type": "linear", "view": "v1", "a": e}), "/x"),
            ("d1", json!({"type": "linear", "a": e}), "/x/view"),
            (
                "d1",
                json!({"type": "linear", "view": "v1", "a": e, "text": "no value"}),
                "/x/text",
            ),
            (
                "d1",
                json!({"type": "linear", "view": "v1", "a": e, "tolerance": {"fit": "H7/H6"}}),
                "/x/tolerance/fit",
            ),
            (
                "d1",
                json!({"type": "linear", "view": "v1", "a": e, "tolerance": {"deviation": ["-0.1 mm", "0.1 mm"]}}),
                "/x/tolerance/deviation",
            ),
            (
                "d1",
                json!({"type": "angular", "view": "v1", "a": e, "b": e, "tolerance": {"fit": "H7"}}),
                "/x/tolerance/fit",
            ),
            (
                "d1",
                json!({"type": "chamfer", "view": "v1", "of": e, "tolerance": {"basic": true}}),
                "/x/tolerance",
            ),
            (
                "d1",
                json!({"type": "radius", "view": "v1", "of": e, "tolerance": {"fit": "H7"}}),
                "/x/tolerance",
            ),
            (
                "d1",
                json!({"type": "ordinate", "view": "v1", "datum": e, "of": e, "along": "aligned"}),
                "/x/along",
            ),
            (
                "g1",
                json!({"type": "feature_control_frame", "view": "v1", "on": e, "characteristic": "flatness",
                "rows": [{"tolerance": "0.1 mm", "datums": ["A"]}]}),
                "/x/rows/0/datums",
            ),
            (
                "g1",
                json!({"type": "feature_control_frame", "view": "v1", "on": e, "characteristic": "parallelism",
                "rows": [{"tolerance": "0.1 mm"}]}),
                "/x/rows/0/datums",
            ),
            (
                "g1",
                json!({"type": "feature_control_frame", "view": "v1", "characteristic": "flatness",
                "rows": [{"tolerance": "0.1 mm"}]}),
                "/x",
            ),
            (
                "g1",
                json!({"type": "datum_feature", "view": "v1", "letter": "I", "on": e}),
                "/x/letter",
            ),
            (
                "n1",
                json!({"type": "note", "sheet": "s1", "text": "{unclosed"}),
                "/x/text",
            ),
            (
                "n1",
                json!({"type": "note", "sheet": "s1", "text": "a", "leader": e}),
                "/x/leader",
            ),
            (
                "t1",
                json!({"type": "bom_table", "sheet": "s1", "flat": true, "depth": 2}),
                "/x/depth",
            ),
            (
                "b1",
                json!({"type": "balloon", "view": "v1", "sheet": "s1", "of": "p1"}),
                "/x/sheet",
            ),
        ];
        for (key, v, pointer) in cases {
            let e = Annotation::from_json(&v, "/x", k(key)).expect_err(&v.to_string());
            assert_eq!(e.code, Code::InvalidDocument, "{v}");
            assert_eq!(e.details["pointer"], pointer, "{v}: {e}");
        }
    }

    #[test]
    fn fits_read_as_classes() {
        for ok in ["H7", "g6", "js6", "JS11", "ZC10", "H7/g6", "h01"] {
            assert!(fit_text_ok(ok), "{ok}");
        }
        for bad in [
            "H", "7", "H19", "H07", "Hx7", "h7/H6", "H7/G6", "abc7", "IT01/h6", "Q7",
        ] {
            assert!(!fit_text_ok(bad), "{bad}");
        }
    }
}
