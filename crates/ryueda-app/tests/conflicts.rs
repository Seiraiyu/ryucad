//! The conflict panel against real daemons and real merges (design 688):
//! a merge opens Conflicts and the panel settles it, a stale revision is
//! refused on its row, Show ours and Show theirs read each side, and a
//! manifest's record opens Conflicts alone. Every repository is the
//! test's own (#920).

mod common;

use common::{OURS, THEIRS, cli, git_ok, merging, one_at_a_time, top_of};

/// Needs no adapter: runs everywhere CI does.
#[test]
fn a_merge_is_made_in_the_tests_own_repository() {
    let _one = one_at_a_time();
    let (top, board) = merging("cf-repo", OURS, THEIRS);
    assert_eq!(top_of(&board), top);
    // `ryueda init` put the driver in this repository's own config.
    assert!(git_ok(
        &top,
        &["config", "--local", "--get-regexp", "^merge\\..*\\.driver$"]
    ));
    assert!(
        top.join(".git").join("MERGE_HEAD").exists(),
        "the merge is in progress"
    );
    let v = cli(&board, &["merge", "status"]);
    let kinds: Vec<&str> = v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"refdes_clash"), "{v}");
}
use common::{daemon, git, gpu, manifest_merging};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryueda_app::core::merge::{Batch, Row, groups, rows};
use ryueda_app::ui::app::App;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

fn window(root: &Path) -> Harness<'static, App> {
    let root = root.to_owned();
    Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, daemon()))
}

/// Steps frames until `done` holds, 30 s at most.
fn frames(h: &mut Harness<'static, App>, done: impl Fn(&App) -> bool) {
    until(h, "", done);
}

/// [`frames`], saying which wait never settled (as #1138 did for
/// ryucad-app's).
fn until(h: &mut Harness<'static, App>, what: &str, done: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !done(h.state()) {
        assert!(Instant::now() < end, "the window never got there: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn kinds(a: &App) -> Vec<String> {
    a.opened()
        .and_then(|w| w.merge.as_ref())
        .map(|m| {
            m.conflicts()
                .iter()
                .filter_map(|c| c["kind"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn row_of(a: &App, kind: &str) -> Row {
    let m = a.opened().unwrap().merge.as_ref().unwrap();
    rows(&m.status)
        .into_iter()
        .find(|r| r.kind == kind)
        .unwrap()
}

/// Nothing waits on the daemon and no resolve is on its way.
fn quiet(a: &App) -> bool {
    a.opened()
        .is_some_and(|w| w.idle() && w.merge.as_ref().is_some_and(|m| !m.resolving))
}

#[test]
fn a_merge_opens_conflicts_and_the_panel_settles_it() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let (top, board) = merging("cf-settle", OURS, THEIRS);
    let mut h = window(&board);
    frames(&mut h, |a| {
        kinds(a).iter().any(|k| k == "refdes_clash") && quiet(a)
    });
    h.step();
    assert!(h.query_by_label("Conflicts").is_some(), "the tile opened");
    assert!(
        h.state().title().contains("(merge in progress)"),
        "{}",
        h.state().title()
    );
    let clash = row_of(h.state(), "refdes_clash");
    let renumber = &clash.actions[0];
    assert_eq!(
        (renumber.repair.as_deref(), renumber.recommended),
        (Some("renumber"), true)
    );
    h.state_mut()
        .opened_mut()
        .unwrap()
        .resolve(Batch::one(&clash, renumber));
    frames(&mut h, |a| {
        quiet(a) && !kinds(a).iter().any(|k| k == "refdes_clash")
    });
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| w.last_resolve().is_some())
    });
    // The rest by their recommended actions, a block's areas together.
    let mut met = Vec::new();
    for _ in 0..8 {
        let m = h.state().opened().unwrap().merge.clone().unwrap();
        if m.clean() {
            break;
        }
        let (singles, groups) = groups(rows(&m.status));
        let b = match (
            groups.first(),
            singles
                .iter()
                .find(|r| r.actions.iter().any(|a| a.recommended)),
        ) {
            (Some(g), _) => {
                met.push("layout_conflict".to_owned());
                Batch::relayout_all(g)
            }
            (None, Some(r)) => {
                met.push(r.kind.clone());
                Batch::one(r, r.actions.iter().find(|a| a.recommended).unwrap())
            }
            (None, None) => panic!("nothing the panel settles: {}", m.status),
        };
        let before = m.status.clone();
        h.state_mut().opened_mut().unwrap().resolve(b);
        frames(&mut h, |a| {
            quiet(a) && a.opened().unwrap().merge.as_ref().unwrap().status != before
        });
    }
    assert!(met.iter().any(|k| k.starts_with("layout_")), "{met:?}");
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.merge.as_ref())
            .is_some_and(|m| m.clean())
    });
    // Undo brings the last resolve's items back; Redo settles them again.
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| w.last_resolve().is_some())
    });
    h.state_mut().opened_mut().unwrap().undo();
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.merge.as_ref())
            .is_some_and(|m| !m.clean())
    });
    h.state_mut().opened_mut().unwrap().redo();
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.merge.as_ref())
            .is_some_and(|m| m.clean())
    });
    // Staged, the shared hook lets the merge commit; the tile closes.
    git(&top, &["add", "-A"]);
    assert!(
        git_ok(&top, &["commit", "-q", "--no-edit"]),
        "the hook refused a settled merge"
    );
    frames(&mut h, |a| a.opened().is_some_and(|w| w.merge.is_none()));
    h.step();
    assert!(h.query_by_label("Conflicts").is_none(), "the tile closed");
}

