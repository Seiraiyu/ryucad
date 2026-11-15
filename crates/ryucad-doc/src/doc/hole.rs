//! Hole features: holes drilled at a sketch's points, or at places given
//! by geometric references, into the solids they name. A hole always
//! cuts.

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
use std::collections::BTreeMap;

/// The feature format that holds a hole's thread, fastener, fit or
/// tolerance (ryucad 28). A hole holding none of them stays at 3, or 7
/// with a geometric up-to. Taken in the order stages merge: if another
/// stage holds 23 when this merges, move this to the next free number;
/// nothing else names it.
pub const INTENT_FORMAT: u64 = 23;

/// The feature format that holds a hole's new geometry (ryucad 28.B):
/// counterdrills, spotfaces, near and far treatments, through_next, tip
/// depths, ball bottoms and tapered walls. A hole holding none stays at
/// its format. Taken in the order stages merge: 25.C took 24 first, so
/// this is 25.
pub const GEOMETRY_FORMAT: u64 = 25;

/// The feature format that holds a hole's places (holes with no sketch)
/// and a pattern's fill (ryucad 28.C). A hole drilled at a sketch, and
/// every other pattern, stays at its format. Taken in the order stages
/// merge: if another stage holds 26 when this merges, move this to the
/// next free number; nothing else names it.
pub const PLACES_FORMAT: u64 = 26;

