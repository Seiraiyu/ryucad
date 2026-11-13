//! A hole's sizes from what it says it is: `derive(hole, tables)` is
//! pure, reading only the hole's fields and the standards (ryu-standards'
//! built-ins and the project's `hole_standards.json`). Every size comes
//! back with its source. The file never holds a derived size; a written
//! one overrides (`source: override`).
//!
//! Lengths from tables are exact nanometres until here, and become f64
//! metres by one division each.

use crate::doc::hole::{
    BoreTolerance, Far, Fastener, Hole, HoleForm, TapDrill, Thread, ThreadDepth,
};
use crate::doc::hole_standards::{Diameters, HOLE_STANDARDS, HoleStandards, Kind, ThreadEntry};
use crate::error::{Code, Error, Result};
use crate::units::Quantity;
use ryu_standards::thread::{Pitch, Standard};
use serde_json::{Map, Value, json};

/// Where a size came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A built-in value: a formula's, a public-domain document's, a fact.
    Standard(ryu_standards::Source),
    /// ryucad's tap-drill rule.
    Rule,
    /// The project's `hole_standards.json`.
    Project,
    /// Written in the hole's file.
    Override,
    /// ISO 286.
    Fit,
    /// What the hole takes when it writes none and no table gives one.
    Default,
}

impl Source {
    pub fn kind(self) -> &'static str {
        match self {
            Source::Standard(s) => s.kind(),
            Source::Rule => "rule",
            Source::Project => "project",
            Source::Override => "override",
            Source::Fit => "fit",
            Source::Default => "default",
        }
    }
}

/// One size: in metres (or radians for an angle), its exact text, and
/// its source.
#[derive(Clone, Debug, PartialEq)]
pub struct Size {
    pub si: f64,
    pub text: String,
    pub angle: bool,
    pub source: Source,
}

impl Size {
    /// A table's length: exact nanometres, written in inches when `inch`
    /// and exact there, else millimetres.
    pub fn nm(nm: i64, inch: bool, source: Source) -> Size {
        let text = match inch.then(|| ryu_standards::in_decimal(nm)).flatten() {
            Some(t) => format!("{t} in"),
            None => format!("{} mm", ryu_standards::mm_decimal(nm)),
        };
        Size {
            si: nm as f64 / 1e9,
            text,
            angle: false,
            source,
        }
    }

    /// A written quantity: a length, or an angle.
    pub fn quantity(q: &Quantity, source: Source) -> Size {
        let angle = q.dim() == crate::units::Dim::ANGLE;
        Size {
            si: q.si(),
            text: q.text(),
            angle,
            source,
        }
    }

    /// The nanometres of a length, rounded: exact for any decimal written
    /// to the nanometre.
    pub fn to_nm(&self) -> i64 {
        (self.si * 1e9).round() as i64
    }

    pub fn to_json(&self) -> Value {
        let key = if self.angle { "rad" } else { "m" };
        let mut v = json!({"value": self.text, "source": self.source.kind()});
        v[key] = json!(self.si);
        if let Source::Standard(s) = self.source {
            v["from"] = json!(s.text());
        }
        v
    }
}

/// A thread's derived sizes (Task 20).
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadSizes {
    pub standard: String,
    pub size: String,
    /// `M6x1.0-6H`, `1/4-20 UNC-2B`: the spec without depth.
    pub designation: String,
    pub major: Option<Size>,
    pub pitch: Size,
    /// Threads per inch, for inch and pipe threads.
    pub tpi: Option<String>,
    pub lead: Size,
    pub class: Option<String>,
    pub minor: Option<[Size; 2]>,
    pub major_min: Option<Size>,
    pub left: bool,
    pub starts: u32,
    /// None is full depth.
    pub depth: Option<Size>,
    pub taper: Option<(u32, u32)>,
    /// A tapered wall's gauge plane: its diameter, and its depth from
    /// the mouth (the project's `gauge_diameter`, `gauge_length`).
    pub gauge: Option<(Size, Size)>,
}

/// The bore's tolerance, worked out.
#[derive(Clone, Debug, PartialEq)]
pub struct FitSizes {
    /// The ISO 286 class, when one.
    pub class: Option<String>,
    pub upper: Size,
    pub lower: Size,
    /// Least and greatest diameter.
    pub limits: [Size; 2],
}

/// A far side's sizes.
#[derive(Clone, Debug, PartialEq)]
pub enum FarSizes {
    /// Diameter, included angle.
    Countersink(Size, Size),
    /// Diameter, depth.
    Counterbore(Size, Size),
}

/// An angle the hole takes when it writes none.
fn default_angle(deg: u32) -> Size {
    Size {
        si: f64::from(deg) * core::f64::consts::PI / 180.0,
        text: format!("{deg} deg"),
        angle: true,
        source: Source::Default,
    }
}

/// Every size a hole is built from.
#[derive(Clone, Debug, PartialEq)]
pub struct Sizes {
    pub drill: Size,
    /// Whether the drill is a standard drill (a tap-drill rule's computed
    /// size when none qualifies is not).
    pub drill_standard: bool,
    /// Diameter and depth.
    pub counterbore: Option<(Size, Size)>,
    /// Diameter and included angle.
    pub countersink: Option<(Size, Size)>,
    /// Diameter, depth and the floor's included angle.
    pub counterdrill: Option<(Size, Size, Size)>,
    /// Diameter and depth (0 for flush).
    pub spotface: Option<(Size, Size)>,
    /// A near chamfer's diameter and included angle.
    pub near: Option<(Size, Size)>,
    pub far: Option<FarSizes>,
    pub thread: Option<ThreadSizes>,
    pub fit: Option<FitSizes>,
}

