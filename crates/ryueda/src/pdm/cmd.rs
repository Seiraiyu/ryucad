//! `pdm.*` and `props.*` over a session: ryupdm's settings and each
//! part's properties, as ryucad's `pdm.rs` gives them. Writes are judged
//! against the whole project first: one that would give any part a
//! property fault it doesn't have now is refused with it.

use super::{NAME, PDM_CONFIG, check_revision, revision, write};
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::session::Session;
use ryupdm::config::{CORE, Config, Decl, Pattern, Scheme, branch_name, property_name};
use ryupdm::props::Pdm;
use serde_json::{Map, Value, json};

/// Every part's properties in the files `files`, judged.
pub(crate) fn pdm_of(files: &dyn crate::source::Source, unit: crate::units::Unit) -> Pdm {
    let parts = ryupdm::product::part_ids::<crate::Eda>(files).unwrap_or_default();
    Pdm::of_for::<crate::Eda>(files, parts, unit)
}

/// A fault's identity, to tell faults a write adds from those it finds.
fn key(e: &Error) -> (Code, Value, Value) {
    (
        e.code,
        e.details.get("part").cloned().unwrap_or_default(),
        e.details.get("property").cloned().unwrap_or_default(),
    )
}

/// The property faults the project would have with `files` that it
/// doesn't have now.
pub(crate) fn added_faults(session: &Session, files: &dyn crate::source::Source) -> Vec<Error> {
    let unit = session.project().manifest.default_unit;
    let before: Vec<_> = pdm_of(&session.snapshot(), unit)
        .faults()
        .iter()
        .map(key)
        .collect();
    pdm_of(files, unit)
        .faults()
        .into_iter()
        .filter(|e| !before.contains(&key(e)))
        .collect()
}

/// Whether any release record exists: from then on, the revision scheme
/// can't change.
fn released(session: &Session) -> bool {
    let prefix = format!("{}/", super::RELEASES);
    session.files().keys().any(|p| p.starts_with(&prefix))
}

/// `pdm.config`: the settings with their defaults filled in, whether the
/// project has `ryupdm.json` yet, its ID, and the PDM revision.
pub fn config(session: &Session) -> Result<Value> {
    let files = session.snapshot();
    let c = Config::read_in(&files, NAME)?;
    let core: Map<String, Value> = CORE
        .iter()
        .map(|n| {
            (
                (*n).to_owned(),
                ryupdm::config::core(n).expect("core").to_json(),
            )
        })
        .collect();
    let custom: Map<String, Value> = c
        .properties
        .iter()
        .map(|(n, d)| (n.clone(), d.to_json()))
        .collect();
    let m = &session.project().manifest;
    Ok(json!({
        "configured": files.contains_key(PDM_CONFIG),
        "project": m.project,
        "forked_from": m.forked_from,
        "kind": super::kind_name(m.project_kind),
        "revisions": c.revisions.as_str(),
        "release_branch": c.release_branch,
        "pattern": c.pattern.as_ref().map(Pattern::text),
        "core": core,
        "properties": custom,
        "approvals": c.approvals.as_ref().map(ryupdm::approvals::Approvals::to_json),
        "dependencies": {"bump": c.bump.as_str()},
        "pdm_revision": revision(session),
    }))
}

fn save(session: &mut Session, c: &Config) -> Result<()> {
    let text = crate::canonical::to_string(&c.to_json());
    write(
        session,
        PDM_CONFIG,
        vec![(PDM_CONFIG.to_owned(), Some(text))],
    )
}

/// `pdm.init`: writes the default settings, keeping any the project has.
pub fn init(session: &mut Session) -> Result<Value> {
    let files = session.snapshot();
    if !files.contains_key(PDM_CONFIG) {
        let c = Config::read_in(&files, NAME)?;
        save(session, &c)?;
    }
    config(session)
}

