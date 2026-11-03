//! Dividing a solid or a sheet by sheets (design 295, plan 12.D).
//!
//! The pipeline is the boolean's ([`super::table`], [`crate::split`],
//! [`super::classify`], [`super::build`], [`super::merge`],
//! [`super::names`]); what differs is which pieces each side keeps:
//!
//! - A target piece is on the tools' **front** or **back**. Pieces joined
//!   across edges no tool touches are one group, with one side. A group
//!   meeting a cut takes the side its pieces leave the cut to, by the
//!   tool face's outward normal there; a group lying on a tool's face
//!   takes the back when their normals agree (its material, or a sheet's
//!   back, is behind the tool), else the front. Sides that disagree within
//!   a group mean a cut that doesn't divide it
//!   ([`BooleanError::ToolDoesNotDivide`]), and a group whose cuts are
//!   all tangent is `Uncertain`.
//! - A group the tools don't reach is placed by a ray cast against the
//!   closed tools (inside one: the back, since a closed sheet's normals
//!   point out of it; outside every one: the front). With no closed tool
//!   nothing says which side it is on, and it is `apart`.
//! - For a solid target, each tool piece inside it is used twice: by the
//!   front's solids reversed (their outward normal points to the back)
//!   and by the back's as it is. A tool piece whose edge no other kept
//!   piece of its side takes is an open edge of the tool inside the
//!   solid: `ToolDoesNotDivide`. A sheet target's sides are its own
//!   pieces alone; a cut ending inside one of its faces leaves a piece
//!   that runs one arc both ways, also `ToolDoesNotDivide`.
//!
//! Each side is built ([`super::build::build`] or `build_sheet`), merged
//! and named as one body, with the tools' entities placed under the
//! side's tool names ([`BooleanRole::Front`], [`BooleanRole::Back`]) and
//! faces and edges counted as split across both sides, then cut into its
//! lumps (a solid region, or a connected sheet), which keep every name.

use super::build::{self, Kept};
use super::classify::{self, Class, inner_point};
use super::names::{self, Sides};
use super::table::{Boundary, GeId, Table};
use super::{FaceRef, Fail, Tool, checked, error, merged, place};
use crate::geom::Vector3;
use crate::intersect::{Inclusion, point_in_faces};
use crate::layout::{self, BuildError, Layout, Plan};
use crate::name::{BooleanRole, Journal, Name, OpKey, Role, ToolKey};
use crate::split::Piece;
use crate::topo::{Body, BodyKind, FaceId, Sense};
use crate::work::{Meter, Work};
use crate::{BooleanError, Error, TopologyError};
use std::collections::{BTreeMap, BTreeSet};

/// What a division gives: the bodies on each side of the tools, each a
/// lump (a solid region with its cavities, or a connected sheet), in the
/// order of their least face names.
#[derive(Clone, Debug, Default)]
pub struct Divided {
    /// On the side the tools' normals point to.
    pub front: Vec<Body>,
    /// Behind the tools.
    pub back: Vec<Body>,
    /// Lumps the tools don't reach when no closed tool says which side
    /// they are on, as they were.
    pub apart: Vec<Body>,
}

/// The least a cut's sides may lean off the tool, as the cosine between
/// the way a piece leaves the cut and the tool's normal: a piece leaving
/// tangentially is no evidence either way.
const SIDE: f64 = 1e-9;

/// Divides `target` (a solid or a sheet) by sheet `tools`, placed and
/// keyed as a boolean's; the tools act as their union, which must be a
/// manifold sheet where it lies in the target. Every cut must run from
/// boundary to boundary of what it divides, or close on itself.
///
/// Fails with `NotASheet` for a solid tool, and with
/// [`Error::InvalidBoolean`] as a boolean does, or with
/// [`BooleanError::ToolDoesNotDivide`] naming the face a cut ends in.
pub fn divide(key: OpKey, target: &Body, tools: &[Tool]) -> Result<(Divided, Journal), Error> {
    divide_with(key, target, tools, &Work::unlimited())
}