/// A hole at each of a sketch's points, or at each of its places.
#[derive(Clone, Debug, PartialEq)]
pub struct Hole {
    /// The sketch whose points it drills at; `None` for a hole at places.
    pub sketch: Option<Id>,
    /// Points of the sketch; empty means every point no curve uses.
    pub points: Vec<Id>,
    /// Where it drills with no sketch, by place ID (ryucad 28.C); empty
    /// for a hole at a sketch.
    pub places: BTreeMap<Id, Place>,
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
    /// A chamfer at the top of the bore.
    pub near: Option<Near>,
    /// A through hole's back-side treatment.
    pub far: Option<Far>,
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
    /// A tapered thread's wall is tapered too (`"wall": "taper"`), not a
    /// straight drill.
    pub taper_wall: bool,
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
        if self.taper_wall {
            v["wall"] = json!("taper");
        }
        v
    }

    /// A thread as a file holds it (a hole's, or a cosmetic thread's).
    pub fn read(v: &Value, default: Unit, env: &Env) -> Result<Thread> {
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
        let taper_wall = match o.take_opt("wall") {
            None => false,
            Some(w) if w == "taper" => true,
            Some(_) => {
                return Err(invalid(
                    "/thread/wall",
                    "wall is \"taper\", written only for a tapered wall",
                ));
            }
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
            taper_wall,
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

/// A chamfer or countersink at the very top of the bore (a tap's lead-in):
/// at the part's face for a simple hole, a counterbore or a counterdrill;
/// at the floor for a spotface.
#[derive(Clone, Debug, PartialEq)]
pub struct Near {
    pub diameter: Quantity,
    /// Included; 90 deg when left out.
    pub angle: Option<Quantity>,
}

/// A back-side treatment where a through bore leaves its target.
#[derive(Clone, Debug, PartialEq)]
pub enum Far {
    Countersink { diameter: Quantity, angle: Quantity },
    Counterbore { diameter: Quantity, depth: Quantity },
}

impl Near {
    fn to_json(&self) -> Value {
        let mut v = json!({"diameter": self.diameter.text()});
        if let Some(a) = &self.angle {
            v["angle"] = json!(a.text());
        }
        v
    }

    fn from_json(v: &Value, default: Unit, env: &Env) -> Result<Near> {
        let mut o = Obj::new(v, "/near")?;
        let diameter = size(
            &o.take("diameter")?,
            Field::Length(default),
            env,
            "/near/diameter",
            "a near chamfer's diameter",
        )?;
        let angle = o
            .take_opt("angle")
            .map(|a| angle(&a, env, "/near/angle", "a near chamfer"))
            .transpose()?;
        o.finish()?;
        Ok(Near { diameter, angle })
    }
}

impl Far {
    fn to_json(&self) -> Value {
        match self {
            Far::Countersink { diameter, angle } => json!({"countersink":
                {"diameter": diameter.text(), "angle": angle.text()}}),
            Far::Counterbore { diameter, depth } => json!({"counterbore":
                {"diameter": diameter.text(), "depth": depth.text()}}),
        }
    }

    fn from_json(v: &Value, default: Unit, env: &Env) -> Result<Far> {
        let Some((k, x)) = v
            .as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.iter().next())
        else {
            return Err(invalid(
                "/far",
                "far is {\"countersink\": {diameter, angle}} or {\"counterbore\": {diameter, depth}}",
            ));
        };
        let at = format!("/far/{k}");
        let mut o = Obj::new(x, &at)?;
        let len = Field::Length(default);
        let diameter = size(
            &o.take("diameter")?,
            len,
            env,
            &format!("{at}/diameter"),
            "a far side's diameter",
        )?;
        let far = match k.as_str() {
            "countersink" => Far::Countersink {
                diameter,
                angle: angle(
                    &o.take("angle")?,
                    env,
                    &format!("{at}/angle"),
                    "a far countersink",
                )?,
            },
            "counterbore" => Far::Counterbore {
                diameter,
                depth: size(
                    &o.take("depth")?,
                    len,
                    env,
                    &format!("{at}/depth"),
                    "a far counterbore's depth",
                )?,
            },
            _ => {
                return Err(invalid(
                    "/far",
                    "far is {\"countersink\": {diameter, angle}} or {\"counterbore\": {diameter, depth}}",
                ));
            }
        };
        o.finish()?;
        Ok(far)
    }
}

/// Where one hole of a hole feature is, with no sketch (design 533,
/// Placement without sketches). Every reference is a geometric one; what
/// kind each must be (a planar face, a straight edge) is checked when it
/// is rebuilt.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub kind: PlaceKind,
    /// Overrides the drilling direction: any axis. Never on a coaxial
    /// place.
    pub direction: Option<GeomRef>,
    /// Drills the other way.
    pub reverse: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaceKind {
    /// At a point (a vertex, a circular edge's centre, a sketch or datum
    /// point, `origin`). With a planar `face` the point lies on it; with
    /// a cylindrical or conical one the hole is radial, at the point's
    /// foot on it; with none, the point lies on exactly one face of the
    /// targets.
    At { at: GeomRef, face: Option<GeomRef> },
    /// Inside a planar face, `distance` from each of two straight edges.
    Edges {
        face: GeomRef,
        from: [(GeomRef, Quantity); 2],
    },
    /// On the axis of a cylindrical or conical face, a circular edge or
    /// any axis.
    Coaxial { of: GeomRef },
}

impl Place {
    pub fn to_json(&self) -> Value {
        let mut v = json!({});
        match &self.kind {
            PlaceKind::At { at, face } => {
                v["at"] = at.to_json();
                if let Some(f) = face {
                    v["face"] = f.to_json();
                }
            }
            PlaceKind::Edges { face, from } => {
                v["face"] = face.to_json();
                v["from"] = json!(
                    from.iter()
                        .map(|(e, d)| json!({"edge": e.to_json(), "distance": d.text()}))
                        .collect::<Vec<_>>()
                );
            }
            PlaceKind::Coaxial { of } => v["coaxial"] = of.to_json(),
        }
        if let Some(d) = &self.direction {
            v["direction"] = d.to_json();
        }
        if self.reverse {
            v["reverse"] = json!(true);
        }
        v
    }

    fn read(v: &Value, at: &str, default: Unit, env: &Env) -> Result<Place> {
        let mut o = Obj::new(v, at)?;
        let g = |o: &mut Obj, k: &str| -> Result<Option<GeomRef>> {
            o.take_opt(k)
                .map(|x| GeomRef::from_json(&x, &format!("{at}/{k}")))
                .transpose()
        };
        let point = g(&mut o, "at")?;
        let face = g(&mut o, "face")?;
        let coaxial = g(&mut o, "coaxial")?;
        let from = o.take_opt("from");
        let direction = g(&mut o, "direction")?;
        let reverse = match o.take_opt("reverse") {
            None => false,
            Some(Value::Bool(b)) => b,
            Some(_) => {
                return Err(invalid(
                    &format!("{at}/reverse"),
                    "reverse is true or false",
                ));
            }
        };
        o.finish()?;
        let kinds = [point.is_some(), from.is_some(), coaxial.is_some()];
        if kinds.iter().filter(|k| **k).count() != 1 {
            return Err(invalid(
                at,
                "a place is one of {at, face?}, {face, from: [two edges]} or {coaxial}",
            ));
        }
        let kind = if let Some(of) = coaxial {
            if face.is_some() {
                return Err(invalid(
                    &format!("{at}/face"),
                    "a coaxial place takes no face",
                ));
            }
            if direction.is_some() {
                return Err(invalid(
                    &format!("{at}/direction"),
                    "a coaxial place drills along its axis: give reverse, not a direction",
                ));
            }
            PlaceKind::Coaxial { of }
        } else if let Some(from) = from {
            let face = face.ok_or_else(|| {
                invalid(
                    &format!("{at}/face"),
                    "a place from two edges gives the face they bound",
                )
            })?;
            let p = format!("{at}/from");
            let a = from
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| invalid(&p, "from holds two {edge, distance}"))?;
            let mut two = Vec::with_capacity(2);
            for (i, x) in a.iter().enumerate() {
                let at_i = format!("{p}/{i}");
                let mut e = Obj::new(x, &at_i)?;
                let edge = GeomRef::from_json(&e.take("edge")?, &format!("{at_i}/edge"))?;
                let d = nonneg(
                    &e.take("distance")?,
                    default,
                    env,
                    &format!("{at_i}/distance"),
                    "a distance from an edge",
                )?;
                e.finish()?;
                two.push((edge, d));
            }
            let [a, b]: [(GeomRef, Quantity); 2] = two.try_into().expect("two");
            PlaceKind::Edges { face, from: [a, b] }
        } else {
            PlaceKind::At {
                at: point.expect("one kind"),
                face,
            }
        };
        Ok(Place {
            kind,
            direction,
            reverse,
        })
    }

    /// Its geometric references, each with its role under `places.<key>`.
    fn geometry<'a>(&'a self, key: &Id, out: &mut Vec<(String, &'a GeomRef)>) {
        let r = |k: &str| format!("places.{key}.{k}");
        match &self.kind {
            PlaceKind::At { at, face } => {
                out.push((r("at"), at));
                if let Some(f) = face {
                    out.push((r("face"), f));
                }
            }
            PlaceKind::Edges { face, from } => {
                out.push((r("face"), face));
                out.push((r("from.0"), &from[0].0));
                out.push((r("from.1"), &from[1].0));
            }
            PlaceKind::Coaxial { of } => out.push((r("coaxial"), of)),
        }
        if let Some(d) = &self.direction {
            out.push((r("direction"), d));
        }
    }
}

