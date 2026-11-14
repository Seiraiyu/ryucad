//! Where a hole at places drills (design 533, Placement without
//! sketches): each place's mouth, the direction it drills, and whether
//! it starts flush on a planar face or reaches back past its targets.
//! Which face a point lies on, and whether a point is inside a face,
//! decide what is built, so they use ryukei's certified point
//! classification (`point_in_face`), never a float sign.

use crate::doc::geometry::GeomRef;
use crate::doc::hole::{Place, PlaceKind};
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::geometry::{At, Want, one};
use crate::rebuild::Built;
use crate::reference::Follow;
use ryukei::geom::{Point2, Point3, Surface, Vector3};
use ryukei::intersect::{Place as InFace, face_box, point_in_face};
use ryukei::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use ryukei::topo::{Body, EntityId, FaceId, Sense};

/// Where one place's hole starts, which way it drills (a unit vector),
/// and whether it starts flush on a planar face, drilled along its
/// inward normal; any other mouth reaches back past the targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mouth {
    pub at: Point3,
    pub down: Vector3,
    pub flush: bool,
}

/// How far a point may lie from a face and still be on it.
const ON: f64 = 10.0 * LINEAR_RESOLUTION;

/// The mouth of place `key` of the hole `at.owner`, in its `targets`.
pub fn mouth(
    at: &At<'_>,
    key: &Id,
    p: &Place,
    targets: &[&Built],
    work: &ryukei::work::Work,
) -> Result<Mouth> {
    let role = |k: &str| format!("places.{key}.{k}");
    let m = match &p.kind {
        PlaceKind::At { at: point, face } => {
            at_point(at, key, &role, point, face.as_ref(), targets)?
        }
        PlaceKind::Edges { face, from } => from_edges(at, key, &role, face, from)?,
        PlaceKind::Coaxial { of } => {
            return coaxial(at, key, &role, of, p.reverse, targets, work);
        }
    };
    let mut down = match &p.direction {
        Some(d) => one(at, &role("direction"), d, Want::Axis)?.axis().dir.get(),
        None => m.down,
    };
    if p.reverse {
        down = -down;
    }
    let along = down.cross(m.down).length() <= ANGULAR_RESOLUTION && down.dot(m.down) > 0.0;
    Ok(Mouth {
        at: m.at,
        down,
        flush: m.flush && along,
    })
}

pub(crate) fn off_face(at: &At<'_>, key: &Id, msg: String, distance: Option<f64>) -> Error {
    let mut e = Error::new(
        Code::HolePlaceOffFace,
        format!("hole {}: place {key} {msg}", at.owner),
    )
    .with("feature", at.owner)
    .with("place", key)
    .with("pointer", format!("/places/{key}"));
    if let Some(d) = distance {
        e = e.with("distance_m", d);
    }
    e
}

/// The face `g` names (in `role`), followed as its carrier.
pub(crate) fn face_of<'a>(at: &At<'a>, role: &str, g: &GeomRef) -> Result<(&'a Built, FaceId)> {
    let Some(r) = g.reference() else {
        return Err(at.fail(
            Code::ReferenceWrongKind,
            role,
            g,
            "is no face of a solid".into(),
        ));
    };
    let (b, e) = crate::reference::resolve(
        at.part,
        at.order,
        at.owner,
        role,
        Follow::Carrier,
        r,
        at.results,
    )?;
    match e {
        EntityId::Face(f) => Ok((b, f)),
        other => Err(crate::reference::wrong_kind(
            at.part,
            at.owner,
            role,
            r,
            crate::reference::kind(other),
            "a face",
        )),
    }
}

/// The face's surface, its point nearest `p`, the outward normal there,
/// and whether that point is inside the face (certified; `None` when
/// the kernel can't tell).
pub(crate) fn foot(body: &Body, f: FaceId, p: Point3) -> (Surface, Point3, Vector3, Option<bool>) {
    let face = body.face(f).expect("a live face");
    let s = body.surface(face.surface).expect("a live surface").clone();
    let pr = s.project(p);
    let (u, v) = pr.param;
    let n = s.normal(u, v).map_or(Vector3::ZERO, |n| n.get());
    let n = if face.sense == Sense::Reversed { -n } else { n };
    let inside = match point_in_face(body, f, Point2::new(u, v)) {
        Ok(InFace::Outside) => Some(false),
        Ok(_) => Some(true),
        Err(_) => None,
    };
    (s, pr.point, n, inside)
}

