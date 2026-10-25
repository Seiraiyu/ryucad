//! An assembly's mates and instance patterns, and the connectors they
//! read: frames defined from named entities of the parts its instances
//! place.
//!
//! A connector is `{"instance": [path], "at": {feature, name} | "origin",
//! "offset": {…}}`: the instance path from this assembly down through
//! subassemblies (a pattern's copy written `<pattern>:<i>`, or
//! `<pattern>:<i>:<j>` past its first row, then the seed it copies), a
//! face, edge or vertex of a solid of the part at the end of the path, by
//! its founder and stable name, or the part's own frame, and an offset in
//! the frame's own axes. Which of the six kinds a mate is, and which of
//! the two an instance pattern is, is its `form` (a feature file's `kind`
//! is its header).

use super::files::{Obj, invalid};
use super::id::Id;
use super::pattern::{OriginAxis, count, flag, skip_from_json, spacing};
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// A frame on an occurrence: where to find it, and how it is moved.
#[derive(Clone, Debug, PartialEq)]
pub struct Connector {
    /// The instance path's first step: an instance of this assembly, or a
    /// pattern whose copy `copy` it names.
    pub root: Id,
    /// For a pattern's copy, its index (never `[0, 0]`, the seed).
    pub copy: Option<[u32; 2]>,
    /// The rest of the path: for a copy, the seed first; then instances
    /// of subassemblies, down to the part the frame is on.
    pub rest: Vec<Id>,
    pub at: At,
    pub offset: Offset,
}

/// What a connector's frame is defined from.
#[derive(Clone, Debug, PartialEq)]
pub enum At {
    /// The part's own origin frame.
    Origin,
    /// A face, edge or vertex of a solid of the part, by founder and name.
    Entity(Reference),
}

/// A connector's frame moved in its own axes by `origin`, then turned
/// about its z by `rotate`, then turned over about its x if `flip`.
#[derive(Clone, Debug, PartialEq)]
pub struct Offset {
    pub origin: Option<[Quantity; 3]>,
    pub rotate: Option<Quantity>,
    pub flip: bool,
}

impl Offset {
    pub const NONE: Offset = Offset {
        origin: None,
        rotate: None,
        flip: false,
    };

    pub fn is_none(&self) -> bool {
        *self == Offset::NONE
    }

    fn to_json(&self) -> Value {
        let mut v = json!({});
        if let Some(o) = &self.origin {
            v["origin"] = json!(o.iter().map(Quantity::text).collect::<Vec<_>>());
        }
        if let Some(r) = &self.rotate {
            v["rotate"] = json!(r.text());
        }
        if self.flip {
            v["flip"] = json!(true);
        }
        v
    }

    fn from_json(v: &Value, at: &str, default: Unit, env: &Env) -> Result<Offset> {
        let mut o = Obj::new(v, at)?;
        let lookup = |n: &str| env.get(n);
        let origin = match o.take_opt("origin") {
            None => None,
            Some(a) => {
                let here = format!("{at}/origin");
                let a = a
                    .as_array()
                    .filter(|a| a.len() == 3)
                    .ok_or_else(|| invalid(&here, "an offset's origin is three lengths"))?;
                let mut out = Vec::with_capacity(3);
                for (i, x) in a.iter().enumerate() {
                    out.push(
                        Quantity::from_json_in(x, default, &lookup)
                            .map_err(|e| e.with("pointer", format!("{here}/{i}")))?,
                    );
                }
                let [x, y, z]: [Quantity; 3] = out.try_into().expect("three");
                Some([x, y, z])
            }
        };
        let rotate = match o.take_opt("rotate") {
            None => None,
            Some(r) => Some(
                Quantity::from_json_as(&r, Field::Angle, &lookup)
                    .map_err(|e| e.with("pointer", format!("{at}/rotate")))?,
            ),
        };
        let flip = match o.take_opt("flip") {
            None => false,
            Some(Value::Bool(b)) => b,
            Some(_) => return Err(invalid(&format!("{at}/flip"), "flip is true or false")),
        };
        o.finish()?;
        Ok(Offset {
            origin,
            rotate,
            flip,
        })
    }
}