impl Sizes {
    /// How deep the mouth goes, in metres; none for a simple hole.
    pub fn mouth_depth(&self) -> Option<f64> {
        let tan = |a: &Size| ryukei::math::tan(a.si / 2.0);
        if let Some((_, depth)) = &self.counterbore {
            return Some(depth.si);
        }
        if let Some((d, depth, a)) = &self.counterdrill {
            return Some(depth.si + (d.si - self.drill.si) / 2.0 / tan(a));
        }
        if let Some((_, depth)) = &self.spotface {
            return Some(depth.si);
        }
        let (d, a) = self.countersink.as_ref()?;
        Some((d.si - self.drill.si) / 2.0 / tan(a))
    }

    /// The radius a near chamfer chamfers: the counterbore's or
    /// counterdrill's, else the drill's.
    pub fn near_under(&self) -> f64 {
        self.counterbore
            .as_ref()
            .map(|(d, _)| d.si)
            .or(self.counterdrill.as_ref().map(|(d, ..)| d.si))
            .unwrap_or(self.drill.si)
            / 2.0
    }

    /// Where a near chamfer ends, from the mouth: at a spotface's floor,
    /// else at the top.
    pub fn near_end(&self) -> Option<f64> {
        let (d, a) = self.near.as_ref()?;
        let top = self.spotface.as_ref().map_or(0.0, |(_, depth)| depth.si);
        Some(top + (d.si / 2.0 - self.near_under()) / ryukei::math::tan(a.si / 2.0))
    }

