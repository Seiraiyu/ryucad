//! Patterns to ryukei tools. A pattern applies its seeds' own bodies
//! again (each a join's, a cut's or a hole's, from the seed's result),
//! moved for each instance: along one or two directions, turned about an
//! axis, or reflected in a plane. Every instance of every seed goes into
//! one kernel call per target.
//!
//! An instance's tool key is SipHash of
//! `ryucad.tool\0<part>\0<pattern>\0<seed>[\0<point>]\0<i>[\0<j>]`: a
//! hole seed's point, and the second index only past the first row, so
//! adding a second direction renames nothing of the first, and skipping
//! an instance or changing a spacing, angle or count renames nothing that
//! remains.

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::pattern::{Direction, Instance, Kind, MirrorPlane, Pattern};
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::sketch::OriginPlane;
use crate::doc::solid::Operation;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Applied, Results};
use crate::reference::{kind, resolve, wrong_kind};
use core::f64::consts::TAU;
use ryukei::geom::{Curve, Frame, Plane, Point3, Rigid, Surface, UnitVector3, Vector3};
use ryukei::name::{OpKey, ToolKey};
use ryukei::topo::EntityId;

/// The tool key of an instance of a seed's tool (a hole seed's at
/// `point`).
pub fn instance_key(
    part: &Id,
    pattern: &Id,
    seed: &Id,
    point: Option<&Id>,
    [i, j]: Instance,
) -> ToolKey {
    let mut bytes = b"ryucad.tool\0".to_vec();
    let mut piece = |s: &str| {
        bytes.extend_from_slice(s.as_bytes());
        bytes.push(0);
    };
    piece(part.as_str());
    piece(pattern.as_str());
    piece(seed.as_str());
    if let Some(p) = point {
        piece(p.as_str());
    }
    piece(&i.to_string());
    if j > 0 {
        piece(&j.to_string());
    }
    bytes.pop();
    ToolKey(OpKey::from_bytes(&bytes).0)
}

/// How an instance is placed: moved rigidly, or reflected in a plane.
#[derive(Clone, Debug)]
pub enum Motion {
    Rigid(Rigid),
    Mirror(Plane),
}

/// The seeds' own bodies as the pattern reads them, with the operation
/// they share. A seed must be a join or a cut (a hole is a cut), and all
/// of them must act alike on the pattern's targets.
pub fn seeds<'a>(
    part: &Part,
    id: &Id,
    p: &'a Pattern,
    before: &'a dyn Results,
) -> Result<(Operation, Vec<(&'a Id, &'a Applied)>)> {
    let mut out: Vec<(&Id, &Applied)> = Vec::new();
    let mut op: Option<Operation> = None;
    for (k, seed) in p.features.iter().enumerate() {
        let at = format!("/features/{k}");
        let out_of = crate::graph::input(part, id, seed, before)?;
        let ty = part.features[seed]
            .as_ref()
            .map_or("feature", Feature::type_name);
        let applied = out_of
            .applied()
            .filter(|a| matches!(a.operation, Operation::Join | Operation::Cut));
        let Some(applied) = applied else {
            return Err(Error::new(
                Code::ReferenceWrongKind,
                format!("pattern {id}: its seed {seed} is a {ty} that joins or cuts nothing; a pattern repeats joins, cuts and holes"),
            )
            .with("feature", id)
            .with("seed", seed)
            .with("pointer", at));
        };
        if op.is_some_and(|o| o != applied.operation) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("pattern {id}: its seeds join and cut; give each its own pattern"),
            )
            .with("feature", id)
            .with("seed", seed)
            .with("pointer", at));
        }
        op = Some(applied.operation);
        let theirs: Vec<&Id> = part.features[seed]
            .as_ref()
            .map(Feature::targets)
            .unwrap_or_default();
        let same = theirs.len() == p.targets.len() && theirs.iter().all(|t| p.targets.contains(t));
        if !same {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "pattern {id} acts on {}, but its seed {seed} on {}: a pattern acts on its seeds' targets",
                    join(p.targets.iter()),
                    join(theirs.into_iter())
                ),
            )
            .with("feature", id)
            .with("seed", seed)
            .with("pointer", "/targets"));
        }
        out.push((seed, applied));
    }
    Ok((op.expect("a pattern has seeds"), out))
}

