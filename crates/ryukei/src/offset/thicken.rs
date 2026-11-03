//! Thickened sheets: a solid between a sheet's two offsets, closed along
//! its boundary by walls.
//!
//! Each side is an offset of the sheet (see the module above): the front
//! by `front` along the faces' normals, the back by `back` against them,
//! every inner edge classified, moved, filled or closed and every inner
//! vertex taken as [`super::offset_body`] takes them; a side at zero is
//! the sheet's own faces. The back is built facing the other way. Along
//! each laminar edge a wall joins the two sides' rails: the surface the
//! edge's points sweep along the face's normal (see [`super::walls`]),
//! one face for a run of laminar edges whose walls lie on one plane. At a
//! vertex where two walls on different surfaces meet, they share an edge
//! along the vertex's normal line, from the back's point to the front's.
//!
//! A closed sheet has no walls: thickened it is two shells, the outer the
//! side its normals point to when they point out of it, the other a
//! cavity.
//!
//! **Names.** On a side at zero everything keeps the sheet's name. On a
//! moved side a face, an edge or vertex that is still one, and a laminar
//! edge's rail take [`ThickenRole::Front`] or [`ThickenRole::Back`] from
//! the sheet's entity; fills, rails, sections, ends and new points are
//! named as an offset names them, under the side's key (that role with
//! no parent). A wall is [`ThickenRole::Wall`] from its laminar edges'
//! names, sorted; an edge along a vertex's normal [`ThickenRole::Normal`]
//! from the vertex. A new distance renames nothing.

use super::build::{EdgeRole, Laid, Side, flip, judge, pcurves};
use super::classify::{Plan, usable};
use super::vertices::{Part, VUse, VertexGeo};
use super::walls::{Wall, ruled, same_plane, wall};
use super::{edges, laminar, vertices};
use crate::geom::{Curve, Line};
use crate::layout::{self, Boundary, BuildError, Use};
use crate::name::{Event, Journal, Name, OffsetRole, OpKey, Role, ThickenRole};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, BodyKind, EdgeId, EntityId, FaceId, FinId, Sense, ShellId, VertexId};
use crate::work::{Meter, Work};
use crate::{Error, OffsetCase, OffsetError};
use std::collections::{BTreeMap, BTreeSet};

/// How thick a sheet is made: `front` along its faces' normals and `back`
/// against them, but for the faces `faces` names, each with its own
/// `(name, front, back)`. Neither is negative; on a face they may not
/// both be zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThickenSpec {
    pub front: f64,
    pub back: f64,
    pub faces: Vec<(Name, f64, f64)>,
}

/// `sheet` thickened by `spec`: a new solid and the journal of what
/// happened to names. See the module's documentation for what is built.
/// Refusals are `Error::InvalidOffset` as [`super::offset_body`] makes
/// them (`BadDistance`, `NoSuchFace`, `FaceCollapses`, `EdgeVanishes`,
/// `SelfIntersecting`, `Unsupported`, `Uncertain`), with
/// [`OffsetCase::LaminarVertex`] for a boundary vertex it doesn't cover;
/// a solid is `NotASheet`. Nothing is built until every step has
/// succeeded.
pub fn thicken(key: OpKey, sheet: &Body, spec: &ThickenSpec) -> Result<(Body, Journal), Error> {
    thicken_with(key, sheet, spec, &Work::unlimited())
}

/// [`thicken`] under `work`: refused with `Cancelled` or
/// `BudgetExceeded` when it stops, building nothing.
pub fn thicken_with(
    key: OpKey,
    sheet: &Body,
    spec: &ThickenSpec,
    work: &Work,
) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |meter| thicken_in(key, sheet, spec, meter))
}

/// The key a side's offset names its new entities under.
fn side_key(key: OpKey, tag: ThickenRole) -> OpKey {
    OpKey(Name::derive(key, Role::Thicken(tag), &[]).to_u128())
}

const TAGS: [ThickenRole; 2] = [ThickenRole::Front, ThickenRole::Back];

