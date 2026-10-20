//! Bodies as agents see them: every face, edge and vertex by stable name,
//! with its key geometry in meters.

use crate::doc::files::{Feature, PrimitiveType};
use crate::rebuild::{Built, placement};
use ryukei::geom::{Curve, Frame, Point3, Surface};
use ryukei::topo::{Body, Sense};
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

/// An axis-aligned box around the feature: its local extent's corners,
/// placed. Tight when the placement is axis-aligned.
pub fn bounds(f: &Feature) -> Value {
    match extent(f) {
        Some((min, max)) => json!({"min": min, "max": max}),
        None => Value::Null,
    }
}

/// [`bounds`] as `(min, max)`; `None` when the placement is invalid.
pub fn extent(f: &Feature) -> Option<([f64; 3], [f64; 3])> {
    let f = f.primitive()?;
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

/// A built feature: its hash, counts, bounds and entities.
pub fn body(f: &Feature, b: &Built) -> Value {
    let c = b.body.counts();
    let mut v = entities(&b.body);
    v["hash"] = json!(b.hash);
    v["counts"] = json!({"faces": c.faces, "edges": c.edges, "vertices": c.vertices});
    v["bounds"] = bounds(f);
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
}
