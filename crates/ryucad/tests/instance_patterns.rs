//! Instance patterns that follow a part (issue #547, design 521):
//! batteries in a tray whose holes are a pattern with a skipped row,
//! screws following a hole's points and a sketch's, keys that survive
//! points added and removed, copies following a mated lid, the BOM's
//! counts, and the refusals.

mod common;

use common::{call, must};
use ryucad::doc::assembly::Rev;
use ryucad::doc::id::Id;
use ryucad::error::Code;
use ryucad::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn id(s: &str) -> Id {
    Id::parse(s, "id").unwrap()
}

/// Task 8's `places::tests::tray`, word for word, with `common::must`.
fn tray() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    let add = |s: &mut Session, f: Value| must(s, "feature.add", f);
    must(&mut s, "part.create", json!({"part": "tray"}));
    add(&mut s, json!({"part": "tray", "type": "box", "id": "plate",
                       "params": {"x": 60, "y": 40, "z": 5},
                       "placement": {"origin": [0, 0, -5]}}));
    add(&mut s, json!({"part": "tray", "type": "sketch", "id": "seat", "plane": "xy"}));
    must(&mut s, "sketch.edit", json!({"part": "tray", "sketch": "seat",
         "ops": [{"op": "add", "type": "point", "at": [10, 10]}]}));
    add(&mut s, json!({"part": "tray", "type": "hole", "id": "h", "sketch": "seat",
                       "diameter": 3, "depth": "through_all"}));
    add(&mut s, json!({"part": "tray", "type": "pattern", "id": "grid", "features": ["h"],
                       "direction": "x", "spacing": 20, "count": 3,
                       "direction2": "y", "spacing2": 20, "count2": 2, "skip": [[2, 1]]}));
    add(&mut s, json!({"part": "tray", "type": "sketch", "id": "layout", "plane": "xy"}));
    must(&mut s, "sketch.edit", json!({"part": "tray", "sketch": "layout", "ops": [
        {"op": "add", "id": "p1", "type": "point", "at": [5, 35]},
        {"op": "add", "id": "p2", "type": "point", "at": [25, 35]},
        {"op": "add", "id": "p3", "type": "point", "at": [2, 2]},
        {"op": "add", "id": "p4", "type": "point", "at": [8, 2]},
        {"op": "add", "id": "p5", "type": "point", "at": [45, 35]},
        {"op": "add", "type": "line", "start": "p3", "end": "p4"}]}));
    add(&mut s, json!({"part": "tray", "type": "hole", "id": "row", "sketch": "layout",
                       "diameter": 3, "depth": "through_all"}));
    must(&mut s, "part.create", json!({"part": "screw"}));
    add(&mut s, json!({"part": "screw", "type": "cylinder", "id": "shank",
                       "params": {"radius": 1.4, "height": 8}}));
    must(&mut s, "part.create", json!({"part": "pack", "kind": "assembly"}));
    for (i, of, at) in [("lid", "tray", [100, 0, 0]), ("screw-0", "screw", [110, 10, 0]),
                        ("bolt-0", "screw", [105, 35, 0]), ("pin-0", "screw", [105, 35, 0])] {
        add(&mut s, json!({"part": "pack", "type": "instance", "id": i, "of": of,
                           "placement": {"origin": at}}));
    }
    (dir, s)
}

fn pattern(s: &mut Session, pid: &str, seed: &str, form: &str, on: &str, more: Value) -> Value {
    let mut f = json!({"part": "pack", "type": "instance_pattern", "id": pid,
                       "instances": [seed], "form": form, "on": on});
    f.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
    must(s, "feature.add", f)
}

/// Every copy's path with its origin, mm.
fn copies(s: &mut Session) -> BTreeMap<String, [i64; 3]> {
    let m = s.model(&id("pack"), &Rev::Working).unwrap();
    m.occurrences
        .iter()
        .filter(|o| o.path[0].copy_of().is_some())
        .map(|o| {
            let t = o.pose.translation_part();
            (
                ryucad::assembly::path_text(&o.path),
                [t.x, t.y, t.z].map(|c| (c * 1e3).round() as i64),
            )
        })
        .collect()
}

fn want(v: &[(&str, [i64; 3])]) -> BTreeMap<String, [i64; 3]> {
    v.iter().map(|(k, a)| (k.to_string(), *a)).collect()
}

