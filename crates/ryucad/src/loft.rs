//! Loft features: a skin through two or more planar sections (a sketch's
//! closed loops, or a planar face's loops) by ryukei's `sweep::loft`,
//! open with end tangents or closed round to the first section, then
//! founding a solid or applied to others as an extrude's body is.
//!
//! The kernel matches every section's loops to the first's by key and
//! their segments by their order from each loop's start. ryucad builds
//! each section so that order is the correspondence the agent named:
//! - each `loops` entry names a loop and where it starts (a point or a
//!   vertex starts it there; a curve or an edge at its first point along
//!   the loop; a lone circle at its only point), and the `i`th loop of
//!   every section takes the first section's `i`th loop's key;
//! - a section with no `loops` takes its one closed loop; the first
//!   starts at its anchor, each later one at its vertex nearest where the
//!   section before starts once both loops' centroids are brought
//!   together (ties in natural order);
//! - a section whose normal points back along the loft is turned over
//!   (its frame turned a half turn about its `x`), so every loop runs
//!   counterclockwise about the loft's direction from its start.
//!
//! Names come from keys only: each section's segment keys (a sketch's
//! entities, or a face's edges by name; see [`crate::profile`]) and the
//! first section's loop keys, so resizing or moving a section renames
//! nothing.

use crate::doc::geometry::GeomRef;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::sweep::{Loft, Section, Source, Tangent as FileTangent, TangentDirection};
use crate::error::{Code, Error, Result};
use crate::geometry::{At, Geom, Want, resolve};
use crate::profile::{Keys, natural};
use crate::rebuild::{Built, Results, Run, op_key};
use ryukei::geom::{Frame, ParamFlip, Point2, Point3, Vector3};
use ryukei::name::{Name, OpKey, Role, SweepRole as R};
use ryukei::sweep::{
    Chain, ChainKey, LoftOptions, Loop, LoopKey, Profile, Segment, Tangent, ValidChain, ValidLoop,
    ValidProfile,
};
use ryukei::tol::LINEAR_RESOLUTION;
use ryukei::{Error as Kernel, LoftError};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One loop of a section as the loft takes it.
#[derive(Clone, Debug)]
pub struct SectionLoop {
    /// What names it: its anchor (a sketch's point or curve, a face's
    /// vertex), whose key the first section's loop takes.
    pub anchor: Id,
    /// Where it starts: a point or vertex, or for a lone circle the
    /// circle.
    pub start: Id,
    /// Its curves or edges from its start, in order.
    pub entities: Vec<Id>,
    /// Its start and its centroid, in model space.
    start_at: Point3,
    centroid: Point3,
}

/// A section's open chain as the loft matches it (ryucad 18): the first
/// section's key, its own, its curves in the order it runs, where it
/// starts and its middle, and whether it runs against the way it was
/// drawn.
#[derive(Clone, Debug)]
pub struct SectionChain {
    pub key: u128,
    pub entities: Vec<Id>,
    start_at: Point3,
    centroid: Point3,
    pub reversed: bool,
}

/// A section, resolved: the kernel's profile (its loops in the order the
/// loft matches them, each from its start, its frame facing along the
/// loft), which entity each key came from, how origins name its source
/// (`{"sketch": id}` or `{"face": {feature, name}}`) and its entities
/// (`entity` or `edge`), its loops, and whether it was turned over.
pub struct Resolved {
    pub valid: ValidProfile,
    pub keys: Keys,
    pub source: Value,
    pub entity: &'static str,
    pub loops: Vec<SectionLoop>,
    /// Its open chains, in the order the loft matches them.
    pub chains: Vec<SectionChain>,
    pub turned: bool,
    /// The mean of its loops' and chains' centroids, in model space.
    centroid: Point3,
}

/// A section's loops before the loft orders them: the profile its
/// source gives, its keys, the entries its `loops` names (each a loop's
/// index among the profile's loops, and where it starts if the entry
/// says), and its source as origins name it.
struct Raw {
    valid: ValidProfile,
    keys: Keys,
    source: Value,
    entity: &'static str,
    /// Each chosen loop: its key, and its start's entity if named.
    chosen: Vec<(u128, Option<Start>)>,
    /// Each chosen open chain's key (a surface loft's).
    chains: Vec<u128>,
}

/// Where an entry starts its loop.
#[derive(Clone, Debug)]
enum Start {
    /// At this point or vertex.
    Point(Id),
    /// At this curve's or edge's first point along the loop.
    Curve(Id),
}

fn loops_of(v: &ValidProfile) -> impl Iterator<Item = &ValidLoop> {
    v.lumps()
        .iter()
        .flat_map(|l| std::iter::once(&l.outer).chain(&l.holes))
}

/// `section k` in a refusal's details: its index, and what it is.
fn which(k: usize, s: &Section) -> Value {
    let mut v = s.to_json();
    if let Some(o) = v.as_object_mut() {
        o.remove("loops");
    }
    v["index"] = json!(k);
    v
}

fn failed(id: &Id, reason: &str, k: Option<(usize, &Section)>, msg: String) -> Error {
    let mut e = Error::new(Code::LoftFailed, format!("loft {id}: {msg}"))
        .with("feature", id)
        .with("reason", reason);
    if let Some((k, s)) = k {
        e = e
            .with("section", which(k, s))
            .with("pointer", format!("/sections/{k}"));
    }
    e
}

