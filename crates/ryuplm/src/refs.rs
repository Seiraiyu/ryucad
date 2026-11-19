//! References (design 938): what a thread, a selection or a highlight
//! names in a catalogue. `item/<id>` is an item, `item/<id>/line/<line>`
//! one of its own BOM lines, both by ID: numbers change and IDs don't.
//! `resolve` says whether each still stands at a revision, which is how
//! the hosted service keeps a thread's place across pushes.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::load::Catalogue;
use crate::session::Session;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ref {
    Item(Id),
    /// An item's own line: the item, then the line.
    Line(Id, Id),
}

impl Ref {
    pub fn parse(s: &str) -> Option<Ref> {
        let parts: Vec<&str> = s.split('/').collect();
        match parts.as_slice() {
            ["item", i] => Some(Ref::Item(Id::parse(i, "item").ok()?)),
            ["item", i, "line", l] => Some(Ref::Line(
                Id::parse(i, "item").ok()?,
                Id::parse(l, "line").ok()?,
            )),
            _ => None,
        }
    }

    pub fn text(&self) -> String {
        match self {
            Ref::Item(i) => format!("item/{i}"),
            Ref::Line(i, l) => format!("item/{i}/line/{l}"),
        }
    }

    /// The item it is, or is a line of.
    pub fn item(&self) -> &Id {
        match self {
            Ref::Item(i) | Ref::Line(i, _) => i,
        }
    }
}

/// One reference against `cat`: `here` with what it names, `removed_in`
/// its item for a line the item no longer has, or `gone`.
pub fn standing(cat: &Catalogue, r: &Ref) -> Value {
    let mut v = json!({"ref": r.text()});
    let Some(item) = cat.items.get(r.item()) else {
        v["standing"] = json!("gone");
        return v;
    };
    v["item"] = json!(r.item());
    v["number"] = json!(item.number);
    if let Some(n) = &item.name {
        v["name"] = json!(n);
    }
    match r {
        Ref::Item(_) => v["standing"] = json!("here"),
        Ref::Line(i, l) => match item.bom.iter().find(|x| &x.line == l) {
            Some(line) => {
                v["standing"] = json!("here");
                v["line"] = json!(l);
                v["child"] = json!(line.item.text());
                v["quantity"] = json!(line.quantity);
            }
            None => {
                v["standing"] = json!("removed_in");
                v["removed_in"] = json!(Ref::Item(i.clone()).text());
            }
        },
    }
    v
}

/// `resolve`: `{schema, rev, commit?, refs: [...]}`, each reference's
/// [`standing`] at `rev` (the working files by default), in the order
/// asked. A reference that doesn't parse is `invalid_param`.
pub fn resolve(session: &mut Session, refs: &[String], rev: Option<&str>) -> Result<Value> {
    let parsed: Vec<Ref> = refs
        .iter()
        .map(|s| {
            Ref::parse(s).ok_or_else(|| {
                Error::new(
                    Code::InvalidParam,
                    format!("{s:?} is no reference: item/<id> or item/<id>/line/<line>"),
                )
                .with("param", "refs")
                .with("ref", s)
            })
        })
        .collect::<Result<_>>()?;
    let (cat, at) = crate::bom::at_rev(session, rev)?;
    let mut v = json!({
        "schema": crate::OUTPUT_SCHEMA,
        "rev": rev.unwrap_or("working"),
        "refs": parsed.iter().map(|r| standing(&cat, r)).collect::<Vec<_>>(),
    });
    if let crate::reps::At::Commit(c) = &at {
        v["commit"] = json!(c);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};
    use serde_json::json;

    #[test]
    fn references_parse_and_print() {
        let r = Ref::parse("item/k3v9q2m8x4c1p").unwrap();
        assert_eq!(r.text(), "item/k3v9q2m8x4c1p");
        let l = Ref::parse("item/k3v9q2m8x4c1p/line/0000000000001").unwrap();
        assert_eq!(l.text(), "item/k3v9q2m8x4c1p/line/0000000000001");
        assert!(matches!(l, Ref::Line(..)));
        for bad in [
            "",
            "item",
            "item/",
            "items/k3v9q2m8x4c1p",
            "item/k3v9q2m8x4c1p/line",
            "item/k3v9q2m8x4c1p/lines/0000000000001",
            "item/k3v9q2m8x4c1p/line/0000000000001/x",
            "item/NOT AN ID",
        ] {
            assert!(Ref::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn references_stand_are_removed_or_are_gone() {
        let (_d, root) = catalogue();
        let mk = |n: &str| {
            ask(
                &root,
                Request::ItemCreate {
                    fields: Default::default(),
                    number: Some(n.into()),
                    next: None,
                    expected_revision: None,
                },
            )
            .unwrap()["item"]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let (kit, glue) = (mk("KIT-1"), mk("GLUE-1"));
        let added = ask(
            &root,
            Request::BomAdd {
                item: kit.clone(),
                fields: serde_json::from_value(json!({"child": glue, "quantity": "2"})).unwrap(),
                expected_revision: None,
            },
        )
        .unwrap();
        let line = added["line"].as_str().unwrap().to_owned();
        let v = ask(
            &root,
            Request::Resolve {
                refs: vec![
                    format!("item/{kit}"),
                    format!("item/{kit}/line/{line}"),
                    format!("item/{kit}/line/zzzzzzzzzzzzf"),
                    "item/zzzzzzzzzzzzf".into(),
                ],
                rev: None,
            },
        )
        .unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["rev"], "working");
        let r = v["refs"].as_array().unwrap();
        assert_eq!(
            r[0],
            json!({"ref": format!("item/{kit}"), "standing": "here", "item": kit, "number": "KIT-1"})
        );
        assert_eq!(r[1]["standing"], "here");
        assert_eq!(r[1]["line"], json!(line));
        assert_eq!(r[1]["child"], json!(glue));
        assert_eq!(r[1]["quantity"], "2");
        assert_eq!(r[2]["standing"], "removed_in");
        assert_eq!(r[2]["removed_in"], json!(format!("item/{kit}")));
        assert_eq!(r[2]["number"], "KIT-1");
        assert_eq!(r[3], json!({"ref": "item/zzzzzzzzzzzzf", "standing": "gone"}));
        // A reference that doesn't parse names itself.
        let e = ask(
            &root,
            Request::Resolve {
                refs: vec!["sheet:1".into()],
                rev: None,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidParam);
        assert_eq!(e.details["param"], "refs");
        assert_eq!(e.details["ref"], "sheet:1");
    }
}