/// [`divide`] under `work`: refused with `Cancelled` or `BudgetExceeded`
/// when it stops, building nothing.
pub fn divide_with(
    key: OpKey,
    target: &Body,
    tools: &[Tool],
    work: &Work,
) -> Result<(Divided, Journal), Error> {
    crate::work::run(work, |m| divide_in(key, target, tools, m))
}

/// Which side a target piece is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Front,
    Back,
    Apart,
}

fn divide_in(
    key: OpKey,
    target: &Body,
    tools: &[Tool],
    m: &Meter,
) -> Result<(Divided, Journal), Error> {
    for t in tools {
        if t.body.kind() == BodyKind::Solid {
            return Err(Error::NotASheet { op: "divide" });
        }
    }
    let mut keys = BTreeSet::new();
    for t in tools {
        if !keys.insert(t.key) {
            return Err(BooleanError::DuplicateToolKey { key: t.key }.into());
        }
    }
    if cfg!(debug_assertions) {
        for b in core::iter::once(target).chain(tools.iter().map(|t| t.body)) {
            if m.poll().is_err() {
                return Err(Error::Cancelled);
            }
            let faults = crate::check::check(b);
            assert!(
                faults.is_empty(),
                "a division's input is invalid: {faults:?}"
            );
        }
    }
    let solid = target.kind() == BodyKind::Solid;
    let ops = place::operands(target, tools)?;
    let named = |f: Fail| error(&ops, f);
    let face_name = |f: FaceRef| ops[f.op].faces[f.face].name;
    let does_not_divide =
        |f: FaceRef| -> Error { BooleanError::ToolDoesNotDivide { face: face_name(f) }.into() };
    let table = super::table::build(&ops, m).map_err(named)?;
    let pieces = crate::split::split_all(&ops, &table, m).map_err(named)?;
    if let Some(f) = dangling(&table, &pieces) {
        return Err(does_not_divide(f));
    }
    // Rays only for a solid target's tool pieces, against the target.
    let classes =
        classify::classify_with(&ops, &table, &pieces, &|op, j| solid && op > 0 && j == 0, m)
            .map_err(named)?;
    if solid && let Some(f) = crossing(&table, &pieces, &classes) {
        return Err(named(f));
    }
    let sides = sides(&ops, &table, &pieces, &classes, m).map_err(|e| match e {
        Refusal::Fail(f) => named(f),
        Refusal::Divides(f) => does_not_divide(f),
    })?;
    let mut kept: [Vec<Kept>; 3] = Default::default();
    for (i, p) in pieces.iter().enumerate() {
        let keep = |reversed| Kept {
            piece: p.clone(),
            reversed,
        };
        if p.face.op == 0 {
            let s = sides[i].expect("every target piece has a side");
            kept[s as usize].push(keep(false));
        } else if solid && used(&classes[i], p.face.op) {
            kept[Side::Front as usize].push(keep(true));
            kept[Side::Back as usize].push(keep(false));
        }
    }
    if solid {
        for side in &kept[..2] {
            if let Some(f) = open_arc(side) {
                return Err(does_not_divide(f));
            }
        }
    }
    // Each side built and merged, then named as part of the whole.
    let mut made: Vec<Option<(Body, super::merge::Sources)>> = Vec::new();
    for side in &kept {
        if side.is_empty() {
            made.push(None);
            continue;
        }
        let built = if solid {
            build::build(&ops, &table, side, key, m)
        } else {
            build::build_sheet(&ops, &table, side, key)
        }
        .map_err(named)?;
        made.push(Some(
            merged(key, &ops, &table, side, built, m).map_err(named)?,
        ));
    }
    // Every piece once: a solid's tool pieces are on both sides as copies.
    let all: Vec<Kept> = kept
        .iter()
        .enumerate()
        .flat_map(|(s, side)| {
            side.iter()
                .filter(move |k| s != Side::Back as usize || k.piece.face.op == 0)
        })
        .cloned()
        .collect();
    let edges: BTreeSet<GeId> = made
        .iter()
        .flatten()
        .flat_map(|(_, s)| s.edges.values().flatten().copied())
        .collect();
    let tool_keys: Vec<ToolKey> = tools.iter().map(|t| t.key).collect();
    let roles = [BooleanRole::Front, BooleanRole::Back, BooleanRole::Front];
    let mut journals = Vec::new();
    let mut out: [Vec<Body>; 3] = Default::default();
    for (s, m_) in made.into_iter().enumerate() {
        let Some((mut body, sources)) = m_ else {
            continue;
        };
        let sides = Sides {
            role: roles[s],
            kept: &all,
            edges: &edges,
        };
        journals.push(names::name(
            key,
            &ops,
            &tool_keys,
            &table,
            (&pieces, &kept[s]),
            &mut body,
            &sources,
            Some(sides),
        ));
        checked(&body)?;
        out[s] = lumps(key, &body)?;
    }
    let live: BTreeSet<Name> = out
        .iter()
        .flatten()
        .flat_map(|b| {
            b.faces()
                .map(|f| b.faces[f].name)
                .chain(b.edges().map(|e| b.edges[e].name))
                .chain(b.vertices().map(|v| b.vertices[v].name))
                .collect::<Vec<_>>()
        })
        .collect();
    let journal = names::combine(&journals, &live);
    let [front, back, apart] = out;
    Ok((Divided { front, back, apart }, journal))
}

