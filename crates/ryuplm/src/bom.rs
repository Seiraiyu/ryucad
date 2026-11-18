//! BOMs (design 734): an item's own lines (`bom` in its file) plus the
//! lines its representations give, each line held in exactly one place.
//! This file judges own lines; the joined BOM, `bom show`, the findings
//! and the diff build on it.

use crate::error::{Code, Error, Result};
use crate::ids::item_path;
use crate::item::Line;
use crate::session::Session;
use crate::write::{Writes, commit};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use crate::id::{Id, ProjectId};
use crate::item::ItemRef;
use crate::load::Catalogue;
use std::collections::{BTreeMap, BTreeSet};

/// The version of the shape `bom show`, `bom diff` and `where-used`
/// answer with. Adding a field is not a change.
pub const SCHEMA: u64 = 1;

/// Whether `q` is a line's quantity: a positive decimal number as
/// written (`4`, `0.5`), digits only, never signed, exponent or unit.
pub fn quantity_ok(q: &str) -> bool {
    let (int, frac) = q.split_once('.').unwrap_or((q, "0"));
    !int.is_empty()
        && !frac.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
        && q.bytes().any(|b| (b'1'..=b'9').contains(&b))
}

/// The child's ID when it is this catalogue's: named bare, or by this
/// catalogue's project ID.
pub fn local<'a>(own: &ProjectId, r: &'a ItemRef) -> Option<&'a Id> {
    match &r.catalogue {
        None => Some(&r.item),
        Some(p) if p == own => Some(&r.item),
        Some(_) => None,
    }
}

/// Each item's own lines' children that the catalogue holds, in line
/// order.
pub fn own_edges(cat: &Catalogue) -> BTreeMap<Id, Vec<Id>> {
    let own = &cat.manifest.project;
    cat.items
        .iter()
        .map(|(id, item)| {
            let kids = item
                .bom
                .iter()
                .filter_map(|l| local(own, &l.item))
                .filter(|c| cat.items.contains_key(*c))
                .cloned()
                .collect();
            (id.clone(), kids)
        })
        .collect()
}

/// The cycles of `edges`, one per edge back to an item still being
/// walked, each rotated to start at its least ID; the list sorted, so
/// the answer doesn't depend on where the walk began.
pub fn cycles(edges: &BTreeMap<Id, Vec<Id>>) -> Vec<Vec<Id>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Open,
        Done,
    }
    let none: Vec<Id> = Vec::new();
    let mut mark: BTreeMap<&Id, Mark> = BTreeMap::new();
    let mut found: BTreeSet<Vec<Id>> = BTreeSet::new();
    for start in edges.keys() {
        if mark.contains_key(start) {
            continue;
        }
        let mut path: Vec<&Id> = vec![start];
        let mut next: Vec<usize> = vec![0];
        mark.insert(start, Mark::Open);
        while let Some(&node) = path.last() {
            let kids = edges.get(node).unwrap_or(&none);
            let i = next.last().copied().unwrap_or(0);
            if i < kids.len() {
                if let Some(n) = next.last_mut() {
                    *n += 1;
                }
                let k = &kids[i];
                match mark.get(k).copied() {
                    None => {
                        mark.insert(k, Mark::Open);
                        path.push(k);
                        next.push(0);
                    }
                    Some(Mark::Open) => {
                        let from = path.iter().position(|n| *n == k).unwrap_or(0);
                        let mut c: Vec<Id> = path[from..].iter().map(|n| (*n).clone()).collect();
                        let least = (0..c.len()).min_by(|a, b| c[*a].cmp(&c[*b])).unwrap_or(0);
                        c.rotate_left(least);
                        found.insert(c);
                    }
                    Some(Mark::Done) => {}
                }
            } else {
                mark.insert(node, Mark::Done);
                path.pop();
                next.pop();
            }
        }
    }
    found.into_iter().collect()
}