/// Section `k` of loft `id` before it is ordered: its source's profile
/// narrowed to the loops its entries name.
fn raw(
    part: &Part,
    id: &Id,
    k: usize,
    s: &Section,
    before: &dyn Results,
    surface: bool,
) -> Result<Raw> {
    let at = format!("/sections/{k}/loops");
    let bad = |i: usize, msg: String| {
        Error::new(Code::InvalidParam, format!("loft {id}: section {k}: {msg}"))
            .with("feature", id)
            .with("pointer", format!("{at}/{i}"))
    };
    let (valid, keys, source, entity, curves) = match &s.source {
        Source::Sketch(sketch) => {
            let solved = crate::rebuild::sketch_input(part, id, sketch, before)?;
            // A point narrows to a loop through a curve that ends there.
            let mut narrow = Vec::new();
            let mut curves = Vec::new();
            for (i, e) in s.loops.iter().enumerate() {
                use crate::doc::sketch::Geometry as G;
                let Some(entity) = solved.sketch.entities.get(e) else {
                    return Err(bad(i, format!("sketch {sketch} has no {e}")));
                };
                match &entity.geometry {
                    G::Point { .. } => {
                        let through = solved.report.loops.iter().flatten().find(|(c, _)| {
                            matches!(&solved.sketch.entities[c].geometry,
                                G::Line { start, end } | G::Arc { start, end, .. }
                                    if start == e || end == e)
                        });
                        // A surface loft's point may name an open chain.
                        let on_chain = || {
                            solved.report.open.iter().flat_map(|(es, _)| es).find(|c| {
                                matches!(&solved.sketch.entities[*c].geometry,
                                    G::Line { start, end } | G::Arc { start, end, .. }
                                        if start == e || end == e)
                            })
                        };
                        let Some(c) = through
                            .map(|(c, _)| c)
                            .or_else(|| if surface { on_chain() } else { None })
                        else {
                            return Err(bad(i, format!("{e} is on no closed loop of {sketch}")));
                        };
                        narrow.push(c.clone());
                    }
                    _ => {
                        narrow.push(e.clone());
                        curves.push(e.clone());
                    }
                }
            }
            let of = if surface {
                crate::profile::surface
            } else {
                crate::profile::profile
            };
            let (valid, keys) = of(&part.id, sketch, solved, &narrow)
                .map_err(|e| e.with("feature", id).with("sketch", sketch))?;
            (valid, keys, json!({"sketch": sketch}), "entity", curves)
        }
        Source::Face(g) => {
            let GeomRef::Entity(r) = g else {
                unreachable!("a face section is an entity, as read")
            };
            let role = format!("sections.{k}");
            let (valid, keys) = crate::profile::of_face(
                part,
                part.order(),
                id,
                &role,
                r,
                &s.loops,
                before,
                surface,
            )?;
            let curves = s
                .loops
                .iter()
                .filter(|e| keys.segments.values().any(|x| x == *e))
                .cloned()
                .collect();
            (valid, keys, json!({"face": g.to_json()}), "edge", curves)
        }
    };
    // Each entry naming a chain chooses it: a chain has no start to name.
    let holds = |c: &ValidChain, e: &Id| {
        c.segments
            .iter()
            .any(|g| keys.segments.get(&g.key.0) == Some(e))
            || keys.points.get(e).is_some_and(|p| {
                c.vertices
                    .iter()
                    .any(|v| v.distance(*p) <= LINEAR_RESOLUTION)
            })
    };
    let mut chains: Vec<u128> = Vec::new();
    for (i, e) in s.loops.iter().enumerate() {
        if let Some(c) = valid.chains().iter().find(|c| holds(c, e)) {
            if chains.contains(&c.key.0) {
                return Err(bad(i, format!("{e} names a chain an earlier entry names")));
            }
            chains.push(c.key.0);
        }
    }
    // Each entry's loop: the loop holding its curve, or its point.
    let mut chosen: Vec<(u128, Option<Start>)> = Vec::new();
    for (i, e) in s.loops.iter().enumerate() {
        if valid.chains().iter().any(|c| holds(c, e)) {
            continue;
        }
        let (key, start) = if curves.contains(e) {
            let l = loops_of(&valid)
                .find(|l| {
                    l.segments
                        .iter()
                        .any(|g| keys.segments.get(&g.key.0) == Some(e))
                })
                .expect("a chosen curve's loop is in the profile");
            (l.key.0, Start::Curve(e.clone()))
        } else {
            let p = keys.points[e];
            let holding: Vec<&ValidLoop> = loops_of(&valid)
                .filter(|l| {
                    l.vertices
                        .iter()
                        .any(|v| v.distance(p) <= LINEAR_RESOLUTION)
                })
                .collect();
            if holding.len() > 1 {
                let both: Vec<&Id> = holding.iter().map(|l| &keys.loops[&l.key.0]).collect();
                return Err(bad(
                    i,
                    format!("{e} is on two loops: name a curve of the one meant"),
                )
                .with("loops", both));
            }
            (holding[0].key.0, Start::Point(e.clone()))
        };
        if chosen.iter().any(|(c, _)| *c == key) {
            return Err(bad(i, format!("{e} names a loop an earlier entry names")));
        }
        chosen.push((key, Some(start)));
    }
    if s.loops.is_empty() {
        let all: Vec<&ValidLoop> = loops_of(&valid).collect();
        let open = valid.chains();
        if all.len() + open.len() != 1 {
            let mut anchors: Vec<&Id> = all.iter().map(|l| &keys.loops[&l.key.0]).collect();
            anchors.extend(open.iter().map(|c| &keys.chains[&c.key.0]));
            let what = if surface {
                "loops and open chains"
            } else {
                "loops"
            };
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "loft {id}: section {k} has {} {what}: name each with loops, in the order the sections match them",
                    all.len() + open.len()
                ),
            )
            .with("feature", id)
            .with("pointer", format!("/sections/{k}/loops"))
            .with("loops", anchors));
        }
        match all.first() {
            Some(l) => chosen.push((l.key.0, None)),
            None => chains.push(open[0].key.0),
        }
    }
    Ok(Raw {
        valid,
        keys,
        source,
        entity,
        chosen,
        chains,
    })
}

/// A point of a profile's plane in model space.
fn world(frame: &Frame, p: Point2) -> Point3 {
    frame.to_world(p.x, p.y, 0.0)
}

fn mean(points: impl Iterator<Item = Point3>) -> Point3 {
    let (mut sum, mut n) = (Vector3::ZERO, 0.0);
    for p in points {
        sum += p.to_vector();
        n += 1.0;
    }
    Point3::ORIGIN + sum / n
}