    /// Where the bore at full diameter starts, from the mouth: below its
    /// mouth form and a near chamfer on the drill (a thread starts here).
    pub fn thread_start(&self) -> f64 {
        let mouth = self.mouth_depth().unwrap_or(0.0);
        let on_drill = self.counterbore.is_none() && self.counterdrill.is_none();
        match self.near_end() {
            Some(e) if on_drill => mouth.max(e),
            _ => mouth,
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = Map::new();
        v.insert("drill".into(), self.drill.to_json());
        if !self.drill_standard {
            v.insert("drill_standard".into(), json!(false));
        }
        if let Some((d, h)) = &self.counterbore {
            v.insert(
                "counterbore".into(),
                json!({"diameter": d.to_json(), "depth": h.to_json()}),
            );
        }
        if let Some((d, a)) = &self.countersink {
            v.insert(
                "countersink".into(),
                json!({"diameter": d.to_json(), "angle": a.to_json()}),
            );
        }
        if let Some((d, h, a)) = &self.counterdrill {
            v.insert(
                "counterdrill".into(),
                json!({"diameter": d.to_json(), "depth": h.to_json(), "angle": a.to_json()}),
            );
        }
        if let Some((d, h)) = &self.spotface {
            v.insert(
                "spotface".into(),
                json!({"diameter": d.to_json(), "depth": h.to_json()}),
            );
        }
        if let Some((d, a)) = &self.near {
            v.insert(
                "near".into(),
                json!({"diameter": d.to_json(), "angle": a.to_json()}),
            );
        }
        match &self.far {
            Some(FarSizes::Countersink(d, a)) => {
                v.insert(
                    "far".into(),
                    json!({"countersink": {"diameter": d.to_json(), "angle": a.to_json()}}),
                );
            }
            Some(FarSizes::Counterbore(d, h)) => {
                v.insert(
                    "far".into(),
                    json!({"counterbore": {"diameter": d.to_json(), "depth": h.to_json()}}),
                );
            }
            None => {}
        }
        if let Some(t) = &self.thread {
            let mut x = json!({
                "standard": t.standard, "size": t.size, "designation": t.designation,
                "pitch": t.pitch.to_json(), "lead": t.lead.to_json(),
                "hand": if t.left { "left" } else { "right" }, "starts": t.starts,
            });
            if let Some(m) = &t.major {
                x["major"] = m.to_json();
            }
            if let Some(n) = &t.tpi {
                x["tpi"] = json!(n);
            }
            if let Some(c) = &t.class {
                x["class"] = json!(c);
            }
            let mut limits = Map::new();
            if let Some(m) = &t.major_min {
                limits.insert("major".into(), json!([m.to_json(), null]));
            }
            if let Some([lo, hi]) = &t.minor {
                limits.insert("minor".into(), json!([lo.to_json(), hi.to_json()]));
            }
            if !limits.is_empty() {
                x["limits"] = Value::Object(limits);
            }
            x["depth"] = t.depth.as_ref().map_or(json!("full"), Size::to_json);
            if let Some((a, b)) = t.taper {
                x["taper"] = json!(format!("{a}:{b}"));
            }
            if let Some((d, l)) = &t.gauge {
                x["gauge"] = json!({"diameter": d.to_json(), "depth": l.to_json()});
            }
            v.insert("thread".into(), x);
        }
        if let Some(f) = &self.fit {
            let mut x = json!({"upper": f.upper.to_json(), "lower": f.lower.to_json(),
                               "limits": [f.limits[0].to_json(), f.limits[1].to_json()]});
            if let Some(c) = &f.class {
                x["class"] = json!(c);
            }
            v.insert("fit".into(), x);
        }
        Value::Object(v)
    }
}

/// The project's tables, as a part sees them.
pub type Tables<'a> = Option<&'a std::result::Result<HoleStandards, Error>>;

/// A hole's sizes.
pub fn derive(h: &Hole, tables: Tables) -> Result<Sizes> {
    let written = |q: &Option<Quantity>| q.as_ref().map(|q| Size::quantity(q, Source::Override));
    let (mut counterbore, mut countersink, mut counterdrill_mouth, mut spotface_mouth) =
        match &h.form {
            HoleForm::Simple => (None, None, None, None),
            HoleForm::Counterbore { diameter, depth } => {
                (written(diameter).zip(written(depth)), None, None, None)
            }
            HoleForm::Countersink { diameter, angle } => {
                (None, written(diameter).zip(written(angle)), None, None)
            }
            HoleForm::Counterdrill {
                diameter, depth, ..
            } => (None, None, written(diameter).zip(written(depth)), None),
            HoleForm::Spotface { diameter, .. } => (None, None, None, written(diameter)),
        };
    let mut thread = None;
    let (drill, drill_standard) = if let Some(t) = &h.thread {
        let (sizes, tap) = thread_sizes(h, t, tables)?;
        let inch = sizes.tpi.is_some();
        thread = Some(sizes);
        match (&h.diameter, tap) {
            (Some(q), _) => (Size::quantity(q, Source::Override), is_standard(q, inch)),
            (None, Some(d)) => d,
            (None, None) => unreachable!("thread_sizes refuses a hole with no drill"),
        }
    } else if let Some(f) = &h.fastener {
        let m = fastener_sizes(h, f, tables)?;
        counterbore = m.counterbore;
        countersink = m.countersink;
        counterdrill_mouth = m.counterdrill;
        spotface_mouth = m.spotface;
        match (&h.diameter, m.clearance) {
            (Some(q), _) => (
                Size::quantity(q, Source::Override),
                is_standard(q, false) || is_standard(q, true),
            ),
            (None, Some(c)) => {
                let standard = is_standard_size(&c);
                (c, standard)
            }
            (None, None) => unreachable!("fastener_sizes refuses a hole with no clearance"),
        }
    } else {
        let q = h
            .diameter
            .as_ref()
            .expect("a plain hole writes its diameter");
        (Size::quantity(q, Source::Override), true)
    };
    let fit = h.bore.as_ref().map(|b| fit_sizes(b, &drill)).transpose()?;
    let counterdrill = match (&h.form, counterdrill_mouth) {
        (HoleForm::Counterdrill { angle, .. }, Some((d, depth))) => Some((
            d,
            depth,
            angle.as_ref().map_or_else(
                || default_angle(118),
                |a| Size::quantity(a, Source::Override),
            ),
        )),
        _ => None,
    };
    let spotface = match (&h.form, spotface_mouth) {
        (HoleForm::Spotface { depth, .. }, Some(d)) => Some((
            d,
            depth.as_ref().map_or_else(
                || Size {
                    si: 0.0,
                    text: "0 mm".into(),
                    angle: false,
                    source: Source::Default,
                },
                |q| Size::quantity(q, Source::Override),
            ),
        )),
        _ => None,
    };
    let near = h.near.as_ref().map(|n| {
        (
            Size::quantity(&n.diameter, Source::Override),
            n.angle.as_ref().map_or_else(
                || default_angle(90),
                |a| Size::quantity(a, Source::Override),
            ),
        )
    });
    let far = h.far.as_ref().map(|f| match f {
        Far::Countersink { diameter, angle } => FarSizes::Countersink(
            Size::quantity(diameter, Source::Override),
            Size::quantity(angle, Source::Override),
        ),
        Far::Counterbore { diameter, depth } => FarSizes::Counterbore(
            Size::quantity(diameter, Source::Override),
            Size::quantity(depth, Source::Override),
        ),
    });
    let sizes = Sizes {
        drill,
        drill_standard,
        counterbore,
        countersink,
        counterdrill,
        spotface,
        near,
        far,
        thread,
        fit,
    };
    wider_than_what_they_cut(&sizes)?;
    Ok(sizes)
}

/// Every mouth, near and far size wider than what it widens, now that a
/// derived drill is known (a written one was checked when read).
fn wider_than_what_they_cut(s: &Sizes) -> Result<()> {
    let refuse = |at: &str, what: &str, d: &Size, under: f64| {
        Err(Error::new(
            Code::InvalidParam,
            format!(
                "{what} {} is no wider than the {} m it widens",
                d.text, under
            ),
        )
        .with("pointer", at)
        .with("value", d.text.clone()))
    };
    let r = s.drill.si;
    for (at, what, d) in [
        (
            "/counterdrill/diameter",
            "a counterdrill",
            s.counterdrill.as_ref().map(|x| &x.0),
        ),
        (
            "/spotface/diameter",
            "a spotface",
            s.spotface.as_ref().map(|x| &x.0),
        ),
        (
            "/counterbore/diameter",
            "a counterbore",
            s.counterbore.as_ref().map(|x| &x.0),
        ),
        (
            "/countersink/diameter",
            "a countersink",
            s.countersink.as_ref().map(|x| &x.0),
        ),
    ] {
        if let Some(d) = d
            && d.si <= r
        {
            return refuse(at, what, d, r);
        }
    }
    if let Some((d, _)) = &s.near
        && d.si / 2.0 <= s.near_under()
    {
        return refuse("/near/diameter", "a near chamfer", d, s.near_under() * 2.0);
    }
    let far = match &s.far {
        Some(FarSizes::Countersink(d, _)) => Some(("/far/countersink/diameter", d)),
        Some(FarSizes::Counterbore(d, _)) => Some(("/far/counterbore/diameter", d)),
        None => None,
    };
    if let Some((at, d)) = far
        && d.si <= r
    {
        return refuse(at, "a far side", d, r);
    }
    Ok(())
}

/// `needs_project_table`: the values a hole needs that no built-in and
/// no project entry gives, with the entry to add, each missing value null.
pub fn needs_table(kind: Kind, standard: &str, size: &str, missing: Vec<String>) -> Error {
    let mut entry = Map::new();
    for m in &missing {
        match m.split_once('.') {
            Some((a, b)) => {
                let o = entry.entry(a.to_owned()).or_insert_with(|| json!({}));
                o[b] = Value::Null;
            }
            None => {
                entry.insert(m.clone(), Value::Null);
            }
        }
    }
    let skeleton = json!({kind.key(): {standard: {size: entry}}});
    Error::new(
        Code::InvalidParam,
        format!(
            "{standard} {size} needs {} from {HOLE_STANDARDS}: ryucad doesn't ship it; add the entry given",
            missing.join(", ")
        ),
    )
    .with("reason", "needs_project_table")
    .with("standard", standard)
    .with("size", size)
    .with("missing", missing)
    .with("file", HOLE_STANDARDS)
    .with("entry", skeleton)
}

/// The project's table when a value is wanted from it: none when the
/// project has none, its error when it can't be read.
pub(crate) fn table<'a>(tables: Tables<'a>) -> Result<Option<&'a HoleStandards>> {
    match tables {
        None => Ok(None),
        Some(Ok(t)) => Ok(Some(t)),
        Some(Err(e)) => Err(e.clone()),
    }
}

