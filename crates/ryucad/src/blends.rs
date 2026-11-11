//! Blends as ryucad reads them (design 445, plan 22.B): ryukei's
//! recognition of a body's fillets, chamfers and ball corners in ryucad's
//! terms: their sizes, the closure a `delete_blend` takes, what a blend
//! edit's report keeps (names as hex, lengths in metres) and how output
//! shows it (references and quantities), and the `blends.get` query with
//! its cache.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::units::Unit;
use ryukei::blend::{BlendKind, Chamfer};
use ryukei::name::Name;
use ryukei::tweak::Recognition;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A blend's size: a fillet's radius, a chamfer's larger setback. The
/// kernel's own rule (`ryukei::tweak::blends::recognise::size`), which it
/// keeps to itself.
pub fn size(kind: &BlendKind) -> f64 {
    match *kind {
        BlendKind::Fillet { radius } => radius,
        BlendKind::Chamfer(Chamfer::Equal { distance }) => distance,
        BlendKind::Chamfer(Chamfer::Distances { first, second, .. }) => first.max(second),
        BlendKind::Chamfer(Chamfer::DistanceAngle { distance, .. }) => distance,
    }
}

/// The blends (indices into `rec.blends`) a `delete_blend` takes: every
/// one at most `below` meters and every one holding a face of `faces` (a
/// corner's face takes the blends it joins), closed over their chains
/// and the corners they reach, as the kernel closes them
/// (`ryukei::tweak::blends::remake::close`). A face no blend or corner
/// holds is skipped here; the kernel refuses it (`not_a_blend`).
pub fn taken(rec: &Recognition, below: Option<f64>, faces: &[Name]) -> BTreeSet<usize> {
    let mut picked = BTreeSet::new();
    for &f in faces {
        match rec.holding(f) {
            Some(Ok(i)) => {
                picked.insert(i);
            }
            Some(Err(j)) => picked.extend(rec.corners[j].blends.iter().copied()),
            None => {}
        }
    }
    if let Some(s) = below {
        for (i, b) in rec.blends.iter().enumerate() {
            if size(&b.kind) <= s {
                picked.insert(i);
            }
        }
    }
    loop {
        let before = picked.len();
        let chains: BTreeSet<usize> = picked.iter().map(|&i| rec.blends[i].chain).collect();
        for (i, b) in rec.blends.iter().enumerate() {
            if chains.contains(&b.chain) {
                picked.insert(i);
            }
        }
        for c in &rec.corners {
            if c.blends.iter().any(|i| picked.contains(i)) {
                picked.extend(c.blends.iter().copied());
            }
        }
        if picked.len() == before {
            return picked;
        }
    }
}

/// The corners (indices into `rec.corners`) that `blends` reach.
pub fn corners_of(rec: &Recognition, blends: &BTreeSet<usize>) -> Vec<usize> {
    rec.corners
        .iter()
        .enumerate()
        .filter(|(_, c)| c.blends.iter().any(|i| blends.contains(i)))
        .map(|(j, _)| j)
        .collect()
}

/// A resize's `(face, kind)` pairs for the kernel, in group order: each
/// group's faces at its kind. Faces of one blend, or of one chain of
/// blends, in two groups are refused (`invalid_param` at the later face,
/// `first` the earlier): the kernel sizes a chain as one. A face no blend
/// holds is left for the kernel to refuse (`not_a_blend`).
pub fn sizes(
    id: &Id,
    rec: &Recognition,
    groups: &[(Vec<Name>, BlendKind)],
) -> Result<Vec<(Name, BlendKind)>> {
    let mut chains: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    let mut out = Vec::new();
    for (i, (faces, kind)) in groups.iter().enumerate() {
        for (j, f) in faces.iter().enumerate() {
            if let Some(Ok(b)) = rec.holding(*f) {
                match chains.get(&rec.blends[b].chain) {
                    Some(&(gi, gj)) if gi != i => {
                        return Err(Error::new(
                            Code::InvalidParam,
                            format!("resize_blend {id}: groups {gi} and {i} name one blend, or one chain of blends, which takes one size"),
                        )
                        .with("feature", id)
                        .with("pointer", format!("/groups/{i}/faces/{j}"))
                        .with("first", format!("/groups/{gi}/faces/{gj}")));
                    }
                    Some(_) => {}
                    None => {
                        chains.insert(rec.blends[b].chain, (i, j));
                    }
                }
            }
            out.push((*f, *kind));
        }
    }
    Ok(out)
}

/// What a resize's report keeps: its target and each blend it makes again
/// (every blend of a chain one of `sizes` names), with its index, faces,
/// chain, the kind it `was` and the kind it is `now`.
pub fn resize_report(target: &Id, rec: &Recognition, sizes: &[(Name, BlendKind)]) -> Value {
    let mut now: BTreeMap<usize, BlendKind> = BTreeMap::new();
    for (f, k) in sizes {
        if let Some(Ok(i)) = rec.holding(*f) {
            now.entry(rec.blends[i].chain).or_insert(*k);
        }
    }
    let resized: Vec<Value> = rec
        .blends
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let k = now.get(&b.chain)?;
            Some(json!({"index": i, "faces": hex(&b.faces), "chain": b.chain,
                        "was": kind_raw(&b.kind), "now": kind_raw(k)}))
        })
        .collect();
    json!({"target": target, "resized": resized})
}

