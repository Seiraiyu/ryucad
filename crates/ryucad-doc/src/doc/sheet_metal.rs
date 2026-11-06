//! Sheet metal features (design 346): a base flange (`base_flange`), which
//! founds a sheet metal solid from a sketch (a plate of its closed loops,
//! or a bent section of one open chain) and holds the solid's settings;
//! and an edge flange (`flange`), which adds a bend and a flange on each
//! of its straight edges of one such solid, taking the settings it
//! doesn't give from that solid's founder. Unfold and refold are plan
//! 16.B's.

use super::files::{Obj, invalid};
use super::id::Id;
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// The feature format base flanges and flanges are new in.
pub const FORMAT: u64 = 11;

/// How long a bend is in the flat (design 346's Bend allowance).
#[derive(Clone, Debug, PartialEq)]
pub enum Allowance {
    /// The neutral radius is `r + k t`: `ba = θ (r + k t)`, `k` in `[0, 1]`.
    KFactor(Quantity),
    /// A bend deduction `bd`, how much shorter the flat is than its two
    /// flanges to their outer mould line: `ba = 2 (r + t) tan(θ / 2) - bd`,
    /// defined under a half turn.
    Deduction(Quantity),
    /// A bend's developed length itself: one bend's, never a solid's
    /// (16.B's overrides).
    Length(Quantity),
}

impl Allowance {
    /// `{"k_factor": n}`, `{"deduction": q}` or `{"allowance": q}`.
    pub fn to_json(&self) -> Value {
        match self {
            Allowance::KFactor(k) => json!({"k_factor": k.text()}),
            Allowance::Deduction(q) => json!({"deduction": q.text()}),
            Allowance::Length(q) => json!({"allowance": q.text()}),
        }
    }

    /// Reads one, at `at`; a solid's (`whole`) is a K-factor or a
    /// deduction.
    pub fn from_json(
        v: &Value,
        at: &str,
        default: Unit,
        env: &Env,
        whole: bool,
    ) -> Result<Allowance> {
        let what = if whole {
            "a solid's allowance is {\"k_factor\": n} or {\"deduction\": length}"
        } else {
            "an allowance is {\"k_factor\": n}, {\"deduction\": length} or {\"allowance\": length}"
        };
        let mut o = Obj::new(v, at).map_err(|_| invalid(at, what))?;
        let (key, x) = match (
            o.take_opt("k_factor"),
            o.take_opt("deduction"),
            o.take_opt("allowance"),
        ) {
            (Some(x), None, None) => ("k_factor", x),
            (None, Some(x), None) => ("deduction", x),
            (None, None, Some(x)) if !whole => ("allowance", x),
            _ => return Err(invalid(at, what)),
        };
        o.finish()?;
        let at = format!("{at}/{key}");
        let lookup = |n: &str| env.get(n);
        Ok(match key {
            "k_factor" => {
                let k = Quantity::from_json_as(&x, Field::Number, &lookup)
                    .map_err(|e| e.with("pointer", &at))?;
                if !(0.0..=1.0).contains(&k.number()) {
                    return Err(out_of_range(
                        &at,
                        &format!("a K-factor is from 0 to 1, not {k}"),
                        &k,
                    ));
                }
                Allowance::KFactor(k)
            }
            _ => {
                let q = positive(&x, default, env, &at, "an allowance or deduction")?;
                if key == "deduction" {
                    Allowance::Deduction(q)
                } else {
                    Allowance::Length(q)
                }
            }
        })
    }
}

/// A bend relief at an end of a flange's edge, sizes as the kernel takes
/// them (`ryukei::sheet_metal::Relief`).
#[derive(Clone, Debug, PartialEq)]
pub enum Relief {
    None,
    Rectangular { width: Quantity, depth: Quantity },
    Obround { width: Quantity, depth: Quantity },
    Tear { width: Quantity },
}