/// `pdm.new_id`: gives a copied project an ID of its own, naming the one
/// it had and the commit `HEAD` is at (`forked_from`); records naming the
/// old ID stay, read as the upstream's history. Refused outside a
/// repository and before the first commit. Guarded by the PDM revision.
pub fn new_id(session: &mut Session, expected: Option<&str>) -> Result<Value> {
    check_revision(session, expected)?;
    let repo: crate::git::Repo = ryu_git::repo::repo(session.root())?;
    let Some(commit) = repo.head_commit() else {
        return Err(Error::new(
            Code::RevNotFound,
            "HEAD names no commit yet: commit the copy before giving it an ID of its own",
        )
        .with("rev", "HEAD"));
    };
    let mut m = session.project().manifest.clone();
    let old = m.project.clone();
    m.project = ryupdm::new_project_id()?;
    m.forked_from = Some(crate::project::ForkedFrom {
        project: old.clone(),
        commit: commit.clone(),
    });
    write(
        session,
        crate::project::MANIFEST,
        vec![(crate::project::MANIFEST.to_owned(), Some(m.to_text()))],
    )?;
    Ok(json!({
        "project": m.project,
        "forked_from": {"project": old, "commit": commit},
        "pdm_revision": revision(session),
    }))
}

/// What `pdm.configure` changes; what it leaves out stays.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub revisions: Option<String>,
    pub release_branch: Option<String>,
    /// A part number pattern, or `""` for none.
    pub pattern: Option<String>,
    /// Declarations by name; `null` removes one.
    pub properties: Map<String, Value>,
    /// `approvals` whole, or `false` to remove them.
    pub approvals: Option<Value>,
    /// `none` or `pr`.
    pub bump: Option<String>,
    /// Write even if parts' properties stop fitting.
    pub force: bool,
}

/// `pdm.configure`: changes the settings, guarded by the PDM revision.
pub fn configure(session: &mut Session, change: Change, expected: Option<&str>) -> Result<Value> {
    check_revision(session, expected)?;
    if change.revisions.is_none()
        && change.release_branch.is_none()
        && change.pattern.is_none()
        && change.properties.is_empty()
        && change.approvals.is_none()
        && change.bump.is_none()
    {
        return Err(Error::new(
            Code::Usage,
            "give at least one change: revisions, release_branch, pattern, approvals, bump or a property",
        ));
    }
    let mut files = session.snapshot();
    let mut c = Config::read_in(&files, NAME)?;
    if let Some(r) = &change.revisions {
        let scheme = Scheme::parse(r).ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                format!("revisions {r:?}: use letters, numeric or numeric2"),
            )
            .with("param", "revisions")
        })?;
        if scheme != c.revisions && released(session) {
            return Err(Error::new(
                Code::RevisionSchemeLocked,
                format!(
                    "parts have been released with {} labels, so the scheme can't change",
                    c.revisions.as_str()
                ),
            )
            .with("revisions", c.revisions.as_str()));
        }
        c.revisions = scheme;
    }
    if let Some(b) = &change.release_branch {
        if !branch_name(b) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("release branch {b:?} isn't a branch name Git accepts"),
            )
            .with("param", "release_branch"));
        }
        c.release_branch = b.clone();
    }
    if let Some(p) = &change.pattern {
        c.pattern = match p.as_str() {
            "" => None,
            p => Some(Pattern::new(p)?),
        };
    }
    for (name, v) in &change.properties {
        let at = format!("/properties/{name}");
        if v.is_null() {
            if c.properties.remove(name).is_none() {
                return Err(Error::new(
                    Code::NotFound,
                    format!("no custom property {name} is declared"),
                )
                .with("property", name));
            }
            continue;
        }
        property_name(name, &at)?;
        c.properties.insert(name.clone(), Decl::from_json(v, &at)?);
    }
    if let Some(a) = &change.approvals {
        c.approvals = match a {
            Value::Bool(false) => None,
            v => {
                let a = ryupdm::approvals::Approvals::from_json(v, "/approvals")?;
                // Teams are the repository owner's: refused here where the
                // clone's origin says who that is, and by the service always.
                if let Some((owner, _)) = crate::git::Repo::open(session.root())
                    .ok()
                    .flatten()
                    .and_then(|r| r.origin_url())
                    .as_deref()
                    .and_then(ryupdm::approvals::github_repo)
                {
                    a.check_owner(&owner, "/approvals")?;
                }
                Some(a)
            }
        };
    }
    if let Some(b) = &change.bump {
        c.bump = ryupdm::config::Bump::parse(b).ok_or_else(|| {
            Error::new(Code::InvalidParam, format!("bump {b:?}: use none or pr"))
                .with("param", "bump")
        })?;
    }
    files.insert(
        PDM_CONFIG.to_owned(),
        crate::canonical::to_string(&c.to_json())
            .into_bytes()
            .into(),
    );
    let added = added_faults(session, &files);
    if let (false, Some(first)) = (change.force, added.first()) {
        let mut e = first.clone();
        e.message = format!(
            "{}; change those parts first, or pass force to write it anyway",
            e.message
        );
        return Err(e.with("faults", &added));
    }
    save(session, &c)?;
    config(session)
}

