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

/// A design's problems (design 629): every error fault of the project's
/// files and doctor's; the schematic's errors and stale layouts; ERC's
/// errors for it, waivers applied; another project's component placed
/// at `working`; every populated occurrence whose component has no maker
/// part number (raw `part_unidentified`, whatever severity it is given);
/// supply findings `parts.json` raises to `error`.
fn design_problems(session: &mut Session, part: &Id, r: &mut Readiness) -> Result<()> {
    use crate::fault::Fault;
    let cats = session.catalogues()?;
    let root = session.root().to_path_buf();
    let p = session.project();
    let errors = |list: Vec<Fault>| -> Vec<Value> {
        list.iter()
            .filter(|f| f.is_error())
            .map(Fault::to_json)
            .collect()
    };
    r.problems.extend(errors(p.faults()));
    r.problems.extend(errors(
        crate::doctor::diagnose(&root, p)
            .iter()
            .filter(|i| !matches!(i.code, Code::UnknownFile | Code::MergeConflict))
            .map(crate::doctor::Issue::fault)
            .collect(),
    ));
    for f in crate::erc::schematic_warnings(p) {
        if f.is_error() || f.error.code == Code::LayoutStale {
            r.problems.push(f.to_json());
        }
    }
    let design = p
        .conn
        .designs
        .get(part)
        .and_then(|d| d.item.as_ref().ok())
        .ok_or_else(|| {
            Error::new(Code::NotFound, format!("no design {part}")).with("part", part)
        })?;
    for b in super::content::reached(&p.conn, &design.root) {
        let Some(files) = p.conn.blocks.get(&b) else {
            continue;
        };
        for (cid, c) in &files.components {
            if let Ok(c) = &c.item
                && let Some(alias) = &c.of.library
            {
                r.problems.push(json!(
                    super::content::not_released(alias, &c.of.id, &format!("design {part}"))
                        .with("component", format!("{b}/{cid}"))
                ));
            }
        }
    }
    match crate::netlist::flatten(p, part) {
        Err(e) => r.problems.push(json!(e)),
        Ok(n) => {
            for f in crate::erc::check_design(p, part)? {
                if f.severity == crate::fault::Severity::Error && f.waived.is_none() {
                    r.problems.push(json!(f));
                }
            }
            // Every populated line must name its maker part, whatever its
            // category or the severity erc.json gives `part_unidentified`;
            // once each, where ERC's configured finding already holds it.
            for (of, c, placed) in crate::erc::placed_by_libcomp(p, &n) {
                if !c.mpn.trim().is_empty()
                    || r.problems
                        .iter()
                        .any(|x| x["code"] == "part_unidentified" && x["details"]["libcomp"] == of)
                {
                    continue;
                }
                let mut refs = vec![of.to_owned()];
                refs.extend(placed.iter().map(|o| o.reference.clone()));
                r.problems.push(json!(
                    Error::new(
                        Code::PartUnidentified,
                        format!("{of} has no maker part number: a release needs every placed part identified"),
                    )
                    .with("libcomp", of)
                    .with("refs", &refs)
                ));
            }
        }
    }
    r.supply = crate::supply::findings(p, &cats)?;
    r.problems.extend(
        r.supply
            .iter()
            .filter(|f| f["severity"] == "error")
            .cloned(),
    );
    Ok(())
}

/// The files a part's revision reads (`ryupdm::history::part_files_for`):
/// the root files, `ryupdm.json`, the part's own files and its
/// product's folders. Results are worked out over these alone, as
/// `release check --results` reads them at a commit, so both agree.
pub(crate) fn part_source(
    files: &ryu_session::fresh::Files,
    part: &Id,
) -> std::collections::BTreeMap<String, Vec<u8>> {
    use ryu_git::Layout;
    let own = <crate::Eda as Product>::part_files(part);
    files
        .iter()
        .filter(|(p, _)| {
            <crate::Eda as Layout>::ROOT_FILES.contains(&p.as_str())
                || p.as_str() == super::PDM_CONFIG
                || own.files.iter().any(|f| f == *p)
                || own.folders.iter().any(|d| {
                    p.strip_prefix(d.as_str())
                        .is_some_and(|r| r.starts_with('/'))
                })
        })
        .map(|(p, b)| (p.clone(), b.to_vec()))
        .collect()
}