#[test]
fn batteries_follow_a_grid_with_a_skipped_row() {
    let (_dir, mut s) = tray();
    pattern(&mut s, "cells", "screw-0", "feature", "pack/lid/grid", json!({}));
    assert_eq!(copies(&mut s), want(&[
        ("cells:1/screw-0", [130, 10, 0]), ("cells:2/screw-0", [150, 10, 0]),
        ("cells:0:1/screw-0", [110, 30, 0]), ("cells:1:1/screw-0", [130, 30, 0]),
    ]));
    // The grid closer along x: every copy moves, none is renamed.
    must(&mut s, "feature.edit", json!({"part": "tray", "id": "grid", "spacing": 15}));
    assert_eq!(copies(&mut s), want(&[
        ("cells:1/screw-0", [125, 10, 0]), ("cells:2/screw-0", [140, 10, 0]),
        ("cells:0:1/screw-0", [110, 30, 0]), ("cells:1:1/screw-0", [125, 30, 0]),
    ]));
    // The skipped cell put back: one copy more, the rest where they were.
    must(&mut s, "feature.edit", json!({"part": "tray", "id": "grid", "skip": []}));
    let c = copies(&mut s);
    assert_eq!((c.len(), c["cells:2:1/screw-0"], c["cells:1/screw-0"]), (5, [140, 30, 0], [125, 10, 0]));
}

#[test]
fn screws_follow_a_hole_and_keep_their_keys_when_points_come_and_go() {
    let (_dir, mut s) = tray();
    pattern(&mut s, "bolts", "bolt-0", "feature", "pack/lid/row", json!({}));
    assert_eq!(copies(&mut s), want(&[("bolts:p2/bolt-0", [125, 35, 0]), ("bolts:p5/bolt-0", [145, 35, 0])]));
    must(&mut s, "sketch.edit", json!({"part": "tray", "sketch": "layout",
         "ops": [{"op": "move", "id": "p2", "at": [30, 35]}]}));
    assert_eq!(copies(&mut s)["bolts:p2/bolt-0"], [130, 35, 0]);
    // A nut fastened to p5's copy, where it is.
    must(&mut s, "feature.add", json!({"part": "pack", "type": "instance", "id": "nut",
         "of": "screw", "fixed": true, "placement": {"origin": [145, 35, 0]}}));
    must(&mut s, "feature.add", json!({"part": "pack", "type": "mate", "id": "m",
         "form": "fastened", "a": "pack/bolts:p5/bolt-0", "b": "pack/nut"}));
    must(&mut s, "sketch.edit", json!({"part": "tray", "sketch": "layout",
         "ops": [{"op": "remove", "id": "p2"}]}));
    assert_eq!(copies(&mut s), want(&[("bolts:p5/bolt-0", [145, 35, 0])]));
    let r = s.rebuilt(&id("pack")).unwrap().clone();
    assert_eq!(r.failed(), 0, "{:?}", r.features);
    assert!(r.solve.as_ref().unwrap().report.converged);
    must(&mut s, "sketch.edit", json!({"part": "tray", "sketch": "layout",
         "ops": [{"op": "add", "id": "p7", "type": "point", "at": [35, 35]}]}));
    assert_eq!(copies(&mut s), want(&[("bolts:p5/bolt-0", [145, 35, 0]), ("bolts:p7/bolt-0", [135, 35, 0])]));
}

#[test]
fn a_points_pattern_with_from_and_listed_points() {
    let (_dir, mut s) = tray();
    // The seed sits at p1's place but names p5 as its own: copies are
    // offset from it as p1 and p2 are from p5.
    pattern(&mut s, "pins", "pin-0", "points", "pack/lid/layout",
            json!({"from": "p5", "points": ["p1", "p2", "p5"]}));
    assert_eq!(copies(&mut s), want(&[("pins:p1/pin-0", [65, 35, 0]), ("pins:p2/pin-0", [85, 35, 0])]));
}

