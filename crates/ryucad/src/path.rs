//! Path and helix features to curves: a path's segments resolved (an
//! edge, grown to its tangent chain when it propagates; a sketch's line,
//! arc or circle; a helix's or another path's segments in place), each
//! run the way that meets its neighbour, keyed for phase 14's sweeps, and
//! checked by ryukei's path rules (`sweep::Path::check`) when it is named,
//! not when it is swept; a helix as ryukei's certified NURBS of it.
//!
//! Keys: an edge's segment is SipHash of `ryucad.path\0<part>\0edge\0<name>`,
//! a sketch curve's the sketch's own segment key
//! (`ryucad.sketch\0<part>\0<sketch>\0<entity>`), a helix's
//! `ryucad.helix\0<part>\0<helix>`; a path's segments keep the keys they
//! had. So moving, resizing or reordering what a path follows renames
//! nothing a sweep along it names.

use crate::doc::geometry::GeomRef;
use crate::doc::id::Id;
use crate::doc::path::{Hand, Helix, Path, PathFeature, Start};
use crate::error::{Code, Error, Result};
use crate::geometry::{At, Bounded, Chain, Line, Want, one};
use crate::rebuild::Output;
use ryukei::geom::{Curve, Frame, Point3, Vector3};
use ryukei::measure::Value as Measured;
use ryukei::name::{Name, OpKey};
use ryukei::sweep::{PathSegment, SegmentKey};
use ryukei::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use ryukei::topo::{EdgeId, EntityId, VertexId};
use serde_json::{Value, json};
use std::sync::Arc;

/// How closely a helix's NURBS follows the true helix: STEP's pipe
/// tolerance. A statement of fidelity to the helix intended, not an
/// edge's tolerance: the NURBS is the model.
pub const HELIX_TOLERANCE: f64 = 1e-7;

/// How closely a path's length is certified, relative to it.
const LENGTH_REL: f64 = 1e-9;

/// Edges at a chain's end tangent to it within this are a fork: the
/// kernel's chain stops where more than one goes on tangentially.
const FORK: f64 = 1e-6;

/// Where a segment of a built path came from.
#[derive(Clone, Debug, PartialEq)]
pub enum From {
    /// An edge of a solid, by founder and name.
    Edge { solid: Id, name: Name },
    /// A sketch's line, arc or circle.
    Sketch { sketch: Id, entity: Id },
    /// A helix.
    Helix(Id),
}

impl From {
    /// As a reference is written in text.
    pub fn text(&self, part: &Id) -> String {
        match self {
            From::Edge { solid, name } => format!("{part}/{solid}/{name}"),
            From::Sketch { sketch, entity } => format!("{part}/{sketch}/{entity}"),
            From::Helix(h) => format!("{part}/{h}"),
        }
    }
}

/// One segment of a built path: its key, its curve over the range it
/// runs along the path, where it came from and whether it runs against
/// its source's curve.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub key: SegmentKey,
    pub curve: Curve,
    pub range: [f64; 2],
    pub from: From,
    pub reversed: bool,
    /// Its edge's tolerance, else resolution.
    pub tolerance: f64,
}

impl Piece {
    fn start(&self) -> Point3 {
        self.curve.point(self.range[0])
    }

    fn end(&self) -> Point3 {
        self.curve.point(self.range[1])
    }

    /// The same piece run the other way.
    fn flipped(&self) -> Piece {
        let (curve, c) = self.curve.reversed();
        Piece {
            curve,
            range: [c - self.range[1], c - self.range[0]],
            reversed: !self.reversed,
            ..self.clone()
        }
    }
}

/// What a helix was built as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HelixMade {
    pub axis: Line,
    pub radius: f64,
    pub pitch: f64,
    pub turns: f64,
    pub height: f64,
    pub hand: Hand,
    pub taper: f64,
    /// The NURBS's certified distance from the true helix.
    pub deviation: f64,
}

/// What a path or helix rebuilds to: its segments in order as checked,
/// whether it closes, its length (certified) and tolerance, a helix's
/// numbers, and a hash of their bits.
#[derive(Clone, Debug, PartialEq)]
pub struct Made {
    pub pieces: Vec<Piece>,
    pub closed: bool,
    pub length: Measured,
    pub tolerance: f64,
    pub helix: Option<HelixMade>,
    pub hash: String,
}

