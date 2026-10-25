//! Pattern features: earlier joins, cuts and holes (the seeds) applied
//! again, moved along one or two directions, turned about an axis, or
//! reflected in a plane, to the solids the seeds act on. Instances are
//! numbered from the seed (0); any but the seed may be skipped. Which of
//! the three a pattern is, is its `form` (a feature file's `kind` is its
//! header).

use super::files::{Obj, invalid};
use super::id::Id;
use super::reference::Reference;
use super::sketch::OriginPlane;
use super::solid::targets_from_json;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// The most instances a pattern may have, seeds' included.
pub const MAX_INSTANCES: f64 = 100_000.0;

/// A pattern of its seeds.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub kind: Kind,
    /// The joins, cuts and holes it repeats, in order.
    pub features: Vec<Id>,
    /// Instances left out, by index.
    pub skip: Vec<Instance>,
    /// The solids it acts on, by their founders: its seeds'.
    pub targets: Vec<Id>,
}

/// An instance's index: along the first direction (or round the axis),
/// and along the second; `[0, 0]` is the seed.
pub type Instance = [u32; 2];

/// How the instances are placed.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Along `first`, and along `second` for each of those.
    Linear { first: Row, second: Option<Row> },
    /// Turned about `axis`: `count` in all, evenly round a whole turn, or
    /// from first to last over a smaller `angle`.
    Circular {
        axis: Direction,
        count: Quantity,
        angle: Quantity,
    },
    /// The mirror image in `plane`: one instance, `[1, 0]`.
    Mirror { plane: MirrorPlane },
}

/// One direction of a linear pattern: `count` in all, `spacing` apart.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub direction: Direction,
    pub spacing: Quantity,
    pub count: Quantity,
    /// Against the direction.
    pub reverse: bool,
}

/// A direction or an axis: one of the part's origin axes, or what an
/// entity gives (a straight edge, a planar face's normal, a circular
/// edge's or a cylindrical or conical face's axis).
#[derive(Clone, Debug, PartialEq)]
pub enum Direction {
    Origin(OriginAxis),
    Entity(Reference),
}

/// An axis of the part's origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginAxis {
    X,
    Y,
    Z,
}

/// The plane a mirror reflects in: an origin plane or a planar face.
#[derive(Clone, Debug, PartialEq)]
pub enum MirrorPlane {
    Origin(OriginPlane),
    Face(Reference),
}

impl OriginAxis {
    pub const ALL: [OriginAxis; 3] = [OriginAxis::X, OriginAxis::Y, OriginAxis::Z];

    pub fn as_str(self) -> &'static str {
        match self {
            OriginAxis::X => "x",
            OriginAxis::Y => "y",
            OriginAxis::Z => "z",
        }
    }

    pub fn parse(s: &str) -> Option<OriginAxis> {
        OriginAxis::ALL.into_iter().find(|a| a.as_str() == s)
    }

    /// Its unit direction.
    pub fn vector(self) -> [f64; 3] {
        match self {
            OriginAxis::X => [1.0, 0.0, 0.0],
            OriginAxis::Y => [0.0, 1.0, 0.0],
            OriginAxis::Z => [0.0, 0.0, 1.0],
        }
    }
}

impl Direction {
    pub fn to_json(&self) -> Value {
        match self {
            Direction::Origin(a) => json!(a.as_str()),
            Direction::Entity(r) => r.to_json(),
        }
    }

    fn from_json(v: &Value, at: &str) -> Result<Direction> {
        match v {
            Value::String(s) => OriginAxis::parse(s).map(Direction::Origin).ok_or_else(|| {
                invalid(at, "an origin axis is x, y or z; else give {feature, name}")
            }),
            v => Ok(Direction::Entity(Reference::from_json(v, at)?)),
        }
    }

    fn reference(&self) -> Option<&Reference> {
        match self {
            Direction::Entity(r) => Some(r),
            Direction::Origin(_) => None,
        }
    }
}