/// Why a division's sides couldn't be told.
enum Refusal {
    Fail(Fail),
    /// Pieces joined across untouched edges lie on both sides of the
    /// tools: a cut ends inside this face.
    Divides(FaceRef),
}

impl From<Fail> for Refusal {
    fn from(f: Fail) -> Self {
        Self::Fail(f)
    }
}

/// A target piece that runs a cut both ways: the cut ends inside its
/// face.
fn dangling(table: &Table, pieces: &[Piece]) -> Option<FaceRef> {
    for p in pieces.iter().filter(|p| p.face.op == 0) {
        let mut seen: BTreeMap<GeId, Sense> = BTreeMap::new();
        for l in &p.loops {
            let Boundary::Arcs(a) = l else { continue };
            for &(g, s) in a {
                let cut = table.edges[g].pcurves.keys().any(|f| f.op > 0);
                if seen.insert(g, s).is_some_and(|t| t != s) && cut {
                    return Some(p.face);
                }
            }
        }
    }
    None
}

/// Tools crossing inside a solid target: an edge more than two of the
/// tool pieces inside it take, where their union isn't a manifold sheet.
fn crossing(table: &Table, pieces: &[Piece], classes: &[Vec<Option<Class>>]) -> Option<Fail> {
    let mut uses: BTreeMap<GeId, Vec<FaceRef>> = BTreeMap::new();
    for (i, p) in pieces.iter().enumerate() {
        if p.face.op == 0 || !used(&classes[i], p.face.op) {
            continue;
        }
        for l in &p.loops {
            if let Boundary::Arcs(a) = l {
                for &(g, _) in a {
                    uses.entry(g).or_default().push(p.face);
                }
            }
        }
    }
    let (&g, faces) = uses.iter().find(|(_, u)| u.len() > 2)?;
    let e = &table.edges[g];
    let at = e.curve.point((e.interval[0] + e.interval[1]) / 2.0);
    Some(Fail::NonManifold {
        at,
        faces: faces.clone(),
    })
}

/// Whether a tool piece bounds a solid target's sides: inside the target,
/// and not lying on a lower tool's face (of coincident tool pieces the
/// lowest tool's stays, as uniting tools does).
fn used(classes: &[Option<Class>], op: usize) -> bool {
    classes[0] == Some(Class::In)
        && classes[1..op]
            .iter()
            .all(|c| !matches!(c, Some(Class::On { .. })))
}

/// The face of a kept piece with an arc no other kept piece of its side
/// takes: in a solid side, an open edge of a tool.
fn open_arc(side: &[Kept]) -> Option<FaceRef> {
    let mut uses: BTreeMap<GeId, Vec<FaceRef>> = BTreeMap::new();
    for k in side {
        for l in &k.piece.loops {
            if let Boundary::Arcs(a) = l {
                for &(g, _) in a {
                    uses.entry(g).or_default().push(k.piece.face);
                }
            }
        }
    }
    uses.values().find(|u| u.len() == 1).map(|u| u[0])
}