impl Made {
    /// `path` or `helix`.
    pub fn kind(&self) -> &'static str {
        if self.helix.is_some() {
            "helix"
        } else {
            "path"
        }
    }

    /// As the resolver gives a curve.
    pub fn chain(&self) -> Chain {
        Chain {
            pieces: self
                .pieces
                .iter()
                .map(|p| Bounded {
                    curve: p.curve.clone(),
                    range: p.range,
                })
                .collect(),
        }
    }

    /// As ryukei's sweeps take it.
    pub fn kernel(&self) -> ryukei::sweep::Path {
        ryukei::sweep::Path {
            segments: self
                .pieces
                .iter()
                .map(|p| PathSegment {
                    key: p.key,
                    curve: p.curve.clone(),
                    range: p.range,
                })
                .collect(),
        }
    }

    /// Its geometry as rebuild reports and `inspect` give it, in meters:
    /// `{segments, closed, length_m, length_bound_m, tolerance_m}`, each
    /// segment `{from, reversed, start_m, end_m}`; a helix adds `helix`.
    pub fn report(&self, part: &Id) -> Value {
        let p = |p: Point3| json!([p.x, p.y, p.z]);
        let segments: Vec<Value> = self
            .pieces
            .iter()
            .map(|s| {
                json!({"from": s.from.text(part), "reversed": s.reversed,
                       "start_m": p(s.start()), "end_m": p(s.end())})
            })
            .collect();
        let mut v = json!({
            "segments": segments,
            "closed": self.closed,
            "length_m": self.length.value(),
            "tolerance_m": self.tolerance,
        });
        if let Measured::Bounded { bound, .. } = self.length {
            v["length_bound_m"] = json!(bound);
        }
        if let Some(h) = &self.helix {
            let d = h.axis.dir.get();
            v["helix"] = json!({
                "axis": {"origin_m": p(h.axis.origin), "direction": [d.x, d.y, d.z]},
                "radius_m": h.radius,
                "pitch_m": h.pitch,
                "turns": h.turns,
                "height_m": h.height,
                "hand": h.hand.as_str(),
                "taper_deg": h.taper.to_degrees(),
                "deviation_m": h.deviation,
            });
        }
        v
    }
}

/// Builds path or helix `at.owner`.
pub fn build(at: &At, f: &PathFeature) -> Result<Made> {
    match f {
        PathFeature::Path(p) => build_path(at, p),
        PathFeature::Helix(h) => build_helix(at, h),
    }
}

/// A run of pieces from one listed segment, in order, which turns over
/// as one.
struct Unit {
    entry: usize,
    pieces: Vec<Piece>,
}

impl Unit {
    fn start(&self) -> Point3 {
        self.pieces[0].start()
    }

    fn end(&self) -> Point3 {
        self.pieces[self.pieces.len() - 1].end()
    }

    fn flip(&mut self) {
        self.pieces.reverse();
        for p in &mut self.pieces {
            *p = p.flipped();
        }
    }
}

fn meets(a: Point3, b: Point3) -> bool {
    a.distance(b) <= LINEAR_RESOLUTION
}

/// The angle between the tangents where `a` ends and `b` starts.
fn joint_angle(a: &Piece, b: &Piece) -> f64 {
    let t = |c: &Curve, s: f64| c.eval(s).d1;
    let (u, v) = (t(&a.curve, a.range[1]), t(&b.curve, b.range[0]));
    ryukei::math::atan2(u.cross(v).length(), u.dot(v))
}

fn path_error(at: &At, code: Code, msg: String) -> Error {
    Error::new(code, format!("{}: {msg}", at.owner)).with("feature", at.owner)
}

/// Turns each unit so it starts where the one before ends: the first so
/// it ends where the second starts or ends, each other so it starts where
/// the one before ends. `Err((i, j, gap))` where units `i` and `j` don't
/// meet.
fn orient(units: &mut [Unit]) -> core::result::Result<(), (usize, usize, f64)> {
    if units.len() < 2 {
        return Ok(());
    }
    let (s1, e1) = (units[1].start(), units[1].end());
    if !(meets(units[0].end(), s1) || meets(units[0].end(), e1)) {
        if meets(units[0].start(), s1) || meets(units[0].start(), e1) {
            units[0].flip();
        } else {
            let (a, z) = (units[0].start(), units[0].end());
            let gap = [
                a.distance(s1),
                a.distance(e1),
                z.distance(s1),
                z.distance(e1),
            ]
            .into_iter()
            .fold(f64::INFINITY, f64::min);
            return Err((0, 1, gap));
        }
    }
    for i in 1..units.len() {
        let end = units[i - 1].end();
        if meets(end, units[i].start()) {
            continue;
        }
        if meets(end, units[i].end()) {
            units[i].flip();
            continue;
        }
        let gap = end
            .distance(units[i].start())
            .min(end.distance(units[i].end()));
        return Err((i - 1, i, gap));
    }
    Ok(())
}

/// An edge's segment key.
pub fn edge_key(part: &Id, name: Name) -> SegmentKey {
    let bytes = format!("ryucad.path\0{part}\0edge\0{name}");
    SegmentKey(OpKey::from_bytes(bytes.as_bytes()).0)
}