fn join<'a>(ids: impl Iterator<Item = &'a Id>) -> String {
    ids.map(Id::as_str).collect::<Vec<_>>().join(", ")
}

/// Each instance with how it is placed, in [`Pattern::instances`] order.
pub fn motions(
    part: &Part,
    id: &Id,
    p: &Pattern,
    before: &dyn Results,
) -> Result<Vec<(Instance, Motion)>> {
    let instances = p.instances();
    match &p.kind {
        Kind::Linear { first, second } => {
            let step = |row: &crate::doc::pattern::Row, role: &str| -> Result<Vector3> {
                let d = direction(part, id, role, &row.direction, before)?;
                let s = row.spacing.meters();
                Ok(d * if row.reverse { -s } else { s })
            };
            let a = step(first, "direction")?;
            let b = match second {
                Some(r) => {
                    let b = step(r, "direction2")?;
                    let (ua, ub) = (a.normalize(), b.normalize());
                    if let (Some(ua), Some(ub)) = (ua, ub)
                        && ua.get().cross(ub.get()).length() <= ryukei::tol::ANGULAR_RESOLUTION
                    {
                        return Err(Error::new(
                            Code::InvalidParam,
                            format!("pattern {id}: its two directions are parallel"),
                        )
                        .with("feature", id)
                        .with("pointer", "/direction2"));
                    }
                    b
                }
                None => Vector3::ZERO,
            };
            Ok(instances
                .into_iter()
                .map(|[i, j]| {
                    let t = a * f64::from(i) + b * f64::from(j);
                    ([i, j], Motion::Rigid(Rigid::translation(t)))
                })
                .collect())
        }
        Kind::Circular { axis, count, angle } => {
            let (c, z) = axis_of(part, id, axis, before)?;
            let n = count.number();
            let a = angle.radians();
            let whole = a >= TAU - ryukei::tol::ANGULAR_RESOLUTION;
            let step = match (whole, n > 1.0) {
                (true, _) => TAU / n,
                (false, true) => a / (n - 1.0),
                (false, false) => 0.0,
            };
            let about = |t: f64| {
                Rigid::translation(c.to_vector())
                    .then_after(&Rigid::rotation(z, t))
                    .then_after(&Rigid::translation(-c.to_vector()))
            };
            Ok(instances
                .into_iter()
                .map(|[i, j]| ([i, j], Motion::Rigid(about(step * f64::from(i)))))
                .collect())
        }
        Kind::Mirror { plane } => {
            let plane = mirror_plane(part, id, plane, before)?;
            Ok(instances
                .into_iter()
                .map(|x| (x, Motion::Mirror(plane)))
                .collect())
        }
    }
}

/// A linear pattern's direction (`role`): an origin axis, a straight
/// edge from its start to its end, or a planar face's outward normal.
fn direction(
    part: &Part,
    id: &Id,
    role: &str,
    d: &Direction,
    before: &dyn Results,
) -> Result<Vector3> {
    let r = match d {
        Direction::Origin(a) => {
            let [x, y, z] = a.vector();
            return Ok(Vector3::new(x, y, z));
        }
        Direction::Entity(r) => r,
    };
    let found = resolve(part, part.order(), id, role, r, before)?;
    let v = match found.1 {
        EntityId::Edge(_) => {
            let (a, b) = crate::reference::line(part, id, role, r, found)?;
            b - a
        }
        EntityId::Face(_) => crate::reference::plane(part, id, role, r, found)?.1,
        e => {
            return Err(wrong_kind(
                part,
                id,
                role,
                r,
                kind(e),
                "a straight edge or a planar face",
            ));
        }
    };
    v.normalize().map(UnitVector3::get).ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            format!("pattern {id}: its {role} has no direction"),
        )
        .with("feature", id)
        .with("role", role)
    })
}

