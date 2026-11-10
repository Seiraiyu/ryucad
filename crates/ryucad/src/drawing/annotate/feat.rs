//! What an annotation's entity is in its view: resolved by stable name in
//! what the view draws, its geometry in the model, and whether the view
//! shows it. A name that is gone is `drawing_reference_unresolved` (with
//! `was`, `removed_by` and `candidates` where the session knows them), one
//! the view doesn't draw (hidden, or behind a section's cut)
//! `drawing_reference_not_shown`. Never a nearest match.

use super::super::{Plan, Source};
use crate::doc::assembly::Rev;
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use ryukei::geom::{Curve, Surface};
use ryukei::topo::{Body, EntityId, Sense};
use serde_json::{Value, json};

pub(crate) type V3 = [f64; 3];

pub(crate) fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// An entity's geometry in the model (where its view places it).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Feat {
    Point(V3),
    /// A straight edge from its start to its end.
    Segment(V3, V3),
    /// A circular edge: its centre, unit axis and radius.
    Circle {
        c: V3,
        n: V3,
        r: f64,
    },
    /// A planar face: a point of it, its outward unit normal, and its
    /// vertices.
    Plane {
        p: V3,
        n: V3,
        pts: Vec<V3>,
    },
    /// A cylindrical or conical face: a point of its axis, the axis's unit
    /// direction, the cylinder's radius (none for a cone), its vertices,
    /// and its circular edges (centre, radius).
    Round {
        o: V3,
        d: V3,
        r: Option<f64>,
        pts: Vec<V3>,
        circles: Vec<(V3, f64)>,
    },
    Sphere {
        c: V3,
        r: f64,
    },
    /// A datum axis: a point and a unit direction, unbounded.
    Line {
        o: V3,
        d: V3,
    },
    /// A datum plane: a point and a unit normal, unbounded.
    DatumPlane {
        p: V3,
        n: V3,
    },
    /// Anything else (a curved edge, a spline face): named, drawn, never
    /// measured.
    Other(&'static str),
}

impl Feat {
    /// What it is, in words.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Feat::Point(_) => "point",
            Feat::Segment(..) => "straight edge",
            Feat::Circle { .. } => "circular edge",
            Feat::Plane { .. } => "planar face",
            Feat::Round { r: Some(_), .. } => "cylindrical face",
            Feat::Round { r: None, .. } => "conical face",
            Feat::Sphere { .. } => "spherical face",
            Feat::Line { .. } => "datum axis",
            Feat::DatumPlane { .. } => "datum plane",
            Feat::Other(k) => k,
        }
    }

    /// A point of it: where a leader may end when nothing drawn says.
    pub(crate) fn point(&self) -> V3 {
        match self {
            Feat::Point(p) => *p,
            Feat::Segment(a, b) => mul(add(*a, *b), 0.5),
            Feat::Circle { c, .. } | Feat::Sphere { c, .. } => *c,
            Feat::Plane { p, pts, .. } | Feat::Round { o: p, pts, .. } => {
                if pts.is_empty() {
                    *p
                } else {
                    mul(
                        pts.iter().fold([0.0; 3], |s, q| add(s, *q)),
                        1.0 / pts.len() as f64,
                    )
                }
            }
            Feat::Line { o, .. } => *o,
            Feat::DatumPlane { p, .. } => *p,
            Feat::Other(_) => [0.0; 3],
        }
    }
}

fn v(p: ryukei::geom::Point3) -> V3 {
    [p.x, p.y, p.z]
}

fn w(p: ryukei::geom::Vector3) -> V3 {
    [p.x, p.y, p.z]
}

/// The face's vertices and circular edges.
fn face_parts(body: &Body, f: ryukei::topo::FaceId) -> (Vec<V3>, Vec<(V3, V3, f64)>) {
    let mut pts = Vec::new();
    let mut circles = Vec::new();
    for l in body.face_loops(f) {
        for fin in body.loop_fins(l) {
            let Some(e) = body.fin(fin).and_then(|x| body.edge(x.edge)) else {
                continue;
            };
            if let Some(vx) = body.vertex(e.start) {
                pts.push(v(vx.point));
            }
            if let Some(Curve::Circle(c)) = body.curve(e.curve) {
                let fr = c.frame();
                circles.push((v(fr.origin()), w(fr.z().get()), c.radius()));
            }
        }
    }
    (pts, circles)
}

