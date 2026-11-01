//! Resolving geometric references ([`GeomRef`]) to what a use wants: a
//! plane, a directed axis, a point or a curve, in the part's frame, read
//! in the reading feature's view as references are (see
//! [`crate::reference`]). One resolver for every field that takes
//! geometry, so a datum, a sketch's plane, a sketch line or point works
//! wherever a face or an edge did.
//!
//! | Source | Plane | Axis | Point | Curve |
//! |---|---|---|---|---|
//! | origin plane | its frame | | | |
//! | origin axis | | through the origin | | |
//! | `origin` | | | the origin | |
//! | planar face | outward normal, a face sketch's frame | | | |
//! | straight edge | | start to end | | the edge |
//! | circular edge | | its circle's axis | its centre | the edge |
//! | other edge | | | | the edge |
//! | cylinder, cone, torus or revolution face | | its surface's axis | | |
//! | sphere face | | | its centre | |
//! | vertex | | | its point | |
//! | datum plane, axis, point | its own kind | | | |
//! | sketch | its frame | | | |
//! | sketch line | | start to end | | the line |
//! | sketch arc or circle | | normal to the sketch through its centre | its centre | the curve |
//! | sketch point | | | its point | |
//!
//! Anything else is `reference_wrong_kind` with `wanted` and `got`. Every
//! value is f64 meters through `ryukei::math` and ryukei's `Frame`, the
//! same bits everywhere; a face's or an edge's are computed exactly as
//! the uses computed them before, so no result moved.

use crate::doc::geometry::GeomRef;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::sketch::Geometry;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Output, Results};
use ryukei::geom::{Axis, Circle, Curve, Frame, Point3, Surface, Vector3};
use ryukei::topo::{EntityId, Sense};

/// A plane as a use takes it: a point on it and its unit normal as its
/// source gives them (a face's surface origin and outward normal, bit for
/// bit), and the frame a sketch on it sees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub point: Point3,
    pub normal: Vector3,
    pub frame: Frame,
}

/// A directed axis as a use takes it: a point on it and its unit
/// direction as its source gives them (a circle's or a surface's axis bit
/// for bit), and a second point along it: a straight edge's or a sketch
/// line's end, else one unit along. Uses that worked from an edge's ends
/// work from `origin` and `to`, so their results keep their bits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub origin: Point3,
    pub dir: ryukei::geom::UnitVector3,
    pub to: Point3,
}

impl Line {
    /// Through `origin` along unit `dir`.
    pub fn along(origin: Point3, dir: ryukei::geom::UnitVector3) -> Line {
        Line {
            origin,
            dir,
            to: origin + dir.get(),
        }
    }

    /// From `a` to `b`; `None` where they are one point.
    pub fn between(a: Point3, b: Point3) -> Option<Line> {
        Some(Line {
            origin: a,
            dir: (b - a).normalize()?,
            to: b,
        })
    }

    /// As ryukei's axis.
    pub fn axis(&self) -> Axis {
        Axis::new(self.origin, self.dir.get()).expect("a unit direction")
    }

    /// Moved by a rigid placement.
    pub fn transformed(&self, r: &ryukei::geom::Rigid) -> Line {
        Line {
            origin: r.apply_point(self.origin),
            dir: r.apply_unit(self.dir),
            to: r.apply_point(self.to),
        }
    }
}

/// A curve over the range it runs, from `range[0]` to `range[1]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bounded {
    pub curve: Curve,
    pub range: [f64; 2],
}

/// What a geometric reference resolves to.
#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    Plane(Plane),
    Axis(Line),
    Point(Point3),
    Curve(Bounded),
}

/// What a field wants a reference to give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Plane,
    Axis,
    Point,
    Curve,
}

impl Want {
    pub fn as_str(self) -> &'static str {
        match self {
            Want::Plane => "a plane",
            Want::Axis => "an axis",
            Want::Point => "a point",
            Want::Curve => "a curve",
        }
    }
}

impl Geom {
    pub fn plane(self) -> Plane {
        match self {
            Geom::Plane(p) => p,
            g => unreachable!("asked for a plane, got {g:?}"),
        }
    }

    pub fn axis(self) -> Line {
        match self {
            Geom::Axis(a) => a,
            g => unreachable!("asked for an axis, got {g:?}"),
        }
    }

    pub fn point(self) -> Point3 {
        match self {
            Geom::Point(p) => p,
            g => unreachable!("asked for a point, got {g:?}"),
        }
    }