/// A blend kind as a report keeps it: `kind`, `size`, a fillet's
/// `radius`, a chamfer's `form` and its sizes, lengths in metres, angles
/// in radians, a chamfer's face by name.
pub fn kind_raw(kind: &BlendKind) -> Value {
    let s = size(kind);
    match *kind {
        BlendKind::Fillet { radius } => json!({"kind": "fillet", "size": s, "radius": radius}),
        BlendKind::Chamfer(Chamfer::Equal { distance }) => {
            json!({"kind": "chamfer", "size": s, "form": "equal", "d": distance})
        }
        BlendKind::Chamfer(Chamfer::Distances {
            first,
            second,
            first_face,
        }) => json!({"kind": "chamfer", "size": s, "form": "distances", "d1": first,
                     "d2": second, "face": first_face.to_string()}),
        BlendKind::Chamfer(Chamfer::DistanceAngle {
            distance,
            angle,
            face,
        }) => json!({"kind": "chamfer", "size": s, "form": "distance_angle", "d": distance,
                     "angle": angle, "face": face.to_string()}),
    }
}

fn hex(names: &[Name]) -> Vec<String> {
    names.iter().map(Name::to_string).collect()
}

/// Blend `i` of `rec` as a report keeps it: its kind (as [`kind_raw`]),
/// `index`, `faces`, `supports` and `chain`.
pub fn blend_raw(rec: &Recognition, i: usize) -> Value {
    let b = &rec.blends[i];
    let mut v = kind_raw(&b.kind);
    v["index"] = json!(i);
    v["faces"] = json!(hex(&b.faces));
    v["supports"] = json!(hex(&b.supports));
    v["chain"] = json!(b.chain);
    v
}

/// A length as output gives it: `{"value": "3 mm", "m": 0.003}`, in `unit`.
pub fn length(m: f64, unit: Unit) -> Value {
    json!({
        "value": format!("{} {}", crate::measure::number(m / unit.factor()), unit.as_str()),
        "m": m,
    })
}

/// A kept blend or kind (as [`blend_raw`] or [`kind_raw`] give them) as
/// output gives it: names as `<part>/<solid>/<name>`, lengths and angles
/// as quantities in `unit` and degrees. Nested `was` and `now` (a
/// resize's) are rendered too.
pub fn render(part: &Id, solid: &Id, unit: Unit, raw: &Value) -> Value {
    let mut v = raw.clone();
    let at = |n: &Value| json!(format!("{part}/{solid}/{}", n.as_str().unwrap_or_default()));
    for k in ["size", "radius", "d", "d1", "d2"] {
        if let Some(x) = raw[k].as_f64() {
            v[k] = length(x, unit);
        }
    }
    if let Some(a) = raw["angle"].as_f64() {
        v["angle"] = json!({
            "value": format!("{} deg", crate::measure::number(a / crate::units::DEG)),
            "rad": a,
        });
    }
    if raw["face"].is_string() {
        v["face"] = at(&raw["face"]);
    }
    for k in ["faces", "supports"] {
        if let Some(a) = raw[k].as_array() {
            v[k] = Value::Array(a.iter().map(at).collect());
        }
    }
    for k in ["was", "now"] {
        if raw[k].is_object() {
            v[k] = render(part, solid, unit, &raw[k]);
        }
    }
    v
}

/// Recognitions already made, by body hash and tolerance (never on disk:
/// they are cheap beside the edits).
#[derive(Clone, Debug, Default)]
pub struct Cache {
    found: BTreeMap<String, Arc<Recognition>>,
}

impl Cache {
    pub fn len(&self) -> usize {
        self.found.len()
    }

    pub fn is_empty(&self) -> bool {
        self.found.is_empty()
    }