/// Where a loop's middle is, in model space: the mean of eight points
/// along each of its curves, evenly by parameter (a circle's centre, a
/// square's middle), so a loop's centroid doesn't lean towards where its
/// vertices fall.
fn centroid_of(frame: &Frame, l: &ValidLoop) -> Point3 {
    const SAMPLES: usize = 8;
    mean(l.segments.iter().flat_map(|g| {
        (0..SAMPLES).map(move |i| {
            let t = g.t0 + (g.t1 - g.t0) * i as f64 / SAMPLES as f64;
            world(frame, g.curve.point(t))
        })
    }))
}

/// Where a chain's middle is, in model space: the mean of eight points
/// along each of its curves, evenly by parameter.
fn chain_centroid(frame: &Frame, c: &ValidChain) -> Point3 {
    const SAMPLES: usize = 8;
    mean(c.segments.iter().flat_map(|g| {
        (0..=SAMPLES).map(move |i| {
            let t = g.t0 + (g.t1 - g.t0) * i as f64 / SAMPLES as f64;
            world(frame, g.curve.point(t))
        })
    }))
}

fn chain_by_key(v: &ValidProfile, key: u128) -> &ValidChain {
    v.chains()
        .iter()
        .find(|c| c.key.0 == key)
        .expect("a chosen chain is in the profile")
}

fn loop_by_key(v: &ValidProfile, key: u128) -> &ValidLoop {
    loops_of(v)
        .find(|l| l.key.0 == key)
        .expect("a chosen loop is in the profile")
}

/// The profile of `loops` and `chains` in `frame`, each loop's segments
/// rotated to start at `starts[i]` and keyed `keys[i]`, each chain run
/// backwards where asked and keyed, all mirrored across the plane's `x`
/// when `mirror`; a surface profile when `surface`.
fn rebuilt(
    frame: Frame,
    loops: &[(&ValidLoop, usize, u128)],
    chains: &[(&ValidChain, bool, u128)],
    mirror: bool,
    surface: bool,
) -> Result<ValidProfile> {
    let curve = |g: &Segment| {
        if mirror {
            g.curve.mirrored(ParamFlip::V(0.0))
        } else {
            g.curve.clone()
        }
    };
    let chains: Vec<Chain> = chains
        .iter()
        .map(|(c, reversed, key)| {
            let mut segments: Vec<Segment> = c
                .segments
                .iter()
                .map(|g| Segment {
                    key: g.key,
                    curve: curve(g),
                    t0: g.t0,
                    t1: g.t1,
                })
                .collect();
            if *reversed {
                segments.reverse();
                for g in &mut segments {
                    core::mem::swap(&mut g.t0, &mut g.t1);
                }
            }
            Chain {
                key: ChainKey(*key),
                segments,
            }
        })
        .collect();
    let loops = loops
        .iter()
        .map(|(l, start, key)| {
            let n = l.segments.len();
            let segments = (0..n)
                .map(|i| {
                    let g = &l.segments[(start + i) % n];
                    Segment {
                        key: g.key,
                        curve: if mirror {
                            g.curve.mirrored(ParamFlip::V(0.0))
                        } else {
                            g.curve.clone()
                        },
                        t0: g.t0,
                        t1: g.t1,
                    }
                })
                .collect();
            Loop {
                key: LoopKey(*key),
                segments,
            }
        })
        .collect();
    let profile = Profile { frame, loops };
    let valid = if surface {
        ValidProfile::surface(profile, chains)
    } else {
        ValidProfile::new(profile)
    };
    valid.map_err(|e| {
        Error::new(Code::Internal, "a section that was valid isn't")
            .with("kernel", format!("{e:?}"))
    })
}

/// The section's vertex `v` names: its point's place, as `keys` has it.
fn named_vertex(l: &ValidLoop, at: Point2) -> Option<usize> {
    l.vertices
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.distance(at)))
        .filter(|(_, d)| *d <= 1e-9)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The entity at a loop's vertex `i`: the point there, or for a lone
/// closed curve the curve itself.
fn vertex_id(l: &ValidLoop, i: usize, keys: &Keys, mirror: bool) -> Id {
    let at = l.vertices[i];
    let at = if mirror { Point2::new(at.x, -at.y) } else { at };
    keys.points
        .iter()
        .filter(|(_, p)| p.distance(at) <= 1e-9)
        .min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)))
        .map_or_else(
            || keys.segments[&l.segments[i].key.0].clone(),
            |(n, _)| n.clone(),
        )
}