#[test]
fn a_stale_revision_is_refused_on_its_row() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let (_top, board) = merging("cf-stale", OURS, THEIRS);
    let mut h = window(&board);
    frames(&mut h, |a| {
        kinds(a).iter().any(|k| k == "refdes_clash")
            && quiet(a)
            && a.opened().unwrap().revisions.project.is_some()
    });
    {
        let w = h.state_mut().opened_mut().unwrap();
        w.revisions.project = Some("0".repeat(32));
        for v in w.revisions.blocks.values_mut() {
            *v = "0".repeat(32);
        }
    }
    let clash = row_of(h.state(), "refdes_clash");
    h.state_mut()
        .opened_mut()
        .unwrap()
        .resolve(Batch::one(&clash, &clash.actions[0]));
    frames(&mut h, |a| {
        quiet(a)
            && a.opened()
                .unwrap()
                .merge
                .as_ref()
                .unwrap()
                .refused_for(&clash.id)
                .is_some()
    });
    let m = h.state().opened().unwrap().merge.clone().unwrap();
    assert_eq!(
        m.refused_for(&clash.id).unwrap().code,
        ryu_doc::error::Code::Conflict
    );
    assert!(m.conflict(&clash.id).is_some(), "nothing was written");
    h.step();
    assert!(h.query_by_label_contains("Refused (conflict)").is_some());
}

/// Every owner a side draws on any of the block's sheets.
fn side_refs(h: &mut Harness<'static, App>, row: &Row, side: &str) -> BTreeSet<String> {
    let commit = h.state().opened().unwrap().merge.as_ref().unwrap().status[side]["commit"]
        .as_str()
        .unwrap()
        .to_owned();
    h.state_mut().opened_mut().unwrap().show_side(row, side);
    let at = |a: &App, sheet: Option<&str>| {
        a.opened().and_then(|w| w.scene.as_ref()).is_some_and(|s| {
            s.read_only()
                && s.commit.as_deref() == Some(commit.as_str())
                && sheet.is_none_or(|x| s.sheet == x)
        })
    };
    frames(h, |a| at(a, None));
    let sheets: Vec<String> = h
        .state()
        .opened()
        .unwrap()
        .scene
        .as_ref()
        .unwrap()
        .sheets
        .iter()
        .map(|(s, _)| s.clone())
        .collect();
    let mut refs = BTreeSet::new();
    for s in sheets {
        {
            let w = h.state_mut().opened_mut().unwrap();
            w.shown.sheet = Some(s.clone());
            w.show_side(row, side);
        }
        frames(h, |a| at(a, Some(&s)));
        let sc = h.state().opened().unwrap().scene.as_ref().unwrap();
        refs.extend(sc.owners.iter().map(|o| o.reference.clone()));
    }
    refs
}

