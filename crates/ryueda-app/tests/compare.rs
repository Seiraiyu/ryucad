//! The compare view against real daemons (design 688): an agent's
//! compare followed, History's Compare, B, a removed component's Show,
//! a reconnect, over the breakout committed and edited.

mod common;

use common::{agent, breakout_in_git, daemon, gpu, one_at_a_time};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