/// Every section of loft `id`, resolved in order: loops chosen and
/// matched, each section turned to face along the loft, each loop from
/// its start. Refusals are `invalid_param` (a loop an entry can't name
/// alone, a section of several loops with none named) and `loft_failed`
/// (`loop_mismatch`, `segment_count_mismatch`, `bad_section` for one
/// edge-on to the loft), naming the
/// section.
pub fn sections(part: &Part, id: &Id, l: &Loft, before: &dyn Results) -> Result<Vec<Resolved>> {
    let raws = l
        .sections
        .iter()
        .enumerate()
        .map(|(k, s)| raw(part, id, k, s, before, l.surface))
        .collect::<Result<Vec<_>>>()?;
    let m = raws.len();
    // Loop counts first: the correspondence is by place.
    let want = raws[0].chosen.len();
    for (k, r) in raws.iter().enumerate().skip(1) {
        if r.chosen.len() != want {
            return Err(failed(
                id,
                "loop_mismatch",
                Some((k, &l.sections[k])),
                format!(
                    "section {k} has {} loops and section 0 has {want}: name as many in each section's loops, in the order they match",
                    r.chosen.len()
                ),
            )
            .with("counts", json!({"first": want, "this": r.chosen.len()})));
        }
    }
    let chains = raws[0].chains.len();
    for (k, r) in raws.iter().enumerate().skip(1) {
        if r.chains.len() != chains {
            return Err(failed(
                id,
                "loop_mismatch",
                Some((k, &l.sections[k])),
                format!(
                    "section {k} has {} open chains and section 0 has {chains}: name as many in each section's loops, in the order they match",
                    r.chains.len()
                ),
            )
            .with("counts", json!({"first": chains, "this": r.chains.len()})));
        }
    }
    // A section's centroid: its loops' and chains' centroids' mean.
    let centroids: Vec<Point3> = raws
        .iter()
        .map(|r| {
            let frame = r.valid.frame();
            let loops = r
                .chosen
                .iter()
                .map(|(key, _)| centroid_of(frame, loop_by_key(&r.valid, *key)));
            let open = r
                .chains
                .iter()
                .map(|key| chain_centroid(frame, chain_by_key(&r.valid, *key)));
            mean(loops.chain(open))
        })
        .collect();
    // Which way the loft runs at each section.
    let along = |k: usize| -> Vector3 {
        if k + 1 < m {
            centroids[k + 1] - centroids[k]
        } else if l.closed {
            centroids[0] - centroids[k]
        } else {
            centroids[k] - centroids[k - 1]
        }
    };
    let mut out: Vec<Resolved> = Vec::with_capacity(m);
    for (k, r) in raws.into_iter().enumerate() {
        let frame = *r.valid.frame();
        // Edge-on: the plane holds the line to the neighbour, within the
        // angular resolution (centroids are sums: an exact zero rounds).
        let line = along(k);
        let facing = frame.z().get().dot(line);
        if facing.abs() <= ryukei::tol::ANGULAR_RESOLUTION * line.length() {
            return Err(failed(
                id,
                "bad_section",
                Some((k, &l.sections[k])),
                format!(
                    "section {k} lies edge-on to the loft: its plane holds the line to its neighbour"
                ),
            ));
        }
        let turned = facing < 0.0;
        // Turned over, the plane's y and z go the other way and its
        // curves are mirrored across its x, so nothing moves.
        let (frame1, valid1) = if turned {
            let f = Frame::from_axes(frame.origin(), -frame.z(), frame.x()).map_err(|e| {
                Error::new(Code::Internal, "a turned frame").with("kernel", format!("{e:?}"))
            })?;
            let all: Vec<(&ValidLoop, usize, u128)> =
                loops_of(&r.valid).map(|x| (x, 0, x.key.0)).collect();
            let open: Vec<(&ValidChain, bool, u128)> = r
                .valid
                .chains()
                .iter()
                .map(|c| (c, false, c.key.0))
                .collect();
            (f, rebuilt(f, &all, &open, true, l.surface)?)
        } else {
            (frame, r.valid.clone())
        };
        let point = |id: &Id| {
            let p = r.keys.points[id];
            if turned { Point2::new(p.x, -p.y) } else { p }
        };
        let mut chosen = Vec::new();
        let mut loops = Vec::new();
        for (i, (key, start)) in r.chosen.iter().enumerate() {
            let lp = loop_by_key(&valid1, *key);
            let start_at = match start {
                Some(Start::Curve(e)) => lp
                    .segments
                    .iter()
                    .position(|g| r.keys.segments.get(&g.key.0) == Some(e))
                    .expect("a chosen curve is on its loop"),
                Some(Start::Point(p)) => {
                    named_vertex(lp, point(p)).expect("a chosen point is on its loop")
                }
                None if k == 0 => {
                    let anchor = &r.keys.loops[key];
                    r.keys
                        .points
                        .get(anchor)
                        .and_then(|_| named_vertex(lp, point(anchor)))
                        .unwrap_or(0)
                }
                None => {
                    // Nearest where the section before starts, once both
                    // loops' centroids are brought together.
                    let prev: &SectionLoop = &out[k - 1].loops[i];
                    let c = centroid_of(&frame1, lp);
                    let aim = prev.start_at - prev.centroid;
                    let ids: Vec<Id> = (0..lp.vertices.len())
                        .map(|j| vertex_id(lp, j, &r.keys, turned))
                        .collect();
                    (0..lp.vertices.len())
                        .min_by(|&a, &b| {
                            let d = |j: usize| (world(&frame1, lp.vertices[j]) - c - aim).length();
                            d(a).total_cmp(&d(b))
                                .then_with(|| natural(&ids[a]).cmp(&natural(&ids[b])))
                        })
                        .expect("a loop has vertices")
                }
            };
            // The first section's loops name every section's.
            let lk = if k == 0 { *key } else { out[0].valid_key(i) };
            chosen.push((lp, start_at, lk));
            let n = lp.segments.len();
            loops.push(SectionLoop {
                anchor: r.keys.loops[key].clone(),
                start: vertex_id(lp, start_at, &r.keys, turned),
                entities: (0..n)
                    .map(|j| r.keys.segments[&lp.segments[(start_at + j) % n].key.0].clone())
                    .collect(),
                start_at: world(&frame1, lp.vertices[start_at]),
                centroid: centroid_of(&frame1, lp),
            });
        }
        // Each chain: the first section's runs as it was drawn; each
        // later one from the end nearer where the section before's starts,
        // once both chains' centroids are brought together.
        let mut chain_list = Vec::new();
        let mut chains_out = Vec::new();
        for (i, key) in r.chains.iter().enumerate() {
            let c = chain_by_key(&valid1, *key);
            let pts: Vec<Point3> = c.vertices.iter().map(|v| world(&frame1, *v)).collect();
            let centroid = chain_centroid(&frame1, c);
            let reversed = if k == 0 {
                false
            } else {
                let prev: &SectionChain = &out[k - 1].chains[i];
                let aim = centroid + (prev.start_at - prev.centroid);
                let (first, last) = (pts[0], pts[pts.len() - 1]);
                (last - aim).length() < (first - aim).length()
            };
            let ck = if k == 0 { *key } else { out[0].chains[i].key };
            let n = c.segments.len();
            let order: Vec<usize> = if reversed {
                (0..n).rev().collect()
            } else {
                (0..n).collect()
            };
            chain_list.push((c, reversed, ck));
            chains_out.push(SectionChain {
                key: ck,
                entities: order
                    .iter()
                    .map(|&j| r.keys.segments[&c.segments[j].key.0].clone())
                    .collect(),
                start_at: if reversed { pts[pts.len() - 1] } else { pts[0] },
                centroid,
                reversed,
            });
        }
        let valid = rebuilt(frame1, &chosen, &chain_list, false, l.surface)?;
        out.push(Resolved {
            valid,
            keys: r.keys,
            source: r.source,
            entity: r.entity,
            loops,
            chains: chains_out,
            turned,
            centroid: centroids[k],
        });
    }
    // Matched loops need as many segments.
    for (k, s) in out.iter().enumerate().skip(1) {
        for (i, lp) in s.loops.iter().enumerate() {
            let first = out[0].loops[i].entities.len();
            if lp.entities.len() != first {
                return Err(failed(
                    id,
                    "segment_count_mismatch",
                    Some((k, &l.sections[k])),
                    format!(
                        "section {k}'s loop {} has {} curves and section 0's loop {} has {first}: split curves in the sketch so they match (a circle lofted to a square: split the circle into four arcs)",
                        lp.anchor,
                        lp.entities.len(),
                        out[0].loops[i].anchor
                    ),
                )
                .with("entities", &lp.entities)
                .with("counts", json!({"first": first, "this": lp.entities.len()})));
            }
        }
        for (i, ch) in s.chains.iter().enumerate() {
            let first = out[0].chains[i].entities.len();
            if ch.entities.len() != first {
                return Err(failed(
                    id,
                    "segment_count_mismatch",
                    Some((k, &l.sections[k])),
                    format!(
                        "section {k}'s open chain {} has {} curves and section 0's {first}: split curves in the sketch so they match",
                        ch.entities[0],
                        ch.entities.len(),
                    ),
                )
                .with("entities", &ch.entities)
                .with("counts", json!({"first": first, "this": ch.entities.len()})));
            }
        }
    }
    Ok(out)
}