#[test]
fn show_ours_and_theirs_read_each_side() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let (_top, board) = merging("cf-sides", OURS, THEIRS);
    let mut h = window(&board);
    frames(&mut h, |a| {
        kinds(a).iter().any(|k| k == "refdes_clash") && quiet(a)
    });
    let clash = row_of(h.state(), "refdes_clash");
    assert_eq!(clash.block.as_deref(), Some("breakout"));
    let theirs = side_refs(&mut h, &clash, "theirs");
    assert!(
        theirs.contains("breakout/r9") && !theirs.contains("breakout/r8"),
        "{theirs:?}"
    );
    h.step();
    assert!(
        h.query_by_label_contains("Theirs (").is_some(),
        "the side bar"
    );
    let ours = side_refs(&mut h, &clash, "ours");
    assert!(
        ours.contains("breakout/r8") && !ours.contains("breakout/r9"),
        "{ours:?}"
    );
    h.state_mut().opened_mut().unwrap().back_to_working();
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| {
            w.peek.is_none() && w.idle() && w.scene.as_ref().is_none_or(|s| !s.read_only())
        })
    });
    // Show selects what the clash concerns.
    h.state_mut().opened_mut().unwrap().show_conflict(&clash);
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| w.selection == clash.refs)
    });
}

/// A side shown ends with the merge: committed (unsettled, past the
/// hook), the scene is the working sheet's again.
#[test]
fn a_side_shown_ends_with_the_merge() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let (top, board) = merging("cf-side-ends", OURS, THEIRS);
    let mut h = window(&board);
    frames(&mut h, |a| {
        kinds(a).iter().any(|k| k == "refdes_clash") && quiet(a)
    });
    let clash = row_of(h.state(), "refdes_clash");
    h.state_mut()
        .opened_mut()
        .unwrap()
        .show_side(&clash, "theirs");
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.scene.as_ref())
            .is_some_and(|s| s.read_only())
    });
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "--no-verify", "--no-edit"]);
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| {
            w.merge.is_none()
                && w.peek.is_none()
                && w.idle()
                && w.scene.as_ref().is_some_and(|s| !s.read_only())
        })
    });
    h.step();
    assert!(h.query_by_label("Conflicts").is_none(), "the tile closed");
}

/// A resolve asked for after the last one's answer but before the
/// revisions after it waits for them, rather than going with the old
/// ones and being refused with `conflict`.
#[test]
fn a_quick_second_resolve_waits_for_the_revisions() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let (_top, board) = merging("cf-quick", OURS, THEIRS);
    let mut h = window(&board);
    frames(&mut h, |a| {
        kinds(a).iter().any(|k| k == "refdes_clash")
            && quiet(a)
            && a.opened().unwrap().revisions.project.is_some()
    });
    let m = h.state().opened().unwrap().merge.clone().unwrap();
    let (singles, _) = groups(rows(&m.status));
    // The clash's Renumber, then another item of the same block (its
    // layout's Take ours), guarded by the block's revision the first moves.
    let picks: Vec<Batch> = singles
        .iter()
        .filter(|r| r.block.as_deref() == Some("breakout"))
        .filter_map(|r| {
            r.actions
                .iter()
                .find(|a| a.recommended)
                .or_else(|| r.actions.first())
                .map(|a| Batch::one(r, a))
        })
        .take(2)
        .collect();
    assert_eq!(picks.len(), 2, "two items to settle: {}", m.status);
    let second_ids = picks[1].ids();
    let w = h.state_mut().opened_mut().unwrap();
    let before = w.revisions.clone();
    w.resolve(picks[0].clone());
    // Taking events one at a time: stops on the resolve's answer, before
    // the `project.info` it asks is answered.
    assert!(w.wait_until(Duration::from_secs(30), |w| {
        w.merge.as_ref().is_none_or(|m| !m.resolving)
    }));
    assert!(w.revisions_due && w.revisions == before, "the gap");
    assert!(w.resolve_busy(), "the panel's buttons are off");
    w.resolve(picks[1].clone());
    assert!(
        w.merge.as_ref().is_some_and(|m| !m.resolving),
        "held, not sent"
    );
    assert!(w.wait_until(Duration::from_secs(30), |w| {
        w.idle()
            && !w.revisions_due
            && w.merge.as_ref().is_some_and(|m| {
                !m.resolving && second_ids.iter().all(|id| m.conflict(id).is_none())
            })
    }));
    let m = w.merge.as_ref().unwrap();
    assert!(m.refused.is_none(), "refused: {:?}", m.refused);
}