/// BLAKE3 of `v`'s canonical JSON, in hex.
fn hex(v: &Value) -> String {
    blake3::hash(crate::canonical::to_string(v).as_bytes())
        .to_hex()
        .to_string()
}

/// A release's `results` (evidence, not identity): for a design,
/// `netlist`, BLAKE3 of the flattened netlist's canonical JSON, and
/// `erc`, of its configured findings; none for a component. Worked out
/// over `src` (a part's files, [`part_source`]), the project at `root`.
pub fn results(
    root: &std::path::Path,
    src: &dyn crate::source::Source,
    part: &Id,
) -> Result<std::collections::BTreeMap<String, String>> {
    let p = crate::project::Project::load_from(root, src, &std::collections::BTreeMap::new())?;
    let mut out = std::collections::BTreeMap::new();
    if p.manifest.project_kind == ProjectKind::Design {
        let n = crate::netlist::flatten(&p, part)?;
        out.insert("netlist".to_owned(), hex(&n.to_json()));
        out.insert(
            "erc".to_owned(),
            hex(&json!(crate::erc::check_design(&p, part)?)),
        );
    }
    Ok(out)
}

/// Works results out again for `release_result_changed`.
pub(crate) struct Again {
    pub(crate) root: std::path::PathBuf,
}

impl ryupdm::check::Results for Again {
    fn bodies(
        &mut self,
        src: &dyn ryu_doc::source::Source,
        part: &Id,
        _content: &str,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        results(&self.root, src, part)
    }
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

    #[test]
    fn a_design_is_ready_when_check_is_clean_for_it_and_every_line_is_identified() {
        let (_d, mut s) = session(ProjectKind::Design);
        crate::pdm::cmd::init(&mut s).unwrap();
        let mut n = serde_json::Map::new();
        n.insert("number".into(), json!("200-0001"));
        crate::pdm::cmd::set(&mut s, "d", n, None).unwrap();
        // The test design is clean, every line identified.
        let r = readiness(&mut s, &id("d")).unwrap();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        // A line with no maker part number isn't, whatever its category
        // and whatever severity ERC gives it.
        let at = s.root().join("library/components/r_100k.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&at).unwrap()).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("mpn");
        o.remove("manufacturer");
        o.remove("alternates");
        crate::source::write_atomic(&at, crate::canonical::to_string(&v)).unwrap();
        s.reload().unwrap();
        let r = readiness(&mut s, &id("d")).unwrap();
        assert!(
            codes(&r.problems).contains(&"part_unidentified".to_owned()),
            "{:?}",
            r.problems
        );
        // Another project's component at working can't be pinned yet.
        let c = s.root().join("blocks/top/components/r1.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&c).unwrap()).unwrap();
        v["of"] = json!({"library": "passives", "id": "r_100k"});
        crate::source::write_atomic(&c, crate::canonical::to_string(&v)).unwrap();
        s.reload().unwrap();
        let r = readiness(&mut s, &id("d")).unwrap();
        assert!(
            codes(&r.problems).contains(&"component_not_released".to_owned()),
            "{:?}",
            r.problems
        );
    }

    #[test]
    fn a_designs_results_are_its_netlists_and_ercs_hashes_and_a_components_none() {
        let (_d, s) = session(ProjectKind::Design);
        let files = s.snapshot();
        let r = results(s.root(), &files, &id("d")).unwrap();
        assert_eq!(r.keys().collect::<Vec<_>>(), ["erc", "netlist"]);
        assert!(r.values().all(|h| h.len() == 64));
        assert_eq!(
            results(s.root(), &files, &id("d")).unwrap(),
            r,
            "deterministic"
        );
        // A part's own files give the same results.
        let own = part_source(&files, &id("d"));
        assert_eq!(results(s.root(), &own, &id("d")).unwrap(), r);
        let (_l, l) = session(ProjectKind::Library);
        assert!(
            results(l.root(), &l.snapshot(), &id("r_100k"))
                .unwrap()
                .is_empty()
        );
    }
}
