//! Bodies as agents see them: every face, edge and vertex by stable name,
//! with its key geometry in meters.

use crate::doc::files::{Feature, PrimitiveType};
use crate::rebuild::{Built, placement};
use ryukei::geom::{Box3, Curve, Frame, Point2, Point3, Surface, Vector3};
use ryukei::intersect::Place;
use ryukei::topo::{Body, FaceId, Sense};
use serde_json::{Value, json};

fn point(p: Point3) -> Value {
    json!([p.x, p.y, p.z])
}

fn frame(f: &Frame) -> Value {
    json!({"origin": point(f.origin()), "z": f.z().get().to_array(), "x": f.x().get().to_array()})
}

fn surface(s: &Surface) -> Value {
    let mut v = match s {
        Surface::Plane(_) => json!({"type": "plane"}),
        Surface::Cylinder(c) => json!({"type": "cylinder", "radius": c.radius()}),
        Surface::Cone(c) => {
            json!({"type": "cone", "radius": c.radius(), "half_angle": c.half_angle()})
        }
        Surface::Sphere(c) => json!({"type": "sphere", "radius": c.radius()}),
        Surface::Torus(c) => {
            json!({"type": "torus", "major_radius": c.major(), "minor_radius": c.minor()})
        }
        Surface::Nurbs(_) => json!({"type": "nurbs"}),
        _ => json!({"type": "other"}),
    };
    if let Some(f) = s.frame() {
        v["frame"] = frame(f);
    }
    v
}

fn curve(c: &Curve) -> Value {
    match c {
        Curve::Line(l) => {
            json!({"type": "line", "origin": point(l.origin()), "dir": l.dir().get().to_array()})
        }
        Curve::Circle(c) => {
            json!({"type": "circle", "radius": c.radius(), "frame": frame(c.frame())})
        }
        Curve::Ellipse(e) => {
            json!({"type": "ellipse", "major": e.major(), "minor": e.minor(), "frame": frame(e.frame())})
        }
        Curve::Nurbs(_) => json!({"type": "nurbs"}),
        _ => json!({"type": "other"}),
    }
}

/// Every face, edge and vertex of `body`, in array order.
pub fn entities(body: &Body) -> Value {
    let faces: Vec<Value> = body
        .faces()
        .map(|f| {
            let face = body.face(f).expect("live face");
            json!({
                "name": face.name.to_string(),
                "surface": surface(body.surface(face.surface).expect("live surface")),
                "reversed": face.sense == Sense::Reversed,
            })
        })
        .collect();
    let edges: Vec<Value> = body
        .edges()
        .map(|e| {
            let edge = body.edge(e).expect("live edge");
            let name = |v| body.vertex(v).expect("live vertex").name.to_string();
            json!({
                "name": edge.name.to_string(),
                "curve": curve(body.curve(edge.curve).expect("live curve")),
                "start": name(edge.start),
                "end": name(edge.end),
                "interval": edge.interval,
            })
        })
        .collect();
    let vertices: Vec<Value> = body
        .vertices()
        .map(|v| {
            let vertex = body.vertex(v).expect("live vertex");
            json!({"name": vertex.name.to_string(), "point": point(vertex.point)})
        })
        .collect();
    json!({"faces": faces, "edges": edges, "vertices": vertices})
}

/// An axis-aligned box around the feature: a primitive's local extent's
/// corners, placed (tight when the placement is axis-aligned), or the box
/// around any other body's faces.
pub fn bounds(f: &Feature, body: &Body) -> Value {
    match extent(f).or_else(|| body_extent(body)) {
        Some((min, max)) => json!({"min": min, "max": max}),
        None => Value::Null,
    }
}

/// The box around a body's faces, exact for analytic ones: their edges'
/// boxes (an arc's to its extremes), their loops' lone vertices, and the
/// points of a sphere or torus where it bulges furthest along an axis,
/// where they lie on the face. Any other face adds its kernel box, which
/// holds it. `None` for an empty body.
pub fn body_extent(body: &Body) -> Option<([f64; 3], [f64; 3])> {
    let b = body
        .faces()
        .map(|f| face_extent(body, f))
        .reduce(|a, b| a.union(&b))?;
    let (lo, hi) = (b.min(), b.max());
    Some(([lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z]))
}

