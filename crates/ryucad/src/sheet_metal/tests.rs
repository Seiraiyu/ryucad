use super::*;
use crate::rebuild::{Built, finish};
use ryukei::geom::Rigid;
use ryukei::name::OpKey;
use ryukei::topo::EntityId;
use serde_json::json;
use std::collections::BTreeMap;

fn id(s: &str) -> Id {
    Id::parse(s, "feature").unwrap()
}

/// A 40 × 30 × 1.5 mm plate, as a base flange `plate` of part `p` would
/// leave it, its origins its feature's.
fn plate() -> Built {
    let made = ryukei::primitive::block(OpKey(7), 0.04, 0.03, 0.0015, &Rigid::IDENTITY).unwrap();
    let origins: crate::rebuild::Origins = crate::rebuild::names(&made.0)
        .into_iter()
        .map(|n| (n.to_string(), json!({"feature": "plate"})))
        .collect();
    finish(&id("plate"), "base_flange", made, origins).unwrap()
}

fn first(b: &Built, edge: bool) -> ryukei::name::Name {
    let e = if edge {
        EntityId::Edge(b.body.edges().next().unwrap())
    } else {
        EntityId::Vertex(b.body.vertices().next().unwrap())
    };
    b.body.name_of(e).unwrap()
}

#[test]
fn a_kernel_refusal_names_what_it_names_with_a_hint() {
    use ryukei::SheetMetalError as S;
    let b = plate();
    let part = Id::parse("p", "part").unwrap();
    let (edge, vertex) = (first(&b, true), first(&b, false));
    let e = refusal(
        &part,
        &id("f"),
        "flange",
        (&id("plate"), &b),
        ryukei::Error::InvalidSheetMetal(S::ReliefNeeded { edge, vertex }),
    );
    assert_eq!(e.code, Code::SheetMetalFailed);
    assert_eq!(e.details["reason"], "relief_needed");
    assert_eq!(e.details["solid"], "plate");
    let entities = e.details["entities"].as_array().unwrap();
    assert_eq!(entities[0]["role"], "edge");
    assert_eq!(entities[0]["reference"], format!("p/plate/{edge}"));
    assert_eq!(entities[0]["kind"], "edge");
    assert_eq!(entities[0]["origin"], json!({"feature": "plate"}));
    assert_eq!(entities[1]["role"], "vertex");
    let hint = e.details["hint"].as_str().unwrap();
    assert!(
        hint.contains(&format!("--end-relief p/plate/{vertex}=")),
        "{hint}"
    );
    // A fault after the flange's booleans: the inner reason, its
    // entities named, not referenced (the flanged body is gone).
    let inner = S::OutOfSlab {
        face: ryukei::name::Name::from_u128(5),
    };
    let e = refusal(
        &part,
        &id("f"),
        "flange",
        (&id("plate"), &b),
        ryukei::Error::InvalidSheetMetal(S::NotSheetAfter {
            fault: Box::new(inner),
        }),
    );
    assert_eq!(
        (e.details["reason"].as_str(), e.details["fault"].as_str()),
        (Some("not_sheet_after"), Some("out_of_slab"))
    );
    assert_eq!(e.details["entities"][0]["role"], "fault.face");
    assert!(e.details["entities"][0].get("reference").is_none());
    assert!(e.details["entities"][0]["name"].is_string());
    // A size, with which.
    let e = refusal(
        &part,
        &id("f"),
        "flange",
        (&id("plate"), &b),
        ryukei::Error::InvalidSheetMetal(S::BadSize {
            what: ryukei::FlangeSize::Radius,
            edge,
        }),
    );
    assert_eq!(
        (e.details["reason"].as_str(), e.details["what"].as_str()),
        (Some("bad_size"), Some("radius"))
    );
    // A boolean's refusal is boolean_failed, as a cut's.
    let e = refusal(
        &part,
        &id("f"),
        "flange",
        (&id("plate"), &b),
        ryukei::Error::InvalidBoolean(ryukei::BooleanError::Uncertain { a: edge, b: vertex }),
    );
    assert_eq!(e.code, Code::BooleanFailed);
    assert_eq!(e.details["feature"], "f");
}

#[test]
fn every_kernel_reason_has_a_name() {
    use ryukei::SheetMetalError as S;
    let n = ryukei::name::Name::from_u128(1);
    for (e, want) in [
        (S::BadTolerance, "bad_tolerance"),
        (S::NotSheet { face: n }, "not_sheet"),
        (S::MixedThickness { faces: [n, n] }, "mixed_thickness"),
        (S::OutOfSlab { face: n }, "out_of_slab"),
        (S::NotRadial { face: n }, "not_radial"),
        (S::SharpBend { edge: n }, "sharp_bend"),
        (S::ConicalBend { face: n }, "conical_bend"),
        (S::BendBlocked { bend: n, face: n }, "bend_blocked"),
        (S::Disconnected, "disconnected"),
        (
            S::BendLoop {
                bends: vec![n],
                faces: vec![n],
            },
            "bend_loop",
        ),
        (S::NoSuchBend { name: n }, "no_such_bend"),
        (S::NoSuchFace { name: n }, "no_such_face"),
        (S::BadAllowance { bend: n }, "bad_allowance"),
        (S::FlatOverlaps { faces: [n, n] }, "flat_overlaps"),
        (S::FoldCollides { faces: [n, n] }, "fold_collides"),
        (S::Uncertain { at: n }, "uncertain"),
        (S::NoSuchEdge { name: n }, "no_such_edge"),
        (S::NotAFlangeEdge { edge: n }, "not_a_flange_edge"),
        (S::DuplicateFlange { edge: n }, "duplicate_flange"),
        (S::FlangesMeet { vertex: n }, "flanges_meet"),
        (S::ReliefNeeded { edge: n, vertex: n }, "relief_needed"),
    ] {
        assert_eq!(reason(&e).0, want);
    }
}

