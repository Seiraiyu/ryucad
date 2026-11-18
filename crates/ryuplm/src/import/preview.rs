//! `import check` (design 734): what a mapping would read from a source,
//! row by row, every refusal at its row and column, before anything is
//! written.

use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde_json::{Value, json};
use std::path::Path;

pub const SCHEMA: u64 = 1;

pub fn check(
    session: &mut Session,
    name: &str,
    file: &Path,
    limit: Option<usize>,
) -> Result<Value> {
    let p = super::prepare(session, name, file)?;
    let used = p.mapping.columns();
    let unread: Vec<&String> = p
        .table
        .columns
        .iter()
        .filter(|c| !used.contains(c.as_str()))
        .collect();
    let shown: Vec<Value> = p
        .rows
        .iter()
        .take(limit.unwrap_or(100))
        .map(|r| {
            let mut v = json!({"row": r.record.at, "key": r.record.key, "values": r.values});
            if !r.extra.is_empty() {
                v["extra"] = json!(r.extra);
            }
            for (k, x) in [
                ("parent", &r.record.parent),
                ("quantity", &r.record.quantity),
                ("find", &r.record.find),
                ("refdes", &r.record.refdes),
            ] {
                if let Some(x) = x {
                    v[k] = json!(x);
                }
            }
            v
        })
        .collect();
    let report = json!({
        "schema": SCHEMA,
        "import": name,
        "file": file,
        "columns": p.table.columns,
        "unread_columns": unread,
        "rows": p.rows.len(),
        "shown": shown,
        "refusals": p.refusals,
    });
    if !p.refusals.is_empty() {
        return Err(Error::new(
            Code::CheckFailed,
            format!("{} row(s) don't read through {name}", p.refusals.len()),
        )
        .with("report", report));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};
    use serde_json::json;

    #[test]
    fn check_reads_every_row_and_refuses_at_row_and_column() {
        let (_d, root) = catalogue();
        std::fs::create_dir_all(root.join("imports")).unwrap();
        std::fs::write(
            root.join("imports/m.json"),
            crate::canonical::to_string(&json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
                "fields": {"number": {"from": "PN"}, "make_buy": {"from": "Src"}}})),
        )
        .unwrap();
        let src = root.join("items.csv");
        std::fs::write(&src, "ID,PN,Src,Note\n1,A-1,buy,hello\n2,A-2,maybe,\n").unwrap();
        let e = ask(
            &root,
            Request::ImportCheck {
                name: "m".into(),
                file: src.display().to_string(),
                limit: None,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::CheckFailed);
        let r = &e.details["report"];
        assert_eq!(r["rows"], 1);
        assert_eq!(
            r["shown"][0]["values"],
            json!({"make_buy": "buy", "number": "A-1"})
        );
        assert_eq!(r["shown"][0]["extra"], json!({"Note": "hello"}));
        assert_eq!(r["unread_columns"], json!(["Note"]));
        assert_eq!(
            r["refusals"],
            json!([{"row": 3, "column": "Src", "message": "make_buy \"maybe\" is neither make nor buy; map it with values"}])
        );
        std::fs::write(&src, "ID,PN,Src,Note\n1,A-1,buy,hello\n").unwrap();
        ask(
            &root,
            Request::ImportCheck {
                name: "m".into(),
                file: src.display().to_string(),
                limit: None,
            },
        )
        .unwrap();
        let e = ask(
            &root,
            Request::ImportCheck {
                name: "nope".into(),
                file: src.display().to_string(),
                limit: None,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::NotFound);
    }
}