fn nm(q: &Quantity) -> i64 {
    (q.si() * 1e9).round() as i64
}

/// A pitch as a size: millimetres, or 1/tpi inch.
pub(crate) fn pitch_size(p: Pitch, source: Source) -> Size {
    match p {
        Pitch::Mm(n) => Size::nm(n, false, source),
        Pitch::Tpi(_) | Pitch::Tpi10(_) => {
            let (num, den) = p.nm();
            Size {
                si: num as f64 / den as f64 / 1e9,
                text: format!("1/{} in", p.text()),
                angle: false,
                source,
            }
        }
    }
}

/// The spec's designation of a thread, without its depth: `M6x1.0-6H`,
/// `1/4-20 UNC-2B`, `1/8-27 NPT`, `G1/4`, `THREAD HOUSE_ACME T10`.
pub fn designation(
    standard: &str,
    size: &str,
    pitch: Option<Pitch>,
    class: Option<&str>,
    left: bool,
) -> String {
    let mut s = match (Standard::parse(standard), pitch) {
        (Some(Standard::IsoMetric), Some(p)) => format!("{size}x{}", p.text()),
        (Some(u @ (Standard::Unc | Standard::Unf | Standard::Unef | Standard::Un)), Some(p)) => {
            format!("{size}-{} {}", p.text(), u.key().to_uppercase())
        }
        (Some(f @ (Standard::Npt | Standard::Nptf | Standard::Nps)), Some(p)) => {
            let word = if f == Standard::Nps {
                "NPSM".to_owned()
            } else {
                f.key().to_uppercase()
            };
            format!("{size}-{} {word}", p.text())
        }
        (Some(Standard::Bspt | Standard::Bspp), _) => size.to_owned(),
        _ => format!("THREAD {} {size}", standard.to_uppercase()),
    };
    match (Standard::parse(standard), class) {
        (Some(_), Some(c)) => s.push_str(&format!("-{c}")),
        (None, Some(c)) => s.push_str(&format!(" CLASS {c}")),
        _ => {}
    }
    if left {
        s.push_str(if Standard::parse(standard).is_some() {
            "-LH"
        } else {
            " LH"
        });
    }
    s
}

