//! An assembly's mates and instance patterns, and the connectors they
//! read: frames defined from named entities of the parts its instances
//! place.
//!
//! A connector is `{"instance": [path], "at": {feature, name} | "origin",
//! "offset": {…}}`: the instance path from this assembly down through
//! subassemblies (a pattern's copy written `<pattern>:<i>`,
//! `<pattern>:<i>:<j>` past its first row, or `<pattern>:<point>` for a
//! pattern that follows a sketch's points, then the seed it copies), a
//! face, edge or vertex of a solid of the part at the end of the path, by
//! its founder and stable name, or the part's own frame, and an offset in
//! the frame's own axes. Which of the six kinds a mate is, and which of
//! the four an instance pattern is, is its `form` (a feature file's `kind`
//! is its header).

use super::files::{Obj, invalid};
use super::id::{CopyKey, Id};
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
    /// For a pattern's copy, its key: an index (never `[0, 0]`, the
    /// seed), or the sketch point that drives it.
    pub copy: Option<CopyKey>,
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
    /// A datum of the part (`{"feature"}`): a plane's frame, an axis
    /// with z along it, a point with the part's axes. Never a sketch: a
    /// sketch's plane reaches a connector through a datum plane offset 0
    /// from it, and a sketch's ID written here fails at rebuild as
    /// `not_found`.
    Datum(Id),
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

pub use super::id::{parse_step, step_text};

impl Connector {
    /// The instance path as a file writes it.
    pub fn path(&self) -> Vec<String> {
        std::iter::once(step_text(&self.root, self.copy.as_ref()))
            .chain(self.rest.iter().map(Id::to_string))
            .collect()
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "instance": self.path(),
            "at": match &self.at {
                At::Origin => json!("origin"),
                At::Entity(r) => r.to_json(),
                At::Datum(d) => json!({"feature": d}),
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
            v @ Value::Object(m) if !m.contains_key("name") => {
                match super::geometry::GeomRef::from_json(v, &format!("{at}/at"))? {
                    super::geometry::GeomRef::Feature(d) => At::Datum(d),
                    _ => {
                        return Err(invalid(
                            &format!("{at}/at"),
                            "a connector is at origin, an entity {feature, name} or a datum {feature}",
                        ));
                    }
                }
            }
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
            At::Datum(d) => d.to_string(),
        };
        format!("{}/{at}", self.path().join("/"))
    }
}

/// The feature format that holds a mate's limits (ryucad 26). A mate
/// without limits stays at format 4.
pub const LIMITS_FORMAT: u64 = 22;

/// A range a mate's free value stays within: either bound may be
/// missing, never both.
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub min: Option<Quantity>,
    pub max: Option<Quantity>,
}

/// A mate's stops on its free angle and distance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits {
    pub angle: Option<Range>,
    pub distance: Option<Range>,
}

/// `invalid_param` at the JSON pointer `at`: a limit the mate can't
/// take, which a write is refused for as a parameter.
fn bad_limit(at: &str, msg: &str) -> Error {
    Error::new(Code::InvalidParam, msg).with("pointer", at)
}

impl Range {
    fn to_json(&self) -> Value {
        let mut v = json!({});
        if let Some(q) = &self.min {
            v["min"] = json!(q.text());
        }
        if let Some(q) = &self.max {
            v["max"] = json!(q.text());
        }
        v
    }

    fn from_json(v: &Value, at: &str, field: Field, env: &Env) -> Result<Range> {
        let mut o = Obj::new(v, at)?;
        let lookup = |n: &str| env.get(n);
        let (min, max) = (o.take_opt("min"), o.take_opt("max"));
        o.finish()?;
        let read = |x: Option<Value>, k: &str| -> Result<Option<Quantity>> {
            x.map(|x| {
                Quantity::from_json_as(&x, field, &lookup)
                    .map_err(|e| e.with("pointer", format!("{at}/{k}")))
            })
            .transpose()
        };
        let (min, max) = (read(min, "min")?, read(max, "max")?);
        if min.is_none() && max.is_none() {
            return Err(bad_limit(at, "a limit has a min, a max or both"));
        }
        Ok(Range { min, max })
    }
}

impl Limits {
    pub fn to_json(&self) -> Value {
        let mut v = json!({});
        if let Some(r) = &self.angle {
            v["angle"] = r.to_json();
        }
        if let Some(r) = &self.distance {
            v["distance"] = r.to_json();
        }
        v
    }