/// `bom_cycle` for one cycle, named at its first item.
pub fn cycle_error(c: &[Id]) -> Error {
    let names: Vec<&str> = c.iter().map(Id::as_str).collect();
    Error::new(
        Code::BomCycle,
        format!(
            "items {} name each other in a loop of BOM lines",
            names.join(" -> ")
        ),
    )
    .with("item", &c[0])
    .with("cycle", c)
}

/// Own-line faults, all errors a write refuses: a quantity or unit that
/// doesn't read (`invalid_quantity`), a child this catalogue hasn't
/// (`bom_item_missing`), each at its line, then every cycle of own lines.
pub fn judge(cat: &Catalogue) -> Vec<Error> {
    let own = &cat.manifest.project;
    let mut out = Vec::new();
    for (id, item) in &cat.items {
        for (i, l) in item.bom.iter().enumerate() {
            let at = format!("/bom/{i}");
            if !quantity_ok(&l.quantity) {
                out.push(
                    Error::new(
                        Code::InvalidQuantity,
                        format!(
                            "item {id}: line {} has quantity {:?}: a positive decimal number, in the child's unit",
                            l.line, l.quantity
                        ),
                    )
                    .with("item", id)
                    .with("line", &l.line)
                    .with("pointer", format!("{at}/quantity")),
                );
            }
            if let Some(u) = &l.unit
                && !crate::item::UNITS.contains(&u.as_str())
            {
                out.push(
                    Error::new(
                        Code::InvalidQuantity,
                        format!(
                            "item {id}: line {} has unit {u:?}: each, m, m2, m3, kg or l",
                            l.line
                        ),
                    )
                    .with("item", id)
                    .with("line", &l.line)
                    .with("pointer", format!("{at}/unit")),
                );
            }
            if local(own, &l.item).is_some_and(|c| !cat.items.contains_key(c)) {
                out.push(
                    Error::new(
                        Code::BomItemMissing,
                        format!(
                            "item {id}: line {} names item {}, which the catalogue hasn't",
                            l.line,
                            l.item.text()
                        ),
                    )
                    .with("item", id)
                    .with("line", &l.line)
                    .with("child", l.item.text())
                    .with("pointer", format!("{at}/item")),
                );
            }
        }
    }
    out.extend(cycles(&own_edges(cat)).iter().map(|c| cycle_error(c)));
    out
}

/// What `bom add` and `bom edit` set on a line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineFields {
    /// The child item: its ID or number, or `<catalogue project ID>:<item ID>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub find: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refdes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// The fields `bom edit --unset` may clear.
const UNSETTABLE: [&str; 4] = ["unit", "find", "refdes", "notes"];

/// The child `s` names: another catalogue's by `<project>:<id>`, else
/// this one's by ID or number.
fn child_of(cat: &Catalogue, s: &str) -> Result<ItemRef> {
    if s.contains(':') {
        return ItemRef::parse(s).ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                format!("{s:?} isn't <catalogue project ID>:<item ID>"),
            )
            .with("param", "child")
        });
    }
    Ok(ItemRef {
        catalogue: None,
        item: cat.find(s)?,
    })
}

fn write_item(session: &mut Session, id: &Id, it: &crate::item::Item, what: &str) -> Result<Value> {
    let text = crate::items::text_of(id, it)?;
    commit(session, what, Writes(vec![(item_path(id), Some(text))]))?;
    Ok(crate::items::shown(session, id))
}

/// `bom.add`: a new own line on `item`, with a fresh line ID.
pub fn add(
    session: &mut Session,
    item: &str,
    fields: LineFields,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut it) = crate::items::current(session, item, expected)?;
    let child = fields
        .child
        .as_deref()
        .ok_or_else(|| Error::new(Code::Usage, "a line needs its child item"))?;
    let child = child_of(session.catalogue(), child)?;
    let quantity = fields
        .quantity
        .clone()
        .ok_or_else(|| Error::new(Code::Usage, "a line needs its quantity"))?;
    let line = crate::ids::random(|s| it.bom.iter().any(|l| l.line.as_str() == s))?;
    it.bom.push(Line {
        line: line.clone(),
        item: child,
        quantity,
        unit: fields.unit,
        find: fields.find,
        refdes: fields.refdes,
        notes: fields.notes,
    });
    let mut v = write_item(session, &id, &it, &format!("adding line {line} to item {id}"))?;
    v["line"] = json!(line);
    Ok(v)
}