/// A thread's sizes, and the tap drill its `drill` gives (none when the
/// hole writes its diameter and nothing else gives one).
fn thread_sizes(
    h: &Hole,
    t: &Thread,
    tables: Tables,
) -> Result<(ThreadSizes, Option<(Size, bool)>)> {
    let built = crate::doc::hole::resolve(t)?;
    let project = table(tables)?;
    let entry: Option<&ThreadEntry> = project.and_then(|p| p.thread(&t.standard, &t.size));
    let inch = match Standard::parse(&t.standard) {
        Some(s) => s.inch(),
        None => entry.is_some_and(|e| e.tpi.is_some()),
    };
    let std_src = |s: ryu_standards::Source| Source::Standard(s);
    // What the thread is: from the built-in, else the project's entry.
    let (major, pitch, class, minor, major_min, taper) = match &built {
        Some(b) => (
            b.major.map(|m| Size::nm(m.value, inch, std_src(m.source))),
            b.pitch.value,
            b.class.clone(),
            b.minor.map(|(lo, hi)| {
                [
                    Size::nm(lo.value, inch, std_src(lo.source)),
                    Size::nm(hi.value, inch, std_src(hi.source)),
                ]
            }),
            b.major_min
                .map(|m| Size::nm(m.value, inch, std_src(m.source))),
            b.taper,
        ),
        None => {
            let Some(e) = entry else {
                return Err(needs_table(
                    Kind::Thread,
                    &t.standard,
                    &t.size,
                    vec!["pitch".into(), "major".into(), "tap_drill".into()],
                ));
            };
            let pitch = match (&e.pitch, &e.tpi) {
                (Some(p), _) => Pitch::Mm(nm(p)),
                (None, Some(n)) => Pitch::Tpi10((n.number() * 10.0).round() as u32),
                (None, None) => unreachable!("a project size has a pitch or tpi"),
            };
            let limits = |d: &Option<Diameters>| match d {
                Some(Diameters::Classes(m)) => t.class.as_ref().and_then(|c| m.get(c)).cloned(),
                _ => None,
            };
            let basic = |d: &Option<Diameters>| match d {
                Some(Diameters::Basic(q)) => Some(Size::quantity(q, Source::Project)),
                _ => None,
            };
            (
                basic(&e.major),
                pitch,
                t.class.clone(),
                limits(&e.minor).map(|[a, b]| {
                    [
                        Size::quantity(&a, Source::Project),
                        Size::quantity(&b, Source::Project),
                    ]
                }),
                limits(&e.major).map(|[a, _]| Size::quantity(&a, Source::Project)),
                None,
            )
        }
    };
    let pitch_src = built
        .as_ref()
        .map_or(Source::Project, |b| std_src(b.pitch.source));
    let starts = t.starts.unwrap_or(1);
    // The lead is the pitch times the starts: exact in millimetres, and
    // `starts/tpi in` for inch threads.
    let lead = match pitch {
        Pitch::Mm(n) => Size::nm(n * i64::from(starts), false, pitch_src),
        p => {
            let one = pitch_size(p, pitch_src);
            Size {
                si: one.si * f64::from(starts),
                text: format!("{starts}/{} in", p.text()),
                ..one
            }
        }
    };
    let depth = match &t.depth {
        Some(ThreadDepth::Length(q)) => Some(Size::quantity(q, Source::Override)),
        _ => None,
    };
    // The tap drill.
    let major_nm = major.as_ref().map(Size::to_nm);
    let minor_nm = minor.as_ref().map(|[a, b]| (a.to_nm(), b.to_nm()));
    let tap =
        |d: ryu_standards::tap::TapDrill, src: Source| (Size::nm(d.nm, inch, src), d.standard);
    let drill = match t.drill {
        TapDrill::Rule => match entry.and_then(|e| e.tap_drill.as_ref()) {
            Some(q) => Some((Size::quantity(q, Source::Project), is_standard(q, inch))),
            None if Standard::parse(&t.standard).is_some_and(Standard::is_pipe) => None,
            None => major_nm
                .and_then(|m| ryu_standards::tap::rule_for(m, pitch, minor_nm, inch))
                .map(|d| tap(d, Source::Rule)),
        },
        TapDrill::Engagement(e) => major_nm.map(|m| {
            tap(
                ryu_standards::tap::engagement_for(m, pitch, inch, e),
                Source::Rule,
            )
        }),
        TapDrill::RollForm => entry
            .and_then(|e| e.roll_form_drill.as_ref())
            .map(|q| (Size::quantity(q, Source::Project), is_standard(q, inch))),
    };
    if drill.is_none() && h.diameter.is_none() {
        let missing = match t.drill {
            TapDrill::RollForm => "roll_form_drill",
            _ => "tap_drill",
        };
        return Err(needs_table(
            Kind::Thread,
            &t.standard,
            &t.size,
            vec![missing.into()],
        ));
    }
    let gauge = if t.taper_wall {
        let e = entry.and_then(|e| e.gauge_diameter.as_ref().zip(e.gauge_length.as_ref()));
        match e {
            Some((d, l)) => Some((
                Size::quantity(d, Source::Project),
                Size::quantity(l, Source::Project),
            )),
            None => {
                return Err(needs_table(
                    Kind::Thread,
                    &t.standard,
                    &t.size,
                    vec!["gauge_diameter".into(), "gauge_length".into()],
                ));
            }
        }
    } else {
        None
    };
    let class_shown = class.as_deref();
    let sizes = ThreadSizes {
        standard: t.standard.clone(),
        size: t.size.clone(),
        designation: designation(
            &t.standard,
            &t.size,
            built.is_some().then_some(pitch),
            class_shown,
            t.left,
        ),
        major,
        pitch: pitch_size(pitch, pitch_src),
        tpi: (!matches!(pitch, Pitch::Mm(_))).then(|| pitch.text()),
        lead,
        class,
        minor,
        major_min,
        left: t.left,
        starts,
        depth,
        taper,
        gauge,
    };
    Ok((sizes, drill))
}

/// What a fastener gives a hole, besides what it writes.
#[derive(Default)]
struct FastenerSizes {
    clearance: Option<Size>,
    counterbore: Option<(Size, Size)>,
    countersink: Option<(Size, Size)>,
    /// Diameter and depth: the fastener's counterbore.
    counterdrill: Option<(Size, Size)>,
    spotface: Option<Size>,
}

/// A fastener's clearance and mouth: what the hole writes, else the
/// project's entry; every value neither gives is named in one refusal.
fn fastener_sizes(h: &Hole, f: &Fastener, tables: Tables) -> Result<FastenerSizes> {
    let project = table(tables)?;
    let e = project.and_then(|p| p.fastener(&f.standard, &f.size));
    let mut missing: Vec<String> = Vec::new();
    let mut need = |m: &str| {
        if !missing.iter().any(|x| x == m) {
            missing.push(m.to_owned());
        }
    };
    let proj = |q: &Quantity| Size::quantity(q, Source::Project);
    let over = |q: &Quantity| Size::quantity(q, Source::Override);
    let clearance = match &h.diameter {
        Some(_) => None,
        None => match e.and_then(|e| e.clearance.get(f.fit.key())) {
            Some(q) => Some(proj(q)),
            None => {
                need(&format!("clearance.{}", f.fit.key()));
                None
            }
        },
    };
    let mut pair = |written: (&Option<Quantity>, &Option<Quantity>),
                    from: Option<&(Quantity, Quantity)>,
                    key: &str| {
        let a = written
            .0
            .as_ref()
            .map(over)
            .or_else(|| from.map(|p| proj(&p.0)));
        let b = written
            .1
            .as_ref()
            .map(over)
            .or_else(|| from.map(|p| proj(&p.1)));
        if a.is_none() || b.is_none() {
            need(key);
        }
        a.zip(b)
    };
    let mut out = FastenerSizes {
        clearance,
        ..FastenerSizes::default()
    };
    match &h.form {
        HoleForm::Simple => {}
        HoleForm::Counterbore { diameter, depth } => {
            out.counterbore = pair(
                (diameter, depth),
                e.and_then(|e| e.counterbore.as_ref()),
                "counterbore",
            );
        }
        HoleForm::Countersink { diameter, angle } => {
            out.countersink = pair(
                (diameter, angle),
                e.and_then(|e| e.countersink.as_ref()),
                "countersink",
            );
        }
        HoleForm::Counterdrill {
            diameter, depth, ..
        } => {
            out.counterdrill = pair(
                (diameter, depth),
                e.and_then(|e| e.counterbore.as_ref()),
                "counterbore",
            );
        }
        HoleForm::Spotface { diameter, .. } => {
            out.spotface = match (diameter, e.and_then(|e| e.spotface.as_ref())) {
                (Some(q), _) => Some(over(q)),
                (None, Some(q)) => Some(proj(q)),
                (None, None) => {
                    need("spotface");
                    None
                }
            };
        }
    }
    if !missing.is_empty() {
        return Err(needs_table(Kind::Fastener, &f.standard, &f.size, missing));
    }
    Ok(out)
}