/// Each face's front and back distances, checked.
fn distances(body: &Body, spec: &ThickenSpec) -> Result<BTreeMap<FaceId, [f64; 2]>, Error> {
    let mut faces: Vec<FaceId> = body.faces().collect();
    faces.sort_by_key(|&f| body.faces[f].name);
    let by_name: BTreeMap<Name, FaceId> = faces.iter().map(|&f| (body.faces[f].name, f)).collect();
    let mut out: BTreeMap<FaceId, [f64; 2]> = faces
        .iter()
        .map(|&f| (f, [spec.front, spec.back]))
        .collect();
    let mut named: BTreeSet<Name> = BTreeSet::new();
    for &(face, f, b) in &spec.faces {
        let id = *by_name.get(&face).ok_or(OffsetError::NoSuchFace { face })?;
        if !named.insert(face) {
            return Err(OffsetError::NoSuchFace { face }.into());
        }
        out.insert(id, [f, b]);
    }
    let ok = |[f, b]: [f64; 2]| usable(f) && usable(b) && f >= 0.0 && b >= 0.0 && f + b > 0.0;
    if out.is_empty() || !out.values().all(|&d| ok(d)) {
        return Err(OffsetError::BadDistance.into());
    }
    Ok(out)
}

/// One side worked out: its plan, edges, rails and vertices.
struct Worked<'a> {
    plan: Plan<'a>,
    geo: BTreeMap<EdgeId, edges::EdgeGeo>,
    rails: BTreeMap<EdgeId, edges::NewCurve>,
    vgeo: BTreeMap<VertexId, VertexGeo>,
    /// Every face of the side at zero: the sheet's own faces.
    zero: bool,
}

fn work_side<'a>(
    body: &'a Body,
    dist: &BTreeMap<FaceId, [f64; 2]>,
    k: usize,
    walls: &BTreeMap<EdgeId, Wall>,
    meter: &Meter,
) -> Result<Worked<'a>, Error> {
    let sign = if k == 0 { 1.0 } else { -1.0 };
    let d: BTreeMap<FaceId, f64> = dist.iter().map(|(&f, x)| (f, sign * x[k])).collect();
    let zero = d.values().all(|&x| x == 0.0);
    let plan = Plan::with_distances(body, d)?;
    let geo = edges::curves(&plan, meter)?;
    let rails = edges::rails(&plan, meter)?;
    let mut vgeo = vertices::vertices(&plan, &geo, meter)?;
    vgeo.extend(laminar::vertices(&plan, &geo, &rails, walls, meter)?);
    Ok(Worked {
        plan,
        geo,
        rails,
        vgeo,
        zero,
    })
}

/// A run of laminar fins in their boundary's order whose walls are one
/// face: all of a boundary (`closed`), or from one vertex where walls
/// change to the next.
struct Group {
    fins: Vec<FinId>,
    closed: bool,
}

/// The boundaries' runs, in the order of each boundary's least-named
/// laminar edge, each boundary from that edge's fin.
fn groups(body: &Body, laminar: &[EdgeId], walls: &BTreeMap<EdgeId, Wall>) -> Vec<Group> {
    let mut seen: BTreeSet<FinId> = BTreeSet::new();
    let mut out = Vec::new();
    // `laminar` is in name order, so each ring starts at its least name.
    for &e in laminar {
        let f0 = body.edges[e].fin;
        if seen.contains(&f0) {
            continue;
        }
        let mut ring = vec![f0];
        let mut f = body.boundary_next(f0);
        while f != f0 && ring.len() <= body.fins.slot_count() {
            ring.push(f);
            f = body.boundary_next(f);
        }
        seen.extend(ring.iter().copied());
        let n = ring.len();
        let wall_of = |f: FinId| &walls[&body.fins[f].edge];
        let joins: Vec<bool> = (0..n)
            .map(|i| n == 1 || same_plane(wall_of(ring[i]), wall_of(ring[(i + 1) % n])))
            .collect();
        if joins.iter().all(|&j| j) {
            out.push(Group {
                fins: ring,
                closed: true,
            });
            continue;
        }
        let first = joins.iter().position(|&j| !j).expect("a break");
        let mut cur: Vec<FinId> = Vec::new();
        for i in 1..=n {
            let k = (first + i) % n;
            cur.push(ring[k]);
            if !joins[k] {
                out.push(Group {
                    fins: std::mem::take(&mut cur),
                    closed: false,
                });
            }
        }
    }
    out
}

