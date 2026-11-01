//! Sweeps into sheets (design 295, plan 12.B).
//!
//! A profile made by [`super::ValidProfile::surface`] sweeps to a sheet.
//! Its loops are swept as a solid by the sweep's own code and their sides
//! copied without the caps; each open chain is a [`Grid`]: a side per
//! segment and stretch, rails along the chain at each station (the
//! profile placed at a cap or a joint) and laterals along the motion at
//! each chain vertex. Everything goes into one layout, built as a sheet
//! by [`crate::layout::build_sheet`], and is named as the solid sweeps
//! name: a chain's segments as a loop's would be, its ends by
//! [`SweepRole::ChainEnd`] stand-ins where a loop has the segment before
//! or after.
//!
//! A side's front is the chain's right in the profile's plane, as a
//! loop's outward side is; its loop runs counterclockwise about it: along
//! the rail at the stretch's start and up the lateral at the chain's far
//! end when the motion runs along the profile's normal, the other way
//! round when it runs against it.

use super::names::Namer;
use super::{ValidChain, ValidProfile};
use crate::geom::{Curve, Point3, Surface};
use crate::layout::{self, Boundary, Built, Layout, Use};
use crate::name::{Journal, Name, OpKey, Role, SweepRole};
use crate::sheet;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EntityId, FaceId, Sense};
use crate::{Error, TopologyError};
use std::collections::BTreeSet;

/// A lateral edge: its curve over an increasing interval, running with the
/// motion, and its tolerance.
#[derive(Clone, Debug)]
pub(crate) struct Lateral {
    pub curve: Curve,
    pub interval: [f64; 2],
    pub tolerance: f64,
}

/// The chain placed at one station: each vertex's point and tolerance,
/// and each segment's rail, running along the chain. A grid that closes in
/// one stretch (a full turn) has no rails: its laterals are closed.
#[derive(Clone, Debug)]
pub(crate) struct Station {
    pub points: Vec<Point3>,
    pub tolerances: Vec<f64>,
    pub rails: Vec<(Curve, [f64; 2])>,
}

/// One stretch from a station to the next: each segment's side and its
/// sense, and each vertex's lateral (`None` for a vertex that stays put,
/// on a revolve's axis).
#[derive(Clone, Debug)]
pub(crate) struct Stretch {
    pub sides: Vec<(Surface, Sense)>,
    pub laterals: Vec<Option<Lateral>>,
}

/// How a grid's entities are named.
#[derive(Clone, Debug)]
pub(crate) enum Naming {
    /// An extrude's, a revolve's or a sweep along a path's: a side from
    /// its segment (and its path segment), a lateral and a vertex from the
    /// segments either side of the chain vertex (and the path segments).
    Swept {
        chain: u128,
        segments: Vec<u128>,
        paths: Option<Vec<u128>>,
    },
    /// A loft's: a side from the chain and its segment in every section,
    /// `sections[i]` holding segment `i`'s keys; a lateral and a vertex
    /// from the sides either side.
    Lofted {
        chain: u128,
        sections: Vec<Vec<u128>>,
    },
}

/// A chain's sweep. `stretches[r]` runs from station `r` to station
/// `r + 1`, or round to station 0 after the last when `closed`. `sign` is
/// `+1` when the motion runs along the profile's normal. `apexes` say
/// whether a chain end left on an axis by a rail-less grid is a vertex of
/// its side (an apex) or not (a pole).
#[derive(Clone, Debug)]
pub(crate) struct Grid {
    pub sign: f64,
    pub stations: Vec<Station>,
    pub stretches: Vec<Stretch>,
    pub closed: bool,
    pub apexes: [bool; 2],
    pub naming: Naming,
}

/// Where a grid went in the layout: faces and laterals by stretch, rails
/// and vertices by station, each by segment or chain vertex.
struct Placed {
    faces: Vec<Vec<usize>>,
    rails: Vec<Vec<usize>>,
    laterals: Vec<Vec<Option<usize>>>,
    vertices: Vec<Vec<Option<usize>>>,
}

impl Grid {
    fn segments(&self) -> usize {
        self.stretches[0].sides.len()
    }

    fn railless(&self) -> bool {
        self.stations[0].rails.is_empty()
    }