/// One step of an instance path as text: an instance's ID, or a copy
/// `<pattern>:<i>` (`<pattern>:<i>:<j>` past the first row).
pub fn step_text(id: &Id, copy: Option<[u32; 2]>) -> String {
    match copy {
        None => id.to_string(),
        Some([i, 0]) => format!("{id}:{i}"),
        Some([i, j]) => format!("{id}:{i}:{j}"),
    }
}

/// Reads one step of an instance path: an ID, or a copy's.
pub fn parse_step(s: &str, at: &str) -> Result<(Id, Option<[u32; 2]>)> {
    let mut parts = s.split(':');
    let head = parts.next().unwrap_or_default();
    let id = Id::parse(head, "instance").map_err(|e| e.with("pointer", at))?;
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        return Ok((id, None));
    }
    let index = |t: &str| t.parse::<u32>().ok().filter(|_| !t.starts_with('+'));
    let copy = match rest.as_slice() {
        [i] => index(i).map(|i| [i, 0]),
        [i, j] => index(i).zip(index(j)).map(|(i, j)| [i, j]),
        _ => None,
    };
    match copy {
        Some([0, 0]) => Err(invalid(
            at,
            "copy 0 of a pattern is its seed: name the seed",
        )),
        Some(c) if step_text(&id, Some(c)) == s => Ok((id, Some(c))),
        _ => Err(invalid(
            at,
            "a pattern's copy is <pattern>:<i>, or <pattern>:<i>:<j> past its first row",
        )),
    }
}

impl Connector {
    /// The instance path as a file writes it.
    pub fn path(&self) -> Vec<String> {
        std::iter::once(step_text(&self.root, self.copy))
            .chain(self.rest.iter().map(Id::to_string))
            .collect()
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "instance": self.path(),
            "at": match &self.at {
                At::Origin => json!("origin"),
                At::Entity(r) => r.to_json(),
            },
        });
        if !self.offset.is_none() {
            v["offset"] = self.offset.to_json();
        }
        v
    }

    pub fn from_json(v: &Value, at: &str, default: Unit, env: &Env) -> Result<Connector> {
        let mut o = Obj::new(v, at)?;
        let path = o.take("instance")?;
        let here = format!("{at}/instance");
        let path = path
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid(&here, "an instance path is a list of instance IDs"))?;
        let mut steps = Vec::with_capacity(path.len());
        for (i, s) in path.iter().enumerate() {
            let where_ = format!("{here}/{i}");
            let s = s
                .as_str()
                .ok_or_else(|| invalid(&where_, "an instance path's steps are strings"))?;
            steps.push(parse_step(s, &where_)?);
        }
        let (root, copy) = steps[0].clone();
        let mut rest = Vec::with_capacity(steps.len() - 1);
        for (i, (id, c)) in steps.into_iter().enumerate().skip(1) {
            if c.is_some() {
                return Err(invalid(
                    &format!("{here}/{i}"),
                    "only a path's first step may be a pattern's copy",
                ));
            }
            rest.push(id);
        }
        if copy.is_some() && rest.is_empty() {
            return Err(invalid(
                &here,
                "a pattern's copy is followed by the seed it copies",
            ));
        }
        let at_v = o.take("at")?;
        let at_ = match &at_v {
            Value::String(s) if s == "origin" => At::Origin,
            v => At::Entity(Reference::from_json(v, &format!("{at}/at"))?),
        };
        let offset = match o.take_opt("offset") {
            None => Offset::NONE,
            Some(x) => Offset::from_json(&x, &format!("{at}/offset"), default, env)?,
        };
        o.finish()?;
        Ok(Connector {
            root,
            copy,
            rest,
            at: at_,
            offset,
        })
    }

    /// The text form, as requests and reports give it: the path joined by
    /// `/`, then `origin` or the solid and name, `<path>/<solid>/<name>`.
    pub fn text(&self) -> String {
        let at = match &self.at {
            At::Origin => "origin".to_owned(),
            At::Entity(r) => format!("{}/{}", r.feature, r.name),
        };
        format!("{}/{at}", self.path().join("/"))
    }
}

/// Which of the six kinds a mate is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Form {
    Fastened,
    Revolute,
    Slider,
    Cylindrical,
    Planar,
    Ball,
}