#[test]
fn copies_follow_a_mated_lid() {
    let (_dir, mut s) = tray();
    must(&mut s, "part.create", json!({"part": "base"}));
    must(&mut s, "feature.add", json!({"part": "base", "type": "box", "id": "slab",
         "params": {"x": 300, "y": 100, "z": 5}, "placement": {"origin": [0, 0, -15]}}));
    must(&mut s, "feature.add", json!({"part": "pack", "type": "instance", "id": "base",
         "of": "base", "fixed": true}));
    must(&mut s, "feature.add", json!({"part": "pack", "type": "mate", "id": "hold",
         "form": "fastened", "a": "pack/base", "b": "pack/lid",
         "a_offset": {"origin": [100, 0, 0]}}));
    pattern(&mut s, "screws", "screw-0", "feature", "pack/lid/grid", json!({}));
    for offset in [json!({"origin": [100, 0, 10]}), json!({"origin": [90, 5, 0], "rotate": "30 deg"})] {
        must(&mut s, "feature.edit", json!({"part": "pack", "id": "hold", "a_offset": offset}));
        let m = s.model(&id("pack"), &Rev::Working).unwrap();
        let pose = |path: &str| {
            let o = m.occurrences.iter().find(|o| ryucad::assembly::path_text(&o.path) == path).unwrap();
            ryucad::connector::pose(&o.pose)
        };
        let (lid, seed) = (pose("lid"), pose("screw-0"));
        for (path, step) in [("screws:1/screw-0", [0.02, 0.0, 0.0]), ("screws:1:1/screw-0", [0.02, 0.02, 0.0])] {
            let place = ryumate::Pose { origin: step, ..ryumate::Pose::IDENTITY };
            let want = lid.then_after(&place).then_after(&lid.inverse()).then_after(&seed);
            let got = pose(path);
            for k in 0..3 {
                assert!((got.origin[k] - want.origin[k]).abs() < 1e-9, "{path}: {got:?} {want:?}");
            }
        }
    }
}

#[test]
fn the_bom_counts_copies_from_files() {
    let (_dir, mut s) = tray();
    pattern(&mut s, "screws", "screw-0", "feature", "pack/lid/grid", json!({}));
    pattern(&mut s, "bolts", "bolt-0", "feature", "pack/lid/row", json!({}));
    pattern(&mut s, "pins", "pin-0", "points", "pack/lid/layout", json!({"points": ["p1", "p5"]}));
    let count = |s: &mut Session| {
        let v = must(s, "bom.get", json!({"part": "pack"}));
        v["tree"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["part"] == "screw")
            .map(|r| r["quantity"]["count"].as_u64().unwrap())
            .unwrap()
    };
    // Three seeds, four on the grid, two on the row, one pin.
    assert_eq!(count(&mut s), 10);
    // From files: a hole that fails to build doesn't change the count.
    must(&mut s, "feature.edit", json!({"part": "tray", "id": "row", "diameter": 500}));
    assert_eq!(count(&mut s), 10);
}

#[test]
fn refusals() {
    let (_dir, mut s) = tray();
    let refused = |s: &mut Session, form: &str, on: &str, more: Value| {
        let mut f = json!({"part": "pack", "type": "instance_pattern", "id": "x",
                           "instances": ["pin-0"], "form": form, "on": on});
        f.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        call(s, "feature.add", f).unwrap_err()
    };
    assert_eq!(refused(&mut s, "feature", "pack/lid/layout", json!({})).code, Code::ReferenceWrongKind);
    assert_eq!(refused(&mut s, "points", "pack/lid/row", json!({})).code, Code::ReferenceWrongKind);
    assert_eq!(refused(&mut s, "feature", "pack/lid/h", json!({})).code, Code::OperationNoEffect);
    assert_eq!(refused(&mut s, "feature", "pack/nobody/grid", json!({})).code, Code::NotFound);
    let e = refused(&mut s, "feature", "pack/lid/row", json!({"from": "p3"}));
    assert_eq!((e.code, e.details["pointer"].as_str()), (Code::InvalidParam, Some("/from")));
    let e = refused(&mut s, "feature", "pack/lid/grid", json!({"instances": ["lid"]}));
    assert_eq!(e.details["pointer"], "/instances");
    // A mirror pattern: no instance can be mirrored. Mirrored in a plane
    // at x 25 mm, so its hole is on the plate and clear of the grid's.
    must(&mut s, "feature.add", json!({"part": "tray", "type": "plane", "id": "mid",
         "by": "offset", "from": "yz", "distance": 25}));
    must(&mut s, "feature.add", json!({"part": "tray", "type": "pattern", "id": "flip",
         "features": ["h"], "plane": "tray/mid"}));
    let e = refused(&mut s, "feature", "pack/lid/flip", json!({}));
    assert_eq!(e.code, Code::InvalidParam, "{e}");
}
