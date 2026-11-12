//! Hole features: simple, counterbored or countersunk holes drilled at a
//! sketch's points into the solids they name, against the sketch's
//! normal. A hole always cuts.

use super::files::{Obj, invalid};
use super::geometry::GeomRef;
use super::id::Id;
use super::reference::Reference;
use super::solid::targets_from_json;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use ryu_standards::thread::{self as builtin, Pitch, Standard, Unknown};
use serde_json::{Value, json};

/// A hole at each of a sketch's points.
#[derive(Clone, Debug, PartialEq)]
pub struct Hole {
    pub sketch: Id,
    /// Points of the sketch; empty means every point no curve uses.
    pub points: Vec<Id>,
    pub form: HoleForm,
    /// The bore's diameter: derived when the hole has a thread or a
    /// fastener and leaves it out, an override when it writes it.
    pub diameter: Option<Quantity>,
    pub depth: Depth,
    pub bottom: Bottom,
    /// The solids it cuts, by their founders.
    pub targets: Vec<Id>,
    /// A tapped hole's thread.
    pub thread: Option<Thread>,
    /// A clearance hole's screw.
    pub fastener: Option<Fastener>,
    /// The bore's tolerance (`fit` or `tolerance` in the file).
    pub bore: Option<BoreTolerance>,
}

/// A tapped hole's thread: what it is, never its sizes (`hole::derive`
/// works those out on every rebuild).
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    /// A built-in standard's key (`iso_metric`, `unc`, `npt`, ...) or a
    /// project standard's.
    pub standard: String,
    /// As the standard names it: `M6`, `1/4`, `#10`, `1/8`, `G1/4`.
    pub size: String,
    /// A metric pitch, written only when not the size's own.
    pub pitch: Option<Quantity>,
    /// Threads per inch, written only when not the series' own.
    pub tpi: Option<Quantity>,
    /// Written only when not the standard's default (`6H`, `2B`).
    pub class: Option<String>,
    pub left: bool,
    /// More than one start; written only then.
    pub starts: Option<u32>,
    /// From the mouth; none is full depth on a through hole. A blind
    /// tapped hole must give it.
    pub depth: Option<ThreadDepth>,
    pub drill: TapDrill,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ThreadDepth {
    Full,
    Length(Quantity),
}

/// How the tap drill is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapDrill {
    /// A project's tap drill for the size, else ryucad's rule.
    Rule,
    /// A percent thread engagement, 50 to 100.
    Engagement(u32),
    /// The project's roll-form drill.
    RollForm,
}

/// A clearance hole for a screw: its sizes come from the project's
/// fastener table.
#[derive(Clone, Debug, PartialEq)]
pub struct Fastener {
    pub standard: String,
    pub size: String,
    pub fit: FastenerFit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FastenerFit {
    Close,
    Normal,
    Loose,
}

impl FastenerFit {
    pub fn key(self) -> &'static str {
        match self {
            FastenerFit::Close => "close",
            FastenerFit::Normal => "normal",
            FastenerFit::Loose => "loose",
        }
    }

    pub fn parse(s: &str) -> Option<FastenerFit> {
        [FastenerFit::Close, FastenerFit::Normal, FastenerFit::Loose]
            .into_iter()
            .find(|f| f.key() == s)
    }
}

/// The bore's tolerance: an ISO 286 hole class, or deviations as
/// drawings take them. Geometry is built at nominal.
#[derive(Clone, Debug, PartialEq)]
pub enum BoreTolerance {
    Fit(String),
    Symmetric(Quantity),
    /// Upper, then lower.
    Deviation(Quantity, Quantity),
}

/// A name in a hole's thread or fastener: a standard's key or a size.
fn name(o: &mut Obj, k: &str, at: &str) -> Result<String> {
    let v = o.take(k)?;
    match v.as_str() {
        Some(s) if !s.is_empty() && !s.contains(char::is_whitespace) => Ok(s.to_owned()),
        _ => Err(invalid(
            &format!("{at}/{k}"),
            &format!("{k} is a name, such as \"M6\""),
        )),
    }
}

