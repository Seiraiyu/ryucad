//! Sewing sheets into solids and larger sheets (design 295).
//!
//! [`sew`] takes sheets (faces cut from solids by
//! [`crate::sheet::extract`], surfaces built or read) and joins them along
//! their laminar edges by healing's own sewing rules (see `sew`): two
//! free edges are one where both ends and 17 samples along each lie
//! within the tolerance of the other, and that gap is under half the
//! shorter one's chord; pairs are taken nearest first, ties by name; an
//! edge is split where another sheet's vertex lies on it (a T-junction);
//! three sheets meeting at one edge are [`HealError::NonManifold`].
//!
//! What sewing leaves joined is then healed piece by piece, in parallel,
//! from healing's third step on: a piece that closes up is a solid,
//! facing out by its volume (see `shells`), unless [`SewOptions::solid`]
//! is false, when it stays a closed sheet; any other piece is a sheet,
//! its free edges laminar, its faces turned to agree from its least-named
//! face. One body comes back per piece, in the order of their least face
//! names, so the result doesn't depend on the order the sheets came in.
//!
//! Faces keep their names. A sewn edge is named as merged from the two it
//! joins, a joined vertex as merged from all of its copies
//! ([`Event::Merged`]), as healing names them; everything else keeps its
//! name. Copies that share a name and end up in one piece unjoined (two
//! copies of one sheet, say) each take a part of it, by their order in
//! the input ([`Event::Split`]). Tolerances grow to cover the gaps, up to
//! the tolerance.

use super::names::Names;
use super::sew::{self, Sewn};
use super::{Edge, Face, HealOptions, Loop, Open, Shape, Shell, Solid, Use, Vertex, pieces};
use crate::name::{Event, Journal, Name, OpKey, Role};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, BodyKind, FaceId};
use crate::{Error, HealError};
use std::collections::BTreeMap;

/// How to sew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SewOptions {
    /// The largest gap sewing closes, and the largest tolerance it may
    /// give an entity, in meters: from [`LINEAR_RESOLUTION`] to healing's
    /// cap, `1e-4`.
    pub tolerance: f64,
    /// Whether a piece that closes up is made a solid; else it stays a
    /// closed sheet.
    pub solid: bool,
}

impl Default for SewOptions {
    fn default() -> Self {
        Self {
            tolerance: 1e-5,
            solid: true,
        }
    }
}

/// Sews `sheets` along their laminar edges (see the module's notes):
/// one body per connected piece, and the journal of what happened to
/// the sheets' names. A body that isn't a sheet is
/// [`Error::NotASheet`]; a tolerance outside its range is
/// [`HealError::Malformed`]; a piece that can't be healed is refused as
/// healing refuses it, naming entities by the sheets' names.
pub fn sew(
    key: OpKey,
    sheets: &[Body],
    options: &SewOptions,
) -> Result<(Vec<Body>, Journal), Error> {
    sew_with(key, sheets, options, &crate::work::Work::unlimited())
}

/// [`sew`] under `work`: `Cancelled` or `BudgetExceeded` once it is
/// spent, building nothing.
pub fn sew_with(
    key: OpKey,
    sheets: &[Body],
    options: &SewOptions,
    work: &crate::work::Work,
) -> Result<(Vec<Body>, Journal), Error> {
    crate::work::run(work, |m| sew_in(key, sheets, options, m))
}