/// Reads `places`: a non-empty object of places by ID.
fn places_from_json(v: &Value, default: Unit, env: &Env) -> Result<BTreeMap<Id, Place>> {
    let o = v.as_object().filter(|o| !o.is_empty()).ok_or_else(|| {
        invalid(
            "/places",
            "places is an object of places by ID, at least one",
        )
    })?;
    let mut out = BTreeMap::new();
    for (k, x) in o {
        let at = format!("/places/{k}");
        let id = Id::parse(k, "place").map_err(|e| e.with("pointer", &at))?;
        out.insert(id, Place::read(x, &at, default, env)?);
    }
    Ok(out)
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
    /// A counterbore whose floor is a drill point of `angle` (118 deg when
    /// left out). With a fastener, `diameter` and `depth` may be left out
    /// (the fastener's counterbore).
    Counterdrill {
        diameter: Option<Quantity>,
        depth: Option<Quantity>,
        angle: Option<Quantity>,
    },
    /// A flat of `diameter` at `depth` below the mouth (0 when left out),
    /// its wall reaching up past the targets' box. With a fastener,
    /// `diameter` may be left out (the fastener's spotface).
    Spotface {
        diameter: Option<Quantity>,
        depth: Option<Quantity>,
    },
}

/// How deep the bore goes, from the sketch's plane.
#[derive(Clone, Debug, PartialEq)]
pub enum Depth {
    /// To the bore's full diameter (a drill point's shoulder).
    Blind(Quantity),
    /// Past everything of its targets.
    ThroughAll,
    /// Through the first wall its axis meets inside its targets, and no
    /// further.
    ThroughNext,
    /// Blind, to the drill point's tip (needs a drill bottom).
    Tip(Quantity),
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
    /// A half sphere of the bore's diameter (a ball end mill's).
    Ball,
    /// A drill point of this included angle.
    Drill(Quantity),
}

impl HoleForm {
    pub const NAMES: [&str; 5] = [
        "simple",
        "counterbore",
        "countersink",
        "counterdrill",
        "spotface",
    ];

    pub fn type_name(&self) -> &'static str {
        match self {
            HoleForm::Simple => "simple",
            HoleForm::Counterbore { .. } => "counterbore",
            HoleForm::Countersink { .. } => "countersink",
            HoleForm::Counterdrill { .. } => "counterdrill",
            HoleForm::Spotface { .. } => "spotface",
        }
    }

    /// The fields its mouth object takes, for messages.
    fn mouth_fields(name: &str) -> &'static str {
        match name {
            "counterbore" => "{diameter, depth}",
            "countersink" => "{diameter, angle}",
            "counterdrill" => "{diameter, depth, angle?}",
            _ => "{diameter, depth?}",
        }
    }
}

