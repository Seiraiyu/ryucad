//! Race body 1573 (boolean seed 885: a cylinder, and a lens of two
//! spheres whose rim circle passes 4 cm from the smaller sphere's poles)
//! healed from a shape whose faces each have their own edges, as a
//! surface model writes them (#373). On main this is the sewing race's
//! cut through 12.C's `heal::sew`; release/0.8 has no `sew`, so the same
//! faces are given to `heal::heal`, which sews them in its first step.

use ryukei::geom::{Rigid, Vector3};
use ryukei::heal::{HealOptions, Loop, Shape, Vertex, heal, seamed};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use std::collections::BTreeMap;

fn body() -> Body {
    let mut g = Lcg::new(885);
    let bytes: Vec<u8> = (0..96).map(|_| (g.next_u64() >> 56) as u8).collect();
    let case = ryukei_testkit::boolean_bytes::boolean_case(&bytes).expect("seed 885's case");
    ryukei::boolean::boolean(OpKey(9), case.op, &case.target, &case.tools())
        .expect("seed 885's boolean")
        .0
}

/// `s` with each face's edges and vertices its own, each face's copies
/// moved by up to `nudge` along a direction of its own; pcurves kept.
fn unshared(s: &Shape, nudge: f64) -> Shape {
    let mut g = Lcg::new(1573);
    let mut unit = || (g.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    let mut t = s.clone();
    t.edges.clear();
    t.vertices.clear();
    for k in 0..s.faces.len() {
        let dir = Vector3::new(unit() - 0.5, unit() - 0.5, unit() - 0.5);
        let by = Rigid::translation(
            dir.normalize()
                .map_or(Vector3::new(0.0, 0.0, 0.0), |d| d.get() * (nudge * unit())),
        );
        let mut vertices: BTreeMap<usize, usize> = BTreeMap::new();
        let mut copy = |t: &mut Shape, v: usize| {
            *vertices.entry(v).or_insert_with(|| {
                t.vertices.push(Vertex {
                    point: by.apply_point(s.vertices[v].point),
                    ..s.vertices[v]
                });
                t.vertices.len() - 1
            })
        };
        // A seam (an edge the face uses twice) stays one edge of the face.
        let mut edges: BTreeMap<usize, usize> = BTreeMap::new();
        let mut loops = s.faces[k].loops.clone();
        for l in &mut loops {
            match l {
                Loop::Vertex(v) => *v = copy(&mut t, *v),
                Loop::Edges(uses) => {
                    for u in uses {
                        if let Some(&n) = edges.get(&u.edge) {
                            u.edge = n;
                            continue;
                        }
                        let e = s.edges[u.edge];
                        t.curves.push(s.curves[e.curve].transformed(&by));
                        let (start, end) = (copy(&mut t, e.start), copy(&mut t, e.end));
                        t.edges.push(ryukei::heal::Edge {
                            curve: t.curves.len() - 1,
                            start,
                            end,
                            ..e
                        });
                        edges.insert(u.edge, t.edges.len() - 1);
                        u.edge = t.edges.len() - 1;
                    }
                }
            }
        }
        t.faces[k].loops = loops;
    }
    t
}

/// Unmoved, the rim's copies are one curve, and the use sewing renames
/// keeps its pcurve; moved, healing tightens the rim onto the spheres'
/// circle, the closed edge taking the branch's own range, where its
/// clamped pcurves fit. Either way no pcurve is fitted afresh round the
/// poles, which fails (`NoPcurve` before the fix).
#[test]
fn race_body_1573_heals_from_its_own_faces_moved_or_not() {
    let b = body();
    let s = seamed(&b, 1e-7).expect("the body as a file describes it");
    for nudge in [0.0, 1e-7, 1e-5] {
        let (h, _, _) = heal(OpKey(24), &unshared(&s, nudge), &HealOptions::default())
            .unwrap_or_else(|e| panic!("moved by {nudge}: {e}"));
        assert_eq!(h.faces().count(), b.faces().count(), "moved by {nudge}");
        assert_eq!(h.regions().count(), b.regions().count(), "moved by {nudge}");
    }
}