/// A helix's segment key.
pub fn helix_key(part: &Id, helix: &Id) -> SegmentKey {
    let bytes = format!("ryucad.helix\0{part}\0{helix}");
    SegmentKey(OpKey::from_bytes(bytes.as_bytes()).0)
}

/// The unit tangent of `edge` leaving `v`, where it has one.
fn leaving(body: &ryukei::topo::Body, edge: EdgeId, v: VertexId) -> Option<Vector3> {
    let e = body.edge(edge)?;
    let c = body.curve(e.curve)?;
    let (t, sign) = if e.start == v {
        (e.interval[0], 1.0)
    } else {
        (e.interval[1], -1.0)
    };
    c.tangent(t).map(|u| u.get() * sign)
}

/// The edges at the open ends of a tangent chain that go on from it
/// tangentially: more than one at an end is a fork (the kernel's chain
/// stops there, as it does where none goes on).
fn forks(body: &ryukei::topo::Body, chain: &[EdgeId]) -> Vec<EdgeId> {
    // An open end is a vertex one edge of the chain reaches once.
    let mut uses: Vec<(VertexId, EdgeId)> = Vec::new();
    for g in chain {
        let e = body.edge(*g).expect("a live edge");
        uses.push((e.start, *g));
        uses.push((e.end, *g));
    }
    let mut out = Vec::new();
    for &(v, edge) in &uses {
        if uses.iter().filter(|(w, _)| *w == v).count() != 1 {
            continue;
        }
        let Some(here) = leaving(body, edge, v) else {
            continue;
        };
        let on: Vec<EdgeId> = body
            .vertex_edges(v)
            .into_iter()
            .filter(|g| *g != edge)
            .filter(|g| {
                leaving(body, *g, v).is_some_and(|d| {
                    ryukei::math::atan2(here.cross(d).length(), -here.dot(d)) <= FORK
                })
            })
            .collect();
        if on.len() > 1 {
            out.extend(on);
        }
    }
    out
}

/// `path_branches` for segment `curve` of path `owner`, whose tangent
/// chain on `feature`'s solid `body` forks into `fork`: the edges by
/// reference, as the segment names its own.
fn branches(
    owner: &Id,
    part: &Id,
    curve: &GeomRef,
    feature: &Id,
    body: &ryukei::topo::Body,
    fork: &[EdgeId],
) -> Error {
    let names: Vec<String> = fork
        .iter()
        .map(|g| {
            format!(
                "{part}/{feature}/{}",
                body.edge(*g).expect("a live edge").name
            )
        })
        .collect();
    Error::new(
        Code::PathBranches,
        format!(
            "{owner}: the tangent chain of {} forks: more than one edge goes on from it",
            curve.text(part)
        ),
    )
    .with("feature", owner)
    .with("segment", curve.text(part))
    .with("edges", names)
}