impl Thread {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"standard": self.standard, "size": self.size});
        if let Some(q) = &self.pitch {
            v["pitch"] = json!(q.text());
        }
        if let Some(q) = &self.tpi {
            v["tpi"] = json!(q.text());
        }
        if let Some(c) = &self.class {
            v["class"] = json!(c);
        }
        if self.left {
            v["hand"] = json!("left");
        }
        if let Some(n) = self.starts {
            v["starts"] = json!(n);
        }
        match &self.depth {
            None => {}
            Some(ThreadDepth::Full) => v["depth"] = json!("full"),
            Some(ThreadDepth::Length(q)) => v["depth"] = json!(q.text()),
        }
        match self.drill {
            TapDrill::Rule => {}
            TapDrill::Engagement(e) => v["drill"] = json!({"engagement": e}),
            TapDrill::RollForm => v["drill"] = json!("roll_form"),
        }
        v
    }

    fn from_json(v: &Value, default: Unit, env: &Env) -> Result<Thread> {
        let at = "/thread";
        let mut o = Obj::new(v, at)?;
        let standard = name(&mut o, "standard", at)?;
        let size_name = name(&mut o, "size", at)?;
        let pitch = o
            .take_opt("pitch")
            .map(|x| {
                size(
                    &x,
                    Field::Length(default),
                    env,
                    "/thread/pitch",
                    "a thread's pitch",
                )
            })
            .transpose()?;
        let tpi = o
            .take_opt("tpi")
            .map(|x| size(&x, Field::Number, env, "/thread/tpi", "a thread's TPI"))
            .transpose()?;
        if pitch.is_some() && tpi.is_some() {
            return Err(invalid(at, "a thread has a pitch or a tpi, not both"));
        }
        let class = match o.take_opt("class") {
            None => None,
            Some(Value::String(c)) if !c.is_empty() => Some(c),
            Some(_) => return Err(invalid("/thread/class", "a class is text, such as \"6H\"")),
        };
        let left = match o.take_opt("hand") {
            None => false,
            Some(h) if h == "left" => true,
            Some(_) => {
                return Err(invalid(
                    "/thread/hand",
                    "hand is \"left\", written only for a left-hand thread",
                ));
            }
        };
        let starts = match o.take_opt("starts") {
            None => None,
            Some(n) => match n.as_u64() {
                Some(n @ 2..=16) => Some(n as u32),
                _ => {
                    return Err(invalid(
                        "/thread/starts",
                        "starts is a whole number from 2 to 16, written only past 1",
                    ));
                }
            },
        };
        let depth = match o.take_opt("depth") {
            None => None,
            Some(d) if d == "full" => Some(ThreadDepth::Full),
            Some(d) => Some(ThreadDepth::Length(size(
                &d,
                Field::Length(default),
                env,
                "/thread/depth",
                "a thread's depth",
            )?)),
        };
        let drill = match o.take_opt("drill") {
            None => TapDrill::Rule,
            Some(d) if d == "roll_form" => TapDrill::RollForm,
            Some(d) => match d.get("engagement").and_then(Value::as_u64) {
                Some(e @ 50..=100) if d.as_object().is_some_and(|m| m.len() == 1) => {
                    TapDrill::Engagement(e as u32)
                }
                _ => {
                    return Err(invalid(
                        "/thread/drill",
                        "a tap drill is {\"engagement\": 50 to 100} or \"roll_form\" (ryucad's rule is written as nothing)",
                    ));
                }
            },
        };
        o.finish()?;
        Ok(Thread {
            standard,
            size: size_name,
            pitch,
            tpi,
            class,
            left,
            starts,
            depth,
            drill,
        })
    }
}

impl Fastener {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"standard": self.standard, "size": self.size});
        if self.fit != FastenerFit::Normal {
            v["fit"] = json!(self.fit.key());
        }
        v
    }

    fn from_json(v: &Value) -> Result<Fastener> {
        let at = "/fastener";
        let mut o = Obj::new(v, at)?;
        let standard = name(&mut o, "standard", at)?;
        let size = name(&mut o, "size", at)?;
        let fit = match o.take_opt("fit") {
            None => FastenerFit::Normal,
            Some(f) => f.as_str().and_then(FastenerFit::parse).ok_or_else(|| {
                invalid(
                    "/fastener/fit",
                    "a fastener's fit is close, normal or loose",
                )
            })?,
        };
        o.finish()?;
        Ok(Fastener {
            standard,
            size,
            fit,
        })
    }
}

impl BoreTolerance {
    fn from_json(
        fit: Option<Value>,
        tol: Option<Value>,
        default: Unit,
        env: &Env,
    ) -> Result<Option<BoreTolerance>> {
        match (fit, tol) {
            (None, None) => Ok(None),
            (Some(_), Some(_)) => Err(invalid(
                "/tolerance",
                "a hole has a fit or a tolerance, not both",
            )),
            (Some(Value::String(f)), None) => Ok(Some(BoreTolerance::Fit(f))),
            (Some(_), None) => Err(invalid(
                "/fit",
                "a fit is an ISO 286 hole class, such as \"H7\"",
            )),
            (None, Some(t)) => {
                let len = |x: &Value, at: &str| {
                    Quantity::from_json_in(x, default, &|n| env.get(n))
                        .map_err(|e| e.with("pointer", at))
                };
                let o = t.as_object().filter(|o| o.len() == 1);
                match o.and_then(|o| o.iter().next()) {
                    Some((k, x)) if k == "symmetric" => {
                        let q = len(x, "/tolerance/symmetric")?;
                        if q.meters() <= 0.0 {
                            return Err(invalid(
                                "/tolerance/symmetric",
                                "a symmetric tolerance is more than 0",
                            ));
                        }
                        Ok(Some(BoreTolerance::Symmetric(q)))
                    }
                    Some((k, Value::Array(a))) if k == "deviation" && a.len() == 2 => {
                        let (u, l) = (
                            len(&a[0], "/tolerance/deviation/0")?,
                            len(&a[1], "/tolerance/deviation/1")?,
                        );
                        if u.meters() < l.meters() {
                            return Err(invalid(
                                "/tolerance/deviation",
                                "the upper deviation is the first, and no less than the lower",
                            ));
                        }
                        Ok(Some(BoreTolerance::Deviation(u, l)))
                    }
                    _ => Err(invalid(
                        "/tolerance",
                        "a tolerance is {symmetric: q} or {deviation: [upper, lower]}",
                    )),
                }
            }
        }
    }

    fn write(&self, v: &mut Value) {
        match self {
            BoreTolerance::Fit(f) => v["fit"] = json!(f),
            BoreTolerance::Symmetric(q) => v["tolerance"] = json!({"symmetric": q.text()}),
            BoreTolerance::Deviation(u, l) => {
                v["tolerance"] = json!({"deviation": [u.text(), l.text()]});
            }
        }
    }
}

/// What the hole's mouth is.
#[derive(Clone, Debug, PartialEq)]
pub enum HoleForm {
    Simple,
    /// A wider, flat-bottomed bore of `diameter` to `depth` first. With a
    /// fastener, either may be left out (derived).
    Counterbore {
        diameter: Option<Quantity>,
        depth: Option<Quantity>,
    },
    /// A cone of `diameter` at the mouth and included `angle`. With a
    /// fastener, either may be left out (derived).
    Countersink {
        diameter: Option<Quantity>,
        angle: Option<Quantity>,
    },
}

