//! Interference: which occurrences of an assembly overlap. Every solid of
//! every occurrence is boxed where the assembly puts it; each pair of
//! solids of different occurrences whose boxes overlap is intersected by
//! ryukei, the second placed in the first's frame, so the answer depends
//! only on the two bodies and how one sits against the other. Those are
//! the cache's key: moving an unrelated instance recomputes nothing.
//!
//! An empty intersection is clear; so is one ryukei refuses as touching
//! itself along an edge or at a point (bodies meeting where a mate puts
//! them), reported as `touching`, as is one whose box or mesh is flat (faces the
//! two share, which hold no volume). A solid intersection is an
//! interference, with its box, its volume estimated from its mesh at
//! normal quality and the faces of each side it touches. Any other
//! refusal leaves the pair unchecked, saying why.
//!
//! A pair with a reference import's body on a side (design 364) is
//! judged on both meshes at normal quality instead ([`mesh`]): clear or
//! touching within the pair's reach, or an approximate interference, with
//! no volume, where a point of one surface lies deep inside the other's
//! closed mesh.

use crate::assembly::{Model, Occurrence, path_text};
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::mesh::Quality;
use crate::rebuild::Built;
use rayon::prelude::*;
use ryu_session::job::{Job, Phase};
use ryukei::boolean::{BooleanOp, Tool};
use ryukei::geom::{Box3, Point3, Rigid};
use ryukei::name::{Event, Name, OpKey, ToolKey};
use ryukei::topo::Body;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

pub mod mesh;
pub use mesh::Prepared;

/// What intersecting two solids found, in the first one's frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Nothing in common.
    Clear,
    /// Meeting along an edge or at a point only.
    Touching,
    /// Overlapping in a volume: its box in the first solid's frame, its
    /// volume estimated from its mesh, and the faces of each solid it
    /// touches, by name.
    Interferes {
        min: [f64; 3],
        max: [f64; 3],
        volume_m3: f64,
        faces_a: Vec<Name>,
        faces_b: Vec<Name>,
    },
    /// Overlapping, judged on meshes (a reference import's side): a point
    /// of one surface certified deep inside the other's closed mesh. Its
    /// volume isn't known; `min`/`max` are the meshes' boxes' overlap in
    /// the first solid's frame; `open` where a side's mesh doesn't close
    /// (a sheet passing through a solid); the faces of each side whose
    /// triangles come within the pair's reach of the other.
    Approximate {
        min: [f64; 3],
        max: [f64; 3],
        open: bool,
        faces_a: Vec<Name>,
        faces_b: Vec<Name>,
    },
    /// ryukei couldn't decide: its refusal.
    CannotCheck { kernel: String },
}

/// Verdicts by the two bodies' hashes and the second's pose in the
/// first's frame, kept by the session.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    verdicts: BTreeMap<String, Arc<Verdict>>,
}

impl Cache {
    pub fn len(&self) -> usize {
        self.verdicts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.verdicts.is_empty()
    }
}

/// One solid of one occurrence, where the assembly puts it.
struct Piece<'m> {
    occurrence: &'m Occurrence,
    founder: &'m Id,
    built: &'m Built,
    world: Box3,
}