impl Relief {
    pub fn kind(&self) -> &'static str {
        match self {
            Relief::None => "none",
            Relief::Rectangular { .. } => "rectangular",
            Relief::Obround { .. } => "obround",
            Relief::Tear { .. } => "tear",
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"kind": self.kind()});
        match self {
            Relief::None => {}
            Relief::Rectangular { width, depth } | Relief::Obround { width, depth } => {
                v["width"] = json!(width.text());
                v["depth"] = json!(depth.text());
            }
            Relief::Tear { width } => v["width"] = json!(width.text()),
        }
        v
    }

    pub fn from_json(v: &Value, at: &str, default: Unit, env: &Env) -> Result<Relief> {
        let what = "a relief is {\"kind\": none | rectangular | obround | tear, width?, depth?}";
        let mut o = Obj::new(v, at).map_err(|_| invalid(at, what))?;
        let kind = o.take("kind")?;
        let width = |o: &mut Obj| {
            positive(
                &o.take("width")?,
                default,
                env,
                &format!("{at}/width"),
                "a relief's width",
            )
        };
        let depth = |o: &mut Obj| {
            non_negative(
                &o.take("depth")?,
                default,
                env,
                &format!("{at}/depth"),
                "a relief's depth",
            )
        };
        let r = match kind.as_str() {
            Some("none") => Relief::None,
            Some("rectangular") => Relief::Rectangular {
                width: width(&mut o)?,
                depth: depth(&mut o)?,
            },
            Some("obround") => Relief::Obround {
                width: width(&mut o)?,
                depth: depth(&mut o)?,
            },
            Some("tear") => Relief::Tear {
                width: width(&mut o)?,
            },
            _ => return Err(invalid(&format!("{at}/kind"), what)),
        };
        o.finish()?;
        Ok(r)
    }
}

/// A bent section's extent: `depth` along the sketch's normal, or
/// half each way when `symmetric`.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub depth: Quantity,
    pub symmetric: bool,
}

/// A base flange: a plate of `sketch`'s closed loops (all, or those
/// holding an entity of `loops`) extruded by `thickness` along its normal
/// (`reverse` against it); or, with a `section`, a bent section of its
/// one open chain `thickness` thick (on the chain's left walked from its
/// first end, `reverse` its right) extruded by the section's depth. It
/// founds a solid and holds that solid's settings: `radius`, the default
/// inner bend radius, `allowance` and `relief`.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseFlange {
    pub sketch: Id,
    pub loops: Vec<Id>,
    pub thickness: Quantity,
    pub reverse: bool,
    pub section: Option<Section>,
    pub radius: Quantity,
    pub allowance: Allowance,
    pub relief: Relief,
}

/// Where an edge flange's length is measured from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Measured {
    /// The outer mould line (written as nothing).
    #[default]
    OuterMould,
    InnerMould,
    Tangent,
}

impl Measured {
    pub fn as_str(self) -> &'static str {
        match self {
            Measured::OuterMould => "outer_mould",
            Measured::InnerMould => "inner_mould",
            Measured::Tangent => "tangent",
        }
    }

    pub fn parse(s: &str) -> Option<Measured> {
        [
            Measured::OuterMould,
            Measured::InnerMould,
            Measured::Tangent,
        ]
        .into_iter()
        .find(|m| m.as_str() == s)
    }
}

/// Where an edge flange's bend sits against its edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    /// Material inside (written as nothing).
    #[default]
    MaterialInside,
    MaterialOutside,
    BendOutside,
}

impl Position {
    pub fn as_str(self) -> &'static str {
        match self {
            Position::MaterialInside => "material_inside",
            Position::MaterialOutside => "material_outside",
            Position::BendOutside => "bend_outside",
        }
    }

    pub fn parse(s: &str) -> Option<Position> {
        [
            Position::MaterialInside,
            Position::MaterialOutside,
            Position::BendOutside,
        ]
        .into_iter()
        .find(|p| p.as_str() == s)
    }
}

/// An edge a flange is made on, and the side face it folds towards when
/// the edge's two faces could both be sides.
#[derive(Clone, Debug, PartialEq)]
pub struct FlangeEdge {
    pub edge: Reference,
    pub side: Option<Reference>,
}

/// What an end vertex of a flange's edges takes instead of the flange's
/// own setback and relief.
#[derive(Clone, Debug, PartialEq)]
pub struct End {
    pub vertex: Reference,
    pub setback: Option<Quantity>,
    pub relief: Option<Relief>,
}

