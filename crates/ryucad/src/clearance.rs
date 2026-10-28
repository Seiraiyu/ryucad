//! `clearance`: the minimum distance between the solids of an assembly's
//! occurrences, with the nearest points, from ryukei's
//! [`ryukei::measure::distance`].
//!
//! Pairs come from [`crate::interference`]'s box sweep along x, grown by
//! `below` (by default a hundredth of the assembly's diagonal): every pair
//! of solids of different occurrences whose boxes come within it. Pairs
//! at 0 touch or interfere; `interference` says which. Distances are kept
//! by the session by both bodies' hashes and the bits of their relative
//! pose, as interference verdicts are.

use crate::assembly::{Model, Occurrence, path_text};
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::interference::{key, under, world_box};
use crate::measure::{Kind, point, value};
use crate::rebuild::Built;
use crate::units::Unit;
use rayon::prelude::*;
use ryukei::geom::{Box3, Rigid};
use ryukei::measure::{self as kernel, DistanceOptions, Entity, Value as Measured};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What a pair's distance came to: the distance in the first solid's
/// frame, or why ryukei refused it.
type Found = std::result::Result<kernel::Distance, String>;

/// Distances already worked out, by pair key.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    found: BTreeMap<String, Arc<Found>>,
}

impl Cache {
    pub fn len(&self) -> usize {
        self.found.len()
    }

    pub fn is_empty(&self) -> bool {
        self.found.is_empty()
    }
}

struct Piece<'m> {
    occurrence: &'m Occurrence,
    founder: &'m Id,
    built: &'m Built,
    world: Box3,
}

fn gap(a: &Box3, b: &Box3) -> f64 {
    let along = |lo1: f64, hi1: f64, lo2: f64, hi2: f64| (lo2 - hi1).max(lo1 - hi2).max(0.0);
    let (a0, a1, b0, b1) = (a.min(), a.max(), b.min(), b.max());
    let d = [
        along(a0.x, a1.x, b0.x, b1.x),
        along(a0.y, a1.y, b0.y, b1.y),
        along(a0.z, a1.z, b0.z, b1.z),
    ];
    ryukei::math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
}