/// The key of a laminar fin's end in its vertex's `laminar` map, and that
/// vertex: at its start (`end` false) or its end.
fn fin_end(body: &Body, f: FinId, end: bool) -> (VertexId, (EdgeId, bool)) {
    let fin = &body.fins[f];
    let forward = fin.sense == Sense::Forward;
    let v = if end {
        body.end_of(f)
    } else {
        body.start_of(f)
    };
    (v, (fin.edge, end == forward))
}

fn thicken_in(
    key: OpKey,
    sheet: &Body,
    spec: &ThickenSpec,
    meter: &Meter,
) -> Result<(Body, Journal), Error> {
    if sheet.kind() != BodyKind::Sheet {
        return Err(Error::NotASheet { op: "thicken" });
    }
    let body = sheet;
    let dist = distances(body, spec)?;
    let mut laminar_edges: Vec<EdgeId> = body.edges().filter(|&e| body.is_laminar(e)).collect();
    laminar_edges.sort_by_key(|&e| body.edges[e].name);
    let walls: BTreeMap<EdgeId, Wall> = laminar_edges.iter().map(|&e| (e, wall(body, e))).collect();
    let sides = [
        work_side(body, &dist, 0, &walls, meter)?,
        work_side(body, &dist, 1, &walls, meter)?,
    ];
    let mut laid = Laid::new(body);
    for (k, w) in sides.iter().enumerate() {
        let side = Side {
            plan: &w.plan,
            geo: &w.geo,
            vgeo: &w.vgeo,
            rails: &w.rails,
            flip: k == 1,
        };
        laid.lay(k, &side)?;
    }
    let groups = groups(body, &laminar_edges, &walls);
    lay_walls(&mut laid, body, &sides, &walls, &groups)?;
    finish(key, body, &sides, &groups, laid, meter)
}