/// An edge flange: a bend of `angle` and a flange of `length` (measured
/// from `measured`) on each of `edges`, a straight edge of `target` between
/// a flange side and a square planar wall, all in one kernel call. Its
/// `radius` and `relief` are the founder's unless written; `setback` is at
/// both ends of every edge (0 unless written); `ends` override both by
/// vertex.
#[derive(Clone, Debug, PartialEq)]
pub struct Flange {
    pub target: Id,
    pub edges: Vec<FlangeEdge>,
    pub angle: Quantity,
    pub length: Quantity,
    pub measured: Measured,
    pub position: Position,
    pub radius: Option<Quantity>,
    pub relief: Option<Relief>,
    pub setback: Option<Quantity>,
    pub ends: Vec<End>,
}

/// The fields a request may give a base flange or a flange.
pub fn request_fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "base_flange" => &[
            "sketch",
            "loops",
            "thickness",
            "reverse",
            "depth",
            "symmetric",
            "radius",
            "allowance",
            "relief",
        ],
        _ => &[
            "target", "targets", "edges", "sides", "angle", "length", "measured", "position",
            "radius", "relief", "setback", "ends",
        ],
    }
}

impl BaseFlange {
    /// Its fields; no reverse, no symmetric, no loops and no relief are
    /// written as nothing.
    pub fn fields(&self) -> Value {
        let mut v = json!({"sketch": self.sketch, "thickness": self.thickness.text(),
                           "radius": self.radius.text(), "allowance": self.allowance.to_json()});
        if !self.loops.is_empty() {
            v["loops"] = json!(self.loops);
        }
        if self.reverse {
            v["reverse"] = json!(true);
        }
        if let Some(s) = &self.section {
            v["depth"] = json!(s.depth.text());
            if s.symmetric {
                v["symmetric"] = json!(true);
            }
        }
        if self.relief != Relief::None {
            v["relief"] = self.relief.to_json();
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<BaseFlange> {
        let sketch = id_field(o, "sketch")?;
        let loops = match o.take_opt("loops") {
            None => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .enumerate()
                .map(|(i, x)| {
                    let at = format!("/loops/{i}");
                    let s = x
                        .as_str()
                        .ok_or_else(|| invalid(&at, "loops names sketch entities"))?;
                    Id::parse(s, "entity").map_err(|e| e.with("pointer", at))
                })
                .collect::<Result<_>>()?,
            Some(_) => return Err(invalid("/loops", "loops is an array of sketch entities")),
        };
        let thickness = positive(
            &o.take("thickness")?,
            default,
            env,
            "/thickness",
            "a thickness",
        )?;
        let reverse = flag(o, "reverse")?;
        let depth = o.take_opt("depth");
        let symmetric = flag(o, "symmetric")?;
        let section = match depth {
            Some(d) => Some(Section {
                depth: positive(&d, default, env, "/depth", "a bent section's depth")?,
                symmetric,
            }),
            None if symmetric => {
                return Err(invalid(
                    "/symmetric",
                    "symmetric is a bent section's: give its depth",
                ));
            }
            None => None,
        };
        if section.is_some() && !loops.is_empty() {
            return Err(invalid(
                "/loops",
                "a bent section is its sketch's one open chain; loops narrow a plate's closed loops",
            ));
        }
        let radius = positive(
            &o.take("radius")?,
            default,
            env,
            "/radius",
            "an inner bend radius",
        )?;
        let allowance =
            Allowance::from_json(&o.take("allowance")?, "/allowance", default, env, true)?;
        let relief = match o.take_opt("relief") {
            None => Relief::None,
            Some(v) => Relief::from_json(&v, "/relief", default, env)?,
        };
        Ok(BaseFlange {
            sketch,
            loops,
            thickness,
            reverse,
            section,
            radius,
            allowance,
            relief,
        })
    }
}

impl Flange {
    /// Its fields; outer mould, material inside, no setback, no ends and
    /// what the founder gives are written as nothing. An edge with no side
    /// is its reference alone.
    pub fn fields(&self) -> Value {
        let edges: Vec<Value> = self
            .edges
            .iter()
            .map(|e| match &e.side {
                None => e.edge.to_json(),
                Some(s) => json!({"edge": e.edge.to_json(), "side": s.to_json()}),
            })
            .collect();
        let mut v = json!({"target": self.target, "edges": edges, "angle": self.angle.text(),
                           "length": self.length.text()});
        if self.measured != Measured::OuterMould {
            v["measured"] = json!(self.measured.as_str());
        }
        if self.position != Position::MaterialInside {
            v["position"] = json!(self.position.as_str());
        }
        if let Some(r) = &self.radius {
            v["radius"] = json!(r.text());
        }
        if let Some(r) = &self.relief {
            v["relief"] = r.to_json();
        }
        if let Some(s) = &self.setback {
            v["setback"] = json!(s.text());
        }
        if !self.ends.is_empty() {
            v["ends"] = Value::Array(
                self.ends
                    .iter()
                    .map(|e| {
                        let mut x = json!({"vertex": e.vertex.to_json()});
                        if let Some(s) = &e.setback {
                            x["setback"] = json!(s.text());
                        }
                        if let Some(r) = &e.relief {
                            x["relief"] = r.to_json();
                        }
                        x
                    })
                    .collect(),
            );
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Flange> {
        let target = id_field(o, "target")?;
        let edges = o.take("edges")?;
        let edges = edges.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
            invalid(
                "/edges",
                "edges is a non-empty array of edges {feature, name}, or {edge, side}",
            )
        })?;
        let edges = edges
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let at = format!("/edges/{i}");
                if x.get("edge").is_some() {
                    let mut e = Obj::new(x, &at)?;
                    let edge = Reference::from_json(&e.take("edge")?, &format!("{at}/edge"))?;
                    let side = Reference::from_json(&e.take("side")?, &format!("{at}/side"))?;
                    e.finish()?;
                    Ok(FlangeEdge {
                        edge,
                        side: Some(side),
                    })
                } else {
                    Ok(FlangeEdge {
                        edge: Reference::from_json(x, &at)?,
                        side: None,
                    })
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let angle = Quantity::from_json_as(&o.take("angle")?, Field::Angle, &|n| env.get(n))
            .map_err(|e| e.with("pointer", "/angle"))?;
        if !(angle.radians() > 0.0 && angle.radians() <= core::f64::consts::PI) {
            return Err(out_of_range(
                "/angle",
                &format!("a flange's angle is more than 0 and at most 180 deg, not {angle}"),
                &angle,
            ));
        }
        let length = positive(
            &o.take("length")?,
            default,
            env,
            "/length",
            "a flange's length",
        )?;
        let measured = word(
            o,
            "measured",
            Measured::parse,
            "measured is outer_mould, inner_mould or tangent",
        )?
        .unwrap_or_default();
        let position = word(
            o,
            "position",
            Position::parse,
            "position is material_inside, material_outside or bend_outside",
        )?
        .unwrap_or_default();
        let radius = o
            .take_opt("radius")
            .map(|v| positive(&v, default, env, "/radius", "an inner bend radius"))
            .transpose()?;
        let relief = o
            .take_opt("relief")
            .map(|v| Relief::from_json(&v, "/relief", default, env))
            .transpose()?;
        let setback = o
            .take_opt("setback")
            .map(|v| non_negative(&v, default, env, "/setback", "a setback"))
            .transpose()?;
        let ends = match o.take_opt("ends") {
            None => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .enumerate()
                .map(|(i, x)| {
                    let at = format!("/ends/{i}");
                    let mut e = Obj::new(x, &at)?;
                    let vertex = Reference::from_json(&e.take("vertex")?, &format!("{at}/vertex"))?;
                    let setback = e
                        .take_opt("setback")
                        .map(|v| {
                            non_negative(&v, default, env, &format!("{at}/setback"), "a setback")
                        })
                        .transpose()?;
                    let relief = e
                        .take_opt("relief")
                        .map(|v| Relief::from_json(&v, &format!("{at}/relief"), default, env))
                        .transpose()?;
                    e.finish()?;
                    if setback.is_none() && relief.is_none() {
                        return Err(invalid(
                            &at,
                            "an end gives its vertex a setback, a relief or both",
                        ));
                    }
                    Ok(End {
                        vertex,
                        setback,
                        relief,
                    })
                })
                .collect::<Result<_>>()?,
            Some(_) => {
                return Err(invalid(
                    "/ends",
                    "ends is an array of {vertex, setback?, relief?}",
                ));
            }
        };
        let f = Flange {
            target,
            edges,
            angle,
            length,
            measured,
            position,
            radius,
            relief,
            setback,
            ends,
        };
        once(&f.references())?;
        Ok(f)
    }

    /// Its references, each with its role (its JSON pointer with dots):
    /// `edges.<i>` (or `edges.<i>.edge` and `edges.<i>.side` for an edge
    /// given its side), then `ends.<i>.vertex`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = Vec::new();
        for (i, e) in self.edges.iter().enumerate() {
            match &e.side {
                None => out.push((format!("edges.{i}"), &e.edge)),
                Some(s) => {
                    out.push((format!("edges.{i}.edge"), &e.edge));
                    out.push((format!("edges.{i}.side"), s));
                }
            }
        }
        for (i, e) in self.ends.iter().enumerate() {
            out.push((format!("ends.{i}.vertex"), &e.vertex));
        }
        out
    }
}

/// The expression strings of a sheet metal field `k` of a feature's JSON:
/// an allowance's size, a relief's sizes, each end's setback and relief,
/// a setback and a flange's length.
pub fn expression_strings<'a>(k: &str, field: &'a mut Value) -> Vec<&'a mut String> {
    fn relief(v: &mut Value) -> Vec<&mut String> {
        let mut out = Vec::new();
        if let Value::Object(r) = v {
            for (k, x) in r.iter_mut() {
                if let ("width" | "depth", Value::String(s)) = (k.as_str(), x) {
                    out.push(s);
                }
            }
        }
        out
    }
    let mut out = Vec::new();
    match (k, field) {
        ("allowance", Value::Object(a)) => {
            for x in a.values_mut() {
                if let Value::String(s) = x {
                    out.push(s);
                }
            }
        }
        ("relief", r) => out.extend(relief(r)),
        ("ends", Value::Array(ends)) => {
            for e in ends.iter_mut() {
                let Value::Object(e) = e else { continue };
                for (k, x) in e.iter_mut() {
                    match (k.as_str(), x) {
                        ("setback", Value::String(s)) => out.push(s),
                        ("relief", r) => out.extend(relief(r)),
                        _ => {}
                    }
                }
            }
        }
        ("setback" | "length", Value::String(s)) => out.push(s),
        _ => {}
    }
    out
}