/// The pieces listed segment `i` (in `role`) stands for, in order.
fn unit(at: &At, i: usize, role: &str, curve: &GeomRef, propagate: bool) -> Result<Unit> {
    let role = role.to_owned();
    let part = &at.part.id;
    match curve {
        GeomRef::Entity(r) => {
            let (b, e) = crate::reference::resolve(
                at.part,
                at.order,
                at.owner,
                &role,
                crate::reference::Follow::One,
                r,
                at.results,
            )?;
            let EntityId::Edge(edge) = e else {
                return Err(at
                    .fail(
                        Code::ReferenceWrongKind,
                        &role,
                        curve,
                        format!("is a {}, not an edge", crate::reference::kind(e)),
                    )
                    .with("wanted", ["an edge"])
                    .with("got", crate::reference::kind(e))
                    .with("kind", crate::reference::kind(e)));
            };
            let body = &b.body;
            let edges = if propagate {
                let chain = ryukei::blend::tangent_chain(body, edge).map_err(|k| {
                    path_error(
                        at,
                        Code::PathInvalid,
                        format!("its tangent chain from {} can't be found", curve.text(part)),
                    )
                    .with("kernel", format!("{k:?}"))
                })?;
                let fork = forks(body, &chain);
                if !fork.is_empty() {
                    return Err(branches(at.owner, part, curve, &r.feature, body, &fork));
                }
                chain
            } else {
                vec![edge]
            };
            let mut units: Vec<Unit> = edges
                .iter()
                .map(|g| {
                    let e = body.edge(*g).expect("a live edge");
                    let c = body.curve(e.curve).expect("a live curve").clone();
                    Unit {
                        entry: i,
                        pieces: vec![Piece {
                            key: edge_key(part, e.name),
                            curve: c,
                            range: e.interval,
                            from: From::Edge {
                                solid: r.feature.clone(),
                                name: e.name,
                            },
                            reversed: false,
                            tolerance: e.tolerance.get(),
                        }],
                    }
                })
                .collect();
            orient(&mut units).map_err(|(a, z, gap)| {
                let t = |k: usize| units[k].pieces[0].from.text(part);
                path_error(
                    at,
                    Code::PathDisconnected,
                    format!("its chain's edges {} and {} don't meet", t(a), t(z)),
                )
                .with("pieces", [t(a), t(z)])
                .with("gap_m", gap)
            })?;
            Ok(Unit {
                entry: i,
                pieces: units.into_iter().flat_map(|u| u.pieces).collect(),
            })
        }
        GeomRef::Sketch { feature, entity } => {
            let chain = one(at, &role, curve, Want::Curve)?.curve();
            let b = chain.pieces.into_iter().next().expect("one piece");
            Ok(Unit {
                entry: i,
                pieces: vec![Piece {
                    key: SegmentKey(crate::profile::entity_key(part, feature, entity)),
                    curve: b.curve,
                    range: b.range,
                    from: From::Sketch {
                        sketch: feature.clone(),
                        entity: entity.clone(),
                    },
                    reversed: false,
                    tolerance: LINEAR_RESOLUTION,
                }],
            })
        }
        GeomRef::Feature(f) => match at.output(&role, curve, f)? {
            Output::Path(m) => Ok(Unit {
                entry: i,
                pieces: m.pieces.clone(),
            }),
            o => {
                let what = at
                    .part
                    .features
                    .get(f)
                    .and_then(|x| x.as_ref().ok())
                    .map_or("feature", crate::doc::files::Feature::type_name);
                let hint = if o.sketch().is_some() {
                    "; name its line, arc or circle: <part>/<sketch>/<entity>"
                } else {
                    ""
                };
                Err(at
                    .fail(
                        Code::ReferenceWrongKind,
                        &role,
                        curve,
                        format!("is a {what}, not a path or helix{hint}"),
                    )
                    .with("wanted", ["a curve"])
                    .with("got", what))
            }
        },
        _ => Err(at
            .fail(
                Code::ReferenceWrongKind,
                &role,
                curve,
                "is no curve".to_owned(),
            )
            .with("wanted", ["a curve"])),
    }
}

fn build_path(at: &At, path: &Path) -> Result<Made> {
    let part = &at.part.id;
    let mut units = path
        .segments
        .iter()
        .enumerate()
        .map(|(i, s)| unit(at, i, &format!("segments.{i}"), &s.curve, s.propagate))
        .collect::<Result<Vec<Unit>>>()?;
    let entry = |i: usize| path.segments[i].curve.text(part);
    orient(&mut units).map_err(|(a, z, gap)| {
        path_error(
            at,
            Code::PathDisconnected,
            format!(
                "its segments {} and {} don't meet: {gap} m apart",
                entry(units[a].entry),
                entry(units[z].entry)
            ),
        )
        .with("segments", [entry(units[a].entry), entry(units[z].entry)])
        .with("gap_m", gap)
    })?;
    if path.reverse {
        units.reverse();
        for u in &mut units {
            u.flip();
        }
    }
    // Which listed segment each piece came from.
    let mut owner = Vec::new();
    let mut pieces = Vec::new();
    for u in units {
        for p in u.pieces {
            owner.push(u.entry);
            pieces.push(p);
        }
    }
    for (j, p) in pieces.iter().enumerate() {
        if let Some(k) = pieces[..j].iter().position(|q| q.key == p.key) {
            return Err(path_error(
                at,
                Code::PathSegmentTwice,
                format!("it follows {} twice", p.from.text(part)),
            )
            .with("segments", [entry(owner[k]), entry(owner[j])])
            .with("piece", p.from.text(part)));
        }
    }
    finish(at, pieces, &|j| entry(owner[j]), None)
}

/// What a sweep (`at.owner`) follows: the curve `curve` names in its
/// `role`, as a path of it alone would be (a path or helix as built, an
/// edge or a sketch's line, arc or circle keyed as a path's segment, so a
/// sweep along one curve names what it makes as one along a path of that
/// curve), checked by ryukei's path rules.
pub fn of_curve(at: &At, role: &str, curve: &GeomRef) -> Result<Made> {
    if let GeomRef::Feature(f) = curve
        && let Output::Path(m) = at.output(role, curve, f)?
    {
        return Ok(m.as_ref().clone());
    }
    let u = unit(at, 0, role, curve, false)?;
    let text = curve.text(&at.part.id);
    finish(at, u.pieces, &|_| text.clone(), None)
}