    pub fn curve(self) -> Bounded {
        match self {
            Geom::Curve(c) => c,
            g => unreachable!("asked for a curve, got {g:?}"),
        }
    }

    /// Which it is.
    pub fn want(&self) -> Want {
        match self {
            Geom::Plane(_) => Want::Plane,
            Geom::Axis(_) => Want::Axis,
            Geom::Point(_) => Want::Point,
            Geom::Curve(_) => Want::Curve,
        }
    }

    /// Itself moved by a rigid placement (an occurrence's pose).
    pub fn transformed(&self, r: &ryukei::geom::Rigid) -> Geom {
        match self {
            Geom::Plane(p) => Geom::Plane(Plane {
                point: r.apply_point(p.point),
                normal: r.apply_vector(p.normal),
                frame: p.frame.transformed(r),
            }),
            Geom::Axis(a) => Geom::Axis(a.transformed(r)),
            Geom::Point(p) => Geom::Point(r.apply_point(*p)),
            Geom::Curve(c) => Geom::Curve(Bounded {
                curve: c.curve.transformed(r),
                range: c.range,
            }),
        }
    }
}

/// The frame a plane through `p0` with unit normal `n` gives a sketch
/// with nothing to say otherwise: the part origin projected onto it, and
/// the part's `x` projected (its `y` where `x` is along the normal), as a
/// sketch on a face has always taken it.
pub fn default_frame(p0: Point3, n: Vector3) -> Result<Frame> {
    let o = Point3::ORIGIN;
    let origin = o - n * (o - p0).dot(n);
    let x = Vector3::new(1.0, 0.0, 0.0);
    let hint = if Frame::new(origin, n, x).is_ok() {
        x
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    Frame::new(origin, n, hint).map_err(|e| {
        Error::new(
            Code::InvalidPlacement,
            "a plane that gives no frame: its origin is outside the ±500 m size box",
        )
        .with("kernel", format!("{e:?}"))
    })
}

/// A plane from a point on it, a unit normal, and its frame by the
/// default rule.
pub fn plane_through(point: Point3, normal: Vector3) -> Result<Plane> {
    Ok(Plane {
        point,
        normal,
        frame: default_frame(point, normal)?,
    })
}

/// A plane whose frame is `frame`: its origin and axis are its point and
/// normal.
pub fn plane_of(frame: Frame) -> Plane {
    Plane {
        point: frame.origin(),
        normal: frame.z().get(),
        frame,
    }
}

/// Where references resolve: the part, its list as it is or as a write
/// would leave it, the feature reading them, and the results of the
/// features built before it.
#[derive(Clone, Copy)]
pub struct At<'a> {
    pub part: &'a Part,
    pub order: &'a [Id],
    pub owner: &'a Id,
    pub results: &'a dyn Results,
}

