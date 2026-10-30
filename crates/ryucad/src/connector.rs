//! Connectors: frames on the parts an assembly's instances place, defined
//! from named faces, edges and vertices (design 72), for mates and
//! instance patterns.
//!
//! A connector's frame, in the coordinates of the part it is on:
//!
//! | Entity | Origin | z | x |
//! |---|---|---|---|
//! | vertex | its point | the part's z | the part's x |
//! | straight edge | the part origin projected onto its line | along its curve | the part's x made perpendicular, else y |
//! | circular edge | the circle's centre | the circle's axis | the circle's x |
//! | planar face | the part origin projected onto it | outward normal | the part's x made perpendicular, else y |
//! | cylindrical or conical face | the part origin projected onto the axis | the surface's axis | the surface's x |
//! | spherical face | the centre | the part's z | the part's x |
//! | `origin` | the part origin | z | x |
//!
//! Anything else (a NURBS or swept face, a torus, an ellipse or a spline
//! edge) is `connector_unsupported`. The offset then moves the frame in
//! its own axes, turns it about its z and turns it over about its x.
//! Connectors on parts deeper in subassemblies are carried through the
//! poses that place them.

use crate::assembly::{Occurrence, Placed, path_text};
use crate::doc::assembly::Pose as DocPose;
use crate::doc::id::Id;
use crate::doc::mate::{At, Connector, Offset};
use crate::error::{Code, Error, Result};
use ryukei::geom::{Curve, Rigid, Surface};
use ryukei::topo::{Body, EntityId, Sense};
use ryumate::Pose;

/// A rigid placement as the solver takes it.
pub fn pose(r: &Rigid) -> Pose {
    let t = r.translation_part();
    Pose {
        rotation: r.rotation_matrix(),
        origin: [t.x, t.y, t.z],
    }
}

/// A solved pose as a file holds it: its origin and its z and x axes,
/// `-0` written as `0` and the axes snapped ([`DocPose::snapped`]).
pub fn doc_pose(p: &Pose) -> DocPose {
    DocPose {
        origin_m: p.origin,
        z: p.axis(2),
        x: p.axis(0),
    }
    .snapped()
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let n = ryukei::math::sqrt(dot(a, a));
    (n > 1e-9).then(|| a.map(|c| c / n))
}

/// `v` with its part along unit `z` taken away, made unit; `None` when
/// nothing is left.
fn perpendicular(v: [f64; 3], z: [f64; 3]) -> Option<[f64; 3]> {
    let d = dot(v, z);
    unit([v[0] - d * z[0], v[1] - d * z[1], v[2] - d * z[2]])
}

/// The frame with origin `o`, axis `z` (unit) and x the part's x made
/// perpendicular to it, else its y.
fn with_part_x(o: [f64; 3], z: [f64; 3]) -> Pose {
    let x = perpendicular([1.0, 0.0, 0.0], z)
        .or_else(|| perpendicular([0.0, 1.0, 0.0], z))
        .expect("x or y is off any axis");
    Pose::from_axes(o, x, cross(z, x), z)
}

/// The foot of the part origin on the line through `o` along unit `d`.
fn foot(o: [f64; 3], d: [f64; 3]) -> [f64; 3] {
    let t = -dot(o, d);
    [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]]
}

fn frame_of(f: &ryukei::geom::Frame) -> Pose {
    let v = |u: ryukei::geom::UnitVector3| {
        let g = u.get();
        [g.x, g.y, g.z]
    };
    let o = f.origin();
    Pose::from_axes([o.x, o.y, o.z], v(f.x()), v(f.y()), v(f.z()))
}