/// Entity `e` of `body` placed by `pose`.
pub(crate) fn of(body: &Body, e: EntityId, pose: &ryukei::geom::Rigid) -> Feat {
    let p = |q: V3| {
        let r = pose.apply_point(ryukei::geom::Point3::new(q[0], q[1], q[2]));
        [r.x, r.y, r.z]
    };
    let d = |q: V3| {
        let r = pose.apply_vector(ryukei::geom::Vector3::new(q[0], q[1], q[2]));
        [r.x, r.y, r.z]
    };
    match e {
        EntityId::Vertex(id) => match body.vertex(id) {
            Some(x) => Feat::Point(p(v(x.point))),
            None => Feat::Other("entity"),
        },
        EntityId::Edge(id) => {
            let Some(edge) = body.edge(id) else {
                return Feat::Other("entity");
            };
            let at = |x| body.vertex(x).map(|x| p(v(x.point)));
            match body.curve(edge.curve) {
                Some(Curve::Line(_)) => match (at(edge.start), at(edge.end)) {
                    (Some(a), Some(b)) => Feat::Segment(a, b),
                    _ => Feat::Other("straight edge"),
                },
                Some(Curve::Circle(c)) => {
                    let fr = c.frame();
                    Feat::Circle {
                        c: p(v(fr.origin())),
                        n: d(w(fr.z().get())),
                        r: c.radius(),
                    }
                }
                _ => Feat::Other("curved edge"),
            }
        }
        EntityId::Face(id) => {
            let Some(face) = body.face(id) else {
                return Feat::Other("entity");
            };
            let (pts, circles) = face_parts(body, id);
            let pts: Vec<V3> = pts.into_iter().map(p).collect();
            let circles: Vec<(V3, f64)> = circles.into_iter().map(|(c, _, r)| (p(c), r)).collect();
            match body.surface(face.surface) {
                Some(s @ Surface::Plane(_)) => {
                    let fr = s.frame().expect("a plane has a frame");
                    let n = w(fr.z().get());
                    let n = if face.sense == Sense::Reversed {
                        mul(n, -1.0)
                    } else {
                        n
                    };
                    Feat::Plane {
                        p: p(v(fr.origin())),
                        n: d(n),
                        pts,
                    }
                }
                Some(s @ Surface::Cylinder(c)) => {
                    let fr = s.frame().expect("a cylinder has a frame");
                    Feat::Round {
                        o: p(v(fr.origin())),
                        d: d(w(fr.z().get())),
                        r: Some(c.radius()),
                        pts,
                        circles,
                    }
                }
                Some(s @ Surface::Cone(_)) => {
                    let fr = s.frame().expect("a cone has a frame");
                    Feat::Round {
                        o: p(v(fr.origin())),
                        d: d(w(fr.z().get())),
                        r: None,
                        pts,
                        circles,
                    }
                }
                Some(s @ Surface::Sphere(sp)) => Feat::Sphere {
                    c: p(v(s.frame().expect("a sphere has a frame").origin())),
                    r: sp.radius(),
                },
                _ => Feat::Other("curved face"),
            }
        }
        _ => Feat::Other("entity"),
    }
}

/// Whether a view shows an entity: drawn visible, drawn only hidden, or
/// not at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    Visible,
    Hidden,
    No,
}

/// An annotation's entity resolved in its view.
#[derive(Clone, Debug)]
pub(crate) struct Resolved {
    /// The text as the file has it.
    pub text: String,
    /// Its source solid in the view, where it is a solid's entity.
    pub body: Option<usize>,
    /// Its name, where it is a solid's entity.
    pub name: Option<String>,
    pub feat: Feat,
    pub shown: Shown,
    /// The paths the view drew of it (an edge, a face's silhouettes and
    /// edges), in the view's frame: where a leader meets it.
    pub paths: Vec<ryu_draw::Path>,
}

/// The names of the edges bounding face `f`, and of the edges at
/// vertex `v`: what a view draws of a face or a vertex.
fn drawn_names(body: &Body, e: EntityId) -> Vec<String> {
    let mut out = Vec::new();
    match e {
        EntityId::Face(f) => {
            if let Some(n) = body.name_of(e) {
                out.push(n.to_string());
            }
            for l in body.face_loops(f) {
                for fin in body.loop_fins(l) {
                    if let Some(x) = body.fin(fin)
                        && let Some(n) = body.name_of(EntityId::Edge(x.edge))
                    {
                        out.push(n.to_string());
                    }
                }
            }
        }
        EntityId::Vertex(vx) => {
            for edge in body.vertex_edges(vx) {
                if let Some(n) = body.name_of(EntityId::Edge(edge)) {
                    out.push(n.to_string());
                }
            }
        }
        e => {
            if let Some(n) = body.name_of(e) {
                out.push(n.to_string());
            }
        }
    }
    out
}

fn gone(role: &str, text: &str, message: String) -> Error {
    Error::new(Code::DrawingReferenceUnresolved, message)
        .with("role", role)
        .with("reference", text)
}

