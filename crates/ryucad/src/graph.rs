//! The dependency graph of a part's features. A feature reads the features
//! it names: an extrude or revolve its sketch, and every reference its
//! target's feature. Inputs must come earlier in the part's list, so the
//! graph has no cycles, and a feature's depth (one more than its deepest
//! input's) orders the rebuild: features of one depth build in parallel.

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Output, Results};
use std::collections::BTreeMap;

/// The features `f` reads, each once, in the order it names them.
pub fn inputs(f: &Feature) -> Vec<&Id> {
    let mut out: Vec<&Id> = Vec::new();
    for id in f.swept().into_iter() {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    for (_, r) in f.references() {
        if !out.contains(&&r.feature) {
            out.push(&r.feature);
        }
    }
    out
}

/// Each listed feature's depth: 0 for one that reads nothing, else one
/// more than its deepest input listed before it. Inputs that aren't
/// listed before it don't count; building the feature reports them.
pub fn depths(part: &Part) -> BTreeMap<Id, usize> {
    let mut out = BTreeMap::new();
    for id in part.order() {
        let d = match part.features.get(id) {
            Some(Ok(f)) => inputs(f)
                .into_iter()
                .filter_map(|i| out.get(i).map(|d| d + 1))
                .max()
                .unwrap_or(0),
            _ => 0,
        };
        out.insert(id.clone(), d);
    }
    out
}

/// The listed features that read `id`, in list order.
pub fn dependents(part: &Part, id: &Id) -> Vec<Id> {
    part.order()
        .iter()
        .filter(|f| match part.features.get(*f) {
            Some(Ok(feature)) => inputs(feature).contains(&id),
            _ => false,
        })
        .cloned()
        .collect()
}

/// Refuses `f` (feature `id`) if, in `order`, it reads a feature that
/// isn't listed (`not_found`) or isn't listed before it
/// (`reference_forward`).
pub fn check_order(part: &Part, order: &[Id], id: &Id, f: &Feature) -> Result<()> {
    let at = |x: &Id| order.iter().position(|o| o == x);
    for input in inputs(f) {
        match (at(input), at(id)) {
            (None, _) => {
                return Err(Error::new(
                    Code::NotFound,
                    format!("{id} reads {input}, which part {} doesn't list", part.id),
                )
                .with("feature", id)
                .with("input", input));
            }
            (Some(i), Some(me)) if i >= me => {
                return Err(Error::new(
                    Code::ReferenceForward,
                    format!("{id} reads {input}, which would come after it"),
                )
                .with("feature", id)
                .with("input", input));
            }
            _ => {}
        }
    }
    Ok(())
}

/// The output of `input`, which feature `id` reads, from the results of
/// the features built before it: `dependency_failed` if it failed.
pub fn input<'a>(part: &Part, id: &Id, input: &Id, before: &'a dyn Results) -> Result<&'a Output> {
    let f = part.features.get(id).and_then(|f| f.as_ref().ok());
    if let Some(f) = f {
        check_order(part, part.order(), id, f)?;
    }
    match before.result(input) {
        Some(Ok(o)) => Ok(o),
        Some(Err(e)) => Err(Error::new(
            Code::DependencyFailed,
            format!("{id} reads {input}, which failed to rebuild"),
        )
        .with("feature", id)
        .with("dependency", input)
        .with("cause", e.code)),
        None => unreachable!("every input listed before a feature is built first"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    #[test]
    fn sweeps_sit_below_their_sketches() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        for f in [
            json!({"type": "sphere", "id": "b", "params": {"radius": 1}}),
            json!({"type": "sketch", "id": "s"}),
            json!({"type": "extrude", "id": "e", "sketch": "s", "extent": {"length": 1}}),
            json!({"type": "extrude", "id": "f", "sketch": "s", "extent": {"length": 2}}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        let p = s.project().part(&id("p")).unwrap();
        let d = depths(p);
        assert_eq!(
            d.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(),
            [("b", 0), ("e", 1), ("f", 1), ("s", 0)]
        );
        assert_eq!(dependents(p, &id("s")), [id("e"), id("f")]);
        assert!(dependents(p, &id("b")).is_empty());
        let f = p.features[&id("e")].as_ref().unwrap();
        let e = check_order(p, &[id("e"), id("s")], &id("e"), f).unwrap_err();
        assert_eq!(e.code, Code::ReferenceForward);
        let e = check_order(p, &[id("e")], &id("e"), f).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        // The list says what each feature reads.
        let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
        assert_eq!(list["features"][2]["uses"], json!(["s"]));
        assert!(list["features"][0].get("uses").is_none());
        // Deleting what others read, or moving it after them, is refused.
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "s"})).unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
        assert_eq!(e.details["features"], json!(["e", "f"]));
        let e = call(
            &mut s,
            "feature.move",
            json!({"part": "p", "id": "s", "after": "e"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceForward);
    }
}