impl Depth {
    pub fn type_name(&self) -> &'static str {
        match self {
            Depth::Blind(_) => "blind",
            Depth::ThroughAll => "through_all",
            Depth::ThroughNext => "through_next",
            Depth::Tip(_) => "tip",
            Depth::UpTo { .. } => "up_to",
        }
    }

    /// Whether it ends in the material: blind, or to a tip.
    pub fn is_blind(&self) -> bool {
        matches!(self, Depth::Blind(_) | Depth::Tip(_))
    }

    /// Whether it goes through: all, or the next wall.
    pub fn is_through(&self) -> bool {
        matches!(self, Depth::ThroughAll | Depth::ThroughNext)
    }

    pub fn to_json(&self) -> Value {
        match self {
            Depth::Blind(q) => json!({"blind": q.text()}),
            Depth::ThroughAll => json!("through_all"),
            Depth::ThroughNext => json!("through_next"),
            Depth::Tip(q) => json!({"tip": q.text()}),
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
        if v == "through_next" {
            return Ok(Depth::ThroughNext);
        }
        let bad = || {
            invalid_extent(
                "/depth",
                "a hole's depth is {\"blind\": length}, {\"tip\": length}, \"through_all\", \"through_next\" or {\"up_to\": {target, offset}}"
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
            "tip" => Ok(Depth::Tip(size(
                x,
                Field::Length(default),
                env,
                "/depth/tip",
                "a hole's depth to its tip",
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
            Bottom::Ball => json!("ball"),
            Bottom::Drill(q) => json!({"drill": q.text()}),
        }
    }

    fn from_json(v: &Value, env: &Env) -> Result<Bottom> {
        if v == "flat" {
            return Ok(Bottom::Flat);
        }
        if v == "ball" {
            return Ok(Bottom::Ball);
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
                "a hole's bottom is \"flat\", \"ball\" or {\"drill\": angle}",
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

/// A tapped hole's thread resolved, as [`resolve_any`] does: an external
/// thread's class (`6g`) is refused, since a tapped hole is internal.
pub fn resolve(t: &Thread) -> Result<Option<builtin::Thread>> {
    match resolve_any(t)? {
        Some(b) if !b.internal => Err(refuse(
            "/thread/class",
            "external_class",
            format!(
                "{} is an external thread's class; a tapped hole is internal: give an internal class (6H, 2B) or none",
                t.class.as_deref().unwrap_or("")
            ),
        )),
        b => Ok(b),
    }
}

/// The built-in thread a hole's thread names, or `None` for a project
/// standard; a size, pitch or class the built-in doesn't have is refused
/// with what it does have.
pub fn resolve_any(t: &Thread) -> Result<Option<builtin::Thread>> {
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

/// A length of zero or more, at `at`.
fn nonneg(v: &Value, default: Unit, env: &Env, at: &str, what: &str) -> Result<Quantity> {
    let q = Quantity::from_json_as(v, Field::Length(default), &|n| env.get(n))
        .map_err(|e| e.with("pointer", at))?;
    if q.meters() < 0.0 {
        return Err(bad_size(at, &format!("{what} is zero or more"), &q));
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
            "form": self.form.type_name(),
            "depth": self.depth.to_json(),
            "targets": self.targets,
        });
        if let Some(s) = &self.sketch {
            v["sketch"] = json!(s);
        }
        if !self.places.is_empty() {
            v["places"] = Value::Object(
                self.places
                    .iter()
                    .map(|(k, p)| (k.to_string(), p.to_json()))
                    .collect(),
            );
        }
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
            HoleForm::Counterdrill {
                diameter,
                depth,
                angle,
            } => {
                v["counterdrill"] =
                    mouth(&[("diameter", diameter), ("depth", depth), ("angle", angle)]);
            }
            HoleForm::Spotface { diameter, depth } => {
                v["spotface"] = mouth(&[("diameter", diameter), ("depth", depth)]);
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
        if let Some(n) = &self.near {
            v["near"] = n.to_json();
        }
        if let Some(f) = &self.far {
            v["far"] = f.to_json();
        }
        v
    }

    /// Whether it holds anything new with ryucad 28: written at
    /// `INTENT_FORMAT`, its sizes derived.
    pub fn has_intent(&self) -> bool {
        self.thread.is_some() || self.fastener.is_some() || self.bore.is_some()
    }

    /// Whether it holds anything new with ryucad 28.B: written at
    /// `GEOMETRY_FORMAT`.
    pub fn has_geometry(&self) -> bool {
        matches!(
            self.form,
            HoleForm::Counterdrill { .. } | HoleForm::Spotface { .. }
        ) || self.near.is_some()
            || self.far.is_some()
            || matches!(self.depth, Depth::ThroughNext | Depth::Tip(_))
            || self.bottom == Bottom::Ball
            || self.thread.as_ref().is_some_and(|t| t.taper_wall)
    }

    /// Whether it is drilled at places (ryucad 28.C): written at
    /// `PLACES_FORMAT`.
    pub fn has_places(&self) -> bool {
        !self.places.is_empty()
    }

    /// Whether its JSON holds anything [`Hole::has_geometry`] counts.
    pub fn json_has_geometry(v: &Value) -> bool {
        matches!(
            v.get("form").and_then(Value::as_str),
            Some("counterdrill" | "spotface")
        ) || v.get("near").is_some()
            || v.get("far").is_some()
            || v.get("depth")
                .is_some_and(|d| d == "through_next" || d.get("tip").is_some())
            || v.get("bottom").is_some_and(|b| b == "ball")
            || v.pointer("/thread/wall").is_some()
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Hole> {
        let length = Field::Length(default);
        let sketch = o
            .take_opt("sketch")
            .map(|s| {
                let s = s
                    .as_str()
                    .ok_or_else(|| invalid("/sketch", "sketch is a sketch feature's ID"))?;
                Id::parse(s, "feature").map_err(|e| e.with("pointer", "/sketch"))
            })
            .transpose()?;
        let points = points_from_json(o.take_opt("points"))?;
        let places = o
            .take_opt("places")
            .map(|v| places_from_json(&v, default, env))
            .transpose()?
            .unwrap_or_default();
        match (&sketch, places.is_empty()) {
            (Some(_), false) => {
                return Err(invalid(
                    "/places",
                    "a hole is drilled at a sketch's points or at places, not both",
                ));
            }
            (None, true) => {
                return Err(invalid(
                    "/sketch",
                    "a hole is drilled at a sketch's points (sketch) or at places (places)",
                ));
            }
            _ => {}
        }
        if sketch.is_none() && !points.is_empty() {
            return Err(invalid(
                "/points",
                "points are a sketch's: a hole at places has none",
            ));
        }
        let thread = o
            .take_opt("thread")
            .map(|t| Thread::read(&t, default, env))
            .transpose()?;
        let fastener = o
            .take_opt("fastener")
            .map(|f| Fastener::from_json(&f))
            .transpose()?;
        let bore =
            BoreTolerance::from_json(o.take_opt("fit"), o.take_opt("tolerance"), default, env)?;
        let near = o
            .take_opt("near")
            .map(|n| Near::from_json(&n, default, env))
            .transpose()?;
        let far = o
            .take_opt("far")
            .map(|f| Far::from_json(&f, default, env))
            .transpose()?;
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
        let mut mouths: Vec<(&str, Option<Value>)> = HoleForm::NAMES[1..]
            .iter()
            .map(|k| (*k, o.take_opt(k)))
            .collect();
        let Some(name) = form.as_str().filter(|f| HoleForm::NAMES.contains(f)) else {
            return Err(invalid(
                "/form",
                "a hole's form is simple, counterbore, countersink, counterdrill or spotface",
            ));
        };
        let others: Vec<&str> = mouths
            .iter()
            .filter(|(k, v)| v.is_some() && *k != name)
            .map(|(k, _)| *k)
            .collect();
        if name == "simple" && !others.is_empty() {
            return Err(invalid(
                "/form",
                &format!("a simple hole has no {}", others.join(" or ")),
            ));
        }
        let own = mouths
            .iter_mut()
            .find(|(k, _)| *k == name)
            .and_then(|(_, v)| v.take());
        if name != "simple" && (own.is_none() || !others.is_empty()) {
            let at = format!("/{name}");
            return Err(invalid(
                &at,
                &format!(
                    "a {name} hole has a {name} {}, and no other mouth",
                    HoleForm::mouth_fields(name)
                ),
            ));
        }
        let form = match (name, own) {
            ("simple", _) => HoleForm::Simple,
            ("counterbore", Some(c)) => {
                let mut c = Obj::new(&c, "/counterbore")?;
                let d = mouth_size(&mut c, "diameter", "/counterbore", "a counterbore", false)?;
                let depth = mouth_size(&mut c, "depth", "/counterbore", "a counterbore", false)?;
                c.finish()?;
                if let (Some(d), Some(b)) = (&d, &diameter) {
                    wider(d, b, "/counterbore/diameter", "a counterbore")?;
                }
                HoleForm::Counterbore { diameter: d, depth }
            }
            ("countersink", Some(c)) => {
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
            ("counterdrill", Some(c)) => {
                let mut c = Obj::new(&c, "/counterdrill")?;
                let d = mouth_size(&mut c, "diameter", "/counterdrill", "a counterdrill", false)?;
                let depth = mouth_size(&mut c, "depth", "/counterdrill", "a counterdrill", false)?;
                // The angle is never derived: 118 deg when left out.
                let angle = c
                    .take_opt("angle")
                    .map(|x| angle(&x, env, "/counterdrill/angle", "a counterdrill"))
                    .transpose()?;
                c.finish()?;
                if let (Some(d), Some(b)) = (&d, &diameter) {
                    wider(d, b, "/counterdrill/diameter", "a counterdrill")?;
                }
                HoleForm::Counterdrill {
                    diameter: d,
                    depth,
                    angle,
                }
            }
            ("spotface", Some(c)) => {
                let mut c = Obj::new(&c, "/spotface")?;
                let d = mouth_size(&mut c, "diameter", "/spotface", "a spotface", false)?;
                let depth = c
                    .take_opt("depth")
                    .map(|x| nonneg(&x, default, env, "/spotface/depth", "a spotface's depth"))
                    .transpose()?;
                c.finish()?;
                if let (Some(d), Some(b)) = (&d, &diameter) {
                    wider(d, b, "/spotface/diameter", "a spotface")?;
                }
                HoleForm::Spotface { diameter: d, depth }
            }
            _ => unreachable!("the form's own mouth was checked"),
        };
        let depth = Depth::from_json(&o.take("depth")?, default, env)?;
        let bottom = match o.take_opt("bottom") {
            None => Bottom::Flat,
            Some(b) => Bottom::from_json(&b, env)?,
        };
        match (&bottom, &depth) {
            (Bottom::Drill(_), Depth::Blind(_) | Depth::Tip(_)) => {}
            (Bottom::Drill(_), d) => {
                return Err(invalid_extent(
                    "/bottom",
                    format!(
                        "a drill point ends a blind hole; a {} hole has no bottom of its own",
                        d.type_name()
                    ),
                ));
            }
            (Bottom::Ball, Depth::Blind(_)) => {}
            (Bottom::Ball, d) => {
                return Err(invalid_extent(
                    "/bottom",
                    format!("a ball ends a blind hole, not a {} one", d.type_name()),
                ));
            }
            (Bottom::Flat, Depth::Tip(_)) => {
                return Err(invalid_extent(
                    "/bottom",
                    "a depth to the tip is to a drill point's: give bottom {\"drill\": angle}"
                        .into(),
                ));
            }
            _ => {}
        }
        // Full depth on a through hole is the default: never written.
        let thread = thread.map(|mut t| {
            if !depth.is_blind() && t.depth == Some(ThreadDepth::Full) {
                t.depth = None;
            }
            t
        });
        let targets = targets_from_json(&o.take("targets")?, "/targets")?;
        let hole = Hole {
            sketch,
            points,
            places,
            form,
            diameter,
            depth,
            bottom,
            targets,
            thread,
            fastener,
            bore,
            near,
            far,
        };
        if hole.near.is_some() && matches!(hole.form, HoleForm::Countersink { .. }) {
            return Err(refuse(
                "/near",
                "near_on_countersink",
                "a countersink is its own near chamfer: take near out, or make the hole simple"
                    .into(),
            ));
        }
        if hole.far.is_some() && !hole.depth.is_through() {
            return Err(refuse(
                "/far",
                "far_needs_through",
                format!(
                    "a far side is where the bore leaves its target: give a through depth, not {}",
                    hole.depth.type_name()
                ),
            ));
        }
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
            if t.taper_wall {
                if built.as_ref().and_then(|b| b.taper).is_none() {
                    return Err(refuse(
                        "/thread/wall",
                        "wall_not_tapered",
                        format!(
                            "{} {} isn't a tapered thread: take wall out",
                            t.standard, t.size
                        ),
                    ));
                }
                if !matches!(t.depth, Some(ThreadDepth::Length(_))) {
                    return Err(refuse(
                        "/thread/depth",
                        "thread_depth_needed",
                        "a tapered wall runs a given length: give the thread's depth".into(),
                    ));
                }
            }
            if let Depth::Blind(drill) | Depth::Tip(drill) = &hole.depth {
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
            HoleForm::Counterdrill {
                diameter,
                depth,
                angle,
            } => {
                let half = angle.as_ref().map_or(118f64.to_radians(), |a| a.radians()) / 2.0;
                let run = (diameter.as_ref()?.meters() - self.diameter.as_ref()?.meters()) / 2.0;
                Some((
                    depth.as_ref()?.meters() + run / ryukei::math::tan(half),
                    "/counterdrill/depth",
                ))
            }
            HoleForm::Spotface { depth, .. } => Some((
                depth.as_ref().map_or(0.0, Quantity::meters),
                "/spotface/depth",
            )),
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
    /// `depth`, and each place's `places.<key>.<field>`.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        let mut out = Vec::new();
        if let Depth::UpTo { target, .. } = &self.depth {
            out.push(("depth".to_owned(), target));
        }
        for (k, p) in &self.places {
            p.geometry(k, &mut out);
        }
        out
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
/// countersink's sizes, a blind depth, an up-to offset, a drill
/// point's angle, and each place's distances from its edges.
pub fn expression_strings<'a>(k: &str, field: &'a mut Value) -> Vec<&'a mut String> {
    let mut out = Vec::new();
    match (k, field) {
        ("places", Value::Object(o)) => {
            for place in o.values_mut() {
                if let Some(Value::Array(from)) = place.get_mut("from") {
                    for e in from {
                        if let Some(Value::String(s)) = e.get_mut("distance") {
                            out.push(s);
                        }
                    }
                }
            }
        }
        ("counterbore" | "countersink" | "counterdrill" | "spotface", Value::Object(o)) => {
            for x in o.values_mut() {
                if let Value::String(s) = x {
                    out.push(s);
                }
            }
        }
        ("near", Value::Object(o)) => {
            for x in o.values_mut() {
                if let Value::String(s) = x {
                    out.push(s);
                }
            }
        }
        ("far", Value::Object(o)) => {
            for inner in o.values_mut() {
                if let Value::Object(i) = inner {
                    for x in i.values_mut() {
                        if let Value::String(s) = x {
                            out.push(s);
                        }
                    }
                }
            }
        }
        ("depth", Value::Object(o)) => {
            for (k, x) in o.iter_mut() {
                match (k.as_str(), x) {
                    ("blind" | "tip", Value::String(s)) => out.push(s),
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
        json!({"format": 23, "kind": "ryucad.feature", "type": "hole", "sketch": "s3",
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
        let v = json!({"format": 23, "kind": "ryucad.feature", "type": "hole", "sketch": "s3",
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
        v["format"] = json!(23);
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
            json!({"format": 23, "kind": "ryucad.feature", "type": "hole",
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
            json!({"standard": "iso_metric", "size": "M6", "class": "6e"}),
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

    #[test]
    fn counterdrills_and_spotfaces_round_trip() {
        for (form, mouth) in [
            (
                "counterdrill",
                json!({"diameter": "10 mm", "depth": "5 mm"}),
            ),
            (
                "counterdrill",
                json!({"diameter": "10 mm", "depth": "5 mm", "angle": "100 deg"}),
            ),
            ("spotface", json!({"diameter": "12 mm"})),
            ("spotface", json!({"diameter": "12 mm", "depth": "0.5 mm"})),
        ] {
            let mut v = hole();
            v["format"] = json!(25);
            v.as_object_mut().unwrap().remove("counterbore");
            v["form"] = json!(form);
            v[form] = mouth.clone();
            let f = read(&v).unwrap_or_else(|e| panic!("{form} {mouth}: {e}"));
            let Feature::Hole(h) = &f else { panic!() };
            assert_eq!(h.form.type_name(), form);
            assert_eq!(f.to_json(), v, "{form} keeps its bytes");
        }
        // A spotface's depth may be zero (flush), never less.
        let mut v = hole();
        v["format"] = json!(25);
        v.as_object_mut().unwrap().remove("counterbore");
        v["form"] = json!("spotface");
        v["spotface"] = json!({"diameter": "12 mm", "depth": "0 mm"});
        read(&v).unwrap();
        v["spotface"]["depth"] = json!("-1 mm");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/spotface/depth");
        // Without a fastener, a counterdrill gives its diameter and depth,
        // a spotface its diameter; each is wider than the bore.
        for (form, mouth, pointer) in [
            (
                "counterdrill",
                json!({"depth": "5 mm"}),
                "/counterdrill/diameter",
            ),
            (
                "counterdrill",
                json!({"diameter": "10 mm"}),
                "/counterdrill/depth",
            ),
            ("spotface", json!({}), "/spotface/diameter"),
            (
                "spotface",
                json!({"diameter": "2 mm"}),
                "/spotface/diameter",
            ),
        ] {
            let mut v = hole();
            v["format"] = json!(25);
            v.as_object_mut().unwrap().remove("counterbore");
            v["form"] = json!(form);
            v[form] = mouth;
            assert_eq!(read(&v).unwrap_err().details["pointer"], pointer, "{form}");
        }
        // Only the form's own mouth.
        let mut v = hole();
        v["form"] = json!("simple");
        v["spotface"] = json!({"diameter": "12 mm"});
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/form");
    }

    #[test]
    fn near_and_far_round_trip_and_refuse() {
        let mut v = hole();
        v["format"] = json!(25);
        // Through: no bottom of its own.
        v.as_object_mut().unwrap().remove("bottom");
        v["depth"] = json!("through_all");
        v["near"] = json!({"diameter": "7 mm"});
        v["far"] = json!({"countersink": {"diameter": "8 mm", "angle": "90 deg"}});
        let f = read(&v).unwrap();
        assert_eq!(f.to_json(), v);
        v["near"] = json!({"diameter": "7 mm", "angle": "120 deg"});
        v["far"] = json!({"counterbore": {"diameter": "8 mm", "depth": "2 mm"}});
        assert_eq!(read(&v).unwrap().to_json(), v);
        // A far side is where the bore leaves its target: through only.
        v["depth"] = json!({"blind": "3 mm"});
        let e = read(&v).unwrap_err();
        assert_eq!(
            (e.details["pointer"].as_str(), e.details["reason"].as_str()),
            (Some("/far"), Some("far_needs_through"))
        );
        // No near chamfer on a countersink: it is one.
        let mut v = hole();
        v["format"] = json!(25);
        v.as_object_mut().unwrap().remove("counterbore");
        v["form"] = json!("countersink");
        v["countersink"] = json!({"diameter": "8 mm", "angle": "90 deg"});
        v["near"] = json!({"diameter": "9 mm"});
        let e = read(&v).unwrap_err();
        assert_eq!(e.details["reason"], "near_on_countersink");
        // One far treatment, whole.
        let mut v = hole();
        v["depth"] = json!("through_all");
        v["far"] = json!({"countersink": {"diameter": "8 mm"}});
        assert_eq!(
            read(&v).unwrap_err().details["pointer"],
            "/far/countersink/angle"
        );
        v["far"] = json!({"countersink": {"diameter": "8 mm", "angle": "90 deg"},
                          "counterbore": {"diameter": "8 mm", "depth": "1 mm"}});
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/far");
    }

    #[test]
    fn new_depths_bottoms_and_tapered_walls() {
        let at24 = |f: &dyn Fn(&mut Value)| {
            let mut v = hole();
            v["format"] = json!(25);
            // Each case gives its own bottom.
            v.as_object_mut().unwrap().remove("bottom");
            f(&mut v);
            v
        };
        // Round trips.
        for v in [
            at24(&|v| v["depth"] = json!("through_next")),
            at24(&|v| {
                v["depth"] = json!({"tip": "12 mm"});
                v["bottom"] = json!({"drill": "118 deg"});
            }),
            at24(&|v| {
                v["depth"] = json!({"blind": "10 mm"});
                v["bottom"] = json!("ball");
            }),
            at24(&|v| {
                v.as_object_mut().unwrap().remove("diameter");
                v["format"] = json!(25);
                v["thread"] =
                    json!({"standard": "npt", "size": "1/8", "depth": "8 mm", "wall": "taper"});
                v["depth"] = json!({"blind": "12 mm"});
            }),
        ] {
            assert_eq!(read(&v).unwrap_or_else(|e| panic!("{v}: {e}")).to_json(), v);
        }
        // Refusals.
        for (v, pointer, reason) in [
            // A tip depth is to a drill point.
            (
                at24(&|v| v["depth"] = json!({"tip": "12 mm"})),
                "/bottom",
                None,
            ),
            // A ball ends a blind hole.
            (
                at24(&|v| {
                    v["depth"] = json!("through_all");
                    v["bottom"] = json!("ball");
                }),
                "/bottom",
                None,
            ),
            // A tapered wall is a tapered thread's...
            (
                at24(&|v| {
                    v.as_object_mut().unwrap().remove("diameter");
                    v["thread"] = json!({"standard": "iso_metric", "size": "M6", "wall": "taper"});
                    v["depth"] = json!("through_all");
                }),
                "/thread/wall",
                Some("wall_not_tapered"),
            ),
            // ...and runs a given length.
            (
                at24(&|v| {
                    v.as_object_mut().unwrap().remove("diameter");
                    v["thread"] = json!({"standard": "npt", "size": "1/8", "wall": "taper"});
                    v["depth"] = json!("through_all");
                }),
                "/thread/depth",
                Some("thread_depth_needed"),
            ),
        ] {
            let e = read(&v).unwrap_err();
            assert_eq!(e.details["pointer"], pointer, "{v}: {e}");
            if let Some(r) = reason {
                assert_eq!(e.details["reason"], r, "{v}: {e}");
            }
        }
        // A blind tapped hole to a tip still gives its thread's depth.
        let v = at24(&|v| {
            v.as_object_mut().unwrap().remove("diameter");
            v["thread"] = json!({"standard": "iso_metric", "size": "M6"});
            v["depth"] = json!({"tip": "15 mm"});
            v["bottom"] = json!({"drill": "118 deg"});
        });
        assert_eq!(
            read(&v).unwrap_err().details["reason"],
            "thread_depth_needed"
        );
        // A far side on through_next is taken.
        let v = at24(&|v| {
            v["depth"] = json!("through_next");
            v["far"] = json!({"counterbore": {"diameter": "8 mm", "depth": "1 mm"}});
        });
        read(&v).unwrap();
    }

    #[test]
    fn a_tapped_hole_is_internal() {
        let mut v = hole();
        v["format"] = json!(23);
        v.as_object_mut().unwrap().remove("diameter");
        v["thread"] = json!({"standard": "iso_metric", "size": "M6", "class": "6g"});
        v["depth"] = json!("through_all");
        v.as_object_mut().unwrap().remove("bottom");
        let e = read(&v).unwrap_err();
        assert_eq!(
            (e.details["pointer"].as_str(), e.details["reason"].as_str()),
            (Some("/thread/class"), Some("external_class"))
        );
    }

    fn hole_of(v: Value) -> Result<Hole> {
        let mut o = Obj::new(&v, "").unwrap();
        let h = Hole::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(h)
    }

    const F: &str = "0123456789abcdef0123456789abcdef";
    const E: &str = "fedcba9876543210fedcba9876543210";
    const G: &str = "00112233445566778899aabbccddeeff";

    #[test]
    fn places_round_trip_in_every_kind() {
        let v = json!({
            "form": "simple", "diameter": "5 mm", "depth": "through_all", "targets": ["b"],
            "places": {
                "a": {"at": {"feature": "b", "name": F}},
                "b": {"at": {"feature": "pt"}, "face": {"feature": "b", "name": E},
                      "direction": "z", "reverse": true},
                "c": {"face": {"feature": "b", "name": E},
                      "from": [{"edge": {"feature": "b", "name": F}, "distance": "10 mm"},
                               {"edge": {"feature": "b", "name": G}, "distance": "d"}]},
                "d": {"coaxial": {"feature": "b", "name": G}}
            }
        });
        let p = [("d".to_owned(), crate::expr::Expr::parse("12 mm").unwrap())].into();
        let env = Env::new(&p);
        let mut o = Obj::new(&v, "").unwrap();
        let h = Hole::from_fields(&mut o, Unit::Mm, &env).unwrap();
        o.finish().unwrap();
        assert_eq!(h.sketch, None);
        assert!(h.points.is_empty());
        assert_eq!(h.places.len(), 4);
        assert!(h.has_places());
        assert_eq!(h.fields(), v);
        // Its roles, in key order, each place's in field order.
        let roles: Vec<String> = h.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(
            roles,
            [
                "places.a.at",
                "places.b.at",
                "places.b.face",
                "places.b.direction",
                "places.c.face",
                "places.c.from.0",
                "places.c.from.1",
                "places.d.coaxial",
            ]
        );
    }

    #[test]
    fn a_hole_is_at_a_sketch_or_at_places() {
        let base = json!({"form": "simple", "diameter": "5 mm", "depth": "through_all",
                          "targets": ["b"]});
        let place = json!({"a": {"at": "origin"}});
        let mut both = base.clone();
        both["sketch"] = json!("s");
        both["places"] = place.clone();
        let e = hole_of(both).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidDocument, Some("/places"))
        );
        let e = hole_of(base.clone()).unwrap_err();
        assert_eq!(e.details["pointer"], "/sketch");
        let mut pts = base.clone();
        pts["places"] = place.clone();
        pts["points"] = json!(["p1"]);
        assert_eq!(hole_of(pts).unwrap_err().details["pointer"], "/points");
        let mut empty = base.clone();
        empty["places"] = json!({});
        assert_eq!(hole_of(empty).unwrap_err().details["pointer"], "/places");
    }

    #[test]
    fn a_place_is_one_of_its_kinds() {
        let base = json!({"form": "simple", "diameter": "5 mm", "depth": "through_all",
                          "targets": ["b"]});
        let refused = |p: Value| -> (Code, String) {
            let mut v = base.clone();
            v["places"] = json!({"a": p});
            let e = hole_of(v).unwrap_err();
            (
                e.code,
                e.details["pointer"].as_str().unwrap_or_default().to_owned(),
            )
        };
        let face = json!({"feature": "b", "name": E});
        let edge = |n: &str| json!({"edge": {"feature": "b", "name": n}, "distance": "5 mm"});
        // Nothing to place it by; two kinds at once; from without a face.
        assert_eq!(refused(json!({})).1, "/places/a");
        assert_eq!(
            refused(json!({"at": "origin", "coaxial": face})).1,
            "/places/a"
        );
        assert_eq!(
            refused(json!({"from": [edge(F), edge(G)]})).1,
            "/places/a/face"
        );
        // From needs exactly two edges, each a distance of zero or more.
        assert_eq!(
            refused(json!({"face": face, "from": [edge(F)]})).1,
            "/places/a/from"
        );
        let mut neg = edge(G);
        neg["distance"] = json!("-1 mm");
        assert_eq!(
            refused(json!({"face": face, "from": [edge(F), neg]})).1,
            "/places/a/from/1/distance"
        );
        // A coaxial place drills along its axis: no direction.
        assert_eq!(
            refused(json!({"coaxial": face, "direction": "z"})).1,
            "/places/a/direction"
        );
        // A face alone is no place.
        assert_eq!(refused(json!({"face": face})).1, "/places/a");
        // Keys are IDs.
        let mut bad = base.clone();
        bad["places"] = json!({"no good": {"at": "origin"}});
        assert_eq!(
            hole_of(bad).unwrap_err().details["pointer"],
            "/places/no good"
        );
    }

    #[test]
    fn a_sketch_hole_reads_and_writes_as_before() {
        let v = json!({"sketch": "s", "points": ["p1", "p2"], "form": "simple",
                       "diameter": "5 mm", "depth": "through_all", "targets": ["b"]});
        let h = hole_of(v.clone()).unwrap();
        assert_eq!(h.sketch, Some(Id::parse("s", "feature").unwrap()));
        assert!(h.places.is_empty() && !h.has_places());
        assert_eq!(h.fields(), v);
    }
}