/// Each piece's side, `None` for tool pieces.
fn sides(
    ops: &[Body],
    table: &Table,
    pieces: &[Piece],
    classes: &[Vec<Option<Class>>],
    m: &Meter,
) -> Result<Vec<Option<Side>>, Refusal> {
    let groups = classify::groups(ops, table, pieces);
    // Each group's votes from tool faces it lies on and tool edges it
    // meets, its votes from its own edges, and whether it meets a cut.
    let mut votes: BTreeMap<usize, (BTreeSet<u8>, BTreeSet<u8>, bool)> = BTreeMap::new();
    for (i, p) in pieces.iter().enumerate() {
        if p.face.op != 0 {
            continue;
        }
        let entry = votes.entry(groups[i]).or_default();
        for c in classes[i][1..].iter().flatten() {
            if let Class::On { same } = c {
                let s = if *same { Side::Back } else { Side::Front };
                entry.0.insert(s as u8);
                entry.2 = true;
            }
        }
        for l in &p.loops {
            let Boundary::Arcs(a) = l else { continue };
            for &(g, sense) in a {
                // The target's own edges vote only for a group nothing
                // else votes for: a stretch merged onto one where a tool
                // crosses it carries the tool's pcurve but may say the
                // wrong side (seeds 65 and 118), while a group whose only
                // contact with the tools is its own edges (a box's sides
                // under a tool flush with its top) has nothing else to go by.
                let own = matches!(
                    table.edges[g].members[0],
                    super::table::Member::Edge(r) if r.op == 0
                );
                for &f in table.edges[g].pcurves.keys().filter(|f| f.op > 0) {
                    entry.2 = true;
                    if let Some(s) = side_at(ops, table, p, g, sense, f) {
                        if own { &mut entry.1 } else { &mut entry.0 }.insert(s as u8);
                    }
                }
            }
        }
    }
    let closed: Vec<usize> = (1..ops.len())
        .filter(|&j| {
            let b = &ops[j];
            b.faces().next().is_some() && b.edges().all(|e| !b.is_laminar(e))
        })
        .collect();
    let mut side_of: BTreeMap<usize, Side> = BTreeMap::new();
    for (&g, (v, own, cut)) in &votes {
        let first = pieces[g].face;
        let v = if v.is_empty() { own } else { v };
        let side = match v.iter().copied().collect::<Vec<u8>>()[..] {
            [s] if s == Side::Front as u8 => Side::Front,
            [_] => Side::Back,
            [_, _] => return Err(Refusal::Divides(first)),
            _ if *cut => return Err(Fail::Uncertain(first, first).into()),
            _ => untouched(ops, table, &pieces[g], &closed, m)?,
        };
        side_of.insert(g, side);
    }
    Ok(pieces
        .iter()
        .enumerate()
        .map(|(i, p)| (p.face.op == 0).then(|| side_of[&groups[i]]))
        .collect())
}

/// The side a piece of the target leaves cut `g` to, against tool face
/// `f`: the way into the piece, square to the cut on the target's
/// surface, by the tool face's outward normal at the cut's middle. `None`
/// where the piece leaves the cut within [`SIDE`] of tangentially.
fn side_at(
    ops: &[Body],
    table: &Table,
    p: &Piece,
    g: GeId,
    sense: Sense,
    f: FaceRef,
) -> Option<Side> {
    let e = &table.edges[g];
    let mid = (e.interval[0] + e.interval[1]) / 2.0;
    let normal_at = |r: FaceRef| -> Option<(Vector3, Vector3, Vector3)> {
        let b = &ops[r.op];
        let face = &b.faces[r.face];
        let surface = &b.surfaces[face.surface];
        let (c, map) = e.pcurves.get(&r)?;
        let s = map[0] + map[1] * mid;
        let uv = c.point(s);
        let at = surface.eval(uv.x, uv.y);
        let sigma = if face.sense == Sense::Forward {
            1.0
        } else {
            -1.0
        };
        // The cut's direction on this face, by the pcurve's parameter.
        let d = c.eval(s).d1 * map[1];
        Some((at.du.cross(at.dv) * sigma, at.du * d.x + at.dv * d.y, at.du))
    };
    let (n, along, _) = normal_at(p.face)?;
    let along = if sense == Sense::Forward {
        along
    } else {
        -along
    };
    // The piece is on the left of its loops about its outward normal.
    let into = n.cross(along);
    let (tool, _, _) = normal_at(f)?;
    let scale = into.length() * tool.length();
    if !(scale > 0.0 && scale.is_finite()) {
        return None;
    }
    let c = into.dot(tool) / scale;
    if c > SIDE {
        Some(Side::Front)
    } else if c < -SIDE {
        Some(Side::Back)
    } else {
        None
    }
}