/// The frame entity `e` of `body` defines, in its part's coordinates, or
/// what it is when it defines none.
pub fn entity_frame(body: &Body, e: EntityId) -> core::result::Result<Pose, &'static str> {
    let xyz = |p: ryukei::geom::Point3| [p.x, p.y, p.z];
    match e {
        EntityId::Vertex(v) => {
            let p = body.vertex(v).expect("a live vertex").point;
            Ok(Pose {
                origin: xyz(p),
                ..Pose::IDENTITY
            })
        }
        EntityId::Edge(id) => {
            let edge = body.edge(id).expect("a live edge");
            match body.curve(edge.curve) {
                Some(Curve::Line(l)) => {
                    let d = l.dir().get();
                    let d = [d.x, d.y, d.z];
                    Ok(with_part_x(foot(xyz(l.origin()), d), d))
                }
                Some(Curve::Circle(c)) => Ok(frame_of(c.frame())),
                Some(Curve::Ellipse(_)) => Err("an elliptical edge"),
                _ => Err("a spline edge"),
            }
        }
        EntityId::Face(f) => {
            let face = body.face(f).expect("a live face");
            let s = body.surface(face.surface).expect("a face's surface");
            match s {
                Surface::Plane(p) => {
                    let fr = frame_of(p.frame());
                    let z = fr.axis(2);
                    let z = if face.sense == Sense::Reversed {
                        z.map(|c| -c)
                    } else {
                        z
                    };
                    // The part origin projected onto the plane.
                    let d = dot(fr.origin, z);
                    Ok(with_part_x([d * z[0], d * z[1], d * z[2]], z))
                }
                Surface::Cylinder(_) | Surface::Cone(_) => {
                    let mut fr = frame_of(s.frame().expect("an analytic surface"));
                    fr.origin = foot(fr.origin, fr.axis(2));
                    Ok(fr)
                }
                Surface::Sphere(_) => Ok(Pose {
                    origin: frame_of(s.frame().expect("an analytic surface")).origin,
                    ..Pose::IDENTITY
                }),
                Surface::Torus(_) => Err("a toroidal face"),
                _ => Err("a spline or swept face"),
            }
        }
        _ => Err("not a face, edge or vertex"),
    }
}

/// `frame` moved by `offset` in its own axes: along them by its origin,
/// then turned about its z, then turned over about its x.
pub fn offset(frame: &Pose, offset: &Offset) -> Pose {
    let mut out = *frame;
    if let Some(o) = &offset.origin {
        let d = o.each_ref().map(|q| q.meters());
        let moved = frame.turn(d);
        for (x, m) in out.origin.iter_mut().zip(moved) {
            *x += m;
        }
    }
    if let Some(r) = &offset.rotate {
        let at = out.origin;
        let turn = pose(&Rigid::rotation(ryukei::geom::UnitVector3::Z, r.radians()));
        out = out.then_after(&turn);
        out.origin = at;
    }
    if offset.flip {
        let at = out.origin;
        // Half a turn about x, exactly: y and z change sign.
        let flip = Pose::from_axes(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, -1.0],
        );
        out = out.then_after(&flip);
        out.origin = at;
    }
    out
}

/// What a connector is on, once resolved: an instance of the assembly, or
/// a copy of a pattern's seed; and the frame in that instance's part
/// coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub instance: Id,
    pub copy: Option<(Id, [u32; 2])>,
    pub frame: Pose,
}

/// What a connector is resolved in: an assembly's instances, by ID, and
/// the copies its instance patterns place.
pub trait Scope {
    /// Instance `id`'s result, if the assembly lists one.
    fn instance(&self, id: &Id) -> Option<core::result::Result<&Placed, &Error>>;

    /// Whether pattern `pattern` places a copy of `seed` at `index`, or
    /// why it failed; `None` when the assembly has no such pattern.
    fn places(&self, pattern: &Id, seed: &Id, index: [u32; 2]) -> Option<Result<bool>>;
}

