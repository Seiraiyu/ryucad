//! `check`: every finding of the catalogue's files, numbers elsewhere,
//! links and BOMs followed on disk, and ryupdm's release findings.

use crate::error::{Code, Error, Result};
use crate::judge::Context;
use crate::session::Session;
use serde_json::{Value, json};
use std::path::PathBuf;

pub const SCHEMA: u64 = 1;

/// The folders searched for other catalogues: `within`, then the
/// `pdm.dependents` preference.
pub fn elsewhere(within: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = within.iter().map(PathBuf::from).collect();
    if let Ok(p) = ryu_session::prefs::Preferences::<crate::Plm>::load()
        && let Some(list) = p.values.get("pdm.dependents").and_then(Value::as_array)
    {
        out.extend(list.iter().filter_map(Value::as_str).map(PathBuf::from));
    }
    out
}

/// What tells one fault from another across revisions: its code, the item
/// it names (else its file), and where in it.
fn key(f: &crate::fault::Fault) -> (String, String, String) {
    let d = &f.error.details;
    let s = |k: &str| d[k].as_str().unwrap_or("").to_owned();
    let at = if d["item"].is_string() {
        s("item")
    } else {
        s("path")
    };
    (
        json!(f.error.code).as_str().unwrap_or("").to_owned(),
        at,
        s("pointer"),
    )
}

/// The catalogue's faults at `rev`, by [`key`], and the commit `rev`
/// names; `None` for the faults when that commit holds no catalogue here.
fn base(
    session: &mut Session,
    rev: &str,
    within: &[String],
) -> Result<(
    String,
    Option<std::collections::BTreeSet<(String, String, String)>>,
)> {
    let (repo, _) = crate::release::history(session)?;
    let commit = crate::reps::commit_hex(repo.gix(), rev)?;
    let files: std::collections::BTreeMap<String, Vec<u8>> =
        ryupdm::history::files_for::<crate::Plm>(repo.gix(), &commit, repo.prefix())?;
    if !files.contains_key(crate::catalogue::MANIFEST) {
        return Ok((commit, None));
    }
    let cat = crate::load::Catalogue::load(&files)?;
    let cx = Context {
        elsewhere: elsewhere(within),
        here: Some(session.root().to_path_buf()),
    };
    let mut faults = crate::judge::all(&files, &cat, &cx);
    let mut reps = crate::bom::disk(session, within);
    let index = reps.index.clone();
    let others = crate::numbers::others(session.root(), &cx.elsewhere);
    faults.extend(crate::bom::findings(
        &cat,
        &index,
        &mut reps,
        &others,
        &crate::reps::At::Commit(commit.clone()),
    ));
    Ok((commit, Some(faults.iter().map(key).collect())))
}

/// `{schema, faults, errors, warnings, release?, against?}`. With
/// `against`, each fault says whether it is `new` since that revision
/// (none there with its code, item and pointer), and `against` holds the
/// revision, its commit, and `diff` and `bom diff` from it to the working
/// files; `absent: true` instead when that commit has no catalogue here.
pub fn report(session: &mut Session, within: &[String], against: Option<&str>) -> Result<Value> {
    let old = against.map(|r| base(session, r, within)).transpose()?;
    let cx = Context {
        elsewhere: elsewhere(within),
        here: Some(session.root().to_path_buf()),
    };
    let mut faults = crate::judge::all(&session.snapshot(), session.catalogue(), &cx);
    let mut reps = crate::bom::disk(session, within);
    let index = reps.index.clone();
    let others = crate::numbers::others(session.root(), &cx.elsewhere);
    faults.extend(crate::bom::findings(
        session.catalogue(),
        &index,
        &mut reps,
        &others,
        &crate::reps::At::Working,
    ));
    let release = crate::release::findings(session)?;
    let errors = faults.iter().filter(|f| f.is_error()).count()
        + release.iter().filter(|f| f["severity"] == "error").count();
    let warnings = faults.len() + release.len() - errors;
    let listed: Vec<Value> = faults
        .iter()
        .map(|f| {
            let mut v = f.to_json();
            if let Some((_, was)) = &old {
                v["new"] = json!(was.as_ref().is_none_or(|w| !w.contains(&key(f))));
            }
            v
        })
        .collect();
    let mut r = json!({
        "schema": SCHEMA,
        "faults": listed,
        "errors": errors,
        "warnings": warnings,
    });
    // Only when there are release findings, as ryueda's.
    if !release.is_empty() {
        r["release"] = json!(release);
    }
    if let (Some(rev), Some((commit, was))) = (against, &old) {
        r["against"] = match was {
            None => json!({"rev": rev, "commit": commit, "absent": true}),
            Some(_) => json!({
                "rev": rev,
                "commit": commit,
                "items": crate::diff::diff(session, Some(rev), None)?,
                "bom": crate::bom_diff::diff(session, None, Some(rev), None, within)?,
            }),
        };
    }
    Ok(r)
}

pub fn check(session: &mut Session, within: &[String], against: Option<&str>) -> Result<Value> {
    let r = report(session, within, against)?;
    let (e, w) = (
        r["errors"].as_u64().unwrap_or(0),
        r["warnings"].as_u64().unwrap_or(0),
    );
    if e > 0 {
        return Err(Error::new(
            Code::CheckFailed,
            format!("check found {e} error(s) and {w} warning(s)"),
        )
        .with("report", r));
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};
    use serde_json::json;

    #[test]
    fn check_reports_errors_and_warnings() {
        let (_d, root) = catalogue();
        let ok = ask(
            &root,
            Request::Check {
                within: vec![],
                against: None,
            },
        )
        .unwrap();
        assert_eq!(
            (
                ok["errors"].clone(),
                ok["warnings"].clone(),
                ok["schema"].clone()
            ),
            (json!(0), json!(0), json!(1))
        );
        // A hand-written item with an undeclared class: check fails.
        let id = "0000000000001";
        std::fs::create_dir_all(root.join("items/00")).unwrap();
        std::fs::write(
            root.join(format!("items/00/{id}.json")),
            crate::canonical::to_string(
                &json!({"format": 1, "kind": "ryuplm.item", "number": "X-1", "class": "nope"}),
            ),
        )
        .unwrap();
        let e = ask(
            &root,
            Request::Check {
                within: vec![],
                against: None,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::CheckFailed);
        let r = &e.details["report"];
        assert_eq!(r["errors"], 1);
        assert_eq!(r["faults"][0]["code"], "item_class_unknown");
        assert_eq!(r["faults"][0]["severity"], "error");
    }
}