impl MirrorPlane {
    pub fn to_json(&self) -> Value {
        match self {
            MirrorPlane::Origin(p) => json!(p.as_str()),
            MirrorPlane::Face(r) => r.to_json(),
        }
    }

    fn from_json(v: &Value) -> Result<MirrorPlane> {
        match v {
            Value::String(s) => OriginPlane::parse(s)
                .map(MirrorPlane::Origin)
                .map_err(|e| e.with("pointer", "/plane")),
            v => Ok(MirrorPlane::Face(Reference::from_json(v, "/plane")?)),
        }
    }
}

impl Kind {
    pub const NAMES: [&str; 3] = ["linear", "circular", "mirror"];

    pub fn type_name(&self) -> &'static str {
        match self {
            Kind::Linear { .. } => "linear",
            Kind::Circular { .. } => "circular",
            Kind::Mirror { .. } => "mirror",
        }
    }

    /// How many instances along each way, the seed's included: `[n, 1]`
    /// for one direction or round an axis, `[2, 1]` for a mirror.
    pub fn counts(&self) -> [u32; 2] {
        let n = |q: &Quantity| q.number() as u32;
        match self {
            Kind::Linear { first, second } => {
                [n(&first.count), second.as_ref().map_or(1, |r| n(&r.count))]
            }
            Kind::Circular { count, .. } => [n(count), 1],
            Kind::Mirror { .. } => [2, 1],
        }
    }
}

fn bad(at: &str, msg: &str, q: &Quantity) -> Error {
    Error::new(Code::InvalidParam, format!("{msg}, not {q}"))
        .with("pointer", at)
        .with("value", q.text())
}

/// A count: a whole number, at least one.
pub(crate) fn count(v: &Value, env: &Env, at: &str) -> Result<Quantity> {
    let q = Quantity::from_json_as(v, Field::Number, &|n| env.get(n))
        .map_err(|e| e.with("pointer", at))?;
    let n = q.number();
    if n.fract() != 0.0 || n < 1.0 || n > MAX_INSTANCES {
        return Err(bad(
            at,
            &format!("a count is a whole number from 1 to {MAX_INSTANCES}"),
            &q,
        ));
    }
    Ok(q)
}

/// A spacing: a length greater than zero.
pub(crate) fn spacing(v: &Value, default: Unit, env: &Env, at: &str) -> Result<Quantity> {
    let q =
        Quantity::from_json_in(v, default, &|n| env.get(n)).map_err(|e| e.with("pointer", at))?;
    if q.meters() <= 0.0 {
        return Err(bad(at, "a spacing must be greater than zero", &q));
    }
    Ok(q)
}

/// Reads an optional `true`/`false` field, false when left out.
pub(crate) fn flag(o: &mut Obj, key: &str) -> Result<bool> {
    match o.take_opt(key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(b),
        Some(_) => Err(invalid(
            &format!("/{key}"),
            &format!("{key} is true or false"),
        )),
    }
}

impl Row {
    /// Writes a row's fields, `suffix` `""` or `"2"`.
    fn write(&self, v: &mut Value, suffix: &str) {
        v[format!("direction{suffix}")] = self.direction.to_json();
        v[format!("spacing{suffix}")] = json!(self.spacing.text());
        v[format!("count{suffix}")] = json!(self.count.text());
        if self.reverse {
            v[format!("reverse{suffix}")] = json!(true);
        }
    }

    fn read(o: &mut Obj, suffix: &str, default: Unit, env: &Env) -> Result<Row> {
        let at = |k: &str| format!("/{k}{suffix}");
        Ok(Row {
            direction: Direction::from_json(
                &o.take(&format!("direction{suffix}"))?,
                &at("direction"),
            )?,
            spacing: spacing(
                &o.take(&format!("spacing{suffix}"))?,
                default,
                env,
                &at("spacing"),
            )?,
            count: count(&o.take(&format!("count{suffix}"))?, env, &at("count"))?,
            reverse: flag(o, &format!("reverse{suffix}"))?,
        })
    }
}

