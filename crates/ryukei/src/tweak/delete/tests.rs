//! Unit tests of deleting faces.

use super::*;
use crate::boolean::{BooleanOp, Tool, boolean};
use crate::geom::{Rigid, Surface, Vector3};
use crate::name::{OpKey, ToolKey};
use crate::primitive;
use crate::topo::Body;
use crate::topo::EntityId;
use crate::work::Meter;

const K: OpKey = OpKey(0x13b0);
const D: OpKey = OpKey(0x13b1);

/// A plate 1 × 1 × 0.2 with a hole of radius 0.1 through its middle.
pub(crate) fn drilled() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(0x13b2), 0.1, 0.6, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0x13b3),
        BooleanOp::Subtract,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &pin,
            place: Rigid::translation(Vector3::new(0.5, 0.5, -0.2)),
        }],
    )
    .unwrap()
    .0
}

/// The plate with a boss of radius 0.1, 0.15 high, on its top.
pub(crate) fn bossed() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (boss, _) = primitive::cylinder(OpKey(0x13b2), 0.1, 0.15, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0x13b3),
        BooleanOp::Unite,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &boss,
            place: Rigid::translation(Vector3::new(0.5, 0.5, 0.2)),
        }],
    )
    .unwrap()
    .0
}

pub(crate) fn cylinders(b: &Body) -> Vec<Name> {
    b.faces()
        .filter(|&f| matches!(b.surfaces[b.faces[f].surface], Surface::Cylinder(_)))
        .map(|f| b.faces[f].name)
        .collect()
}

fn volume(b: &Body) -> f64 {
    assert_eq!(crate::check::check(b), []);
    assert_eq!(crate::check::check_intersections(b), []);
    let m = crate::mesh::tessellate(b, &crate::mesh::MeshParams::new(1e-4, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

fn spec(faces: Vec<Name>, heal: Heal) -> DeleteSpec {
    DeleteSpec { faces, heal }
}

#[test]
fn a_holes_rims_are_holes_of_the_plate_and_its_foot_is_not_the_bosss_tops() {
    let b = drilled();
    let wall: BTreeSet<FaceId> = b
        .faces()
        .filter(|&f| cylinders(&b).contains(&b.faces[f].name))
        .collect();
    let s = sets::sets(&b, &wall);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].hosts.len(), 2);
    assert!(s[0].closable(&b));
    let b = bossed();
    let side: BTreeSet<FaceId> = b
        .faces()
        .filter(|&f| cylinders(&b).contains(&b.faces[f].name))
        .collect();
    let s = sets::sets(&b, &side);
    // The foot is a hole of the plate's top; the rim is the boss top's
    // only loop.
    assert_eq!(s[0].hosts.len(), 2);
    assert!(!s[0].closable(&b));
}

#[test]
fn a_cap_lays_the_plate_out_without_its_hole() {
    let b = drilled();
    let wall: BTreeSet<FaceId> = b
        .faces()
        .filter(|&f| cylinders(&b).contains(&b.faces[f].name))
        .collect();
    let s = sets::sets(&b, &wall);
    // unmetered: a unit test.
    let m = Meter::free();
    let (o, j) = cap::cap(D, &b, &[&s[0]], &m).unwrap();
    assert_eq!(
        (o.faces().count(), o.edges().count(), o.vertices().count()),
        (6, 12, 8)
    );
    assert!((volume(&o) - 0.2).abs() < 1e-9);
    let modified = j
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Modified(_)))
        .count();
    // The top and the bottom; the wall, both rims and their vertices go.
    assert_eq!(modified, 2);
    assert_eq!(j.events().len(), 2 + 1 + 2 + 2);
}

/// The plate with two holes of radius 0.1 through it, at x = 0.3 and 0.7.
fn drilled_twice() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(0x13b2), 0.1, 0.6, &Rigid::IDENTITY).unwrap();
    let tool = |key, x| Tool {
        key: ToolKey(key),
        body: &pin,
        place: Rigid::translation(Vector3::new(x, 0.5, -0.2)),
    };
    boolean(
        OpKey(0x13b3),
        BooleanOp::Subtract,
        &plate,
        &[tool(1, 0.3), tool(2, 0.7)],
    )
    .unwrap()
    .0
}