/// A circular pattern's axis: an origin axis through the origin, a
/// straight edge from its start to its end, a circular edge's axis, or a
/// cylindrical or conical face's axis.
fn axis_of(
    part: &Part,
    id: &Id,
    d: &Direction,
    before: &dyn Results,
) -> Result<(Point3, UnitVector3)> {
    let r: &Reference = match d {
        Direction::Origin(a) => {
            let [x, y, z] = a.vector();
            let z = Vector3::new(x, y, z)
                .normalize()
                .expect("an origin axis is unit");
            return Ok((Point3::ORIGIN, z));
        }
        Direction::Entity(r) => r,
    };
    let role = "axis";
    let found = resolve(part, part.order(), id, role, r, before)?;
    let (b, e) = found;
    let want = "a straight or circular edge, or a cylindrical or conical face";
    match e {
        EntityId::Edge(x) => {
            let edge = b.body.edge(x).expect("a live edge");
            match b.body.curve(edge.curve) {
                Some(Curve::Line(_)) => {
                    let (a, c) = crate::reference::line(part, id, role, r, found)?;
                    let z = (c - a).normalize().ok_or_else(|| {
                        Error::new(
                            Code::InvalidParam,
                            format!("pattern {id}: its axis has no length"),
                        )
                        .with("feature", id)
                    })?;
                    Ok((a, z))
                }
                Some(Curve::Circle(c)) => Ok((c.frame().origin(), c.frame().z())),
                _ => Err(wrong_kind(part, id, role, r, "curved edge", want)),
            }
        }
        EntityId::Face(f) => {
            let face = b.body.face(f).expect("a live face");
            match b.body.surface(face.surface) {
                Some(s @ (Surface::Cylinder(_) | Surface::Cone(_))) => {
                    let frame = s.frame().expect("an analytic surface");
                    Ok((frame.origin(), frame.z()))
                }
                _ => Err(wrong_kind(part, id, role, r, "face of another kind", want)),
            }
        }
        e => Err(wrong_kind(part, id, role, r, kind(e), want)),
    }
}

/// A mirror's plane: an origin plane through the origin, or a planar
/// face's plane.
fn mirror_plane(part: &Part, id: &Id, m: &MirrorPlane, before: &dyn Results) -> Result<Plane> {
    let (p, n) = match m {
        MirrorPlane::Origin(o) => {
            let n = match o {
                OriginPlane::Xy => Vector3::new(0.0, 0.0, 1.0),
                OriginPlane::Yz => Vector3::new(1.0, 0.0, 0.0),
                OriginPlane::Zx => Vector3::new(0.0, 1.0, 0.0),
            };
            (Point3::ORIGIN, n)
        }
        MirrorPlane::Face(r) => {
            let found = resolve(part, part.order(), id, "plane", r, before)?;
            crate::reference::plane(part, id, "plane", r, found)?
        }
    };
    let hint = if n.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let frame = Frame::new(p, n, hint).map_err(|e| {
        Error::new(Code::Internal, "a mirror plane that isn't one").with("kernel", format!("{e:?}"))
    })?;
    Ok(Plane::new(frame))
}

#[cfg(test)]
mod tests {
    use crate::doc::files::Feature;
    use crate::doc::id::Id;
    use crate::error::Code;
    use crate::rebuild::Built;
    use crate::server::tests::call;
    use crate::session::Session;
    use crate::units::Unit;
    use core::f64::consts::PI;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// Writes feature `f` of type `ty` (its fields, in millimetres) last
    /// in part `p`, or where it already is.
    fn put(s: &mut Session, f: &str, ty: &str, fields: Value) {
        let mut v = fields;
        v["format"] = json!(3);
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!(ty);
        let feature = Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap();
        let pid = id("p");
        let mut list = crate::server::order(s.project().part(&pid).unwrap()).unwrap();
        if !list.contains(&id(f)) {
            list.push(id(f));
        }
        crate::server::save(s, &pid, &[(&id(f), &feature)], list).unwrap();
    }

    /// Solid `solid` as feature `f` left it, or why it failed.
    fn after(s: &mut Session, f: &str, solid: &str) -> Result<Built, crate::error::Error> {
        let r = s.rebuilt(&id("p")).unwrap();
        let (_, out) = r.features.iter().find(|(x, _)| *x == id(f)).unwrap();
        out.clone()
            .map(|o| o.solid(&id(solid)).expect("the solid").clone())
    }

