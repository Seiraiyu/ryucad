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

pub fn report(session: &mut Session, within: &[String]) -> Result<Value> {
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
    ));
    let release = crate::release::findings(session)?;
    let errors = faults.iter().filter(|f| f.is_error()).count()
        + release.iter().filter(|f| f["severity"] == "error").count();
    let warnings = faults.len() + release.len() - errors;
    let mut r = json!({
        "schema": SCHEMA,
        "faults": crate::fault::list(&faults),
        "errors": errors,
        "warnings": warnings,
    });
    // Only when there are release findings, as ryueda's.
    if !release.is_empty() {
        r["release"] = json!(release);
    }
    Ok(r)
}

pub fn check(session: &mut Session, within: &[String]) -> Result<Value> {
    let r = report(session, within)?;
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
        let ok = ask(&root, Request::Check { within: vec![] }).unwrap();
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
        let e = ask(&root, Request::Check { within: vec![] }).unwrap_err();
        assert_eq!(e.code, crate::error::Code::CheckFailed);
        let r = &e.details["report"];
        assert_eq!(r["errors"], 1);
        assert_eq!(r["faults"][0]["code"], "item_class_unknown");
        assert_eq!(r["faults"][0]["severity"], "error");
    }
}