/// The side of a group no tool reaches: inside a closed tool is its
/// back, outside every closed tool their front; with none, apart.
fn untouched(
    ops: &[Body],
    table: &Table,
    piece: &Piece,
    closed: &[usize],
    m: &Meter,
) -> Result<Side, Fail> {
    if closed.is_empty() {
        return Ok(Side::Apart);
    }
    let fail = || Fail::Uncertain(piece.face, piece.face);
    'attempts: for attempt in 0..4 {
        let Some((p, _)) = inner_point(ops, table, piece, attempt) else {
            continue;
        };
        let mut inside = false;
        for &j in closed {
            let faces: Vec<FaceId> = ops[j].faces().collect();
            match point_in_faces(&ops[j], &faces, p, m) {
                Ok(Inclusion::Inside) => inside = true,
                Ok(Inclusion::Outside) => {}
                _ => continue 'attempts,
            }
        }
        return Ok(if inside { Side::Back } else { Side::Front });
    }
    Err(fail())
}

/// A body's lumps, each a body of its own keeping every name: a solid's
/// regions with their cavities, a sheet's connected shells; in the order
/// of their least face names.
fn lumps(key: OpKey, body: &Body) -> Result<Vec<Body>, Error> {
    let sheet = body.kind() == BodyKind::Sheet;
    // Each lump's shells, the outer one first.
    let mut groups: Vec<Vec<crate::topo::ShellId>> = Vec::new();
    if sheet {
        groups.extend(body.shells.live().map(|(s, _)| vec![s]));
    } else {
        for (r, region) in body.regions.live() {
            if region.kind == crate::topo::RegionKind::Solid {
                groups.push(body.region_shells(r).collect());
            }
        }
    }
    if groups.len() <= 1 {
        return Ok(vec![body.clone()]);
    }
    let fail = |_: BuildError| Error::Topology(TopologyError::Inconsistent);
    let mut out = Vec::new();
    for shells in groups {
        let chosen: BTreeSet<FaceId> = shells.iter().flat_map(|&s| body.shell_faces(s)).collect();
        let mut l = Layout::default();
        let copied = crate::sheet::copy(&mut l, body, &chosen);
        let role = Role::Boolean(BooleanRole::Merged);
        let mut built = if sheet {
            layout::build_sheet(&l, key, role).map_err(fail)?
        } else {
            let plan = Plan::new(&l).map_err(fail)?;
            let found = plan.shells();
            // A layout face's shell in `body`.
            let shell_of: BTreeMap<usize, crate::topo::ShellId> = copied
                .faces
                .iter()
                .map(|&(f, k)| (k, body.faces[f].shell))
                .collect();
            let outer = found
                .iter()
                .position(|fs| shell_of[&fs[0]] == shells[0])
                .ok_or(Error::Topology(TopologyError::Inconsistent))?;
            let outer_of: Vec<Option<usize>> = (0..found.len())
                .map(|s| (s != outer).then_some(outer))
                .collect();
            plan.build(key, role, &found, &outer_of).map_err(fail)?
        };
        let mut lump = std::mem::take(&mut built.body);
        lump.set_recipes(true);
        crate::sheet::name_copied(&mut lump, body, &copied, &built);
        out.push(lump);
    }
    let least = |b: &Body| b.faces().map(|f| b.faces[f].name).min();
    out.sort_by_key(least);
    Ok(out)
}