    /// The volume a body encloses, in mm³, meshed finely; the body
    /// checks clean.
    fn volume(b: &Built) -> f64 {
        assert_eq!(ryukei::check::check_intersections(&b.body), []);
        let m =
            ryukei::mesh::tessellate(&b.body, &ryukei::mesh::MeshParams::new(2e-6, 0.1)).unwrap();
        assert!(m.faults.is_empty(), "{:?}", m.faults);
        ryukei_testkit::mesh::volume_and_area(&m).0 * 1e9
    }

    fn near(got: f64, want: f64) {
        assert!((got - want).abs() < 2e-4 * want, "{got} mm³ != {want} mm³");
    }

    /// A hole of diameter 2 mm, 3 mm deep, at `points` of sketch `s`.
    fn hole(points: &[&str]) -> Value {
        json!({"sketch": "s", "points": points, "form": "simple", "diameter": 2,
               "depth": {"blind": 3}, "targets": ["b"]})
    }

    /// The entities pattern `f` made in `b`, by name, with the instance
    /// each came from.
    fn made_by(b: &Built, f: &str) -> BTreeSet<(String, String)> {
        b.origins
            .iter()
            .filter(|(_, o)| o["feature"] == f)
            .map(|(n, o)| (n.clone(), o["instance"].to_string()))
            .collect()
    }