/// One face's box for [`body_extent`].
fn face_extent(body: &Body, face: FaceId) -> Box3 {
    let loose = || ryukei::intersect::face_box(body, face);
    let f = body.face(face).expect("live face");
    let surface = body.surface(f.surface).expect("live surface");
    if !matches!(
        surface,
        Surface::Plane(_)
            | Surface::Cylinder(_)
            | Surface::Cone(_)
            | Surface::Sphere(_)
            | Surface::Torus(_)
    ) {
        return loose();
    }
    let mut out: Option<Box3> = None;
    let mut add = |b: Box3| out = Some(out.map_or(b, |o| o.union(&b)));
    for l in body.face_loops(face) {
        if let Some(v) = body.loop_(l).expect("live loop").vertex {
            let p = body.vertex(v).expect("live vertex").point;
            add(Box3::new(p, p));
        }
        for fin in body.loop_fins(l) {
            let e = body
                .edge(body.fin(fin).expect("live fin").edge)
                .expect("live edge");
            add(curve_extent(
                body.curve(e.curve).expect("live curve"),
                e.interval,
            ));
        }
    }
    // A plane, cylinder or cone is flat along its rulings, so its extremes
    // reach its boundary. A sphere or torus may bulge inside its loops.
    for p in bulges(surface) {
        let (u, v) = surface.project(p).param;
        match ryukei::intersect::point_in_face(body, face, Point2::new(u, v)) {
            Ok(Place::Outside) => {}
            Ok(_) => add(Box3::new(p, p)),
            Err(_) => return loose(),
        }
    }
    out.unwrap_or_else(loose)
}

/// The box around a curve over `[t0, t1]`: a line's ends, a circle's or
/// an ellipse's ends and the extremes it passes; a spline's hull.
fn curve_extent(c: &Curve, [t0, t1]: [f64; 2]) -> Box3 {
    match c {
        Curve::Circle(_) | Curve::Ellipse(_) => {
            // c(t) = m + a cos t + b sin t.
            use core::f64::consts::{PI, TAU};
            let m = c.point(0.0).lerp(c.point(PI), 0.5);
            let (a, b) = (c.point(0.0) - m, c.point(PI / 2.0) - m);
            let mut out =
                Box3::new(c.point(t0), c.point(t0)).union(&Box3::new(c.point(t1), c.point(t1)));
            for (ak, bk) in [(a.x, b.x), (a.y, b.y), (a.z, b.z)] {
                let t = ryukei::math::atan2(bk, ak);
                for t in [t, t + PI] {
                    // The first turn of t at or after t0.
                    let t = t0 + (t - t0).rem_euclid(TAU);
                    if t <= t1 {
                        let p = c.point(t);
                        out = out.union(&Box3::new(p, p));
                    }
                }
            }
            out
        }
        _ => c.bounds([t0, t1]),
    }
}