/// A place at a point: on the planar face named, radial on the
/// cylindrical or conical face named, or on the one face of the targets
/// it lies on.
fn at_point(
    at: &At<'_>,
    key: &Id,
    role: &dyn Fn(&str) -> String,
    point: &GeomRef,
    face: Option<&GeomRef>,
    targets: &[&Built],
) -> Result<Mouth> {
    let p = one(at, &role("at"), point, Want::Point)?.point();
    let (body, f) = match face {
        Some(g) => {
            let (b, f) = face_of(at, &role("face"), g)?;
            (&b.body, f)
        }
        None => {
            let on = faces_at(targets, p);
            match on.as_slice() {
                [(b, f)] => (*b, *f),
                [] => {
                    return Err(off_face(
                        at,
                        key,
                        "lies on no face of the hole's targets: name the face to drill".into(),
                        None,
                    ));
                }
                many => {
                    let names: Vec<String> = many
                        .iter()
                        .map(|(b, f)| format!("{}", b.face(*f).expect("live").name))
                        .collect();
                    return Err(Error::new(
                        Code::HolePlaceAmbiguous,
                        format!(
                            "hole {}: place {key} lies on {} faces of its targets: name one as face",
                            at.owner,
                            many.len()
                        ),
                    )
                    .with("feature", at.owner)
                    .with("place", key)
                    .with("faces", names)
                    .with("pointer", format!("/places/{key}")));
                }
            }
        }
    };
    let (s, foot_at, n, inside) = foot(body, f, p);
    match s {
        Surface::Plane(_) => {
            let d = (p - foot_at).length();
            if d > ON {
                return Err(off_face(at, key, format!("is {d} m off its face"), Some(d)));
            }
            if inside == Some(false) {
                return Err(off_face(at, key, "is outside its face".into(), Some(0.0)));
            }
            Ok(Mouth {
                at: p,
                down: -n,
                flush: true,
            })
        }
        Surface::Cylinder(_) | Surface::Cone(_) if face.is_some() => {
            if inside == Some(false) {
                return Err(off_face(
                    at,
                    key,
                    "has its foot outside the face it drills radially".into(),
                    Some((p - foot_at).length()),
                ));
            }
            Ok(Mouth {
                at: foot_at,
                down: -n,
                flush: false,
            })
        }
        _ => Err(at.fail(
            Code::ReferenceWrongKind,
            &role("face"),
            face.unwrap_or(point),
            "is no planar, cylindrical or conical face".into(),
        )),
    }
}

/// The faces of the targets `p` lies on (within `ON`, and not outside
/// them), in target then face order.
pub(crate) fn faces_at<'a>(targets: &[&'a Built], p: Point3) -> Vec<(&'a Body, FaceId)> {
    let pad = Vector3::new(ON, ON, ON);
    let mut out = Vec::new();
    for t in targets {
        for f in t.body.faces() {
            let bx = face_box(&t.body, f);
            let (lo, hi) = (bx.min() - pad, bx.max() + pad);
            if p.x < lo.x || p.y < lo.y || p.z < lo.z || p.x > hi.x || p.y > hi.y || p.z > hi.z {
                continue;
            }
            let (_, q, _, inside) = foot(&t.body, f, p);
            if (p - q).length() <= ON && inside != Some(false) {
                out.push((&t.body, f));
            }
        }
    }
    out
}