/// How deep the bore goes, from the sketch's plane.
#[derive(Clone, Debug, PartialEq)]
pub enum Depth {
    /// To the bore's full diameter (a drill point's shoulder).
    Blind(Quantity),
    /// Past everything of its targets.
    ThroughAll,
    /// To a planar face parallel to the sketch, or a vertex, then
    /// `offset` further (a length, of either sign).
    UpTo {
        target: GeomRef,
        offset: Option<Quantity>,
    },
}

/// The bottom of a blind hole.
#[derive(Clone, Debug, PartialEq)]
pub enum Bottom {
    Flat,
    /// A drill point of this included angle.
    Drill(Quantity),
}

impl HoleForm {
    pub const NAMES: [&str; 3] = ["simple", "counterbore", "countersink"];

    pub fn type_name(&self) -> &'static str {
        match self {
            HoleForm::Simple => "simple",
            HoleForm::Counterbore { .. } => "counterbore",
            HoleForm::Countersink { .. } => "countersink",
        }
    }
}

impl Depth {
    pub fn type_name(&self) -> &'static str {
        match self {
            Depth::Blind(_) => "blind",
            Depth::ThroughAll => "through_all",
            Depth::UpTo { .. } => "up_to",
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Depth::Blind(q) => json!({"blind": q.text()}),
            Depth::ThroughAll => json!("through_all"),
            Depth::UpTo { target, offset } => {
                let mut v = json!({"target": target.to_json()});
                if let Some(q) = offset {
                    v["offset"] = json!(q.text());
                }
                json!({ "up_to": v })
            }
        }
    }

    fn from_json(v: &Value, default: Unit, env: &Env) -> Result<Depth> {
        if v == "through_all" {
            return Ok(Depth::ThroughAll);
        }
        let bad = || {
            invalid_extent(
                "/depth",
                "a hole's depth is {\"blind\": length}, \"through_all\" or {\"up_to\": {target, offset}}"
                    .to_owned(),
            )
        };
        let Some(o) = v.as_object().filter(|o| o.len() == 1) else {
            return Err(bad());
        };
        let (k, x) = o.iter().next().expect("one field");
        match k.as_str() {
            "blind" => Ok(Depth::Blind(size(
                x,
                Field::Length(default),
                env,
                "/depth/blind",
                "a hole's depth",
            )?)),
            "up_to" => {
                let mut o = Obj::new(x, "/depth/up_to")?;
                let target = GeomRef::from_json(&o.take("target")?, "/depth/up_to/target")?;
                let offset = o
                    .take_opt("offset")
                    .map(|v| {
                        Quantity::from_json_in(&v, default, &|n| env.get(n))
                            .map_err(|e| e.with("pointer", "/depth/up_to/offset"))
                    })
                    .transpose()?;
                o.finish()?;
                Ok(Depth::UpTo { target, offset })
            }
            _ => Err(bad()),
        }
    }
}

impl Bottom {
    pub fn to_json(&self) -> Value {
        match self {
            Bottom::Flat => json!("flat"),
            Bottom::Drill(q) => json!({"drill": q.text()}),
        }
    }

    fn from_json(v: &Value, env: &Env) -> Result<Bottom> {
        if v == "flat" {
            return Ok(Bottom::Flat);
        }
        match v
            .as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.get("drill"))
        {
            Some(a) => Ok(Bottom::Drill(angle(
                a,
                env,
                "/bottom/drill",
                "a drill point",
            )?)),
            None => Err(invalid(
                "/bottom",
                "a hole's bottom is \"flat\" or {\"drill\": angle}",
            )),
        }
    }
}

fn refuse(at: &str, reason: &str, msg: String) -> Error {
    Error::new(Code::InvalidParam, msg)
        .with("pointer", at)
        .with("reason", reason)
}

/// A thread's pitch as the built-in lookup takes it: millimetres for ISO
/// metric, a whole TPI for unified, tenths for pipe threads.
fn pitch_of(standard: Standard, t: &Thread) -> Result<Option<Pitch>> {
    match (standard.inch(), &t.pitch, &t.tpi) {
        (_, None, None) => Ok(None),
        (false, Some(p), None) => Ok(Some(Pitch::Mm((p.meters() * 1e9).round() as i64))),
        (true, None, Some(n)) if standard.is_pipe() => {
            Ok(Some(Pitch::Tpi10((n.number() * 10.0).round() as u32)))
        }
        (true, None, Some(n)) if n.number().fract() == 0.0 => {
            Ok(Some(Pitch::Tpi(n.number() as u16)))
        }
        (inch, ..) => {
            let (at, what) = if inch {
                ("/thread/pitch", "a TPI (tpi)")
            } else {
                ("/thread/tpi", "a pitch (pitch)")
            };
            Err(refuse(
                at,
                "pitch_not_standard",
                format!("{} threads take {what}", standard.key()),
            ))
        }
    }
}

/// The built-in thread a hole's thread names, or `None` for a project
/// standard; a size, pitch or class the built-in doesn't have is refused
/// with what it does have.
pub fn resolve(t: &Thread) -> Result<Option<builtin::Thread>> {
    let Some(standard) = Standard::parse(&t.standard) else {
        return Ok(None);
    };
    let pitch = pitch_of(standard, t)?;
    builtin::lookup(standard, &t.size, pitch, t.class.as_deref())
        .map(Some)
        .map_err(|u| match u {
            Unknown::Size { known } => refuse(
                "/thread/size",
                "unknown_thread_size",
                format!(
                    "{} has no size {}; the nearest are {}",
                    t.standard,
                    t.size,
                    known.join(", ")
                ),
            )
            .with("known", known),
            Unknown::Pitch { listed } => refuse(
                if t.tpi.is_some() {
                    "/thread/tpi"
                } else {
                    "/thread/pitch"
                },
                "pitch_not_standard",
                format!("{} {} lists {}", t.standard, t.size, listed.join(", ")),
            )
            .with("listed", listed),
            Unknown::Class { known } => refuse(
                "/thread/class",
                "unknown_thread_class",
                format!(
                    "{} has no class {:?}; it has {}",
                    t.standard,
                    t.class.as_deref().unwrap_or(""),
                    known.join(", ")
                ),
            )
            .with("known", known),
            Unknown::ClassNotTaken => refuse(
                "/thread/class",
                "class_not_taken",
                format!("{} threads take no class: take it out", t.standard),
            ),
        })
}

