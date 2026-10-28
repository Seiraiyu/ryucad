//! Interference: which occurrences of an assembly overlap. Every solid of
//! every occurrence is boxed where the assembly puts it; each pair of
//! solids of different occurrences whose boxes overlap is intersected by
//! ryukei, the second placed in the first's frame, so the answer depends
//! only on the two bodies and how one sits against the other. Those are
//! the cache's key: moving an unrelated instance recomputes nothing.
//!
//! An empty intersection is clear; so is one ryukei refuses as touching
//! itself along an edge or at a point (bodies meeting where a mate puts
//! them), reported as `touching`. A solid intersection is an
//! interference, with its box, its volume estimated from its mesh at
//! normal quality and the faces of each side it touches. Any other
//! refusal leaves the pair unchecked, saying why.

use crate::assembly::{Model, Occurrence, path_text};
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::mesh::Quality;
use crate::rebuild::Built;
use rayon::prelude::*;
use ryukei::boolean::{BooleanOp, Tool};
use ryukei::geom::{Box3, Point3, Rigid};
use ryukei::name::{Event, Name, OpKey, ToolKey};
use ryukei::topo::Body;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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
    body.faces()
        .map(|f| ryukei::intersect::face_box(body, f))
        .reduce(|a, b| a.union(&b))
        .map(|b| b.transform_rigid(pose))
}

/// Intersects `a` with `b` placed at `rel` in `a`'s frame.
pub fn verdict(a: &Body, b: &Body, rel: Rigid) -> Verdict {
    let tool = Tool {
        key: ToolKey(OpKey::from_bytes(b"ryucad.interference\0b").0),
        body: b,
        place: rel,
    };
    let key = OpKey::from_bytes(b"ryucad.interference");
    let (body, journal) = match ryukei::boolean::boolean(key, BooleanOp::Intersect, a, &[tool]) {
        Ok(r) => r,
        Err(ryukei::Error::InvalidBoolean(ryukei::BooleanError::NonManifold { .. })) => {
            return Verdict::Touching;
        }
        Err(e) => {
            return Verdict::CannotCheck {
                kernel: format!("{e:?}"),
            };
        }
    };
    if body.faces().next().is_none() {
        return Verdict::Clear;
    }
    // Sized by its faces' boxes, which may hold whole spheres or circles;
    // its own box is its mesh's.
    let loose = body
        .faces()
        .map(|f| ryukei::intersect::face_box(&body, f))
        .reduce(|x, y| x.union(&y))
        .expect("a face");
    let diagonal = (loose.max() - loose.min()).length();
    let m = match ryukei::mesh::tessellate(&body, &Quality::Normal.params(diagonal)) {
        Ok(m) => m,
        Err(e) => {
            return Verdict::CannotCheck {
                kernel: format!("{e:?}"),
            };
        }
    };
    let mut volume_m3 = 0.0;
    for [i, j, k] in m.welded_triangles() {
        let [p, q, r] = [i, j, k].map(|n| m.positions[n as usize].to_vector());
        volume_m3 += p.dot(q.cross(r)) / 6.0;
    }
    let bx = Box3::from_points(m.positions.iter().copied()).expect("a face has points");
    let (lo, hi) = (bx.min(), bx.max());
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
    Verdict::Interferes {
        min: [lo.x, lo.y, lo.z],
        max: [hi.x, hi.y, hi.z],
        volume_m3,
        faces_a: faces_a.into_iter().collect(),
        faces_b: faces_b.into_iter().collect(),
    }
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
    for (i, o) in model.occurrences.iter().enumerate() {
        for s in &o.solids {
            if let Some(world) = world_box(&s.built.body, &o.pose) {
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
    // Pairs whose boxes overlap, by sweeping along x: in the model's
    // order, the first side first.
    let mut by_x: Vec<usize> = (0..pieces.len()).collect();
    by_x.sort_by(|&a, &b| {
        let (pa, pb) = (pieces[a].1.world.min().x, pieces[b].1.world.min().x);
        pa.total_cmp(&pb).then(a.cmp(&b))
    });
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (k, &a) in by_x.iter().enumerate() {
        let hi = pieces[a].1.world.max().x;
        for &b in &by_x[k + 1..] {
            if pieces[b].1.world.min().x > hi {
                break;
            }
            let ((oa, x), (ob, y)) = (&pieces[a], &pieces[b]);
            if oa == ob || !x.world.intersects(&y.world) {
                continue;
            }
            if !under(x.occurrence, only) && !under(y.occurrence, only) {
                continue;
            }
            pairs.push((a.min(b), a.max(b)));
        }
    }
    pairs.sort_unstable();
    let keyed: Vec<(usize, usize, Rigid, String)> = pairs
        .into_iter()
        .map(|(a, b)| {
            let (x, y) = (&pieces[a].1, &pieces[b].1);
            let rel = x.occurrence.pose.inverse().then_after(&y.occurrence.pose);
            let k = key(x.built, y.built, &rel);
            (a, b, rel, k)
        })
        .collect();
    let missing: BTreeMap<&str, (usize, usize, Rigid)> = keyed
        .iter()
        .filter(|(_, _, _, k)| !cache.verdicts.contains_key(k))
        .map(|(a, b, rel, k)| (k.as_str(), (*a, *b, *rel)))
        .collect();
    let worked: Vec<(String, Verdict)> = missing
        .into_par_iter()
        .map(|(k, (a, b, rel))| {
            let v = verdict(&pieces[a].1.built.body, &pieces[b].1.built.body, rel);
            (k.to_owned(), v)
        })
        .collect();
    for (k, v) in worked {
        cache.verdicts.insert(k, Arc::new(v));
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
        "volume": "estimated from each intersection's mesh at normal quality",
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
    let mut v = find(&pid, &model, &only, &mut session.interference)?;
    v["revision"] = json!(revision);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::tests::{place_in, project};
    use crate::doc::project::write_json;
    use serde_json::json;

    const MM3: f64 = 1e-9;

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