#[test]
fn a_manifest_record_opens_conflicts_alone() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let (_top, board) = manifest_merging("cf-manifest");
    let id = "ryueda.json#/properties/rev";
    let mut h = window(&board);
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| {
            w.unloaded && w.merge.as_ref().is_some_and(|m| m.conflict(id).is_some())
        })
    });
    assert!(
        h.state().opened().unwrap().scene.is_none(),
        "no sheet over the record"
    );
    h.step();
    assert!(h.query_by_label("Conflicts").is_some());
    let row = {
        let m = h.state().opened().unwrap().merge.as_ref().unwrap();
        rows(&m.status).into_iter().find(|r| r.id == id).unwrap()
    };
    let theirs = row
        .actions
        .iter()
        .find(|a| a.choice == "theirs")
        .unwrap()
        .clone();
    h.state_mut()
        .opened_mut()
        .unwrap()
        .resolve(Batch::one(&row, &theirs));
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| !w.unloaded && w.scene.as_ref().is_some_and(|s| s.block == "breakout"))
    });
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(board.join("ryueda.json")).unwrap()).unwrap();
    assert_eq!(m["properties"]["rev"], "theirs", "{m}");
}

/// #1103: the part a value conflict is in is drawn, at the value its file
/// holds, and Show frames it; the navigator keeps its tree.
#[test]
fn a_conflicted_part_is_drawn_and_show_frames_it() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let (_top, board) = merging(
        "cf-value",
        &[&["component", "edit", "breakout/r1", "--value", "5k6"]],
        &[&["component", "edit", "breakout/r1", "--value", "6k8"]],
    );
    let mut h = window(&board);
    until(&mut h, "the value conflict, quiet", |a| {
        kinds(a).iter().any(|k| k == "value") && quiet(a)
    });
    until(&mut h, "the navigator's tree under breakout", |a| {
        a.opened().is_some_and(|w| {
            w.nav.design_ids().any(|d| {
                let t = w.nav.tree(d);
                t.block == "breakout" && !t.children.is_empty()
            })
        })
    });
    until(&mut h, "the sheet drawn, with R1", |a| {
        a.opened().is_some_and(|w| {
            w.camera.is_some()
                && w.scene
                    .as_ref()
                    .is_some_and(|s| s.owner("breakout/r1").is_some())
        })
    });
    let row = row_of(h.state(), "value");
    assert!(
        row.refs.iter().any(|r| r == "breakout/r1"),
        "{:?}",
        row.refs
    );
    let whole = h.state().opened().unwrap().camera.unwrap().zoom;
    let r1 = {
        let sc = h.state().opened().unwrap().scene.as_ref().unwrap();
        let b = sc.owners[sc.owner("breakout/r1").unwrap()]
            .boxed
            .expect("R1's box");
        [(b[0][0] + b[1][0]) / 2.0, (b[0][1] + b[1][1]) / 2.0]
    };
    h.state_mut().opened_mut().unwrap().show_conflict(&row);
    // The canvas takes the frame Show asks for on the next frame drawn,
    // so what is seen is the camera it leaves: on R1, closer than the
    // whole sheet.
    until(&mut h, "Show framing R1", |a| {
        a.opened().is_some_and(|w| {
            w.selection == row.refs
                && w.frame.is_none()
                && w.camera.as_ref().is_some_and(|c| {
                    c.zoom > whole * 1.5
                        && (c.centre[0] - r1[0]).abs() < 1.0
                        && (c.centre[1] - r1[1]).abs() < 1.0
                })
        })
    });
    let sc = h.state().opened().unwrap().scene.as_ref().unwrap();
    let r1 = sc.owner("breakout/r1").expect("R1 drawn while unsettled");
    assert!(sc.owners[r1].unsettled, "R1 marked unsettled");
}

/// #1104: the typed-value command wraps inside the Conflicts tile, its
/// start never left of the row it belongs to.
#[test]
fn a_typed_values_command_stays_inside_its_tile() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let (_top, board) = merging(
        "cf-typed",
        &[&["component", "edit", "breakout/r1", "--value", "5k6"]],
        &[&["component", "edit", "breakout/r1", "--value", "6k8"]],
    );
    let root = board.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 832.0))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, daemon()));
    frames(&mut h, |a| {
        kinds(a).iter().any(|k| k == "value") && quiet(a)
    });
    for _ in 0..3 {
        h.step();
    }
    let lead = h
        .query_by_label("A value of your own:")
        .expect("the row")
        .rect();
    let cmd = h
        .query_by_label_contains("ryueda merge resolve")
        .expect("the command")
        .rect();
    assert!(
        cmd.min.x >= lead.min.x - 0.5,
        "clipped left: {cmd:?} vs {lead:?}"
    );
    assert!(cmd.max.x <= 1280.0, "{cmd:?}");
}