fn sew_in(
    key: OpKey,
    sheets: &[Body],
    options: &SewOptions,
    m: &crate::work::Meter,
) -> Result<(Vec<Body>, Journal), Error> {
    let cap = HealOptions::default().tolerance_cap;
    if !(options.tolerance.is_finite() && (LINEAR_RESOLUTION..=cap).contains(&options.tolerance)) {
        return Err(HealError::Malformed {
            what: "a sewing tolerance outside the resolution and the cap",
        }
        .into());
    }
    let mut shape = Shape::default();
    let mut names = Names {
        vertices: Vec::new(),
        edges: Vec::new(),
        faces: Vec::new(),
    };
    for b in sheets {
        match b.kind() {
            BodyKind::Sheet => gather(&mut shape, &mut names, b),
            BodyKind::Empty => {}
            BodyKind::Solid | BodyKind::Mixed => return Err(Error::NotASheet { op: "sew" }),
        }
    }
    let sewn = sew::sew(key, &mut shape, &mut names, options.tolerance, m)?;
    let mut journal = sewn.journal;
    // Each piece as a shape of its own, in the order of its least face.
    let mut cut: Vec<(Name, Shape, Names)> = pieces::pieces(&shape)
        .into_iter()
        .map(|faces| {
            let (mut s, mut n) = piece(&shape, &names, &faces);
            journal.append(distinct(key, &mut n));
            let closed = closes(&s);
            s.shells.push(Shell {
                faces: (0..s.faces.len()).collect(),
                closed,
            });
            if closed && options.solid {
                s.solids.push(Solid {
                    outer: 0,
                    voids: Vec::new(),
                });
            } else {
                s.sheets.push(0);
            }
            let least = n.faces.iter().copied().min().expect("a piece has faces");
            (least, s, n)
        })
        .collect();
    cut.sort_by_key(|x| x.0);
    let heal = HealOptions {
        tolerance_cap: options.tolerance,
        tighten: true,
        // Sewing refuses what crosses, as it did before self-union: a
        // caller who wants it repaired asks `repair::self_union`.
        repair: false,
        open: Open::Sheet,
    };
    let healed: Vec<Result<(Body, Journal), Error>> = m.par_map(&cut, |m, (_, s, n)| {
        let sheet = s.solids.is_empty();
        super::after_sewing(key, s.clone(), n.clone(), &Sewn::default(), &heal, sheet, m)
            .map(|x| (x.0, x.1))
    });
    let mut bodies = Vec::with_capacity(healed.len());
    for h in healed {
        let (body, steps) = h?;
        // What the sheets had keeps its name: nothing is created.
        for e in steps.events() {
            if !matches!(e, Event::Created(_)) {
                journal.push(e.clone());
            }
        }
        bodies.push(body);
    }
    Ok((bodies, journal))
}

/// Adds a sheet's faces, loops, edges and vertices to `s`, with their
/// tolerances and pcurves, and their names to `names`.
fn gather(s: &mut Shape, names: &mut Names, b: &Body) {
    let mut surfaces = BTreeMap::new();
    let mut curves = BTreeMap::new();
    let mut vertices = BTreeMap::new();
    let mut edges = BTreeMap::new();
    for v in b.vertices() {
        vertices.insert(v, s.vertices.len());
        s.vertices.push(Vertex {
            point: b.vertices[v].point,
            tolerance: b.vertices[v].tolerance.get(),
            seam: false,
        });
        names.vertices.push(b.vertices[v].name);
    }
    for e in b.edges() {
        let edge = &b.edges[e];
        let c = *curves.entry(edge.curve).or_insert_with(|| {
            s.curves.push(b.curves[edge.curve].clone());
            s.curves.len() - 1
        });
        edges.insert(e, s.edges.len());
        s.edges.push(Edge {
            curve: c,
            interval: edge.interval,
            start: vertices[&edge.start],
            end: vertices[&edge.end],
            tolerance: edge.tolerance.get(),
        });
        names.edges.push(edge.name);
    }
    let mut faces: BTreeMap<FaceId, usize> = BTreeMap::new();
    for f in b.faces() {
        let face = &b.faces[f];
        let surface = *surfaces.entry(face.surface).or_insert_with(|| {
            s.surfaces.push(b.surfaces[face.surface].clone());
            s.surfaces.len() - 1
        });
        let mut loops = Vec::new();
        for l in b.face_loops(f) {
            loops.push(match b.loops[l].vertex {
                Some(v) => Loop::Vertex(vertices[&v]),
                None => Loop::Edges(
                    b.loop_fins(l)
                        .map(|fin| Use {
                            edge: edges[&b.fins[fin].edge],
                            sense: b.fins[fin].sense,
                            pcurve: b.fins[fin].pcurve.map(|p| {
                                s.curves2.push(b.curves2[p.curve].clone());
                                s.curves2.len() - 1
                            }),
                        })
                        .collect(),
                ),
            });
        }
        faces.insert(f, s.faces.len());
        s.faces.push(Face {
            surface,
            sense: face.sense,
            loops,
            tolerance: 0.0,
        });
        names.faces.push(face.name);
    }
}