use crate::rebuild::rebuild_part;
use crate::solid::tests::part;
use crate::units::Unit;
use serde_json::Value;

fn feature(mut v: Value) -> crate::doc::files::Feature {
    v["kind"] = json!("ryucad.feature");
    if v.get("format").is_none() {
        v["format"] = json!(11);
    }
    crate::doc::files::Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap()
}

/// A sketch on xy of points `pts` (mm), joined in order by lines `l1`,
/// `l2`, …, closed back to the first when `closed`.
fn polyline(pts: &[[f64; 2]], closed: bool) -> crate::doc::files::Feature {
    let mut entities = serde_json::Map::new();
    for (i, p) in pts.iter().enumerate() {
        entities.insert(
            format!("p{}", i + 1),
            json!({"type": "point", "m": [p[0] * 1e-3, p[1] * 1e-3]}),
        );
    }
    let n = if closed { pts.len() } else { pts.len() - 1 };
    for i in 0..n {
        let j = (i + 1) % pts.len();
        entities.insert(
            format!("l{}", i + 1),
            json!({"type": "line", "start": format!("p{}", i + 1), "end": format!("p{}", j + 1)}),
        );
    }
    feature(
        json!({"format": 2, "type": "sketch", "plane": "xy", "entities": entities, "constraints": {}}),
    )
}

/// The base flange `b` of the sketch `s`: `extra`'s fields over a 1.5 mm
/// sheet with a 1 mm radius and a K-factor of 0.44.
fn base_of(extra: Value) -> crate::doc::files::Feature {
    let mut v = json!({"type": "base_flange", "sketch": "s", "thickness": "1.5 mm", "radius": "1 mm",
                       "allowance": {"k_factor": "0.44"}});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    feature(v)
}

fn built(p: &crate::doc::project::Part, f: &str) -> crate::error::Result<std::sync::Arc<Built>> {
    let r = rebuild_part(p);
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == f)
        .unwrap()
        .clone();
    out.map(|o| std::sync::Arc::new(o.solid(&id(f)).unwrap().clone()))
}

fn volume(b: &ryukei::topo::Body) -> f64 {
    let m = ryukei::measure::mass(b, 1.0, &ryukei::measure::MassOptions::default()).unwrap();
    m.volume.value() * 1e9
}

#[test]
fn a_plate_is_its_sketch_extruded_by_its_thickness_and_named_as_an_extrude() {
    let p = part(vec![
        (
            "s",
            polyline(&[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]], true),
        ),
        ("b", base_of(json!({}))),
    ]);
    let b = built(&p, "b").unwrap();
    assert!((volume(&b.body) - 60.0 * 40.0 * 1.5).abs() < 1e-9);
    // Named as the same extrude is: the same op key and the sketch's keys.
    let e = part(vec![
        (
            "s",
            polyline(&[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]], true),
        ),
        (
            "b",
            feature(json!({"format": 2, "type": "extrude", "sketch": "s",
                                           "extent": {"type": "blind", "length": "1.5 mm"}})),
        ),
    ]);
    let x = built(&e, "b").unwrap();
    assert_eq!(
        crate::rebuild::names(&b.body),
        crate::rebuild::names(&x.body)
    );
    // Its origins say it made them, from which sketch entity.
    let side = b
        .origins
        .values()
        .find(|o| o.get("entity").is_some())
        .unwrap();
    assert_eq!(side["feature"], "b");
    assert_eq!(side["sketch"], "s");
    // Reversed, it lies below the sketch.
    let p = part(vec![
        (
            "s",
            polyline(&[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]], true),
        ),
        ("b", base_of(json!({"reverse": true}))),
    ]);
    let (lo, hi) = crate::inspect::body_extent(&built(&p, "b").unwrap().body).unwrap();
    assert!((lo[2] + 0.0015).abs() < 1e-12 && hi[2].abs() < 1e-12);
}

#[test]
fn a_bent_section_is_its_chain_offset_and_extruded_by_its_depth() {
    // An L: from (40, 0) to the origin, then up; the material on its left.
    let (t, r, d) = (1.5, 1.0, 20.0);
    let p = part(vec![
        (
            "s",
            polyline(&[[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]], false),
        ),
        ("b", base_of(json!({"depth": "20 mm"}))),
    ]);
    let b = built(&p, "b").unwrap();
    let area = t * ((40.0 - r) + (30.0 - r)) + core::f64::consts::FRAC_PI_2 * t * (r + t / 2.0);
    let v = volume(&b.body);
    assert!(
        (v - area * d).abs() < 1e-6 * area * d,
        "{v} against {}",
        area * d
    );
    // Recognized: one bend of the inner radius, a quarter turn.
    let m = ryukei::sheet_metal::recognize(&b.body, &Default::default()).unwrap();
    assert_eq!(m.bends.len(), 1);
    assert!((m.bends[0].radius - 1e-3).abs() < 1e-12);
    assert!((m.thickness - 1.5e-3).abs() < 1e-12);
    // Symmetric about the sketch's plane.
    let p = part(vec![
        (
            "s",
            polyline(&[[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]], false),
        ),
        ("b", base_of(json!({"depth": "20 mm", "symmetric": true}))),
    ]);
    let (lo, hi) = crate::inspect::body_extent(&built(&p, "b").unwrap().body).unwrap();
    assert!((lo[2] + 0.01).abs() < 1e-12 && (hi[2] - 0.01).abs() < 1e-12);
}

#[test]
fn a_bent_sections_names_last_across_its_sizes() {
    let names = |t: &str, r: &str, y: f64| {
        let p = part(vec![
            ("s", polyline(&[[40.0, 0.0], [0.0, 0.0], [0.0, y]], false)),
            (
                "b",
                base_of(json!({"depth": "20 mm", "thickness": t, "radius": r})),
            ),
        ]);
        crate::rebuild::names(&built(&p, "b").unwrap().body)
    };
    let first = names("1.5 mm", "1 mm", 30.0);
    assert_eq!(names("2 mm", "1 mm", 30.0), first);
    assert_eq!(names("1.5 mm", "3 mm", 30.0), first);
    assert_eq!(names("1.5 mm", "1 mm", 45.0), first);
}