/// Where a sphere or a torus is furthest out along each axis, either way:
/// the points its normal is parallel to the axis at (a torus's along its
/// own axis are circles: a point of each).
fn bulges(s: &Surface) -> Vec<Point3> {
    let axes = [
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
    ];
    let Some(f) = s.frame() else {
        return Vec::new();
    };
    match s {
        Surface::Sphere(x) => {
            let (o, r) = (f.origin(), x.radius());
            axes.iter().flat_map(|k| [o + *k * r, o - *k * r]).collect()
        }
        Surface::Torus(x) => {
            let (o, big, r) = (f.origin(), x.major(), x.minor());
            let (fx, fy, fz) = (f.x().get(), f.y().get(), f.z().get());
            let mut out = Vec::new();
            for k in axes {
                let a = k.dot(fz);
                let p = fx * k.dot(fx) + fy * k.dot(fy);
                let n = p.length();
                if n < 1e-12 {
                    for s in [1.0, -1.0] {
                        out.push(o + fx * big + fz * (s * r));
                    }
                    continue;
                }
                for s in [1.0, -1.0] {
                    for c in [1.0, -1.0] {
                        let w = p * (s * c / n);
                        out.push(o + w * (big + r * c * n) + fz * (r * s * a));
                    }
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// The box around a body's faces' kernel boxes, which may hold a whole
/// circle where a face has only part of it; `None` for an empty body.
/// Cheap, for sizing a display mesh.
pub fn loose_extent(body: &Body) -> Option<([f64; 3], [f64; 3])> {
    let b = body
        .faces()
        .map(|f| ryukei::intersect::face_box(body, f))
        .reduce(|a, b| a.union(&b))?;
    let (lo, hi) = (b.min(), b.max());
    Some(([lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z]))
}

/// [`bounds`] as `(min, max)`; `None` when the placement is invalid.
pub fn extent(f: &Feature) -> Option<([f64; 3], [f64; 3])> {
    // A primitive that acts on other solids leaves them, not itself.
    let f = f.primitive().filter(|p| p.combine.is_new())?;
    let m = |k| f.meters(k);
    let (lo, hi) = match f.ty {
        PrimitiveType::Box => ([0.0; 3], [m("x"), m("y"), m("z")]),
        PrimitiveType::Cylinder | PrimitiveType::Cone => {
            let r = m("radius").max(m("top_radius"));
            ([-r, -r, 0.0], [r, r, m("height")])
        }
        PrimitiveType::Sphere => {
            let r = m("radius");
            ([-r; 3], [r; 3])
        }
        PrimitiveType::Torus => {
            let (a, b) = (m("major_radius") + m("minor_radius"), m("minor_radius"));
            ([-a, -a, -b], [a, a, b])
        }
    };
    let at = placement(&f.placement).ok()?;
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for i in 0..8 {
        let c = |k: usize| if i >> k & 1 == 0 { lo[k] } else { hi[k] };
        let p = at.apply_point(Point3::new(c(0), c(1), c(2)));
        for (k, x) in [p.x, p.y, p.z].into_iter().enumerate() {
            min[k] = min[k].min(x);
            max[k] = max[k].max(x);
        }
    }
    Some((min, max))
}

/// A built feature: its hash, counts, bounds and entities, each with its
/// origin when it came from a sketch.
pub fn body(f: &Feature, b: &Built) -> Value {
    let c = b.body.counts();
    let mut v = entities(&b.body);
    for list in ["faces", "edges", "vertices"] {
        for e in v[list].as_array_mut().expect("lists") {
            if let Some(o) = e["name"].as_str().and_then(|n| b.origins.get(n)) {
                e["origin"] = o.clone();
            }
        }
    }
    v["hash"] = json!(b.hash);
    v["counts"] = json!({"faces": c.faces, "edges": c.edges, "vertices": c.vertices});
    v["bounds"] = bounds(f, &b.body);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{Placement, Primitive};
    use crate::doc::id::Id;
    use crate::rebuild::build_feature;
    use crate::units::{Quantity, Unit};

    fn feature(ty: PrimitiveType, params: &[(&str, &str)]) -> Feature {
        Feature::Primitive(Primitive {
            ty,
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), Quantity::parse(v, Unit::M).unwrap()))
                .collect(),
            placement: Placement::identity(Unit::M),
            combine: crate::doc::solid::Combine::NEW,
        })
    }

    #[test]
    fn a_box_lists_everything_by_name() {
        let f = feature(PrimitiveType::Box, &[("x", "1"), ("y", "2"), ("z", "3")]);
        let id = Id::parse("b", "feature").unwrap();
        let b = build_feature(&id, &id, &f).unwrap();
        let v = body(&f, &b);
        assert_eq!(v["counts"], json!({"faces": 6, "edges": 12, "vertices": 8}));
        assert_eq!(v["faces"].as_array().unwrap().len(), 6);
        assert!(
            v["faces"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["surface"]["type"] == "plane")
        );
        assert!(
            v["edges"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["curve"]["type"] == "line")
        );
        assert_eq!(
            v["bounds"],
            json!({"min": [0.0, 0.0, 0.0], "max": [1.0, 2.0, 3.0]})
        );
        assert_eq!(v["faces"][0]["name"].as_str().unwrap().len(), 32);
    }

    #[test]
    fn closed_surfaces_have_only_a_face() {
        let f = feature(
            PrimitiveType::Torus,
            &[("major_radius", "3"), ("minor_radius", "1")],
        );
        let id = Id::parse("t", "feature").unwrap();
        let v = body(&f, &build_feature(&id, &id, &f).unwrap());
        assert_eq!(v["counts"], json!({"faces": 1, "edges": 0, "vertices": 0}));
        assert_eq!(v["faces"][0]["surface"]["type"], "torus");
        assert_eq!(v["faces"][0]["surface"]["major_radius"], 3.0);
        assert_eq!(v["bounds"]["max"], json!([4.0, 4.0, 1.0]));
    }

    #[test]
    fn bounds_hold_the_body_not_its_surfaces() {
        use crate::server::tests::call;
        // A half-round notch in the y = 0 face (#97): nothing lies below
        // y = 0, and the bottom is at z = 0 exactly.
        let (_dir, mut s, _) = crate::sketch::tests::with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "c",
                   "params": {"radius": 2, "height": 20},
                   "placement": {"origin": [10, 0, -5]}, "operation": "cut"}),
        )
        .unwrap();
        let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        let b = &v["bodies"][0]["bounds"];
        assert_eq!(b["min"], json!([0.0, 0.0, 0.0]), "{b}");
        assert_eq!(b["max"], json!([0.02, 0.01, 0.005]), "{b}");
        // A boss over the y = 10 face reaches its whole circle there,
        // within the chord.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "boss",
                   "params": {"radius": 3, "height": 8},
                   "placement": {"origin": [4, 9, 0]}, "operation": "join"}),
        )
        .unwrap();
        let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        let b = &v["bodies"][0]["bounds"];
        let at = |k: &str, i: usize| b[k][i].as_f64().unwrap();
        assert_eq!(b["min"], json!([0.0, 0.0, 0.0]), "{b}");
        assert_eq!((at("max", 0), at("max", 2)), (0.02, 0.008), "{b}");
        let y = at("max", 1);
        assert!(y <= 0.012 && y > 0.012 - 1e-5, "{b}");
    }
}