impl Pattern {
    pub fn fields(&self) -> Value {
        let mut v = json!({
            "form": self.kind.type_name(),
            "features": self.features,
            "targets": self.targets,
        });
        match &self.kind {
            Kind::Linear { first, second } => {
                first.write(&mut v, "");
                if let Some(r) = second {
                    r.write(&mut v, "2");
                }
            }
            Kind::Circular { axis, count, angle } => {
                v["axis"] = axis.to_json();
                v["count"] = json!(count.text());
                v["angle"] = json!(angle.text());
            }
            Kind::Mirror { plane } => v["plane"] = plane.to_json(),
        }
        if !self.skip.is_empty() {
            let two = matches!(
                &self.kind,
                Kind::Linear {
                    second: Some(_),
                    ..
                }
            );
            let skip: Vec<Value> = self
                .skip
                .iter()
                .map(|[i, j]| if two { json!([i, j]) } else { json!(i) })
                .collect();
            v["skip"] = json!(skip);
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Pattern> {
        let kind = o.take("form")?;
        let kind = match kind.as_str() {
            Some("linear") => {
                let first = Row::read(o, "", default, env)?;
                let second = if ["direction2", "spacing2", "count2", "reverse2"]
                    .iter()
                    .any(|k| o.has(k))
                {
                    Some(Row::read(o, "2", default, env)?)
                } else {
                    None
                };
                Kind::Linear { first, second }
            }
            Some("circular") => {
                let axis = Direction::from_json(&o.take("axis")?, "/axis")?;
                let count = count(&o.take("count")?, env, "/count")?;
                let angle =
                    Quantity::from_json_as(&o.take("angle")?, Field::Angle, &|n| env.get(n))
                        .map_err(|e| e.with("pointer", "/angle"))?;
                let a = angle.radians();
                if a <= 0.0 || a > core::f64::consts::TAU + ryukei::tol::ANGULAR_RESOLUTION {
                    return Err(bad(
                        "/angle",
                        "a circular pattern's angle is in (0, 360 deg]",
                        &angle,
                    ));
                }
                Kind::Circular { axis, count, angle }
            }
            Some("mirror") => Kind::Mirror {
                plane: MirrorPlane::from_json(&o.take("plane")?)?,
            },
            _ => {
                return Err(invalid(
                    "/form",
                    "a pattern's form is linear, circular or mirror",
                ));
            }
        };
        let [n, m] = kind.counts();
        if f64::from(n) * f64::from(m) > MAX_INSTANCES {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "a pattern has at most {MAX_INSTANCES} instances, not {}",
                    u64::from(n) * u64::from(m)
                ),
            )
            .with("pointer", "/count2"));
        }
        let features = targets_from_json(&o.take("features")?, "/features")?;
        let skip = match o.take_opt("skip") {
            None => Vec::new(),
            // A mirror has nothing to skip.
            Some(_) if matches!(kind, Kind::Mirror { .. }) => {
                return Err(invalid(
                    "/skip",
                    "a mirror has one instance, and nothing to skip",
                ));
            }
            Some(v) => {
                let two = matches!(
                    kind,
                    Kind::Linear {
                        second: Some(_),
                        ..
                    }
                );
                skip_from_json(&v, kind.counts(), two)?
            }
        };
        let targets = targets_from_json(&o.take("targets")?, "/targets")?;
        Ok(Pattern {
            kind,
            features,
            skip,
            targets,
        })
    }

    /// Every instance but the seed and those skipped, in order: along the
    /// first direction, then each further step along the second.
    pub fn instances(&self) -> Vec<Instance> {
        let [n, m] = self.kind.counts();
        (0..m)
            .flat_map(|j| (0..n).map(move |i| [i, j]))
            .filter(|x| *x != [0, 0] && !self.skip.contains(x))
            .collect()
    }

    /// Its references, each with its role: `direction`, `direction2`,
    /// `axis`, `plane`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = Vec::new();
        match &self.kind {
            Kind::Linear { first, second } => {
                if let Some(r) = first.direction.reference() {
                    out.push(("direction".to_owned(), r));
                }
                if let Some(r) = second.as_ref().and_then(|s| s.direction.reference()) {
                    out.push(("direction2".to_owned(), r));
                }
            }
            Kind::Circular { axis, .. } => {
                if let Some(r) = axis.reference() {
                    out.push(("axis".to_owned(), r));
                }
            }
            Kind::Mirror { plane } => {
                if let MirrorPlane::Face(r) = plane {
                    out.push(("plane".to_owned(), r));
                }
            }
        }
        out
    }
}

