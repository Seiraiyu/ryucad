//! Blends on sheets against blends on solids (design 295, plan 12.F):
//! seeded boxes, cylinders and frustums, each with one planar face `F`
//! taken away, and random edges off `F` filleted or chamfered. Blending
//! the sheet and blending the solid then taking `F` away must agree:
//! both refuse, or both build, with the same face names and the same
//! area, and the sheet passes the checker and meshes. At `F`'s vertices
//! the solid's blend ends on `F` and the sheet's at its open boundary,
//! in `F`'s plane: the same curve. Slow in a debug build, so release
//! only (`cargo test -p ryukei --release --test sheet_blend_props`).

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Rigid, Surface, Vector3};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::{Name, OpKey};
use ryukei::primitive;
use ryukei::sheet::remove;
use ryukei::topo::{Body, BodyKind};
use ryukei_testkit::Lcg;
use std::collections::BTreeSet;
use std::f64::consts::PI;

const CASES: u64 = 64;

fn place(g: &mut Lcg) -> Rigid {
    let axis = loop {
        let v = Vector3::new(
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.0, 1.0),
        );
        if v.length() > 0.2 {
            break v.normalize().unwrap();
        }
    };
    let t = Vector3::new(
        g.next_f64(-3.0, 3.0),
        g.next_f64(-3.0, 3.0),
        g.next_f64(-3.0, 3.0),
    );
    Rigid::translation(t).then_after(&Rigid::rotation(axis, g.next_f64(0.0, PI)))
}

/// A random primitive and its smallest dimension.
fn body(g: &mut Lcg) -> (Body, f64) {
    let at = place(g);
    match g.next_u64() % 3 {
        0 => {
            let d = [0, 1, 2].map(|_| g.next_f64(0.5, 2.0));
            let b = primitive::block(OpKey(1), d[0], d[1], d[2], &at).unwrap().0;
            (b, d[0].min(d[1]).min(d[2]))
        }
        1 => {
            let (r, h) = (g.next_f64(0.4, 1.2), g.next_f64(0.4, 2.0));
            (
                primitive::cylinder(OpKey(1), r, h, &at).unwrap().0,
                r.min(h),
            )
        }
        _ => {
            let (r0, r1, h) = (
                g.next_f64(0.6, 1.2),
                g.next_f64(0.3, 0.5),
                g.next_f64(0.5, 1.5),
            );
            let b = primitive::frustum(OpKey(1), r0, r1, h, &at).unwrap().0;
            (b, r1.min(h))
        }
    }
}

fn area(b: &Body) -> f64 {
    match properties(b, Entity::Body).unwrap() {
        Properties::Sheet { area } => area.value(),
        p => panic!("not a sheet: {p:?}"),
    }
}

fn face_names(b: &Body) -> BTreeSet<Name> {
    b.faces().map(|f| b.face(f).unwrap().name).collect()
}

/// A seeded case: the solid, the planar face taken away, and the blends
/// of edges off it.
fn case(seed: u64) -> (Body, Name, Vec<EdgeBlend>) {
    let mut g = Lcg::new(0x5b1e_0000 + seed);
    let (b, size) = body(&mut g);
    let planar: Vec<_> = b
        .faces()
        .filter(|&f| {
            matches!(
                b.surface(b.face(f).unwrap().surface),
                Some(Surface::Plane(_))
            )
        })
        .collect();
    let f = planar[(g.next_u64() % planar.len() as u64) as usize];
    let off: Vec<_> = b
        .edges()
        .filter(|&e| {
            !b.edge_fins(e)
                .any(|x| b.loop_(b.fin(x).unwrap().loop_).unwrap().face == f)
        })
        .collect();
    let mut chosen = Vec::new();
    let n = 1 + (g.next_u64() % 3) as usize;
    for _ in 0..n.min(off.len()) {
        let e = off[(g.next_u64() % off.len() as u64) as usize];
        if !chosen.contains(&e) {
            chosen.push(e);
        }
    }
    let fillet = g.next_u64().is_multiple_of(2);
    let s = g.next_f64(0.05, 0.3) * size;
    let blends = chosen
        .iter()
        .map(|&e| EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind: if fillet {
                BlendKind::Fillet { radius: s }
            } else {
                BlendKind::Chamfer(Chamfer::Equal { distance: s })
            },
        })
        .collect();
    let name = b.face(f).unwrap().name;
    (b, name, blends)
}

#[cfg_attr(debug_assertions, ignore = "slow in a debug build")]
#[test]
fn sheets_blend_as_their_solids_do() {
    let key = OpKey(0x5b1e);
    let cut = OpKey(0x5b1f);
    let (mut judged, mut refused) = (0, 0);
    for seed in 0..CASES {
        let (solid, f, blends) = case(seed);
        let sheet = remove(cut, &solid, &[f]).unwrap().0;
        let on_sheet = blend(key, &sheet, &blends);
        let on_solid = blend(key, &solid, &blends);
        match (on_solid, on_sheet) {
            (Err(_), Err(_)) => refused += 1,
            (Ok((a, _)), Ok((b, _))) => {
                let a = remove(cut, &a, &[f]).unwrap().0;
                assert_eq!(b.kind(), BodyKind::Sheet, "seed {seed}");
                assert_eq!(check(&b), [], "seed {seed}");
                assert_eq!(check_intersections(&b), [], "seed {seed}");
                let m = tessellate(&b, &MeshParams::new(1e-3, 0.3)).unwrap();
                assert!(m.faults.is_empty(), "seed {seed}: {:?}", m.faults);
                assert_eq!(face_names(&a), face_names(&b), "seed {seed}");
                let (x, y) = (area(&a), area(&b));
                assert!((x - y).abs() <= 1e-7 * x.max(1.0), "seed {seed}: {x} {y}");
                judged += 1;
            }
            (x, y) => panic!(
                "seed {seed}: the solid {:?}, the sheet {:?}",
                x.map(|_| ()),
                y.map(|_| ())
            ),
        }
    }
    println!("{judged} judged, {refused} refused by both, of {CASES}");
    assert!(judged * 4 >= CASES as usize * 3, "{judged} judged");
}