/// The faces `list` of `s` as a shape of their own, with what they use
/// and its names, in the order of `s`. No shells yet.
fn piece(s: &Shape, names: &Names, list: &[usize]) -> (Shape, Names) {
    let mut out = Shape::default();
    let mut n = Names {
        vertices: Vec::new(),
        edges: Vec::new(),
        faces: Vec::new(),
    };
    let mut surfaces: BTreeMap<usize, usize> = BTreeMap::new();
    let mut curves: BTreeMap<usize, usize> = BTreeMap::new();
    let mut vertices: BTreeMap<usize, usize> = BTreeMap::new();
    let mut edges: BTreeMap<usize, usize> = BTreeMap::new();
    let mut vertex = |out: &mut Shape, n: &mut Names, v: usize| -> usize {
        *vertices.entry(v).or_insert_with(|| {
            out.vertices.push(s.vertices[v]);
            n.vertices.push(names.vertices[v]);
            out.vertices.len() - 1
        })
    };
    for &k in list {
        let face = &s.faces[k];
        let surface = *surfaces.entry(face.surface).or_insert_with(|| {
            out.surfaces.push(s.surfaces[face.surface].clone());
            out.surfaces.len() - 1
        });
        let mut loops = Vec::with_capacity(face.loops.len());
        for l in &face.loops {
            loops.push(match l {
                Loop::Vertex(v) => Loop::Vertex(vertex(&mut out, &mut n, *v)),
                Loop::Edges(uses) => {
                    let mut list = Vec::with_capacity(uses.len());
                    for u in uses {
                        let g = match edges.get(&u.edge) {
                            Some(&g) => g,
                            None => {
                                let e = s.edges[u.edge];
                                let c = *curves.entry(e.curve).or_insert_with(|| {
                                    out.curves.push(s.curves[e.curve].clone());
                                    out.curves.len() - 1
                                });
                                let (start, end) = (
                                    vertex(&mut out, &mut n, e.start),
                                    vertex(&mut out, &mut n, e.end),
                                );
                                out.edges.push(Edge {
                                    curve: c,
                                    start,
                                    end,
                                    ..e
                                });
                                n.edges.push(names.edges[u.edge]);
                                edges.insert(u.edge, out.edges.len() - 1);
                                out.edges.len() - 1
                            }
                        };
                        let pcurve = u.pcurve.map(|p| {
                            out.curves2.push(s.curves2[p].clone());
                            out.curves2.len() - 1
                        });
                        list.push(Use {
                            edge: g,
                            sense: u.sense,
                            pcurve,
                        });
                    }
                    Loop::Edges(list)
                }
            });
        }
        out.faces.push(Face {
            surface,
            sense: face.sense,
            loops,
            tolerance: face.tolerance,
        });
        n.faces.push(names.faces[k]);
    }
    (out, n)
}

/// Whether every edge of `s` is used at least twice: the piece closes.
fn closes(s: &Shape) -> bool {
    let mut count = vec![0_usize; s.edges.len()];
    for f in &s.faces {
        for l in &f.loops {
            if let Loop::Edges(list) = l {
                for u in list {
                    count[u.edge] += 1;
                }
            }
        }
    }
    count.iter().all(|&c| c >= 2)
}