/// Resolves `text`, named in `role` of an annotation on `plan`'s view
/// (whose source is `src`, and whose drawing drew `named`).
pub(super) fn resolve(
    session: &mut Session,
    plan: &Plan,
    src: &Source,
    named: Option<&std::collections::BTreeMap<(usize, String), super::super::Named>>,
    role: &str,
    text: &str,
) -> Result<Resolved> {
    // A datum of the view's part, by its ID.
    if !text.contains('/') {
        let reference = format!("{}/{text}", plan.of);
        return match crate::geometry::locate_at(session, &reference, &plan.rev) {
            Ok(Some(l)) => {
                let feat = match l.geom {
                    crate::geometry::Geom::Point(p) => Feat::Point(v(p)),
                    crate::geometry::Geom::Axis(a) => Feat::Line {
                        o: v(a.origin),
                        d: w(a.dir.get()),
                    },
                    crate::geometry::Geom::Plane(p) => Feat::DatumPlane {
                        p: v(p.point),
                        n: w(p.normal),
                    },
                    crate::geometry::Geom::Curve(_) => Feat::Other("path"),
                };
                Ok(Resolved {
                    text: text.to_owned(),
                    body: None,
                    name: None,
                    feat,
                    shown: Shown::Visible,
                    paths: Vec::new(),
                })
            }
            Ok(None) => Err(gone(
                role,
                text,
                format!("{text} is no datum of {}", plan.of),
            )),
            Err(e) => Err(gone(role, text, e.message.clone()).with("error", &e)),
        };
    }
    let (prefix, name) = text.rsplit_once('/').expect("has a slash");
    let label = format!("{}/{prefix}", plan.of);
    let Some(i) = src.iter().position(|(_, _, l)| *l == label) else {
        let e = gone(
            role,
            text,
            format!(
                "{text} names a solid view {} doesn't draw ({label})",
                plan.key
            ),
        )
        .with(
            "solids",
            src.iter()
                .map(|(_, _, l)| l.split_once('/').map_or(l.as_str(), |x| x.1).to_owned())
                .collect::<Vec<_>>(),
        );
        return Err(e);
    };
    let (built, pose, _) = &src[i];
    let n = crate::doc::reference::parse_name(name);
    let Some(entity) = n.and_then(|n| built.body.by_name(n)) else {
        // Behind a section's cut: the uncut model has it.
        if matches!(plan.extra, super::super::Extra::Section(_)) {
            let whole = super::super::source(session, &plan.of, &plan.rev)?;
            if whole
                .iter()
                .any(|(b, _, l)| *l == label && n.and_then(|n| b.body.by_name(n)).is_some())
            {
                return Err(Error::new(
                    Code::DrawingReferenceNotShown,
                    format!("{text} lies in what section view {} cuts away", plan.key),
                )
                .with("role", role)
                .with("reference", text)
                .with("reason", "cut"));
            }
        }
        let mut e = gone(
            role,
            text,
            format!("{text}: {} no longer has that entity", label),
        );
        if plan.rev == Rev::Working {
            for (k, x) in explain(session, &plan.of, prefix, name) {
                e = e.with(&k, x);
            }
        }
        return Err(e);
    };
    let feat = of(&built.body, entity, pose);
    let names = drawn_names(&built.body, entity);
    let mut shown = Shown::No;
    let mut paths = Vec::new();
    if let Some(named) = named {
        for n in &names {
            if let Some(d) = named.get(&(i, n.clone())) {
                if d.visible {
                    shown = Shown::Visible;
                } else if d.hidden && shown == Shown::No {
                    shown = Shown::Hidden;
                }
                if matches!(entity, EntityId::Vertex(_)) {
                    continue;
                }
                paths.extend(d.paths.iter().cloned());
            }
        }
    } else {
        // A view that draws no pieces (a flat pattern): nothing shown.
        shown = Shown::No;
    }
    if shown == Shown::No {
        return Err(Error::new(
            Code::DrawingReferenceNotShown,
            format!("view {} doesn't draw {text}", plan.key),
        )
        .with("role", role)
        .with("reference", text)
        .with("reason", "not_drawn"));
    }
    Ok(Resolved {
        text: text.to_owned(),
        body: Some(i),
        name: Some(name.to_owned()),
        feat,
        shown,
        paths,
    })
}

/// What a name the working model lost was (`was`), what took it
/// (`removed_by`) and what it became (`candidates`, written as the
/// reference was), for a part's own solid or an occurrence's.
fn explain(session: &mut Session, of: &Id, prefix: &str, name: &str) -> Vec<(String, Value)> {
    let steps: Vec<&str> = prefix.split('/').collect();
    let (solid, path) = steps.split_last().expect("a solid");
    let Ok(solid) = Id::parse(solid, "solid") else {
        return Vec::new();
    };
    // The part whose solid it is: the view's own, or the occurrence's.
    let part = if path.is_empty() {
        of.clone()
    } else {
        let Ok(model) = session.model(of, &Rev::Working) else {
            return Vec::new();
        };
        let want: Vec<String> = path.iter().map(|s| (*s).to_owned()).collect();
        match model
            .occurrences
            .iter()
            .find(|o| o.path.iter().map(Id::to_string).collect::<Vec<_>>() == want)
        {
            Some(o) => o.part.clone(),
            None => return Vec::new(),
        }
    };
    let _ = session.rebuilt(&part);
    let mut out = Vec::new();
    let m = session.explain_gone(&part, &solid, name);
    for k in ["was", "removed_by"] {
        if let Some(x) = m.get(k) {
            out.push((k.to_owned(), x.clone()));
        }
    }
    if let Some(Value::Array(c)) = m.get("candidates") {
        out.push((
            "candidates".to_owned(),
            json!(
                c.iter()
                    .filter_map(Value::as_str)
                    .map(|n| format!("{prefix}/{n}"))
                    .collect::<Vec<_>>()
            ),
        ));
    }
    out
}