/// Reads `skip`: instance indices, each `i` (one way) or `[i, j]` (`two`
/// directions), within `counts`, each once, never the seed's.
pub(crate) fn skip_from_json(v: &Value, counts: [u32; 2], two: bool) -> Result<Vec<Instance>> {
    let a = v
        .as_array()
        .ok_or_else(|| invalid("/skip", "skip is an array of instance indices"))?;
    let [n, m] = counts;
    let index = |x: &Value| x.as_u64().and_then(|i| u32::try_from(i).ok());
    let mut out: Vec<Instance> = Vec::with_capacity(a.len());
    for (k, x) in a.iter().enumerate() {
        let at = format!("/skip/{k}");
        let i = match (x, two) {
            (Value::Array(p), true) if p.len() == 2 => {
                index(&p[0]).zip(index(&p[1])).map(|(i, j)| [i, j])
            }
            (x, false) => index(x).map(|i| [i, 0]),
            _ => None,
        };
        let shape = if two { "[i, j]" } else { "a whole number" };
        let i = i.ok_or_else(|| invalid(&at, &format!("an instance to skip is {shape}")))?;
        if i == [0, 0] {
            return Err(invalid(
                &at,
                "instance 0 is the seed, which can't be skipped",
            ));
        }
        if i[0] >= n || i[1] >= m {
            return Err(Error::new(
                Code::InvalidParam,
                format!("the pattern has no instance {i:?}: it counts {n} by {m}"),
            )
            .with("pointer", &at));
        }
        if out.contains(&i) {
            return Err(invalid(&at, &format!("instance {i:?} is skipped twice")));
        }
        out.push(i);
    }
    Ok(out)
}