impl SectionLoop {
    /// Where it starts, in model space.
    pub fn start_at(&self) -> Point3 {
        self.start_at
    }
}

impl Resolved {
    /// The mean of its loops' centroids, in model space.
    pub fn centroid(&self) -> Point3 {
        self.centroid
    }

    /// Its loops as the kernel takes them, in the order the loft matches
    /// them, each from its start; `first` is the loft's first section,
    /// whose keys every section's loops take.
    pub fn kernel_loops<'a>(&'a self, first: &Resolved) -> Vec<&'a ValidLoop> {
        (0..self.loops.len())
            .map(|i| loop_by_key(&self.valid, first.valid_key(i)))
            .collect()
    }

    /// The key of its `i`th loop as the kernel has it.
    fn valid_key(&self, i: usize) -> u128 {
        let anchor = &self.loops[i].anchor;
        loops_of(&self.valid)
            .find(|l| &self.keys.loops[&l.key.0] == anchor)
            .expect("its loops are its own")
            .key
            .0
    }
}

/// The kernel's tangent for an end of loft `id`: its direction (the end
/// section's normal facing along the loft, or a reference's direction
/// made to face along it) and its weight times the straight distance
/// between the first and last sections' centroids.
fn tangent(at: &At, t: &FileTangent, role: &str, end: &Resolved, span: f64) -> Result<Tangent> {
    let z = end.valid.frame().z();
    let direction = match &t.direction {
        TangentDirection::Normal => z,
        TangentDirection::Ref(g) => {
            let v = match resolve(at, role, g, &[Want::Axis, Want::Plane])? {
                Geom::Axis(l) => l.dir.get(),
                Geom::Plane(p) => p.normal,
                _ => unreachable!("asked for an axis or a plane"),
            };
            let u = v.normalize().ok_or_else(|| {
                at.fail(
                    Code::ReferenceWrongKind,
                    role,
                    g,
                    "gives no direction".to_owned(),
                )
            })?;
            if u.dot(z) < 0.0 { -u } else { u }
        }
    };
    Ok(Tangent {
        direction,
        weight: t.weight_number() * span,
    })
}

/// The kernel's options for loft `id` over its resolved sections.
fn options(at: &At, l: &Loft, s: &[Resolved]) -> Result<LoftOptions> {
    let span = (s[s.len() - 1].centroid - s[0].centroid).length();
    let end = |t: &Option<FileTangent>, role: &str, r: &Resolved| {
        t.as_ref()
            .map(|t| tangent(at, t, role, r, span))
            .transpose()
    };
    Ok(LoftOptions {
        closed: l.closed,
        start: end(&l.start_tangent, "start_tangent", &s[0])?,
        end: end(&l.end_tangent, "end_tangent", &s[s.len() - 1])?,
    })
}

/// A loft's own body: its sections skinned, under the request's cancel.
pub fn build(part: &Part, id: &Id, l: &Loft, before: &dyn Results, run: &Run) -> Result<Built> {
    let at = At {
        part,
        order: part.order(),
        owner: id,
        results: before,
    };
    let s = sections(part, id, l, before)?;
    let opts = options(&at, l, &s)?;
    let key = op_key(&part.id, id);
    let valid: Vec<ValidProfile> = s.iter().map(|r| r.valid.clone()).collect();
    let origins = origins(key, &s, l.closed);
    let work = crate::work::work(run.job);
    let made =
        ryukei::sweep::loft_with(key, &valid, &opts, &work).map_err(
            |e| match crate::work::stopped(&e, run.job) {
                Some(stop) => stop,
                None => refused(&part.id, id, e, l, &s, &origins),
            },
        )?;
    let mut origins = origins;
    for o in origins.values_mut() {
        o["feature"] = json!(id);
    }
    crate::rebuild::finish(id, "loft", made, origins)
}

/// A kernel refusal's `reason`, in snake case.
fn reason(e: &LoftError) -> &'static str {
    match e {
        LoftError::TooFewSections => "too_few_sections",
        LoftError::LoopMismatch { .. } => "loop_mismatch",
        LoftError::SegmentCountMismatch { .. } => "segment_count_mismatch",
        LoftError::BadSection { .. } => "bad_section",
        LoftError::BadTangent => "bad_tangent",
        LoftError::BadWeights { .. } => "bad_weights",
        LoftError::SelfIntersecting { .. } => "self_intersecting",
        LoftError::Uncertain { .. } => "uncertain",
    }
}

