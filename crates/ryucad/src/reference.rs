//! Resolving references against the bodies of features built before: a
//! name resolves to exactly the face, edge or vertex that carries it, or
//! the feature holding the reference fails. Never a nearest match.

use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Results};
use ryukei::geom::{Curve, Point3, Surface, Vector3};
use ryukei::topo::{EntityId, Sense};

/// The feature built from `r`, and the entity its name carries. `owner`
/// holds the reference in its `role`; `order` is the part's list as it
/// is, or as a write would leave it.
pub fn resolve<'a>(
    part: &Part,
    order: &[Id],
    owner: &Id,
    role: &str,
    r: &Reference,
    results: &'a dyn Results,
) -> Result<(&'a Built, EntityId)> {
    let text = r.text(&part.id);
    let fail = |code, msg: String| {
        Error::new(code, format!("{owner}: its {role} {text} {msg}"))
            .with("feature", owner)
            .with("role", role)
            .with("reference", &text)
    };
    let at = |x: &Id| order.iter().position(|o| o == x);
    match (at(&r.feature), at(owner)) {
        (None, _) => {
            return Err(fail(
                Code::NotFound,
                format!("names {}, which the part doesn't list", r.feature),
            ));
        }
        (Some(i), Some(me)) if i >= me => {
            return Err(fail(
                Code::ReferenceForward,
                format!("names {}, which doesn't come before it", r.feature),
            ));
        }
        _ => {}
    }
    match results.result(&r.feature) {
        None => Err(fail(
            Code::NotFound,
            format!("names {}, which hasn't been built", r.feature),
        )),
        Some(Err(e)) => Err(fail(
            Code::DependencyFailed,
            format!("names {}, which failed to rebuild", r.feature),
        )
        .with("dependency", &r.feature)
        .with("cause", e.code)),
        Some(Ok(o)) => {
            let Some(b) = o.body() else {
                return Err(fail(
                    Code::ReferenceWrongKind,
                    format!(
                        "names {}, a sketch, which has no faces, edges or vertices",
                        r.feature
                    ),
                ));
            };
            match b.body.by_name(r.name) {
                Some(e) => Ok((b, e)),
                None => Err(fail(
                    Code::ReferenceUnresolved,
                    format!("is gone from {}'s body", r.feature),
                )
                .with("dependency", &r.feature)),
            }
        }
    }
}

/// What a resolved reference was, for messages: `face`, `edge`, `vertex`.
pub fn kind(e: EntityId) -> &'static str {
    match e {
        EntityId::Face(_) => "face",
        EntityId::Edge(_) => "edge",
        EntityId::Vertex(_) => "vertex",
        _ => "entity",
    }
}

/// `reference_wrong_kind` for a reference in `role` that resolved to `e`
/// where `want` goes.
pub fn wrong_kind(
    part: &Part,
    owner: &Id,
    role: &str,
    r: &Reference,
    e: &str,
    want: &str,
) -> Error {
    let text = r.text(&part.id);
    Error::new(
        Code::ReferenceWrongKind,
        format!("{owner}: its {role} {text} is a {e}, not {want}"),
    )
    .with("feature", owner)
    .with("role", role)
    .with("reference", text)
    .with("kind", e)
}

/// A planar face's plane: a point on it and its outward normal.
pub fn plane(
    part: &Part,
    owner: &Id,
    role: &str,
    r: &Reference,
    (b, e): (&Built, EntityId),
) -> Result<(Point3, Vector3)> {
    let wrong = |what: &str| wrong_kind(part, owner, role, r, what, "a planar face");
    let EntityId::Face(f) = e else {
        return Err(wrong(kind(e)));
    };
    let face = b.body.face(f).expect("a live face");
    match b.body.surface(face.surface) {
        Some(s @ Surface::Plane(_)) => {
            let frame = s.frame().expect("a plane has a frame");
            let n = frame.z().get();
            let n = if face.sense == Sense::Reversed { -n } else { n };
            Ok((frame.origin(), n))
        }
        _ => Err(wrong("curved face")),
    }
}

/// A vertex's point.
pub fn point(
    part: &Part,
    owner: &Id,
    role: &str,
    r: &Reference,
    (b, e): (&Built, EntityId),
) -> Result<Point3> {
    match e {
        EntityId::Vertex(v) => Ok(b.body.vertex(v).expect("a live vertex").point),
        _ => Err(wrong_kind(part, owner, role, r, kind(e), "a vertex")),
    }
}

/// A straight edge's start and end points, in the edge's direction.
pub fn line(
    part: &Part,
    owner: &Id,
    role: &str,
    r: &Reference,
    (b, e): (&Built, EntityId),
) -> Result<(Point3, Point3)> {
    let wrong = |what: &str| wrong_kind(part, owner, role, r, what, "a straight edge");
    let EntityId::Edge(id) = e else {
        return Err(wrong(kind(e)));
    };
    let edge = b.body.edge(id).expect("a live edge");
    match b.body.curve(edge.curve) {
        Some(Curve::Line(_)) => {
            let at = |v| b.body.vertex(v).expect("a live vertex").point;
            Ok((at(edge.start), at(edge.end)))
        }
        _ => Err(wrong("curved edge")),
    }
}