fn invalid_extent(at: &str, msg: String) -> Error {
    Error::new(Code::InvalidExtent, msg).with("pointer", at)
}

/// A length or an angle greater than zero, at `at`.
fn size(v: &Value, field: Field, env: &Env, at: &str, what: &str) -> Result<Quantity> {
    let q = Quantity::from_json_as(v, field, &|n| env.get(n)).map_err(|e| e.with("pointer", at))?;
    let si = match field {
        Field::Length(_) => q.meters(),
        Field::Angle => q.radians(),
        Field::Number => q.number(),
    };
    if si <= 0.0 {
        return Err(bad_size(
            at,
            &format!("{what} must be greater than zero"),
            &q,
        ));
    }
    Ok(q)
}

/// An included angle, in `(0, 180 deg)`.
fn angle(v: &Value, env: &Env, at: &str, what: &str) -> Result<Quantity> {
    let q = size(v, Field::Angle, env, at, &format!("{what}'s angle"))?;
    if q.radians() >= core::f64::consts::PI {
        return Err(bad_size(
            at,
            &format!("{what}'s angle is under 180 deg"),
            &q,
        ));
    }
    Ok(q)
}

fn bad_size(at: &str, msg: &str, q: &Quantity) -> Error {
    Error::new(Code::InvalidParam, format!("{msg}, not {q}"))
        .with("pointer", at)
        .with("value", q.text())
}

impl Hole {
    pub fn fields(&self) -> Value {
        let mut v = json!({
            "sketch": self.sketch,
            "form": self.form.type_name(),
            "depth": self.depth.to_json(),
            "targets": self.targets,
        });
        if let Some(d) = &self.diameter {
            v["diameter"] = json!(d.text());
        }
        if !self.points.is_empty() {
            v["points"] = json!(self.points);
        }
        match &self.form {
            HoleForm::Simple => {}
            HoleForm::Counterbore { diameter, depth } => {
                v["counterbore"] = mouth(&[("diameter", diameter), ("depth", depth)]);
            }
            HoleForm::Countersink { diameter, angle } => {
                v["countersink"] = mouth(&[("diameter", diameter), ("angle", angle)]);
            }
        }
        if self.bottom != Bottom::Flat {
            v["bottom"] = self.bottom.to_json();
        }
        if let Some(t) = &self.thread {
            v["thread"] = t.to_json();
        }
        if let Some(f) = &self.fastener {
            v["fastener"] = f.to_json();
        }
        if let Some(b) = &self.bore {
            b.write(&mut v);
        }
        v
    }

