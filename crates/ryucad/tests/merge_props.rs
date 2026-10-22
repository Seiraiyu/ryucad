//! The merge rules on generated files: sketches, part lists and parameter
//! files, each edited at random on two sides. No change on one side gives
//! the other, the same change on both gives it, changes to different keys
//! commute, and every result is canonical: clean, or carrying conflicts
//! that make loading fail with `merge_conflict`.

use proptest::prelude::*;
use ryucad::doc::canonical;
use ryucad::merge::{Outcome, bytes, merge};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One edit to a sketch. Points and lines are picked by index; new IDs are
/// one past the largest of their prefix, as ryucad generates them, so two
/// sides adding at once collide.
#[derive(Clone, Debug)]
enum Edit {
    Move(usize, i32, i32),
    AddPoint(i32, i32),
    AddLine(usize, usize),
    Remove(usize),
    Dimension(usize, u8),
    Construction(usize),
    Horizontal(usize),
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (0usize..64, -50i32..50, -50i32..50).prop_map(|(i, x, y)| Edit::Move(i, x, y)),
        (-50i32..50, -50i32..50).prop_map(|(x, y)| Edit::AddPoint(x, y)),
        (0usize..64, 0usize..64).prop_map(|(a, b)| Edit::AddLine(a, b)),
        (0usize..64).prop_map(Edit::Remove),
        (0usize..64, 1u8..40).prop_map(|(i, v)| Edit::Dimension(i, v)),
        (0usize..64).prop_map(Edit::Construction),
        (0usize..64).prop_map(Edit::Horizontal),
    ]
}

/// A square's corners and sides, with a length on the first.
fn base_sketch() -> Value {
    let mut entities = serde_json::Map::new();
    let corners = [[0, 0], [10, 0], [10, 10], [0, 10]];
    for (i, [x, y]) in corners.iter().enumerate() {
        entities.insert(
            format!("p{}", i + 1),
            json!({"type": "point", "m": [*x as f64 / 1000.0, *y as f64 / 1000.0]}),
        );
        entities.insert(
            format!("l{}", i + 1),
            json!({"type": "line", "start": format!("p{}", i + 1),
                   "end": format!("p{}", (i + 1) % 4 + 1)}),
        );
    }
    json!({"format": 2, "kind": "ryucad.feature", "type": "sketch", "plane": "xy",
           "entities": entities,
           "constraints": {"d1": {"type": "length", "a": "l1", "value": "10 mm"}}})
}

/// The IDs of `group` with `prefix`, in order.
fn of(v: &Value, group: &str, prefix: &str) -> Vec<String> {
    v[group]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(k, e)| {
            k.starts_with(prefix)
                && (group != "entities"
                    || e["type"] == if prefix == "p" { "point" } else { "line" })
        })
        .map(|(k, _)| k.clone())
        .collect()
}

/// One past the largest ID with `prefix` across entities and constraints.
fn next(v: &Value, prefix: &str) -> String {
    let n = ["entities", "constraints"]
        .iter()
        .flat_map(|g| v[*g].as_object().unwrap().keys())
        .filter_map(|k| k.strip_prefix(prefix)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}{}", n + 1)
}

