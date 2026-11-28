//! The compare view against real daemons (design 688): an agent's
//! compare followed, History's Compare, B, a removed component's Show,
//! a reconnect, over the breakout committed and edited.

mod common;

use common::{agent, breakout_in_git, daemon, gpu, one_at_a_time};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryueda_app::core::compare::Side;
use ryueda_app::core::scene::Diff;
use ryueda_app::ui::app::App;
use serde_json::json;
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
    let end = Instant::now() + Duration::from_secs(30);
    while !done(h.state()) {
        assert!(Instant::now() < end, "the window never got there");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn shown(a: &App) -> Option<(String, String)> {
    let s = a.opened()?.scene.as_ref()?;
    Some((s.block.clone(), s.sheet.clone()))
}

fn comparing(a: &App) -> bool {
    a.opened()
        .and_then(|w| w.scene.as_ref())
        .is_some_and(|s| s.against.is_some() && s.block == "breakout")
}

#[test]
fn an_agents_compare_colours_the_window() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout_in_git("cmp-agent");
    let mut h = window(&root);
    frames(&mut h, |a| shown(a).is_some());
    agent(&root, "compare.set", json!({"rev": "HEAD"}));
    frames(&mut h, comparing);
    let w = h.state().opened().unwrap();
    assert_eq!(w.blink.of.as_deref(), Some("HEAD"));
    let sc = w.scene.as_ref().unwrap();
    assert_eq!(
        sc.owners[sc.owner("breakout/r1").unwrap()].mark,
        Some(Diff::Changed)
    );
    assert!(sc.ghosts.iter().any(|g| g.reference == "breakout/c3"));
    let c = sc.counts.unwrap();
    h.step();
    assert!(h.query_by_label_contains("Against HEAD").is_some());
    assert!(
        h.query_by_label(&format!("{} changed", c.changed))
            .is_some()
    );
    agent(&root, "compare.set", json!({"off": true}));
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| {
            w.blink.of.is_none() && w.scene.as_ref().is_some_and(|s| s.against.is_none())
        })
    });
}

#[test]
fn compare_from_history() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout_in_git("cmp-history");
    let mut h = window(&root);
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.history.as_ref().is_some_and(|x| !x.commits.is_empty()))
    });
    let id = h
        .state()
        .opened()
        .unwrap()
        .history
        .as_ref()
        .unwrap()
        .commits[0]
        .id
        .clone();
    h.state_mut()
        .opened_mut()
        .unwrap()
        .set_compare(Some(id.clone()));
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.blink.of.as_deref() == Some(id.as_str()))
            && comparing(a)
    });
}

#[test]
fn b_flips_to_the_revision_and_back() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout_in_git("cmp-blink");
    let mut h = window(&root);
    frames(&mut h, |a| shown(a).is_some());
    agent(&root, "compare.set", json!({"rev": "HEAD"}));
    frames(&mut h, comparing);
    // #1106: the bar names the commit on every side.
    let short: String = h
        .state()
        .opened()
        .unwrap()
        .scene
        .as_ref()
        .unwrap()
        .against
        .as_ref()
        .unwrap()
        .commit
        .chars()
        .take(7)
        .collect();
    assert_eq!(short.chars().count(), 7);
    let bar = format!("Against HEAD ({short})");
    h.step();
    assert!(h.query_by_label(&bar).is_some(), "Both: {bar}");
    let canvas = h.get_by_label("Schematic sheet").rect().center();
    h.event(egui::Event::PointerMoved(canvas));
    h.key_press(egui::Key::B);
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.scene.as_ref())
            .is_some_and(|s| s.read_only() && s.owner("breakout/c3").is_some())
    });
    assert_eq!(h.state().opened().unwrap().blink.side, Side::Rev);
    h.step();
    assert!(
        h.query_by_label(&bar).is_some(),
        "the revision's side: {bar}"
    );
    h.key_press(egui::Key::B);
    frames(&mut h, |a| {
        a.opened().and_then(|w| w.scene.as_ref()).is_some_and(|s| {
            !s.read_only() && s.against.is_none() && s.owner("breakout/c3").is_none()
        })
    });
    h.step();
    assert!(h.query_by_label(&bar).is_some(), "the working side: {bar}");
}

#[test]
fn a_removed_components_change_frames_its_ghost() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout_in_git("cmp-ghost");
    let mut h = window(&root);
    frames(&mut h, |a| shown(a).is_some());
    agent(&root, "compare.set", json!({"rev": "HEAD"}));
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.changes.as_ref().is_some_and(|c| c.loaded()))
    });
    let row = {
        let c = h.state().opened().unwrap().changes.clone().unwrap();
        c.blocks
            .iter()
            .flat_map(|(_, r)| r)
            .find(|r| r.reference == "breakout/c3")
            .cloned()
            .expect("c3's row")
    };
    assert_eq!(row.verb, "removed");
    let show = ryueda_app::core::changes::Changes::show(&row);
    h.state_mut().opened_mut().unwrap().frame = None;
    h.state_mut().opened_mut().unwrap().show_change(&show);
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.frame.is_some() || w.camera.is_some())
            && comparing(a)
    });
    let w = h.state().opened().unwrap();
    let g = w
        .scene
        .as_ref()
        .unwrap()
        .ghosts
        .iter()
        .find(|g| g.reference == "breakout/c3")
        .unwrap();
    assert!(g.boxed.is_some());
}

#[test]
fn a_new_session_is_told_the_compare_again() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout_in_git("cmp-reconnect");
    let mut h = window(&root);
    frames(&mut h, |a| shown(a).is_some());
    h.state_mut()
        .opened_mut()
        .unwrap()
        .set_compare(Some("HEAD".into()));
    frames(&mut h, comparing);
    agent(&root, "daemon.shutdown", json!({}));
    frames(&mut h, |a| a.opened().is_some_and(|w| w.lost));
    frames(&mut h, |a| a.opened().is_some_and(|w| !w.lost));
    // The new session has the compare once it names it on the window's
    // own participant (the scene drawn before the loss still compares).
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| {
            w.people
                .get(w.me())
                .is_some_and(|p| p.view.compare.as_deref() == Some("HEAD"))
        })
    });
    frames(&mut h, comparing);
    let v = agent(&root, "compare.get", json!({}));
    assert_eq!(v["compare"]["rev"], "HEAD", "{v}");
}

#[test]
fn a_selected_change_lists_its_lines() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout_in_git("cmp-props");
    let mut h = window(&root);
    frames(&mut h, |a| shown(a).is_some());
    agent(&root, "compare.set", json!({"rev": "HEAD"}));
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.changes.as_ref().is_some_and(|c| c.loaded()))
    });
    h.state_mut().opened_mut().unwrap().go_to("breakout/r1");
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| w.selection == ["breakout/r1"])
    });
    h.step();
    // The breakout's r1 had no value at HEAD.
    assert!(h.query_by_label("Changes since HEAD").is_some());
    assert!(h.query_by_label("value: none → 10k").is_some());
}