/// Gives the entities of a piece that share a name with another of
/// their kind a part of it each, by their order; returns the journal.
fn distinct(key: OpKey, n: &mut Names) -> Journal {
    let mut journal = Journal::new();
    for list in [&mut n.vertices, &mut n.edges, &mut n.faces] {
        let mut by: BTreeMap<Name, Vec<usize>> = BTreeMap::new();
        for (i, &name) in list.iter().enumerate() {
            by.entry(name).or_default().push(i);
        }
        for (parent, members) in by {
            if members.len() < 2 {
                continue;
            }
            let mut children = Vec::with_capacity(members.len());
            for (rank, &i) in members.iter().enumerate() {
                let part = u8::try_from(rank).unwrap_or(u8::MAX);
                list[i] = Name::derive(
                    key,
                    Role::Part(part),
                    &[parent, Name::from_u128(rank as u128)],
                );
                children.push(list[i]);
            }
            journal.push(Event::Split { parent, children });
        }
    }
    journal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rigid, Vector3};
    use crate::measure::{Entity, Properties, properties};
    use crate::{check, primitive, sheet};

    fn block() -> Body {
        let place = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
        primitive::block(OpKey(1), 1.0, 2.0, 3.0, &place).unwrap().0
    }

    /// Each face of `b` as a sheet of its own, moved by `nudge` along a
    /// direction of its own.
    fn faces_of(b: &Body, nudge: f64) -> Vec<Body> {
        b.faces()
            .enumerate()
            .map(|(k, f)| {
                let s = sheet::extract(OpKey(2), b, &[b.faces[f].name]).unwrap().0;
                let dir = Vector3::new((k * 7 % 5) as f64 - 2.0, (k * 3 % 5) as f64 - 2.0, 1.0)
                    .normalize()
                    .unwrap();
                s.transformed(&Rigid::translation(dir.get() * nudge))
                    .unwrap()
            })
            .collect()
    }

    fn volume(b: &Body) -> f64 {
        match properties(b, Entity::Body) {
            Ok(Properties::Body { volume, .. }) => volume.value(),
            other => panic!("{other:?}"),
        }
    }

    fn face_names(b: &Body) -> Vec<Name> {
        let mut x: Vec<Name> = b.faces().map(|f| b.faces[f].name).collect();
        x.sort();
        x
    }

    #[test]
    fn a_blocks_faces_sew_back_into_the_block() {
        let b = block();
        let (out, journal) = sew(OpKey(3), &faces_of(&b, 2e-7), &SewOptions::default()).unwrap();
        let [s] = &out[..] else {
            panic!("{}", out.len())
        };
        assert_eq!(check::check(s), []);
        assert_eq!(s.kind(), BodyKind::Solid);
        assert_eq!(s.counts(), b.counts());
        assert_eq!(face_names(s), face_names(&b));
        // Within what the nudges moved the faces: 22 m² by 0.2 µm.
        assert!((volume(s) - 6.0).abs() < 5e-6, "{}", volume(s));
        // Twelve edges sewn, eight vertices joined from three copies each.
        let merged: Vec<usize> = journal
            .events()
            .iter()
            .filter_map(|e| match e {
                Event::Merged { parents, .. } => Some(parents.len()),
                _ => None,
            })
            .collect();
        assert_eq!(merged.iter().filter(|&&n| n == 2).count(), 12);
        assert_eq!(merged.iter().filter(|&&n| n == 3).count(), 8);
        assert!(
            journal
                .events()
                .iter()
                .all(|e| !matches!(e, Event::Created(_)))
        );
    }

    #[test]
    fn edges_shorter_than_twice_the_gap_sew_once_their_ends_are_joined() {
        // A plate 10 µm thick: its four short edges are shorter than
        // twice the 6 µm its faces are moved by, so the chord rule alone
        // leaves them free and the plate comes back open (#374, race
        // body 1218's 9.6 µm edges).
        let place = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
        let b = primitive::block(OpKey(1), 1.0, 2.0, 1e-5, &place)
            .unwrap()
            .0;
        let options = SewOptions {
            tolerance: 1e-4,
            solid: true,
        };
        let (out, _) = sew(OpKey(3), &faces_of(&b, 6e-6), &options).unwrap();
        let [s] = &out[..] else {
            panic!("{}", out.len())
        };
        assert_eq!(s.kind(), BodyKind::Solid);
        assert_eq!(s.counts(), b.counts());
        assert_eq!(face_names(s), face_names(&b));
    }

    #[test]
    fn a_closed_piece_stays_a_sheet_when_asked_and_an_open_one_is_a_sheet() {
        let b = block();
        let faces = faces_of(&b, 0.0);
        let closed = SewOptions {
            solid: false,
            ..SewOptions::default()
        };
        let (out, _) = sew(OpKey(3), &faces, &closed).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(check::check(&out[0]), []);
        assert_eq!(out[0].kind(), BodyKind::Sheet);
        assert!(out[0].edges().all(|e| !out[0].is_laminar(e)));
        // Five faces: an open box, its rim laminar.
        let (out, _) = sew(OpKey(3), &faces[1..], &SewOptions::default()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(check::check(&out[0]), []);
        assert_eq!(out[0].kind(), BodyKind::Sheet);
        assert_eq!(out[0].edges().filter(|&e| out[0].is_laminar(e)).count(), 4);
    }

    #[test]
    fn a_cylinders_faces_sew_back_along_closed_edges() {
        let c = primitive::cylinder(OpKey(1), 0.5, 2.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let (out, _) = sew(OpKey(3), &faces_of(&c, 3e-7), &SewOptions::default()).unwrap();
        let [s] = &out[..] else {
            panic!("{}", out.len())
        };
        assert_eq!(check::check(s), []);
        assert_eq!(s.counts(), c.counts());
        let want = core::f64::consts::PI * 0.25 * 2.0;
        assert!((volume(s) - want).abs() < 1e-6 * want, "{}", volume(s));
    }

    #[test]
    fn pieces_come_back_in_the_order_of_their_names_whatever_the_input() {
        let a = block();
        let far = Rigid::translation(Vector3::new(9.0, 0.0, 0.0));
        let b = primitive::block(OpKey(5), 1.0, 1.0, 1.0, &far).unwrap().0;
        let mut sheets = faces_of(&a, 0.0);
        sheets.extend(faces_of(&b, 0.0));
        let (out, _) = sew(OpKey(3), &sheets, &SewOptions::default()).unwrap();
        assert_eq!(out.len(), 2);
        sheets.reverse();
        let (again, _) = sew(OpKey(3), &sheets, &SewOptions::default()).unwrap();
        let dumps = |x: &[Body]| x.iter().map(Body::dump).collect::<Vec<_>>();
        assert_eq!(dumps(&out), dumps(&again));
        let least = |b: &Body| face_names(b)[0];
        assert!(least(&out[0]) < least(&out[1]));
    }

    #[test]
    fn solids_tolerances_and_three_sheets_at_an_edge_are_refused() {
        let b = block();
        assert!(matches!(
            sew(OpKey(3), std::slice::from_ref(&b), &SewOptions::default()),
            Err(Error::NotASheet { op: "sew" })
        ));
        for tolerance in [0.0, 1e-3, f64::NAN] {
            let options = SewOptions {
                tolerance,
                ..SewOptions::default()
            };
            assert!(matches!(
                sew(OpKey(3), &faces_of(&b, 0.0), &options),
                Err(Error::InvalidHeal(HealError::Malformed { .. }))
            ));
        }
        let faces = faces_of(&b, 0.0);
        let three = [faces[0].clone(), faces[1].clone(), faces[1].clone()];
        assert!(matches!(
            sew(OpKey(3), &three, &SewOptions::default()),
            Err(Error::InvalidHeal(HealError::NonManifold { .. }))
        ));
    }

    #[test]
    fn a_sew_stops_at_a_cancel_and_otherwise_sews_a_block_back() {
        use std::sync::atomic::AtomicBool;
        let b = crate::primitive::block(OpKey::from_bytes(b"b"), 1.0, 2.0, 3.0, &crate::geom::Rigid::IDENTITY)
            .unwrap()
            .0;
        let sheets: Vec<Body> = b
            .faces()
            .map(|f| {
                let n = b.name_of(crate::topo::EntityId::Face(f)).unwrap();
                crate::sheet::extract(OpKey::from_bytes(n.to_string().as_bytes()), &b, &[n]).unwrap().0
            })
            .collect();
        let opts = SewOptions::default();
        let stop = AtomicBool::new(true);
        let e = sew_with(OpKey::from_bytes(b"s"), &sheets, &opts, &crate::work::Work::unlimited().with_cancel(&stop))
            .unwrap_err();
        assert_eq!(e, Error::Cancelled);
        let (pieces, _) = sew_with(OpKey::from_bytes(b"s"), &sheets, &opts, &crate::work::Work::unlimited()).unwrap();
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].kind(), crate::topo::BodyKind::Solid);
        // `sew` is `sew_with` unlimited, the same bits.
        let (again, _) = sew(OpKey::from_bytes(b"s"), &sheets, &opts).unwrap();
        assert_eq!(again[0].dump(), pieces[0].dump());
    }
}