/// Applies `e` to `v`, keeping it a valid sketch: removing an entity
/// removes what names it.
fn apply(v: &mut Value, e: &Edit) {
    let pick = |list: &[String], i: usize| (!list.is_empty()).then(|| list[i % list.len()].clone());
    let (points, lines) = (of(v, "entities", "p"), of(v, "entities", "l"));
    match *e {
        Edit::Move(i, x, y) => {
            if let Some(p) = pick(&points, i) {
                v["entities"][p]["m"] = json!([x as f64 / 1000.0, y as f64 / 1000.0]);
            }
        }
        Edit::AddPoint(x, y) => {
            let id = next(v, "p");
            v["entities"][id] =
                json!({"type": "point", "m": [x as f64 / 1000.0, y as f64 / 1000.0]});
        }
        Edit::AddLine(a, b) => {
            if let (Some(a), Some(b)) = (pick(&points, a), pick(&points, b))
                && a != b
            {
                let id = next(v, "l");
                v["entities"][id] = json!({"type": "line", "start": a, "end": b});
            }
        }
        Edit::Remove(i) => {
            let all: Vec<String> = v["entities"].as_object().unwrap().keys().cloned().collect();
            if let Some(id) = pick(&all, i) {
                let mut gone = vec![id];
                while let Some(g) = gone.pop() {
                    for group in ["entities", "constraints"] {
                        let o = v[group].as_object_mut().unwrap();
                        o.remove(&g);
                        let users: Vec<String> = o
                            .iter()
                            .filter(|(_, m)| ryucad::merge::names_in(m).contains(&g.as_str()))
                            .map(|(k, _)| k.clone())
                            .collect();
                        gone.extend(users);
                    }
                }
            }
        }
        Edit::Dimension(i, mm) => {
            if let Some(l) = pick(&lines, i) {
                let existing = v["constraints"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .find(|(_, c)| c["type"] == "length" && c["a"] == l.as_str())
                    .map(|(k, _)| k.clone());
                let id = existing.unwrap_or_else(|| next(v, "d"));
                v["constraints"][id] =
                    json!({"type": "length", "a": l, "value": format!("{mm} mm")});
            }
        }
        Edit::Construction(i) => {
            if let Some(l) = pick(&lines, i) {
                let e = v["entities"][l].as_object_mut().unwrap();
                if e.remove("construction").is_none() {
                    e.insert("construction".to_owned(), json!(true));
                }
            }
        }
        Edit::Horizontal(i) => {
            if let Some(l) = pick(&lines, i) {
                let id = next(v, "c");
                v["constraints"][id] = json!({"type": "horizontal", "a": l});
            }
        }
    }
}

fn edited(edits: &[Edit]) -> Value {
    let mut v = base_sketch();
    for e in edits {
        apply(&mut v, e);
    }
    v
}

fn text(v: &Value) -> Vec<u8> {
    canonical::to_string(v).into_bytes()
}

/// The sketch's file, loaded as feature `s` of part `p`.
fn load(bytes: &[u8]) -> ryucad::error::Result<ryucad::doc::files::Feature> {
    let files: BTreeMap<String, Vec<u8>> = [
        (
            "ryucad.json",
            text(&json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"})),
        ),
        (
            "parts/p/part.json",
            text(&json!({"features": ["s"], "format": 1, "kind": "ryucad.part"})),
        ),
        ("parts/p/features/s.json", bytes.to_vec()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let project = ryucad::doc::project::Project::load_from(std::path::Path::new("/x"), &files)?;
    let part = &project.parts[&ryucad::doc::id::Id::parse("p", "part").unwrap()];
    part.features[&ryucad::doc::id::Id::parse("s", "feature").unwrap()].clone()
}

/// Checks what every merge must be: canonical, conflicted exactly when it
/// says so, and refused by loading when conflicted.
fn sound(out: &[u8], outcome: Outcome) {
    let text = std::str::from_utf8(out).unwrap();
    assert!(canonical::is_canonical(text), "{text}");
    let v: Value = serde_json::from_str(text).unwrap();
    let record = ryucad::merge::record(&v);
    match outcome {
        Outcome::Clean => assert!(v.get("conflicts").is_none()),
        Outcome::Conflicts(n) => {
            assert_eq!(record.len(), n);
            let e = load(out).unwrap_err();
            assert_eq!(e.code, ryucad::error::Code::MergeConflict);
        }
        Outcome::Text(_) => panic!("canonical files never fall back to lines"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn a_side_that_changed_nothing_gives_the_other(edits in prop::collection::vec(edit(), 0..8)) {
        let base = base_sketch();
        let theirs = edited(&edits);
        prop_assert_eq!(bytes(&text(&base), &text(&base), &text(&theirs)), (text(&theirs), Outcome::Clean));
        prop_assert_eq!(bytes(&text(&base), &text(&theirs), &text(&base)), (text(&theirs), Outcome::Clean));
        prop_assert_eq!(bytes(&text(&base), &text(&theirs), &text(&theirs)), (text(&theirs), Outcome::Clean));
        // Clean sketches made this way load.
        prop_assert!(load(&text(&theirs)).is_ok());
    }

    #[test]
    fn every_merge_is_canonical(
        a in prop::collection::vec(edit(), 0..8),
        b in prop::collection::vec(edit(), 0..8),
    ) {
        let (base, ours, theirs) = (base_sketch(), edited(&a), edited(&b));
        let (out, outcome) = bytes(&text(&base), &text(&ours), &text(&theirs));
        sound(&out, outcome);
        // The same three files merge to the same bytes every time.
        prop_assert_eq!(bytes(&text(&base), &text(&ours), &text(&theirs)), (out, outcome));
    }

    #[test]
    fn changes_to_different_entities_commute(
        a in prop::collection::vec((0usize..4, -50i32..50, 1u8..40), 0..6),
        b in prop::collection::vec((0usize..4, -50i32..50, 1u8..40), 0..6),
    ) {
        // Ours edits the first two corners and sides, theirs the last two;
        // ours adds under `o`-prefixed IDs, theirs under `t`.
        let side = |edits: &[(usize, i32, u8)], first: usize, prefix: &str| {
            let mut v = base_sketch();
            for (k, &(i, x, mm)) in edits.iter().enumerate() {
                let n = first + i % 2 + 1;
                match i {
                    0 => v["entities"][format!("p{n}")]["m"] = json!([x as f64 / 1000.0, 0.0]),
                    1 => v["entities"][format!("l{n}")]["construction"] = json!(true),
                    2 => v["constraints"][format!("{prefix}d{k}")] =
                        json!({"type": "length", "a": format!("l{n}"), "value": format!("{mm} mm")}),
                    _ => v["entities"][format!("{prefix}p{k}")] =
                        json!({"type": "point", "m": [x as f64 / 1000.0, 0.01]}),
                }
            }
            v
        };
        let (ours, theirs) = (side(&a, 0, "o"), side(&b, 2, "t"));
        let base = base_sketch();
        let one = merge(Some(&base), &ours, &theirs);
        let other = merge(Some(&base), &theirs, &ours);
        prop_assert!(one.is_clean(), "{:?}", one.conflicts);
        prop_assert_eq!(&one.value, &other.value);
        prop_assert!(load(&text(&one.value)).is_ok());
    }

    #[test]
    fn lists_keep_what_both_keep(
        drop_a in prop::collection::btree_set(0usize..8, 0..3),
        drop_b in prop::collection::btree_set(0usize..8, 0..3),
        add_a in prop::collection::vec(0usize..9, 0..3),
        add_b in prop::collection::vec(0usize..9, 0..3),
    ) {
        let base: Vec<String> = (0..8).map(|i| format!("f{i}")).collect();
        let side = |drop: &std::collections::BTreeSet<usize>, add: &[usize], prefix: &str| {
            let mut l: Vec<String> = base
                .iter()
                .enumerate()
                .filter(|(i, _)| !drop.contains(i))
                .map(|(_, f)| f.clone())
                .collect();
            for (k, at) in add.iter().enumerate() {
                l.insert((*at).min(l.len()), format!("{prefix}{k}"));
            }
            l
        };
        let (ours, theirs) = (side(&drop_a, &add_a, "o"), side(&drop_b, &add_b, "t"));
        let part = |l: &[String]| json!({"features": l, "format": 1, "kind": "ryucad.part"});
        let (out, outcome) = bytes(&text(&part(&base)), &text(&part(&ours)), &text(&part(&theirs)));
        prop_assert_eq!(outcome, Outcome::Clean);
        let v: Value = serde_json::from_slice(&out).unwrap();
        let merged: Vec<String> = serde_json::from_value(v["features"].clone()).unwrap();
        for f in &merged {
            prop_assert_eq!(merged.iter().filter(|g| *g == f).count(), 1);
        }
        // Every feature both kept or one added is there, nothing else, and
        // each side's own order is kept.
        for f in base.iter().filter(|f| ours.contains(f) && theirs.contains(f)).chain(
            ours.iter().chain(&theirs).filter(|f| !base.contains(f)),
        ) {
            prop_assert!(merged.contains(f), "{f} missing from {merged:?}");
        }
        prop_assert!(merged.iter().all(|f| ours.contains(f) || theirs.contains(f)));
        for side in [&ours, &theirs] {
            let kept: Vec<&String> = side.iter().filter(|f| merged.contains(f)).collect();
            let order: Vec<&String> = merged.iter().filter(|f| side.contains(f)).collect();
            prop_assert_eq!(kept, order);
        }
    }

    #[test]
    fn parameters_merge_per_name(
        a in prop::collection::btree_map(0u8..6, 1u8..9, 0..4),
        b in prop::collection::btree_map(0u8..6, 1u8..9, 0..4),
    ) {
        let file = |p: &BTreeMap<u8, u8>| {
            let ps: serde_json::Map<String, Value> =
                p.iter().map(|(k, v)| (format!("p{k}"), json!(format!("{v} mm")))).collect();
            json!({"format": 1, "kind": "ryucad.parameters", "parameters": ps})
        };
        let base = BTreeMap::new();
        let m = merge(Some(&file(&base)), &file(&a), &file(&b));
        for (k, v) in a.iter().chain(&b) {
            let clash = a.get(k).zip(b.get(k)).is_some_and(|(x, y)| x != y);
            let at = format!("/parameters/p{k}");
            prop_assert_eq!(m.conflicts.contains_key(&at), clash);
            if !clash {
                prop_assert_eq!(&m.value["parameters"][format!("p{k}")], &json!(format!("{v} mm")));
            }
        }
    }
}