/// A place inside planar face `face`, `from[i].1` from each straight edge
/// `from[i].0`'s line, on the face's side of it.
fn from_edges(
    at: &At<'_>,
    key: &Id,
    role: &dyn Fn(&str) -> String,
    face: &GeomRef,
    from: &[(GeomRef, crate::units::Quantity); 2],
) -> Result<Mouth> {
    let (b, f) = face_of(at, &role("face"), face)?;
    let r = face.reference().expect("face_of took a reference");
    let (o, n) =
        crate::reference::plane(at.part, at.owner, &role("face"), r, (b, EntityId::Face(f)))?;
    let size = {
        let bx = face_box(&b.body, f);
        (bx.max() - bx.min()).length().max(1e-6)
    };
    // Each edge's line in the plane, and its unit normal into the face.
    let mut lines = Vec::with_capacity(2);
    for (i, (g, d)) in from.iter().enumerate() {
        let er = format!("{}.{i}", role("from"));
        let Some(rr) = g.reference() else {
            return Err(at.fail(
                Code::ReferenceWrongKind,
                &er,
                g,
                "is no straight edge".into(),
            ));
        };
        let hit = crate::reference::resolve(
            at.part,
            at.order,
            at.owner,
            &er,
            Follow::One,
            rr,
            at.results,
        )?;
        let (a0, a1) = crate::reference::line(at.part, at.owner, &er, rr, hit)?;
        let along = a1 - a0;
        // Into the plane: drop any part along its normal.
        let along = along - n * along.dot(n);
        let len = along.length();
        if len <= LINEAR_RESOLUTION {
            return Err(at.fail(
                Code::ReferenceWrongKind,
                &er,
                g,
                "doesn't run along the face's plane".into(),
            ));
        }
        let t = along * (1.0 / len);
        let mut side = n.cross(t);
        // Which side is the face's: probe just off the edge's middle.
        let mid = a0 + (a1 - a0) * 0.5;
        let probe = |s: Vector3| foot(&b.body, f, mid + s * (size * 1e-3)).3;
        match (probe(side), probe(-side)) {
            (Some(true), _) => {}
            (_, Some(true)) => side = -side,
            _ => {
                return Err(off_face(
                    at,
                    key,
                    format!("can't tell which side of its edge {i} the face is"),
                    None,
                ));
            }
        }
        lines.push((a0, t, side, d.meters()));
    }
    let (a, ta, na, da) = lines[0];
    let (b0, tb, nb, db) = lines[1];
    if ta.cross(tb).length() <= ANGULAR_RESOLUTION {
        return Err(Error::new(
            Code::InvalidParam,
            format!("hole {}: place {key}'s two edges are parallel", at.owner),
        )
        .with("feature", at.owner)
        .with("place", key)
        .with("reason", "edges_parallel")
        .with("pointer", format!("/places/{key}/from")));
    }
    // p = o + x·u + y·v in the plane, with (p − a)·na = da and
    // (p − b)·nb = db.
    let frame = ryukei::geom::Frame::new(o, n, ta).map_err(|e| {
        Error::new(Code::Internal, "a face's plane that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let (u, v) = (frame.x().get(), frame.y().get());
    let (m11, m12, r1) = (na.dot(u), na.dot(v), da - (o - a).dot(na));
    let (m21, m22, r2) = (nb.dot(u), nb.dot(v), db - (o - b0).dot(nb));
    let det = m11 * m22 - m12 * m21;
    let x = (r1 * m22 - m12 * r2) / det;
    let y = (m11 * r2 - r1 * m21) / det;
    let p = o + u * x + v * y;
    match foot(&b.body, f, p).3 {
        Some(true) => Ok(Mouth {
            at: p,
            down: -n,
            flush: true,
        }),
        _ => Err(off_face(
            at,
            key,
            "lies outside its face at those distances from its edges".into(),
            Some(0.0),
        )),
    }
}

/// A place on the axis `of` gives, drilling along it (`reverse` turns
/// it): from the first, along it, of a face's circular edges' centres,
/// a circular edge's own centre, or where any other axis first enters
/// the targets. Never flush.
#[allow(clippy::too_many_arguments)]
fn coaxial(
    at: &At<'_>,
    key: &Id,
    role: &dyn Fn(&str) -> String,
    of: &GeomRef,
    reverse: bool,
    targets: &[&Built],
    work: &ryukei::work::Work,
) -> Result<Mouth> {
    let line = one(at, &role("coaxial"), of, Want::Axis)?.axis();
    let (o, mut down) = (line.origin, line.dir.get());
    if reverse {
        down = -down;
    }
    // Centres on the axis of the entity's circles: a face's rims, or a
    // circular edge itself.
    let mut centres: Vec<Point3> = Vec::new();
    if let Some(r) = of.reference() {
        let hit = crate::reference::resolve(
            at.part,
            at.order,
            at.owner,
            &role("coaxial"),
            Follow::Carrier,
            r,
            at.results,
        )?;
        let body = &hit.0.body;
        let edges: Vec<ryukei::topo::EdgeId> = match hit.1 {
            EntityId::Face(f) => body
                .face_loops(f)
                .flat_map(|l| body.loop_fins(l).collect::<Vec<_>>())
                .map(|fin| body.fin(fin).expect("live fin").edge)
                .collect(),
            EntityId::Edge(e) => vec![e],
            _ => Vec::new(),
        };
        for e in edges {
            let edge = body.edge(e).expect("live edge");
            if let Some(ryukei::geom::Curve::Circle(c)) = body.curve(edge.curve) {
                centres.push(c.frame().origin());
            }
        }
    }
    let first = centres
        .into_iter()
        .map(|c| ((c - o).dot(down), c))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let at_p = match first {
        Some((_, c)) => c,
        None => {
            // Where the axis first enters the targets, along `down`.
            let mut best: Option<f64> = None;
            for t in targets {
                let s = super::exits::exits(&t.body, o, down, work).map_err(|_| {
                    Error::new(
                        Code::RebuildFailed,
                        format!(
                            "hole {}: where place {key}'s axis enters its targets is uncertain",
                            at.owner
                        ),
                    )
                    .with("feature", at.owner)
                    .with("place", key)
                })?;
                if let Some([t_in, _]) = s.first() {
                    best = Some(best.map_or(*t_in, |b: f64| b.min(*t_in)));
                }
            }
            match best {
                Some(t) => o + down * t,
                None => {
                    return Err(off_face(
                        at,
                        key,
                        "has an axis that meets none of its targets".into(),
                        None,
                    ));
                }
            }
        }
    };
    Ok(Mouth {
        at: at_p,
        down,
        flush: false,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::files::Obj;
    use crate::doc::hole::Hole;
    use crate::expr::Env;
    use crate::session::Session;
    use crate::testing::call;
    use crate::units::Unit;
    use serde_json::{Value, json};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// Box `b` (20 × 10 × 5 mm from the origin), a datum point `pt` at
    /// (5, 5, 5) mm on its top, a datum point `mid` at (5, 5, 2) inside it,
    /// a cylinder `c` (r 4 mm, 10 mm long, on z, at (40, 0, 0)), and the
    /// box's body as `inspect` gives it.
    pub(crate) fn part() -> (tempfile::TempDir, Session, Value) {
        let (dir, mut s, body) = crate::sketch::tests::with_box();
        for (i, at) in [
            ("pt", ["5 mm", "5 mm", "5 mm"]),
            ("mid", ["5 mm", "5 mm", "2 mm"]),
        ] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "point", "id": i,
                       "by": "coordinates", "coordinates": at}),
            )
            .unwrap();
        }
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "c",
                   "params": {"radius": 4, "height": 10},
                   "placement": {"origin": [40, 0, 0]}}),
        )
        .unwrap();
        (dir, s, body)
    }

    /// The solid `t` as the last feature that holds it leaves it.
    pub(crate) fn solid<'a>(r: &'a crate::rebuild::PartResult, t: &Id) -> &'a Built {
        r.features
            .iter()
            .rev()
            .find_map(|(_, o)| o.as_ref().ok()?.solid(t))
            .unwrap_or_else(|| panic!("no solid {t}"))
    }

    /// The mouth of place `place` of a hole into `b` (and `c` when
    /// `both`), resolved after every feature of the part.
    pub(crate) fn mouth_of(s: &mut Session, place: Value, both: bool) -> Result<Mouth> {
        let pid = Id::parse("p", "part").unwrap();
        let r = s.rebuilt(&pid).unwrap().clone();
        let part = s.project().part(&pid).unwrap().clone();
        let targets = if both {
            json!(["b", "c"])
        } else {
            json!(["b"])
        };
        let v = json!({"form": "simple", "diameter": "2 mm", "depth": "through_all",
                       "targets": targets, "places": {"a": place}});
        let mut o = Obj::new(&v, "").unwrap();
        let h = Hole::from_fields(&mut o, Unit::Mm, &Env::empty()).unwrap();
        let owner = id("h");
        let mut order = part.order().to_vec();
        order.push(owner.clone());
        let at = At {
            part: &part,
            order: &order,
            owner: &owner,
            results: &r,
        };
        let solids: Vec<&Built> = h.targets.iter().map(|t| solid(&r, t)).collect();
        let work = ryukei::work::Work::unlimited();
        let (k, p) = h.places.iter().next().unwrap();
        mouth(&at, k, p, &solids, &work)
    }

    /// The point of the vertex named `name` in an `inspect` body.
    pub(crate) fn at(body: &Value, name: &Value) -> Value {
        body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == *name)
            .map(|v| v["point"].clone())
            .unwrap()
    }

    /// The top face's name and its edge along y = 0 and along x = 0.
    pub(crate) fn top(body: &Value) -> (String, String, String) {
        let face = |z: f64| -> String {
            body["faces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| {
                    let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
                    f["surface"]["type"] == "plane"
                        && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == z
                })
                .map(|f| f["name"].as_str().unwrap().to_owned())
                .unwrap()
        };
        let edge = |axis: usize| -> String {
            // A straight edge on the top (z = 5 mm) whose ends share
            // coordinate `axis` = 0.
            body["edges"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| {
                    let (a, b) = (at(body, &e["start"]), at(body, &e["end"]));
                    a[2].as_f64() == Some(0.005)
                        && b[2].as_f64() == Some(0.005)
                        && a[axis].as_f64() == Some(0.0)
                        && b[axis].as_f64() == Some(0.0)
                })
                .map(|e| e["name"].as_str().unwrap().to_owned())
                .unwrap()
        };
        (face(1.0), edge(1), edge(0))
    }

    pub(crate) fn close(p: Point3, want: [f64; 3]) {
        let got = [p.x, p.y, p.z];
        assert!(
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-12),
            "{got:?} != {want:?}"
        );
    }

    #[test]
    fn at_a_point_on_one_face_drills_into_it_flush() {
        let (_dir, mut s, _) = part();
        let m = mouth_of(&mut s, json!({"at": {"feature": "pt"}}), false).unwrap();
        close(m.at, [0.005, 0.005, 0.005]);
        close(Point3::ORIGIN + m.down, [0.0, 0.0, -1.0]);
        assert!(m.flush);
    }

    #[test]
    fn a_vertex_on_three_faces_is_ambiguous_until_a_face_is_given() {
        let (_dir, mut s, body) = part();
        let (top, _, _) = top(&body);
        // The box's corner at (0, 0, 5) mm: on its top, front and left.
        let v = body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["point"] == json!([0.0, 0.0, 0.005]))
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .unwrap();
        let at = json!({"feature": "b", "name": v});
        let e = mouth_of(&mut s, json!({"at": at}), false).unwrap_err();
        assert_eq!(e.code, Code::HolePlaceAmbiguous);
        assert_eq!(e.details["faces"].as_array().unwrap().len(), 3);
        let m = mouth_of(
            &mut s,
            json!({"at": at, "face": {"feature": "b", "name": top}}),
            false,
        )
        .unwrap();
        close(m.at, [0.0, 0.0, 0.005]);
        assert!(m.flush);
    }

    #[test]
    fn a_point_off_its_face_or_inside_is_refused() {
        let (_dir, mut s, body) = part();
        let (top, _, _) = top(&body);
        let e = mouth_of(
            &mut s,
            json!({"at": {"feature": "mid"}, "face": {"feature": "b", "name": top}}),
            false,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::HolePlaceOffFace);
        assert!((e.details["distance_m"].as_f64().unwrap() - 0.003).abs() < 1e-12);
        // On no face of the targets at all.
        let e = mouth_of(&mut s, json!({"at": {"feature": "mid"}}), false).unwrap_err();
        assert_eq!(e.code, Code::HolePlaceOffFace);
    }

    #[test]
    fn a_direction_turns_the_drill_and_the_mouth_reaches_back() {
        let (_dir, mut s, _) = part();
        let m = mouth_of(
            &mut s,
            json!({"at": {"feature": "pt"}, "direction": "x"}),
            false,
        )
        .unwrap();
        close(Point3::ORIGIN + m.down, [1.0, 0.0, 0.0]);
        assert!(!m.flush);
        let m = mouth_of(
            &mut s,
            json!({"at": {"feature": "pt"}, "direction": "z", "reverse": true}),
            false,
        )
        .unwrap();
        close(Point3::ORIGIN + m.down, [0.0, 0.0, -1.0]);
        assert!(m.flush, "z reversed is the face's inward normal");
    }

    /// The name of cylinder `c`'s side.
    pub(crate) fn side(s: &mut Session) -> String {
        let c = call(s, "inspect", json!({"part": "p", "feature": "c"})).unwrap();
        c["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["surface"]["type"] == "cylinder")
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .unwrap()
    }

    #[test]
    fn radial_on_a_cylinder_drills_toward_its_axis() {
        let (_dir, mut s, _) = part();
        let side = side(&mut s);
        // A point 10 mm out along +x from the axis, half way up: its foot
        // is on the side at (44, 0, 5) mm.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "point", "id": "out",
                   "by": "coordinates", "coordinates": ["50 mm", "0 mm", "5 mm"]}),
        )
        .unwrap();
        let m = mouth_of(
            &mut s,
            json!({"at": {"feature": "out"}, "face": {"feature": "c", "name": side}}),
            true,
        )
        .unwrap();
        close(m.at, [0.044, 0.0, 0.005]);
        close(Point3::ORIGIN + m.down, [-1.0, 0.0, 0.0]);
        assert!(!m.flush);
    }

    #[test]
    fn from_two_edges_is_dimensioned_inside_the_face() {
        let (_dir, mut s, body) = part();
        let (top, y0, x0) = top(&body);
        let place = |dx: &str, dy: &str| {
            json!({"face": {"feature": "b", "name": top},
                   "from": [{"edge": {"feature": "b", "name": x0}, "distance": dx},
                            {"edge": {"feature": "b", "name": y0}, "distance": dy}]})
        };
        let m = mouth_of(&mut s, place("6 mm", "3 mm"), false).unwrap();
        close(m.at, [0.006, 0.003, 0.005]);
        close(Point3::ORIGIN + m.down, [0.0, 0.0, -1.0]);
        assert!(m.flush);
        // Off the face's far side: 30 mm along a 20 mm face.
        let e = mouth_of(&mut s, place("30 mm", "3 mm"), false).unwrap_err();
        assert_eq!(e.code, Code::HolePlaceOffFace);
    }

    #[test]
    fn parallel_edges_are_refused() {
        let (_dir, mut s, body) = part();
        let (top, y0, _) = top(&body);
        let e = mouth_of(
            &mut s,
            json!({"face": {"feature": "b", "name": top},
                   "from": [{"edge": {"feature": "b", "name": y0}, "distance": "1 mm"},
                            {"edge": {"feature": "b", "name": y0}, "distance": "2 mm"}]}),
            false,
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["reason"].as_str()),
            (Code::InvalidParam, Some("edges_parallel"))
        );
    }

    #[test]
    fn coaxial_with_a_cylinder_starts_at_its_first_rim() {
        let (_dir, mut s, _) = part();
        let of = json!({"feature": "c", "name": side(&mut s)});
        // The cylinder's own frame runs up z: the first rim along it is
        // the bottom one, at z = 0; reverse drills down from the top.
        let m = mouth_of(&mut s, json!({"coaxial": of}), true).unwrap();
        close(m.at, [0.040, 0.0, 0.0]);
        close(Point3::ORIGIN + m.down, [0.0, 0.0, 1.0]);
        assert!(!m.flush);
        let m = mouth_of(&mut s, json!({"coaxial": of, "reverse": true}), true).unwrap();
        close(m.at, [0.040, 0.0, 0.010]);
        close(Point3::ORIGIN + m.down, [0.0, 0.0, -1.0]);
    }

    #[test]
    fn coaxial_with_an_axis_starts_where_it_enters_the_targets() {
        let (_dir, mut s, _) = part();
        // A datum axis from pt (5, 5, 5) to mid (5, 5, 2) mm: it has no
        // circle, runs down -z, and enters the box at its top.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "axis", "id": "ax", "by": "two_points",
                   "points": ["p/pt", "p/mid"]}),
        )
        .unwrap();
        let m = mouth_of(&mut s, json!({"coaxial": {"feature": "ax"}}), false).unwrap();
        close(m.at, [0.005, 0.005, 0.005]);
        close(Point3::ORIGIN + m.down, [0.0, 0.0, -1.0]);
        assert!(!m.flush);
    }
}