/// The part `part` names, if the project has it: a design in a design
/// project, a library component in a library project.
pub(crate) fn part_id(session: &Session, part: &str) -> Result<Id> {
    let id = Id::parse(part, "part")?;
    if !ryupdm::product::part_ids::<crate::Eda>(&session.snapshot())?.contains(&id) {
        let kind = super::kind_name(session.project().manifest.project_kind);
        return Err(Error::new(Code::NotFound, format!("no {kind} {id}")).with("part", &id));
    }
    Ok(id)
}

/// A notice for a design about to get its first property or release
/// with a random-looking ID, which its tags will carry (design 629).
pub(crate) fn notice(session: &Session, id: &Id) -> Option<Value> {
    (session.project().manifest.project_kind == crate::project::ProjectKind::Design
        && super::random_looking(id))
    .then(|| {
        json!({
            "code": "design_id_random",
            "message": format!(
                "design {id} has a random ID, which its tags (ryupdm/{id}/<label>) will carry: create designs with --id to give them readable ones"
            ),
        })
    })
}

/// `props.get`: every core and declared property, set or not, with its
/// declaration, its value and where the value comes from (`properties`,
/// or `product`: a default from ryueda's own files), and its faults; then
/// any set but unknown; and the part's release state.
pub fn get(session: &mut Session, part: &str) -> Result<Value> {
    let id = part_id(session, part)?;
    let files = session.snapshot();
    let pdm = pdm_of(&files, session.project().manifest.default_unit);
    let c = pdm.config?;
    let pp = &pdm.parts[&id];
    let file = pp.file.clone()?;
    let faults_of = |name: &str| -> Vec<&Error> {
        pp.faults
            .iter()
            .filter(|e| e.details["property"] == name)
            .collect()
    };
    let shown = |name: &str| -> Option<(Value, &'static str)> {
        match (file.values.get(name), pp.defaults.get(name)) {
            (Some(v), _) => Some((v.clone(), "properties")),
            (None, Some(v)) => Some((v.clone(), "product")),
            _ => None,
        }
    };
    let mut list: Vec<Value> = c
        .decls()
        .into_iter()
        .map(|(name, decl)| {
            let mut v = json!({"name": name, "core": CORE.contains(&name.as_str()),
                               "declaration": decl.to_json(), "faults": faults_of(&name)});
            if let Some((x, from)) = shown(&name) {
                v["value"] = x;
                v["from"] = json!(from);
            }
            v
        })
        .collect();
    list.extend(
        file.values
            .iter()
            .filter(|(n, _)| c.decl(n).is_none())
            .map(|(name, x)| {
                json!({"name": name, "core": false, "declaration": null, "value": x,
                       "from": "properties", "faults": faults_of(name)})
            }),
    );
    let faults = pp.faults.clone();
    let mut v = json!({
        "part": id,
        "kind": super::kind_name(session.project().manifest.project_kind),
        "properties": list,
        "faults": faults,
        "state": super::state_of(session, &id),
        "pdm_revision": revision(session),
    });
    if let Some(n) = notice(session, &id) {
        v["notices"] = json!([n]);
    }
    Ok(v)
}

