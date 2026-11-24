//! Design 938's bars for the window, on a hardware adapter (ignored:
//! `RYUPLM_BARS=1 cargo test -p ryuplm-app --release --test frames --
//! --ignored --nocapture`): a 100,000-item catalogue's first rows within
//! 2 s of opening on a warm daemon, scrolling at 60 frames a second, a
//! search's rows within 100 ms of the last key, and a pick seen by an
//! agent within 200 ms. Without RYUPLM_BARS, ten times looser.

mod common;

use common::{agent, daemon, one_at_a_time};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryuplm_app::ui::app::App;
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

fn strict() -> bool {
    std::env::var("RYUPLM_BARS").is_ok_and(|v| v == "1")
}

fn within(what: &str, took: Duration, limit: Duration) {
    let bar = if strict() { limit } else { limit * 10 };
    println!("{what}: {took:?} (bar {bar:?})");
    assert!(took <= bar, "{what} took {took:?}, over {bar:?}");
}

/// A hardware adapter, or a skip: lavapipe is no measure of frames.
fn hardware() -> bool {
    let Some((adapter, _, _)) = ryuplm_app::ui::smoke::headless() else {
        eprintln!("no adapter: skipped");
        return false;
    };
    let info = adapter.get_info();
    let soft = info.device_type == eframe::egui_wgpu::wgpu::DeviceType::Cpu;
    if soft {
        eprintln!("{}: a software adapter, skipped", info.name);
    }
    !soft
}

/// 100,000 seeded items, written once per run (as `ryuplm/tests/bar.rs`
/// writes them), and a daemon warmed by one list.
fn big() -> std::path::PathBuf {
    let root = common::write("big", ryuplm::testing::catalogue("bar", 100_000));
    agent(&root, "item.list", json!({"limit": 0}));
    root
}

fn window(root: &Path) -> Harness<'static, App> {
    let root = root.to_owned();
    Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, daemon()))
}

#[test]
#[ignore = "a bar: hardware adapters, a quiet machine"]
fn a_hundred_thousand_items_in_the_window() {
    let _one = one_at_a_time();
    if !hardware() {
        return;
    }
    let root = big();
    let t = Instant::now();
    let mut h = window(&root);
    while !h.state().opened().is_some_and(|w| w.rows.get(0).is_some()) {
        h.step();
    }
    h.step();
    within("the first rows drawn", t.elapsed(), Duration::from_secs(2));

    // Scrolling: 120 frames, three rows a frame.
    let at = h.get_by_label("Items").rect().center() + egui::vec2(0.0, 200.0);
    let mut times = Vec::new();
    for _ in 0..120 {
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -3.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        let t = Instant::now();
        h.step();
        times.push(t.elapsed());
    }
    times.sort();
    within(
        "a scrolled frame (median)",
        times[times.len() / 2],
        Duration::from_micros(16_667),
    );

    // A search: the rows of the last key.
    let w = h.state_mut().opened_mut().unwrap();
    let t = Instant::now();
    w.set_filter(ryuplm_app::core::rows::Filter {
        text: Some("item 4242".into()),
        ..Default::default()
    });
    // The search's own rows, never the previous filter's still held.
    while !h
        .state()
        .opened()
        .is_some_and(|w| w.rows.filter.text.as_deref() == Some("item 4242") && w.rows.current())
    {
        h.step();
    }
    h.step();
    within("a search's rows", t.elapsed(), Duration::from_millis(100));

    // A pick seen by an agent.
    let w = h.state_mut().opened_mut().unwrap();
    w.set_filter(Default::default());
    while !h.state().opened().is_some_and(|w| {
        w.rows.filter.text.is_none() && w.rows.current() && w.rows.get(0).is_some()
    }) {
        h.step();
    }
    // Drawn, so the row is there to click.
    h.step();
    h.step();
    let r = h.state().opened().unwrap().rows.get(0).unwrap().clone();
    let t = Instant::now();
    h.get_by_label(format!("{} {}", r.number, r.name.clone().unwrap_or_default()).trim())
        .click();
    loop {
        h.step();
        if agent(&root, "selection.get", json!({}))["selection"] == json!([r.reference()]) {
            break;
        }
    }
    within(
        "a pick seen by an agent",
        t.elapsed(),
        Duration::from_millis(200),
    );
}