/// The expression strings of a pattern: spacings and counts (its angle is
/// any feature's `angle`).
pub fn expression_strings<'a>(k: &str, field: &'a mut Value) -> Option<&'a mut String> {
    match (k, field) {
        ("spacing" | "spacing2" | "count" | "count2", Value::String(s)) => Some(s),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::canonical;
    use crate::doc::files::Feature;

    const N: &str = "0123456789abcdef0123456789abcdef";

    fn read(v: &Value) -> Result<Feature> {
        let p = [("n".to_owned(), crate::expr::Expr::parse("3").unwrap())].into();
        Feature::from_json(v, Unit::Mm, &Env::new(&p))
    }

    fn with(v: Value) -> Value {
        let mut v = v;
        v["format"] = json!(3);
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!("pattern");
        v["features"] = json!(["hole1"]);
        v["targets"] = json!(["b"]);
        v
    }

    fn linear() -> Value {
        let mut v = with(json!({}));
        v["form"] = json!("linear");
        v["direction"] = json!("x");
        v["spacing"] = json!("10 mm");
        v["count"] = json!("n + 1");
        v["direction2"] = json!({"feature": "b", "name": N});
        v["spacing2"] = json!(5);
        v["count2"] = json!(2);
        v["reverse2"] = json!(true);
        v["skip"] = json!([[1, 1], [3, 0]]);
        v
    }

    fn pattern(f: &Feature) -> &Pattern {
        match f {
            Feature::Pattern(p) => p,
            _ => panic!("a pattern"),
        }
    }

    #[test]
    fn patterns_round_trip() {
        let v = linear();
        let f = read(&v).unwrap();
        let p = pattern(&f);
        assert_eq!(p.kind.counts(), [4, 2]);
        assert_eq!(p.skip, [[1, 1], [3, 0]]);
        assert_eq!(p.instances(), [[1, 0], [2, 0], [0, 1], [2, 1], [3, 1]]);
        assert_eq!(f.format(), 3);
        assert_eq!(f.targets(), [&Id::parse("b", "feature").unwrap()]);
        assert_eq!(
            f.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["direction2"]
        );
        let text = canonical::to_string(&f.to_json());
        assert_eq!(read(&canonical::parse(&text).unwrap()).unwrap(), f);
        let w = f.to_json();
        assert_eq!(
            (&w["count"], &w["spacing2"], &w["count2"]),
            (&json!("n + 1"), &json!("5 mm"), &json!("2"))
        );
        assert!(w.get("reverse").is_none());
        // One direction: skips are whole numbers.
        let mut v = with(json!({"direction": "z", "spacing": 2, "count": 3, "skip": [2]}));
        v["form"] = json!("linear");
        let f = read(&v).unwrap();
        assert_eq!(pattern(&f).instances(), [[1, 0]]);
        assert_eq!(f.to_json()["skip"], json!([2]));
        // Round an axis, and in a plane.
        let mut v = with(json!({"axis": {"feature": "b", "name": N}, "count": 6,
                                "angle": 360}));
        v["form"] = json!("circular");
        let f = read(&v).unwrap();
        assert_eq!(pattern(&f).kind.counts(), [6, 1]);
        assert_eq!(f.to_json()["angle"], "360 deg");
        assert_eq!(f.references()[0].0, "axis");
        assert_eq!(read(&f.to_json()).unwrap(), f);
        let mut v = with(json!({"plane": "yz"}));
        v["form"] = json!("mirror");
        let f = read(&v).unwrap();
        assert_eq!(pattern(&f).instances(), [[1, 0]]);
        assert!(f.references().is_empty());
        assert_eq!(read(&f.to_json()).unwrap(), f);
    }

    #[test]
    fn patterns_are_checked() {
        let cases: [(&str, Value, Code, &str); 12] = [
            ("/form", json!("spiral"), Code::InvalidDocument, "/form"),
            (
                "/direction",
                json!("w"),
                Code::InvalidDocument,
                "/direction",
            ),
            ("/spacing", json!("0 mm"), Code::InvalidParam, "/spacing"),
            ("/count", json!("2.5"), Code::InvalidParam, "/count"),
            ("/count", json!("0"), Code::InvalidParam, "/count"),
            ("/count", json!("3 mm"), Code::UnitMismatch, "/count"),
            ("/count2", json!(100000), Code::InvalidParam, "/count2"),
            ("/skip", json!([[0, 0]]), Code::InvalidDocument, "/skip/0"),
            ("/skip", json!([[4, 0]]), Code::InvalidParam, "/skip/0"),
            ("/skip", json!([2]), Code::InvalidDocument, "/skip/0"),
            (
                "/skip",
                json!([[1, 1], [1, 1]]),
                Code::InvalidDocument,
                "/skip/1",
            ),
            ("/features", json!([]), Code::InvalidDocument, "/features"),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = linear();
            v[&path[1..]] = value;
            let e = read(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{path}: {e}"
            );
        }
        let mut v = with(json!({"axis": "z", "count": 6, "angle": "400 deg"}));
        v["form"] = json!("circular");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/angle");
        let mut v = with(json!({"plane": "xz"}));
        v["form"] = json!("mirror");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/plane");
        let mut v = with(json!({"plane": "xy", "skip": [1]}));
        v["form"] = json!("mirror");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/skip");
    }

    #[test]
    fn sizes_read_parameters() {
        let f = read(&linear()).unwrap();
        let names = crate::doc::files::feature_names(&f.to_json());
        assert_eq!(names.into_iter().collect::<Vec<_>>(), ["n"]);
        let mut w = f.to_json();
        assert!(crate::doc::files::rename_in_feature(&mut w, "n", "rows"));
        assert_eq!(w["count"], "rows + 1");
    }
}