/// Resolves connector `c`, held by `owner` in its `role`, in `scope`.
pub fn resolve(owner: &Id, role: &str, c: &Connector, scope: &dyn Scope) -> Result<Found> {
    let text = c.text();
    let fail = |code, msg: String| {
        Error::new(code, format!("{owner}: its {role} {text} {msg}"))
            .with("feature", owner)
            .with("role", role)
            .with("connector", &text)
    };
    let (instance, inside, copy) = match c.copy {
        None => (c.root.clone(), &c.rest[..], None),
        Some(index) => {
            let seed = c.rest[0].clone();
            match scope.places(&c.root, &seed, index) {
                None => {
                    return Err(fail(
                        Code::NotFound,
                        format!("names pattern {}, which the assembly doesn't have", c.root),
                    ));
                }
                Some(Err(e)) => {
                    return Err(fail(
                        Code::DependencyFailed,
                        format!("names a copy of pattern {}, which failed", c.root),
                    )
                    .with("dependency", &c.root)
                    .with("cause", e.code));
                }
                Some(Ok(false)) => {
                    return Err(fail(
                        Code::NotFound,
                        format!(
                            "names a copy pattern {} doesn't place (of {seed}, at {index:?})",
                            c.root
                        ),
                    ));
                }
                Some(Ok(true)) => {}
            }
            (seed, &c.rest[1..], Some((c.root.clone(), index)))
        }
    };
    let p = match scope.instance(&instance) {
        None => {
            return Err(fail(
                Code::NotFound,
                format!("names {instance}, which isn't an instance the assembly lists"),
            ));
        }
        Some(Err(e)) => {
            return Err(fail(
                Code::DependencyFailed,
                format!("names {instance}, which failed to rebuild"),
            )
            .with("dependency", &instance)
            .with("cause", e.code));
        }
        Some(Ok(p)) => p,
    };
    let Some(o) = p.model.occurrences.iter().find(|o| o.path == inside) else {
        return Err(fail(
            Code::NotFound,
            format!(
                "names {}, where {} places no part",
                if inside.is_empty() {
                    instance.to_string()
                } else {
                    format!("{instance}/{}", path_text(inside))
                },
                p.part
            ),
        ));
    };
    let on_part = match &c.at {
        At::Origin => Pose::IDENTITY,
        At::Entity(r) => entity(o, r).map_err(|(code, msg)| {
            let mut e = fail(code, msg)
                .with("part", &o.part)
                .with("rev", o.rev.to_json());
            if code == Code::ReferenceUnresolved {
                e = e.with("solid", &r.feature);
            }
            e
        })?,
    };
    let frame = pose(&o.pose).then_after(&on_part);
    Ok(Found {
        instance,
        copy,
        frame: offset(&frame, &c.offset),
    })
}

