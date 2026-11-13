//! Section views (design 417, plan 9.B): a cut resolved against what the
//! view draws, each body cut by it (a full section through
//! `ryukei::project::section`, an offset one through a stepped tool and
//! the boolean), the faces the cut makes (its caps) hatched, and the
//! cutting-plane line drawn on the parent.
//!
//! A cut body is kept by its body's hash, its pose's bits and the cut's
//! bits, in the session and on disk (`sections`), with its caps by name:
//! the next build of the view projects it again from the cache.

use super::{Frame, Source};
use crate::doc::assembly::Rev;
use crate::doc::drawing::{Cut, ModelPoint};
use crate::doc::geometry::GeomRef;
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use ryu_draw::{Anchor, Colour, Item, LineKind, P, Path};
use ryukei::geom::{Point3, Rigid, Vector3};
use ryukei::name::{Event, Name, OpKey, ToolKey};
use ryukei::topo::{Body, EntityId, FaceId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// How a section cuts.
#[derive(Clone, Debug)]
pub enum How {
    /// A full section: what lies on the side the plane's normal points to
    /// goes.
    Plane(crate::section::Plane),
    /// An offset section through `points` (model metres) in order, each
    /// pair's line swept along `square` (its parent's line of sight).
    Steps {
        points: Vec<[f64; 3]>,
        square: [f64; 3],
    },
}

/// A section view's cut.
#[derive(Clone, Debug)]
pub struct Cutting {
    pub parent: super::Key,
    pub letter: String,
    /// Towards the side cut away, unit: the view looks along its reverse.
    pub normal: [f64; 3],
    pub how: How,
    /// Instance paths left whole.
    pub unsectioned: Vec<String>,
}

fn bad(pointer: String, message: String) -> Error {
    Error::new(Code::InvalidParam, message).with("pointer", pointer)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn text_of(of: &Id, g: &GeomRef) -> String {
    match g {
        GeomRef::Plane(p) => p.as_str().to_owned(),
        GeomRef::Entity(r) => format!("{of}/{}/{}", r.feature, r.name),
        GeomRef::Feature(f) => format!("{of}/{f}"),
        other => other.text(of),
    }
}

/// A model point a view names, in the model `of` at `rev` gives.
pub(super) fn point_at(
    session: &mut Session,
    of: &Id,
    rev: &Rev,
    p: &ModelPoint,
    at: &str,
) -> Result<[f64; 3]> {
    let g = match p {
        ModelPoint::At(q) => return Ok(q.clone().map(|x| x.meters())),
        ModelPoint::Ref(g) => g,
    };
    let text = text_of(of, g);
    let not_a_point = || {
        bad(
            at.to_owned(),
            format!("{text} names no point: give a vertex, a datum point, origin or three lengths"),
        )
    };
    match g {
        GeomRef::Origin => Ok([0.0; 3]),
        GeomRef::Feature(_) => match crate::geometry::locate_at(session, &text, rev)? {
            Some(crate::geometry::Located {
                geom: crate::geometry::Geom::Point(q),
                ..
            }) => Ok(q.to_array()),
            _ => Err(not_a_point()),
        },
        GeomRef::Entity(_) => {
            let l = crate::measure::locate_at(session, &text, rev)?;
            let [l] = l.as_slice() else {
                return Err(not_a_point());
            };
            let ryukei::measure::Entity::Vertex(v) = l.entity else {
                return Err(not_a_point());
            };
            let q = l.built.body.vertex(v).ok_or_else(not_a_point)?.point;
            Ok(l.pose.apply_point(q).to_array())
        }
        _ => Err(not_a_point()),
    }
}

/// The cut of view `k`, from `parent` (its frame), in the model `of` at
/// `rev` gives.
#[allow(clippy::too_many_arguments)]
pub(super) fn cutting(
    session: &mut Session,
    of: &Id,
    rev: &Rev,
    k: super::Key,
    parent: (super::Key, &Frame),
    cut: &Cut,
    flip: bool,
    unsectioned: &[String],
    letter: &str,
) -> Result<Cutting> {
    let at = format!("/views/{k}/cut");
    let (normal, how) = match cut {
        Cut::Plane { plane, offset } => {
            let p = match plane {
                GeomRef::Plane(_) | GeomRef::Entity(_) | GeomRef::Feature(_) => {
                    crate::section::named_at(session, &text_of(of, plane), rev, &at)?
                }
                _ => {
                    return Err(bad(
                        format!("{at}/plane"),
                        format!("view {k} cuts on {}, which is no plane", text_of(of, plane)),
                    ));
                }
            };
            let p = match offset {
                Some(d) => p.moved(d.meters()),
                None => p,
            };
            let p = if flip { p.flipped() } else { p };
            (p.normal, How::Plane(p))
        }
        Cut::At { origin, normal } => {
            let o = origin.clone().map(|q| q.meters());
            let p = crate::section::Plane::new(o, *normal)
                .ok_or_else(|| bad(format!("{at}/normal"), "a cut's normal has a length".into()))?;
            let p = if flip { p.flipped() } else { p };
            (p.normal, How::Plane(p))
        }
        Cut::Through(points) => {
            let pts: Vec<[f64; 3]> = points
                .iter()
                .enumerate()
                .map(|(i, p)| point_at(session, of, rev, p, &format!("{at}/through/{i}")))
                .collect::<Result<_>>()?;
            let f = parent.1;
            let img: Vec<P> = pts.iter().map(|p| f.image(*p)).collect();
            let d0 = [img[1][0] - img[0][0], img[1][1] - img[0][1]];
            let len = ryu_math::hypot(d0[0], d0[1]);
            if len < 1e-9 {
                return Err(bad(
                    format!("{at}/through/1"),
                    format!(
                        "view {k}'s first two points lie on one line of sight of view {}",
                        parent.0
                    ),
                ));
            }
            // Each step runs on from the last, never back along the first
            // segment's way: else the cut would fold.
            for (i, w) in img.windows(2).enumerate() {
                let d = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
                if d[0] * d0[0] + d[1] * d0[1] < -1e-12 * len * ryu_math::hypot(d[0], d[1]) {
                    return Err(bad(
                        format!("{at}/through/{}", i + 1),
                        format!("view {k}'s cut turns back on itself: each point goes on along its first segment's way"),
                    )
                    .with("reason", "folds"));
                }
            }
            // To the left of the first segment, seen in the parent.
            let s = if flip { -1.0 } else { 1.0 };
            let m = [-d0[1] / len * s, d0[0] / len * s];
            let n = [0, 1, 2].map(|i| m[0] * f.x[i] + m[1] * f.y[i]);
            (
                n,
                How::Steps {
                    points: pts,
                    square: f.towards,
                },
            )
        }
    };
    Ok(Cutting {
        parent: parent.0,
        letter: letter.to_owned(),
        normal,
        how,
        unsectioned: unsectioned.to_vec(),
    })
}

/// A section view's frame: looking along `-normal`, up its parent's up
/// unless that runs along the normal, else away from its parent's eye (a
/// top view's way).
pub(super) fn frame_of(normal: [f64; 3], parent: &Frame) -> Result<Frame> {
    let up = if dot(normal, parent.y).abs() < 0.9 {
        parent.y
    } else {
        parent.towards.map(|x| -x)
    };
    Frame::new(normal, up)
}

/// A cut body and its caps, as the session keeps them.
#[derive(Clone, Debug)]
pub struct Cut1 {
    pub built: Arc<crate::rebuild::Built>,
    pub caps: Vec<FaceId>,
}

/// Cut bodies by their key, kept by the session.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    kept: BTreeMap<String, Arc<Cut1>>,
}

impl Cache {
    pub const MAX: usize = 256;

    fn get(&self, k: &str) -> Option<Arc<Cut1>> {
        self.kept.get(k).cloned()
    }

    fn put(&mut self, k: String, c: Arc<Cut1>) {
        if self.kept.len() >= Self::MAX {
            self.kept.clear();
        }
        self.kept.insert(k, c);
    }
}

/// The key of `body` at `pose` cut by `how`.
fn key(b: &crate::rebuild::Built, pose: &Rigid, how: &How) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"ryucad.section.1\0");
    h.update(b.hash.as_bytes());
    h.update(&ryukei::codec::encode_rigid(pose));
    let mut bits = |x: f64| {
        h.update(&x.to_bits().to_le_bytes());
    };
    match how {
        How::Plane(p) => {
            p.origin_m.into_iter().chain(p.normal).for_each(&mut bits);
        }
        How::Steps { points, square } => {
            points
                .iter()
                .flatten()
                .copied()
                .chain(*square)
                .for_each(&mut bits);
        }
    }
    h.finalize().to_hex().to_string()
}

