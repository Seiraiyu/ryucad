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
use ryupdm::history::Releases;
use ryupdm::product::Product;
use ryupdm::record::{BuiltWith, Record, Records, Release};
use ryupdm::state::Status;
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
            .filter(|i| {
                !matches!(
                    i.code,
                    Code::UnknownFile
                        | Code::MergeConflict
                        | Code::DependencyUnlocked
                        | Code::LockUnused
                )
            })
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
            let Ok(c) = &c.item else { continue };
            let Some(alias) = &c.of.library else { continue };
            let at = format!("{b}/{cid}");
            let what = format!("design {part}");
            let label = p
                .manifest
                .libraries
                .get(alias)
                .and_then(|a| a.labels.get(&c.of.id));
            match (label, p.pinned.get(&(alias.to_string(), c.of.id.clone()))) {
                (None, _) | (_, None) => r.problems.push(json!(
                    super::content::not_released(alias, &c.of.id, &what).with("component", &at)
                )),
                (Some(_), Some(Err(e))) => r.problems.push(json!(e.clone().with("component", &at))),
                (Some(label), Some(Ok(pn))) if pn.obsolete => r.problems.push(json!(
                    Error::new(
                        Code::ComponentObsolete,
                        format!(
                            "{what} places {alias}/{}@{label}, which is obsolete: pin a current label (`ryueda lib update`)",
                            c.of.id
                        ),
                    )
                    .with("alias", alias)
                    .with("component", &at)
                    .with("label", label)
                )),
                _ => {}
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
            // Every populated BOM line must name its maker part, whatever
            // its category or the severity erc.json gives
            // `part_unidentified` (design 629); a line whose component ERC
            // already reports as an error isn't listed twice.
            let all: Vec<&crate::netlist::Occurrence> = n.occurrences.iter().collect();
            for l in crate::pdm::bom::lines_of(p, &all, 1) {
                if l.quantity == 0 || !l.columns.get("mpn").is_none_or(|m| m.trim().is_empty()) {
                    continue;
                }
                let refdes = l.columns.get("refdes").cloned().unwrap_or_default();
                let id = l.part.rsplit(['/', ':']).next().unwrap_or_default();
                let reported = r.problems.iter().any(|x| {
                    x["code"] == "part_unidentified"
                        && x["details"]["libcomp"]
                            .as_str()
                            .is_some_and(|of| of.ends_with(&format!("/{id}")))
                });
                if reported {
                    continue;
                }
                r.problems.push(json!(
                    Error::new(
                        Code::BomUnidentified,
                        format!(
                            "{} ({refdes}) has no maker part number: a release needs every placed part identified",
                            l.part
                        ),
                    )
                    .with("part", &l.part)
                    .with("refdes", &refdes)
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
    pins: &dyn crate::pin::Resolve,
) -> Result<std::collections::BTreeMap<String, String>> {
    let p =
        crate::project::Project::load_from(root, src, &std::collections::BTreeMap::new(), pins)?;
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
    /// Resolves the pins of the designs checked.
    pub(crate) pins: crate::pin::Pins,
}

impl ryupdm::check::Results for Again {
    fn bodies(
        &mut self,
        src: &dyn ryu_doc::source::Source,
        part: &Id,
        _content: &str,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        results(&self.root, src, part, &self.pins)
    }
}

/// What a proposal of `part` would write, every check passed.
struct Proposal {
    part: Id,
    next: ryupdm::release::Next,
    content: String,
    results: std::collections::BTreeMap<String, String>,
}

/// `release_not_ready` for `part` with its problems.
fn not_ready(part: &Id, problems: Vec<Value>) -> Error {
    Error::new(
        Code::ReleaseNotReady,
        format!(
            "{part} can't be released as it is: {}",
            problems
                .iter()
                .filter_map(|p| p["message"].as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ),
    )
    .with("part", part)
    .with("problems", &problems)
}

/// Every check `release.propose` makes of `part`, and what its record
/// would hold. Writes nothing; `supply` collects the supply findings.
fn prepare(
    session: &mut Session,
    part: &Id,
    releases: &Releases,
    supply: &mut Vec<Value>,
) -> Result<Proposal> {
    let files = session.snapshot();
    let config = Config::read_in(&files, NAME)?;
    let records = Records::read_in(&files, NAME)?;
    let next = ryupdm::release::next_for::<crate::Eda>(&files, &config, &records, releases, part)?;
    let r = readiness(session, part)?;
    if supply.is_empty() {
        *supply = r.supply.clone();
    }
    if !r.problems.is_empty() {
        return Err(not_ready(part, r.problems));
    }
    let src = part_source(&files, part);
    let content = <crate::Eda as Product>::content(&src, part)?;
    let results = results(session.root(), &src, part, &*session.pins)?;
    Ok(Proposal {
        part: part.clone(),
        next,
        content,
        results,
    })
}

/// The parts `--all` proposes: those in work or modified.
fn proposable(session: &Session, releases: &Releases) -> Vec<Id> {
    let files = session.snapshot();
    let parts = ryupdm::product::part_ids::<crate::Eda>(&files).unwrap_or_default();
    ryupdm::state::derive_all_for::<crate::Eda>(&files, releases)
        .into_iter()
        .filter(|(_, s)| matches!(s.status, Status::InWork | Status::Modified(_)))
        .filter(|(p, _)| parts.contains(p))
        .map(|(p, _)| p)
        .collect()
}

/// `release.propose`: writes the record of each named part's next
/// revision (every part in work or modified with `all`) as one logged,
/// undoable change. One part named alone is refused whole as ryucad
/// refuses it; several or `all` propose every ready one and list the
/// rest under `refused`, each with its error. With `dry_run`, writes
/// nothing and answers each part's label, readiness and problems.
pub fn propose(
    session: &mut Session,
    participant: &str,
    parts: &[String],
    all: bool,
    note: Option<String>,
    expected: Option<&str>,
    dry_run: bool,
) -> Result<Value> {
    super::check_revision(session, expected)?;
    let (repo, releases) = super::history(session)?;
    ryupdm::history::check_folder(repo.prefix())?;
    let mut ids: Vec<Id> = parts
        .iter()
        .map(|p| super::cmd::part_id(session, p))
        .collect::<Result<_>>()?;
    if all {
        ids.extend(proposable(session, &releases));
    }
    ids.sort();
    ids.dedup();
    if ids.is_empty() && !all {
        return Err(Error::new(
            Code::Usage,
            "name a part to release, or pass --all",
        ));
    }
    let alone = ids.len() == 1 && !all;
    let mut supply = Vec::new();
    let (mut ready, mut refused) = (Vec::new(), Vec::new());
    for id in &ids {
        match prepare(session, id, &releases, &mut supply) {
            Ok(p) => ready.push(p),
            Err(e) if alone && !dry_run => return Err(e),
            Err(e) => refused.push((id.clone(), e)),
        }
    }
    let notices: Vec<Value> = ids
        .iter()
        .filter_map(|id| super::cmd::notice(session, id))
        .collect();
    if dry_run {
        let files = session.snapshot();
        let mut out: Vec<Value> = ready
            .iter()
            .map(|p| {
                json!({"part": p.part, "label": p.next.label, "supersedes": p.next.supersedes,
                       "ready": true, "problems": []})
            })
            .collect();
        for (id, e) in &refused {
            let problems = match e.code {
                Code::ReleaseNotReady => e.details["problems"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
                _ => vec![json!(e)],
            };
            let next = ryupdm::state::derive_for::<crate::Eda>(&files, id, &releases).next;
            out.push(
                json!({"part": id, "label": next, "supersedes": null, "ready": false,
                            "problems": problems}),
            );
        }
        out.sort_by(|a, b| a["part"].as_str().cmp(&b["part"].as_str()));
        return Ok(json!({"parts": out, "supply": supply, "notices": notices}));
    }
    if ready.is_empty() && !refused.is_empty() && !all {
        let first = refused.remove(0).1;
        return Err(first.with(
            "refused",
            refused
                .iter()
                .map(|(p, e)| json!({"part": p, "error": e}))
                .collect::<Vec<_>>(),
        ));
    }
    let project = session.project().manifest.project.clone();
    let v = crate::proto::VersionInfo::current();
    let by = ryupdm::release::released_by(repo.gix(), participant);
    let files = session.snapshot();
    let config = Config::read_in(&files, NAME)?;
    let records: Vec<Record> = ready
        .into_iter()
        .map(|p| {
            let props = ryupdm::props::read_for::<crate::Eda>(&files, &p.part).unwrap_or_default();
            Record::Release(Release {
                project: project.clone(),
                part: p.part,
                label: p.next.label,
                content: p.content,
                properties: ryupdm::record::revising(&config, &props),
                results: p.results,
                built_with: BuiltWith::of_product(NAME, &v.ryueda, &v.commit),
                released_by: by.clone(),
                supersedes: p.next.supersedes,
                note: note.clone(),
            })
        })
        .collect();
    if !records.is_empty() {
        super::write(
            session,
            "the release",
            records
                .iter()
                .map(|r| (r.path(), Some(crate::canonical::to_string(&r.to_json()))))
                .collect(),
        )?;
    }
    let (_, releases) = super::history(session)?;
    let files = session.snapshot();
    let proposed: Vec<Value> = records
        .iter()
        .map(|r| {
            let s = ryupdm::state::derive_for::<crate::Eda>(&files, r.part(), &releases);
            let mut v = json!({"part": r.part(), "label": r.label(), "path": r.path(),
                               "record": r.to_json(), "state": s.to_json()});
            if let Some(a) = ryupdm::release::required(r.part(), &releases) {
                v["approval"] = a;
            }
            v
        })
        .collect();
    Ok(json!({
        "proposed": proposed,
        "refused": refused
            .iter()
            .map(|(p, e)| json!({"part": p, "error": e}))
            .collect::<Vec<_>>(),
        "supply": supply,
        "notices": notices,
        "pdm_revision": super::revision(session),
    }))
}

/// `release.obsolete`: writes an obsolete record for `part`'s latest
/// official revision, which must be `label`. The part may be gone.
pub fn obsolete(
    session: &mut Session,
    participant: &str,
    part: &str,
    label: &str,
    note: Option<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let id = Id::parse(part, "part")?;
    super::check_revision(session, expected)?;
    let (repo, releases) = super::history(session)?;
    ryupdm::history::check_folder(repo.prefix())?;
    let files = session.snapshot();
    let config = Config::read_in(&files, NAME)?;
    let records = Records::read_in(&files, NAME)?;
    ryupdm::release::obsolete_target(&config, &records, &releases, &id, label)?;
    let record = Record::Obsolete(ryupdm::record::Obsolete {
        project: session.project().manifest.project.clone(),
        part: id.clone(),
        label: label.to_owned(),
        released_by: ryupdm::release::released_by(repo.gix(), participant),
        note,
    });
    super::write(
        session,
        "the obsolete record",
        vec![(
            record.path(),
            Some(crate::canonical::to_string(&record.to_json())),
        )],
    )?;
    let (_, releases) = super::history(session)?;
    let s = ryupdm::state::derive_for::<crate::Eda>(&session.snapshot(), &id, &releases);
    Ok(
        json!({"path": record.path(), "record": record.to_json(), "state": s.to_json(),
              "pdm_revision": super::revision(session)}),
    )
}

/// `release.list`: every record (or one part's) with where it became
/// official and its tag, and each part's state. In a fork, records
/// naming the ID it had are listed apart as `inherited`.
pub fn list(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let part = part.map(|p| Id::parse(p, "part")).transpose()?;
    let (has_repo, releases) = match super::history(session) {
        Ok((_, r)) => (true, r),
        Err(e) if e.code == Code::NotAGitRepo => (false, Releases::default()),
        Err(e) => return Err(e),
    };
    let files = session.snapshot();
    let config = Config::read_in(&files, NAME)?;
    let records = Records::read_in(&files, NAME)?;
    let mine = |p: &Id| part.as_ref().is_none_or(|x| x == p);
    if let Some(p) = &part
        && !ryupdm::product::part_ids::<crate::Eda>(&files)?.contains(p)
        && records.of(p).next().is_none()
    {
        return Err(Error::new(Code::NotFound, format!("no part {p}")).with("part", p));
    }
    let forked = session
        .project()
        .manifest
        .forked_from
        .as_ref()
        .map(|f| f.project.clone());
    let upstream = |e: &ryupdm::record::Entry| {
        e.record
            .as_ref()
            .is_ok_and(|r| Some(r.project()) == forked.as_ref())
    };
    let (inherited, own): (Vec<_>, Vec<_>) = records
        .entries
        .iter()
        .filter(|e| mine(&e.part))
        .partition(|e| upstream(e));
    let show = |l: Vec<&ryupdm::record::Entry>| -> Vec<Value> {
        l.into_iter()
            .map(|e| ryupdm::release::listed(e, &releases, has_repo))
            .collect()
    };
    let parts: Vec<Value> = ryupdm::state::derive_all_for::<crate::Eda>(&files, &releases)
        .into_iter()
        .filter(|(p, _)| mine(p))
        .map(|(p, s)| {
            let mut v = s.to_json();
            v["part"] = json!(p);
            v
        })
        .collect();
    let mut v = json!({
        "repository": has_repo,
        "release_branch": config.release_branch,
        "branch": releases.branch.as_ref().map(|b| json!({"name": b.name, "tip": b.tip})),
        "shallow": releases.shallow,
        "records": show(own),
        "parts": parts,
        "pdm_revision": super::revision(session),
    });
    if let Some(f) = &forked {
        v["inherited"] = json!(show(inherited));
        v["forked_from"] = json!(f);
    }
    Ok(v)
}

/// The release findings for the project (`schema: 1` each, with
/// `source: release`); with `again`, official releases' results are
/// worked out again (`release_result_changed`).
pub fn findings(session: &mut Session, again: bool) -> Result<Vec<Value>> {
    let files = session.snapshot();
    let root = session.root().to_path_buf();
    let found = match super::history(session) {
        Ok((repo, releases)) => {
            let h = ryupdm::check::History {
                repo: repo.gix(),
                prefix: repo.prefix(),
                releases: &releases,
            };
            let mut a = Again {
                root,
                pins: crate::pin::Pins::default(),
            };
            let r: Option<&mut dyn ryupdm::check::Results> =
                if again { Some(&mut a) } else { None };
            ryupdm::check::run_for::<crate::Eda>(&files, Some(h), r)?
        }
        Err(e) if e.code == Code::NotAGitRepo => {
            ryupdm::check::run_for::<crate::Eda>(&files, None, None)?
        }
        Err(e) => return Err(e),
    };
    Ok(found
        .iter()
        .map(|f| {
            let mut v = f.to_json();
            v["source"] = json!("release");
            v
        })
        .collect())
}

/// `release.check`: the release checks alone, reported as `check`
/// reports; fails with `check_failed` when any finding is an error.
pub fn check(session: &mut Session, again: bool) -> Result<Value> {
    let findings = findings(session, again)?;
    let errors = findings.iter().filter(|f| f["severity"] == "error").count();
    let report = json!({
        "schema": ryupdm::check::SCHEMA,
        "passed": errors == 0,
        "errors": errors,
        "warnings": findings.len() - errors,
        "findings": findings,
    });
    if errors > 0 {
        return Err(Error {
            code: Code::CheckFailed,
            message: format!("{errors} release check(s) failed"),
            details: report,
        });
    }
    Ok(report)
}

/// `release.sync`: the tag of every official release, created where
/// missing and moved where it points elsewhere; never pushed or deleted.
/// Records under approval rules are left to the hosted service.
pub fn sync(session: &mut Session) -> Result<Value> {
    let (repo, releases) = super::history(session)?;
    ryupdm::release::sync(repo.gix(), &releases, NAME)
}

/// What parts' release states and tags move with: the PDM revision and
/// the release branch's refs (none outside a repository).
pub fn key(session: &mut Session) -> String {
    let refs = ryu_git::repo::repo::<crate::Eda>(session.root())
        .ok()
        .and_then(|repo: crate::git::Repo| {
            let c = Config::read_in(&session.snapshot(), NAME).ok()?;
            ryupdm::release::refs_key(repo.gix(), &c).ok()
        })
        .unwrap_or_default();
    format!("{}\0{refs}", super::revision(session))
}

/// Each part's release state and its releases' tags, for `pdm.changed`,
/// by part.
pub fn summary(session: &mut Session) -> std::collections::BTreeMap<String, Value> {
    let releases = super::releases_or_none(session);
    ryupdm::state::derive_all_for::<crate::Eda>(&session.snapshot(), &releases)
        .into_iter()
        .map(|(p, s)| {
            (
                p.to_string(),
                ryupdm::release::state_value(&p, &s, &releases),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdm::cmd::tests::session;
    use crate::pdm::tests::id;

    #[test]
    fn a_design_is_ready_only_with_its_pins_official_and_current() {
        let (_d, board, mut fake) = crate::project::tests::pinned_board();
        let ready = |fake: &crate::project::tests::Fake| {
            let mut s = Session::open_with_pins(&board, Box::new(fake.clone())).unwrap();
            codes(&readiness(&mut s, &id("d")).unwrap().problems)
        };
        let c = ready(&fake);
        assert!(!c.contains(&"component_not_released".to_owned()), "{c:?}");
        assert!(!c.contains(&"component_obsolete".to_owned()), "{c:?}");
        fake.obsolete("r_100k", "A");
        let c = ready(&fake);
        assert!(c.contains(&"component_obsolete".to_owned()), "{c:?}");
        assert!(!c.contains(&"component_not_released".to_owned()), "{c:?}");
        // A pin that doesn't resolve says why.
        fake.pins.clear();
        let c = ready(&fake);
        assert!(c.contains(&"label_not_found".to_owned()), "{c:?}");
    }

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
            codes(&r.problems).contains(&"bom_unidentified".to_owned()),
            "{:?}",
            r.problems
        );
        let x = r
            .problems
            .iter()
            .find(|x| x["code"] == "bom_unidentified")
            .unwrap();
        assert!(
            x["details"]["part"].is_string() && x["details"]["refdes"].is_string(),
            "{x}"
        );
        // Another project's component at working: not released.
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
        let r = results(s.root(), &files, &id("d"), &*s.pins).unwrap();
        assert_eq!(r.keys().collect::<Vec<_>>(), ["erc", "netlist"]);
        assert!(r.values().all(|h| h.len() == 64));
        assert_eq!(
            results(s.root(), &files, &id("d"), &*s.pins).unwrap(),
            r,
            "deterministic"
        );
        // A part's own files give the same results.
        let own = part_source(&files, &id("d"));
        assert_eq!(results(s.root(), &own, &id("d"), &*s.pins).unwrap(), r);
        let (_l, l) = session(ProjectKind::Library);
        assert!(
            results(l.root(), &l.snapshot(), &id("r_100k"), &*l.pins)
                .unwrap()
                .is_empty()
        );
    }

    pub(crate) fn in_git(s: &mut Session) {
        let g = |args: &[&str]| crate::git::tests::git(s.root(), args);
        g(&["init", "-q", "-b", "main"]);
        g(&["add", "-A"]);
        g(&["commit", "-q", "-m", "start"]);
    }

    fn number(s: &mut Session, part: &str, n: &str) {
        let mut m = serde_json::Map::new();
        m.insert("number".into(), json!(n));
        crate::pdm::cmd::set(s, part, m, None).unwrap();
    }

    fn one(part: &str) -> Vec<String> {
        vec![part.to_owned()]
    }

    #[test]
    fn components_are_proposed_one_at_a_time_or_all_at_once() {
        let (_d, mut s) = session(ProjectKind::Library);
        crate::pdm::cmd::init(&mut s).unwrap();
        in_git(&mut s);
        let dry = propose(&mut s, "agent", &one("r_100k"), false, None, None, true).unwrap();
        assert_eq!(dry["parts"][0]["ready"], false, "{dry}");
        assert_eq!(dry["parts"][0]["label"], "A");
        assert!(
            !s.root().join("releases").exists(),
            "a dry run writes nothing"
        );
        assert_eq!(
            propose(&mut s, "agent", &one("r_100k"), false, None, None, false)
                .unwrap_err()
                .code,
            Code::ReleaseNotReady
        );
        number(&mut s, "r_100k", "100-0001");
        let v = propose(
            &mut s,
            "agent",
            &[],
            true,
            Some("first".into()),
            None,
            false,
        )
        .unwrap();
        assert_eq!(v["proposed"][0]["label"], "A", "{v}");
        let rec = &v["proposed"][0]["record"];
        assert_eq!(
            (
                rec["format"].as_u64(),
                rec["built_with"]["product"].as_str()
            ),
            (Some(2), Some("ryueda"))
        );
        assert_eq!(rec["results"], json!({}));
        assert!(s.root().join("releases/r_100k/A.json").is_file());
        // Proposed, not official: a second proposal is pending.
        assert_eq!(
            propose(&mut s, "agent", &one("r_100k"), false, None, None, false)
                .unwrap_err()
                .code,
            Code::ReleasePending
        );
        // --all with nothing in work or modified proposes nothing.
        crate::git::tests::git(s.root(), &["add", "-A"]);
        crate::git::tests::git(s.root(), &["commit", "-q", "-m", "A"]);
        let v = propose(&mut s, "agent", &[], true, None, None, false).unwrap();
        assert_eq!(v["proposed"], json!([]), "{v}");
        let v = sync(&mut s).unwrap();
        assert_eq!(v["created"][0]["tag"], "ryupdm/r_100k/A", "{v}");
        assert_eq!(sync(&mut s).unwrap()["unchanged"], 1);
        assert_eq!(
            obsolete(&mut s, "agent", "r_100k", "B", None, None)
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
        let v = obsolete(
            &mut s,
            "agent",
            "r_100k",
            "A",
            Some("replaced".into()),
            None,
        )
        .unwrap();
        assert_eq!(v["record"]["kind"], "ryupdm.obsolete", "{v}");
        assert!(s.root().join("releases/r_100k/A.obsolete.json").is_file());
        let v = list(&mut s, None).unwrap();
        assert_eq!(v["repository"], true);
        let recs = v["records"].as_array().unwrap();
        assert_eq!(recs.len(), 2, "{v}");
        assert_eq!(
            (recs[0]["label"].as_str(), recs[0]["status"].as_str()),
            (Some("A"), Some("official"))
        );
        assert_eq!(recs[1]["status"], "proposed");
        assert_eq!(v["parts"][0]["part"], "r_100k");
        assert_eq!(list(&mut s, Some("nope")).unwrap_err().code, Code::NotFound);
    }

    #[test]
    fn a_designs_record_holds_its_results() {
        let (_d, mut s) = session(ProjectKind::Design);
        crate::pdm::cmd::init(&mut s).unwrap();
        number(&mut s, "d", "200-0001");
        in_git(&mut s);
        let dry = propose(&mut s, "agent", &one("d"), false, None, None, true).unwrap();
        assert_eq!(dry["parts"][0]["ready"], true, "{dry}");
        let v = propose(&mut s, "agent", &one("d"), false, None, None, false).unwrap();
        let r = &v["proposed"][0]["record"]["results"];
        assert!(r["netlist"].is_string() && r["erc"].is_string(), "{v}");
        // What it records is what a check at the commit works out again.
        let files = s.snapshot();
        let again = results(s.root(), &part_source(&files, &id("d")), &id("d"), &*s.pins).unwrap();
        assert_eq!(r, &json!(again));
    }

    #[test]
    fn the_release_checks_find_a_stale_proposal() {
        let (_d, mut s) = session(ProjectKind::Library);
        crate::pdm::cmd::init(&mut s).unwrap();
        number(&mut s, "r_100k", "100-0001");
        in_git(&mut s);
        propose(&mut s, "agent", &one("r_100k"), false, None, None, false).unwrap();
        assert_eq!(check(&mut s, false).unwrap()["passed"], true);
        // ryueda check reports nothing of releases while none is wrong.
        let cats = s.catalogues().unwrap();
        let f = findings(&mut s, false).unwrap();
        let v = crate::erc::check_project(s.project(), &cats, &[], &f, &[], &[]).unwrap();
        assert!(v.get("release").is_none(), "{v}");
        let at = s.root().join("library/components/r_100k.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&at).unwrap()).unwrap();
        v["value"] = json!("99k");
        crate::source::write_atomic(&at, crate::canonical::to_string(&v)).unwrap();
        s.reload().unwrap();
        let e = check(&mut s, false).unwrap_err();
        assert_eq!(e.code, Code::CheckFailed);
        let codes: Vec<&str> = e.details["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["code"].as_str())
            .collect();
        assert!(codes.contains(&"release_stale"), "{codes:?}");
        // `ryueda check` reports them as its `release` section.
        let f = findings(&mut s, false).unwrap();
        let e = crate::erc::check_project(s.project(), &cats, &[], &f, &[], &[]).unwrap_err();
        assert!(
            e.details["report"]["release"]
                .as_array()
                .is_some_and(|l| !l.is_empty())
        );
    }
}