    /// Whether it holds anything new with ryucad 28: written at
    /// `INTENT_FORMAT`, its sizes derived.
    pub fn has_intent(&self) -> bool {
        self.thread.is_some() || self.fastener.is_some() || self.bore.is_some()
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Hole> {
        let length = Field::Length(default);
        let sketch = o.take("sketch")?;
        let sketch = sketch
            .as_str()
            .ok_or_else(|| invalid("/sketch", "sketch is a sketch feature's ID"))?;
        let sketch = Id::parse(sketch, "feature").map_err(|e| e.with("pointer", "/sketch"))?;
        let points = points_from_json(o.take_opt("points"))?;
        let thread = o
            .take_opt("thread")
            .map(|t| Thread::from_json(&t, default, env))
            .transpose()?;
        let fastener = o
            .take_opt("fastener")
            .map(|f| Fastener::from_json(&f))
            .transpose()?;
        let bore =
            BoreTolerance::from_json(o.take_opt("fit"), o.take_opt("tolerance"), default, env)?;
        let derived = thread.is_some() || fastener.is_some();
        let diameter = match o.take_opt("diameter") {
            Some(d) => Some(size(&d, length, env, "/diameter", "a hole's diameter")?),
            None if derived => None,
            None => {
                return Err(invalid(
                    "/diameter",
                    "a hole with no thread or fastener gives its diameter",
                ));
            }
        };
        // A mouth's size: required unless a fastener derives it.
        let sized = fastener.is_some();
        let mouth_size = |c: &mut Obj, k: &str, at: &str, what: &str, angled: bool| {
            let p = format!("{at}/{k}");
            match c.take_opt(k) {
                Some(x) if angled => angle(&x, env, &p, what).map(Some),
                Some(x) => size(&x, length, env, &p, &format!("{what}'s {k}")).map(Some),
                None if sized => Ok(None),
                None => Err(invalid(
                    &p,
                    &format!("{what} with no fastener gives its {k}"),
                )),
            }
        };
        let form = o.take("form")?;
        let (cbore, csink) = (o.take_opt("counterbore"), o.take_opt("countersink"));
        let form = match (form.as_str(), cbore, csink) {
            (Some("simple"), None, None) => HoleForm::Simple,
            (Some("counterbore"), Some(c), None) => {
                let mut c = Obj::new(&c, "/counterbore")?;
                let d = mouth_size(&mut c, "diameter", "/counterbore", "a counterbore", false)?;
                let depth = mouth_size(&mut c, "depth", "/counterbore", "a counterbore", false)?;
                c.finish()?;
                if let (Some(d), Some(b)) = (&d, &diameter) {
                    wider(d, b, "/counterbore/diameter", "a counterbore")?;
                }
                HoleForm::Counterbore { diameter: d, depth }
            }
            (Some("countersink"), None, Some(c)) => {
                let mut c = Obj::new(&c, "/countersink")?;
                let d = mouth_size(&mut c, "diameter", "/countersink", "a countersink", false)?;
                let a = mouth_size(&mut c, "angle", "/countersink", "a countersink", true)?;
                c.finish()?;
                if let (Some(d), Some(b)) = (&d, &diameter) {
                    wider(d, b, "/countersink/diameter", "a countersink")?;
                }
                HoleForm::Countersink {
                    diameter: d,
                    angle: a,
                }
            }
            (Some(f @ ("simple" | "counterbore" | "countersink")), _, _) => {
                let (at, msg) = match f {
                    "counterbore" => (
                        "/counterbore",
                        "a counterbore hole has a counterbore {diameter, depth}, and no countersink",
                    ),
                    "countersink" => (
                        "/countersink",
                        "a countersink hole has a countersink {diameter, angle}, and no counterbore",
                    ),
                    _ => ("/form", "a simple hole has no counterbore or countersink"),
                };
                return Err(invalid(at, msg));
            }
            _ => {
                return Err(invalid(
                    "/form",
                    "a hole's form is simple, counterbore or countersink",
                ));
            }
        };
        let depth = Depth::from_json(&o.take("depth")?, default, env)?;
        let bottom = match o.take_opt("bottom") {
            None => Bottom::Flat,
            Some(b) => Bottom::from_json(&b, env)?,
        };
        if matches!(bottom, Bottom::Drill(_)) && !matches!(depth, Depth::Blind(_)) {
            return Err(invalid_extent(
                "/bottom",
                format!(
                    "a drill point ends a blind hole; a {} hole has no bottom of its own",
                    depth.type_name()
                ),
            ));
        }
        // Full depth on a through hole is the default: never written.
        let thread = thread.map(|mut t| {
            if !matches!(depth, Depth::Blind(_)) && t.depth == Some(ThreadDepth::Full) {
                t.depth = None;
            }
            t
        });
        let targets = targets_from_json(&o.take("targets")?, "/targets")?;
        let hole = Hole {
            sketch,
            points,
            form,
            diameter,
            depth,
            bottom,
            targets,
            thread,
            fastener,
            bore,
        };
        if let (Depth::Blind(d), Some((mouth, at))) = (&hole.depth, hole.mouth_depth())
            && mouth >= d.meters()
        {
            return Err(invalid_extent(
                at,
                format!(
                    "the hole's {} reaches {mouth} m, past its depth {d}",
                    hole.form.type_name()
                ),
            ));
        }
        if hole.thread.is_some() && hole.fastener.is_some() {
            return Err(refuse(
                "/fastener",
                "thread_and_fastener",
                "a hole is tapped (thread) or a clearance hole (fastener), not both".into(),
            ));
        }
        match &hole.bore {
            Some(_) if hole.thread.is_some() => {
                return Err(refuse(
                    "/fit",
                    "fit_on_thread",
                    "a tapped hole's class is its tolerance: give the thread a class instead"
                        .into(),
                ));
            }
            Some(BoreTolerance::Fit(f)) => match ryu_standards::fit::Class::parse(f) {
                Some(c) if c.member == ryu_standards::fit::Member::Hole => {}
                _ => {
                    return Err(refuse(
                        "/fit",
                        "fit_undefined",
                        format!("{f:?} is no ISO 286 hole class, such as \"H7\""),
                    ));
                }
            },
            _ => {}
        }
        if let Some(t) = &hole.thread {
            let built = resolve(t)?;
            if let Depth::Blind(drill) = &hole.depth {
                match &t.depth {
                    None => {
                        let mut e = refuse(
                            "/thread/depth",
                            "thread_depth_needed",
                            "a blind tapped hole gives its thread's depth: \"full\" or a length"
                                .into(),
                        );
                        if let Some(b) = &built
                            && drill.is_literal()
                        {
                            let (pn, pd) = b.pitch.value.nm();
                            let nm = (drill.meters() * 1e9).round() as i128 - 3 * pn / pd;
                            if nm > 0 {
                                e = e.with(
                                    "suggested",
                                    format!("{} mm", ryu_standards::mm_decimal(nm as i64)),
                                );
                            }
                        }
                        return Err(e);
                    }
                    Some(ThreadDepth::Length(q)) if q.meters() > drill.meters() => {
                        return Err(refuse(
                            "/thread/depth",
                            "thread_past_drill",
                            format!("the thread runs {q}, past the drill's {drill}"),
                        ));
                    }
                    _ => {}
                }
            }
        }
        Ok(hole)
    }

    /// How deep a counterbore or countersink goes, in meters, and the
    /// pointer of what sets it; `None` for a simple hole, or one whose
    /// sizes are derived (rebuild checks those from its `Sizes`).
    pub fn mouth_depth(&self) -> Option<(f64, &'static str)> {
        match &self.form {
            HoleForm::Simple => None,
            HoleForm::Counterbore { depth, .. } => {
                Some((depth.as_ref()?.meters(), "/counterbore/depth"))
            }
            HoleForm::Countersink { diameter, angle } => {
                let run = (diameter.as_ref()?.meters() - self.diameter.as_ref()?.meters()) / 2.0;
                let half = angle.as_ref()?.radians() / 2.0;
                Some((run / ryukei::math::tan(half), "/countersink"))
            }
        }
    }

    /// Its references, each with its role: an up-to target's `depth`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        self.geometry()
            .into_iter()
            .filter_map(|(k, g)| Some((k, g.reference()?)))
            .collect()
    }

    /// Its geometric references, each with its role: an up-to target's
    /// `depth`.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        match &self.depth {
            Depth::UpTo { target, .. } => vec![("depth".to_owned(), target)],
            _ => Vec::new(),
        }
    }
}

/// A counterbore's or countersink's written sizes.
fn mouth(sizes: &[(&str, &Option<Quantity>)]) -> Value {
    Value::Object(
        sizes
            .iter()
            .filter_map(|(k, q)| Some(((*k).to_owned(), json!(q.as_ref()?.text()))))
            .collect(),
    )
}