    fn from_json(v: &Value, form: Form, default: Unit, env: &Env) -> Result<Limits> {
        let mut o = Obj::new(v, "/limits")?;
        let angle = match o.take_opt("angle") {
            None => None,
            Some(_) if !form.takes_angle() => {
                return Err(bad_limit(
                    "/limits/angle",
                    &format!("a {} mate leaves no turn to limit", form.as_str()),
                ));
            }
            Some(x) => Some(Range::from_json(&x, "/limits/angle", Field::Angle, env)?),
        };
        let distance = match o.take_opt("distance") {
            None => None,
            Some(_) if !form.takes_distance_limit() => {
                return Err(bad_limit(
                    "/limits/distance",
                    &format!(
                        "a {} mate leaves no slide to limit (a planar mate's distance is always driven)",
                        form.as_str()
                    ),
                ));
            }
            Some(x) => Some(Range::from_json(
                &x,
                "/limits/distance",
                Field::Length(default),
                env,
            )?),
        };
        o.finish()?;
        if angle.is_none() && distance.is_none() {
            return Err(bad_limit(
                "/limits",
                "limits holds an angle, a distance or both",
            ));
        }
        for (k, r, si) in [
            ("angle", &angle, Quantity::radians as fn(&Quantity) -> f64),
            (
                "distance",
                &distance,
                Quantity::meters as fn(&Quantity) -> f64,
            ),
        ] {
            let Some(Range {
                min: Some(lo),
                max: Some(hi),
            }) = r
            else {
                continue;
            };
            if si(lo) > si(hi) {
                return Err(bad_limit(
                    &format!("/limits/{k}"),
                    &format!("its min {} is past its max {}", lo.text(), hi.text()),
                ));
            }
            if k == "angle" && si(hi) - si(lo) >= core::f64::consts::TAU {
                return Err(bad_limit(
                    "/limits/angle",
                    "an angle limit spans less than a whole turn (360 deg)",
                ));
            }
        }
        Ok(Limits { angle, distance })
    }
}