/// Checks pieces by ryukei's path rules, and measures them.
fn finish(
    at: &At,
    pieces: Vec<Piece>,
    entry: &dyn Fn(usize) -> String,
    helix: Option<HelixMade>,
) -> Result<Made> {
    let part = &at.part.id;
    let kernel = ryukei::sweep::Path {
        segments: pieces
            .iter()
            .map(|p| PathSegment {
                key: p.key,
                curve: p.curve.clone(),
                range: p.range,
            })
            .collect(),
    };
    let closed = kernel.check().map_err(|e| {
        let at_key = |k: SegmentKey| pieces.iter().position(|p| p.key == k).unwrap_or(0);
        match e {
            ryukei::Error::InvalidSweep(ryukei::SweepError::PathNotG1 { at: k }) => {
                let j = at_key(k);
                let i = if j == 0 { pieces.len() - 1 } else { j - 1 };
                let (a, b) = (&pieces[i], &pieces[j]);
                let gap = a.end().distance(b.start());
                if gap > LINEAR_RESOLUTION {
                    path_error(
                        at,
                        Code::PathDisconnected,
                        format!(
                            "{} and {} don't meet: {gap} m apart",
                            a.from.text(part),
                            b.from.text(part)
                        ),
                    )
                    .with("segments", [entry(i), entry(j)])
                    .with("pieces", [a.from.text(part), b.from.text(part)])
                    .with("gap_m", gap)
                } else {
                    let angle = joint_angle(a, b);
                    path_error(
                        at,
                        Code::PathNotTangent,
                        format!(
                            "{} and {} meet at {} deg: a path's joints are tangent",
                            a.from.text(part),
                            b.from.text(part),
                            angle.to_degrees()
                        ),
                    )
                    .with("segments", [entry(i), entry(j)])
                    .with("pieces", [a.from.text(part), b.from.text(part)])
                    .with("angle_rad", angle)
                }
            }
            ryukei::Error::InvalidSweep(ryukei::SweepError::BadPathSegment { segment: k }) => {
                let j = at_key(k);
                path_error(
                    at,
                    Code::PathInvalid,
                    format!(
                        "{} can't be a path's segment: it is too short, has no tangent at an end, or is a whole closed curve among others",
                        pieces[j].from.text(part)
                    ),
                )
                .with("segment", entry(j))
                .with("piece", pieces[j].from.text(part))
            }
            e => path_error(at, Code::PathInvalid, "ryukei refuses it as a path".into())
                .with("kernel", format!("{e:?}")),
        }
    })?;
    let mut length = Measured::Exact(0.0);
    for (j, p) in pieces.iter().enumerate() {
        let l = ryukei::measure::curve_length(&p.curve, p.range, LENGTH_REL).ok_or_else(|| {
            path_error(
                at,
                Code::PathInvalid,
                format!("{}'s length can't be certified", p.from.text(part)),
            )
            .with("segment", entry(j))
        })?;
        length = match (length, l) {
            (Measured::Exact(a), Measured::Exact(b)) => Measured::Exact(a + b),
            (a, b) => Measured::Bounded {
                value: a.value() + b.value(),
                bound: a.bound() + b.bound(),
            },
        };
    }
    let tolerance = pieces
        .iter()
        .map(|p| p.tolerance)
        .fold(LINEAR_RESOLUTION, f64::max);
    let hash = hash(&pieces, closed, helix.as_ref());
    Ok(Made {
        pieces,
        closed,
        length,
        tolerance,
        helix,
        hash,
    })
}

fn helix_invalid(at: &At, pointer: &str, msg: &str) -> Error {
    path_error(at, Code::HelixInvalid, msg.to_owned()).with("pointer", pointer)
}