    #[test]
    fn linear_patterns_repeat_their_seeds() {
        let (_dir, mut s) = crate::hole::tests::drilled();
        let one = PI * 3.0;
        put(&mut s, "h", "hole", hole(&["p1"]));
        let linear = |extra: Value| {
            let mut v = json!({"form": "linear", "features": ["h"], "direction": "x",
                               "spacing": 3, "count": 3, "targets": ["b"]});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        put(&mut s, "pat", "pattern", linear(json!({})));
        let b = after(&mut s, "pat", "b").unwrap();
        near(volume(&b), 1000.0 - 3.0 * one);
        let row = made_by(&b, "pat");
        // A second direction, against y: the first row is named as it was.
        put(
            &mut s,
            "pat",
            "pattern",
            linear(json!({"direction2": "y", "spacing2": 3, "count2": 2, "reverse2": true})),
        );
        let b = after(&mut s, "pat", "b").unwrap();
        near(volume(&b), 1000.0 - 6.0 * one);
        let grid = made_by(&b, "pat");
        let first: BTreeSet<String> = row.iter().map(|(n, _)| n.clone()).collect();
        assert!(
            first.iter().all(|n| grid.iter().any(|(m, _)| m == n)),
            "the first row renamed"
        );
        // Skipping one, or a wider spacing, renames nothing that remains.
        put(
            &mut s,
            "pat",
            "pattern",
            linear(
                json!({"direction2": "y", "spacing2": 3.5, "count2": 2, "reverse2": true,
                          "skip": [[1, 1]]}),
            ),
        );
        let b = after(&mut s, "pat", "b").unwrap();
        near(volume(&b), 1000.0 - 5.0 * one);
        let skipped = made_by(&b, "pat");
        assert!(skipped.is_subset(&grid));
        assert!(skipped.iter().all(|(_, i)| i != "[1,1]"));
        // Several seeds, each repeated: along y, clear of the holes so far
        // (one that lands on a hole already there changes nothing).
        put(&mut s, "h2", "hole", hole(&["p2"]));
        put(
            &mut s,
            "both",
            "pattern",
            json!({"form": "linear", "features": ["h", "h2"], "direction": "y",
                   "spacing": 3, "count": 2, "targets": ["b"]}),
        );
        let b = after(&mut s, "both", "b").unwrap();
        near(volume(&b), 1000.0 - 8.0 * one);
        // The seeds' own bodies are read, not built again.
        let r = s.rebuilt(&id("p")).unwrap();
        assert!(r.output(&id("h")).unwrap().applied().is_some());
    }

    /// Part `p`: a cylinder `b` of radius 10 mm, 5 mm tall, about the z
    /// axis, and a sketch `s` on its top with a point `p1` at (6, 0) mm.
    fn disc() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = crate::server::tests::fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "b",
                   "params": {"radius": 10, "height": 5}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s",
                   "placement": {"origin": [0, 0, 5]}}),
        )
        .unwrap();
        let ops = json!([
            {"op": "add", "type": "point", "at": [6, 0]},
            {"op": "add", "type": "fix", "a": "p1"},
        ]);
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": ops}),
        )
        .unwrap();
        (dir, s)
    }

    #[test]
    fn circular_patterns_turn_about_their_axis() {
        let (_dir, mut s) = disc();
        let disc = PI * 100.0 * 5.0;
        put(&mut s, "h", "hole", hole(&["p1"]));
        let circular = |axis: Value, count: u32, angle: &str| {
            json!({"form": "circular", "features": ["h"], "axis": axis, "count": count,
                   "angle": angle, "targets": ["b"]})
        };
        // Six round the whole turn, about the z axis.
        put(&mut s, "pat", "pattern", circular(json!("z"), 6, "360 deg"));
        let b = after(&mut s, "pat", "b").unwrap();
        near(volume(&b), disc - 6.0 * PI * 3.0);
        // About the cylinder's own side, three over a quarter turn.
        let r = s.rebuilt(&id("p")).unwrap();
        let body = r.output(&id("b")).unwrap().body().unwrap().clone();
        let side = body
            .body
            .faces()
            .find(|f| {
                let face = body.body.face(*f).unwrap();
                matches!(
                    body.body.surface(face.surface),
                    Some(ryukei::geom::Surface::Cylinder(_))
                )
            })
            .unwrap();
        let name = body.body.face(side).unwrap().name.to_string();
        put(
            &mut s,
            "pat",
            "pattern",
            circular(json!({"feature": "b", "name": name}), 3, "90 deg"),
        );
        let b = after(&mut s, "pat", "b").unwrap();
        near(volume(&b), disc - 3.0 * PI * 3.0);
        // Where the instances went: one a quarter turn round, at (0, 6).
        let quarter = b.body.vertices().any(|v| {
            let p = b.body.vertex(v).unwrap().point;
            ryukei::math::hypot(p.x * 1e3, p.y * 1e3 - 6.0) < 1.0 + 1e-6
        });
        assert!(quarter);
    }

    #[test]
    fn mirrors_match_the_same_features_by_hand() {
        let (_dir, mut s) = disc();
        // A countersunk hole and a boss off to one side, mirrored in the
        // yz plane, against both modelled again at the mirrored places.
        put(
            &mut s,
            "h",
            "hole",
            json!({"sketch": "s", "points": ["p1"], "form": "countersink", "diameter": 2,
                   "countersink": {"diameter": 4, "angle": 90},
                   "depth": {"blind": 3}, "targets": ["b"]}),
        );
        put(
            &mut s,
            "boss",
            "box",
            json!({"params": {"x": 2, "y": 3, "z": 2},
                   "placement": {"origin": ["2 mm", "-7 mm", "5 mm"], "x": [1, 0, 0], "z": [0, 0, 1]},
                   "operation": "join", "targets": ["b"]}),
        );
        put(
            &mut s,
            "m",
            "pattern",
            json!({"form": "mirror", "features": ["boss"], "plane": "yz", "targets": ["b"]}),
        );
        put(
            &mut s,
            "mh",
            "pattern",
            json!({"form": "mirror", "features": ["h"], "plane": "yz", "targets": ["b"]}),
        );
        let mirrored = after(&mut s, "mh", "b").unwrap();
        let by_pattern = volume(&mirrored);
        // By hand: the boss's image is the box from x = -4 to -2, and the
        // hole's is at (-6, 0).
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s",
                   "ops": [{"op": "add", "type": "point", "at": [-6, 0]},
                           {"op": "add", "type": "fix", "a": "p2"}]}),
        )
        .unwrap();
        put(
            &mut s,
            "mh",
            "hole",
            json!({"sketch": "s", "points": ["p2"], "form": "countersink", "diameter": 2,
                   "countersink": {"diameter": 4, "angle": 90},
                   "depth": {"blind": 3}, "targets": ["b"]}),
        );
        put(
            &mut s,
            "m",
            "box",
            json!({"params": {"x": 2, "y": 3, "z": 2},
                   "placement": {"origin": ["-4 mm", "-7 mm", "5 mm"], "x": [1, 0, 0], "z": [0, 0, 1]},
                   "operation": "join", "targets": ["b"]}),
        );
        let by_hand = volume(&after(&mut s, "mh", "b").unwrap());
        near(by_pattern, by_hand);
        let counts = |b: &Built| {
            let c = b.body.counts();
            (c.faces, c.edges, c.vertices)
        };
        assert_eq!(
            counts(&mirrored),
            counts(&after(&mut s, "mh", "b").unwrap())
        );
    }

    #[test]
    fn instances_that_touch_are_named() {
        let (_dir, mut s) = crate::hole::tests::drilled();
        put(&mut s, "h", "hole", hole(&["p1"]));
        // Holes 2 mm across, 2 mm apart, touch: the kernel refuses, and
        // the error says which instance alone it refuses.
        put(
            &mut s,
            "pat",
            "pattern",
            json!({"form": "linear", "features": ["h"], "direction": "y", "spacing": 2,
                   "count": 2, "reverse": true, "targets": ["b"]}),
        );
        let e = after(&mut s, "pat", "b").unwrap_err();
        assert_eq!(e.code, Code::BooleanFailed, "{e}");
        assert_eq!(
            (&e.details["seed"], &e.details["instance"]),
            (&json!("h"), &json!([1]))
        );
    }

    #[test]
    fn patterns_that_wont_do() {
        let (_dir, mut s) = crate::hole::tests::drilled();
        put(&mut s, "h", "hole", hole(&["p1"]));
        let linear = |features: Value, extra: Value| {
            let mut v = json!({"form": "linear", "features": features, "direction": "x",
                               "spacing": 3, "count": 2, "targets": ["b"]});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        // An instance off the box cuts nothing: the error says which.
        put(
            &mut s,
            "pat",
            "pattern",
            linear(json!(["h"]), json!({"spacing": 30})),
        );
        let e = after(&mut s, "pat", "b").unwrap_err();
        assert_eq!(e.code, Code::OperationNoEffect, "{e}");
        assert_eq!(
            (&e.details["seed"], &e.details["instance"]),
            (&json!("h"), &json!([1]))
        );
        // Every instance skipped leaves nothing to do.
        put(
            &mut s,
            "pat",
            "pattern",
            linear(json!(["h"]), json!({"skip": [1]})),
        );
        let e = after(&mut s, "pat", "b").unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::OperationNoEffect, Some("/skip"))
        );
        // A seed that founds a solid, or acts elsewhere; parallel
        // directions; a direction that is no direction.
        let r = s.rebuilt(&id("p")).unwrap();
        let body = r.output(&id("b")).unwrap().body().unwrap().clone();
        let vertex = body.body.vertices().next().unwrap();
        let vertex = body.body.vertex(vertex).unwrap().name.to_string();
        for (fields, code, pointer) in [
            (
                linear(json!(["b"]), json!({})),
                Code::ReferenceWrongKind,
                "/features/0",
            ),
            (
                linear(
                    json!(["h"]),
                    json!({"direction2": "x", "spacing2": 1, "count2": 2}),
                ),
                Code::InvalidParam,
                "/direction2",
            ),
        ] {
            put(&mut s, "pat", "pattern", fields.clone());
            let e = after(&mut s, "pat", "b").unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{fields}: {e}"
            );
        }
        put(
            &mut s,
            "pat",
            "pattern",
            linear(
                json!(["h"]),
                json!({"direction": {"feature": "b", "name": vertex}}),
            ),
        );
        let e = after(&mut s, "pat", "b").unwrap_err();
        assert_eq!(
            (e.code, e.details["role"].as_str()),
            (Code::ReferenceWrongKind, Some("direction"))
        );
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "o", "params": {"x": 1, "y": 1, "z": 1},
                   "placement": {"origin": [30, 0, 0]}}),
        )
        .unwrap();
        put(&mut s, "other", "pattern", {
            let mut v = linear(json!(["h"]), json!({}));
            v["targets"] = json!(["o"]);
            v
        });
        let e = after(&mut s, "other", "o").unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidParam, Some("/targets")),
            "{e}"
        );
    }
}