/// The walls: each group's face, and the edges along vertices' normals
/// where groups meet.
fn lay_walls(
    laid: &mut Laid,
    body: &Body,
    sides: &[Worked; 2],
    walls: &BTreeMap<EdgeId, Wall>,
    groups: &[Group],
) -> Result<(), Error> {
    let mut normals: BTreeMap<VertexId, usize> = BTreeMap::new();
    for g in groups {
        let e0 = body.fins[g.fins[0]].edge;
        let at = body.edges[e0].name;
        let surface = match &walls[&e0] {
            Wall::Exact(s) => s.clone(),
            Wall::Ruled => {
                if g.closed {
                    // A closed ruled wall would be a clamped surface whose
                    // ends meet, which nothing meshes.
                    return Err(OffsetError::Unsupported {
                        at: body.vertices[body.edges[e0].start].name,
                        case: OffsetCase::LaminarVertex,
                    }
                    .into());
                }
                let rail = |k: usize| {
                    let le = &laid.layout.edges[laid.rails[&(k, e0)]];
                    (laid.layout.curves[le.curve].clone(), le.interval)
                };
                let (f, b) = (rail(0), rail(1));
                ruled((&f.0, f.1), (&b.0, b.1)).ok_or(OffsetError::Uncertain { at })?
            }
        };
        // Facing away from the sheet, across the first fin's edge.
        let fin = g.fins[0];
        let edge = &body.edges[e0];
        let t = (edge.interval[0] + edge.interval[1]) / 2.0;
        let c = body.curves[edge.curve].eval(t);
        let along = if body.fins[fin].sense == Sense::Forward {
            c.d1
        } else {
            -c.d1
        };
        let out = along.cross(super::classify::normal(body, fin, t));
        let foot = surface.project(c.point);
        let n = surface
            .normal(foot.param.0, foot.param.1)
            .ok_or(OffsetError::Uncertain { at })?
            .get();
        let sense = if n.dot(out) > 0.0 {
            Sense::Forward
        } else {
            Sense::Reversed
        };
        // The point index of a fin's end on side k.
        let rail_use = |laid: &mut Laid, k: usize, f: FinId| -> Use {
            let e = body.fins[f].edge;
            let s = body.fins[f].sense;
            Use {
                edge: laid.rails[&(k, e)],
                sense: if k == 0 { flip(s) } else { s },
                pcurve: None,
            }
        };
        // The wall's arcs at a vertex inside the run on side k, from the
        // fin `a` before it to `b` after it (front), or back.
        let extras = |laid: &mut Laid, k: usize, a: FinId, b: FinId| -> Vec<Use> {
            let (v, key_a) = fin_end(body, a, true);
            let (_, key_b) = fin_end(body, b, false);
            let g = &sides[k].vgeo[&v];
            let (pa, pb) = (g.laminar[&key_a].0, g.laminar[&key_b].0);
            if pa == pb {
                return Vec::new();
            }
            // The arc between the two faces' points: the front runs from
            // `b`'s point to `a`'s, the back from `a`'s to `b`'s.
            let Some(i) = g
                .edges
                .iter()
                .position(|x| x.parts.iter().any(|p| matches!(p.0, Part::Wall(_))))
            else {
                return Vec::new();
            };
            let from = if k == 0 { pb } else { pa };
            vec![laid.vuse(
                k,
                v,
                g,
                VUse { edge: i, from },
                Part::Wall(body.fins[a].edge),
            )]
        };
        let n_ = g.fins.len();
        let mut front: Vec<Use> = Vec::new();
        for i in (0..n_).rev() {
            front.push(rail_use(laid, 0, g.fins[i]));
            if i > 0 {
                let x = extras(laid, 0, g.fins[i - 1], g.fins[i]);
                front.extend(x);
            }
        }
        if g.closed {
            let x = extras(laid, 0, g.fins[n_ - 1], g.fins[0]);
            front.extend(x);
        }
        let mut back: Vec<Use> = Vec::new();
        for i in 0..n_ {
            back.push(rail_use(laid, 1, g.fins[i]));
            if i + 1 < n_ {
                let x = extras(laid, 1, g.fins[i], g.fins[i + 1]);
                back.extend(x);
            }
        }
        if g.closed {
            let x = extras(laid, 1, g.fins[n_ - 1], g.fins[0]);
            back.extend(x);
        }
        let loops = if g.closed {
            vec![Boundary::Arcs(front), Boundary::Arcs(back)]
        } else {
            let mut normal = |laid: &mut Laid, f: FinId, end: bool| -> Result<usize, Error> {
                let (v, _) = fin_end(body, f, end);
                if let Some(&id) = normals.get(&v) {
                    return Ok(id);
                }
                let pts = [
                    point_of(laid, body, sides, f, end, 1),
                    point_of(laid, body, sides, f, end, 0),
                ];
                let (b, fr) = (
                    laid.layout.vertices[pts[0]].point,
                    laid.layout.vertices[pts[1]].point,
                );
                let at = body.vertices[v].name;
                let line = Line::new(b, fr - b).map_err(|_| OffsetError::Uncertain { at })?;
                let len = (fr - b).length();
                let id = laid.add_edge(
                    &Curve::Line(line),
                    [0.0, len],
                    (pts[0], pts[1]),
                    LINEAR_RESOLUTION,
                    EdgeRole::Normal(v),
                );
                normals.insert(v, id);
                Ok(id)
            };
            let start = normal(laid, g.fins[0], false)?;
            let end = normal(laid, g.fins[n_ - 1], true)?;
            let mut arcs = front;
            arcs.push(Use {
                edge: start,
                sense: Sense::Reversed,
                pcurve: None,
            });
            arcs.extend(back);
            arcs.push(Use {
                edge: end,
                sense: Sense::Forward,
                pcurve: None,
            });
            vec![Boundary::Arcs(arcs)]
        };
        laid.layout.surfaces.push(surface);
        let s = laid.layout.surfaces.len() - 1;
        laid.layout.faces.push(layout::Face {
            surface: s,
            sense,
            loops,
        });
        laid.parts.push((0, Part::Wall(e0)));
    }
    Ok(())
}

/// The layout vertex of a laminar fin's end on side `k`.
fn point_of(laid: &Laid, body: &Body, sides: &[Worked; 2], f: FinId, end: bool, k: usize) -> usize {
    let (v, key) = fin_end(body, f, end);
    let idx = sides[k].vgeo[&v].laminar[&key].0;
    laid.points[&(k, v, idx)]
}