/// The clearances of `model` (assembly `asm`'s) between pairs whose
/// boxes come within `below` meters (`None`: a hundredth of the
/// assembly's diagonal), with a side under one of `only`'s paths when
/// there are any.
pub fn find(
    asm: &Id,
    model: &Model,
    only: &[Vec<Id>],
    below: Option<f64>,
    unit: Unit,
    cache: &mut Cache,
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
    let mut pieces: Vec<(usize, Piece)> = Vec::new();
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
    let below = below.unwrap_or_else(|| {
        pieces
            .iter()
            .map(|(_, p)| p.world)
            .reduce(|a, b| a.union(&b))
            .map_or(0.0, |b| (b.max() - b.min()).length() / 100.0)
    });
    let mut by_x: Vec<usize> = (0..pieces.len()).collect();
    by_x.sort_by(|&a, &b| {
        let (pa, pb) = (pieces[a].1.world.min().x, pieces[b].1.world.min().x);
        pa.total_cmp(&pb).then(a.cmp(&b))
    });
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (k, &a) in by_x.iter().enumerate() {
        let hi = pieces[a].1.world.max().x + below;
        for &b in &by_x[k + 1..] {
            if pieces[b].1.world.min().x > hi {
                break;
            }
            let ((oa, x), (ob, y)) = (&pieces[a], &pieces[b]);
            if oa == ob || gap(&x.world, &y.world) > below {
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
            (a, b, rel, key(x.built, y.built, &rel))
        })
        .collect();
    let missing: BTreeMap<&str, (usize, usize, Rigid)> = keyed
        .iter()
        .filter(|(_, _, _, k)| !cache.found.contains_key(k))
        .map(|(a, b, rel, k)| (k.as_str(), (*a, *b, *rel)))
        .collect();
    let worked: Vec<(String, Found)> = missing
        .into_par_iter()
        .map(|(k, (a, b, rel))| {
            let d = kernel::distance(
                (&pieces[a].1.built.body, Entity::Body, &Rigid::IDENTITY),
                (&pieces[b].1.built.body, Entity::Body, &rel),
                &DistanceOptions::default(),
            );
            (k.to_owned(), d.map_err(|e| e.to_string()))
        })
        .collect();
    for (k, d) in worked {
        cache.found.insert(k, Arc::new(d));
    }
    let solid = |p: &Piece| format!("{asm}/{}/{}", path_text(&p.occurrence.path), p.founder);
    let mut clearances: Vec<(f64, Value)> = Vec::new();
    let mut unchecked = Vec::new();
    for (a, b, _, k) in &keyed {
        let (x, y) = (&pieces[*a].1, &pieces[*b].1);
        let (sa, sb) = (solid(x), solid(y));
        match cache.found[k].as_ref() {
            Err(kernel) => unchecked.push(json!({"a": sa, "b": sb, "kernel": kernel})),
            Ok(d) => {
                let pose = &x.occurrence.pose;
                let mut v = json!({
                    "a": sa,
                    "b": sb,
                    "distance": value(d.distance, Kind::Length, unit),
                    "points": [point(pose.apply_point(d.points[0]), unit),
                               point(pose.apply_point(d.points[1]), unit)],
                });
                if d.distance == Measured::Exact(0.0) {
                    v["contact"] = json!(true);
                }
                clearances.push((d.distance.value(), v));
            }
        }
    }
    // Closest first; ties in the model's order.
    clearances.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(json!({
        "part": asm,
        "occurrences": model.occurrences.len(),
        "below": value(Measured::Exact(below), Kind::Length, unit),
        "pairs": keyed.len(),
        "clearances": clearances.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "unchecked": unchecked,
        "contact": "pairs at 0 touch or interfere: ryucad interference says which",
    }))
}

/// `clearance`: assembly `part`'s clearances, or with `instances` those
/// of pairs with a side under one of them; `below` a length (default
/// unit) the pairs' boxes come within.
pub fn run(
    session: &mut crate::session::Session,
    part: &str,
    instances: &[String],
    below: Option<&str>,
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
    let only = crate::interference::instance_paths(&pid, instances)?;
    let unit = session.project().manifest.default_unit;
    let below = below
        .map(|b| {
            let q = crate::units::Quantity::parse(b, unit)?;
            if q.meters() < 0.0 {
                return Err(
                    Error::new(Code::InvalidParam, format!("below {b:?} is negative"))
                        .with("below", b),
                );
            }
            Ok(q.meters())
        })
        .transpose()?;
    let model = session.model(&pid, &crate::doc::assembly::Rev::Working)?;
    let revision = session.project().part(&pid)?.revision.clone();
    let mut v = find(&pid, &model, &only, below, unit, &mut session.clearance)?;
    v["revision"] = json!(revision);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use crate::assembly::tests::{place_in, project};
    use crate::error::Code;
    use crate::server::tests::call;
    use serde_json::json;

    #[test]
    fn pairs_come_closest_first_and_contacts_are_zero() {
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "p1",
            "plate",
            json!("working"),
            [0.0, 0.0, 0.0],
        );
        // Standing on the plate, 10 mm past its end, and far away.
        place_in(
            dir.path(),
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.05, 0.025, 0.005],
        );
        place_in(
            dir.path(),
            "frame",
            "b2",
            "bolt",
            json!("working"),
            [0.113, 0.025, 0.0],
        );
        place_in(
            dir.path(),
            "frame",
            "b3",
            "bolt",
            json!("working"),
            [2.0, 0.0, 0.0],
        );
        s.refresh().unwrap();
        let v = call(&mut s, "clearance", json!({"part": "frame"})).unwrap();
        let c = v["clearances"].as_array().unwrap();
        // The default reach (a hundredth of the diagonal, about 20 mm)
        // takes the two bolts by the plate, not the far one, nor the two
        // bolts 57 mm apart.
        assert_eq!(c.len(), 2, "{v}");
        assert_eq!(
            (&c[0]["a"], &c[0]["b"]),
            (&json!("frame/p1/slab"), &json!("frame/b1/shank"))
        );
        assert_eq!(c[0]["distance"]["exact"], true, "{v}");
        assert_eq!(c[0]["contact"], true);
        let d = c[1]["distance"]["m"].as_f64().unwrap();
        assert!((d - 0.01).abs() < 1e-9, "{v}");
        assert_eq!(c[1]["b"], "frame/b2/shank");
        // Asking again works out nothing new.
        let n = s.clearance.len();
        assert_eq!(
            call(&mut s, "clearance", json!({"part": "frame"})).unwrap(),
            v
        );
        assert_eq!(s.clearance.len(), n);
        let v = call(
            &mut s,
            "clearance",
            json!({"part": "frame", "below": "3 m"}),
        )
        .unwrap();
        assert_eq!(v["clearances"].as_array().unwrap().len(), 6, "{v}");
        let v = call(
            &mut s,
            "clearance",
            json!({"part": "frame", "instances": ["b3"], "below": "3 m"}),
        )
        .unwrap();
        assert_eq!(v["clearances"].as_array().unwrap().len(), 3, "{v}");
    }

    #[test]
    fn refusals() {
        let (_dir, mut s) = project();
        assert_eq!(
            call(&mut s, "clearance", json!({"part": "plate"}))
                .unwrap_err()
                .code,
            Code::WrongPartKind
        );
        assert_eq!(
            call(
                &mut s,
                "clearance",
                json!({"part": "frame", "instances": ["x"]})
            )
            .unwrap_err()
            .code,
            Code::NotFound
        );
        assert_eq!(
            call(
                &mut s,
                "clearance",
                json!({"part": "frame", "below": "-1 mm"})
            )
            .unwrap_err()
            .code,
            Code::InvalidParam
        );
    }
}