/// Every named entity's geometry as the dump writes it, by name, with no
/// array index in it: a face's sense and surface; each fin's pcurve and
/// map, by its face, edge and sense; an edge's curve, interval, tolerance
/// and end vertices; a vertex's point and tolerance.
fn geometry(b: &Body) -> std::collections::BTreeMap<String, String> {
    use crate::topo::dump::{bits, curve, curve2, point, surface};
    let mut out = std::collections::BTreeMap::new();
    for f in b.faces() {
        let face = &b.faces[f];
        let text = format!(
            "face {:?} {}",
            face.sense,
            surface(&b.surfaces[face.surface])
        );
        assert!(out.insert(face.name.to_string(), text).is_none());
        for l in b.face_loops(f) {
            for i in b.loop_fins(l) {
                let fin = &b.fins[i];
                let text = fin.pcurve.map_or_else(String::new, |p| {
                    format!(
                        "{} map={},{}",
                        curve2(&b.curves2[p.curve]),
                        bits(p.map[0]),
                        bits(p.map[1])
                    )
                });
                let key = format!("{}/{}/{:?}", face.name, b.edges[fin.edge].name, fin.sense);
                assert!(out.insert(key, text).is_none());
            }
        }
    }
    for e in b.edges() {
        let edge = &b.edges[e];
        let text = format!(
            "edge {} interval={},{} tol={} start={} end={}",
            curve(&b.curves[edge.curve]),
            bits(edge.interval[0]),
            bits(edge.interval[1]),
            bits(edge.tolerance.get()),
            b.vertices[edge.start].name,
            b.vertices[edge.end].name
        );
        assert!(out.insert(edge.name.to_string(), text).is_none());
    }
    for v in b.vertices() {
        let vertex = &b.vertices[v];
        let text = format!(
            "vertex {} tol={}",
            point(vertex.point),
            bits(vertex.tolerance.get())
        );
        assert!(out.insert(vertex.name.to_string(), text).is_none());
    }
    out
}

#[test]
fn a_through_hole_is_capped_bit_for_bit() {
    let b = drilled_twice();
    let walls = cylinders(&b);
    assert_eq!(walls.len(), 2);
    let (o, j) = delete_faces(D, &b, &spec(vec![walls[0]], Heal::Auto)).unwrap();
    assert_eq!(o.faces().count(), 7);
    // The other hole's wall is curved: its mesh at a chord of 1e-4 holds
    // the volume to about the chord times the wall's area (0.126 m²).
    assert!((volume(&o) - (0.2 - 0.2 * core::f64::consts::PI * 0.01)).abs() < 2e-5);
    // Every face, edge and vertex the cap keeps is as it was, bit for bit:
    // its geometry, tolerances, pcurves (the other hole's rims' too) and
    // name. The cap makes nothing new.
    let (before, after) = (geometry(&b), geometry(&o));
    for (name, text) in &after {
        assert_eq!(Some(text), before.get(name), "{name}");
    }
    assert!(after.contains_key(&walls[1].to_string()));
    assert!(!after.contains_key(&walls[0].to_string()));
    // The other hole's rims keep their fins on the top and bottom.
    let rims = after.keys().filter(|k| k.matches('/').count() == 2).count();
    assert_eq!(
        rims,
        before
            .keys()
            .filter(|k| k.matches('/').count() == 2)
            .count()
            - 4
    );
    assert!(j.events().contains(&Event::Deleted(walls[0])));
    assert_eq!(
        j.events()
            .iter()
            .filter(|e| matches!(e, Event::Modified(_)))
            .count(),
        2
    );
    let b = drilled();
    let (o, _) = delete_faces(D, &b, &spec(cylinders(&b), Heal::Auto)).unwrap();
    assert_eq!(o.faces().count(), 6);
    let (before, after) = (geometry(&b), geometry(&o));
    for (name, text) in &after {
        assert_eq!(Some(text), before.get(name), "{name}");
    }
    assert_eq!(
        delete_faces(D, &b, &spec(cylinders(&b), Heal::Grow)).unwrap_err(),
        TweakError::GrowWouldCap {
            face: cylinders(&b)[0]
        }
        .into()
    );
}

#[test]
fn a_bosss_side_and_top_are_one_feature_capped_by_the_plate() {
    let b = bossed();
    let f = feature_faces(&b, cylinders(&b)[0]).unwrap();
    assert_eq!(f.len(), 2);
    let (o, _) = delete_faces(D, &b, &spec(f, Heal::Cap)).unwrap();
    assert!((volume(&o) - 0.2).abs() < 1e-9);
    // The side alone: no cap; growth takes the top too.
    assert_eq!(
        delete_faces(D, &b, &spec(cylinders(&b), Heal::Cap)).unwrap_err(),
        TweakError::CapNotOnFace {
            face: cylinders(&b)[0]
        }
        .into()
    );
    let (o, _) = delete_faces(D, &b, &spec(cylinders(&b), Heal::Auto)).unwrap();
    assert!((volume(&o) - 0.2).abs() < 1e-9);
}

/// The test block with its edge between top and front rounded 0.1,
/// and the fillet's face.
fn filleted_block() -> (Body, FaceId) {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let top = Name::derive(
        K,
        crate::name::Role::Face(crate::name::FaceRole::PlusZ),
        &[],
    );
    let front = Name::derive(
        K,
        crate::name::Role::Face(crate::name::FaceRole::MinusY),
        &[],
    );
    let e = b
        .edges()
        .find(|&e| {
            let n: Vec<Name> = sets::faces_of(&b, e)
                .iter()
                .map(|&f| b.faces[f].name)
                .collect();
            n.contains(&top) && n.contains(&front)
        })
        .unwrap();
    let (r, _) = crate::blend::blend(
        OpKey(0x13b4),
        &b,
        &[crate::blend::EdgeBlend {
            edge: b.edges[e].name,
            kind: crate::blend::BlendKind::Fillet { radius: 0.1 },
        }],
    )
    .unwrap();
    let fillet = cylinders(&r)[0];
    let EntityId::Face(f) = r.by_name(fillet).unwrap() else {
        panic!()
    };
    (r, f)
}