/// The frame of the entity `r` names in occurrence `o`, in its part's
/// coordinates.
fn entity(
    o: &Occurrence,
    r: &crate::doc::reference::Reference,
) -> core::result::Result<Pose, (Code, String)> {
    let rev = o.rev.text();
    let Some(s) = o.solids.iter().find(|s| s.founder == r.feature) else {
        return Err((
            Code::ReferenceWrongKind,
            format!(
                "names {}, which founds no solid {} shows at {rev}",
                r.feature, o.part
            ),
        ));
    };
    let Some(e) = s.built.body.by_name(r.name) else {
        return Err((
            Code::ReferenceUnresolved,
            format!("is gone from solid {} of {} at {rev}", r.feature, o.part),
        ));
    };
    entity_frame(&s.built.body, e).map_err(|what| {
        (
            Code::ConnectorUnsupported,
            format!("is on {what}, which defines no connector frame"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::{Quantity, Unit};
    use ryukei::geom::{Frame, Point3, Vector3};

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-12)
    }

    fn body_of(f: &crate::doc::files::Feature) -> Body {
        let part = Id::parse("p", "part").unwrap();
        let id = Id::parse("b", "feature").unwrap();
        crate::rebuild::build_feature(&part, &id, f).unwrap().body
    }

    fn primitive(ty: &str, params: serde_json::Value) -> crate::doc::files::Feature {
        let v = serde_json::json!({"format": 1, "kind": "ryucad.feature", "type": ty,
            "params": params,
            "placement": {"origin": ["10 mm", "0 mm", "0 mm"], "z": [0, 0, 1], "x": [1, 0, 0]}});
        crate::doc::files::Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap()
    }

    #[test]
    fn faces_edges_and_vertices_define_frames() {
        // A 20 x 10 x 5 mm box with its corner at x = 10 mm.
        let b = body_of(&primitive(
            "box",
            serde_json::json!({"x": 20, "y": 10, "z": 5}),
        ));
        let mut planes = 0;
        for fid in b.faces() {
            let face = b.face(fid).unwrap();
            let f = entity_frame(&b, EntityId::Face(fid)).unwrap();
            assert!(f.skew() < 1e-15);
            if let Some(Surface::Plane(_)) = b.surface(face.surface) {
                planes += 1;
            }
            // The top's outward normal is up; its origin is over the part's.
            if near(f.axis(2), [0.0, 0.0, 1.0]) {
                assert!(near(f.origin, [0.0, 0.0, 0.005]));
                assert!(near(f.axis(0), [1.0, 0.0, 0.0]));
            }
            // A side facing x: x can't be the part's x, so it is its y.
            if near(f.axis(2), [1.0, 0.0, 0.0]) {
                assert!(near(f.origin, [0.03, 0.0, 0.0]));
                assert!(near(f.axis(0), [0.0, 1.0, 0.0]));
            }
        }
        assert_eq!(planes, 6);
        for eid in b.edges() {
            let f = entity_frame(&b, EntityId::Edge(eid)).unwrap();
            // The foot of the origin on the edge's line is on the line.
            assert!(dot(f.origin, f.axis(2)).abs() < 1e-15);
        }
        for vid in b.vertices() {
            let v = b.vertex(vid).unwrap();
            let f = entity_frame(&b, EntityId::Vertex(vid)).unwrap();
            assert_eq!(f.origin, [v.point.x, v.point.y, v.point.z]);
            assert_eq!(f.rotation, Pose::IDENTITY.rotation);
        }
        // A cylinder: its side's axis, its rims' circles.
        let c = body_of(&primitive(
            "cylinder",
            serde_json::json!({"radius": 3, "height": 4}),
        ));
        for fid in c.faces() {
            let face = c.face(fid).unwrap();
            let f = entity_frame(&c, EntityId::Face(fid)).unwrap();
            if let Some(Surface::Cylinder(_)) = c.surface(face.surface) {
                assert!(near(f.origin, [0.01, 0.0, 0.0]));
                assert!(near(f.axis(2), [0.0, 0.0, 1.0]));
            }
        }
        for eid in c.edges() {
            // A rim's axis is its circle's, which runs either way.
            let f = entity_frame(&c, EntityId::Edge(eid)).unwrap();
            assert!(near([f.origin[0], f.origin[1], 0.0], [0.01, 0.0, 0.0]));
            assert!((f.axis(2)[2].abs() - 1.0).abs() < 1e-15);
        }
        // A sphere's centre, a torus none.
        let s = body_of(&primitive("sphere", serde_json::json!({"radius": 3})));
        let fid = s.faces().next().unwrap();
        assert!(near(
            entity_frame(&s, EntityId::Face(fid)).unwrap().origin,
            [0.01, 0.0, 0.0]
        ));
        let t = body_of(&primitive(
            "torus",
            serde_json::json!({"major_radius": 5, "minor_radius": 1}),
        ));
        let fid = t.faces().next().unwrap();
        assert_eq!(
            entity_frame(&t, EntityId::Face(fid)),
            Err("a toroidal face")
        );
    }

    #[test]
    fn offsets_move_turn_and_flip_in_the_frames_own_axes() {
        let f = frame_of(
            &Frame::new(
                Point3::new(1.0, 2.0, 3.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            )
            .unwrap(),
        );
        let q = |s: &str| Quantity::parse(s, Unit::Mm).unwrap();
        let o = Offset {
            origin: Some([q("1"), q("0"), q("2")]),
            rotate: Some(
                Quantity::eval_as("90 deg", crate::units::Field::Angle, &|_| unreachable!())
                    .unwrap(),
            ),
            flip: true,
        };
        let g = offset(&f, &o);
        // Along its x (the world's y) 1 mm, along its z (the world's x) 2 mm.
        assert!(near(g.origin, [1.002, 2.001, 3.0]));
        // Turned a quarter about z, its x is its old y; flipped, z reverses.
        assert!(near(g.axis(0), f.axis(1)));
        assert!(near(g.axis(2), f.axis(2).map(|c| -c)));
        assert!(g.skew() < 1e-15);
        assert_eq!(offset(&f, &Offset::NONE), f);
    }
}