impl Form {
    pub const ALL: [Form; 6] = [
        Form::Fastened,
        Form::Revolute,
        Form::Slider,
        Form::Cylindrical,
        Form::Planar,
        Form::Ball,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Form::Fastened => "fastened",
            Form::Revolute => "revolute",
            Form::Slider => "slider",
            Form::Cylindrical => "cylindrical",
            Form::Planar => "planar",
            Form::Ball => "ball",
        }
    }

    pub fn parse(s: &str) -> Option<Form> {
        Form::ALL.into_iter().find(|f| f.as_str() == s)
    }

    /// Whether `angle` may drive it: the turn about z it leaves free.
    pub fn takes_angle(self) -> bool {
        matches!(self, Form::Revolute | Form::Cylindrical | Form::Planar)
    }

    /// Whether `distance` may drive it: along z.
    pub fn takes_distance(self) -> bool {
        matches!(self, Form::Slider | Form::Cylindrical | Form::Planar)
    }
}

/// A `mate`: connector `b`'s frame follows connector `a`'s as its form
/// says, `angle` and `distance` driving what it leaves free.
#[derive(Clone, Debug, PartialEq)]
pub struct Mate {
    pub form: Form,
    pub a: Connector,
    pub b: Connector,
    pub angle: Option<Quantity>,
    pub distance: Option<Quantity>,
}

impl Mate {
    pub fn fields(&self) -> Value {
        let mut v = json!({"form": self.form.as_str(), "a": self.a.to_json(),
                           "b": self.b.to_json()});
        if let Some(a) = &self.angle {
            v["angle"] = json!(a.text());
        }
        if let Some(d) = &self.distance {
            v["distance"] = json!(d.text());
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Mate> {
        let form = o.take("form")?;
        let form = form.as_str().and_then(Form::parse).ok_or_else(|| {
            invalid(
                "/form",
                "a mate's form is fastened, revolute, slider, cylindrical, planar or ball",
            )
        })?;
        let a = Connector::from_json(&o.take("a")?, "/a", default, env)?;
        let b = Connector::from_json(&o.take("b")?, "/b", default, env)?;
        let lookup = |n: &str| env.get(n);
        let angle = match o.take_opt("angle") {
            None => None,
            Some(_) if !form.takes_angle() => {
                return Err(invalid(
                    "/angle",
                    &format!("a {} mate leaves no turn to drive", form.as_str()),
                ));
            }
            Some(x) => Some(
                Quantity::from_json_as(&x, Field::Angle, &lookup)
                    .map_err(|e| e.with("pointer", "/angle"))?,
            ),
        };
        let distance = match o.take_opt("distance") {
            None => None,
            Some(_) if !form.takes_distance() => {
                return Err(invalid(
                    "/distance",
                    &format!("a {} mate leaves no slide to drive", form.as_str()),
                ));
            }
            Some(x) => Some(
                Quantity::from_json_in(&x, default, &lookup)
                    .map_err(|e| e.with("pointer", "/distance"))?,
            ),
        };
        Ok(Mate {
            form,
            a,
            b,
            angle,
            distance,
        })
    }

    pub fn connectors(&self) -> [&Connector; 2] {
        [&self.a, &self.b]
    }
}

/// An instance pattern's direction or axis: an origin axis of the
/// assembly, or a connector's z axis (through its origin).
#[derive(Clone, Debug, PartialEq)]
pub enum Direction {
    Origin(OriginAxis),
    Connector(Box<Connector>),
}

impl Direction {
    pub fn to_json(&self) -> Value {
        match self {
            Direction::Origin(a) => json!(a.as_str()),
            Direction::Connector(c) => c.to_json(),
        }
    }

    fn from_json(v: &Value, at: &str, default: Unit, env: &Env) -> Result<Direction> {
        match v {
            Value::String(s) => OriginAxis::parse(s).map(Direction::Origin).ok_or_else(|| {
                invalid(
                    at,
                    "an origin axis is x, y or z; else give a connector {instance, at}",
                )
            }),
            v => Ok(Direction::Connector(Box::new(Connector::from_json(
                v, at, default, env,
            )?))),
        }
    }

    pub fn connector(&self) -> Option<&Connector> {
        match self {
            Direction::Connector(c) => Some(c),
            Direction::Origin(_) => None,
        }
    }
}

/// One direction of a linear instance pattern: `count` in all (the seed
/// included), `spacing` apart.
#[derive(Clone, Debug, PartialEq)]
pub struct Way {
    pub direction: Direction,
    pub spacing: Quantity,
    pub count: Quantity,
    pub reverse: bool,
}

/// How an instance pattern places its copies.
#[derive(Clone, Debug, PartialEq)]
pub enum PatternForm {
    Linear {
        first: Way,
        second: Option<Way>,
    },
    /// `count` in all, evenly round a whole turn, or from first to last
    /// over a smaller `angle`.
    Circular {
        axis: Direction,
        count: Quantity,
        angle: Quantity,
    },
}

impl PatternForm {
    pub fn type_name(&self) -> &'static str {
        match self {
            PatternForm::Linear { .. } => "linear",
            PatternForm::Circular { .. } => "circular",
        }
    }

    /// How many along each way, the seed's place included.
    pub fn counts(&self) -> [u32; 2] {
        let n = |q: &Quantity| q.number() as u32;
        match self {
            PatternForm::Linear { first, second } => {
                [n(&first.count), second.as_ref().map_or(1, |w| n(&w.count))]
            }
            PatternForm::Circular { count, .. } => [n(count), 1],
        }
    }
}

/// An `instance_pattern`: copies of earlier instances (its seeds), each a
/// seed's part at the seed's revision, posed by the seed's pose moved by
/// the pattern; they move with the seed and are never solved on their
/// own.
#[derive(Clone, Debug, PartialEq)]
pub struct InstancePattern {
    pub form: PatternForm,
    pub instances: Vec<Id>,
    /// Copies left out, by index.
    pub skip: Vec<[u32; 2]>,
}

impl Way {
    fn write(&self, v: &mut Value, suffix: &str) {
        v[format!("direction{suffix}")] = self.direction.to_json();
        v[format!("spacing{suffix}")] = json!(self.spacing.text());
        v[format!("count{suffix}")] = json!(self.count.text());
        if self.reverse {
            v[format!("reverse{suffix}")] = json!(true);
        }
    }