    /// The station a stretch ends at.
    fn next(&self, r: usize) -> usize {
        if self.closed {
            (r + 1) % self.stations.len()
        } else {
            r + 1
        }
    }
}

/// A one-use loop step: edge `e` the way `forward` says, flipped when the
/// grid runs against the normal.
fn step(e: usize, forward: bool, sign: f64) -> Use {
    let sense = if forward == (sign > 0.0) {
        Sense::Forward
    } else {
        Sense::Reversed
    };
    Use {
        edge: e,
        sense,
        pcurve: None,
    }
}

fn place(out: &mut Layout, g: &Grid) -> Placed {
    let n = g.segments();
    let stations = g.stations.len();
    let railless = g.railless();
    let mut vertices: Vec<Vec<Option<usize>>> = Vec::with_capacity(stations);
    for (s, station) in g.stations.iter().enumerate() {
        let row = (0..=n)
            .map(|i| {
                // A vertex that stays put is the one before it.
                if s > 0 && g.stretches[s - 1].laterals[i].is_none() {
                    return vertices[s - 1][i];
                }
                let apex = (i == 0 && g.apexes[0]) || (i == n && g.apexes[1]);
                if railless && g.stretches[0].laterals[i].is_none() && !apex {
                    return None;
                }
                out.vertices.push(layout::Vertex {
                    point: station.points[i],
                    tolerance: station.tolerances[i],
                });
                Some(out.vertices.len() - 1)
            })
            .collect();
        vertices.push(row);
    }
    let edge = |out: &mut Layout, a: usize, z: usize, curve: Curve, interval, tolerance| {
        out.curves.push(curve);
        out.edges.push(layout::Edge {
            start: a,
            end: z,
            curve: out.curves.len() - 1,
            interval,
            tolerance,
        });
        out.edges.len() - 1
    };
    let mut rails = Vec::with_capacity(stations);
    for (s, station) in g.stations.iter().enumerate() {
        let row: Vec<usize> = station
            .rails
            .iter()
            .enumerate()
            .map(|(i, (curve, interval))| {
                let (a, z) = (vertices[s][i], vertices[s][i + 1]);
                edge(
                    out,
                    a.expect("a rail's start"),
                    z.expect("a rail's end"),
                    curve.clone(),
                    *interval,
                    LINEAR_RESOLUTION,
                )
            })
            .collect();
        rails.push(row);
    }
    let mut laterals = Vec::with_capacity(g.stretches.len());
    for (r, stretch) in g.stretches.iter().enumerate() {
        let to = g.next(r);
        let row: Vec<Option<usize>> = stretch
            .laterals
            .iter()
            .enumerate()
            .map(|(i, l)| {
                l.as_ref().map(|l| {
                    let (a, z) = (vertices[r][i], vertices[to][i]);
                    edge(
                        out,
                        a.expect("a lateral's start"),
                        z.expect("a lateral's end"),
                        l.curve.clone(),
                        l.interval,
                        l.tolerance,
                    )
                })
            })
            .collect();
        laterals.push(row);
    }
    let mut faces = Vec::with_capacity(g.stretches.len());
    for (r, stretch) in g.stretches.iter().enumerate() {
        let to = g.next(r);
        let mut row = Vec::with_capacity(n);
        for (i, (surface, sense)) in stretch.sides.iter().enumerate() {
            out.surfaces.push(surface.clone());
            let (before, after) = (laterals[r][i], laterals[r][i + 1]);
            let loops = if railless {
                let mut loops: Vec<Boundary> = Vec::new();
                for (lat, forward, end) in [(after, true, i + 1), (before, false, i)] {
                    match lat {
                        Some(e) => loops.push(Boundary::Arcs(vec![step(e, forward, g.sign)])),
                        None => {
                            if let Some(v) = vertices[0][end] {
                                loops.push(Boundary::Lone(v));
                            }
                        }
                    }
                }
                loops
            } else {
                let mut uses = vec![step(rails[r][i], true, 1.0)];
                uses.extend(after.map(|e| step(e, true, 1.0)));
                uses.push(step(rails[to][i], false, 1.0));
                uses.extend(before.map(|e| step(e, false, 1.0)));
                if g.sign < 0.0 {
                    uses.reverse();
                    for u in &mut uses {
                        u.sense = u.sense.flipped();
                    }
                }
                vec![Boundary::Arcs(uses)]
            };
            out.faces.push(layout::Face {
                surface: out.surfaces.len() - 1,
                sense: *sense,
                loops,
            });
            row.push(out.faces.len() - 1);
        }
        faces.push(row);
    }
    Placed {
        faces,
        rails,
        laterals,
        vertices,
    }
}