/// The expression strings of a mate's `limits`: each bound.
pub fn limit_strings(v: &mut Value) -> Vec<&mut String> {
    v.as_object_mut()
        .into_iter()
        .flat_map(|o| o.values_mut())
        .filter_map(Value::as_object_mut)
        .flat_map(|r| r.values_mut())
        .filter_map(|x| match x {
            Value::String(s) => Some(s),
            _ => None,
        })
        .collect()
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

    /// Whether `limits.distance` may stop it: a slide it leaves free (a
    /// planar mate's distance is always driven).
    pub fn takes_distance_limit(self) -> bool {
        matches!(self, Form::Slider | Form::Cylindrical)
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
    /// Stops on what it leaves free (ryucad 26, format 22).
    pub limits: Option<Limits>,
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
        if let Some(l) = &self.limits {
            v["limits"] = l.to_json();
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
        let limits = match o.take_opt("limits") {
            None => None,
            Some(x) => Some(Limits::from_json(&x, form, default, env)?),
        };
        Ok(Mate {
            form,
            a,
            b,
            angle,
            distance,
            limits,
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

/// The feature format of an instance pattern that follows a part's
/// pattern, hole or sketch (ryucad 25.C, design 521). Linear and
/// circular patterns stay at 4.
pub const DRIVEN_FORMAT: u64 = 24;

/// Where a following pattern's places are: feature `feature` of the part
/// at the end of `instance`, a path from this assembly down. No step is
/// a copy: places move with an instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Driver {
    pub instance: Vec<Id>,
    pub feature: Id,
}

impl Driver {
    pub fn to_json(&self) -> Value {
        json!({"instance": self.instance, "feature": self.feature})
    }

    fn from_json(v: &Value, at: &str) -> Result<Driver> {
        let mut o = Obj::new(v, at)?;
        let here = format!("{at}/instance");
        let path = o.take("instance")?;
        let path = path
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid(&here, "an instance path is a list of instance IDs"))?;
        let mut instance = Vec::with_capacity(path.len());
        for (i, s) in path.iter().enumerate() {
            let w = format!("{here}/{i}");
            let s = s
                .as_str()
                .ok_or_else(|| invalid(&w, "an instance path's steps are strings"))?;
            match parse_step(s, &w)? {
                (id, None) => instance.push(id),
                (_, Some(_)) => {
                    return Err(invalid(
                        &w,
                        "places move with an instance, never with a pattern's copy",
                    ));
                }
            }
        }
        let w = format!("{at}/feature");
        let feature = o.take("feature")?;
        let feature = feature
            .as_str()
            .ok_or_else(|| invalid(&w, "a feature's ID"))?;
        let feature = Id::parse(feature, "feature").map_err(|e| e.with("pointer", &w))?;
        o.finish()?;
        Ok(Driver { instance, feature })
    }

    /// The path, then the feature, joined by `/`: `lid/hole-pattern`.
    pub fn text(&self) -> String {
        let mut s: Vec<&str> = self.instance.iter().map(Id::as_str).collect();
        s.push(self.feature.as_str());
        s.join("/")
    }
}

/// Point IDs, each once.
fn point_ids(v: &Value, at: &str) -> Result<Vec<Id>> {
    let a = v
        .as_array()
        .ok_or_else(|| invalid(at, "a list of the sketch's point IDs"))?;
    let mut out: Vec<Id> = Vec::with_capacity(a.len());
    for (i, x) in a.iter().enumerate() {
        let w = format!("{at}/{i}");
        let id = x
            .as_str()
            .ok_or_else(|| invalid(&w, "a point's ID"))
            .and_then(|s| Id::parse(s, "point").map_err(|e| e.with("pointer", &w)))?;
        if out.contains(&id) {
            return Err(invalid(&w, &format!("{id} is named twice")));
        }
        out.push(id);
    }
    Ok(out)
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
    /// A copy at each instance of a part's `pattern`, or at each point of
    /// a `hole` but the seed's (`from`, else its first).
    Feature {
        on: Driver,
        from: Option<Id>,
    },
    /// A copy at each point of a sketch but the seed's: `points` when
    /// listed, else every point no curve uses; the seed at `from`, else
    /// the first.
    Points {
        on: Driver,
        from: Option<Id>,
        points: Vec<Id>,
    },
}

impl PatternForm {
    pub fn type_name(&self) -> &'static str {
        match self {
            PatternForm::Linear { .. } => "linear",
            PatternForm::Circular { .. } => "circular",
            PatternForm::Feature { .. } => "feature",
            PatternForm::Points { .. } => "points",
        }
    }

    /// How many along each way, the seed's place included; none for a
    /// pattern that follows a part, whose places its part says.
    pub fn counts(&self) -> Option<[u32; 2]> {
        let n = |q: &Quantity| q.number() as u32;
        match self {
            PatternForm::Linear { first, second } => {
                Some([n(&first.count), second.as_ref().map_or(1, |w| n(&w.count))])
            }
            PatternForm::Circular { count, .. } => Some([n(count), 1]),
            PatternForm::Feature { .. } | PatternForm::Points { .. } => None,
        }
    }

    /// Where a following pattern's places are.
    pub fn driver(&self) -> Option<&Driver> {
        match self {
            PatternForm::Feature { on, .. } | PatternForm::Points { on, .. } => Some(on),
            _ => None,
        }
    }

    /// Whether it follows a part's places (forms `feature` and `points`).
    pub fn driven(&self) -> bool {
        self.driver().is_some()
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
            PatternForm::Feature { on, from } => {
                v["on"] = on.to_json();
                if let Some(f) = from {
                    v["from"] = json!(f);
                }
            }
            PatternForm::Points { on, from, points } => {
                v["on"] = on.to_json();
                if let Some(f) = from {
                    v["from"] = json!(f);
                }
                if !points.is_empty() {
                    v["points"] = json!(points);
                }
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
            Some(f @ ("feature" | "points")) => {
                let on = Driver::from_json(&o.take("on")?, "/on")?;
                let from = match o.take_opt("from") {
                    None => None,
                    Some(v) => Some(
                        v.as_str()
                            .ok_or_else(|| invalid("/from", "a point's ID"))
                            .and_then(|s| {
                                Id::parse(s, "point").map_err(|e| e.with("pointer", "/from"))
                            })?,
                    ),
                };
                if f == "feature" {
                    PatternForm::Feature { on, from }
                } else {
                    let points = match o.take_opt("points") {
                        None => Vec::new(),
                        Some(v) => point_ids(&v, "/points")?,
                    };
                    PatternForm::Points { on, from, points }
                }
            }
            _ => {
                return Err(invalid(
                    "/form",
                    "an instance pattern's form is linear, circular, feature or points",
                ));
            }
        };
        if let Some([n, m]) = form.counts()
            && f64::from(n) * f64::from(m) > super::pattern::MAX_INSTANCES
        {
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
            Some(_) if form.driven() => {
                return Err(invalid(
                    "/skip",
                    "a pattern that follows a part skips nothing: list the points it takes",
                ));
            }
            Some(v) => skip_from_json(&v, form.counts().expect("not driven"), two)?,
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
        let Some([n, m]) = self.form.counts() else {
            return Vec::new();
        };
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
            PatternForm::Feature { .. } | PatternForm::Points { .. } => Vec::new(),
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

    fn limited() -> Value {
        let mut v = mate();
        v.as_object_mut().unwrap().remove("angle");
        v["format"] = json!(LIMITS_FORMAT);
        v["limits"] = json!({"angle": {"min": "-6 deg", "max": "tilt"}});
        v
    }

    #[test]
    fn limits_read_and_write_at_their_format() {
        let v = limited();
        let f = read(&v).unwrap();
        let Feature::Mate(m) = &f else {
            panic!("a mate")
        };
        let r = m.limits.as_ref().unwrap().angle.as_ref().unwrap();
        assert_eq!(r.max.as_ref().unwrap().text(), "tilt");
        assert!(r.min.as_ref().unwrap().radians() < 0.0);
        assert_eq!(f.to_json(), v);
        assert_eq!(f.format(), LIMITS_FORMAT);
        assert_eq!(feature_format(&v), LIMITS_FORMAT);
        assert_eq!(
            crate::doc::files::feature_names(&v)
                .into_iter()
                .collect::<Vec<_>>(),
            ["tilt"]
        );
        // A mate without limits is written as before.
        assert_eq!(read(&mate()).unwrap().format(), 4);
        // One-sided, and both values on a cylindrical mate.
        let mut v = limited();
        v["form"] = json!("cylindrical");
        v["limits"] =
            json!({"angle": {"max": "6 deg"}, "distance": {"min": "0 mm", "max": "10 mm"}});
        assert_eq!(read(&v).unwrap().to_json(), v);
    }

    #[test]
    fn limits_are_checked() {
        let bad = |edit: &dyn Fn(&mut Value), pointer: &str| {
            let mut v = limited();
            edit(&mut v);
            let e = read(&v).unwrap_err();
            assert!(
                e.details["pointer"].as_str().unwrap().starts_with(pointer),
                "{pointer}: {e:?}"
            );
        };
        bad(&|v| v["limits"] = json!({}), "/limits");
        bad(&|v| v["limits"] = json!({"angle": {}}), "/limits/angle");
        bad(
            &|v| v["limits"] = json!({"distance": {"max": "2 mm"}}),
            "/limits/distance",
        );
        bad(
            &|v| v["limits"] = json!({"angle": {"min": "10 deg", "max": "5 deg"}}),
            "/limits/angle",
        );
        bad(
            &|v| v["limits"] = json!({"angle": {"min": "-180 deg", "max": "180 deg"}}),
            "/limits/angle",
        );
        bad(
            &|v| v["limits"] = json!({"angle": {"min": "2 mm"}}),
            "/limits/angle/min",
        );
        bad(
            &|v| v["limits"] = json!({"angle": {"low": "1 deg"}}),
            "/limits/angle",
        );
        bad(
            &|v| {
                v["form"] = json!("planar");
                v["limits"] = json!({"distance": {"max": "1 mm"}});
            },
            "/limits/distance",
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
        assert_eq!(
            (m.a.root.as_str(), m.a.copy.clone()),
            ("p1", Some(CopyKey::Index([2, 1])))
        );
        assert_eq!(m.a.path(), ["p1:2:1", "bolt", "nut"]);
        assert_eq!(f.to_json(), v);
    }

    #[test]
    fn following_patterns_read_and_write_at_format_24() {
        let v = json!({"format": 24, "kind": "ryucad.feature", "type": "instance_pattern",
                       "form": "feature", "instances": ["screw-0"],
                       "on": {"instance": ["lid"], "feature": "hole-pattern"}});
        let f = read(&v).unwrap();
        assert_eq!(f.to_json(), v);
        assert_eq!((feature_format(&v), f.format()), (24, 24));
        let Feature::InstancePattern(p) = &f else {
            panic!()
        };
        assert!(p.form.driven() && p.copies().is_empty() && p.connectors().is_empty());
        assert_eq!(p.form.driver().unwrap().text(), "lid/hole-pattern");
        let v = json!({"format": 24, "kind": "ryucad.feature", "type": "instance_pattern",
                       "form": "points", "instances": ["screw-0"], "from": "p3",
                       "on": {"instance": ["sub", "lid"], "feature": "screw-layout"},
                       "points": ["p3", "p4"]});
        let f = read(&v).unwrap();
        assert_eq!(f.to_json(), v);
        let Feature::InstancePattern(p) = &f else {
            panic!()
        };
        assert_eq!(p.form.type_name(), "points");
        // Refusals: a copy in the path, a skip, points on form feature, a
        // point named twice, an unknown form.
        for (k, x, at) in [
            (
                "on",
                json!({"instance": ["ring:2", "lid"], "feature": "h"}),
                "/on/instance/0",
            ),
            ("skip", json!([1]), "/skip"),
            ("points", json!(["p1", "p1"]), "/points/1"),
        ] {
            let mut w = v.clone();
            w[k] = x;
            assert_eq!(read(&w).unwrap_err().details["pointer"], at, "{k}");
        }
        let mut w = v.clone();
        w["form"] = json!("feature");
        assert!(read(&w).is_err(), "points belong to form points");
        let mut w = v.clone();
        w["form"] = json!("holes");
        assert_eq!(read(&w).unwrap_err().details["pointer"], "/form");
        // A linear or circular pattern stays at format 4.
        assert_eq!(feature_format(&circular()), 4);
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