/// A field naming a feature by its ID: a sketch, or a solid by its
/// founder.
fn id_field(o: &mut Obj, key: &str) -> Result<Id> {
    let at = format!("/{key}");
    let t = o.take(key)?;
    let t = t
        .as_str()
        .ok_or_else(|| invalid(&at, &format!("{key} is a feature ID")))?;
    Id::parse(t, "feature").map_err(|e| e.with("pointer", at))
}

fn flag(o: &mut Obj, key: &str) -> Result<bool> {
    match o.take_opt(key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(b),
        Some(_) => Err(invalid(
            &format!("/{key}"),
            &format!("{key} is true or false"),
        )),
    }
}

fn word<T>(o: &mut Obj, key: &str, parse: fn(&str) -> Option<T>, what: &str) -> Result<Option<T>> {
    match o.take_opt(key) {
        None => Ok(None),
        Some(Value::String(s)) => parse(&s)
            .map(Some)
            .ok_or_else(|| invalid(&format!("/{key}"), what)),
        Some(_) => Err(invalid(&format!("/{key}"), what)),
    }
}

fn length(v: &Value, default: Unit, env: &Env, at: &str) -> Result<Quantity> {
    Quantity::from_json_as(v, Field::Length(default), &|n| env.get(n))
        .map_err(|e| e.with("pointer", at))
}