/// The bore's tolerance at the drill's size.
fn fit_sizes(bore: &BoreTolerance, drill: &Size) -> Result<FitSizes> {
    let nominal = drill.to_nm();
    let size = |n: i64, s| Size::nm(n, false, s);
    let (class, upper, lower, source) = match bore {
        BoreTolerance::Fit(c) => {
            let class = ryu_standards::fit::Class::parse(c).expect("doc::hole checked the class");
            let um = u64::try_from((nominal + 500) / 1000).unwrap_or(0);
            let Some((u, l)) = ryu_standards::fit::deviations(&class, um) else {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("ISO 286 doesn't define {c} at {}", drill.text),
                )
                .with("reason", "fit_undefined")
                .with("pointer", "/fit"));
            };
            // Tenths of a micrometre to nanometres.
            (Some(c.clone()), u * 100, l * 100, Source::Fit)
        }
        BoreTolerance::Symmetric(q) => (None, nm(q), -nm(q), Source::Override),
        BoreTolerance::Deviation(u, l) => (None, nm(u), nm(l), Source::Override),
    };
    Ok(FitSizes {
        class,
        upper: size(upper, source),
        lower: size(lower, source),
        limits: [size(nominal + lower, source), size(nominal + upper, source)],
    })
}

/// Whether a size is a standard drill of either system.
fn is_standard_size(s: &Size) -> bool {
    let nm = s.to_nm();
    ryu_standards::drill::standard(nm, false)
        .or(ryu_standards::drill::standard(nm, true))
        .is_some()
}

/// Whether a written size is a standard drill of its system.
fn is_standard(q: &Quantity, inch: bool) -> bool {
    ryu_standards::drill::standard(nm(q), inch).is_some()
}

/// A refusal at write time as a rebuild reports it: a project entry
/// that was there when the hole was written and is gone now is
/// `not_found`, naming the file.
pub fn at_rebuild(e: Error) -> Error {
    if e.details.get("reason").and_then(Value::as_str) == Some("needs_project_table") {
        let mut e = e;
        e.code = Code::NotFound;
        return e;
    }
    e
}