#[test]
fn a_fillet_grows_back_to_its_sharp_edge() {
    let top = Name::derive(
        K,
        crate::name::Role::Face(crate::name::FaceRole::PlusZ),
        &[],
    );
    let front = Name::derive(
        K,
        crate::name::Role::Face(crate::name::FaceRole::MinusY),
        &[],
    );
    let (r, f) = filleted_block();
    let fillet = r.faces[f].name;
    // unmetered: a unit test.
    let m = Meter::free();
    let (o, j) = grow::grow(D, &r, &BTreeSet::from([f]), &m).unwrap();
    assert!((volume(&o) - 0.24).abs() < 1e-9);
    let sharp: Vec<Name> = o
        .edges()
        .filter(|&e| {
            let n: Vec<Name> = sets::faces_of(&o, e)
                .iter()
                .map(|&f| o.faces[f].name)
                .collect();
            n.contains(&top) && n.contains(&front)
        })
        .map(|e| o.edges[e].name)
        .collect();
    assert_eq!(sharp.len(), 1);
    assert!(
        j.events().contains(&Event::Replaced {
            old: fillet,
            new: sharp.clone()
        }),
        "{:?}",
        j.events()
    );
    // Both rails go to the sharp edge too.
    let rails = j
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Replaced { old, new } if *old != fillet && *new == sharp))
        .count();
    assert_eq!(rails, 2);
}

#[test]
fn deleting_a_whole_body_or_a_slabs_side_heals_nothing() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let all: Vec<Name> = b.faces().map(|f| b.faces[f].name).collect();
    assert!(matches!(
        delete_faces(D, &b, &spec(all, Heal::Auto)),
        Err(Error::InvalidTweak(TweakError::GapNotHealed { .. }))
    ));
    let side = Name::derive(
        K,
        crate::name::Role::Face(crate::name::FaceRole::PlusX),
        &[],
    );
    assert_eq!(
        delete_faces(D, &b, &spec(vec![side], Heal::Auto)).unwrap_err(),
        TweakError::GapNotHealed { face: side }.into()
    );
}

/// Design 919, plan 23.B: a set's whole ladder under one gap. A slab's
/// side (nothing within reach closes it: every rung's walk fails) spends
/// `all` units refusing `GapNotHealed`. Given `all`, or a unit less (the
/// gap spent in its last rung), it answers the last finished rung's
/// cause, `GapNotHealed` at the side. Given one unit, no rung finishes:
/// `Uncertain` at the side, its set's least face. The same units and
/// answers on 1, 2 and 8 threads.
#[test]
fn a_set_past_its_gap_refuses_with_its_last_finished_rungs_cause() {
    use crate::name::{FaceRole, Role};
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let name = Name::derive(K, Role::Face(FaceRole::PlusX), &[]);
    let side = b.faces().find(|&f| b.faces[f].name == name).unwrap();
    let deleted = BTreeSet::from([side]);
    let run = |units: u64, threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let w = Work::unlimited();
            let m = Meter::new(&w);
            let r = grow::grow_within(D, &b, &deleted, units, &m).map(|_| ());
            (r, m.spent())
        })
    };
    let (r, all) = run(u64::MAX, 1);
    assert_eq!(
        r.unwrap_err(),
        TweakError::GapNotHealed { face: name }.into()
    );
    for threads in [1, 2, 8] {
        assert_eq!(run(u64::MAX, threads).1, all, "{threads} threads");
        for units in [all, all - 1] {
            let (r, _) = run(units, threads);
            assert_eq!(
                r.unwrap_err(),
                TweakError::GapNotHealed { face: name }.into(),
                "{units} units, {threads} threads"
            );
        }
        let (r, spent) = run(1, threads);
        assert_eq!(r.unwrap_err(), TweakError::Uncertain { at: name }.into());
        assert!(spent < all, "{spent} of {all}");
    }
}

/// Plan 23.B: a set that builds spends what it did. A fillet grown back
/// to its sharp edge under `GAP`: the same body, journal and units as
/// with no gap.
#[test]
fn a_set_that_builds_spends_what_it_did_under_its_gap() {
    let (b, fillet) = filleted_block();
    let deleted = BTreeSet::from([fillet]);
    let w = Work::unlimited();
    let m = Meter::new(&w);
    let (o, j) = grow::grow_within(D, &b, &deleted, u64::MAX, &m).unwrap();
    let free = m.spent();
    let w = Work::unlimited();
    let m = Meter::new(&w);
    let (o2, j2) = grow::grow(D, &b, &deleted, &m).unwrap();
    assert_eq!(o.dump(), o2.dump());
    assert_eq!(j.events(), j2.events());
    assert_eq!(m.spent(), free);
}