fn line_index(id: &Id, it: &crate::item::Item, line: &str) -> Result<usize> {
    it.bom
        .iter()
        .position(|l| l.line.as_str() == line)
        .ok_or_else(|| {
            Error::new(Code::NotFound, format!("item {id} has no line {line}"))
                .with("line", line)
        })
}

/// `bom.edit`: the fields given set on the line, those in `unset` cleared.
pub fn edit(
    session: &mut Session,
    item: &str,
    line: &str,
    fields: LineFields,
    unset: Vec<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut it) = crate::items::current(session, item, expected)?;
    let i = line_index(&id, &it, line)?;
    let child = fields
        .child
        .as_deref()
        .map(|c| child_of(session.catalogue(), c))
        .transpose()?;
    let l = &mut it.bom[i];
    if let Some(c) = child {
        l.item = c;
    }
    if let Some(q) = fields.quantity {
        l.quantity = q;
    }
    for (slot, v) in [
        (&mut l.unit, fields.unit),
        (&mut l.find, fields.find),
        (&mut l.refdes, fields.refdes),
        (&mut l.notes, fields.notes),
    ] {
        if v.is_some() {
            *slot = v;
        }
    }
    for u in &unset {
        match u.as_str() {
            "unit" => l.unit = None,
            "find" => l.find = None,
            "refdes" => l.refdes = None,
            "notes" => l.notes = None,
            other => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("can't unset {other:?}: {}", UNSETTABLE.join(", ")),
                )
                .with("field", other));
            }
        }
    }
    write_item(session, &id, &it, &format!("editing line {line} of item {id}"))
}