/// What a refusal means, for people.
fn describe(e: &LoftError) -> &'static str {
    match e {
        LoftError::TooFewSections => "it needs two sections, or three round a ring",
        LoftError::LoopMismatch { .. } => "a section's loops don't match the first's",
        LoftError::SegmentCountMismatch { .. } => {
            "a section's loop has another number of curves than the first's"
        }
        LoftError::BadSection { .. } => {
            "a section lies in the plane of the one before it, or turns a right angle or more from it: add sections between"
        }
        LoftError::BadTangent => "an end tangent lies in its section's plane",
        LoftError::BadWeights { .. } => {
            "the skin through these curves can't be made: its weights fall to zero between sections"
        }
        LoftError::SelfIntersecting { .. } => "the lofted faces meet each other",
        LoftError::Uncertain { .. } => "where two lofted faces meet couldn't be certified",
    }
}

/// The kernel's refusal of loft `id`, in its own terms: `loft_failed`
/// with its `reason` and what it names (the section, the first
/// section's curve whose skin can't be made, the two faces that meet).
fn refused(
    part: &Id,
    id: &Id,
    e: Kernel,
    l: &Loft,
    s: &[Resolved],
    origins: &BTreeMap<String, Value>,
) -> Error {
    let kernel = format!("{e:?}");
    let Kernel::InvalidLoft(x) = e else {
        return Error::new(Code::RebuildFailed, format!("loft {id}: {e}"))
            .with("feature", id)
            .with("kernel", kernel);
    };
    let section = match x {
        LoftError::LoopMismatch { section, .. }
        | LoftError::SegmentCountMismatch { section, .. }
        | LoftError::BadSection { section } => Some((section, &l.sections[section])),
        _ => None,
    };
    let mut err = failed(id, reason(&x), section, describe(&x).to_owned()).with("kernel", kernel);
    match x {
        LoftError::BadWeights { segment, .. } => {
            if let Some(e) = s[0].keys.segments.get(&segment.0) {
                err = err.with("entities", [e]);
            }
        }
        LoftError::SelfIntersecting { faces } | LoftError::Uncertain { faces } => {
            let faces: Vec<Value> = faces
                .iter()
                .map(|n| {
                    let mut v = json!({"reference": format!("{part}/{id}/{n}")});
                    if let Some(o) = origins.get(&n.to_string()) {
                        let mut o = o.clone();
                        o["feature"] = json!(id);
                        v["origin"] = o;
                    }
                    v
                })
                .collect();
            err = err.with("faces", faces);
        }
        _ => {}
    }
    err
}

/// Where each entity of a loft came from, by the hex of its stable name,
/// derived from the keys as the kernel names them: a side from its loop
/// and its curve in every section (`sections: [{sketch, entity} | {face,
/// edge}]`); a lateral and its vertices from the curves either side of
/// it in every section (`sections: [{…, entities}]`); caps from their
/// loop and end, cap edges from their side and end. Entities a loft
/// doesn't make simply have no origin.
pub fn origins(key: OpKey, s: &[Resolved], closed: bool) -> BTreeMap<String, Value> {
    let derive = |role: R, parents: &[Name]| Name::derive(key, Role::Sweep(role), parents);
    let mut out = BTreeMap::new();
    let first = &s[0];
    for lump in first.valid.lumps() {
        let stand = derive(R::Loop, &[Name::from_u128(lump.outer.key.0)]);
        let anchor = &first.keys.loops[&lump.outer.key.0];
        let caps = [
            ("start", derive(R::StartCap, &[stand])),
            ("end", derive(R::EndCap, &[stand])),
        ];
        if !closed {
            for (end, cap) in caps {
                out.insert(cap.to_string(), json!({"cap": end, "loop": anchor}));
            }
        }
        for lp in std::iter::once(&lump.outer).chain(&lump.holes) {
            let i = first
                .loops
                .iter()
                .position(|x| x.anchor == first.keys.loops[&lp.key.0])
                .expect("a loop of the first section");
            let lstand = derive(R::Loop, &[Name::from_u128(lp.key.0)]);
            let n = lp.segments.len();
            // Each section's segment `j` of this loop, as the kernel
            // has it.
            let seg = |r: &Resolved, j: usize| -> (u128, Id) {
                let mine = loops_of(&r.valid)
                    .find(|x| x.key.0 == lp.key.0)
                    .expect("matched by key");
                let k = mine.segments[j].key.0;
                (k, r.keys.segments[&k].clone())
            };
            let side_of = |j: usize| -> (Name, Value) {
                let mut parents = vec![lstand];
                let mut from = Vec::new();
                for r in s {
                    let (k, e) = seg(r, j);
                    parents.push(derive(R::Segment, &[Name::from_u128(k)]));
                    let mut v = r.source.clone();
                    v[r.entity] = json!(e);
                    from.push(v);
                }
                (derive(R::Side, &parents), json!(from))
            };
            let sides: Vec<(Name, Value)> = (0..n).map(side_of).collect();
            let loop_anchor = &first.loops[i].anchor;
            for (side, from) in &sides {
                out.insert(
                    side.to_string(),
                    json!({"loop": loop_anchor, "sections": from}),
                );
                if !closed {
                    for (end, cap) in caps {
                        out.insert(
                            derive(R::CapEdge, &[cap, *side]).to_string(),
                            json!({"loop": loop_anchor, "sections": from, "cap": end}),
                        );
                    }
                }
            }
            // The vertex between curves j - 1 and j, either way round (a
            // hole's paths run the other way).
            for j in 0..n {
                let a = (j + n - 1) % n;
                let from: Vec<Value> = s
                    .iter()
                    .map(|r| {
                        let mut v = r.source.clone();
                        v["entities"] = json!([seg(r, a).1, seg(r, j).1]);
                        v
                    })
                    .collect();
                for pair in [[sides[a].0, sides[j].0], [sides[j].0, sides[a].0]] {
                    let mut put = |role: R, extra: Option<(&str, &str)>| {
                        let mut v = json!({"loop": loop_anchor, "sections": from});
                        if let Some((k, x)) = extra {
                            v[k] = json!(x);
                        }
                        out.insert(derive(role, &pair).to_string(), v);
                    };
                    put(R::Lateral, None);
                    if closed {
                        put(R::Vertex, None);
                    } else {
                        put(R::StartVertex, Some(("cap", "start")));
                        put(R::EndVertex, Some(("cap", "end")));
                    }
                }
            }
        }
    }
    // Open chains (ryucad 18), named as `ryukei::sweep::sheets` names a
    // lofted chain: a side from the chain and its curve in every section;
    // laterals and vertices from the sides either side (the chain's ends
    // by `ChainEnd` stand-ins); rails as cap edges of the chain's caps.
    for (i, c) in first.chains.iter().enumerate() {
        let ch = derive(R::Loop, &[Name::from_u128(c.key)]);
        let least = &first.keys.chains[&c.key];
        let ends = [0u128, 1].map(|e| derive(R::ChainEnd, &[ch, Name::from_u128(e)]));
        let caps = [
            ("start", derive(R::StartCap, &[ch])),
            ("end", derive(R::EndCap, &[ch])),
        ];
        let n = c.entities.len();
        // Each section's curve `j` of this chain, as the kernel has it.
        let seg = |r: &Resolved, j: usize| -> (u128, Id) {
            let mine = r
                .valid
                .chains()
                .iter()
                .find(|x| x.key.0 == r.chains[i].key)
                .expect("matched by key");
            let k = mine.segments[j].key.0;
            (k, r.keys.segments[&k].clone())
        };
        let sides: Vec<(Name, Value)> = (0..n)
            .map(|j| {
                let mut parents = vec![ch];
                let mut from = Vec::new();
                for r in s {
                    let (k, e) = seg(r, j);
                    parents.push(derive(R::Segment, &[Name::from_u128(k)]));
                    let mut v = r.source.clone();
                    v[r.entity] = json!(e);
                    from.push(v);
                }
                (derive(R::Side, &parents), json!(from))
            })
            .collect();
        for (side, from) in &sides {
            out.insert(side.to_string(), json!({"chain": least, "sections": from}));
            if !closed {
                for (end, cap) in caps {
                    out.insert(
                        derive(R::CapEdge, &[cap, *side]).to_string(),
                        json!({"chain": least, "sections": from, "cap": end}),
                    );
                }
            }
        }
        for j in 0..=n {
            let before = if j == 0 { ends[0] } else { sides[j - 1].0 };
            let after = if j == n { ends[1] } else { sides[j].0 };
            let from: Vec<Value> = s
                .iter()
                .map(|r| {
                    let mut v = r.source.clone();
                    match j {
                        0 => {
                            v[r.entity] = json!(seg(r, 0).1);
                            v["chain_end"] = json!(0);
                        }
                        j if j == n => {
                            v[r.entity] = json!(seg(r, n - 1).1);
                            v["chain_end"] = json!(1);
                        }
                        j => v["entities"] = json!([seg(r, j - 1).1, seg(r, j).1]),
                    }
                    v
                })
                .collect();
            let mut put = |role: R, extra: Option<(&str, &str)>| {
                let mut v = json!({"chain": least, "sections": from});
                if let Some((k, x)) = extra {
                    v[k] = json!(x);
                }
                out.insert(derive(role, &[before, after]).to_string(), v);
            };
            put(R::Lateral, None);
            put(R::Vertex, None);
            put(R::StartVertex, Some(("cap", "start")));
            put(R::EndVertex, Some(("cap", "end")));
        }
    }
    out
}

