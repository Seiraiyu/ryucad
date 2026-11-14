//! `import.convert`: an import feature's mode set to a solid, or back to
//! a reference (design 364), as one logged, undoable change. Face names
//! don't change (a reference is named as healing names it); edges and
//! vertices sewing merges take healing's `Merged` names.

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::import::{Import, Mode};
use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde_json::{Value, json};

/// What `import.convert` was asked.
pub struct Ask {
    pub part: String,
    pub feature: Option<String>,
    pub reference: bool,
    pub expected: Option<String>,
}

pub fn convert(session: &mut Session, ask: Ask) -> Result<Value> {
    session.job().phase(ryu_session::job::Phase::Heal);
    let pid = crate::part::part_id(&ask.part)?;
    let part = session.project().part(&pid)?.clone();
    crate::part::check_revision(&part, ask.expected.as_deref())?;
    let order = crate::part::order(&part)?;
    let to = if ask.reference {
        Mode::Reference
    } else {
        Mode::Solid
    };
    let imports: Vec<(Id, Import)> = match &ask.feature {
        Some(f) => {
            let fid = crate::part::feature_id(f)?;
            match part.features.get(&fid) {
                None => {
                    return Err(
                        Error::new(Code::NotFound, format!("{pid} has no feature {fid}"))
                            .with("part", &pid)
                            .with("feature", &fid),
                    );
                }
                Some(Ok(Feature::Import(i))) => vec![(fid, i.clone())],
                Some(_) => {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!("{pid}/{fid} isn't an import: only an import feature has a mode"),
                    )
                    .with("pointer", "feature"));
                }
            }
        }
        None => order
            .iter()
            .filter_map(|fid| match part.features.get(fid) {
                Some(Ok(Feature::Import(i))) => Some((fid.clone(), i.clone())),
                _ => None,
            })
            .collect(),
    };
    if imports.is_empty() {
        return Err(
            Error::new(Code::InvalidParam, format!("{pid} has no import features"))
                .with("pointer", "part"),
        );
    }
    let changing: Vec<(Id, Feature)> = imports
        .into_iter()
        .filter(|(_, i)| i.mode() != to)
        .map(|(fid, mut i)| {
            i.mode = (to == Mode::Reference).then_some(Mode::Reference);
            (fid, Feature::Import(i))
        })
        .collect();
    if to == Mode::Reference {
        // Nothing may act on what becomes a reference (decision 5).
        let solids = crate::solid::Solids::of(&part);
        for (fid, _) in &changing {
            let acting: Vec<&Id> = solids
                .chain(fid)
                .unwrap_or_default()
                .iter()
                .filter(|f| *f != fid)
                .collect();
            if !acting.is_empty() {
                let names: Vec<&str> = acting.iter().map(|a| a.as_str()).collect();
                return Err(Error::new(
                    Code::ReferenceBody,
                    format!(
                        "{fid} can't become a reference: {} act on it, and nothing may change a reference",
                        names.join(", ")
                    ),
                )
                .with("feature", fid)
                .with("solid", fid)
                .with("acting", &acting));
            }
        }
    }
    let converted: Vec<Id> = changing.iter().map(|(f, _)| f.clone()).collect();
    if !changing.is_empty() {
        session.job().check()?;
        session.job().writing();
        let writes: Vec<(&Id, &Feature)> = changing.iter().map(|(f, x)| (f, x)).collect();
        crate::save::save(session, &pid, &writes, order)?;
    }
    let (mut v, _) = crate::part::part_report(session, &pid, false)?;
    v["converted"] = json!(converted);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use crate::doc::import::Mode;
    use crate::error::Code;
    use crate::import::tests::{occt, with_import, with_import_as};
    use crate::testing::call;
    use serde_json::{Value, json};

    fn convert(s: &mut crate::session::Session, p: Value) -> crate::error::Result<Value> {
        call(s, "import.convert", p)
    }

    /// `p/i/<name>` of the solid's first edge.
    fn any_edge(s: &mut crate::session::Session) -> String {
        let v = call(s, "inspect", json!({"part": "p"})).unwrap();
        format!(
            "p/i/{}",
            v["bodies"][0]["edges"][0]["name"].as_str().unwrap()
        )
    }

    #[test]
    fn convert_both_ways_with_undo() {
        let (_dir, mut s) = with_import_as(
            &occt("box_ap214.stp"),
            Some(("part 2", "part 2")),
            Some(Mode::Reference),
        );
        let mut live = crate::live::Live::new();
        let rev = s
            .project()
            .part(&crate::doc::id::Id::parse("p", "part").unwrap())
            .unwrap()
            .revision
            .clone();
        let call_live = crate::testing::call_live;
        let v = call_live(
            &mut s,
            &mut live,
            None,
            "import.convert",
            json!({"part": "p", "expected_revision": rev}),
        )
        .unwrap();
        assert_eq!(v["converted"], json!(["i"]), "{v}");
        assert!(v["features"][0]["import"]["heal"].is_object(), "{v}");
        assert!(v["solids"][0].get("reference").is_none(), "{v}");
        // Again: nothing to do, nothing written.
        let before = s.snapshot();
        let again = convert(&mut s, json!({"part": "p"})).unwrap();
        assert_eq!(again["converted"], json!([]));
        assert_eq!(s.snapshot(), before);
        // Back to a reference, then undo returns the solid.
        call_live(
            &mut s,
            &mut live,
            None,
            "import.convert",
            json!({"part": "p", "feature": "i", "reference": true}),
        )
        .unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        assert_eq!(r["parts"][0]["solids"][0]["reference"], true, "{r}");
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        assert!(r["parts"][0]["solids"][0].get("reference").is_none(), "{r}");
    }

    #[test]
    fn refusals() {
        let (_dir, mut s) = with_import(&occt("box_ap214.stp"), Some(("part 2", "part 2")));
        // A fillet on the solid keeps it from becoming a reference.
        let edge = any_edge(&mut s);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "fillet", "id": "f", "radius": 0.1, "edges": [edge]}),
        )
        .unwrap();
        let e = convert(&mut s, json!({"part": "p", "reference": true})).unwrap_err();
        assert_eq!(e.code, Code::ReferenceBody);
        assert_eq!(e.details["acting"], json!(["f"]));
        let e = convert(&mut s, json!({"part": "p", "feature": "f"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let e = convert(&mut s, json!({"part": "p", "feature": "g"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = convert(&mut s, json!({"part": "q"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = convert(&mut s, json!({"part": "p", "expected_revision": "0"})).unwrap_err();
        assert_eq!(e.code, Code::Conflict);
    }
}
