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

/// What a live selection, highlight or status names (design 938, The
/// live session): `item/<id or number>` (kept by the item's ID),
/// `item/<id or number>/line/<line>` (a line the item has), or
/// `import/<name>` (a mapping the catalogue has). The reference as the
/// session keeps it; `invalid_param` when it doesn't parse,
/// `item_not_found`, or `not_found` for a line or mapping that isn't
/// there.
pub fn target(session: &Session, s: &str) -> Result<String> {
    let bad = || {
        Error::new(
            Code::InvalidParam,
            format!(
                "{s:?} is no reference: item/<id or number>, item/<id>/line/<line> or import/<name>"
            ),
        )
        .with("param", "refs")
        .with("ref", s)
    };
    if let Some(name) = s.strip_prefix("import/") {
        let path = crate::import::mapping_path(name);
        if crate::import::name_of(&path).is_none() {
            return Err(bad());
        }
        use crate::source::Source;
        return match session.snapshot().read(&path)? {
            Some(_) => Ok(s.to_owned()),
            None => {
                Err(Error::new(Code::NotFound, format!("no mapping {path}")).with("import", name))
            }
        };
    }
    let cat = session.catalogue();
    let parts: Vec<&str> = s.split('/').collect();
    match parts.as_slice() {
        ["item", i] if !i.is_empty() => Ok(Ref::Item(cat.find(i)?).text()),
        ["item", i, "line", l] if !i.is_empty() => {
            let id = cat.find(i)?;
            let line = Id::parse(l, "line").map_err(|_| bad())?;
            if cat.items[&id].bom.iter().any(|x| x.line == line) {
                Ok(Ref::Line(id, line).text())
            } else {
                Err(
                    Error::new(Code::NotFound, format!("item {id} has no line {line}"))
                        .with("line", &line),
                )
            }
        }
        _ => Err(bad()),
    }
}

/// [`target`] of each, the first refusal failing them all.
pub fn targets(session: &Session, refs: &[String]) -> Result<Vec<String>> {
    refs.iter().map(|r| target(session, r)).collect()
}

/// Whether the live session keeps `t` after a change: it still names
/// what it named, by the same text.
pub fn stands(session: &Session, t: &str) -> bool {
    target(session, t).is_ok_and(|c| c == t)
}

/// What each live reference names now, for an agent reading a person's
/// selection: [`standing`] for items and lines, the mapping for imports.
pub fn objects(session: &Session, refs: &[String]) -> Vec<Value> {
    refs.iter()
        .map(|r| match (r.strip_prefix("import/"), Ref::parse(r)) {
            (Some(name), _) => json!({"ref": r, "standing": "here", "import": name}),
            (None, Some(x)) => standing(session.catalogue(), &x),
            (None, None) => json!({"ref": r, "standing": "gone"}),
        })
        .collect()
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
        assert_eq!(
            r[3],
            json!({"ref": "item/zzzzzzzzzzzzf", "standing": "gone"})
        );
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

    use crate::server::handle_safely;
    use crate::session::Session;
    use ryu_session::live::{Kind, Live, SelectMode};

    fn create(root: &std::path::Path, number: &str) -> String {
        let v = ask(
            root,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some(number.into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        v["item"]["id"].as_str().unwrap().to_owned()
    }

    #[test]
    fn live_targets_take_numbers_and_keep_ids() {
        let (_d, root) = catalogue();
        let a = create(&root, "A-1");
        let b = create(&root, "B-1");
        let line = ask(
            &root,
            Request::BomAdd {
                item: "A-1".into(),
                fields: serde_json::from_value(json!({"child": "B-1", "quantity": "2"})).unwrap(),
                expected_revision: None,
            },
        )
        .unwrap()["line"]
            .as_str()
            .unwrap()
            .to_owned();
        let s = Session::open(&root).unwrap();
        assert_eq!(target(&s, "item/A-1").unwrap(), format!("item/{a}"));
        assert_eq!(
            target(&s, &format!("item/{b}")).unwrap(),
            format!("item/{b}")
        );
        assert_eq!(
            target(&s, &format!("item/A-1/line/{line}")).unwrap(),
            format!("item/{a}/line/{line}")
        );
        assert_eq!(
            target(&s, "item/NOPE").unwrap_err().code,
            Code::ItemNotFound
        );
        assert_eq!(
            target(&s, "item/B-1/line/0000000000001").unwrap_err().code,
            Code::NotFound
        );
        assert_eq!(target(&s, "import/erp").unwrap_err().code, Code::NotFound);
        assert_eq!(
            target(&s, "import/../x").unwrap_err().code,
            Code::InvalidParam
        );
        assert_eq!(target(&s, "part/x").unwrap_err().code, Code::InvalidParam);
        assert!(stands(&s, &format!("item/{a}")));
        assert!(!stands(&s, "item/A-1"), "kept by ID only");
        let o = objects(&s, &[format!("item/{a}"), "import/erp".into()]);
        assert_eq!(
            (o[0]["number"].clone(), o[0]["standing"].clone()),
            (json!("A-1"), json!("here"))
        );
        assert_eq!(o[1]["import"], "erp");
    }

    #[test]
    fn what_a_write_takes_away_leaves_every_selection() {
        let (_d, root) = catalogue();
        let a = create(&root, "A-1");
        let b = create(&root, "B-1");
        let mut slot = None;
        let mut live = Live::<crate::Plm>::new();
        let dana = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        live.select(
            &dana,
            vec![format!("item/{a}"), format!("item/{b}")],
            SelectMode::Replace,
        )
        .unwrap();
        handle_safely(
            &mut slot,
            Some(&mut live),
            None,
            &root,
            Request::ItemDelete {
                item: "B-1".into(),
                expected_revision: None,
            },
        )
        .unwrap();
        assert_eq!(live.get(&dana).unwrap().selection, [format!("item/{a}")]);
        // A change from outside (a file deleted by hand) prunes too.
        let path = root.join(crate::ids::item_path(
            &crate::id::Id::parse(&a, "item").unwrap(),
        ));
        std::fs::remove_file(path).unwrap();
        handle_safely(
            &mut slot,
            Some(&mut live),
            None,
            &root,
            Request::ProjectInfo {},
        )
        .unwrap();
        assert!(live.get(&dana).unwrap().selection.is_empty());
    }
}