#[test]
fn base_flanges_that_cant_be_made_say_why() {
    let poly = |pts: &[[f64; 2]], closed| polyline(pts, closed);
    // A plate from a sketch with no closed loop.
    let p = part(vec![
        ("s", poly(&[[0.0, 0.0], [10.0, 0.0]], false)),
        ("b", base_of(json!({}))),
    ]);
    assert_eq!(built(&p, "b").unwrap_err().code, Code::ProfileInvalid);
    // A section from a sketch with no open chain, or two.
    let p = part(vec![
        ("s", poly(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]], true)),
        ("b", base_of(json!({"depth": "5 mm"}))),
    ]);
    assert_eq!(built(&p, "b").unwrap_err().code, Code::ProfileInvalid);
    // A corner whose bends eat a line.
    let p = part(vec![
        (
            "s",
            poly(&[[0.0, 10.0], [0.0, 0.0], [1.5, 0.0], [1.5, 10.0]], false),
        ),
        ("b", base_of(json!({"depth": "5 mm"}))),
    ]);
    let e = built(&p, "b").unwrap_err();
    assert_eq!(
        (e.code, e.details["entities"].clone()),
        (Code::ProfileInvalid, json!(["l2"]))
    );
    // A corner bent at a radius under the resolution (its arc would be
    // no circle): refused at the radius, never a panic.
    for radius in ["0.00001 mm", "1e-12 mm"] {
        let p = part(vec![
            ("s", poly(&[[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]], false)),
            ("b", base_of(json!({"depth": "20 mm", "radius": radius}))),
        ]);
        let e = built(&p, "b").unwrap_err();
        assert_eq!(
            (
                e.code,
                e.details["pointer"].clone(),
                e.details["entities"].clone()
            ),
            (Code::ProfileInvalid, json!("/radius"), json!(["l1", "l2"])),
            "{e}"
        );
    }
    // A plate thinner across its sketch than its thickness isn't read as
    // the sheet it says: recognition reads it as some other sheet (its
    // thinnest way) and refuses its corners, or reads another thickness.
    let p = part(vec![
        (
            "s",
            poly(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], true),
        ),
        ("b", base_of(json!({"thickness": "5 mm"}))),
    ]);
    let e = built(&p, "b").unwrap_err();
    assert_eq!(e.code, Code::SheetMetalFailed, "{e}");
    assert!(
        matches!(
            e.details["reason"].as_str(),
            Some("thickness_mismatch" | "sharp_bend")
        ),
        "{e}"
    );
}

use crate::solid::Solids;

const PLATE: [[f64; 2]; 4] = [[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]];

/// The edge of `b` from `a` to `c` (mm), by name.
fn edge_at(b: &ryukei::topo::Body, a: [f64; 3], c: [f64; 3]) -> ryukei::name::Name {
    let near = |p: ryukei::geom::Point3, q: [f64; 3]| {
        (p.x - q[0] * 1e-3).abs() < 1e-9
            && (p.y - q[1] * 1e-3).abs() < 1e-9
            && (p.z - q[2] * 1e-3).abs() < 1e-9
    };
    b.edges()
        .find(|e| {
            let x = b.edge(*e).unwrap();
            let (s, t) = (
                b.vertex(x.start).unwrap().point,
                b.vertex(x.end).unwrap().point,
            );
            (near(s, a) && near(t, c)) || (near(s, c) && near(t, a))
        })
        .map(|e| b.name_of(EntityId::Edge(e)).unwrap())
        .unwrap_or_else(|| panic!("no edge from {a:?} to {c:?}"))
}

/// The vertex of `b` at `a` (mm), by name.
fn vertex_at(b: &ryukei::topo::Body, a: [f64; 3]) -> ryukei::name::Name {
    b.vertices()
        .find(|v| {
            let p = b.vertex(*v).unwrap().point;
            (p.x - a[0] * 1e-3).abs() < 1e-9
                && (p.y - a[1] * 1e-3).abs() < 1e-9
                && (p.z - a[2] * 1e-3).abs() < 1e-9
        })
        .map(|v| b.name_of(EntityId::Vertex(v)).unwrap())
        .unwrap_or_else(|| panic!("no vertex at {a:?}"))
}

/// The plate's right top edge (x = 60, z = 1.5) and its two vertices.
fn rim(b: &ryukei::topo::Body) -> (ryukei::name::Name, [ryukei::name::Name; 2]) {
    let (a, c) = ([60.0, 0.0, 1.5], [60.0, 40.0, 1.5]);
    (edge_at(b, a, c), [vertex_at(b, a), vertex_at(b, c)])
}

fn r(feature: &str, n: ryukei::name::Name) -> Value {
    json!({"feature": feature, "name": n.to_string()})
}

fn plated() -> (crate::doc::project::Part, std::sync::Arc<Built>) {
    let p = part(vec![
        ("s", polyline(&PLATE, true)),
        ("plate", base_of(json!({}))),
    ]);
    let b = built(&p, "plate").unwrap();
    (p, b)
}