/// A length greater than zero.
fn positive(v: &Value, default: Unit, env: &Env, at: &str, what: &str) -> Result<Quantity> {
    let q = length(v, default, env, at)?;
    if q.meters() <= 0.0 {
        return Err(out_of_range(
            at,
            &format!("{what} must be greater than zero, not {q}"),
            &q,
        ));
    }
    Ok(q)
}

/// A length of zero or more.
fn non_negative(v: &Value, default: Unit, env: &Env, at: &str, what: &str) -> Result<Quantity> {
    let q = length(v, default, env, at)?;
    if q.meters() < 0.0 {
        return Err(out_of_range(
            at,
            &format!("{what} can't be negative, not {q}"),
            &q,
        ));
    }
    Ok(q)
}

fn out_of_range(at: &str, msg: &str, q: &Quantity) -> Error {
    Error::new(Code::InvalidParam, msg)
        .with("pointer", at)
        .with("value", q.text())
}

/// Refuses an edge or end vertex named twice; the pointer is the second
/// naming's.
fn once(refs: &[(String, &Reference)]) -> Result<()> {
    for (k, (role, r)) in refs.iter().enumerate() {
        if role.ends_with(".side") {
            continue;
        }
        if let Some((first, _)) = refs[..k]
            .iter()
            .find(|(f, x)| !f.ends_with(".side") && x == r)
        {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{}/{} is named twice, as {first} and {role}: a flange names each edge and each end once", r.feature, r.name),
            )
            .with("pointer", format!("/{}", role.replace('.', "/")))
            .with("first", format!("/{}", first.replace('.', "/"))));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";
    const C: &str = "00112233445566778899aabbccddeeff";

    fn r(n: &str) -> Value {
        json!({"feature": "plate", "name": n})
    }

    fn base(v: Value) -> Result<BaseFlange> {
        let mut o = Obj::new(&v, "")?;
        let b = BaseFlange::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(b)
    }

    fn flange(v: Value) -> Result<Flange> {
        let mut o = Obj::new(&v, "")?;
        let f = Flange::from_fields(&mut o, Unit::Mm, &Env::empty())?;
        o.finish()?;
        Ok(f)
    }

    #[test]
    fn base_flanges_round_trip() {
        // A plate: bare numbers take the default unit, written out; no
        // relief, no reverse, no loops written as nothing.
        let b = base(json!({"sketch": "s", "thickness": 1.5, "radius": 1,
                            "allowance": {"k_factor": 0.44}}))
        .unwrap();
        assert!(b.section.is_none() && b.relief == Relief::None);
        assert_eq!(
            b.fields(),
            json!({"sketch": "s", "thickness": "1.5 mm", "radius": "1 mm",
                   "allowance": {"k_factor": "0.44"}})
        );
        assert_eq!(base(b.fields()).unwrap(), b);
        // A bent section, symmetric, with a deduction and a relief.
        let v = json!({"sketch": "s", "thickness": "2 mm", "radius": "1 mm", "depth": "40 mm",
                       "symmetric": true, "reverse": true, "allowance": {"deduction": "3 mm"},
                       "relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}});
        let b = base(v.clone()).unwrap();
        assert_eq!(b.section.as_ref().unwrap().depth.meters(), 0.04);
        assert_eq!(b.fields(), v);
        // A relief of none is written as nothing.
        let b = base(json!({"sketch": "s", "thickness": 1, "radius": 1,
                            "allowance": {"k_factor": "0.5"}, "relief": {"kind": "none"}}))
        .unwrap();
        assert!(b.fields().get("relief").is_none());
    }

    #[test]
    fn flanges_round_trip() {
        let v = json!({"target": "plate", "edges": [r(A)], "angle": 90, "length": 20});
        let f = flange(v).unwrap();
        assert_eq!(
            (f.measured, f.position),
            (Measured::OuterMould, Position::MaterialInside)
        );
        assert!(f.radius.is_none() && f.relief.is_none() && f.setback.is_none());
        assert_eq!(
            f.fields(),
            json!({"target": "plate", "edges": [r(A)], "angle": "90 deg", "length": "20 mm"})
        );
        let v = json!({"target": "plate", "edges": [{"edge": r(A), "side": r(B)}, r(B)],
                       "angle": "45 deg", "length": "10 mm", "measured": "tangent",
                       "position": "bend_outside", "radius": "2 mm", "setback": "1 mm",
                       "relief": {"kind": "tear", "width": "0.5 mm"},
                       "ends": [{"vertex": r(C), "relief": {"kind": "none"}}]});
        let f = flange(v.clone()).unwrap();
        assert_eq!(f.fields(), v);
        assert_eq!(flange(f.fields()).unwrap(), f);
        let roles: Vec<String> = f.references().into_iter().map(|(r, _)| r).collect();
        assert_eq!(
            roles,
            ["edges.0.edge", "edges.0.side", "edges.1", "ends.0.vertex"]
        );
    }

    #[test]
    fn sheet_metal_files_are_checked() {
        let plate =
            || json!({"sketch": "s", "thickness": 1, "radius": 1, "allowance": {"k_factor": 0.5}});
        for (edit, code, pointer) in [
            (json!({"thickness": 0}), Code::InvalidParam, "/thickness"),
            (json!({"radius": -1}), Code::InvalidParam, "/radius"),
            (
                json!({"allowance": {"k_factor": 1.5}}),
                Code::InvalidParam,
                "/allowance/k_factor",
            ),
            (
                json!({"allowance": {"allowance": "2 mm"}}),
                Code::InvalidDocument,
                "/allowance",
            ),
            (
                json!({"allowance": {"k_factor": 0.5, "deduction": 1}}),
                Code::InvalidDocument,
                "/allowance",
            ),
            (
                json!({"symmetric": true}),
                Code::InvalidDocument,
                "/symmetric",
            ),
            (
                json!({"depth": 40, "loops": ["l1"]}),
                Code::InvalidDocument,
                "/loops",
            ),
            (
                json!({"relief": {"kind": "square"}}),
                Code::InvalidDocument,
                "/relief/kind",
            ),
            (
                json!({"relief": {"kind": "tear"}}),
                Code::InvalidDocument,
                "/relief/width",
            ),
            (
                json!({"relief": {"kind": "tear", "width": 0}}),
                Code::InvalidParam,
                "/relief/width",
            ),
            (
                json!({"thickness": "1 deg"}),
                Code::UnitMismatch,
                "/thickness",
            ),
        ] {
            let mut v = plate();
            for (k, x) in edit.as_object().unwrap() {
                v[k] = x.clone();
            }
            let e = base(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
        let one = || json!({"target": "plate", "edges": [r(A)], "angle": 90, "length": 20});
        for (edit, code, pointer) in [
            (json!({"edges": []}), Code::InvalidDocument, "/edges"),
            (
                json!({"edges": [r(A), r(A)]}),
                Code::InvalidParam,
                "/edges/1",
            ),
            (json!({"angle": 0}), Code::InvalidParam, "/angle"),
            (json!({"angle": "190 deg"}), Code::InvalidParam, "/angle"),
            (json!({"length": 0}), Code::InvalidParam, "/length"),
            (
                json!({"measured": "middle"}),
                Code::InvalidDocument,
                "/measured",
            ),
            (
                json!({"position": "inside"}),
                Code::InvalidDocument,
                "/position",
            ),
            (json!({"setback": -1}), Code::InvalidParam, "/setback"),
            (
                json!({"ends": [{"vertex": r(B)}]}),
                Code::InvalidDocument,
                "/ends/0",
            ),
            (
                json!({"ends": [{"vertex": r(B), "setback": 1}, {"vertex": r(B), "setback": 2}]}),
                Code::InvalidParam,
                "/ends/1/vertex",
            ),
            (json!({"target": "Plate"}), Code::InvalidId, "/target"),
        ] {
            let mut v = one();
            for (k, x) in edit.as_object().unwrap() {
                v[k] = x.clone();
            }
            let e = flange(v.clone()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
    }

    #[test]
    fn sizes_read_parameters() {
        let p = [
            ("t".to_owned(), crate::expr::Expr::parse("1.5 mm").unwrap()),
            ("k".to_owned(), crate::expr::Expr::parse("0.44").unwrap()),
        ]
        .into();
        let v = json!({"sketch": "s", "thickness": "t", "radius": "2 * t",
                       "allowance": {"k_factor": "k"}});
        let mut o = Obj::new(&v, "").unwrap();
        let b = BaseFlange::from_fields(&mut o, Unit::Mm, &Env::new(&p)).unwrap();
        assert_eq!(b.radius.meters(), 0.003);
        assert_eq!(b.radius.text(), "2 * t");
        let k = match &b.allowance {
            Allowance::KFactor(k) => k.number(),
            _ => -1.0,
        };
        assert_eq!(k, 0.44);
    }
}