/// `inner` is narrower than `outer`, the bore's diameter.
fn wider(outer: &Quantity, inner: &Quantity, at: &str, what: &str) -> Result<()> {
    if outer.meters() <= inner.meters() {
        return Err(bad_size(
            at,
            &format!("{what} is wider than the hole's diameter {inner}"),
            outer,
        ));
    }
    Ok(())
}

/// Reads the optional `points` field: point IDs, each once.
fn points_from_json(v: Option<Value>) -> Result<Vec<Id>> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let a = v
        .as_array()
        .ok_or_else(|| invalid("/points", "points is an array of the sketch's point IDs"))?;
    let mut out: Vec<Id> = Vec::with_capacity(a.len());
    for (i, x) in a.iter().enumerate() {
        let at = format!("/points/{i}");
        let s = x
            .as_str()
            .ok_or_else(|| invalid(&at, "expected a point's ID"))?;
        let id = Id::parse(s, "entity").map_err(|e| e.with("pointer", &at))?;
        if out.contains(&id) {
            return Err(invalid(&at, &format!("{id} is named twice")));
        }
        out.push(id);
    }
    Ok(out)
}

/// The expression strings of a hole: its diameter, its counterbore's or
/// countersink's sizes, a blind depth, an up-to offset and a drill
/// point's angle.
pub fn expression_strings<'a>(k: &str, field: &'a mut Value) -> Vec<&'a mut String> {
    let mut out = Vec::new();
    match (k, field) {
        ("counterbore" | "countersink", Value::Object(o)) => {
            for x in o.values_mut() {
                if let Value::String(s) = x {
                    out.push(s);
                }
            }
        }
        ("depth", Value::Object(o)) => {
            for (k, x) in o.iter_mut() {
                match (k.as_str(), x) {
                    ("blind", Value::String(s)) => out.push(s),
                    ("up_to", Value::Object(u)) => {
                        if let Some(Value::String(s)) = u.get_mut("offset") {
                            out.push(s);
                        }
                    }
                    _ => {}
                }
            }
        }
        ("bottom", Value::Object(o)) => {
            if let Some(Value::String(s)) = o.get_mut("drill") {
                out.push(s);
            }
        }
        ("thread", Value::Object(o)) => {
            for (k, x) in o.iter_mut() {
                if let ("pitch" | "tpi" | "depth", Value::String(s)) = (k.as_str(), x)
                    && s != "full"
                {
                    out.push(s);
                }
            }
        }
        ("tolerance", Value::Object(o)) => {
            for x in o.values_mut() {
                match x {
                    Value::String(s) => out.push(s),
                    Value::Array(a) => a.iter_mut().for_each(|y| {
                        if let Value::String(s) = y {
                            out.push(s);
                        }
                    }),
                    _ => {}
                }
            }
        }

        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::canonical;
    use crate::doc::files::Feature;

    fn read(v: &Value) -> Result<Feature> {
        let p = [("d".to_owned(), crate::expr::Expr::parse("6 mm").unwrap())].into();
        Feature::from_json(v, Unit::Mm, &Env::new(&p))
    }

    fn hole() -> Value {
        json!({"format": 3, "kind": "ryucad.feature", "type": "hole", "sketch": "s3",
               "points": ["p1", "p2"], "form": "counterbore", "diameter": "d + 0.6 mm",
               "counterbore": {"diameter": "11 mm", "depth": "6.8 mm"},
               "depth": {"blind": "20 mm"}, "bottom": {"drill": "118 deg"},
               "targets": ["box1"]})
    }

    #[test]
    fn holes_round_trip() {
        let f = read(&hole()).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        assert_eq!(h.diameter.as_ref().unwrap().meters(), 0.0066);
        assert_eq!(h.form.type_name(), "counterbore");
        assert_eq!(h.mouth_depth().unwrap().0, 0.0068);
        assert_eq!(h.bottom.to_json(), json!({"drill": "118 deg"}));
        assert_eq!(f.format(), 3);
        assert_eq!(f.targets(), [&Id::parse("box1", "feature").unwrap()]);
        assert_eq!(f.profiles().first().map(|s| s.as_str()), Some("s3"));
        assert!(f.references().is_empty());
        let text = canonical::to_string(&f.to_json());
        assert_eq!(read(&canonical::parse(&text).unwrap()).unwrap(), f);
        assert_eq!(f.to_json()["diameter"], "d + 0.6 mm");
        // Defaults go unwritten: every point, a flat bottom.
        let mut v = hole();
        let o = v.as_object_mut().unwrap();
        o.remove("points");
        o.remove("bottom");
        o.remove("counterbore");
        o.insert("form".into(), json!("countersink"));
        o.insert(
            "countersink".into(),
            json!({"diameter": "12 mm", "angle": 90}),
        );
        let f = read(&v).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        assert!(h.points.is_empty() && h.bottom == Bottom::Flat);
        // A 90 deg countersink from 12 mm to 6.6 mm is 2.7 mm deep.
        assert!((h.mouth_depth().unwrap().0 - 0.0027).abs() < 1e-15);
        let w = f.to_json();
        assert!(w.get("points").is_none() && w.get("bottom").is_none());
        assert_eq!(w["countersink"]["angle"], "90 deg");
        assert_eq!(read(&w).unwrap(), f);
    }

    #[test]
    fn depths() {
        const N: &str = "0123456789abcdef0123456789abcdef";
        let mut v = hole();
        v.as_object_mut().unwrap().remove("bottom");
        v["depth"] = json!("through_all");
        let f = read(&v).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        assert_eq!(h.depth, Depth::ThroughAll);
        assert_eq!(read(&f.to_json()).unwrap(), f);
        v["depth"] = json!({"up_to": {"target": {"feature": "box1", "name": N}, "offset": "-d"}});
        let f = read(&v).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        let Depth::UpTo { target, offset } = &h.depth else {
            panic!()
        };
        assert_eq!(target.feature().unwrap().as_str(), "box1");
        assert_eq!(offset.as_ref().unwrap().meters(), -0.006);
        assert_eq!(
            f.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["depth"]
        );
        assert_eq!(read(&f.to_json()).unwrap(), f);
    }

    #[test]
    fn holes_are_checked() {
        let cases: [(&str, Value, Code, &str); 14] = [
            ("/diameter", json!("0 mm"), Code::InvalidParam, "/diameter"),
            ("/diameter", json!("3 deg"), Code::UnitMismatch, "/diameter"),
            ("/form", json!("tapped"), Code::InvalidDocument, "/form"),
            (
                "/counterbore/diameter",
                json!("5 mm"),
                Code::InvalidParam,
                "/counterbore/diameter",
            ),
            (
                "/counterbore/depth",
                json!("20 mm"),
                Code::InvalidExtent,
                "/counterbore/depth",
            ),
            (
                "/counterbore/glue",
                json!(1),
                Code::InvalidDocument,
                "/counterbore/glue",
            ),
            ("/depth", json!("deep"), Code::InvalidExtent, "/depth"),
            (
                "/depth",
                json!({"blind": "-1 mm"}),
                Code::InvalidParam,
                "/depth/blind",
            ),
            (
                "/depth",
                json!({"up_to": {}}),
                Code::InvalidDocument,
                "/depth/up_to/target",
            ),
            (
                "/bottom",
                json!({"drill": "180 deg"}),
                Code::InvalidParam,
                "/bottom/drill",
            ),
            ("/bottom", json!("round"), Code::InvalidDocument, "/bottom"),
            (
                "/points",
                json!(["p1", "p1"]),
                Code::InvalidDocument,
                "/points/1",
            ),
            ("/targets", json!([]), Code::InvalidDocument, "/targets"),
            ("/sketch", json!("S"), Code::InvalidId, "/sketch"),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = hole();
            let (parent, key) = path.rsplit_once('/').unwrap();
            v.pointer_mut(if parent.is_empty() { "" } else { parent })
                .unwrap()[key] = value;
            let e = read(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{path}: {e}"
            );
        }
        // A drill point only ends a blind hole.
        let mut v = hole();
        v["depth"] = json!("through_all");
        let e = read(&v).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidExtent, Some("/bottom"))
        );
        // Each form has its own fields, and only those.
        let mut v = hole();
        v["form"] = json!("simple");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/form");
        v["form"] = json!("countersink");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/countersink");
        v.as_object_mut().unwrap().remove("targets");
        v["form"] = json!("counterbore");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/targets");
    }

    #[test]
    fn sizes_read_parameters() {
        let v = hole();
        let f = read(&v).unwrap();
        assert_eq!(
            crate::doc::files::feature_names(&f.to_json())
                .into_iter()
                .collect::<Vec<_>>(),
            ["d"]
        );
        let mut w = f.to_json();
        assert!(crate::doc::files::rename_in_feature(&mut w, "d", "bore"));
        assert_eq!(w["diameter"], "bore + 0.6 mm");
        let mut v = hole();
        v["depth"] = json!({"blind": "2 * d"});
        v["counterbore"]["depth"] = json!("d");
        v["bottom"]["drill"] = json!("d / 1 mm * 20 deg");
        let f = read(&v).unwrap();
        let names = crate::doc::files::feature_names(&f.to_json());
        assert_eq!(names.into_iter().collect::<Vec<_>>(), ["d"]);
        let mut w = f.to_json();
        assert!(crate::doc::files::rename_in_feature(&mut w, "d", "bore"));
        assert_eq!(
            (
                &w["depth"]["blind"],
                &w["counterbore"]["depth"],
                &w["bottom"]["drill"]
            ),
            (
                &json!("2 * bore"),
                &json!("bore"),
                &json!("bore / 1 mm * 20 deg")
            )
        );
    }

    fn tapped() -> Value {
        json!({"format": 3, "kind": "ryucad.feature", "type": "hole", "sketch": "s3",
               "form": "simple",
               "thread": {"standard": "iso_metric", "size": "M8", "pitch": "1 mm",
                          "class": "5H", "hand": "left", "starts": 2,
                          "depth": "12 mm", "drill": {"engagement": 70}},
               "depth": {"blind": "20 mm"}, "bottom": {"drill": "118 deg"},
               "targets": ["box1"]})
    }

    #[test]
    fn intent_fields_round_trip() {
        let f = read(&tapped()).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        let t = h.thread.as_ref().unwrap();
        assert_eq!((t.standard.as_str(), t.size.as_str()), ("iso_metric", "M8"));
        assert_eq!(t.pitch.as_ref().unwrap().meters(), 0.001);
        assert!(t.left && t.starts == Some(2));
        assert_eq!(t.drill, TapDrill::Engagement(70));
        assert_eq!(
            t.depth,
            Some(ThreadDepth::Length(
                Quantity::parse("12 mm", Unit::Mm).unwrap()
            ))
        );
        assert!(h.diameter.is_none() && h.has_intent());
        assert_eq!(read(&f.to_json()).unwrap(), f);
        // A fastener and a fit.
        let mut v = tapped();
        let o = v.as_object_mut().unwrap();
        o.remove("thread");
        o.insert(
            "fastener".into(),
            json!({"standard": "house_cap", "size": "M5", "fit": "close"}),
        );
        o.insert("fit".into(), json!("H7"));
        let f = read(&v).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        assert_eq!(h.fastener.as_ref().unwrap().fit, FastenerFit::Close);
        assert_eq!(h.bore, Some(BoreTolerance::Fit("H7".into())));
        assert_eq!(read(&f.to_json()).unwrap(), f);
        // Defaults go unwritten: a normal fit, the rule's drill, full depth
        // on a through hole, a right hand, one start.
        let v = json!({"format": 3, "kind": "ryucad.feature", "type": "hole", "sketch": "s3",
                       "form": "simple", "thread": {"standard": "iso_metric", "size": "M6",
                       "depth": "full"}, "depth": "through_all", "targets": ["box1"]});
        let w = read(&v).unwrap().to_json();
        assert_eq!(w["thread"], json!({"standard": "iso_metric", "size": "M6"}));
        // A tolerance as drawings take it.
        let mut v = tapped();
        v.as_object_mut().unwrap().remove("thread");
        v["diameter"] = json!("6 mm");
        v["tolerance"] = json!({"deviation": ["0.02 mm", "0 mm"]});
        let f = read(&v).unwrap();
        assert_eq!(read(&f.to_json()).unwrap(), f);
    }

    #[test]
    fn intent_sizes_read_parameters() {
        let mut v = tapped();
        v["thread"]["depth"] = json!("d * 2");
        v["thread"]["pitch"] = json!("d / 6");
        let f = read(&v).unwrap();
        let names = crate::doc::files::feature_names(&f.to_json());
        assert_eq!(names.into_iter().collect::<Vec<_>>(), ["d"]);
        let mut w = f.to_json();
        assert!(crate::doc::files::rename_in_feature(&mut w, "d", "bore"));
        assert_eq!(w["thread"]["depth"], "bore * 2");
        // The size and standard are names, never expressions.
        assert_eq!(w["thread"]["size"], "M8");
    }

    #[test]
    fn sizes_are_needed_only_without_intent() {
        let mut v = hole();
        v.as_object_mut().unwrap().remove("diameter");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/diameter");
        v["fastener"] = json!({"standard": "house_cap", "size": "M5"});
        v["format"] = json!(3); // 23 from Task 18
        v["counterbore"] = json!({"depth": "6 mm"}); // the diameter from the fastener
        let f = read(&v).unwrap();
        let Feature::Hole(h) = &f else { panic!() };
        assert!(h.diameter.is_none());
        assert!(matches!(
            &h.form,
            HoleForm::Counterbore {
                diameter: None,
                depth: Some(_)
            }
        ));
        assert_eq!(h.mouth_depth(), Some((0.006, "/counterbore/depth")));
        assert_eq!(read(&f.to_json()).unwrap(), f);
    }

    #[test]
    fn intent_refusals_say_what_to_do() {
        let base = || {
            json!({"format": 3, "kind": "ryucad.feature", "type": "hole",
                             "sketch": "s3", "form": "simple", "depth": "through_all",
                             "targets": ["box1"]})
        };
        let with = |k: &str, x: Value| {
            let mut v = base();
            v[k] = x;
            v
        };
        let err = |v: Value| read(&v).unwrap_err();
        // A thread and a fastener together.
        let mut v = with("thread", json!({"standard": "iso_metric", "size": "M6"}));
        v["fastener"] = json!({"standard": "house_cap", "size": "M5"});
        assert_eq!(err(v).details["reason"], "thread_and_fastener");
        // A fit on a tapped hole: its class is its tolerance.
        let mut v = with("thread", json!({"standard": "iso_metric", "size": "M6"}));
        v["fit"] = json!("H7");
        assert_eq!(err(v).details["reason"], "fit_on_thread");
        // An unknown size, with the nearest.
        let e = err(with(
            "thread",
            json!({"standard": "iso_metric", "size": "M6.5"}),
        ));
        assert_eq!(
            (e.code, e.details["reason"].as_str()),
            (Code::InvalidParam, Some("unknown_thread_size"))
        );
        assert_eq!(e.details["known"], json!(["M5", "M6", "M7", "M8"]));
        assert_eq!(e.details["pointer"], "/thread/size");
        // A pitch the size doesn't list.
        let e = err(with(
            "thread",
            json!({"standard": "iso_metric", "size": "M6", "pitch": "1.25 mm"}),
        ));
        assert_eq!(e.details["reason"], "pitch_not_standard");
        assert_eq!(e.details["listed"], json!(["1.0", "0.75"]));
        let e = err(with(
            "thread",
            json!({"standard": "iso_metric", "size": "M6", "class": "6g"}),
        ));
        assert_eq!(e.details["reason"], "unknown_thread_class");
        let e = err(with(
            "thread",
            json!({"standard": "npt", "size": "1/8", "class": "2B"}),
        ));
        assert_eq!(e.details["reason"], "class_not_taken");
        let e = err(with(
            "thread",
            json!({"standard": "iso_metric", "size": "M6", "tpi": 20}),
        ));
        assert_eq!(e.details["pointer"], "/thread/tpi");
        // A blind tapped hole without its thread depth: the fix.
        let mut v = with("thread", json!({"standard": "iso_metric", "size": "M8"}));
        v["depth"] = json!({"blind": "20 mm"});
        let e = err(v);
        assert_eq!(e.details["reason"], "thread_depth_needed");
        assert_eq!(e.details["suggested"], "16.25 mm"); // 20 - 3 × 1.25
        // A thread past its drill.
        let mut v = with(
            "thread",
            json!({"standard": "iso_metric", "size": "M8", "depth": "22 mm"}),
        );
        v["depth"] = json!({"blind": "20 mm"});
        assert_eq!(err(v).details["reason"], "thread_past_drill");
        // A fit that isn't an ISO 286 hole class.
        // (A plain hole gives its diameter.)
        let mut v = with("fit", json!("g6"));
        v["diameter"] = json!("6 mm");
        let e = err(v);
        assert_eq!(e.details["reason"], "fit_undefined");
        // A project standard reads: its sizes are checked when written.
        let v = with("thread", json!({"standard": "house_acme", "size": "T10"}));
        assert!(read(&v).is_ok());
    }
}