    fn read(o: &mut Obj, suffix: &str, default: Unit, env: &Env) -> Result<Way> {
        let at = |k: &str| format!("/{k}{suffix}");
        Ok(Way {
            direction: Direction::from_json(
                &o.take(&format!("direction{suffix}"))?,
                &at("direction"),
                default,
                env,
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

impl InstancePattern {
    pub fn fields(&self) -> Value {
        let mut v = json!({"form": self.form.type_name(), "instances": self.instances});
        match &self.form {
            PatternForm::Linear { first, second } => {
                first.write(&mut v, "");
                if let Some(w) = second {
                    w.write(&mut v, "2");
                }
            }
            PatternForm::Circular { axis, count, angle } => {
                v["axis"] = axis.to_json();
                v["count"] = json!(count.text());
                v["angle"] = json!(angle.text());
            }
        }
        if !self.skip.is_empty() {
            let two = self.two_ways();
            let skip: Vec<Value> = self
                .skip
                .iter()
                .map(|[i, j]| if two { json!([i, j]) } else { json!(i) })
                .collect();
            v["skip"] = json!(skip);
        }
        v
    }

    /// Whether it has a second direction.
    pub fn two_ways(&self) -> bool {
        matches!(
            &self.form,
            PatternForm::Linear {
                second: Some(_),
                ..
            }
        )
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<InstancePattern> {
        let form = o.take("form")?;
        let form = match form.as_str() {
            Some("linear") => {
                let first = Way::read(o, "", default, env)?;
                let second = if ["direction2", "spacing2", "count2", "reverse2"]
                    .iter()
                    .any(|k| o.has(k))
                {
                    Some(Way::read(o, "2", default, env)?)
                } else {
                    None
                };
                PatternForm::Linear { first, second }
            }
            Some("circular") => {
                let axis = Direction::from_json(&o.take("axis")?, "/axis", default, env)?;
                let count = count(&o.take("count")?, env, "/count")?;
                let angle =
                    Quantity::from_json_as(&o.take("angle")?, Field::Angle, &|n| env.get(n))
                        .map_err(|e| e.with("pointer", "/angle"))?;
                let a = angle.radians();
                if a <= 0.0 || a > core::f64::consts::TAU + ryukei::tol::ANGULAR_RESOLUTION {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!(
                            "a circular pattern's angle is in (0, 360 deg], not {}",
                            angle.text()
                        ),
                    )
                    .with("pointer", "/angle"));
                }
                PatternForm::Circular { axis, count, angle }
            }
            _ => {
                return Err(invalid(
                    "/form",
                    "an instance pattern's form is linear or circular",
                ));
            }
        };
        let [n, m] = form.counts();
        if f64::from(n) * f64::from(m) > super::pattern::MAX_INSTANCES {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "a pattern has at most {} copies, not {}",
                    super::pattern::MAX_INSTANCES,
                    u64::from(n) * u64::from(m)
                ),
            )
            .with("pointer", "/count2"));
        }
        let instances = super::solid::targets_from_json(&o.take("instances")?, "/instances")?;
        let two = matches!(
            &form,
            PatternForm::Linear {
                second: Some(_),
                ..
            }
        );
        let skip = match o.take_opt("skip") {
            None => Vec::new(),
            Some(v) => skip_from_json(&v, [n, m], two)?,
        };
        Ok(InstancePattern {
            form,
            instances,
            skip,
        })
    }

    /// Every copy but the seeds' own place and those skipped, in order:
    /// along the first direction, then each further step along the second.
    pub fn copies(&self) -> Vec<[u32; 2]> {
        let [n, m] = self.form.counts();
        (0..m)
            .flat_map(|j| (0..n).map(move |i| [i, j]))
            .filter(|x| *x != [0, 0] && !self.skip.contains(x))
            .collect()
    }

    /// Its connectors, each with its role: `direction`, `direction2`,
    /// `axis`.
    pub fn connectors(&self) -> Vec<(&'static str, &Connector)> {
        match &self.form {
            PatternForm::Linear { first, second } => first
                .direction
                .connector()
                .map(|c| ("direction", c))
                .into_iter()
                .chain(
                    second
                        .as_ref()
                        .and_then(|w| w.direction.connector())
                        .map(|c| ("direction2", c)),
                )
                .collect(),
            PatternForm::Circular { axis, .. } => {
                axis.connector().map(|c| ("axis", c)).into_iter().collect()
            }
        }
    }
}

/// The expression strings of a connector's JSON: its offset's origin and
/// turn.
pub fn expression_strings(v: &mut Value) -> Vec<&mut String> {
    let mut out = Vec::new();
    let Some(Value::Object(o)) = v.get_mut("offset") else {
        return out;
    };
    for (k, x) in o.iter_mut() {
        match (k.as_str(), x) {
            ("origin", Value::Array(a)) => out.extend(a.iter_mut().filter_map(|x| match x {
                Value::String(s) => Some(s),
                _ => None,
            })),
            ("rotate", Value::String(s)) => out.push(s),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{Feature, feature_format};

    const N: &str = "0123456789abcdef0123456789abcdef";

    fn read(v: &Value) -> Result<Feature> {
        let p = [(
            "tilt".to_owned(),
            crate::expr::Expr::parse("30 deg").unwrap(),
        )]
        .into();
        Feature::from_json(v, Unit::Mm, &Env::new(&p))
    }

    fn mate() -> Value {
        json!({"format": 4, "kind": "ryucad.feature", "type": "mate", "form": "revolute",
               "a": {"instance": ["base"], "at": {"feature": "plate", "name": N}},
               "b": {"instance": ["arm"], "at": "origin",
                     "offset": {"origin": ["0 mm", "0 mm", "2 mm"], "rotate": "tilt", "flip": true}},
               "angle": "30 deg"})
    }

    #[test]
    fn mates_read_and_write() {
        let v = mate();
        let f = read(&v).unwrap();
        let Feature::Mate(m) = &f else {
            panic!("a mate")
        };
        assert_eq!(m.form, Form::Revolute);
        assert_eq!(m.a.root.as_str(), "base");
        assert_eq!(m.b.at, At::Origin);
        assert!(m.b.offset.flip);
        assert_eq!(
            m.b.offset.rotate.as_ref().unwrap().radians(),
            m.angle.as_ref().unwrap().radians()
        );
        assert_eq!(m.a.text(), format!("base/plate/{N}"));
        assert_eq!(m.b.text(), "arm/origin");
        assert_eq!(f.to_json(), v);
        assert_eq!(f.format(), 4);
        assert_eq!(feature_format(&v), 4);
        assert_eq!(
            crate::doc::files::feature_names(&v)
                .into_iter()
                .collect::<Vec<_>>(),
            ["tilt"]
        );
    }

    #[test]
    fn mates_are_checked() {
        let bad = |edit: &dyn Fn(&mut Value), pointer: &str| {
            let mut v = mate();
            edit(&mut v);
            let e = read(&v).unwrap_err();
            assert_eq!(e.details["pointer"], pointer, "{e:?}");
        };
        bad(&|v| v["form"] = json!("weld"), "/form");
        bad(&|v| v["form"] = json!("ball"), "/angle");
        bad(&|v| v["distance"] = json!("2 mm"), "/distance");
        bad(&|v| v["a"]["instance"] = json!([]), "/a/instance");
        bad(&|v| v["a"]["instance"] = json!(["Base"]), "/a/instance/0");
        bad(&|v| v["a"]["instance"] = json!(["p1:3"]), "/a/instance");
        bad(
            &|v| v["a"]["instance"] = json!(["p1:0", "bolt"]),
            "/a/instance/0",
        );
        bad(
            &|v| v["a"]["instance"] = json!(["p1:2:0", "bolt"]),
            "/a/instance/0",
        );
        bad(
            &|v| v["a"]["instance"] = json!(["p1:2", "q:1"]),
            "/a/instance/1",
        );
        bad(&|v| v["a"]["at"] = json!("middle"), "/a/at");
        bad(
            &|v| v["b"]["offset"]["origin"] = json!([1, 2]),
            "/b/offset/origin",
        );
        bad(&|v| v["b"]["offset"]["flip"] = json!(1), "/b/offset/flip");
        // A copy, then its seed.
        let mut v = mate();
        v["a"]["instance"] = json!(["p1:2:1", "bolt", "nut"]);
        let f = read(&v).unwrap();
        let Feature::Mate(m) = &f else { panic!() };
        assert_eq!((m.a.root.as_str(), m.a.copy), ("p1", Some([2, 1])));
        assert_eq!(m.a.path(), ["p1:2:1", "bolt", "nut"]);
        assert_eq!(f.to_json(), v);
    }

    fn circular() -> Value {
        json!({"format": 4, "kind": "ryucad.feature", "type": "instance_pattern",
               "form": "circular", "instances": ["bolt1"],
               "axis": {"instance": ["flange"], "at": {"feature": "flange", "name": N}},
               "count": 6, "angle": "360 deg", "skip": [3]})
    }

    #[test]
    fn instance_patterns_read_and_write() {
        let v = circular();
        let f = read(&v).unwrap();
        let Feature::InstancePattern(p) = &f else {
            panic!("an instance pattern")
        };
        assert_eq!(p.copies(), [[1, 0], [2, 0], [4, 0], [5, 0]]);
        assert_eq!(p.connectors().len(), 1);
        let mut written = v.clone();
        written["count"] = json!("6");
        assert_eq!(f.to_json(), written);
        assert_eq!(feature_format(&v), 4);
        let linear = json!({"format": 4, "kind": "ryucad.feature", "type": "instance_pattern",
                            "form": "linear", "instances": ["a", "b"], "direction": "x",
                            "spacing": "10 mm", "count": "3", "direction2": "y",
                            "spacing2": "5 mm", "count2": "2", "reverse2": true,
                            "skip": [[2, 1]]});
        let f = read(&linear).unwrap();
        assert_eq!(f.to_json(), linear);
        let Feature::InstancePattern(p) = &f else {
            panic!()
        };
        assert_eq!(p.copies(), [[1, 0], [2, 0], [0, 1], [1, 1]]);
        let mut v = circular();
        v["form"] = json!("mirror");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/form");
        let mut v = circular();
        v["skip"] = json!([6]);
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/skip/0");
    }
}