/// The layout built as a solid: one shell per open piece of the sheet,
/// two per closed piece (the outer the side its normals point out to),
/// named, given every pcurve, journalled and checked.
fn finish(
    key: OpKey,
    body: &Body,
    sides: &[Worked; 2],
    groups: &[Group],
    laid: Laid,
    meter: &Meter,
) -> Result<(Body, Journal), Error> {
    let least = body
        .faces()
        .map(|f| body.faces[f].name)
        .min()
        .expect("a face");
    let uncertain = |at: Name| -> Error { OffsetError::Uncertain { at }.into() };
    let fail = |e: BuildError| -> Error {
        match e {
            BuildError::NonManifold { faces, .. } => {
                let at = faces
                    .iter()
                    .find_map(|&k| match laid.parts[k].1 {
                        Part::Face(f) => Some(body.faces[f].name),
                        _ => None,
                    })
                    .unwrap_or(least);
                uncertain(at)
            }
            BuildError::Inconsistent => uncertain(least),
        }
    };
    let lp = layout::Plan::new(&laid.layout).map_err(fail)?;
    let shells = lp.shells();
    // Each layout shell's sheet shell and side, from a face's offset in it.
    let of = |s: &[usize]| -> Option<(ShellId, usize)> {
        s.iter().find_map(|&k| match laid.parts[k] {
            (side, Part::Face(f)) => Some((body.faces[f].shell, side)),
            _ => None,
        })
    };
    let mut outer_of: Vec<Option<usize>> = vec![None; shells.len()];
    let mut by_sheet: BTreeMap<ShellId, Vec<(usize, usize)>> = BTreeMap::new();
    for (i, s) in shells.iter().enumerate() {
        let (sh, side) = of(s).ok_or_else(|| uncertain(least))?;
        by_sheet.entry(sh).or_default().push((i, side));
    }
    for (sh, list) in &by_sheet {
        match list.as_slice() {
            [_] => {}
            [(a, sa), (b, _)] => {
                let faces: Vec<FaceId> = body.shell_faces(*sh).collect();
                let out = crate::heal::shells::facing(body, &faces, meter)?;
                // Normals out of the closed sheet: the front is outer.
                let front_outer = out;
                let (outer, inner) = if (*sa == 0) == front_outer {
                    (*a, *b)
                } else {
                    (*b, *a)
                };
                outer_of[inner] = Some(outer);
            }
            _ => return Err(uncertain(least)),
        }
    }
    let built = lp
        .build(key, Role::Offset(OffsetRole::Point), &shells, &outer_of)
        .map_err(fail)?;
    let mut out = built.body;
    out.set_recipes(true);
    let mut journal = Journal::new();
    let keys = [side_key(key, TAGS[0]), side_key(key, TAGS[1])];
    // A sheet entity's name on side k: its own at zero, else the side's.
    let moved = |k: usize, n: Name| -> Name {
        if sides[k].zero {
            n
        } else {
            Name::derive(key, Role::Thicken(TAGS[k]), &[n])
        }
    };
    let group_name = |e: EdgeId| -> Name {
        let g = groups
            .iter()
            .find(|g| body.fins[g.fins[0]].edge == e)
            .expect("a group");
        let mut names: Vec<Name> = g
            .fins
            .iter()
            .map(|&f| body.edges[body.fins[f].edge].name)
            .collect();
        names.sort();
        names.dedup();
        Name::derive(key, Role::Thicken(ThickenRole::Wall), &names)
    };
    let part_name = |(k, p): (usize, Part)| -> Name {
        match p {
            Part::Face(f) => moved(k, body.faces[f].name),
            Part::Fill(e) => Name::derive(
                keys[k],
                Role::Offset(OffsetRole::Edge),
                &[body.edges[e].name],
            ),
            Part::Sphere(v) => Name::derive(
                keys[k],
                Role::Offset(OffsetRole::Vertex),
                &[body.vertices[v].name],
            ),
            Part::Wall(e) => group_name(e),
        }
    };
    let mut seen: BTreeSet<Name> = BTreeSet::new();
    let note = |journal: &mut Journal, n: Name, kept: bool| {
        journal.push(if kept {
            Event::Modified(n)
        } else {
            Event::Created(n)
        });
    };
    let mut face_names: BTreeMap<usize, Name> = BTreeMap::new();
    for (k, &f) in built.faces.iter().enumerate() {
        let part = laid.parts[k];
        let name = part_name(part);
        let kept = matches!(part, (s, Part::Face(_)) if sides[s].zero);
        if !seen.insert(name) {
            return Err(uncertain(name));
        }
        note(&mut journal, name, kept);
        face_names.insert(k, name);
        out.rename(EntityId::Face(f), name);
    }
    for (&g, &e) in &built.edges {
        let (name, kept) = match laid.roles[g] {
            EdgeRole::Kept(k, o) | EdgeRole::Laminar(k, o) => {
                (moved(k, body.edges[o].name), sides[k].zero)
            }
            EdgeRole::Rail(k, o, side) => {
                let fill = part_name((k, Part::Fill(o)));
                let face = part_name((
                    k,
                    Part::Face(sides[k].plan.face_of(sides[k].plan.fins(o)[side])),
                ));
                (
                    Name::derive(keys[k], Role::Offset(OffsetRole::Rail), &[fill, face]),
                    false,
                )
            }
            EdgeRole::Vertex(k, v, parts) => {
                let names = parts.map(|p| part_name((k, p)));
                let vn = body.vertices[v].name;
                let n = match parts {
                    [Part::Fill(_), Part::Face(_) | Part::Wall(_)] => Name::derive(
                        keys[k],
                        Role::Offset(OffsetRole::End),
                        &[names[0], names[1], vn],
                    ),
                    _ => {
                        let mut s = names;
                        s.sort();
                        Name::derive(
                            keys[k],
                            Role::Offset(OffsetRole::Section),
                            &[s[0], s[1], vn],
                        )
                    }
                };
                (n, false)
            }
            EdgeRole::Normal(v) => (
                Name::derive(
                    key,
                    Role::Thicken(ThickenRole::Normal),
                    &[body.vertices[v].name],
                ),
                false,
            ),
        };
        if !seen.insert(name) {
            return Err(uncertain(name));
        }
        note(&mut journal, name, kept);
        out.rename(EntityId::Edge(e), name);
    }
    // Vertices: one a side keeps as one vertex is the sheet's, moved;
    // any other is named by the edges there, under its side's key.
    let mut kept_points: BTreeMap<usize, (usize, VertexId)> = BTreeMap::new();
    let mut side_of: BTreeMap<usize, usize> = BTreeMap::new();
    for (&(k, v, _), &lv) in &laid.points {
        side_of.insert(lv, k);
        if sides[k].vgeo[&v].kept {
            kept_points.insert(lv, (k, v));
        }
    }
    for (&g, &v) in &built.vertices {
        let (name, kept) = match kept_points.get(&g) {
            Some(&(k, o)) => (moved(k, body.vertices[o].name), sides[k].zero),
            None => {
                let mut edges: Vec<Name> = out
                    .vertex_edges(v)
                    .into_iter()
                    .map(|e| out.edges[e].name)
                    .collect();
                edges.sort();
                edges.dedup();
                let k = side_of.get(&g).copied().unwrap_or(0);
                (
                    Name::derive(keys[k], Role::Offset(OffsetRole::Point), &edges),
                    false,
                )
            }
        };
        if !seen.insert(name) {
            return Err(uncertain(name));
        }
        note(&mut journal, name, kept);
        out.rename(EntityId::Vertex(v), name);
    }
    // What the sheet's faces and laminar edges became.
    for f in body.faces() {
        let n = body.faces[f].name;
        if sides.iter().any(|s| s.zero) {
            continue;
        }
        let new: Vec<Name> = (0..2).map(|k| moved(k, n)).collect();
        journal.push(Event::Replaced { old: n, new });
    }
    for (k, &(_, p)) in laid.parts.iter().enumerate() {
        if let Part::Wall(e0) = p {
            let g = groups
                .iter()
                .find(|g| body.fins[g.fins[0]].edge == e0)
                .expect("a group");
            for &f in &g.fins {
                let e = body.fins[f].edge;
                journal.push(Event::Replaced {
                    old: body.edges[e].name,
                    new: vec![face_names[&k]],
                });
            }
        }
    }
    pcurves(&mut out, meter).map_err(|_| uncertain(least))?;
    judge(&out, meter)?;
    Ok((out, journal))
}
