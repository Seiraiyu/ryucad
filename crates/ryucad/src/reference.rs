//! Resolving references against the solids of features built before. A
//! reference names a solid by the feature that founded it; its name
//! resolves to exactly the face, edge or vertex that carries it in the
//! solid as the features listed before the referring one left it (its
//! view, see [`crate::solid`]), or the feature holding the reference
//! fails. Never a nearest match.

use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Results};
use ryukei::geom::{Curve, Point3, Surface, Vector3};
use ryukei::topo::{EntityId, Sense};

/// The solid `r` names, as `owner` sees it, and the entity its name
/// carries. `owner` holds the reference in its `role`; `order` is the
/// part's list as it is, or as a write would leave it.
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
    let solids = crate::solid::Solids::of_order(part, order);
    if let Some(by) = solids.consumed_by(&r.feature, owner) {
        return Err(fail(
            Code::BodyConsumed,
            format!(
                "names {}, which the boolean {by} consumed: refer to its target instead",
                r.feature
            ),
        )
        .with("solid", &r.feature)
        .with("consumed_by", by));
    }
    // The feature whose result holds the solid as `owner` sees it.
    let read = solids.view(&r.feature, owner).unwrap_or(&r.feature);
    match results.result(read) {
        None => Err(fail(
            Code::NotFound,
            format!("names {}, which hasn't been built", r.feature),
        )),
        Some(Err(e)) => Err(fail(
            Code::DependencyFailed,
            format!("names {}, which failed to rebuild", r.feature),
        )
        .with("dependency", read)
        .with("cause", e.code)),
        Some(Ok(o)) if o.sketch().is_some() => Err(fail(
            Code::ReferenceWrongKind,
            format!(
                "names {}, a sketch, which has no faces, edges or vertices",
                r.feature
            ),
        )),
        Some(Ok(o)) => {
            let Some(b) = o.solid(&r.feature) else {
                return Err(
                    crate::rebuild::not_a_solid(part, &solids, owner, role, &r.feature)
                        .with("reference", &text),
                );
            };
            match b.body.by_name(r.name) {
                Some(e) => Ok((b, e)),
                None => Err(fail(
                    Code::ReferenceUnresolved,
                    format!("is gone from solid {}", r.feature),
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

#[cfg(test)]
mod tests {
    use crate::error::Code;
    use crate::server::tests::call;
    use crate::session::Session;
    use crate::sketch::tests::{edge_between, face_along, rectangle_ops, vertex_at, with_box};
    use serde_json::{Value, json};

    /// The box `b` with the rectangle sketch `s` on xy beside it, and the
    /// box's body in `inspect`.
    fn with_box_and_sketch() -> (tempfile::TempDir, Session, Value) {
        let (dir, mut s, body) = with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": rectangle_ops()}),
        )
        .unwrap();
        (dir, s, body)
    }

    /// The top of extrude `e`'s box.
    fn top(s: &mut Session) -> f64 {
        call(s, "inspect", json!({"part": "p", "feature": "e"})).unwrap()["bodies"][0]["bounds"]
            ["max"][2]
            .as_f64()
            .unwrap()
    }

    #[test]
    fn up_to_a_face_or_a_vertex() {
        let (_dir, mut s, body) = with_box_and_sketch();
        let face = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"type": "up_to", "target": face}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.005).abs() < 1e-15);
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "e"})).unwrap();
        assert_eq!(show["extent"], json!({"type": "up_to", "target": face}));
        // An offset alone keeps the target.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "e", "extent": {"offset": "-1 mm"}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.004).abs() < 1e-15);
        // The target moves; the extrude follows.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"z": 8}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.007).abs() < 1e-15);
        // Up to a vertex, keeping the offset.
        let corner = format!("p/b/{}", vertex_at(&body, [0.0, 0.0, 0.005]));
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "e", "extent": {"type": "up_to", "target": corner}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.007).abs() < 1e-15);
        // A new type starts afresh.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "e", "extent": {"type": "blind", "length": 2}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.002).abs() < 1e-15);
    }

    #[test]
    fn revolve_about_an_edge() {
        let (_dir, mut s, body) = with_box();
        // On zx the sketch's x is the part's z and its y the part's x, so
        // the box's bottom edge along x is the sketch's y axis.
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
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.0]),
            vertex_at(&body, [0.02, 0.0, 0.0]),
        );
        let along_x = edge_between(&body, &a, &b);
        // Which way the edge runs decides which way the turn goes.
        let starts_at_origin = {
            let e = body["edges"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["name"] == along_x.as_str())
                .unwrap();
            e["start"] == a.as_str()
        };
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s",
                   "axis": format!("p/b/{along_x}"), "angle": 90}),
        )
        .unwrap();
        let v = call(&mut s, "inspect", json!({"part": "p", "feature": "r"})).unwrap();
        // A quarter turn about x takes the sketch's far corner, at z = 20 mm,
        // to y = -20 mm (or +20 mm, turning the other way), z = 0.
        let want = if starts_at_origin { -0.02 } else { 0.02 };
        let swung = v["bodies"][0]["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| {
                let p = &x["point"];
                (p[1].as_f64().unwrap() - want).abs() < 1e-9 && p[2].as_f64().unwrap().abs() < 1e-9
            });
        assert!(swung, "{}", v["bodies"][0]["vertices"]);
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "r"})).unwrap();
        assert_eq!(show["axis"], format!("p/b/{along_x}"));
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
    }

    #[test]
    fn axes_that_wont_do() {
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
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "sketch": "s", "axis": top, "angle": 90}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind);
        // The back bottom edge is 10 mm behind the sketch's plane.
        let (a, b) = (
            vertex_at(&body, [0.0, 0.01, 0.0]),
            vertex_at(&body, [0.02, 0.01, 0.0]),
        );
        let back = format!("p/b/{}", edge_between(&body, &a, &b));
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s", "axis": back,
                   "angle": 90}),
        )
        .unwrap();
        let e = call(&mut s, "rebuild", json!({})).unwrap_err();
        let f = &e.details["parts"][0]["features"][2];
        assert_eq!(f["error"]["code"], "invalid_extent", "{f}");
        assert_eq!(f["error"]["details"]["role"], "axis");
    }

    #[test]
    fn an_unresolved_reference_says_what_it_was() {
        let (_dir, mut s) = crate::sketch::tests::with_sketch();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": rectangle_ops()}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"length": 5}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "p", "feature": "e"})).unwrap()["bodies"]
            [0]
        .clone();
        let from_l2 = json!({"feature": "e", "sketch": "s", "entity": "l2"});
        let side = body["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["origin"] == from_l2)
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": format!("p/e/{side}")}),
        )
        .unwrap();
        // l2 gives way to l5: the face it made goes, and t with it.
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "remove", "id": "l2"},
                {"op": "add", "id": "l5", "type": "line", "start": "p2", "end": "p3"}
            ]}),
        )
        .unwrap();
        let e = call(&mut s, "rebuild", json!({})).unwrap_err();
        let t = &e.details["parts"][0]["features"][2]["error"];
        assert_eq!(t["code"], "reference_unresolved", "{t}");
        assert_eq!(t["details"]["role"], "plane");
        assert_eq!(
            t["details"]["was"],
            json!({"kind": "face", "from": from_l2})
        );
        // The extrude made it and makes it no more.
        assert_eq!(t["details"]["removed_by"], "e");
        // An edit that names it again is refused the same way.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "face": format!("p/e/{side}")}),
        )
        .unwrap_err();
        assert_eq!(e.details["was"]["from"], from_l2);
    }

    #[test]
    fn a_name_a_later_feature_took_says_which() {
        let (_dir, mut s, body) = crate::sketch::tests::with_box();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        // A cut taking the box's top millimetre.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "c", "operation": "cut",
                   "params": {"x": 30, "y": 30, "z": 2},
                   "placement": {"origin": [-5, -5, 4]}}),
        )
        .unwrap();
        // A sketch on the top, after the cut: the name is gone, and the
        // cut took it.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": top}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceUnresolved, "{e}");
        assert_eq!(e.details["removed_by"], "c");
        assert_eq!(
            e.details["was"],
            json!({"kind": "face", "from": {"feature": "b"}})
        );
        // Before the cut it is still there.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": top, "after": "b"}),
        )
        .unwrap();
    }

    #[test]
    fn targets_that_wont_do() {
        let (_dir, mut s, body) = with_box_and_sketch();
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.005]),
            vertex_at(&body, [0.02, 0.0, 0.005]),
        );
        let edge = format!("p/b/{}", edge_between(&body, &a, &b));
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "sketch": "s",
                   "extent": {"type": "up_to", "target": edge}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind);
        // A side, the sketch's own plane, and a face behind it: only
        // rebuild can tell.
        let mut id = 0;
        for (target, reverse) in [
            (face_along(&body, [0.0, -1.0, 0.0]), false),
            (face_along(&body, [0.0, 0.0, -1.0]), false),
            (face_along(&body, [0.0, 0.0, 1.0]), true),
        ] {
            id += 1;
            call(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "extrude", "id": format!("e{id}"), "sketch": "s",
                       "reverse": reverse,
                       "extent": {"type": "up_to", "target": format!("p/b/{target}")}}),
            )
            .unwrap();
        }
        let e = call(&mut s, "rebuild", json!({})).unwrap_err();
        for f in &e.details["parts"][0]["features"].as_array().unwrap()[2..] {
            assert_eq!(f["error"]["code"], "invalid_extent", "{f}");
            assert_eq!(f["error"]["details"]["role"], "target", "{f}");
        }
    }
}