/// `bom.remove`: the line taken off the item.
pub fn remove(
    session: &mut Session,
    item: &str,
    line: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut it) = crate::items::current(session, item, expected)?;
    let i = line_index(&id, &it, line)?;
    it.bom.remove(i);
    let mut v = write_item(session, &id, &it, &format!("removing line {line} from item {id}"))?;
    v["removed"] = json!(line);
    Ok(v)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::item::{Item, ItemRef, Line};
    use crate::judge::tests::{codes, files};

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "item").unwrap()
    }

    /// An own line `n` (its ID `000000000000<n>`) naming `child`.
    pub(crate) fn line(n: u8, child: &str, quantity: &str) -> Line {
        Line {
            line: id(&format!("000000000000{n}")),
            item: ItemRef::parse(child).unwrap(),
            quantity: quantity.into(),
            unit: None,
            find: None,
            refdes: None,
            notes: None,
        }
    }

    #[test]
    fn quantities_read_as_positive_decimals() {
        for q in ["1", "4", "0.5", "12.250", "10"] {
            assert!(quantity_ok(q), "{q}");
        }
        for q in ["0", "0.0", "", "-1", "1.", ".5", "1e3", "2 mm", " 1"] {
            assert!(!quantity_ok(q), "{q}");
        }
    }

    #[test]
    fn own_lines_are_judged_in_the_catalogue() {
        let mut a = Item::new("A-1");
        a.bom = vec![
            line(1, "0000000000002", "4"),
            line(2, "0000000000009", "1"), // not in the catalogue
            line(3, "0000000000002", "0"), // bad quantity
        ];
        a.bom[0].unit = Some("pcs".into()); // bad unit
        let mut b = Item::new("A-2");
        // Another catalogue's item: check's business, not a write's.
        b.bom = vec![line(1, "0123456789abcdef0123456789abcdef:0000000000009", "1")];
        let f = files(&[], None, &[("0000000000001", a), ("0000000000002", b)]);
        assert_eq!(
            codes(&f),
            [
                ("invalid_quantity".into(), "error", "0000000000001".into()),
                ("bom_item_missing".into(), "error", "0000000000001".into()),
                ("invalid_quantity".into(), "error", "0000000000001".into()),
            ]
        );
    }

    #[test]
    fn a_loop_of_own_lines_is_one_cycle() {
        let mut a = Item::new("A-1");
        a.bom = vec![line(1, "0000000000002", "1")];
        let mut b = Item::new("A-2");
        b.bom = vec![line(1, "0000000000003", "1")];
        let mut c = Item::new("A-3");
        c.bom = vec![line(1, "0000000000001", "1")];
        let mut s = Item::new("A-4");
        s.bom = vec![line(1, "0000000000004", "1")]; // itself
        let f = files(
            &[],
            None,
            &[
                ("0000000000002", b),
                ("0000000000003", c),
                ("0000000000001", a),
                ("0000000000004", s),
            ],
        );
        let cat = crate::load::Catalogue::load(&f).unwrap();
        let found = cycles(&own_edges(&cat));
        assert_eq!(
            found,
            [
                vec![id("0000000000001"), id("0000000000002"), id("0000000000003")],
                vec![id("0000000000004")],
            ]
        );
        assert_eq!(
            codes(&f),
            [
                ("bom_cycle".into(), "error", "0000000000001".into()),
                ("bom_cycle".into(), "error", "0000000000004".into()),
            ]
        );
    }

    #[test]
    fn own_lines_from_add_to_remove() {
        use crate::proto::Request;
        use crate::server::tests::{ask, catalogue};
        let (_d, root) = catalogue();
        for n in ["KIT-1", "SCREW-1", "LABEL-1"] {
            ask(
                &root,
                Request::ItemCreate {
                    fields: Default::default(),
                    number: Some(n.into()),
                    next: None,
                    expected_revision: None,
                },
            )
            .unwrap();
        }
        let fields = |v: serde_json::Value| -> LineFields { serde_json::from_value(v).unwrap() };
        let added = ask(
            &root,
            Request::BomAdd {
                item: "KIT-1".into(),
                fields: fields(serde_json::json!({"child": "SCREW-1", "quantity": "4", "find": "10"})),
                expected_revision: None,
            },
        )
        .unwrap();
        let line = added["line"].as_str().unwrap().to_owned();
        assert_eq!(added["item"]["bom"][0]["quantity"], "4");
        assert_eq!(added["item"]["bom"][0]["find"], "10");
        // A bad quantity is refused by the judge, at its pointer.
        let bad = ask(
            &root,
            Request::BomAdd {
                item: "KIT-1".into(),
                fields: fields(serde_json::json!({"child": "LABEL-1", "quantity": "lots"})),
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(bad.code, Code::InvalidQuantity);
        // A loop is refused.
        let lp = ask(
            &root,
            Request::BomAdd {
                item: "SCREW-1".into(),
                fields: fields(serde_json::json!({"child": "KIT-1", "quantity": "1"})),
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(lp.code, Code::BomCycle);
        // The child can't be deleted while a line names it.
        let del = ask(
            &root,
            Request::ItemDelete {
                item: "SCREW-1".into(),
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(del.code, Code::BomItemMissing);
        let edited = ask(
            &root,
            Request::BomEdit {
                item: "KIT-1".into(),
                line: line.clone(),
                fields: fields(serde_json::json!({"quantity": "6", "notes": "M3x8"})),
                unset: vec!["find".into()],
                expected_revision: None,
            },
        )
        .unwrap();
        let l = &edited["item"]["bom"][0];
        assert_eq!((l["quantity"].clone(), l["notes"].clone()), (serde_json::json!("6"), serde_json::json!("M3x8")));
        assert!(l.get("find").is_none(), "{l}");
        let removed = ask(
            &root,
            Request::BomRemove {
                item: "KIT-1".into(),
                line: line.clone(),
                expected_revision: None,
            },
        )
        .unwrap();
        assert!(removed["item"].get("bom").is_none(), "{removed}");
        let gone = ask(
            &root,
            Request::BomRemove {
                item: "KIT-1".into(),
                line,
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(gone.code, Code::NotFound);
    }
}