fn build_helix(at: &At, h: &Helix) -> Result<Made> {
    let part = &at.part.id;
    let axis = one(at, "axis", &h.axis, Want::Axis)?.axis();
    let d = axis.dir.get();
    let (pitch, turns, height) = h.size.values();
    if !(pitch.is_finite() && turns.is_finite() && height.is_finite()) {
        return Err(helix_invalid(at, "/pitch", "its size isn't finite"));
    }
    if turns > crate::doc::path::MAX_TURNS {
        return Err(helix_invalid(
            at,
            "/turns",
            &format!("it turns {turns} times: at most 10000"),
        ));
    }
    let (base, x, radius) = match &h.start {
        Start::Point(g) => {
            let q = one(at, "start", g, Want::Point)?.point();
            let foot = axis.origin + d * (q - axis.origin).dot(d);
            let r = q - foot;
            if r.length() <= LINEAR_RESOLUTION {
                return Err(helix_invalid(at, "/start", "its start lies on its axis"));
            }
            (foot, r, r.length())
        }
        Start::Radius(q) => {
            // The part's x across the axis, its y where x is along it.
            let x = Vector3::new(1.0, 0.0, 0.0);
            let hint = if x.cross(d).length() > ANGULAR_RESOLUTION {
                x
            } else {
                Vector3::new(0.0, 1.0, 0.0)
            };
            (axis.origin, hint, q.meters())
        }
    };
    let frame = Frame::new(base, d, x).map_err(|e| {
        Error::new(
            Code::InvalidPlacement,
            format!("{}: a helix outside the ±500 m size box", at.owner),
        )
        .with("feature", at.owner)
        .with("kernel", format!("{e:?}"))
    })?;
    let taper = h.taper.as_ref().map_or(0.0, |t| t.radians());
    let kernel = ryukei::geom::Helix {
        frame,
        radius,
        pitch,
        taper,
        turns,
        hand: match h.hand {
            Hand::Right => ryukei::geom::Hand::Right,
            Hand::Left => ryukei::geom::Hand::Left,
        },
    };
    if kernel.radius_at(kernel.end()) <= 0.0 {
        return Err(helix_invalid(
            at,
            "/taper",
            "its taper narrows its radius to nothing within its height",
        ));
    }
    let (nurbs, deviation) = kernel.to_nurbs(HELIX_TOLERANCE).map_err(|e| {
        helix_invalid(at, "/turns", "ryukei can't make its NURBS form")
            .with("kernel", format!("{e:?}"))
    })?;
    let range = nurbs.domain();
    let piece = Piece {
        key: helix_key(part, at.owner),
        curve: Curve::Nurbs(Arc::new(nurbs)),
        range,
        from: From::Helix(at.owner.clone()),
        reversed: false,
        tolerance: LINEAR_RESOLUTION,
    };
    let made = HelixMade {
        axis,
        radius,
        pitch,
        turns,
        height,
        hand: h.hand,
        taper,
        deviation,
    };
    let me = format!("{part}/{}", at.owner);
    finish(at, vec![piece], &|_| me.clone(), Some(made))
}

/// A hash of every bit of the pieces' curves, ranges, keys and sources,
/// whether it closes and a helix's numbers.
fn hash(pieces: &[Piece], closed: bool, helix: Option<&HelixMade>) -> String {
    let mut h = blake3::Hasher::new();
    let f = |h: &mut blake3::Hasher, xs: &[f64]| {
        for x in xs {
            h.update(&x.to_bits().to_le_bytes());
        }
    };
    let p3 = |p: Point3| [p.x, p.y, p.z];
    let v3 = |v: Vector3| [v.x, v.y, v.z];
    for p in pieces {
        h.update(&p.key.0.to_le_bytes());
        h.update(&[u8::from(p.reversed)]);
        h.update(format!("{:?}", p.from).as_bytes());
        f(&mut h, &p.range);
        f(&mut h, &[p.tolerance]);
        match &p.curve {
            Curve::Line(l) => {
                h.update(b"line");
                f(&mut h, &p3(l.origin()));
                f(&mut h, &v3(l.dir().get()));
            }
            Curve::Circle(c) => {
                h.update(b"circle");
                let fr = c.frame();
                f(&mut h, &p3(fr.origin()));
                f(&mut h, &v3(fr.x().get()));
                f(&mut h, &v3(fr.z().get()));
                f(&mut h, &[c.radius()]);
            }
            Curve::Ellipse(e) => {
                h.update(b"ellipse");
                let fr = e.frame();
                f(&mut h, &p3(fr.origin()));
                f(&mut h, &v3(fr.x().get()));
                f(&mut h, &v3(fr.z().get()));
                f(&mut h, &[e.major(), e.minor()]);
            }
            Curve::Nurbs(n) => {
                h.update(b"nurbs");
                let kv = n.knots();
                h.update(&(kv.degree() as u64).to_le_bytes());
                h.update(&[u8::from(kv.is_periodic())]);
                f(&mut h, kv.knots());
                for m in kv.mults() {
                    h.update(&(*m as u64).to_le_bytes());
                }
                for q in n.control_points() {
                    f(&mut h, &p3(q));
                }
                f(&mut h, n.weights());
            }
            c => {
                h.update(format!("{c:?}").as_bytes());
            }
        }
    }
    h.update(&[u8::from(closed)]);
    if let Some(x) = helix {
        f(&mut h, &p3(x.axis.origin));
        f(&mut h, &v3(x.axis.dir.get()));
        f(
            &mut h,
            &[x.radius, x.pitch, x.turns, x.height, x.taper, x.deviation],
        );
        h.update(x.hand.as_str().as_bytes());
    }
    h.finalize().to_hex()[..32].to_owned()
}