/// The names of the stand-ins a chain's naming needs.
struct Stands {
    /// Each side's parents but its path segment, by segment.
    sides: Vec<Vec<Name>>,
    /// Each path segment's stand-in, by stretch.
    paths: Option<Vec<Name>>,
    /// The chain's ends and caps.
    ends: [Name; 2],
    caps: [Name; 2],
    /// Laterals and vertices take the names of the sides either side, not
    /// of the segments.
    by_sides: bool,
}

fn stands(b: &mut Body, namer: &Namer, naming: &Naming) -> Stands {
    let (chain, by_sides) = match naming {
        Naming::Swept { chain, .. } => (*chain, false),
        Naming::Lofted { chain, .. } => (*chain, true),
    };
    let ch = namer.loop_(b, chain);
    let ends = [0, 1].map(|e| {
        let marker = Name::from_u128(e);
        namer.stand_in(b, SweepRole::ChainEnd, &[ch, marker])
    });
    let caps = [SweepRole::StartCap, SweepRole::EndCap].map(|r| namer.stand_in(b, r, &[ch]));
    let (sides, paths) = match naming {
        Naming::Swept {
            segments, paths, ..
        } => (
            segments
                .iter()
                .map(|&k| vec![namer.segment(b, k)])
                .collect(),
            paths
                .as_ref()
                .map(|p| p.iter().map(|&k| namer.path_segment(b, k)).collect()),
        ),
        Naming::Lofted { sections, .. } => (
            sections
                .iter()
                .map(|keys| {
                    let mut parents = vec![ch];
                    parents.extend(keys.iter().map(|&k| namer.segment(b, k)));
                    parents
                })
                .collect(),
            None,
        ),
    };
    Stands {
        sides,
        paths,
        ends,
        caps,
        by_sides,
    }
}

fn name_grid(b: &mut Body, namer: &Namer, g: &Grid, at: &Placed, built: &Built) {
    let st = stands(b, namer, &g.naming);
    let n = g.segments();
    let stations = g.stations.len();
    let rows = g.stretches.len();
    let path = |r: usize| st.paths.as_ref().map(|p| p[r]);
    let with_path = |mut parents: Vec<Name>, r: usize| {
        parents.extend(path(r));
        parents
    };
    // Sides, by stretch and segment.
    let mut sides: Vec<Vec<Name>> = Vec::with_capacity(rows);
    for (r, row) in at.faces.iter().enumerate() {
        let mut named = Vec::with_capacity(n);
        for (i, &k) in row.iter().enumerate() {
            let face = EntityId::Face(built.faces[k]);
            let parents = with_path(st.sides[i].clone(), r);
            named.push(namer.name(b, face, SweepRole::Side, &parents));
        }
        sides.push(named);
    }
    // The pair either side of chain vertex `i`, in a stretch.
    let pair = |r: usize, i: usize| -> [Name; 2] {
        let ahead = |k: usize| {
            if st.by_sides {
                sides[r][k]
            } else {
                st.sides[k][0]
            }
        };
        let before = if i == 0 { st.ends[0] } else { ahead(i - 1) };
        let after = if i == n { st.ends[1] } else { ahead(i) };
        [before, after]
    };
    if !g.railless() {
        for (s, row) in at.rails.iter().enumerate() {
            for (i, rail) in row.iter().enumerate() {
                let e = EntityId::Edge(built.edges[rail]);
                let (role, parents) = if !g.closed && s == 0 {
                    (SweepRole::CapEdge, vec![st.caps[0], sides[0][i]])
                } else if !g.closed && s == stations - 1 {
                    (SweepRole::CapEdge, vec![st.caps[1], sides[rows - 1][i]])
                } else {
                    let before = (s + rows - 1) % rows;
                    (SweepRole::Section, vec![sides[before][i], sides[s][i]])
                };
                namer.name(b, e, role, &parents);
            }
        }
    }
    for r in 0..rows {
        for i in 0..=n {
            if let Some(e) = at.laterals[r][i] {
                let e = EntityId::Edge(built.edges[&e]);
                namer.name(b, e, SweepRole::Lateral, &with_path(pair(r, i).to_vec(), r));
            }
        }
    }
    let mut named = BTreeSet::new();
    for s in 0..stations {
        for i in 0..=n {
            let Some(v) = at.vertices[s][i] else {
                continue;
            };
            if !named.insert(v) {
                continue;
            }
            let r = s.min(rows - 1);
            let p = pair(r, i).to_vec();
            let fixed = (0..rows).all(|q| at.laterals[q][i].is_none());
            let (role, parents) = if g.railless() {
                match st.paths.as_ref() {
                    _ if fixed => (SweepRole::Apex, p),
                    Some(paths) => (
                        SweepRole::JointVertex,
                        [p, vec![paths[0], paths[0]]].concat(),
                    ),
                    None => (SweepRole::Vertex, p),
                }
            } else if fixed {
                (SweepRole::Vertex, p)
            } else if g.closed || (s > 0 && s < stations - 1) {
                let before = (s + rows - 1) % rows;
                match st.paths.as_ref() {
                    Some(paths) => (
                        SweepRole::JointVertex,
                        [p, vec![paths[before], paths[s]]].concat(),
                    ),
                    None => (SweepRole::Vertex, p),
                }
            } else if s == 0 {
                (SweepRole::StartVertex, p)
            } else {
                (SweepRole::EndVertex, p)
            };
            namer.name(b, EntityId::Vertex(built.vertices[&v]), role, &parents);
        }
    }
}