/// Refuses loft `id` whose sections don't resolve now: each section's
/// sketch listed before it with the entities its loops name, or its
/// planar face; as many loops in every section, matched loops with as
/// many curves, no section edge-on; each tangent's direction an axis or
/// a plane. Whether the kernel can skin it only rebuild tells.
pub fn check(part: &Part, order: &[Id], id: &Id, l: &Loft, results: &dyn Results) -> Result<()> {
    let at = At {
        part,
        order,
        owner: id,
        results,
    };
    for (k, s) in l.sections.iter().enumerate() {
        if let Source::Face(GeomRef::Entity(r)) = &s.source {
            let role = format!("sections.{k}");
            let found = crate::reference::resolve(part, order, id, &role, r, results)?;
            crate::reference::plane(part, id, &role, r, found)?;
        }
    }
    for (role, t) in [
        ("start_tangent", &l.start_tangent),
        ("end_tangent", &l.end_tangent),
    ] {
        if let Some(FileTangent {
            direction: TangentDirection::Ref(g),
            ..
        }) = t
        {
            resolve(&at, role, g, &[Want::Axis, Want::Plane])?;
        }
    }
    match sections(part, id, l, results) {
        // A section whose sketch fails now is the rebuild's to report.
        Err(e) if e.code == Code::DependencyFailed => Ok(()),
        r => r.map(|_| ()),
    }
}

/// A loft as rebuild reports and `inspect --feature` give it: each
/// section (`{sketch}` or `{face}`, whether it was turned over, and its
/// loops in order: each loop's anchor, start, curves from it and their
/// count), whether it is closed, and its tangents (direction and weight,
/// and the derivative's length in meters).
pub fn report(part: &Id, s: &[Resolved], l: &Loft) -> Value {
    let sections: Vec<Value> = s
        .iter()
        .map(|r| {
            let mut v = r.source.clone();
            if let Some(g) = v.get("face").cloned()
                && let Ok(g) = GeomRef::from_json(&g, "/face")
            {
                v["face"] = json!(g.text(part));
            }
            v["turned"] = json!(r.turned);
            v["loops"] = json!(r
                .loops
                .iter()
                .map(|x| json!({"anchor": x.anchor, "start": x.start, r.entity: x.entities, "segments": x.entities.len()}))
                .collect::<Vec<_>>());
            if !r.chains.is_empty() {
                v["chains"] = json!(r
                    .chains
                    .iter()
                    .map(|c| json!({"chain": s[0].keys.chains.get(&c.key), r.entity: c.entities, "reversed": c.reversed}))
                    .collect::<Vec<_>>());
            }
            v
        })
        .collect();
    let span = (s[s.len() - 1].centroid - s[0].centroid).length();
    let mut v = json!({"sections": sections, "closed": l.closed});
    for (k, t) in [
        ("start_tangent", &l.start_tangent),
        ("end_tangent", &l.end_tangent),
    ] {
        if let Some(t) = t {
            v[k] = json!({
                "direction": match &t.direction {
                    TangentDirection::Normal => "normal".to_owned(),
                    TangentDirection::Ref(g) => g.text(part),
                },
                "weight": t.weight_number(),
                "length_m": t.weight_number() * span,
            });
        }
    }
    v
}