/// A path or helix of type `ty` from `current` (none for a new one) with
/// the request's fields laid over it: references as text in `part`,
/// quantities as expressions. A path's `segments` replace them all. A
/// helix's `radius` and `start` replace each other; of its pitch, turns
/// and height those given are kept, and the others as they were, in that
/// order, until there are two; `hand` `right` and a `taper` of `""` are
/// written as nothing.
pub fn from_given(
    ty: &str,
    current: Option<&PathFeature>,
    f: &crate::proto::FeatureFields,
    part: &Id,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<crate::doc::files::Feature> {
    let geom = |t: &str, at: &str| crate::server::geom_json(t, part, at);
    let mut v = current.map_or_else(|| json!({}), PathFeature::fields);
    let o = v.as_object_mut().expect("fields are an object");
    let bad = |param: &str, msg: String| Error::new(Code::InvalidParam, msg).with("param", param);
    if ty == "path" {
        if let Some(list) = &f.segments {
            let mut out = Vec::with_capacity(list.len());
            for (i, item) in list.iter().enumerate() {
                let at = format!("/segments/{i}");
                let (text, propagate) = match item {
                    Value::String(t) => (t.as_str(), false),
                    Value::Object(m) => {
                        let t = m.get("curve").and_then(Value::as_str).ok_or_else(|| {
                            bad("segments", format!("segment {i} names its curve as text"))
                        })?;
                        let p = match m.get("propagate") {
                            None => false,
                            Some(Value::Bool(b)) => *b,
                            Some(_) => {
                                return Err(bad(
                                    "segments",
                                    format!("segment {i}'s propagate is true or false"),
                                ));
                            }
                        };
                        if let Some(k) = m.keys().find(|k| *k != "curve" && *k != "propagate") {
                            return Err(bad("segments", format!("a segment has no {k}")));
                        }
                        (t, p)
                    }
                    _ => {
                        return Err(bad(
                            "segments",
                            format!("segment {i} is a curve's text or {{curve, propagate}}"),
                        ));
                    }
                };
                let mut g = geom(text, &at)?;
                if propagate {
                    g["propagate"] = json!(true);
                }
                out.push(g);
            }
            o.insert("segments".into(), json!(out));
        }
        if !o.contains_key("segments") {
            return Err(bad(
                "segments",
                "a path names its segments: curves end to end, in order".into(),
            ));
        }
        match f.reverse {
            Some(true) => {
                o.insert("reverse".into(), json!(true));
            }
            Some(false) => {
                o.remove("reverse");
            }
            None => {}
        }
    } else {
        if let Some(t) = &f.axis {
            o.insert("axis".into(), geom(t, "/axis")?);
        }
        match (&f.radius, &f.start) {
            (Some(_), Some(_)) => {
                return Err(bad(
                    "radius",
                    "a helix starts at a radius or at a start point, not both".into(),
                ));
            }
            (Some(r), None) => {
                o.remove("start");
                o.insert("radius".into(), r.clone());
            }
            (None, Some(t)) => {
                o.remove("radius");
                o.insert("start".into(), geom(t, "/start")?);
            }
            (None, None) => {}
        }
        let given = [
            ("pitch", &f.pitch),
            ("turns", &f.turns),
            ("height", &f.height),
        ];
        let named = given.iter().filter(|(_, x)| x.is_some()).count();
        if named == 3 {
            return Err(bad(
                "pitch",
                "a helix takes two of pitch, turns and height: the third follows".into(),
            ));
        }
        let mut keep: Vec<&str> = given
            .iter()
            .filter(|(_, x)| x.is_some())
            .map(|(k, _)| *k)
            .collect();
        for k in ["pitch", "turns", "height"] {
            if keep.len() < 2 && !keep.contains(&k) && o.contains_key(k) {
                keep.push(k);
            }
        }
        for (k, x) in given {
            match x {
                Some(q) => {
                    o.insert(k.into(), q.clone());
                }
                None if !keep.contains(&k) => {
                    o.remove(k);
                }
                None => {}
            }
        }
        match f.hand.as_deref() {
            None => {}
            Some("right") => {
                o.remove("hand");
            }
            Some("left") => {
                o.insert("hand".into(), json!("left"));
            }
            Some(h) => {
                return Err(bad(
                    "hand",
                    format!("a helix's hand is right or left, not {h}"),
                ));
            }
        }
        match &f.taper {
            None => {}
            Some(Value::String(t)) if t.is_empty() => {
                o.remove("taper");
            }
            Some(t) => {
                o.insert("taper".into(), t.clone());
            }
        }
        for k in ["axis", "pitch"] {
            if !o.contains_key(k)
                && !(k == "pitch" && (o.contains_key("turns") && o.contains_key("height")))
            {
                return Err(bad(
                    k,
                    if k == "axis" {
                        "a helix names its axis".into()
                    } else {
                        "a helix takes two of pitch, turns and height".into()
                    },
                ));
            }
        }
        if !o.contains_key("radius") && !o.contains_key("start") {
            return Err(bad(
                "radius",
                "a helix starts at a radius or at a start point".into(),
            ));
        }
    }
    v["format"] = json!(crate::doc::path::FORMAT);
    v["kind"] = json!(crate::doc::files::FEATURE_KIND);
    v["type"] = json!(ty);
    crate::doc::files::Feature::from_json(&v, unit, env)
}

/// Refuses path or helix `id` whose references don't resolve to the
/// kinds their fields want now: a path's segments curves (an edge, a
/// sketch's line, arc or circle, a path or helix), a helix's axis an axis
/// and its start a point. Whether they meet only rebuild can tell.
pub fn check(
    part: &crate::doc::project::Part,
    order: &[Id],
    id: &Id,
    f: &PathFeature,
    results: &dyn crate::rebuild::Results,
) -> Result<()> {
    let at = At {
        part,
        order,
        owner: id,
        results,
    };
    match f {
        PathFeature::Path(p) => {
            for (i, s) in p.segments.iter().enumerate() {
                let role = format!("segments.{i}");
                match &s.curve {
                    GeomRef::Feature(g) => {
                        if !matches!(at.output(&role, &s.curve, g)?, Output::Path(_)) {
                            unit(&at, i, &role, &s.curve, false)?;
                        }
                    }
                    GeomRef::Entity(r) => {
                        let (_, e) = crate::reference::resolve(
                            part,
                            order,
                            id,
                            &role,
                            crate::reference::Follow::One,
                            r,
                            results,
                        )?;
                        if !matches!(e, EntityId::Edge(_)) {
                            unit(&at, i, &role, &s.curve, false)?;
                        }
                    }
                    g => {
                        one(&at, &role, g, Want::Curve)?;
                    }
                }
            }
            Ok(())
        }
        PathFeature::Helix(h) => {
            one(&at, "axis", &h.axis, Want::Axis)?;
            if let Start::Point(g) = &h.start {
                one(&at, "start", g, Want::Point)?;
            }
            Ok(())
        }
    }
}

/// A path or helix as output reports it: its file's fields, every
/// reference as text (a propagated edge `{curve, propagate}`), every
/// quantity as written and as a value.
pub fn report(part: &Id, f: &PathFeature) -> Value {
    let mut v = f.fields();
    match f {
        PathFeature::Path(p) => {
            v["segments"] = p
                .segments
                .iter()
                .map(|s| {
                    if s.propagate {
                        json!({"curve": s.curve.text(part), "propagate": true})
                    } else {
                        json!(s.curve.text(part))
                    }
                })
                .collect();
        }
        PathFeature::Helix(_) => {
            for (role, g) in f.geometry() {
                v[role] = json!(g.text(part));
            }
        }
    }
    for (k, q) in f.quantities() {
        v[k] = q.report();
    }
    v
}

/// `path.get`: a path's or helix's polyline in model space, for the app to
/// draw: its pieces cut as renders cut them ([`crate::render::path_polyline`]),
/// to `quality`'s chord for the path's own size, with whether it closes
/// and its part's revision (the app asks again when that moves).
pub fn get(
    session: &mut crate::session::Session,
    part: &str,
    path: &str,
    quality: crate::mesh::Quality,
) -> Result<Value> {
    let (pid, fid) = (
        crate::server::part_id(part)?,
        crate::server::feature_id(path)?,
    );
    let revision = session.project().part(&pid)?.revision.clone();
    let built = session.rebuilt(&pid)?;
    let made = match built.features.iter().find(|(id, _)| *id == fid) {
        Some((_, Ok(Output::Path(m)))) => m.clone(),
        Some((_, Ok(_))) => {
            return Err(Error::new(
                Code::ReferenceWrongKind,
                format!("{pid}/{fid} is not a path or helix"),
            )
            .with("part", &pid)
            .with("feature", &fid));
        }
        Some((_, Err(e))) => return Err(e.clone()),
        None => {
            return Err(
                Error::new(Code::NotFound, format!("{pid} has no feature {fid}"))
                    .with("part", &pid)
                    .with("feature", &fid),
            );
        }
    };
    let chain = made.chain();
    let params = quality.params(crate::render::chain_span(&chain));
    Ok(json!({
        "feature": fid,
        "type": made.kind(),
        "revision": revision,
        "quality": quality.as_str(),
        "closed": made.closed,
        "polyline": crate::render::path_polyline(&chain, params.chord, params.angle),
    }))
}

#[cfg(test)]
mod tests;
