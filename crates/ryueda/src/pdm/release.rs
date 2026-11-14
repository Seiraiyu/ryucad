//! Releases of ryueda's parts over a session, as ryucad's `release.rs`
//! gives them (design 629's "Releasing"): readiness, results, records
//! proposed and made obsolete, the list, the checks and the tags. The
//! steps that need no session are `ryupdm::release`'s.

use super::NAME;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::ProjectKind;
use crate::session::Session;
use ryupdm::config::Config;
use ryupdm::product::Product;
use serde_json::{Value, json};

/// Why a part can't be released, and the supply findings a release
/// reports (never records).
#[derive(Debug, Default)]
pub struct Readiness {
    /// Each an error as JSON, `release_not_ready`'s `problems`.
    pub problems: Vec<Value>,
    /// The project's supply findings from this machine's parts cache, with
    /// `fetched_at`; those `parts.json` raises to `error` are problems too.
    pub supply: Vec<Value>,
}

/// The PDM properties' problems: each fault, then each property a
/// release needs that neither the file nor a default sets.
fn property_problems(session: &Session, part: &Id, config: &Config) -> Vec<Value> {
    let files = session.snapshot();
    let pdm = super::cmd::pdm_of(&files, session.project().manifest.default_unit);
    let Some(pp) = pdm.parts.get(part) else {
        return Vec::new();
    };
    let props = match (&pp.file, pp.effective()) {
        (Err(e), _) => return vec![json!(e)],
        (Ok(_), Some(p)) => p,
        (Ok(_), None) => return Vec::new(),
    };
    let mut out: Vec<Value> = pp.faults.iter().map(|e| json!(e)).collect();
    for name in ryupdm::props::missing(config, &props) {
        out.push(json!(
            Error::new(
                Code::PropertyInvalid,
                format!("part {part}: {name} is required to release")
            )
            .with("part", part)
            .with("property", &name)
            .with("missing", true)
        ));
    }
    out
}

/// A library component's own errors: its file's, its symbol's and its
/// footprints', as the library judges them.
fn component_problems(session: &Session, part: &Id) -> Vec<Value> {
    let mut paths = vec![super::content::object_path(
        crate::library::COMPONENTS,
        part,
    )];
    if let Ok(c) = super::content::libcomp(&session.snapshot(), part) {
        if c.symbol.library.is_none() {
            paths.push(super::content::object_path(
                crate::library::SYMBOLS,
                &c.symbol.id,
            ));
        }
        for u in c
            .footprints
            .iter()
            .filter(|u| u.footprint.library.is_none())
        {
            paths.push(super::content::object_path(
                crate::library::FOOTPRINTS,
                &u.footprint.id,
            ));
        }
    }
    session
        .project()
        .faults()
        .iter()
        .filter(|f| f.is_error())
        .filter(|f| {
            f.error.details["path"]
                .as_str()
                .is_some_and(|x| paths.iter().any(|p| p == x))
        })
        .map(crate::fault::Fault::to_json)
        .collect()
}

/// Every reason `part` can't be released as it is (design 629): for a
/// library component, the library's judgement of it and its properties;
/// for a design, its check, ERC and identification, and its properties.
pub fn readiness(session: &mut Session, part: &Id) -> Result<Readiness> {
    let config = Config::read_in(&session.snapshot(), NAME)?;
    let mut r = Readiness::default();
    match session.project().manifest.project_kind {
        ProjectKind::Library => r.problems.extend(component_problems(session, part)),
        ProjectKind::Design => design_problems(session, part, &mut r)?,
    }
    r.problems.extend(property_problems(session, part, &config));
    Ok(r)
}

/// A design's problems: Task 16.
fn design_problems(session: &mut Session, part: &Id, r: &mut Readiness) -> Result<()> {
    let _ = (session, part, r);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdm::cmd::tests::session;
    use crate::pdm::tests::id;

    fn codes(problems: &[Value]) -> Vec<String> {
        problems
            .iter()
            .map(|p| p["code"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn a_component_is_ready_when_its_library_judges_it_clean_and_its_properties_pass() {
        let (_d, mut s) = session(ProjectKind::Library);
        crate::pdm::cmd::init(&mut s).unwrap();
        // `number` is required to release.
        let r = readiness(&mut s, &id("r_100k")).unwrap();
        assert_eq!(codes(&r.problems), ["property_invalid"], "{:?}", r.problems);
        assert_eq!(r.problems[0]["details"]["property"], "number");
        let mut n = serde_json::Map::new();
        n.insert("number".into(), json!("100-0001"));
        crate::pdm::cmd::set(&mut s, "r_100k", n, None).unwrap();
        assert!(
            readiness(&mut s, &id("r_100k"))
                .unwrap()
                .problems
                .is_empty(),
            "{:?}",
            readiness(&mut s, &id("r_100k")).unwrap().problems
        );
        // A footprint pad its symbol's pins don't reach is the library's error.
        let fp = s.root().join("library/footprints/r_0402_1005metric.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&fp).unwrap()).unwrap();
        v["pads"] = json!([]);
        crate::source::write_atomic(&fp, crate::canonical::to_string(&v)).unwrap();
        s.reload().unwrap();
        let r = readiness(&mut s, &id("r_100k")).unwrap();
        assert!(
            !r.problems.is_empty(),
            "the component's own faults block it"
        );
    }
}