/// `ryucad.section.1`: the caps' names, then the body.
fn encode(c: &Cut1) -> Vec<u8> {
    let mut out = b"ryucad.section.1\n".to_vec();
    out.extend_from_slice(&(c.caps.len() as u32).to_le_bytes());
    for f in &c.caps {
        let n = c.built.body.face(*f).expect("a live face").name;
        out.extend_from_slice(&n.to_u128().to_le_bytes());
    }
    out.extend_from_slice(&ryukei::codec::encode_body(&c.built.body));
    out
}

fn decode(bytes: &[u8], hash: &str, origins: &Arc<crate::rebuild::Origins>) -> Option<Cut1> {
    let rest = bytes.strip_prefix(b"ryucad.section.1\n")?;
    let n = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
    let names = rest.get(4..4 + 16 * n)?;
    let body = ryukei::codec::decode_body(rest.get(4 + 16 * n..)?).ok()?;
    let caps = names
        .as_chunks::<16>()
        .0
        .iter()
        .map(
            |c| match body.by_name(Name::from_u128(u128::from_le_bytes(*c))) {
                Some(EntityId::Face(f)) => Some(f),
                _ => None,
            },
        )
        .collect::<Option<Vec<_>>>()?;
    Some(Cut1 {
        built: Arc::new(crate::rebuild::Built {
            body,
            journal: ryukei::name::Journal::new(),
            origins: origins.clone(),
            hash: hash.to_owned(),
            reference: false,
            moved: None,
        }),
        caps,
    })
}