/// What a reference names, before a use takes what it wants of it.
enum Source<'a> {
    OriginPlane(crate::doc::sketch::OriginPlane),
    OriginAxis(crate::doc::pattern::OriginAxis),
    Origin,
    Entity(&'a crate::rebuild::Built, EntityId),
    Datum(&'a crate::datum::Made),
    Sketch(&'a crate::sketch::Solved),
    SketchEntity(&'a crate::sketch::Solved, Id),
}

impl At<'_> {
    fn fail(&self, code: Code, role: &str, g: &GeomRef, msg: String) -> Error {
        let text = g.text(&self.part.id);
        Error::new(code, format!("{}: its {role} {text} {msg}", self.owner))
            .with("feature", self.owner)
            .with("role", role)
            .with("reference", text)
    }

    /// The feature `f` that `g` (in `role`) names, built and listed before
    /// the owner.
    fn output(&self, role: &str, g: &GeomRef, f: &Id) -> Result<&Output> {
        let at = |x: &Id| self.order.iter().position(|o| o == x);
        match (at(f), at(self.owner)) {
            (None, _) => {
                return Err(self.fail(
                    Code::NotFound,
                    role,
                    g,
                    format!("names {f}, which the part doesn't list"),
                ));
            }
            (Some(i), Some(me)) if i >= me => {
                return Err(self.fail(
                    Code::ReferenceForward,
                    role,
                    g,
                    format!("names {f}, which doesn't come before it"),
                ));
            }
            _ => {}
        }
        match self.results.result(f) {
            None => Err(self.fail(
                Code::NotFound,
                role,
                g,
                format!("names {f}, which hasn't been built"),
            )),
            Some(Err(e)) => Err(self
                .fail(
                    Code::DependencyFailed,
                    role,
                    g,
                    format!("names {f}, which failed to rebuild"),
                )
                .with("dependency", f)
                .with("cause", e.code)),
            Some(Ok(o)) => Ok(o),
        }
    }

    fn source(&self, role: &str, g: &GeomRef) -> Result<Source<'_>> {
        match g {
            GeomRef::Plane(p) => Ok(Source::OriginPlane(*p)),
            GeomRef::Axis(a) => Ok(Source::OriginAxis(*a)),
            GeomRef::Origin => Ok(Source::Origin),
            GeomRef::Entity(r) => {
                let (b, e) = crate::reference::resolve(
                    self.part,
                    self.order,
                    self.owner,
                    role,
                    r,
                    self.results,
                )?;
                Ok(Source::Entity(b, e))
            }
            GeomRef::Feature(f) => match self.output(role, g, f)? {
                Output::Datum(d) => Ok(Source::Datum(d)),
                Output::Sketch(s) => Ok(Source::Sketch(s)),
                o => {
                    let what = self
                        .part
                        .features
                        .get(f)
                        .and_then(|x| x.as_ref().ok())
                        .map_or("feature", crate::doc::files::Feature::type_name);
                    let solid = !o.solids().is_empty();
                    let hint = if solid {
                        "; a solid is never a plane, an axis or a point: name its face, edge or vertex"
                    } else {
                        ""
                    };
                    Err(self
                        .fail(
                            Code::ReferenceWrongKind,
                            role,
                            g,
                            format!("is a {what}{hint}"),
                        )
                        .with("got", what))
                }
            },
            GeomRef::Sketch { feature, entity } => match self.output(role, g, feature)? {
                Output::Sketch(s) => {
                    if s.sketch.entities.contains_key(entity) {
                        Ok(Source::SketchEntity(s, entity.clone()))
                    } else {
                        Err(self
                            .fail(
                                Code::NotFound,
                                role,
                                g,
                                format!("names {entity}, which sketch {feature} doesn't have"),
                            )
                            .with("entities", s.sketch.entities.keys().collect::<Vec<_>>()))
                    }
                }
                _ => Err(self
                    .fail(
                        Code::ReferenceWrongKind,
                        role,
                        g,
                        format!("names {feature}, which isn't a sketch"),
                    )
                    .with("got", "feature")),
            },
        }
    }
}

/// What a source is, for messages, and each kind it gives.
fn kinds(s: &Source) -> (String, Vec<Geom>) {
    let unit = |v: [f64; 3]| Vector3::new(v[0], v[1], v[2]);
    match s {
        Source::OriginPlane(p) => {
            let (z, x) = p.axes();
            let frame = Frame::new(Point3::ORIGIN, unit(z), unit(x)).expect("an origin plane");
            ("origin plane".into(), vec![Geom::Plane(plane_of(frame))])
        }
        Source::OriginAxis(a) => {
            let dir = unit(a.vector()).normalize().expect("an origin axis");
            (
                "origin axis".into(),
                vec![Geom::Axis(Line::along(Point3::ORIGIN, dir))],
            )
        }
        Source::Origin => ("origin".into(), vec![Geom::Point(Point3::ORIGIN)]),
        Source::Entity(b, e) => entity_kinds(b, *e),
        Source::Datum(d) => (format!("datum {}", d.kind()), vec![d.geom.clone()]),
        Source::Sketch(s) => {
            let frame = Frame::from_rigid(&s.frame).expect("a sketch's frame is a frame");
            ("sketch".into(), vec![Geom::Plane(plane_of(frame))])
        }
        Source::SketchEntity(s, id) => sketch_entity_kinds(s, id),
    }
}

fn entity_kinds(b: &crate::rebuild::Built, e: EntityId) -> (String, Vec<Geom>) {
    let body = &b.body;
    match e {
        EntityId::Vertex(v) => (
            "vertex".into(),
            vec![Geom::Point(body.vertex(v).expect("a live vertex").point)],
        ),
        EntityId::Edge(id) => {
            let edge = body.edge(id).expect("a live edge");
            let curve = body.curve(edge.curve).expect("a live curve").clone();
            let at = |v| body.vertex(v).expect("a live vertex").point;
            let bounded = Geom::Curve(Bounded {
                curve: curve.clone(),
                range: edge.interval,
            });
            match &curve {
                Curve::Line(_) => {
                    let (a, z) = (at(edge.start), at(edge.end));
                    let mut out = Vec::new();
                    if let Some(axis) = Line::between(a, z) {
                        out.push(Geom::Axis(axis));
                    }
                    out.push(bounded);
                    ("straight edge".into(), out)
                }
                Curve::Circle(c) => {
                    let f = c.frame();
                    let axis = Line::along(f.origin(), f.z());
                    (
                        "circular edge".into(),
                        vec![Geom::Axis(axis), Geom::Point(f.origin()), bounded],
                    )
                }
                _ => ("curved edge".into(), vec![bounded]),
            }
        }
        EntityId::Face(f) => {
            let face = body.face(f).expect("a live face");
            let s = body.surface(face.surface).expect("a live surface");
            match s {
                Surface::Plane(_) => {
                    let frame = s.frame().expect("a plane has a frame");
                    let n = frame.z().get();
                    let n = if face.sense == Sense::Reversed { -n } else { n };
                    let point = frame.origin();
                    match default_frame(point, n) {
                        Ok(fr) => (
                            "planar face".into(),
                            vec![Geom::Plane(Plane {
                                point,
                                normal: n,
                                frame: fr,
                            })],
                        ),
                        Err(_) => ("planar face".into(), Vec::new()),
                    }
                }
                Surface::Cylinder(_) | Surface::Cone(_) | Surface::Torus(_) => {
                    let frame = s.frame().expect("an analytic surface");
                    let axis = Line::along(frame.origin(), frame.z());
                    ("face of revolution".into(), vec![Geom::Axis(axis)])
                }
                Surface::Revolution(r) => {
                    let frame = r.frame();
                    let axis = Line::along(frame.origin(), frame.z());
                    ("face of revolution".into(), vec![Geom::Axis(axis)])
                }
                Surface::Sphere(_) => {
                    let frame = s.frame().expect("a sphere has a frame");
                    ("spherical face".into(), vec![Geom::Point(frame.origin())])
                }
                _ => ("curved face".into(), Vec::new()),
            }
        }
        _ => ("entity".into(), Vec::new()),
    }
}

fn sketch_entity_kinds(s: &crate::sketch::Solved, id: &Id) -> (String, Vec<Geom>) {
    let sk = &s.sketch;
    let at = |p: &Id| match sk.entities[p].geometry {
        Geometry::Point { m } => s.frame.apply_point(Point3::new(m[0], m[1], 0.0)),
        _ => unreachable!("validated: curves join points"),
    };
    let n = s.frame.apply_vector(Vector3::new(0.0, 0.0, 1.0));
    match &sk.entities[id].geometry {
        Geometry::Point { .. } => ("sketch point".into(), vec![Geom::Point(at(id))]),
        Geometry::Line { start, end } => {
            let (a, z) = (at(start), at(end));
            let mut out = Vec::new();
            if let Some(axis) = Line::between(a, z) {
                out.push(Geom::Axis(axis));
            }
            if let Ok(l) = ryukei::geom::Line::new(a, z - a) {
                out.push(Geom::Curve(Bounded {
                    curve: Curve::Line(l),
                    range: [0.0, (z - a).length()],
                }));
            }
            ("sketch line".into(), out)
        }
        Geometry::Arc { center, start, end } => {
            let (c, a, z) = (at(center), at(start), at(end));
            let mut out = Vec::new();
            if let Some(dir) = n.normalize() {
                out.push(Geom::Axis(Line::along(c, dir)));
            }
            out.push(Geom::Point(c));
            if let Ok(frame) = Frame::new(c, n, a - c)
                && let Ok(circle) = Circle::new(frame, (a - c).length())
            {
                let q = frame.to_local(z);
                let mut sweep = ryukei::math::atan2(q.y, q.x);
                if sweep <= 0.0 {
                    sweep += core::f64::consts::TAU;
                }
                out.push(Geom::Curve(Bounded {
                    curve: Curve::Circle(circle),
                    range: [0.0, sweep],
                }));
            }
            ("sketch arc".into(), out)
        }
        Geometry::Circle { center, r_m } => {
            let c = at(center);
            let mut out = Vec::new();
            if let Some(dir) = n.normalize() {
                out.push(Geom::Axis(Line::along(c, dir)));
            }
            out.push(Geom::Point(c));
            let x = s.frame.apply_vector(Vector3::new(1.0, 0.0, 0.0));
            if let Ok(frame) = Frame::new(c, n, x)
                && let Ok(circle) = Circle::new(frame, *r_m)
            {
                out.push(Geom::Curve(Bounded {
                    curve: Curve::Circle(circle),
                    range: [0.0, core::f64::consts::TAU],
                }));
            }
            ("sketch circle".into(), out)
        }
    }
}

/// Resolves `g`, held in `role`, to the first of `wants` it gives, in
/// the part's frame. A reference that doesn't resolve fails as references
/// do; one that gives none of `wants` is `reference_wrong_kind` with
/// `wanted` and `got`.
pub fn resolve(at: &At, role: &str, g: &GeomRef, wants: &[Want]) -> Result<Geom> {
    let s = at.source(role, g)?;
    let (got, gives) = kinds(&s);
    for w in wants {
        if let Some(x) = gives.iter().find(|x| x.want() == *w) {
            return Ok(x.clone());
        }
    }
    let wanted: Vec<&str> = wants.iter().map(|w| w.as_str()).collect();
    let mut e = at
        .fail(
            Code::ReferenceWrongKind,
            role,
            g,
            format!("is a {got}, not {}", wanted.join(" or ")),
        )
        .with("wanted", &wanted)
        .with("got", &got);
    // As the uses said before: what an entity resolved to.
    if let Source::Entity(_, ent) = s {
        e = e.with("kind", crate::reference::kind(ent));
    }
    Err(e)
}

/// [`resolve`] for one kind.
pub fn one(at: &At, role: &str, g: &GeomRef, want: Want) -> Result<Geom> {
    resolve(at, role, g, &[want])
}

/// A datum a request names, placed: of a part (`<part>/<datum>`) or of an
/// occurrence (`<asm>/<instance>/…/<datum>`, placed by its pose).
#[derive(Clone, Debug)]
pub struct Located {
    /// The part the datum is a feature of.
    pub part: Id,
    pub datum: Id,
    /// Its geometry in the frame of the model the reference starts at.
    pub geom: Geom,
}

/// The datum `reference` names, placed in the model it starts at, or
/// `None` where it names no datum (an entity, a solid, an occurrence):
/// the caller reads it as it always has. A step names an instance while
/// the part before it is an assembly and a feature once it is a part.
pub fn locate(session: &mut crate::session::Session, reference: &str) -> Result<Option<Located>> {
    use crate::doc::files::PartKind;
    let mut steps = reference.split('/');
    let Some(first) = steps.next() else {
        return Ok(None);
    };
    let Ok(part) = Id::parse(first, "part") else {
        return Ok(None);
    };
    let Some(p) = session.project().parts.get(&part) else {
        return Ok(None);
    };
    let assembly = crate::server::part_kind(p) == PartKind::Assembly;
    let rest: Vec<&str> = steps.collect();
    let Some((last, path)) = rest.split_last() else {
        return Ok(None);
    };
    if !assembly && !path.is_empty() {
        return Ok(None);
    }
    let Ok(datum) = Id::parse(last, "feature") else {
        return Ok(None);
    };
    let path = if assembly {
        match crate::entity::EntityRef::parse(&format!("{part}/{}", path.join("/"))) {
            Ok(r) if !path.is_empty() => r.in_assembly().path,
            _ => return Ok(None),
        }
    } else {
        Vec::new()
    };
    let model = session.model(&part, &crate::doc::assembly::Rev::Working)?;
    let Some(o) = model.occurrences.iter().find(|o| o.path == path) else {
        return Ok(None);
    };
    let Some((_, made)) = o.datums.iter().find(|(id, _)| *id == datum) else {
        // A datum of the part that didn't build says why.
        let leaf = o.part.clone();
        if let Some(Ok(f)) = session.project().part(&leaf)?.features.get(&datum).cloned()
            && matches!(f, crate::doc::files::Feature::Datum(_))
        {
            let r = session.rebuilt(&leaf)?;
            if let Some((_, Err(e))) = r.features.iter().find(|(id, _)| *id == datum) {
                return Err(Error::new(
                    Code::DependencyFailed,
                    format!("{reference} names a datum that failed to rebuild"),
                )
                .with("entity", reference)
                .with("cause", e.code));
            }
        }
        return Ok(None);
    };
    Ok(Some(Located {
        part: o.part.clone(),
        datum,
        geom: made.geom.transformed(&o.pose),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::call;
    use crate::sketch::tests::{edge_between, face_along, rectangle_ops, vertex_at, with_box};
    use serde_json::json;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// Every source the box `b` and a sketch `s` on it give, against what
    /// each use wants, in the part's current rebuild.
    #[test]
    fn the_table_row_by_row() {
        let (_dir, mut s, body) = with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "zx"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": rectangle_ops()}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "id": "p9", "type": "point", "at": [3, 3]},
                {"op": "add", "id": "ci1", "type": "circle", "center": "p9", "r": 1}
            ]}),
        )
        .unwrap();
        let p = s.project().part(&id("p")).unwrap().clone();
        let results = s.rebuilt(&id("p")).unwrap().clone();
        let mut order = p.order().to_vec();
        order.push(id("reader"));
        let at = At {
            part: &p,
            order: &order,
            owner: &id("reader"),
            results: &results,
        };
        let g = |t: &str| GeomRef::parse(t, &id("p")).unwrap();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        let (a, z) = (
            vertex_at(&body, [0.0, 0.0, 0.0]),
            vertex_at(&body, [0.02, 0.0, 0.0]),
        );
        let edge = format!("p/b/{}", edge_between(&body, &a, &z));
        // An origin plane, axis and point.
        let xy = one(&at, "r", &g("xy"), Want::Plane).unwrap().plane();
        assert_eq!(xy.normal, Vector3::new(0.0, 0.0, 1.0));
        let y = one(&at, "r", &g("y"), Want::Axis).unwrap().axis();
        assert_eq!(y.dir.get(), Vector3::new(0.0, 1.0, 0.0));
        assert_eq!(
            one(&at, "r", &g("origin"), Want::Point).unwrap().point(),
            Point3::ORIGIN
        );
        // A planar face: its outward normal, and a face sketch's frame.
        let t = one(&at, "r", &g(&top), Want::Plane).unwrap().plane();
        assert_eq!(t.normal, Vector3::new(0.0, 0.0, 1.0));
        assert_eq!(t.frame.origin(), Point3::new(0.0, 0.0, 0.005));
        // A straight edge: an axis from its start, or a curve.
        let e = one(&at, "r", &g(&edge), Want::Axis).unwrap().axis();
        assert!(e.dir.get().cross(Vector3::new(1.0, 0.0, 0.0)).length() < 1e-15);
        let c = one(&at, "r", &g(&edge), Want::Curve).unwrap().curve();
        assert!(matches!(c.curve, Curve::Line(_)));
        // A vertex is a point; asking it for a plane says what it is.
        let v = format!("p/b/{a}");
        assert_eq!(
            one(&at, "r", &g(&v), Want::Point).unwrap().point(),
            Point3::ORIGIN
        );
        let err = one(&at, "r", &g(&v), Want::Plane).unwrap_err();
        assert_eq!(err.code, Code::ReferenceWrongKind);
        assert_eq!(err.details["got"], "vertex");
        assert_eq!(err.details["wanted"], json!(["a plane"]));
        // A sketch is a plane; its lines axes, its circles axes, centres
        // and curves, its points points.
        let sk = one(&at, "r", &g("p/s"), Want::Plane).unwrap().plane();
        assert_eq!(sk.normal, Vector3::new(0.0, 1.0, 0.0));
        let l = one(&at, "r", &g("p/s/l1"), Want::Axis).unwrap().axis();
        assert!(l.dir.get().length() > 0.99);
        let ci = resolve(&at, "r", &g("p/s/ci1"), &[Want::Point])
            .unwrap()
            .point();
        assert!((ci - Point3::new(0.003, 0.0, 0.003)).length() < 1e-15);
        let ax = one(&at, "r", &g("p/s/ci1"), Want::Axis).unwrap().axis();
        assert_eq!(ax.dir.get(), Vector3::new(0.0, 1.0, 0.0));
        let arc = one(&at, "r", &g("p/s/ci1"), Want::Curve).unwrap().curve();
        assert_eq!(arc.range, [0.0, core::f64::consts::TAU]);
        // A solid alone is never a plane, axis or point.
        let err = one(&at, "r", &g("p/b"), Want::Plane).unwrap_err();
        assert_eq!(err.code, Code::ReferenceWrongKind, "{err}");
        // What isn't there, or comes after, fails as references do.
        let err = one(&at, "r", &g("p/zz"), Want::Plane).unwrap_err();
        assert_eq!(err.code, Code::NotFound);
        let err = one(&at, "r", &g("p/s/p77"), Want::Point).unwrap_err();
        assert_eq!(err.code, Code::NotFound);
        let early = At {
            owner: &id("b"),
            ..at
        };
        let err = one(&early, "r", &g("p/s"), Want::Plane).unwrap_err();
        assert_eq!(err.code, Code::ReferenceForward);
    }
}