/// The caps of the solid a surface profile's loops sweep: each lump's
/// start and end caps, by the names every solid sweep gives them.
fn caps(key: OpKey, solid: &Body, profile: &ValidProfile) -> BTreeSet<FaceId> {
    let sweep = |r: SweepRole| Role::Sweep(r);
    let mut out = BTreeSet::new();
    for lump in profile.lumps() {
        let outer = Name::from_u128(lump.outer.key.0);
        let lp = Name::derive(key, sweep(SweepRole::Loop), &[outer]);
        for r in [SweepRole::StartCap, SweepRole::EndCap] {
            if let Some(EntityId::Face(f)) = solid.by_name(Name::derive(key, sweep(r), &[lp])) {
                out.insert(f);
            }
        }
    }
    out
}

/// A surface profile's sheet: the sides of `solid` (the profile's loops
/// swept as a solid with the same key, if it has any) and every chain's
/// grid, built as one body, named, and checked as the sweep's `namer`
/// finishes it.
pub(crate) fn assemble(
    key: OpKey,
    namer: &Namer,
    profile: &ValidProfile,
    solid: Option<&Body>,
    grids: &[Grid],
) -> Result<(Body, Journal), Error> {
    let mut out = Layout::default();
    let copied = solid.map(|s| {
        let lids = caps(key, s, profile);
        let sides: BTreeSet<FaceId> = s.faces().filter(|f| !lids.contains(f)).collect();
        (s, sheet::copy(&mut out, s, &sides))
    });
    let placed: Vec<Placed> = grids.iter().map(|g| place(&mut out, g)).collect();
    let mut built = layout::build_sheet(&out, key, Role::Merged)
        .map_err(|_| Error::Topology(TopologyError::Inconsistent))?;
    let mut b = std::mem::take(&mut built.body);
    b.set_recipes(true);
    if let Some((s, c)) = &copied {
        sheet::name_copied(&mut b, s, c, &built);
    }
    for (g, at) in grids.iter().zip(&placed) {
        name_grid(&mut b, namer, g, at, &built);
    }
    let journal = namer.finish(&mut b)?;
    Ok((b, journal))
}

/// The chain's segment keys, in chain order.
pub(crate) fn segment_keys(c: &ValidChain) -> Vec<u128> {
    c.segments.iter().map(|s| s.key.0).collect()
}