/// The key of a pair: both hashes and the bits of `rel`, the second
/// body's pose in the first's frame.
pub(crate) fn key(a: &Built, b: &Built, rel: &Rigid) -> String {
    let mut h = blake3::Hasher::new();
    h.update(format!("{} {}\n", a.hash, b.hash).as_bytes());
    // Judged on meshes: never a verdict a boolean gave.
    if a.reference || b.reference {
        h.update(b"mesh\n");
    }
    for row in rel.rotation_matrix() {
        for x in row {
            h.update(&x.to_bits().to_le_bytes());
        }
    }
    let t = rel.translation_part();
    for x in [t.x, t.y, t.z] {
        h.update(&x.to_bits().to_le_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// The box round `body`'s faces moved by `pose`, `None` for an empty one.
pub(crate) fn world_box(body: &Body, pose: &Rigid) -> Option<Box3> {
    body_box(body).map(|b| b.transform_rigid(pose))
}

/// The box round `body`'s faces in its own frame, `None` for an empty one.
fn body_box(body: &Body) -> Option<Box3> {
    body.faces()
        .map(|f| ryukei::intersect::face_box(body, f))
        .reduce(|a, b| a.union(&b))
}

/// Where `b` sits in `a`'s frame, through the deepest assembly both are
/// in: two occurrences of a subassembly relate as they do in it, to the
/// bit, wherever it is placed. Through the world poses of a subassembly
/// placed 150 m out, a pin standing on its plate came out rounded off the
/// plate's face, and every such pair took the kernel's slow paths.
fn relative(a: &Occurrence, b: &Occurrence) -> Rigid {
    let shared = a
        .path
        .iter()
        .zip(&b.path)
        .take_while(|(x, y)| x == y)
        .count();
    match (a.within.get(shared), b.within.get(shared)) {
        (Some(pa), Some(pb)) => pa.inverse().then_after(pb),
        _ => a.pose.inverse().then_after(&b.pose),
    }
}

/// Cells along each axis a box may span before it is paired by brute
/// force instead: a part metres long among parts centimetres long.
const SPAN: f64 = 4.0;

/// Every pair `(a, b)`, `a < b`, of `boxes` that overlap, in order. Boxes
/// go into a grid of cells about twice the median box's longest side, so
/// a box meets only the boxes near it; one spanning more than [`SPAN`]
/// cells along an axis is met with every other box.
fn overlapping(boxes: &[Box3]) -> Vec<(usize, usize)> {
    let longest = |b: &Box3| {
        let (lo, hi) = (b.min(), b.max());
        (hi.x - lo.x).max(hi.y - lo.y).max(hi.z - lo.z)
    };
    let mut sizes: Vec<f64> = boxes.iter().map(longest).collect();
    sizes.sort_by(f64::total_cmp);
    let cell = sizes
        .get(sizes.len() / 2)
        .map_or(1.0, |m| 2.0 * m.max(1e-6));
    let at = |v: f64| (v / cell).floor() as i64;
    let mut grid: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    let mut large = Vec::new();
    for (i, b) in boxes.iter().enumerate() {
        if longest(b) > SPAN * cell {
            large.push(i);
            continue;
        }
        let (lo, hi) = (b.min(), b.max());
        for x in at(lo.x)..=at(hi.x) {
            for y in at(lo.y)..=at(hi.y) {
                for z in at(lo.z)..=at(hi.z) {
                    grid.entry([x, y, z]).or_default().push(i);
                }
            }
        }
    }
    let mut out = Vec::new();
    for members in grid.values() {
        for (k, &a) in members.iter().enumerate() {
            for &b in &members[k + 1..] {
                if boxes[a].intersects(&boxes[b]) {
                    out.push((a.min(b), a.max(b)));
                }
            }
        }
    }
    for &a in &large {
        for (b, other) in boxes.iter().enumerate() {
            if a != b && boxes[a].intersects(other) {
                out.push((a.min(b), a.max(b)));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Intersects `a` with `b` placed at `rel` in `a`'s frame.
pub fn verdict(a: &Body, b: &Body, rel: Rigid) -> Verdict {
    match verdict_for(a, b, rel, &Job::default()) {
        Ok(v) => v,
        Err(_) => unreachable!("no job, no cancel"),
    }
}

/// Whether an intersection's box is too thin to hold a volume: under ten
/// resolutions across in some direction, which only faces the two solids
/// share give.
fn flat(min: [f64; 3], max: [f64; 3]) -> bool {
    (0..3).any(|k| max[k] - min[k] < 10.0 * ryukei::tol::LINEAR_RESOLUTION)
}

/// Whether points lie within ten resolutions of one another along some
/// direction: the result's mesh along one of its planar faces' normals.
/// A face the two solids share holds no volume whichever way it lies, and
/// its box is flat only when it lies square to an axis (#526).
fn thin(points: &[Point3], normals: &[[f64; 3]]) -> bool {
    normals.iter().any(|n| {
        let along = points.iter().map(|p| p.x * n[0] + p.y * n[1] + p.z * n[2]);
        let (lo, hi) = along.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), d| {
            (lo.min(d), hi.max(d))
        });
        hi - lo < 10.0 * ryukei::tol::LINEAR_RESOLUTION
    })
}

/// [`mesh::verdict`] of two prepared meshes, `None` past its budget: how
/// a pair with a reference side is judged, public for tests.
pub fn mesh_verdict(a: &Prepared, b: &Prepared, rel: &Rigid) -> Option<Verdict> {
    let job = Job::default();
    mesh::verdict(a, b, rel, &mut mesh::Search::new(&job)).ok()
}

/// [`verdict`] for a request's `job`: a cancel stops the kernel inside the
/// pair, and the pair answers the job's `cancelled` (never a verdict).
pub fn verdict_for(a: &Body, b: &Body, rel: Rigid, job: &Job) -> Result<Verdict> {
    let work = crate::work::work(job);
    let tool = Tool {
        key: ToolKey(OpKey::from_bytes(b"ryucad.interference\0b").0),
        body: b,
        place: rel,
    };
    let key = OpKey::from_bytes(b"ryucad.interference");
    let (body, journal) =
        match ryukei::boolean::boolean_with(key, BooleanOp::Intersect, a, &[tool], &work) {
            Ok(r) => r,
            Err(ryukei::Error::InvalidBoolean(ryukei::BooleanError::NonManifold { .. })) => {
                return Ok(Verdict::Touching);
            }
            Err(e) => {
                if let Some(e) = crate::work::stopped(&e, job) {
                    return Err(e);
                }
                return Ok(Verdict::CannotCheck {
                    kernel: format!("{e:?}"),
                });
            }
        };
    if body.faces().next().is_none() {
        return Ok(Verdict::Clear);
    }
    // Sized by its faces' boxes, which may hold whole spheres or circles;
    // its own box is its mesh's.
    let loose = body
        .faces()
        .map(|f| ryukei::intersect::face_box(&body, f))
        .reduce(|x, y| x.union(&y))
        .expect("a face");
    let diagonal = (loose.max() - loose.min()).length();
    let m = match ryukei::mesh::tessellate_with(&body, &Quality::Normal.params(diagonal), &work) {
        Ok(m) => m,
        Err(e) => {
            if let Some(e) = crate::work::stopped(&e, job) {
                return Err(e);
            }
            return Ok(Verdict::CannotCheck {
                kernel: format!("{e:?}"),
            });
        }
    };
    let mut volume_m3 = 0.0;
    for [i, j, k] in m.welded_triangles() {
        let [p, q, r] = [i, j, k].map(|n| m.positions[n as usize].to_vector());
        volume_m3 += p.dot(q.cross(r)) / 6.0;
    }
    let bx = Box3::from_points(m.positions.iter().copied()).expect("a face has points");
    let (lo, hi) = (bx.min(), bx.max());
    let normals: Vec<[f64; 3]> = body
        .faces()
        .filter_map(|f| match body.surface(body.face(f)?.surface)? {
            ryukei::geom::Surface::Plane(p) => Some(p.frame().z().get().to_array()),
            _ => None,
        })
        .collect();
    if flat([lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z]) || thin(&m.positions, &normals) {
        // Faces both sides hold (a file's tolerant bodies sharing a face):
        // the result holds no volume, so the pair touches (#523).
        return Ok(Verdict::Touching);
    }
    // Each face of the result is a piece of a face of one side: followed
    // back through the journal to the name it had there.
    let own_a: BTreeSet<Name> = a.faces().map(|f| a.face(f).expect("live").name).collect();
    let mut back: BTreeMap<Name, Vec<Name>> = BTreeMap::new();
    let mut placed: BTreeMap<Name, Name> = BTreeMap::new();
    for e in journal.events() {
        match e {
            Event::Split { parent, children } => {
                for c in children {
                    back.entry(*c).or_default().push(*parent);
                }
            }
            Event::Merged { parents, child } => {
                back.entry(*child)
                    .or_default()
                    .extend(parents.iter().copied());
            }
            Event::Placed { from, to, .. } => {
                placed.insert(*to, *from);
            }
            _ => {}
        }
    }
    let (mut faces_a, mut faces_b) = (BTreeSet::new(), BTreeSet::new());
    for f in body.faces() {
        let mut todo = vec![body.face(f).expect("live").name];
        let mut seen = BTreeSet::new();
        while let Some(n) = todo.pop() {
            if !seen.insert(n) {
                continue;
            }
            if own_a.contains(&n) {
                faces_a.insert(n);
            } else if let Some(from) = placed.get(&n) {
                faces_b.insert(*from);
            } else if let Some(ps) = back.get(&n) {
                todo.extend(ps.iter().copied());
            }
        }
    }
    Ok(Verdict::Interferes {
        min: [lo.x, lo.y, lo.z],
        max: [hi.x, hi.y, hi.z],
        volume_m3,
        faces_a: faces_a.into_iter().collect(),
        faces_b: faces_b.into_iter().collect(),
    })
}

/// Whether occurrence `o` lies under one of `only` (instance paths, as
/// references write them); everything does when `only` is empty.
pub(crate) fn under(o: &Occurrence, only: &[Vec<Id>]) -> bool {
    only.is_empty() || only.iter().any(|p| o.path.starts_with(p))
}

/// Which solids of `model` (assembly `asm`'s) interfere, from `cache` and
/// what it lacks, worked out in parallel and added to it. With `only`,
/// just the pairs with a side under one of those instance paths, each of
/// which must place something (`not_found` otherwise).
pub fn find(asm: &Id, model: &Model, only: &[Vec<Id>], cache: &mut Cache) -> Result<Value> {
    find_for(asm, model, only, cache, &Job::default())
}

/// [`find`] for a request's `job`: each pair it intersects is a
/// boundary. Cancelled, it keeps the verdicts it reached and answers
/// `cancelled`.
pub fn find_for(
    asm: &Id,
    model: &Model,
    only: &[Vec<Id>],
    cache: &mut Cache,
    job: &Job,
) -> Result<Value> {
    for p in only {
        if !model.occurrences.iter().any(|o| o.path.starts_with(p)) {
            return Err(Error::new(
                Code::NotFound,
                format!("{asm} has no instance {}", path_text(p)),
            )
            .with("part", asm)
            .with("instance", path_text(p)));
        }
    }
    let mut pieces = Vec::new();
    // Each body's box once, however many occurrences place it.
    let mut local: HashMap<*const Built, Option<Box3>> = HashMap::new();
    for (i, o) in model.occurrences.iter().enumerate() {
        for s in &o.solids {
            // A sheet shares no volume: interference pairs solids, and
            // reference imports, which are judged on their meshes.
            if !s.built.reference && s.built.body.kind() != ryukei::topo::BodyKind::Solid {
                continue;
            }
            let own = *local
                .entry(Arc::as_ptr(&s.built))
                .or_insert_with(|| body_box(&s.built.body));
            if let Some(world) = own.map(|b| b.transform_rigid(&o.pose)) {
                pieces.push((
                    i,
                    Piece {
                        occurrence: o,
                        founder: &s.founder,
                        built: &s.built,
                        world,
                    },
                ));
            }
        }
    }
    // Pairs whose boxes overlap, found through a grid of cells (a sweep
    // along x is quadratic when occurrences line up across x, as 100,000
    // in subassemblies laid out alike do).
    let boxes: Vec<Box3> = pieces.iter().map(|(_, p)| p.world).collect();
    let mut pairs: Vec<(usize, usize)> = overlapping(&boxes)
        .into_iter()
        .filter(|&(a, b)| {
            let ((oa, x), (ob, y)) = (&pieces[a], &pieces[b]);
            oa != ob && (under(x.occurrence, only) || under(y.occurrence, only))
        })
        .collect();
    pairs.sort_unstable();
    let keyed: Vec<(usize, usize, Rigid, String)> = pairs
        .into_iter()
        .map(|(a, b)| {
            let (x, y) = (&pieces[a].1, &pieces[b].1);
            let rel = relative(x.occurrence, y.occurrence);
            let k = key(x.built, y.built, &rel);
            (a, b, rel, k)
        })
        .collect();
    let missing: BTreeMap<&str, (usize, usize, Rigid)> = keyed
        .iter()
        .filter(|(_, _, _, k)| !cache.verdicts.contains_key(k))
        .map(|(a, b, rel, k)| (k.as_str(), (*a, *b, *rel)))
        .collect();
    // Each body a missing pair with a reference side needs, meshed once.
    // Keyed by address (as usize: a raw pointer isn't Send).
    let mut need: BTreeMap<usize, &Built> = BTreeMap::new();
    for (a, b, _) in missing.values() {
        let (x, y) = (pieces[*a].1.built, pieces[*b].1.built);
        if x.reference || y.reference {
            need.insert(x as *const Built as usize, x);
            need.insert(y as *const Built as usize, y);
        }
    }
    let work = crate::work::work(job);
    let need: Vec<(usize, &Built)> = need.into_iter().collect();
    let meshed: Vec<(usize, std::result::Result<Arc<Prepared>, ryukei::Error>)> = need
        .into_par_iter()
        .map(|(k, b)| (k, Prepared::new_for(&b.body, &work).map(Arc::new)))
        .collect();
    let mut prepared: HashMap<usize, std::result::Result<Arc<Prepared>, String>> = HashMap::new();
    for (k, r) in meshed {
        let r = match r {
            Ok(p) => Ok(p),
            Err(e) => match crate::work::stopped(&e, job) {
                Some(stop) => return Err(stop),
                None => Err(format!("{e:?}")),
            },
        };
        prepared.insert(k, r);
    }
    let stage = job.begin(Phase::Intersect, "pairs", missing.len());
    let worked: Vec<Result<(String, Verdict)>> = missing
        .into_par_iter()
        .map(|(k, (a, b, rel))| {
            let (x, y) = (&pieces[a].1, &pieces[b].1);
            stage.step(|| {
                let at = |p: &Piece| format!("{}/{}", path_text(&p.occurrence.path), p.founder);
                format!("{} {}", at(x), at(y))
            })?;
            let v = if x.built.reference || y.built.reference {
                let get = |b: &Built| prepared[&(b as *const Built as usize)].clone();
                match (get(x.built), get(y.built)) {
                    (Ok(ma), Ok(mb)) => {
                        let mut search = mesh::Search::new(job);
                        match mesh::verdict(&ma, &mb, &rel, &mut search) {
                            Ok(v) => v,
                            Err(mesh::Halt::OverBudget) => Verdict::CannotCheck {
                                kernel: "mesh search past its budget".to_owned(),
                            },
                            Err(mesh::Halt::Cancelled) => {
                                return Err(crate::work::stopped(&ryukei::Error::Cancelled, job)
                                    .expect("a cancel is stopped"));
                            }
                        }
                    }
                    (Err(e), _) | (_, Err(e)) => Verdict::CannotCheck { kernel: e },
                }
            } else {
                verdict_for(&x.built.body, &y.built.body, rel, job)?
            };
            Ok((k.to_owned(), v))
        })
        .collect();
    drop(stage);
    let mut stopped = None;
    for r in worked {
        match r {
            Ok((k, v)) => {
                cache.verdicts.insert(k, Arc::new(v));
            }
            Err(e) => {
                stopped.get_or_insert(e);
            }
        }
    }
    if let Some(e) = stopped {
        return Err(e);
    }
    let solid = |p: &Piece| format!("{asm}/{}/{}", path_text(&p.occurrence.path), p.founder);
    let (mut interferences, mut touching, mut unchecked) = (Vec::new(), Vec::new(), Vec::new());
    let mut clear = 0;
    for (a, b, _, k) in &keyed {
        let (x, y) = (&pieces[*a].1, &pieces[*b].1);
        let (sa, sb) = (solid(x), solid(y));
        match cache.verdicts[k].as_ref() {
            Verdict::Clear => clear += 1,
            Verdict::Touching => touching.push(json!({"a": sa, "b": sb})),
            Verdict::CannotCheck { kernel } => {
                unchecked.push(json!({"a": sa, "b": sb, "kernel": kernel}));
            }
            Verdict::Approximate {
                min,
                max,
                open,
                faces_a,
                faces_b,
            } => {
                let p = |c: &[f64; 3]| Point3::new(c[0], c[1], c[2]);
                let at = Box3::new(p(min), p(max)).transform_rigid(&x.occurrence.pose);
                let (lo, hi) = (at.min(), at.max());
                let names = |s: &str, ns: &[Name]| -> Vec<String> {
                    ns.iter().map(|n| format!("{s}/{n}")).collect()
                };
                let mut i = json!({
                    "a": sa, "b": sb, "volume_m3": null, "approximate": true,
                    "box": {"min_m": [lo.x, lo.y, lo.z], "max_m": [hi.x, hi.y, hi.z]},
                    "faces": {"a": names(&sa, faces_a), "b": names(&sb, faces_b)},
                });
                if *open {
                    i["open"] = json!(true);
                }
                interferences.push(i);
            }
            Verdict::Interferes {
                min,
                max,
                volume_m3,
                faces_a,
                faces_b,
            } => {
                // The box in the assembly's frame.
                let p = |c: &[f64; 3]| Point3::new(c[0], c[1], c[2]);
                let at = Box3::new(p(min), p(max)).transform_rigid(&x.occurrence.pose);
                let (lo, hi) = (at.min(), at.max());
                let names = |s: &str, ns: &[Name]| -> Vec<String> {
                    ns.iter().map(|n| format!("{s}/{n}")).collect()
                };
                interferences.push(json!({
                    "a": sa, "b": sb,
                    "volume_m3": volume_m3,
                    "box": {"min_m": [lo.x, lo.y, lo.z], "max_m": [hi.x, hi.y, hi.z]},
                    "faces": {"a": names(&sa, faces_a), "b": names(&sb, faces_b)},
                }));
            }
        }
    }
    Ok(json!({
        "part": asm,
        "occurrences": model.occurrences.len(),
        "pairs": keyed.len(),
        "clear": clear,
        "interferences": interferences,
        "touching": touching,
        "unchecked": unchecked,
        "volume": "estimated from each intersection's mesh at normal quality; null where judged on meshes (approximate: a reference import's side)",
    }))
}

/// Instance paths under assembly `pid` (`s1/b2`, `pattern:2/bolt`), as
/// `interference` and `clearance` take them.
pub(crate) fn instance_paths(pid: &Id, instances: &[String]) -> Result<Vec<Vec<Id>>> {
    instances
        .iter()
        .map(|i| {
            let r = crate::entity::EntityRef::parse(&format!("{pid}/{i}"))?.in_assembly();
            if !r.is_occurrence() {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{i:?} isn't an instance path; use <instance>/… under {pid}"),
                )
                .with("instance", i));
            }
            Ok(r.path)
        })
        .collect()
}

/// `interference`: which solids of assembly `part`'s occurrences overlap,
/// or with `instances` (paths under the assembly, as `s1/b2` or
/// `pattern:2/bolt`), those of a pair with a side under one of them.
pub fn run(
    session: &mut crate::session::Session,
    part: &str,
    instances: &[String],
) -> Result<Value> {
    let pid = crate::server::part_id(part)?;
    let p = session.project().part(&pid)?;
    if crate::server::part_kind(p) != crate::doc::files::PartKind::Assembly {
        return Err(Error::new(
            Code::WrongPartKind,
            format!("{pid} is a part, not an assembly"),
        )
        .with("part", &pid));
    }
    let only = instance_paths(&pid, instances)?;
    let model = session.model(&pid, &crate::doc::assembly::Rev::Working)?;
    let revision = session.project().part(&pid)?.revision.clone();
    let job = session.job.clone();
    let mut v = find_for(&pid, &model, &only, &mut session.interference, &job)?;
    v["revision"] = json!(revision);
    // Poses a failed mate left where a solve without it put them make
    // clashes that aren't the design's (issue #155).
    let mut assemblies = vec![pid.clone()];
    assemblies.extend(crate::assembly::placed_assemblies(session.project(), &pid));
    let warnings = crate::mate::failing_warnings(
        session,
        &assemblies,
        "; interference was found at those poses",
    );
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::assembly::tests::{place_in, project};
    use crate::doc::project::write_json;
    use serde_json::json;

    const MM3: f64 = 1e-9;

    #[test]
    fn the_grid_finds_every_pair_a_brute_force_does() {
        // Small boxes in rows that line up across x, and a few long ones.
        let mut g = ryukei_testkit::Lcg::new(192);
        let mut boxes = Vec::new();
        for k in 0..600 {
            let at = [
                g.next_f64(0.0, 0.3),
                g.next_f64(0.0, 0.3),
                g.next_f64(0.0, 0.05),
            ];
            let size = if k % 100 == 0 {
                0.5
            } else {
                g.next_f64(0.001, 0.03)
            };
            let a = Point3::new(at[0], at[1], at[2]);
            let b = Point3::new(at[0] + size, at[1] + size / 2.0, at[2] + size / 3.0);
            boxes.push(Box3::new(a, b));
        }
        let mut brute = Vec::new();
        for a in 0..boxes.len() {
            for b in a + 1..boxes.len() {
                if boxes[a].intersects(&boxes[b]) {
                    brute.push((a, b));
                }
            }
        }
        assert!(brute.len() > 600, "{}", brute.len());
        assert_eq!(overlapping(&boxes), brute);
        assert!(overlapping(&[]).is_empty());
    }

    /// [`run`] after reading the files again, as every request does,
    /// with how many pairs it intersected (`computed`).
    fn find_in(s: &mut crate::session::Session, part: &str, only: &[String]) -> Result<Value> {
        s.refresh()?;
        let before = s.interference.len();
        let mut v = run(s, part, only)?;
        v["computed"] = json!(s.interference.len() - before);
        Ok(v)
    }

    #[test]
    fn a_bolt_through_a_plate_interferes_by_their_common_volume() {
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "p1",
            "plate",
            json!("working"),
            [0.0; 3],
        );
        place_in(
            dir.path(),
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.01, 0.0],
        );
        let v = find_in(&mut s, "frame", &[]).unwrap();
        assert_eq!(v["pairs"], 1, "{v}");
        let i = &v["interferences"][0];
        assert_eq!(i["a"], "frame/p1/slab");
        assert_eq!(i["b"], "frame/b1/shank");
        // A cylinder of radius 3 mm through 5 mm of plate.
        let want = core::f64::consts::PI * 9.0 * 5.0 * MM3;
        let got = i["volume_m3"].as_f64().unwrap();
        assert!((got - want).abs() < 1e-3 * want, "{got} against {want}");
        let lo: Vec<f64> = serde_json::from_value(i["box"]["min_m"].clone()).unwrap();
        let hi: Vec<f64> = serde_json::from_value(i["box"]["max_m"].clone()).unwrap();
        assert!(
            (lo[2]).abs() < 1e-9 && (hi[2] - 0.005).abs() < 1e-9,
            "{lo:?} {hi:?}"
        );
        // The plate's top and bottom, and the bolt's side: its bottom lies
        // on the plate's, which the intersection keeps.
        assert_eq!(i["faces"]["a"].as_array().unwrap().len(), 2, "{i}");
        assert_eq!(i["faces"]["b"].as_array().unwrap().len(), 1, "{i}");
        for f in i["faces"]["a"].as_array().unwrap() {
            let r = f.as_str().unwrap();
            assert!(r.starts_with("frame/p1/slab/"), "{r}");
            let t = crate::entity::EntityRef::parse(r).unwrap().resolve(&mut s);
            assert_eq!(
                t.unwrap(),
                crate::entity::Target::Body(crate::entity::EntityKind::Face)
            );
        }
    }

    #[test]
    fn an_intersection_with_a_flat_box_touches() {
        // #523's pair: two bodies of the NUCLEO PCBA sharing a face, their
        // intersection's box 1.15 mm by nothing by 0.1 mm.
        let (min, max) = (
            [0.064900021370781, 0.058560005087188, 0.0],
            [0.066050021610781, 0.058560005087188, 0.0000999998],
        );
        assert!(flat(min, max));
        // A pad standing 2 µm into a board is an interference.
        assert!(!flat([0.0; 3], [1e-3, 1e-3, 2e-6]));
    }

    #[test]
    fn a_shared_face_lying_askew_touches() {
        // A face two solids share, tilted 30° about x: its box is wide every
        // way, but along its normal it holds no volume (#526).
        let (s, c) = (0.5, 0.75f64.sqrt());
        let normal = [0.0, -s, c];
        let at = |u: f64, v: f64, h: f64| Point3::new(u, v * c - h * s, v * s + h * c);
        let square = [
            at(0.0, 0.0, 0.0),
            at(1e-3, 0.0, 0.0),
            at(1e-3, 1e-3, 0.0),
            at(0.0, 1e-3, 0.0),
        ];
        let (lo, hi) = (
            Box3::from_points(square).unwrap().min(),
            Box3::from_points(square).unwrap().max(),
        );
        assert!(!flat([lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z]));
        assert!(thin(&square, &[normal]));
        // The same tilted, 2 µm thick, is an interference.
        let mut slab = square.to_vec();
        slab.extend([at(0.0, 0.0, 2e-6), at(1e-3, 1e-3, 2e-6)]);
        assert!(!thin(&slab, &[normal]));
        // With no planar face, nothing is judged thin.
        assert!(!thin(&square, &[]));
    }

    /// Two 10 mm blocks, the second standing on the first's top and sunk
    /// `d` into it, its footprint `inset` in from each side, both turned
    /// by `turn` about x: their verdict.
    fn sunk(d: f64, turn: f64, inset: f64) -> Verdict {
        use ryukei::geom::{UnitVector3, Vector3};
        let side = 0.01;
        let r = Rigid::rotation(UnitVector3::X, turn);
        let key = |k: &[u8]| OpKey::from_bytes(k);
        let (a, _) = ryukei::primitive::block(key(b"a"), side, side, side, &r).unwrap();
        let foot = side - 2.0 * inset;
        let (b, _) =
            ryukei::primitive::block(key(b"b"), foot, foot, side, &Rigid::IDENTITY).unwrap();
        let rel = r.then_after(&Rigid::translation(Vector3::new(inset, inset, side - d)));
        verdict(&a, &b, rel)
    }

    #[test]
    fn blocks_sunk_less_than_a_touch_are_never_unchecked() {
        // #558: flush blocks sunk 30 to 80 nm into each other across a
        // face turned 30° were left unchecked (`Uncertain`). Under a
        // hundred nanometres they meet only within the bodies'
        // tolerances: clear (or touching), never unchecked. (ryukei's
        // `booleans` test takes them square too.)
        let turn = std::f64::consts::PI / 6.0;
        for (d, inset) in [
            (10e-9, 0.0),
            (30e-9, 0.0),
            (50e-9, 0.0),
            (80e-9, 0.0),
            (50e-9, 1e-3),
        ] {
            let v = sunk(d, turn, inset);
            assert!(
                matches!(v, Verdict::Clear | Verdict::Touching),
                "d {d} inset {inset}: {v:?}"
            );
        }
        // Past the touch reach the overlap is an interference of the
        // slab's volume.
        match sunk(2e-6, turn, 0.0) {
            Verdict::Interferes { volume_m3, .. } => {
                let want = 1e-4 * 2e-6;
                assert!((volume_m3 - want).abs() <= 1e-2 * want, "{volume_m3}");
            }
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn touching_and_apart_are_clear() {
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "p1",
            "plate",
            json!("working"),
            [0.0; 3],
        );
        // Standing on the plate's top, and well away from it.
        place_in(
            dir.path(),
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.01, 0.005],
        );
        place_in(
            dir.path(),
            "frame",
            "b2",
            "bolt",
            json!("working"),
            [0.5, 0.0, 0.0],
        );
        let v = find_in(&mut s, "frame", &[]).unwrap();
        assert_eq!(v["pairs"], 1, "{v}");
        assert_eq!(v["clear"], 1, "{v}");
        assert_eq!(v["interferences"], json!([]));
        assert_eq!(v["unchecked"], json!([]));
    }

    #[test]
    fn plates_overlapping_by_a_block() {
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "p1",
            "plate",
            json!("working"),
            [0.0; 3],
        );
        place_in(
            dir.path(),
            "frame",
            "p2",
            "plate",
            json!("working"),
            [0.05, 0.025, 0.002],
        );
        let v = find_in(&mut s, "frame", &[]).unwrap();
        let got = v["interferences"][0]["volume_m3"].as_f64().unwrap();
        // 50 × 25 × 3 mm, meshed exactly: every face is planar.
        assert!((got - 3750.0 * MM3).abs() < 1e-12 * 3750.0, "{got}");
    }

    #[test]
    fn verdicts_are_kept_by_bodies_and_how_they_sit() {
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "p1",
            "plate",
            json!("working"),
            [0.0; 3],
        );
        place_in(
            dir.path(),
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.01, 0.0],
        );
        place_in(
            dir.path(),
            "frame",
            "b2",
            "bolt",
            json!("working"),
            [0.03, 0.01, 0.0],
        );
        let v = find_in(&mut s, "frame", &[]).unwrap();
        assert_eq!(
            (v["pairs"].as_u64(), v["computed"].as_u64()),
            (Some(2), Some(2)),
            "{v}"
        );
        // Asked again: nothing new.
        let v = find_in(&mut s, "frame", &[]).unwrap();
        assert_eq!(v["computed"], 0, "{v}");
        // A third, away from everything: nothing new.
        place_in(
            dir.path(),
            "frame",
            "b3",
            "bolt",
            json!("working"),
            [0.5, 0.0, 0.0],
        );
        let v = find_in(&mut s, "frame", &[]).unwrap();
        assert_eq!(
            (v["pairs"].as_u64(), v["computed"].as_u64()),
            (Some(2), Some(0)),
            "{v}"
        );
        assert_eq!(v["interferences"].as_array().unwrap().len(), 2);
        // A bolt moved: it sits against the plate as it didn't.
        let path = crate::doc::project::feature_path(
            dir.path(),
            &Id::parse("frame", "part").unwrap(),
            &Id::parse("b1", "feature").unwrap(),
        );
        let moved = json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                           "part": "bolt", "rev": "working",
                           "pose": {"origin_m": [0.01, 0.02, 0.0], "z": [0, 0, 1], "x": [1, 0, 0]}});
        write_json(&path, &moved).unwrap();
        let v = find_in(&mut s, "frame", &[]).unwrap();
        assert_eq!(
            (v["pairs"].as_u64(), v["computed"].as_u64()),
            (Some(2), Some(1)),
            "{v}"
        );
        // One into its neighbour: new against the plate, and against b2.
        place_in(
            dir.path(),
            "frame",
            "b4",
            "bolt",
            json!("working"),
            [0.032, 0.01, 0.0],
        );
        let v = find_in(&mut s, "frame", &[]).unwrap();
        assert_eq!(
            (v["pairs"].as_u64(), v["computed"].as_u64()),
            (Some(4), Some(2)),
            "{v}"
        );
        let pair = v["interferences"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["a"] == "frame/b2/shank")
            .unwrap();
        assert_eq!(pair["b"], "frame/b4/shank");
    }

    #[test]
    fn a_subassembly_far_out_relates_its_occurrences_as_it_does_alone() {
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "p1",
            "plate",
            json!("working"),
            [0.0; 3],
        );
        // Standing on the plate: faces in contact, clear.
        let on = [0.0137, 0.0211, 0.005];
        place_in(dir.path(), "frame", "b1", "bolt", json!("working"), on);
        let v = find_in(&mut s, "frame", &[]).unwrap();
        assert_eq!(
            (v["pairs"].as_u64(), v["computed"].as_u64()),
            (Some(1), Some(1))
        );
        assert_eq!(v["clear"], 1, "{v}");
        // The frame placed 150 m out: the same pair, through the frame, so
        // the same verdict from the cache.
        crate::server::tests::call(
            &mut s,
            "part.create",
            json!({"part": "site", "kind": "assembly"}),
        )
        .unwrap();
        let far = [137.31, -151.73, 3.17];
        place_in(dir.path(), "site", "f1", "frame", json!("working"), far);
        let v = find_in(&mut s, "site", &[]).unwrap();
        assert_eq!(
            (v["pairs"].as_u64(), v["computed"].as_u64()),
            (Some(1), Some(0)),
            "{v}"
        );
        assert_eq!(v["clear"], 1, "{v}");
    }

    #[test]
    fn instances_narrow_the_pairs() {
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "p1",
            "plate",
            json!("working"),
            [0.0; 3],
        );
        place_in(
            dir.path(),
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.01, 0.0],
        );
        place_in(
            dir.path(),
            "frame",
            "b2",
            "bolt",
            json!("working"),
            [0.03, 0.01, 0.0],
        );
        let v = find_in(&mut s, "frame", &["b2".to_owned()]).unwrap();
        assert_eq!(v["pairs"], 1, "{v}");
        assert_eq!(v["interferences"][0]["b"], "frame/b2/shank");
        let e = find_in(&mut s, "frame", &["b9".to_owned()]).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = find_in(&mut s, "bolt", &[]).unwrap_err();
        assert_eq!(e.code, Code::WrongPartKind);
    }
}
