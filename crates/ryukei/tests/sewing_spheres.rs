//! Race body 1573 (boolean seed 885: a cylinder, and a lens of two
//! spheres whose rim circle passes 4 cm from the smaller sphere's poles)
//! healed from a shape whose faces each have their own edges, as a
//! surface model writes them (#373). On main this is the sewing race's
//! cut through 12.C's `heal::sew`; release/0.8 has no `sew`, so the same
//! faces are given to `heal::heal`, which sews them in its first step.
//! Since #488 the file holds the race bodies of #348 main's `sewing.rs`
//! sews back.

use ryukei::geom::{Rigid, Vector3};
use ryukei::heal::{HealOptions, Loop, Shape, Vertex, heal, seamed};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use std::collections::BTreeMap;

/// Healing's cap: the race moves faces by up to a tenth of it.
const TOLERANCE: f64 = 1e-4;

fn body() -> Body {
    ryukei_testkit::race::boolean_body(885).expect("seed 885's boolean")
}

/// Heals race body `index` from its faces moved as the race moves them,
/// and requires a body holding every face.
fn heals_back(index: u64, b: &Body) {
    let s = seamed(b, 1e-7).expect("the body as a file describes it");
    let (h, _, _) = heal(
        OpKey(24),
        &unshared(&s, index, 0.1 * TOLERANCE),
        &HealOptions::default(),
    )
    .unwrap_or_else(|e| panic!("race body {index}: {e}"));
    assert_eq!(h.faces().count(), b.faces().count(), "race body {index}");
}

/// Race bodies whose loops cross a seam or a pole of a periodic face at a
/// vertex (#348): moved, the vertex lies microns off the seam or the pole
/// (within its tolerance, past resolution), so the mesher refused the face
/// at every chord and shell facing had no volume sign. Its second try now
/// moves the seam off the face's vertices, and puts a vertex within its
/// tolerance of a pole on the pole.
#[test]
fn loops_crossed_at_a_moved_vertex_heal_back() {
    for (index, seed) in [(300, 165), (574, 318), (990, 552)] {
        let b = ryukei_testkit::race::boolean_body(seed).expect("the seed builds");
        heals_back(index, &b);
    }
}

/// `s` with each face's edges and vertices its own, each face moved
/// (surface, curves and vertices) by up to `nudge` along a direction of
/// its own, as the sewing race cuts race body `seed`; pcurves kept.
fn unshared(s: &Shape, seed: u64, nudge: f64) -> Shape {
    let mut g = Lcg::new(seed);
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
        t.surfaces
            .push(s.surfaces[s.faces[k].surface].transformed(&by));
        t.faces[k].surface = t.surfaces.len() - 1;
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
        let (h, _, _) = heal(
            OpKey(24),
            &unshared(&s, 1573, nudge),
            &HealOptions::default(),
        )
        .unwrap_or_else(|e| panic!("moved by {nudge}: {e}"));
        assert_eq!(h.faces().count(), b.faces().count(), "moved by {nudge}");
        assert_eq!(h.regions().count(), b.regions().count(), "moved by {nudge}");
    }
}

/// Race bodies 1010 (boolean seed 564: a sphere face, a cone and two
/// planes) and 1503 (boolean seed 844: a sphere face, a cylinder and a
/// plane) from their own faces, moved as the race moves them (#370):
/// neither call returned. 1503's sphere face, once sewn, has two
/// tolerant edges ending at a vertex on the pole and spiralling round it
/// within their tolerances; the mesher stopped asking for samples where
/// crossings can't be parted (#370), and on its second try puts those
/// edges' samples inside their tubes on the line from the pole (#497), so
/// both heal back.
#[test]
fn race_body_1010_heals_back() {
    let b = ryukei_testkit::race::boolean_body(564).expect("seed 564's boolean");
    heals_back(1010, &b);
}

#[test]
fn race_body_1503_heals_back() {
    let b = ryukei_testkit::race::boolean_body(844).expect("seed 844's boolean");
    heals_back(1503, &b);
}