/// `props.set`: sets properties, or clears those given as `null`, as one
/// change, guarded by the PDM revision. Text is read as each property's
/// type (`"3"` as a whole number). A file left with no values is deleted.
pub fn set(
    session: &mut Session,
    part: &str,
    values: Map<String, Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let id = part_id(session, part)?;
    check_revision(session, expected)?;
    if values.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "give at least one property: NAME=VALUE, or NAME= to clear it",
        ));
    }
    let mut files = session.snapshot();
    let c = Config::read_in(&files, NAME)?;
    let mut file = ryupdm::props::read_for::<crate::Eda>(&files, &id)?;
    for (name, v) in values {
        if v.is_null() {
            file.values.remove(&name);
            continue;
        }
        let v = match c.decl(&name) {
            Some(d) => ryupdm::props::coerce(&d, v),
            None => v,
        };
        file.values.insert(name, v);
    }
    let at = ryupdm::props::path_for::<crate::Eda>(&id);
    let text = (!file.values.is_empty()).then(|| crate::canonical::to_string(&file.to_json()));
    match &text {
        Some(t) => files.insert(at.clone(), t.clone().into_bytes().into()),
        None => files.remove(&at),
    };
    // A duplicate number is reported on both parts: say it of this one.
    let added = added_faults(session, &files);
    if let Some(e) = added
        .iter()
        .find(|e| e.details["part"] == id.as_str())
        .or(added.first())
    {
        return Err(e.clone());
    }
    write(session, "properties", vec![(at, text)])?;
    get(session, part)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::ProjectKind;
    use crate::project::tests::{design_in, library_in, manifest};

    pub(crate) fn session(kind: ProjectKind) -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        manifest(&root, kind, &[]);
        library_in(&root);
        if kind == ProjectKind::Design {
            design_in(&root);
        }
        let s = Session::open(&root).unwrap();
        (dir, s)
    }

    #[test]
    fn init_writes_the_settings_once_and_configure_changes_them() {
        let (_d, mut s) = session(ProjectKind::Design);
        let v = config(&s).unwrap();
        assert_eq!(
            (v["configured"].as_bool(), v["revisions"].as_str()),
            (Some(false), Some("letters"))
        );
        let v = init(&mut s).unwrap();
        assert_eq!(v["configured"], true);
        assert!(s.root().join("ryupdm.json").is_file());
        let rev = v["pdm_revision"].as_str().unwrap().to_owned();
        let change = Change {
            revisions: Some("numeric".into()),
            ..Change::default()
        };
        let v = configure(&mut s, change.clone(), Some(&rev)).unwrap();
        assert_eq!(v["revisions"], "numeric");
        // The revision it was guarded by has moved on.
        assert_eq!(
            configure(&mut s, change, Some(&rev)).unwrap_err().code,
            Code::Conflict
        );
        assert_eq!(
            configure(&mut s, Change::default(), None).unwrap_err().code,
            Code::Usage
        );
    }

    #[test]
    fn a_copy_takes_an_id_of_its_own() {
        let (_d, mut s) = session(ProjectKind::Library);
        // Outside a repository: refused.
        assert_eq!(new_id(&mut s, None).unwrap_err().code, Code::NotAGitRepo);
        crate::git::tests::git(s.root(), &["init", "-q", "-b", "main"]);
        crate::git::tests::git(s.root(), &["add", "-A"]);
        crate::git::tests::git(s.root(), &["commit", "-q", "-m", "start"]);
        let old = s.project().manifest.project.clone();
        let v = new_id(&mut s, None).unwrap();
        assert_ne!(s.project().manifest.project, old);
        assert_eq!(v["forked_from"]["project"], serde_json::json!(old));
        assert_eq!(
            s.project().manifest.forked_from.as_ref().unwrap().project,
            old
        );
    }

    fn property(v: &Value, name: &str) -> Value {
        v["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap()
            .clone()
    }

    #[test]
    fn properties_are_set_cleared_and_shown_with_their_defaults() {
        let (_d, mut s) = session(ProjectKind::Design);
        init(&mut s).unwrap();
        let v = get(&mut s, "d").unwrap();
        let name = property(&v, "name");
        assert_eq!(
            (name["value"].as_str(), name["from"].as_str()),
            (Some("Breakout"), Some("product"))
        );
        assert_eq!(v["kind"], "design");
        assert_eq!(v["state"]["state"], "in_work");
        let mut set_to = Map::new();
        set_to.insert("number".into(), json!("100-0001"));
        set_to.insert("name".into(), json!("Breakout board"));
        let v = set(&mut s, "d", set_to, None).unwrap();
        let name = property(&v, "name");
        assert_eq!(
            (name["value"].as_str(), name["from"].as_str()),
            (Some("Breakout board"), Some("properties"))
        );
        assert!(s.root().join("properties/d.json").is_file());
        // An unknown property is refused, nothing written.
        let mut bad = Map::new();
        bad.insert("colour".into(), json!("red"));
        assert_eq!(
            set(&mut s, "d", bad, None).unwrap_err().code,
            Code::PropertyUnknown
        );
        // Clearing every value deletes the file.
        let mut clear = Map::new();
        clear.insert("number".into(), Value::Null);
        clear.insert("name".into(), Value::Null);
        set(&mut s, "d", clear, None).unwrap();
        assert!(!s.root().join("properties/d.json").exists());
        assert_eq!(get(&mut s, "nope").unwrap_err().code, Code::NotFound);
    }
}