    /// The recognition of the body hashing `hash` at `tolerance`: kept,
    /// else made by `make` and kept if it succeeds (a cancelled or refused
    /// one isn't).
    pub fn get_or(
        &mut self,
        hash: &str,
        tolerance: f64,
        make: impl FnOnce() -> core::result::Result<Recognition, ryukei::Error>,
    ) -> core::result::Result<Arc<Recognition>, ryukei::Error> {
        let key = format!("{hash} {:016x}", tolerance.to_bits());
        if let Some(r) = self.found.get(&key) {
            return Ok(r.clone());
        }
        let r = Arc::new(make()?);
        self.found.insert(key, r.clone());
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::name::{OpKey, Role};
    use ryukei::tweak::{Corner, Recognised};

    fn n(k: u64) -> Name {
        Name::derive(OpKey(u128::from(k)), Role::NewFace, &[])
    }

    /// Four blends: 0 and 1 a chain of 2 mm fillets, 2 a 5 mm fillet,
    /// 3 a 1 mm chamfer; a corner joins 1 and 2.
    fn rec() -> Recognition {
        let fillet = |r: f64| BlendKind::Fillet { radius: r };
        let b = |face: u64, kind, chain| Recognised {
            faces: vec![n(face)],
            kind,
            supports: [n(100), n(101)],
            chain,
        };
        Recognition {
            blends: vec![
                b(1, fillet(2e-3), 0),
                b(2, fillet(2e-3), 0),
                b(3, fillet(5e-3), 1),
                b(4, BlendKind::Chamfer(Chamfer::Equal { distance: 1e-3 }), 2),
            ],
            corners: vec![Corner {
                face: n(9),
                blends: vec![1, 2],
            }],
        }
    }

    #[test]
    fn what_a_delete_takes_is_closed_over_chains_and_corners() {
        let r = rec();
        let set = |v: &[usize]| v.iter().copied().collect::<BTreeSet<usize>>();
        // Below 1.5 mm: the chamfer alone.
        assert_eq!(taken(&r, Some(1.5e-3), &[]), set(&[3]));
        // Below 2 mm: the chain, and the corner it reaches brings the 5 mm.
        assert_eq!(taken(&r, Some(2e-3), &[]), set(&[0, 1, 2, 3]));
        // By face: the 5 mm brings the corner's other blend, and its chain.
        assert_eq!(taken(&r, None, &[n(3)]), set(&[0, 1, 2]));
        // A corner's face takes the blends it joins.
        assert_eq!(taken(&r, None, &[n(9)]), set(&[0, 1, 2]));
        // Nothing picked, nothing taken; a face of no blend is skipped.
        assert_eq!(taken(&r, Some(0.5e-3), &[n(77)]), set(&[]));
        assert_eq!(corners_of(&r, &set(&[0, 1, 2])), [0]);
        assert!(corners_of(&r, &set(&[3])).is_empty());
    }

    #[test]
    fn sizes_and_kinds_are_kept_raw_and_rendered() {
        assert_eq!(size(&BlendKind::Fillet { radius: 2e-3 }), 2e-3);
        let k = BlendKind::Chamfer(Chamfer::Distances {
            first: 1e-3,
            second: 3e-3,
            first_face: n(7),
        });
        assert_eq!(size(&k), 3e-3);
        let raw = kind_raw(&k);
        assert_eq!(raw["kind"], "chamfer");
        assert_eq!(raw["form"], "distances");
        assert_eq!(raw["face"], n(7).to_string());
        let part = Id::parse("p", "part").unwrap();
        let solid = Id::parse("b", "feature").unwrap();
        let shown = render(&part, &solid, Unit::Mm, &raw);
        assert_eq!(shown["size"], json!({"value": "3 mm", "m": 3e-3}));
        assert_eq!(shown["d1"]["value"], "1 mm");
        assert_eq!(shown["face"], format!("p/b/{}", n(7)));
        let r = rec();
        let b = render(&part, &solid, Unit::Mm, &blend_raw(&r, 2));
        assert_eq!(b["faces"], json!([format!("p/b/{}", n(3))]));
        assert_eq!(b["supports"][1], format!("p/b/{}", n(101)));
        assert_eq!(
            (b["index"].as_u64(), b["chain"].as_u64()),
            (Some(2), Some(1))
        );
        assert_eq!(b["radius"]["value"], "5 mm");
        let a = render(
            &part,
            &solid,
            Unit::Mm,
            &kind_raw(&BlendKind::Chamfer(Chamfer::DistanceAngle {
                distance: 1e-3,
                angle: core::f64::consts::FRAC_PI_4,
                face: n(7),
            })),
        );
        assert_eq!(a["angle"]["value"], "45 deg");
    }

    #[test]
    fn a_blend_in_two_groups_is_refused() {
        let r = rec();
        let id = Id::parse("rz", "feature").unwrap();
        let three = BlendKind::Fillet { radius: 3e-3 };
        let four = BlendKind::Fillet { radius: 4e-3 };
        // Faces 1 and 2 are one chain: one group may name both.
        let ok = sizes(&id, &r, &[(vec![n(1), n(2)], three), (vec![n(3)], four)]).unwrap();
        assert_eq!(ok.len(), 3);
        // In two groups they are refused, at the second.
        let e = sizes(&id, &r, &[(vec![n(1)], three), (vec![n(2)], four)]).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["pointer"], "/groups/1/faces/0");
        assert_eq!(e.details["first"], "/groups/0/faces/0");
        // What the resize reports: the chain's two blends and the 5 mm's.
        let rep = resize_report(&id, &r, &ok);
        let got: Vec<u64> = rep["resized"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["index"].as_u64().unwrap())
            .collect();
        assert_eq!(got, [0, 1, 2]);
        assert_eq!(rep["resized"][2]["was"]["radius"], 5e-3);
        assert_eq!(rep["resized"][2]["now"]["radius"], 4e-3);
    }
}