/// `hole.derive`: a spec and fields to a hole, its sizes and its
/// canonical spec. The hole needs no sketch or target: placeholders stand
/// in, and are left out of the answer. Fields are laid over the spec's,
/// and a field given as `""` (or `null`) takes the spec's out.
pub fn request(
    session: &crate::session::Session,
    spec: Option<&str>,
    fields: Option<&Value>,
    part: Option<&str>,
) -> Result<Value> {
    let project = session.project();
    let unit = project.manifest.default_unit;
    let (env, tables) = match part {
        Some(p) => {
            let p = project.part(&crate::doc::id::Id::parse(p, "part")?)?;
            (p.env.clone(), p.hole_standards.clone())
        }
        None => (project.env.clone(), project.hole_standards.clone()),
    };
    let mut v = Map::new();
    for (k, x) in [
        ("sketch", json!("s")),
        ("form", json!("simple")),
        ("depth", json!("through_all")),
        ("targets", json!(["b"])),
    ] {
        v.insert(k.into(), x);
    }
    if let Some(s) = spec {
        let (named, _) = crate::hole::spec::Spec::parse(s)?.apply(unit)?;
        v.extend(named);
    }
    match fields {
        None => {}
        Some(Value::Object(f)) => v.extend(f.iter().map(|(k, x)| (k.clone(), x.clone()))),
        Some(_) => {
            return Err(Error::new(
                Code::InvalidParam,
                "fields is an object of hole fields, such as {\"diameter\": \"6.9 mm\"}",
            )
            .with("pointer", "/fields"));
        }
    }
    v.retain(|_, x| !(x.is_null() || x.as_str() == Some("")));
    if v.get("bottom").and_then(Value::as_str) == Some("flat") {
        v.remove("bottom");
    }
    let v = Value::Object(v);
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let h = Hole::from_fields(&mut o, unit, &env)?;
    o.finish()?;
    let sizes = derive(&h, tables.as_deref())?;
    let mut shown = h.fields();
    if let Some(o) = shown.as_object_mut() {
        o.remove("sketch");
        o.remove("targets");
    }
    Ok(json!({
        "spec": crate::hole::spec::canonical(&h, unit),
        "fields": shown,
        "sizes": sizes.to_json(),
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::files::Feature;
    use crate::expr::Env;
    use crate::units::Unit;
    use serde_json::json;

    pub(crate) fn hole(fields: Value) -> Hole {
        let mut v = json!({"format": 23, "kind": "ryucad.feature", "type": "hole",
                           "sketch": "s", "form": "simple", "depth": "through_all",
                           "targets": ["b"]});
        for (k, x) in fields.as_object().unwrap() {
            v[k] = x.clone();
        }
        match Feature::from_json(&v, Unit::Mm, &Env::empty()).unwrap() {
            Feature::Hole(h) => *h,
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_plain_hole_is_built_from_what_it_writes() {
        let h = hole(json!({"form": "counterbore", "diameter": "6.6 mm",
                            "counterbore": {"diameter": "11 mm", "depth": "6.8 mm"}}));
        let s = derive(&h, None).unwrap();
        assert_eq!(s.drill.si, 0.0066);
        assert_eq!(s.drill.source, Source::Override);
        let (d, depth) = s.counterbore.as_ref().unwrap();
        assert_eq!((d.si, depth.si), (0.011, 0.0068));
        assert_eq!(s.mouth_depth(), Some(0.0068));
        assert_eq!(
            s.to_json()["drill"],
            json!({"value": "6.6 mm", "m": 0.0066, "source": "override"})
        );
    }

    fn tables() -> std::result::Result<HoleStandards, Error> {
        HoleStandards::from_json(&crate::doc::fixtures::house_standards())
    }

    #[test]
    fn m6_is_a_5_mm_drill_by_the_rule() {
        let h = hole(json!({"thread": {"standard": "iso_metric", "size": "M6"}}));
        let s = derive(&h, None).unwrap();
        assert_eq!(
            (s.drill.text.as_str(), s.drill.source),
            ("5 mm", Source::Rule)
        );
        let t = s.thread.as_ref().unwrap();
        assert_eq!(t.designation, "M6x1.0-6H");
        assert_eq!(t.pitch.text, "1 mm");
        assert_eq!(t.minor.as_ref().unwrap()[1].text, "5.153 mm");
        assert_eq!(
            t.major.as_ref().unwrap().source,
            Source::Standard(ryu_standards::Source::Fact)
        );
        assert_eq!(t.depth, None);
        assert_eq!(
            s.to_json()["thread"]["limits"]["minor"][0]["value"],
            "4.917 mm"
        );
    }

    #[test]
    fn a_projects_tap_drill_wins_and_overrides_win_over_it() {
        let t = tables();
        let h = hole(json!({"thread": {"standard": "iso_metric", "size": "M6"}}));
        let s = derive(&h, Some(&t)).unwrap();
        assert_eq!(
            (s.drill.text.as_str(), s.drill.source),
            ("5.1 mm", Source::Project)
        );
        let h = hole(
            json!({"thread": {"standard": "iso_metric", "size": "M6"}, "diameter": "5.05 mm"}),
        );
        assert_eq!(derive(&h, Some(&t)).unwrap().drill.source, Source::Override);
    }

    #[test]
    fn engagement_and_roll_form() {
        let t = tables();
        let h = hole(
            json!({"thread": {"standard": "iso_metric", "size": "M6", "drill": {"engagement": 75}}}),
        );
        assert_eq!(derive(&h, None).unwrap().drill.text, "5.2 mm");
        let h =
            hole(json!({"thread": {"standard": "iso_metric", "size": "M6", "drill": "roll_form"}}));
        assert_eq!(derive(&h, Some(&t)).unwrap().drill.text, "5.55 mm");
        let h =
            hole(json!({"thread": {"standard": "iso_metric", "size": "M8", "drill": "roll_form"}}));
        let e = derive(&h, Some(&t)).unwrap_err();
        assert_eq!(e.details["reason"], "needs_project_table");
        assert_eq!(e.details["missing"], json!(["roll_form_drill"]));
        assert_eq!(
            e.details["entry"],
            json!({"threads": {"iso_metric": {"M8": {"roll_form_drill": null}}}})
        );
        assert_eq!(e.details["file"], "hole_standards.json");
    }

    #[test]
    fn unified_pipe_and_house_threads() {
        let t = tables();
        let h = hole(json!({"thread": {"standard": "unc", "size": "1/4"}}));
        let s = derive(&h, None).unwrap();
        assert_eq!(s.drill.text, "0.201 in");
        assert_eq!(s.thread.as_ref().unwrap().designation, "1/4-20 UNC-2B");
        // A pipe thread's tap drill is the project's, or a refusal.
        let h = hole(json!({"thread": {"standard": "npt", "size": "1/8"}}));
        assert_eq!(derive(&h, Some(&t)).unwrap().drill.text, "8.7 mm");
        let h = hole(json!({"thread": {"standard": "npt", "size": "1/4"}}));
        assert_eq!(
            derive(&h, Some(&t)).unwrap_err().details["missing"],
            json!(["tap_drill"])
        );
        // A house thread from the project's own entry.
        let h = hole(json!({"thread": {"standard": "house_acme", "size": "T10"}}));
        let s = derive(&h, Some(&t)).unwrap();
        assert_eq!(s.drill.text, "8 mm");
        assert_eq!(s.thread.as_ref().unwrap().pitch.source, Source::Project);
        let h = hole(json!({"thread": {"standard": "house_acme", "size": "T12"}}));
        assert_eq!(
            derive(&h, Some(&t)).unwrap_err().details["reason"],
            "needs_project_table"
        );
        // A broken table fails the holes that read it, and only those.
        let broken: std::result::Result<HoleStandards, Error> =
            Err(Error::new(Code::InvalidJson, "bad").with("path", "hole_standards.json"));
        let h = hole(json!({"thread": {"standard": "iso_metric", "size": "M6"}}));
        assert_eq!(
            derive(&h, Some(&broken)).unwrap_err().code,
            Code::InvalidJson
        );
        let h = hole(json!({"diameter": "5 mm"}));
        assert!(derive(&h, Some(&broken)).is_ok());
    }

    #[test]
    fn a_left_hand_two_start_thread() {
        let h = hole(
            json!({"thread": {"standard": "iso_metric", "size": "M8", "hand": "left", "starts": 2}}),
        );
        let t = derive(&h, None).unwrap().thread.unwrap();
        assert!(t.left);
        assert_eq!((t.starts, t.lead.text.as_str()), (2, "2.5 mm"));
        assert_eq!(t.designation, "M8x1.25-6H-LH");
    }

    #[test]
    fn a_fastener_gives_the_clearance_and_its_mouth() {
        let t = tables();
        let h = hole(json!({"fastener": {"standard": "house_cap", "size": "M5"},
                            "form": "counterbore", "counterbore": {}}));
        let s = derive(&h, Some(&t)).unwrap();
        assert_eq!(
            (s.drill.text.as_str(), s.drill.source),
            ("5.6 mm", Source::Project)
        );
        let (d, depth) = s.counterbore.unwrap();
        assert_eq!((d.text.as_str(), depth.text.as_str()), ("9.7 mm", "5.3 mm"));
        // An override is reported as one.
        let h = hole(
            json!({"fastener": {"standard": "house_cap", "size": "M5", "fit": "close"},
                            "form": "counterbore", "counterbore": {"depth": "6 mm"}}),
        );
        let s = derive(&h, Some(&t)).unwrap();
        assert_eq!(s.drill.text, "5.4 mm");
        assert_eq!(s.counterbore.unwrap().1.source, Source::Override);
        // Everything missing, in one refusal.
        let h = hole(
            json!({"fastener": {"standard": "house_flat", "size": "M5", "fit": "loose"},
                            "form": "counterbore", "counterbore": {}}),
        );
        let e = derive(&h, Some(&t)).unwrap_err();
        assert_eq!(
            e.details["missing"],
            json!(["clearance.loose", "counterbore"])
        );
        assert_eq!(
            e.details["entry"],
            json!({"fasteners": {"house_flat": {"M5": {"clearance": {"loose": null}, "counterbore": null}}}})
        );
        // No table at all: the same refusal.
        let h = hole(json!({"fastener": {"standard": "iso4762", "size": "M5"}}));
        assert_eq!(
            derive(&h, None).unwrap_err().details["reason"],
            "needs_project_table"
        );
    }

    #[test]
    fn a_fit_at_its_nominal() {
        let h = hole(json!({"diameter": "6 mm", "fit": "H7"}));
        let f = derive(&h, None).unwrap().fit.unwrap();
        // ISO 286: H7 from 3 to 6 mm is +12/0 µm; 6 mm is in that step.
        assert_eq!(
            (f.upper.text.as_str(), f.lower.text.as_str()),
            ("0.012 mm", "0 mm")
        );
        assert_eq!(
            (f.limits[0].text.as_str(), f.limits[1].text.as_str()),
            ("6 mm", "6.012 mm")
        );
        assert_eq!(f.upper.source, Source::Fit);
        let h = hole(json!({"diameter": "6 mm", "tolerance": {"symmetric": "0.01 mm"}}));
        let f = derive(&h, None).unwrap().fit.unwrap();
        assert_eq!(f.limits[0].text, "5.99 mm");
        let h = hole(json!({"diameter": "4000 mm", "fit": "H7"}));
        assert_eq!(
            derive(&h, None).unwrap_err().details["reason"],
            "fit_undefined"
        );
    }

    #[test]
    fn new_mouths_near_and_far() {
        let d = |v: Value| derive(&hole(v), None).unwrap();
        let s = d(
            json!({"format": 24, "form": "counterdrill", "diameter": "5 mm",
                         "counterdrill": {"diameter": "9 mm", "depth": "4 mm"},
                         "depth": "through_all"}),
        );
        let (dia, depth, angle) = s.counterdrill.as_ref().unwrap();
        assert_eq!((dia.text.as_str(), depth.text.as_str()), ("9 mm", "4 mm"));
        assert_eq!(
            (angle.text.as_str(), angle.source),
            ("118 deg", Source::Default)
        );
        // 4 mm, then the cone from 4.5 to 2.5 mm radius at 59 deg.
        let cone = 2.0 / ryukei::math::tan(59f64.to_radians());
        assert!((s.mouth_depth().unwrap() - (0.004 + cone / 1000.0)).abs() < 1e-15);
        let s = d(json!({"format": 24, "form": "spotface", "diameter": "5 mm",
                         "spotface": {"diameter": "12 mm"}, "depth": "through_all",
                         "near": {"diameter": "6 mm"}}));
        assert_eq!(s.spotface.as_ref().unwrap().1.text, "0 mm");
        let (nd, na) = s.near.as_ref().unwrap();
        assert_eq!(
            (nd.text.as_str(), na.text.as_str(), na.source),
            ("6 mm", "90 deg", Source::Default)
        );
        // The thread starts below the near chamfer: 0.5 mm at 45 deg.
        assert!((s.thread_start() - 0.0005).abs() < 1e-15);
        let s = d(json!({"format": 24, "form": "simple", "diameter": "5 mm",
                         "depth": "through_all",
                         "far": {"counterbore": {"diameter": "8 mm", "depth": "1 mm"}}}));
        assert!(matches!(s.far, Some(FarSizes::Counterbore(..))));
        let j = s.to_json();
        assert_eq!(j["far"]["counterbore"]["diameter"]["value"], "8 mm");
        // A near chamfer no wider than what it chamfers is refused.
        let e = derive(
            &hole(
                json!({"format": 24, "form": "counterbore", "diameter": "5 mm",
                         "counterbore": {"diameter": "9 mm", "depth": "2 mm"},
                         "depth": "through_all", "near": {"diameter": "8 mm"}}),
            ),
            None,
        )
        .unwrap_err();
        assert_eq!(e.details["pointer"], "/near/diameter");
        // From a fastener: a counterdrill takes its counterbore entry, a
        // spotface its spotface; missing, named in needs_project_table.
        let t = tables();
        let s = derive(
            &hole(
                json!({"format": 24, "form": "counterdrill", "counterdrill": {},
                         "fastener": {"standard": "house_cap", "size": "M5"},
                         "depth": "through_all"}),
            ),
            Some(&t),
        )
        .unwrap();
        assert_eq!(s.counterdrill.as_ref().unwrap().0.text, "9.7 mm");
        let e = derive(
            &hole(json!({"format": 24, "form": "spotface", "spotface": {},
                         "fastener": {"standard": "house_cap", "size": "M5"},
                         "depth": "through_all"})),
            Some(&t),
        )
        .unwrap_err();
        assert_eq!(e.details["missing"], json!(["spotface"]));
    }
}