/// The names `journal` made of `names`: placed, split and merged, to a
/// fixed point (as the kernel follows a section's base).
fn follow(
    journal: &ryukei::name::Journal,
    names: impl IntoIterator<Item = Name>,
) -> BTreeSet<Name> {
    let mut set: BTreeSet<Name> = names.into_iter().collect();
    loop {
        let before = set.len();
        for e in journal.events() {
            match e {
                Event::Placed { from, to, .. } if set.contains(from) => {
                    set.insert(*to);
                }
                Event::Split { parent, children } if set.contains(parent) => {
                    set.extend(children.iter().copied());
                }
                Event::Merged { parents, child } if parents.iter().any(|p| set.contains(p)) => {
                    set.insert(*child);
                }
                _ => {}
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

/// The tool an offset section subtracts, in model space: the polygon of
/// the points seen along `square`, run on past both ends by `reach` and
/// closed round the side `normal` points to, extruded `reach` each way.
fn stepped_tool(
    points: &[[f64; 3]],
    square: [f64; 3],
    normal: [f64; 3],
    reach: f64,
) -> std::result::Result<Body, ryukei::Error> {
    use ryukei::geom::{Curve2, Frame as F3, Line2, Point2};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile};
    let o = Point3::new(points[0][0], points[0][1], points[0][2]);
    let frame = F3::new(
        o,
        Vector3::new(square[0], square[1], square[2]),
        Vector3::new(normal[0], normal[1], normal[2]),
    )?;
    let (x, y) = (frame.x().get().to_array(), frame.y().get().to_array());
    let flat = |p: [f64; 3]| {
        let d = [
            p[0] - points[0][0],
            p[1] - points[0][1],
            p[2] - points[0][2],
        ];
        [dot(d, x), dot(d, y)]
    };
    let q: Vec<P> = points.iter().map(|p| flat(*p)).collect();
    let unit = |a: P, b: P| {
        let l = ryu_math::hypot(b[0] - a[0], b[1] - a[1]);
        [(b[0] - a[0]) / l, (b[1] - a[1]) / l]
    };
    let (n, last) = (q.len(), q[q.len() - 1]);
    let d0 = unit(q[0], q[1]);
    let dl = unit(q[n - 2], last);
    let m = [dot(normal, x), dot(normal, y)];
    let a = [q[0][0] - d0[0] * reach, q[0][1] - d0[1] * reach];
    let b = [last[0] + dl[0] * reach, last[1] + dl[1] * reach];
    let mut ring = vec![a];
    ring.extend(q.iter().copied());
    ring.push(b);
    ring.push([b[0] + 2.0 * reach * m[0], b[1] + 2.0 * reach * m[1]]);
    ring.push([a[0] + 2.0 * reach * m[0], a[1] + 2.0 * reach * m[1]]);
    let mut segments = Vec::new();
    for (i, w) in ring.iter().zip(ring.iter().cycle().skip(1)).enumerate() {
        let (p, r) = (Point2::new(w.0[0], w.0[1]), Point2::new(w.1[0], w.1[1]));
        segments.push(Segment {
            key: SegmentKey(i as u128 + 1),
            curve: Curve2::Line2(Line2::through(p, r)?),
            t0: 0.0,
            t1: (r - p).length(),
        });
    }
    let profile = ValidProfile::new(Profile {
        frame,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })?;
    let key = OpKey::from_bytes(b"ryucad.drawing.offset_section");
    let (tool, _) = ryukei::sweep::extrude(
        key,
        &profile,
        Vector3::new(square[0], square[1], square[2]),
        -reach,
        reach,
    )?;
    Ok(tool)
}

/// The tool's key in an offset section's boolean.
const STEPS: ToolKey = ToolKey(0x5ec7_0ff5);

/// `b` at `pose` cut by `how`: the cut body (in its own frame, placed by
/// `pose` as before) and its caps.
fn cut_one(
    b: &crate::rebuild::Built,
    pose: &Rigid,
    how: &How,
    normal: [f64; 3],
    reach: f64,
    work: &ryukei::work::Work,
) -> std::result::Result<(Body, Vec<FaceId>), ryukei::Error> {
    let inv = pose.inverse();
    match how {
        How::Plane(p) => {
            let o = inv.apply_point(Point3::new(p.origin_m[0], p.origin_m[1], p.origin_m[2]));
            let n = inv.apply_vector(Vector3::new(p.normal[0], p.normal[1], p.normal[2]));
            let hint = if n.x.abs() < 0.9 {
                Vector3::new(1.0, 0.0, 0.0)
            } else {
                Vector3::new(0.0, 1.0, 0.0)
            };
            let plane = ryukei::geom::Plane::new(ryukei::geom::Frame::new(o, n, hint)?);
            let s = ryukei::project::section_with(
                OpKey::from_bytes(b"ryucad.drawing.section"),
                &b.body,
                &plane,
                work,
            )?;
            Ok((s.body, s.faces))
        }
        How::Steps { points, square } => {
            let tool = stepped_tool(points, *square, normal, reach)?;
            let names: Vec<Name> = tool
                .faces()
                .filter_map(|f| tool.face(f).map(|x| x.name))
                .collect();
            let (cut, journal) = ryukei::boolean::boolean_with(
                OpKey::from_bytes(b"ryucad.drawing.section"),
                ryukei::boolean::BooleanOp::Subtract,
                &b.body,
                &[ryukei::boolean::Tool {
                    key: STEPS,
                    body: &tool,
                    place: inv,
                }],
                work,
            )?;
            let made = follow(&journal, names);
            let caps = cut
                .faces()
                .filter(|&f| cut.face(f).is_some_and(|x| made.contains(&x.name)))
                .collect();
            Ok((cut, caps))
        }
    }
}

/// A cut the kernel made, or why it couldn't.
type Made = std::result::Result<(Body, Vec<FaceId>), ryukei::Error>;

/// Whether the solid named `name` (`<of>/<path…>/<founder>`) is under one
/// of `paths`.
fn whole(name: &str, paths: &[String]) -> bool {
    let inner = name.split_once('/').map_or("", |(_, r)| r);
    paths.iter().any(|p| inner.starts_with(&format!("{p}/")))
}

/// Each section plan's source cut, its caps beside it; a plan whose cut
/// the kernel refuses has its error. A cancel stops it all.
pub(super) fn cut_all(session: &mut Session, plans: &mut [(super::Plan, Source)]) -> Result<()> {
    struct Job1 {
        plan: usize,
        solid: usize,
        key: String,
    }
    let mut jobs = Vec::new();
    for (i, (plan, src)) in plans.iter_mut().enumerate() {
        let super::Extra::Section(c) = &plan.extra else {
            continue;
        };
        plan.caps = vec![Vec::new(); src.len()];
        for (j, (b, pose, name)) in src.iter().enumerate() {
            if !whole(name, &c.unsectioned) {
                jobs.push(Job1 {
                    plan: i,
                    solid: j,
                    key: key(b, pose, &c.how),
                });
            }
        }
    }
    let disk = session.disk().cloned();
    let mut found: Vec<Option<Arc<Cut1>>> = jobs
        .iter()
        .map(|j| {
            session.caches.sections.get(&j.key).or_else(|| {
                let d = disk.as_ref()?;
                let bytes = d.get(crate::cache::Kind::Section, j.key.as_bytes())?;
                let origins = plans[j.plan].1[j.solid].0.origins.clone();
                match decode(&bytes, &j.key, &origins) {
                    Some(c) => Some(Arc::new(c)),
                    None => {
                        d.discard(crate::cache::Kind::Section, j.key.as_bytes());
                        None
                    }
                }
            })
        })
        .collect();
    let job = session.job.clone();
    let stage = job.begin(ryu_session::job::Phase::Project, "sections", jobs.len());
    use rayon::prelude::*;
    let made: Vec<Option<Made>> = jobs
        .par_iter()
        .zip(found.par_iter())
        .map(|(j, have)| {
            if have.is_some() {
                return None;
            }
            let (plan, src) = &plans[j.plan];
            let super::Extra::Section(c) = &plan.extra else {
                unreachable!("a section's job")
            };
            let (b, pose, _) = &src[j.solid];
            let reach = super::reach(src);
            let work = crate::work::work(&job);
            Some(cut_one(b, pose, &c.how, c.normal, reach, &work))
        })
        .collect();
    for (n, m) in made.into_iter().enumerate() {
        let j = &jobs[n];
        stage.step(|| format!("{}", plans[j.plan].0.key))?;
        let cut = match m {
            None => found[n].take().expect("found"),
            Some(Ok((body, caps))) => {
                let b = &plans[j.plan].1[j.solid].0;
                let c = Arc::new(Cut1 {
                    built: Arc::new(crate::rebuild::Built {
                        body,
                        journal: ryukei::name::Journal::new(),
                        origins: b.origins.clone(),
                        hash: j.key.clone(),
                        reference: false,
                        moved: None,
                    }),
                    caps,
                });
                if let Some(d) = &disk {
                    d.put(crate::cache::Kind::Section, j.key.as_bytes(), &encode(&c));
                }
                session.caches.sections.put(j.key.clone(), c.clone());
                c
            }
            Some(Err(e)) => {
                if let Some(c) = crate::work::stopped(&e, &job) {
                    return Err(c);
                }
                let plan = &mut plans[j.plan].0;
                if plan.failed.is_none() {
                    plan.failed = Some(super::view_failed(plan, &e));
                }
                continue;
            }
        };
        let (plan, src) = &mut plans[j.plan];
        src[j.solid].0 = cut.built.clone();
        plan.caps[j.solid] = cut.caps.clone();
    }
    Ok(())
}

/// Each source solid's caps facing the view, as boundary paths in the
/// view's frame (model metres about the anchor's image): the input of its
/// hatching, by solid.
pub(super) fn cap_paths(
    src: &Source,
    caps: &[Vec<FaceId>],
    frame: &Frame,
    anchor: [f64; 3],
) -> Vec<Vec<Path>> {
    let c = frame.image(anchor);
    let view = frame.view();
    src.iter()
        .zip(caps)
        .map(|((b, pose, _), faces)| {
            let body = &b.body;
            let mut out = Vec::new();
            for f in faces {
                let Some(face) = body.face(*f) else { continue };
                // Facing the view: a plane whose outward normal is the
                // view's way (an offset section's steps are edge-on).
                let Some(ryukei::geom::Surface::Plane(p)) = body.surface(face.surface) else {
                    continue;
                };
                let z = p.frame().z().get();
                let z = match face.sense {
                    ryukei::topo::Sense::Forward => z,
                    ryukei::topo::Sense::Reversed => -z,
                };
                if dot(pose.apply_vector(z).to_array(), frame.towards) < 1.0 - 1e-9 {
                    continue;
                }
                for lp in body.face_loops(*f) {
                    for fin in body.loop_fins(lp) {
                        let Some(e) = body.fin(fin).and_then(|x| body.edge(x.edge)) else {
                            continue;
                        };
                        let Some(curve) = body.curve(e.curve) else {
                            continue;
                        };
                        let curve = curve.transformed(pose);
                        let Ok(images) = ryukei::project::project_curve(&curve, e.interval, &view)
                        else {
                            continue;
                        };
                        for im in images {
                            if let Ok(Some(path)) = super::curves::path(&im) {
                                out.push(path.moved([-c[0], -c[1]]));
                            }
                        }
                    }
                }
            }
            out
        })
        .collect()
}

/// The hatch spacing on a sheet `paper` millimetres across: 2.5 mm up to
/// ANSI B and A3, 3.5 mm up to ANSI D and A1, else 5 mm.
pub(super) fn spacing(paper: P) -> f64 {
    let long = paper[0].max(paper[1]);
    if long <= 432.0 {
        2.5
    } else if long <= 865.0 {
        3.5
    } else {
        5.0
    }
}

/// The `i`th sectioned solid's hatch: 45° or 135° alternately, every
/// other pair at six tenths the spacing, so neighbouring parts read apart.
pub(super) fn hatch_of(i: usize, base: f64) -> (f64, f64) {
    let quarter = std::f64::consts::FRAC_PI_4;
    let angle = if i.is_multiple_of(2) {
        quarter
    } else {
        3.0 * quarter
    };
    let spacing = if (i / 2).is_multiple_of(2) {
        base
    } else {
        0.6 * base
    };
    (angle, spacing)
}

/// The items marking a cut on its parent: the cutting-plane line through
/// `line` (sheet millimetres, in order), an arrow at each end pointing
/// along `sight` (the section's line of sight on the sheet, unit), and the
/// letter beyond each.
pub(super) fn cutting_marks(
    line: &[P],
    sight: P,
    letter: &str,
    style: ryu_draw::Standard,
) -> Vec<Item> {
    let mut out = Vec::new();
    out.push(Item::Path {
        path: Path::polyline(line, false),
        stroke: Some(style.stroke(LineKind::Cutting)),
        fill: None,
    });
    // ISO 128-3: thick at its ends (and its bends) over 5 mm.
    if style == ryu_draw::Standard::Iso {
        let thick = style.stroke(LineKind::Visible);
        let along = |a: P, b: P, l: f64| {
            let d = ryu_math::hypot(b[0] - a[0], b[1] - a[1]).max(1e-12);
            [a[0] + (b[0] - a[0]) * l / d, a[1] + (b[1] - a[1]) * l / d]
        };
        let n = line.len();
        let mut stubs = vec![
            [line[0], along(line[0], line[1], 5.0)],
            [line[n - 1], along(line[n - 1], line[n - 2], 5.0)],
        ];
        for w in line.windows(3) {
            stubs.push([along(w[1], w[0], 2.5), w[1]]);
            stubs.push([w[1], along(w[1], w[2], 2.5)]);
        }
        for [a, b] in stubs {
            out.push(Item::Path {
                path: Path::polyline(&[a, b], false),
                stroke: Some(thick.clone()),
                fill: None,
            });
        }
    }
    let thin = style.stroke(LineKind::Thin);
    let n = line.len();
    for (end, next) in [(line[0], line[1]), (line[n - 1], line[n - 2])] {
        // A stem of 6 mm along the sight, an arrowhead 3.5 by 1.2 at its tip.
        let tip = [end[0] + 6.0 * sight[0], end[1] + 6.0 * sight[1]];
        out.push(Item::Path {
            path: Path::polyline(&[end, tip], false),
            stroke: Some(thin.clone()),
            fill: None,
        });
        let back = [tip[0] - 3.5 * sight[0], tip[1] - 3.5 * sight[1]];
        let side = [-sight[1] * 0.6, sight[0] * 0.6];
        out.push(Item::Path {
            path: Path::polyline(
                &[
                    tip,
                    [back[0] + side[0], back[1] + side[1]],
                    [back[0] - side[0], back[1] - side[1]],
                ],
                true,
            ),
            stroke: None,
            fill: Some(Colour::BLACK),
        });
        // The letter beyond the line's end, along it.
        let d = ryu_math::hypot(end[0] - next[0], end[1] - next[1]).max(1e-12);
        let out_dir = [(end[0] - next[0]) / d, (end[1] - next[1]) / d];
        out.push(Item::Text {
            text: letter.to_owned(),
            at: [
                end[0] + 4.0 * out_dir[0] + 3.0 * sight[0],
                end[1] + 4.0 * out_dir[1] + 3.0 * sight[1] - 2.5,
            ],
            size: 5.0,
            angle: 0.0,
            anchor: Anchor::Middle,
            colour: Colour::BLACK,
        });
    }
    out
}

/// A section's caption: ASME's `SECTION A-A`, ISO's `A-A`.
pub(super) fn caption(letter: &str, standard: crate::doc::drawing::Standard) -> String {
    match standard {
        crate::doc::drawing::Standard::Asme => format!("SECTION {letter}-{letter}"),
        crate::doc::drawing::Standard::Iso => format!("{letter}-{letter}"),
    }
}