/// What loft `id` built from, for its rebuild report: [`report`] over
/// its sections resolved again from `results` (cheap once built), or
/// `None` where they no longer resolve.
pub fn described(part: &Part, id: &Id, l: &Loft, results: &dyn Results) -> Option<Value> {
    let s = sections(part, id, l, results).ok()?;
    Some(report(&part.id, &s, l))
}

/// A loft as output reports it: its file's fields, every reference as
/// text, every quantity as written and as a value.
pub fn feature_report(part: &Id, l: &Loft) -> Value {
    let mut v = l.fields();
    for (i, s) in l.sections.iter().enumerate() {
        if let Source::Face(g) = &s.source {
            v["sections"][i]["face"] = json!(g.text(part));
        }
    }
    for (k, t) in [
        ("start_tangent", &l.start_tangent),
        ("end_tangent", &l.end_tangent),
    ] {
        if let Some(t) = t {
            if let TangentDirection::Ref(g) = &t.direction {
                v[k]["direction"] = json!(g.text(part));
            }
            if let Some(q) = &t.weight {
                v[k]["weight"] = q.report();
            }
        }
    }
    v
}

/// A section as a request gives it: text naming a sketch (its ID, or
/// `<part>/<sketch>`) or a planar face (`<part>/<feature>/<name>`), or
/// `{section, loops}`.
fn section_json(given: &Value, part: &Id, k: usize) -> Result<Value> {
    let bad = |msg: &str| {
        Error::new(Code::InvalidParam, format!("section {k}: {msg}"))
            .with("param", "sections")
            .with("pointer", format!("/sections/{k}"))
    };
    let (text, loops) = match given {
        Value::String(t) => (t.clone(), None),
        Value::Object(o) => {
            let t = o
                .get("section")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("a section is text, or {section, loops}"))?;
            for key in o.keys() {
                if key != "section" && key != "loops" {
                    return Err(bad(&format!("a section has no field {key}")));
                }
            }
            (t.to_owned(), o.get("loops").cloned())
        }
        _ => return Err(bad("a section is text, or {section, loops}")),
    };
    let steps: Vec<&str> = text.split('/').collect();
    let mut v = match steps.as_slice() {
        [s] => json!({"sketch": s}),
        [p, s] if *p == part.as_str() => json!({"sketch": s}),
        [_, _] => return Err(bad("a section's sketch is one of this part's")),
        [_, _, _] => {
            json!({"face": crate::server::geom_json(&text, part, &format!("/sections/{k}/face"))?})
        }
        _ => {
            return Err(bad(
                "a section is a sketch (its ID or <part>/<sketch>) or a planar face <part>/<feature>/<name>",
            ));
        }
    };
    if let Some(l) = loops
        && l.as_array().is_some_and(|a| !a.is_empty())
    {
        v["loops"] = l;
    }
    Ok(v)
}

/// A loft from `current` (none for a new one) with the request's fields
/// laid over it: `sections` replaces them all; `closed`; a tangent's
/// direction (`normal` or a reference; `""` drops it, its weight with
/// it) and weight (`""` back to 1); `operation` `new` drops `targets`.
pub fn from_given(
    current: Option<&Loft>,
    f: &crate::proto::FeatureFields,
    part: &Part,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<crate::doc::files::Feature> {
    let pid = &part.id;
    let bad = |param: &str, msg: String| Error::new(Code::InvalidParam, msg).with("param", param);
    let mut v = current.map_or_else(|| json!({}), Loft::fields);
    let o = v.as_object_mut().expect("fields are an object");
    if let Some(s) = &f.sections {
        let s = s
            .iter()
            .enumerate()
            .map(|(k, x)| section_json(x, pid, k))
            .collect::<Result<Vec<_>>>()?;
        o.insert("sections".into(), json!(s));
    }
    if !o.contains_key("sections") {
        return Err(bad(
            "sections",
            "a loft skins two sections or more: sketches, or planar faces".into(),
        ));
    }
    for (key, x) in [("closed", f.closed), ("surface", f.surface)] {
        match x {
            Some(true) => {
                o.insert(key.into(), json!(true));
            }
            Some(false) => {
                o.remove(key);
            }
            None => {}
        }
    }
    for (key, dir, weight) in [
        ("start_tangent", &f.start_tangent, &f.start_weight),
        ("end_tangent", &f.end_tangent, &f.end_weight),
    ] {
        match dir.as_deref() {
            None => {}
            Some("") => {
                o.remove(key);
            }
            Some(d) => {
                let d = if d == "normal" {
                    json!("normal")
                } else {
                    crate::server::geom_json(d, pid, &format!("/{key}/direction"))?
                };
                let mut t = o.get(key).cloned().unwrap_or_else(|| json!({}));
                t["direction"] = d;
                o.insert(key.into(), t);
            }
        }
        match weight {
            None => {}
            Some(w) => {
                let Some(t) = o.get_mut(key).and_then(Value::as_object_mut) else {
                    return Err(bad(
                        key,
                        format!("a weight goes with a tangent: give {key} too"),
                    ));
                };
                if w.as_str() == Some("") {
                    t.remove("weight");
                } else {
                    t.insert("weight".into(), w.clone());
                }
            }
        }
    }
    if let Some(op) = &f.operation {
        if op == "new" {
            o.remove("operation");
            o.remove("targets");
        } else {
            o.insert("operation".into(), json!(op));
        }
    }
    if let Some(t) = &f.targets {
        o.insert("targets".into(), json!(t));
    }
    v["format"] = json!(crate::doc::sweep::FORMAT);
    v["kind"] = json!(crate::doc::files::FEATURE_KIND);
    v["type"] = json!("loft");
    crate::doc::files::Feature::from_json(&v, unit, env)
}