#[test]
fn a_flanges_ends_are_its_vertices_and_its_settings_its_founders() {
    let (_, b) = plated();
    let (edge, [v0, v1]) = rim(&b.body);
    let f = feature(
        json!({"type": "flange", "target": "plate", "edges": [r("plate", edge)],
                           "angle": "90 deg", "length": "20 mm", "setback": "2 mm",
                           "ends": [{"vertex": r("plate", v1), "setback": "5 mm",
                                     "relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}]}),
    );
    let p = part(vec![
        ("s", polyline(&PLATE, true)),
        ("plate", base_of(json!({}))),
        ("f", f),
    ]);
    let results = rebuild_part(&p);
    let x = p.features[&id("f")].as_ref().unwrap().flange().unwrap();
    let got = resolve(&p, &Solids::of(&p), &id("f"), x, &results).unwrap();
    let k = &got.flanges[0];
    assert_eq!(k.edge, edge);
    assert!((k.radius - 1e-3).abs() < 1e-15, "the founder's radius");
    assert!((k.angle - core::f64::consts::FRAC_PI_2).abs() < 1e-15);
    // Which end is v1 is the edge's direction's business.
    let e = b
        .body
        .edge(match b.body.by_name(edge) {
            Some(EntityId::Edge(e)) => e,
            _ => unreachable!(),
        })
        .unwrap();
    let at = if b.body.vertex(e.start).unwrap().name == v1 {
        0
    } else {
        1
    };
    assert_eq!(k.setbacks[at], 5e-3);
    assert_eq!(k.setbacks[1 - at], 2e-3);
    assert_eq!(
        k.reliefs[at],
        ryukei::sheet_metal::Relief::Rectangular {
            width: 1e-3,
            depth: 5e-4
        }
    );
    assert_eq!(k.reliefs[1 - at], ryukei::sheet_metal::Relief::None);
    let _ = v0;
}

#[test]
fn a_flange_says_what_it_cant_resolve() {
    let (_, b) = plated();
    let (edge, [v0, _]) = rim(&b.body);
    let face = b
        .body
        .name_of(EntityId::Face(b.body.faces().next().unwrap()))
        .unwrap();
    let flange = |extra: Value| {
        let mut v = json!({"type": "flange", "target": "plate", "edges": [r("plate", edge)],
                           "angle": "90 deg", "length": "20 mm"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        feature(v)
    };
    let fails = |f, founder: crate::doc::files::Feature| {
        let p = part(vec![
            ("s", polyline(&PLATE, true)),
            ("plate", founder),
            ("f", f),
        ]);
        let results = rebuild_part(&p);
        let x = p.features[&id("f")]
            .as_ref()
            .unwrap()
            .flange()
            .unwrap()
            .clone();
        resolve(&p, &Solids::of(&p), &id("f"), &x, &results)
            .map(drop)
            .unwrap_err()
    };
    // A face where an edge goes.
    let e = fails(
        flange(json!({"edges": [r("plate", face)]})),
        base_of(json!({})),
    );
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    // A vertex that isn't an end of an edge it flanges: a corner far away.
    let far = vertex_at(&b.body, [0.0, 0.0, 0.0]);
    let e = fails(
        flange(json!({"ends": [{"vertex": r("plate", far), "setback": "1 mm"}]})),
        base_of(json!({})),
    );
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/ends/0/vertex")),
        "{e}"
    );
    let _ = v0;
    // A solid founded by an extrude has no settings to give: the radius
    // must be the flange's own.
    let extrude = feature(json!({"format": 2, "type": "extrude", "sketch": "s",
                                 "extent": {"type": "blind", "length": "1.5 mm"}}));
    let e = fails(flange(json!({})), extrude);
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/radius")),
        "{e}"
    );
}

fn with_flange(extra: Value) -> crate::doc::project::Part {
    let (_, b) = plated();
    let (edge, _) = rim(&b.body);
    let mut v = json!({"type": "flange", "target": "plate", "edges": [r("plate", edge)],
                       "angle": "90 deg", "length": "20 mm"});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    part(vec![
        ("s", polyline(&PLATE, true)),
        ("plate", base_of(json!({}))),
        ("f", feature(v)),
    ])
}

#[test]
fn a_square_flange_makes_an_l_of_its_closed_form() {
    // Material inside: the plate cut back by r + t, a quarter annulus, and
    // a straight flange of the length to the outer mould less r + t.
    let p = with_flange(json!({}));
    let r = rebuild_part(&p);
    let out = r
        .output(&id("f"))
        .unwrap_or_else(|| panic!("{:?}", r.features));
    let b = &out.solid(&id("plate")).unwrap().body;
    let (w, a, t, rr, l) = (40.0, 60.0, 1.5, 1.0, 20.0);
    let want = w * t * (a - (rr + t))
        + w * core::f64::consts::FRAC_PI_4 * ((rr + t) * (rr + t) - rr * rr)
        + w * t * (l - (rr + t));
    let m = ryukei::measure::mass(b, 1.0, &ryukei::measure::MassOptions::default()).unwrap();
    assert!(
        m.volume.holds(want * 1e-9, 1e-15),
        "{:?} against {want}",
        m.volume
    );
    assert_eq!(ryukei::check::check_intersections(b), []);
    // Recognized: one bend, the founder's radius.
    let model = ryukei::sheet_metal::recognize(b, &Default::default()).unwrap();
    assert_eq!(model.bends.len(), 1);
    // The bend's and the flange's faces say which they are and whose.
    let built = out.solid(&id("plate")).unwrap();
    let parts: std::collections::BTreeSet<String> = built
        .origins
        .values()
        .filter(|o| o["feature"] == "f")
        .filter_map(|o| o["part"].as_str().map(str::to_owned))
        .collect();
    assert!(
        parts.contains("bend") && parts.contains("flange"),
        "{parts:?}"
    );
    let (edge, _) = rim(&built_of(&p, "plate").body);
    assert!(
        built
            .origins
            .values()
            .any(|o| o["flange_of"] == edge.to_string())
    );
}

fn built_of(p: &crate::doc::project::Part, f: &str) -> std::sync::Arc<Built> {
    std::sync::Arc::new(
        rebuild_part(p)
            .output(&id(f))
            .unwrap()
            .solid(&id("plate"))
            .unwrap()
            .clone(),
    )
}

#[test]
fn a_flanges_names_last_across_its_sizes() {
    let names = |extra: Value| crate::rebuild::names(&built_of(&with_flange(extra), "f").body);
    let first = names(json!({}));
    for edit in [
        json!({"angle": "80 deg"}),
        json!({"length": "25 mm"}),
        json!({"radius": "2 mm"}),
    ] {
        assert_eq!(names(edit.clone()), first, "{edit}: names moved");
    }
    // A setback cuts the base anew (the trim no longer runs the edge's
    // whole length, and a relief adds faces), so the base's cut faces are
    // others, and so are its end walls and edges at its set-back ends; the
    // bend's cylinders and the flange's sides keep their names.
    let own = |extra: Value| -> std::collections::BTreeSet<String> {
        let b = built_of(&with_flange(extra), "f");
        b.origins
            .iter()
            .filter(|(_, o)| {
                o["feature"] == "f" && matches!(o["part"].as_str(), Some("bend" | "flange"))
            })
            .filter(|(n, _)| {
                // The bend's cylinders (their axis along the edge, y) and
                // the flange's sides (planes facing across it): the end
                // walls, square to y, are cut anew by a setback.
                let n = crate::doc::reference::parse_name(n).expect("a name");
                let Some(EntityId::Face(f)) = b.body.by_name(n) else {
                    return false;
                };
                let face = b.body.face(f).expect("a live face");
                let surface = b.body.surface(face.surface).expect("its surface");
                let along = surface.frame().map_or(0.0, |f| f.z().get().y.abs());
                match surface {
                    ryukei::geom::Surface::Cylinder(_) => along > 1.0 - 1e-9,
                    ryukei::geom::Surface::Plane(_) => along < 1e-9,
                    _ => false,
                }
            })
            .map(|(n, _)| n.clone())
            .collect()
    };
    let set = json!({"setback": "3 mm", "relief": {"kind": "rectangular", "width": "1 mm", "depth": "1 mm"}});
    let (before, after) = (own(json!({})), own(set));
    assert!(!before.is_empty());
    assert!(
        before.is_subset(&after),
        "the bend's or flange's names moved: {before:?} against {after:?}"
    );
}

#[test]
fn the_founders_settings_key_a_flanges_result() {
    let hash = |radius: &str| {
        let (_, b) = plated();
        let (edge, _) = rim(&b.body);
        let f = feature(
            json!({"type": "flange", "target": "plate", "edges": [r("plate", edge)],
                               "angle": "90 deg", "length": "20 mm"}),
        );
        let p = part(vec![
            ("s", polyline(&PLATE, true)),
            ("plate", base_of(json!({"radius": radius}))),
            ("f", f.clone()),
        ]);
        let before: BTreeMap<Id, crate::error::Result<crate::rebuild::Output>> =
            rebuild_part(&p).features.into_iter().collect();
        crate::rebuild::input_hash(
            &p,
            &Solids::of(&p),
            &id("f"),
            &f,
            &before,
            &crate::rebuild::Run::none(),
        )
    };
    // The plate is the same body either way; only the flange reads it.
    assert_ne!(hash("1 mm"), hash("2 mm"));
}

/// The plate flanged on its right top edge, 90° and 20 mm, then `extra`.
fn flanged(extra: Vec<(&'static str, crate::doc::files::Feature)>) -> crate::doc::project::Part {
    let (_, plate) = plated();
    let (edge, _) = rim(&plate.body);
    let mut fs = vec![
        ("s", polyline(&PLATE, true)),
        ("plate", base_of(json!({}))),
        (
            "f",
            feature(
                json!({"type": "flange", "target": "plate", "edges": [r("plate", edge)],
                           "angle": "90 deg", "length": "20 mm"}),
            ),
        ),
    ];
    fs.extend(extra);
    part(fs)
}

/// Feature `f`'s output in `p`'s rebuild.
fn output_of(
    p: &crate::doc::project::Part,
    f: &str,
) -> crate::error::Result<crate::rebuild::Output> {
    rebuild_part(p)
        .features
        .into_iter()
        .find(|(i, _)| i.as_str() == f)
        .expect("listed")
        .1
}

#[test]
fn a_made_with_its_flat_goes_through_the_cache_and_one_without_keeps_its_bytes() {
    use crate::cache::results::{decode_made, encode_made};
    use crate::rebuild::{Flat, FlatBend, Made};
    use std::sync::Arc;
    let p = flanged(vec![]);
    let folded = output_of(&p, "f")
        .unwrap()
        .solid(&id("plate"))
        .unwrap()
        .clone();
    let model = ryukei::sheet_metal::recognize(&folded.body, &Default::default()).unwrap();
    let u = ryukei::sheet_metal::unfold(
        OpKey(1),
        &folded.body,
        &model,
        &ryukei::sheet_metal::UnfoldSpec::all(ryukei::sheet_metal::Allowance::KFactor(0.44)),
    )
    .unwrap();
    let b = &model.bends[0];
    let flat = Flat {
        unfold: id("u"),
        map: u.map.clone(),
        pattern: u.pattern.clone(),
        bends: vec![FlatBend {
            name: b.name,
            inner: b.inner.clone(),
            outer: b.outer.clone(),
            allowance: "K0.44".into(),
        }],
        partial: false,
    };
    let plain = Made::new(vec![(id("plate"), Arc::new(folded))]);
    let with = Made {
        flat: Some(Arc::new(flat.clone())),
        ..plain.clone()
    };
    let (a, b) = (encode_made(&plain), encode_made(&with));
    // A flat is only ever appended: an entry without one is as it was.
    assert!(b.len() > a.len() && b.starts_with(&a));
    assert!(decode_made(&a).unwrap().flat.is_none());
    let back = decode_made(&b).unwrap();
    assert_eq!(back.flat.as_deref(), Some(&flat));
    assert_eq!(encode_made(&back), b);
    // Anything short of a whole flat, or past it, is a miss.
    for n in [a.len() + 1, a.len() + 20, b.len() - 1] {
        assert!(decode_made(&b[..n]).is_none(), "{n}");
    }
    let mut longer = b.clone();
    longer.push(0);
    assert!(decode_made(&longer).is_none());
}

/// A refold names a new edge in a folded bend's end plane by that bend
/// (`bend_line_of`): the folded flange's edges along its bend lines are
/// such edges, its plate's far rim and its faces aren't.
#[test]
fn an_edge_in_a_bends_end_plane_is_its_bend_line() {
    use crate::rebuild::{Flat, FlatBend};
    let p = flanged(vec![]);
    let folded = output_of(&p, "f")
        .unwrap()
        .solid(&id("plate"))
        .unwrap()
        .clone();
    let model = ryukei::sheet_metal::recognize(&folded.body, &Default::default()).unwrap();
    let u = ryukei::sheet_metal::unfold(
        OpKey(1),
        &folded.body,
        &model,
        &ryukei::sheet_metal::UnfoldSpec::all(ryukei::sheet_metal::Allowance::KFactor(0.44)),
    )
    .unwrap();
    let b = &model.bends[0];
    let flat = Flat {
        unfold: id("u"),
        map: u.map,
        pattern: u.pattern,
        bends: vec![FlatBend {
            name: b.name,
            inner: b.inner.clone(),
            outer: b.outer.clone(),
            allowance: "K0.44".into(),
        }],
        partial: false,
    };
    let body = &folded.body;
    let mut lines = 0;
    for e in body.edges() {
        let n = body.name_of(EntityId::Edge(e)).unwrap();
        if let Some(fb) = unfold::bend_line_of(body, n, &flat) {
            assert_eq!(fb.name, b.name);
            lines += 1;
        }
    }
    // Each end plane holds the bend's two lines (inner and outer).
    assert!(lines >= 4, "{lines}");
    assert!(lines < body.edges().count());
    // The plate's rim opposite the flange lies in neither end plane.
    let far = edge_at(body, [0.0, 40.0, 1.5], [0.0, 0.0, 1.5]);
    assert!(unfold::bend_line_of(body, far, &flat).is_none());
    // A face is no edge.
    assert!(unfold::bend_line_of(body, b.inner[0], &flat).is_none());
}

fn unfold_of(extra: Value) -> crate::doc::files::Feature {
    let mut v = json!({"format": 14, "type": "unfold", "target": "plate"});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    feature(v)
}

/// The flange's bend as recognized, and the plate's top face (z = 1.5,
/// facing +z).
fn bend_and_top(p: &crate::doc::project::Part) -> (ryukei::sheet_metal::Bend, ryukei::name::Name) {
    let f = output_of(p, "f")
        .unwrap()
        .solid(&id("plate"))
        .unwrap()
        .clone();
    let model = ryukei::sheet_metal::recognize(&f.body, &Default::default()).unwrap();
    let top = model
        .flanges
        .iter()
        .flat_map(|g| g.sides.iter())
        .find(|s| {
            let z = s.plane.frame().z().get();
            (z.z - 1.0).abs() < 1e-12 && (s.plane.frame().origin().z - 1.5e-3).abs() < 1e-12
        })
        .expect("the plate's top side")
        .faces[0];
    (model.bends[0].clone(), top)
}

#[test]
fn an_unfold_lays_a_flange_flat_keeping_every_name_and_origin() {
    let p0 = flanged(vec![]);
    let (bend, top) = bend_and_top(&p0);
    let p = flanged(vec![(
        "u",
        unfold_of(json!({"fixed": r("plate", top), "allowance": {"k_factor": "0.5"}})),
    )]);
    let folded = output_of(&p, "f")
        .unwrap()
        .solid(&id("plate"))
        .unwrap()
        .clone();
    let out = output_of(&p, "u").unwrap();
    let flat = out.solid(&id("plate")).unwrap();
    // Flat: recognized with no bends, every name and origin kept.
    let m = ryukei::sheet_metal::recognize(&flat.body, &Default::default()).unwrap();
    assert!(m.bends.is_empty());
    assert_eq!(
        crate::rebuild::names(&flat.body),
        crate::rebuild::names(&folded.body)
    );
    assert_eq!(flat.origins, folded.origins);
    // The plate stayed where it was: its top face still at z = 1.5.
    assert!(flat.body.by_name(top).is_some());
    // At a K-factor of a half, the volume is kept.
    assert!((volume(&flat.body) - volume(&folded.body)).abs() < 1e-3);
    // The flat: one strip, its bend named by its faces, its note.
    let fl = out.flat().expect("an unfold's flat");
    assert_eq!(fl.unfold, id("u"));
    assert!(!fl.partial);
    assert_eq!(fl.bends.len(), 1);
    assert_eq!(fl.bends[0].name, bend.name);
    assert_eq!(fl.bends[0].allowance, "K0.5");
    assert!(fl.bend_of(bend.outer[0]).is_some());
    let want = bend.angle * (bend.radius + 0.5 * 1.5e-3);
    assert!((fl.map.strips[0].length - want).abs() < 1e-15);
}

#[test]
fn an_unfolds_allowance_is_its_founders_its_own_or_its_bends() {
    let p0 = flanged(vec![]);
    let (bend, _) = bend_and_top(&p0);
    let (r_, t) = (1e-3, 1.5e-3);
    let length = |extra: Value| -> f64 {
        let p = flanged(vec![("u", unfold_of(extra))]);
        output_of(&p, "u").unwrap().flat().unwrap().map.strips[0].length
    };
    // The founder's K-factor of 0.44.
    let k = length(json!({}));
    assert!((k - bend.angle * (r_ + 0.44 * t)).abs() < 1e-15, "{k}");
    // Its own deduction: 2 (r + t) tan(θ / 2) - bd.
    let d = length(json!({"allowance": {"deduction": "2.4 mm"}}));
    let want = 2.0 * (r_ + t) * ryukei::math::tan(bend.angle / 2.0) - 2.4e-3;
    assert!((d - want).abs() < 1e-15, "{d}");
    // The bend's own developed length, over both.
    let a = length(json!({"allowance": {"deduction": "2.4 mm"},
                          "bends": [{"bend": r("plate", bend.inner[0]), "allowance": "2 mm"}]}));
    assert!((a - 2e-3).abs() < 1e-15, "{a}");
    // The founder's allowance keys an unfold with none of its own.
    let p = flanged(vec![("u", unfold_of(json!({})))]);
    let u = p.features[&id("u")].as_ref().unwrap();
    assert!(founder_settings(&p, u).is_some());
    let p = flanged(vec![(
        "u",
        unfold_of(json!({"allowance": {"k_factor": "0.3"}})),
    )]);
    let u = p.features[&id("u")].as_ref().unwrap();
    assert!(founder_settings(&p, u).is_none());
}

#[test]
fn unfolds_that_cant_be_made_say_where() {
    let p0 = flanged(vec![]);
    let (bend, top) = bend_and_top(&p0);
    let pointer = |extra: Value| -> (crate::error::Code, Value) {
        let p = flanged(vec![("u", unfold_of(extra))]);
        let e = output_of(&p, "u").unwrap_err();
        (e.code, e.details["pointer"].clone())
    };
    // A bend's face held still.
    assert_eq!(
        pointer(json!({"fixed": r("plate", bend.inner[0])})),
        (crate::error::Code::InvalidParam, json!("/fixed"))
    );
    // A side face where a bend goes.
    assert_eq!(
        pointer(json!({"only": [r("plate", top)]})),
        (crate::error::Code::InvalidParam, json!("/only/0"))
    );
    assert_eq!(
        pointer(json!({"bends": [{"bend": r("plate", top), "k_factor": "0.4"}]})),
        (crate::error::Code::InvalidParam, json!("/bends/0/bend"))
    );
    // A hem by a deduction: no outer mould line at a half turn.
    let (_, plate) = plated();
    let (edge, _) = rim(&plate.body);
    let p = part(vec![
        ("s", polyline(&PLATE, true)),
        (
            "plate",
            base_of(json!({"allowance": {"deduction": "2 mm"}})),
        ),
        (
            "f",
            feature(
                json!({"type": "flange", "target": "plate", "edges": [r("plate", edge)],
                           "angle": "180 deg", "length": "10 mm",
                           "measured": "tangent"}),
            ),
        ),
        ("u", unfold_of(json!({}))),
    ]);
    let e = output_of(&p, "u").unwrap_err();
    assert_eq!(
        (e.code, e.details["pointer"].clone()),
        (crate::error::Code::InvalidParam, json!("/allowance")),
        "{e}"
    );
    assert!(e.message.contains("half turn"), "{e}");
}

fn refold_of(extra: Value) -> crate::doc::files::Feature {
    let mut v = json!({"format": 14, "type": "refold", "target": "plate", "unfold": "u"});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    feature(v)
}

/// A 4 × 4 mm slot sketched on xy across the strip of an unfold about the
/// plate's top at a K-factor of a half, cut through.
fn slot() -> Vec<(&'static str, crate::doc::files::Feature)> {
    // The strip runs from x = 60 - (r + t) = 57.5 for θ (r + t / 2).
    let ba = core::f64::consts::FRAC_PI_2 * 1.75;
    let mid = 57.5 + ba / 2.0;
    vec![
        (
            "slot_s",
            polyline(
                &[
                    [mid - 2.0, 18.0],
                    [mid + 2.0, 18.0],
                    [mid + 2.0, 22.0],
                    [mid - 2.0, 22.0],
                ],
                true,
            ),
        ),
        (
            "slot",
            feature(
                json!({"format": 3, "type": "extrude", "sketch": "slot_s", "operation": "cut",
                           "targets": ["plate"], "extent": {"type": "through_all", "both": true}}),
            ),
        ),
    ]
}

#[test]
fn a_refold_folds_its_unfold_back() {
    let p0 = flanged(vec![]);
    let (bend, top) = bend_and_top(&p0);
    let p = flanged(vec![
        ("u", unfold_of(json!({"fixed": r("plate", top)}))),
        ("rf", refold_of(json!({}))),
    ]);
    let folded = output_of(&p, "f")
        .unwrap()
        .solid(&id("plate"))
        .unwrap()
        .clone();
    let back = output_of(&p, "rf")
        .unwrap()
        .solid(&id("plate"))
        .unwrap()
        .clone();
    assert_eq!(
        crate::rebuild::names(&back.body),
        crate::rebuild::names(&folded.body)
    );
    for (n, o) in folded.origins.iter() {
        assert_eq!(back.origins.get(n), Some(o), "{n}");
    }
    let m = ryukei::sheet_metal::recognize(&back.body, &Default::default()).unwrap();
    assert_eq!(m.bends.len(), 1);
    assert!((m.bends[0].angle - bend.angle).abs() < 1e-9);
    assert!((m.bends[0].radius - bend.radius).abs() < 1e-12);
    assert!((volume(&back.body) - volume(&folded.body)).abs() < 1e-6 * volume(&folded.body));
}

#[test]
fn a_slot_cut_across_the_bend_in_the_flat_folds_with_it() {
    let p0 = flanged(vec![]);
    let (bend, top) = bend_and_top(&p0);
    let mut extra = vec![(
        "u",
        unfold_of(json!({"fixed": r("plate", top), "allowance": {"k_factor": "0.5"}})),
    )];
    extra.extend(slot());
    extra.push(("rf", refold_of(json!({}))));
    let p = flanged(extra);
    let cut = output_of(&p, "slot")
        .unwrap()
        .solid(&id("plate"))
        .unwrap()
        .clone();
    let back = output_of(&p, "rf")
        .unwrap()
        .solid(&id("plate"))
        .unwrap()
        .clone();
    assert!(ryukei::check::check(&back.body).is_empty());
    assert_eq!(ryukei::check::check_intersections(&back.body), []);
    // A half K-factor keeps the volume of a cut through the sheet.
    let v = volume(&cut.body);
    assert!((volume(&back.body) - v).abs() < 1e-6 * v);
    // The cut in the flat already split the slot's walls at the bend
    // lines, so the refold's imprint makes nothing new: every name, and
    // the origin of every name, is the cut's.
    assert_eq!(
        crate::rebuild::names(&back.body),
        crate::rebuild::names(&cut.body)
    );
    // A new edge the imprint makes on a folded bend's end plane would be
    // that bend's line.
    let lines: Vec<&Value> = back
        .origins
        .values()
        .filter(|o| o.get("bend_line_of").is_some())
        .collect();
    for o in lines {
        assert_eq!(o["feature"], "rf");
        assert_eq!(o["bend_line_of"], bend.inner[0].to_string());
    }
}

#[test]
fn refolds_that_cant_be_made_say_where() {
    let p0 = flanged(vec![]);
    let (_, top) = bend_and_top(&p0);
    let at = |extra: Vec<(&'static str, crate::doc::files::Feature)>, f: &str| -> Value {
        let p = flanged(extra);
        let e = output_of(&p, f).unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidParam, "{e}");
        e.details["pointer"].clone()
    };
    // Not an unfold.
    assert_eq!(
        at(vec![("rf", refold_of(json!({"unfold": "f"})))], "rf"),
        json!("/unfold")
    );
    // Folded once already.
    assert_eq!(
        at(
            vec![
                ("u", unfold_of(json!({}))),
                ("rf", refold_of(json!({}))),
                ("rf2", refold_of(json!({}))),
            ],
            "rf2"
        ),
        json!("/unfold")
    );
    // A side face where a bend goes.
    assert_eq!(
        at(
            vec![
                ("u", unfold_of(json!({}))),
                ("rf", refold_of(json!({"only": [r("plate", top)]}))),
            ],
            "rf"
        ),
        json!("/only/0")
    );
}

/// Through the protocol, in `part` (made here): a 60 × 40 × 1.5 mm plate
/// `plate` (K 0.44, rectangular reliefs 1 × 0.5 mm) with one flange `f`
/// on its four top edges, 90° and 15 mm, set back 5 mm: a tray. Later
/// tasks' tests share it.
pub(crate) fn tray(s: &mut crate::session::Session, part: &str) {
    use crate::server::tests::call;
    call(s, "part.create", json!({"part": part})).unwrap();
    call(
        s,
        "feature.add",
        json!({"part": part, "type": "sketch", "id": "s", "plane": "xy"}),
    )
    .unwrap();
    let mut ops = Vec::new();
    for (i, at) in PLATE.iter().enumerate() {
        ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    for i in 1..=4 {
        ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                        "start": format!("p{i}"), "end": format!("p{}", i % 4 + 1)}));
    }
    call(
        s,
        "sketch.edit",
        json!({"part": part, "sketch": "s", "ops": ops}),
    )
    .unwrap();
    call(
        s,
        "feature.add",
        json!({"part": part, "type": "base_flange", "id": "plate",
        "sketch": "s", "thickness": "1.5 mm", "radius": "1 mm", "allowance": {"k_factor": "0.44"},
        "relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}),
    )
    .unwrap();
    let pid = Id::parse(part, "part").unwrap();
    let b = s
        .rebuilt(&pid)
        .unwrap()
        .output(&id("plate"))
        .unwrap()
        .solid(&id("plate"))
        .unwrap()
        .body
        .clone();
    let z = 1.5;
    let corners = [
        [0.0, 0.0, z],
        [60.0, 0.0, z],
        [60.0, 40.0, z],
        [0.0, 40.0, z],
    ];
    let edges: Vec<String> = (0..4)
        .map(|i| {
            format!(
                "{part}/plate/{}",
                edge_at(&b, corners[i], corners[(i + 1) % 4])
            )
        })
        .collect();
    call(
        s,
        "feature.add",
        json!({"part": part, "type": "flange", "id": "f", "target": "plate",
        "edges": edges, "angle": "90 deg", "length": "15 mm", "setback": "5 mm"}),
    )
    .unwrap();
}

#[test]
fn the_sheet_report_says_flat_partly_or_folded() {
    use crate::server::tests::{call, fresh};
    let (_dir, mut s) = fresh();
    tray(&mut s, "p");
    let state = |s: &mut crate::session::Session| -> Value {
        let v = call(s, "sheet.get", json!({"part": "p"})).unwrap();
        json!([
            v["state"],
            v["unfold"],
            v["unfolded"].as_array().map_or(0, Vec::len)
        ])
    };
    assert_eq!(state(&mut s), json!(["folded", null, 0]));
    let sheet = call(&mut s, "sheet.get", json!({"part": "p"})).unwrap();
    let bends: Vec<String> = sheet["bends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["bend"].as_str().unwrap().to_owned())
        .collect();
    // Two bends of four unfolded: partly.
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "unfold", "id": "u",
        "only": [bends[0], bends[1]]}),
    )
    .unwrap();
    assert_eq!(state(&mut s), json!(["partly", "u", 2]));
    let v = call(&mut s, "sheet.get", json!({"part": "p"})).unwrap();
    assert_eq!(v["bends"].as_array().unwrap().len(), 2, "{v}");
    // Every one: flat.
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "u", "only": []}),
    )
    .unwrap();
    assert_eq!(state(&mut s), json!(["flat", "u", 4]));
    // One folded back: partly; every one: folded.
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "refold", "id": "rf",
        "unfold": "u", "only": [bends[0]]}),
    )
    .unwrap();
    assert_eq!(state(&mut s)[0], "partly");
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "rf", "only": []}),
    )
    .unwrap();
    assert_eq!(state(&mut s), json!(["folded", null, 0]));
}
