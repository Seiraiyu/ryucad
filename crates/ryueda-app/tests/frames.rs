//! Design 519's window bars, release only (ignored otherwise; strict with
//! RYUEDA_BARS=1, ten times looser without): a 500-part sheet at 60
//! frames a second on a hardware adapter, a pick seen by another window
//! and the agent within 200 ms, and the app's size and start.

mod common;

#[path = "../../ryueda/tests/support/generated.rs"]
mod generated;

use common::{agent, daemon, gpu};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryueda_app::core::merge::{Batch, rows};
use ryueda_app::ui::app::App;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn strict() -> bool {
    std::env::var_os("RYUEDA_BARS").is_some()
}

fn bar(d: Duration) -> Duration {
    if strict() { d } else { d * 10 }
}

/// One channel of 500 resistors, laid out: block `ch`, in a folder of the
/// caller's own (#1097: the two bars ran in parallel on one).
fn five_hundred(name: &str) -> PathBuf {
    let root = common::shared().join(name);
    let _ = std::fs::remove_dir_all(&root);
    generated::write(&root, 1, 500);
    let root = std::fs::canonicalize(root).unwrap();
    agent(&root, "layout", json!({"block": "ch"}));
    root
}

fn window(root: &Path, size: egui::Vec2) -> Harness<'static, App> {
    let root = root.to_owned();
    Harness::builder()
        .with_size(size)
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, daemon()))
}

fn loaded(h: &mut Harness<'static, App>, block: &str, within: Duration) -> Duration {
    let start = Instant::now();
    while !h
        .state()
        .opened()
        .and_then(|w| w.scene.as_ref())
        .is_some_and(|s| s.block == block)
    {
        assert!(start.elapsed() < within, "no sheet after {within:?}");
        h.step();
    }
    start.elapsed()
}

#[test]
#[ignore = "release, hardware adapters: cargo test -p ryueda-app --release --test frames -- --ignored --nocapture"]
fn a_five_hundred_part_sheet_pans_at_60_frames_a_second() {
    let _one = common::one_at_a_time();
    let Some((adapter, _, _)) = ryueda_app::ui::smoke::headless() else {
        assert!(!strict(), "no adapter");
        return;
    };
    let info = adapter.get_info();
    if strict() {
        assert_ne!(
            info.device_type,
            eframe::wgpu::DeviceType::Cpu,
            "the bar is for hardware: {} is software",
            info.name
        );
    }
    let root = five_hundred("five-hundred-plain");
    let mut h = window(&root, egui::vec2(1280.0, 800.0));
    h.state_mut()
        .opened_mut()
        .unwrap()
        .show(Some("ch".into()), None, None);
    loaded(&mut h, "ch", Duration::from_secs(120));
    let rect = h.get_by_label("Schematic sheet").rect();
    let mut times = Vec::new();
    for f in 0..240 {
        let t = f as f32 / 240.0;
        let at = rect.center() + egui::vec2((t * 6.0).sin() * 200.0, (t * 4.0).cos() * 120.0);
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, if f % 60 < 30 { 20.0 } else { -20.0 }),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        let begin = Instant::now();
        h.step();
        let _ = h.render();
        times.push(begin.elapsed().as_secs_f64() * 1e3);
    }
    let mut steady = times[10..].to_vec();
    steady.sort_by(f64::total_cmp);
    let median = steady[steady.len() / 2];
    let worst = steady[steady.len() - 1];
    println!(
        "{} ({:?}): 500 parts, median {median:.1} ms ({:.0} fps), worst {worst:.1} ms",
        info.name,
        info.backend,
        1000.0 / median
    );
    if strict() {
        assert!(
            1000.0 / median >= 60.0,
            "{:.0} frames a second",
            1000.0 / median
        );
    }
}

#[test]
#[ignore = "release: cargo test -p ryueda-app --release --test frames -- --ignored --nocapture"]
fn a_pick_is_seen_by_another_window_and_the_agent_within_200_ms() {
    let _one = common::one_at_a_time();
    if !gpu() {
        return;
    }
    let root = common::breakout("round-trip");
    let mut a = window(&root, egui::vec2(1200.0, 800.0));
    let mut b = window(&root, egui::vec2(1200.0, 800.0));
    loaded(&mut a, "breakout", Duration::from_secs(30));
    loaded(&mut b, "breakout", Duration::from_secs(30));
    let me = a.state().opened().unwrap().me().to_owned();
    let start = Instant::now();
    a.state_mut()
        .opened_mut()
        .unwrap()
        .pick(Some("breakout/r1".into()), false);
    let (mut seen_b, mut seen_agent) = (None, None);
    while seen_b.is_none() || seen_agent.is_none() {
        assert!(start.elapsed() < Duration::from_secs(5), "never seen");
        a.step();
        b.step();
        if seen_b.is_none()
            && b.state()
                .opened()
                .unwrap()
                .people
                .get(&me)
                .is_some_and(|p| p.selection == ["breakout/r1"])
        {
            seen_b = Some(start.elapsed());
        }
        if seen_agent.is_none()
            && agent(&root, "selection.get", json!({"participant": me}))["selection"]
                == json!(["breakout/r1"])
        {
            seen_agent = Some(start.elapsed());
        }
    }
    let (sb, sa) = (seen_b.unwrap(), seen_agent.unwrap());
    println!("a pick seen by another window in {sb:?}, by the agent in {sa:?}");
    assert!(sb <= bar(Duration::from_millis(200)) && sa <= bar(Duration::from_millis(200)));
}

#[test]
#[ignore = "release: cargo test -p ryueda-app --release --test frames -- --ignored --nocapture"]
fn the_app_is_no_larger_than_ryucad_s_and_draws_within_two_seconds() {
    let _one = common::one_at_a_time();
    if !gpu() {
        return;
    }
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_ryueda-app"));
    let mine = exe.metadata().unwrap().len();
    let theirs = exe.with_file_name(format!("ryucad-app{}", std::env::consts::EXE_SUFFIX));
    match theirs.metadata() {
        Ok(m) => {
            println!("ryueda-app {mine} bytes, ryucad-app {} bytes", m.len());
            assert!(mine <= m.len(), "ryueda-app is larger than ryucad-app");
        }
        Err(_) => println!("ryueda-app {mine} bytes (build ryucad-app in this profile to compare)"),
    }
    let root = common::breakout("start");
    let start = Instant::now();
    let mut h = window(&root, egui::vec2(1200.0, 800.0));
    loaded(&mut h, "breakout", Duration::from_secs(30));
    let took = start.elapsed();
    println!("a drawn sheet {took:?} after start");
    assert!(took <= bar(Duration::from_secs(2)), "{took:?}");
}

/// Design 688's bar: the 500-part sheet with every owner marked and 100
/// ghosts at 60 frames a second, on a hardware adapter. The marks are put
/// on the scene the window holds: the bar is the window's drawing, not
/// the daemon's compare (its own bar is in ryueda's `bar.rs`).
#[test]
#[ignore = "release, hardware adapters: cargo test -p ryueda-app --release --test frames -- --ignored --nocapture"]
fn a_marked_five_hundred_part_sheet_pans_at_60_frames_a_second() {
    use ryueda_app::core::scene::{Diff, DiffColours, Ghost, Item};
    let _one = common::one_at_a_time();
    let Some((adapter, _, _)) = ryueda_app::ui::smoke::headless() else {
        assert!(!strict(), "no adapter");
        return;
    };
    let info = adapter.get_info();
    if strict() {
        assert_ne!(
            info.device_type,
            eframe::wgpu::DeviceType::Cpu,
            "the bar is for hardware: {} is software",
            info.name
        );
    }
    let root = five_hundred("five-hundred-marked");
    let mut h = window(&root, egui::vec2(1280.0, 800.0));
    h.state_mut()
        .opened_mut()
        .unwrap()
        .show(Some("ch".into()), None, None);
    loaded(&mut h, "ch", Duration::from_secs(120));
    {
        let sc = h.state_mut().opened_mut().unwrap().scene.as_mut().unwrap();
        sc.paper.diff = Some(DiffColours {
            added: egui::Color32::from_rgb(0x1a, 0x7f, 0x37),
            changed: egui::Color32::from_rgb(0x9a, 0x67, 0x00),
            removed: egui::Color32::from_rgb(0xcf, 0x22, 0x2e),
        });
        for o in &mut sc.owners {
            o.mark = Some(Diff::Changed);
        }
        let parts: Vec<Item> = sc
            .items
            .iter()
            .filter(|it| it.owner.is_some_and(|o| sc.owners[o].kind == "component"))
            .take(100)
            .cloned()
            .collect();
        for (i, mut it) in parts.into_iter().enumerate() {
            it.owner = None;
            it.ghost = Some(i);
            sc.ghosts.push(Ghost {
                reference: format!("ch/ghost{i}"),
                kind: "component".into(),
                boxed: None,
            });
            sc.items.insert(i, it);
        }
    }
    let rect = h.get_by_label("Schematic sheet").rect();
    let mut times = Vec::new();
    for f in 0..240 {
        let t = f as f32 / 240.0;
        let at = rect.center() + egui::vec2((t * 6.0).sin() * 200.0, (t * 4.0).cos() * 120.0);
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, if f % 60 < 30 { 20.0 } else { -20.0 }),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        let begin = Instant::now();
        h.step();
        let _ = h.render();
        times.push(begin.elapsed().as_secs_f64() * 1e3);
    }
    let mut steady = times[10..].to_vec();
    steady.sort_by(f64::total_cmp);
    let median = steady[steady.len() / 2];
    println!(
        "{} ({:?}): 500 parts marked, 100 ghosts, median {median:.1} ms ({:.0} fps), worst {:.1} ms",
        info.name,
        info.backend,
        1000.0 / median,
        steady[steady.len() - 1]
    );
    if strict() {
        assert!(
            1000.0 / median >= 60.0,
            "{:.0} frames a second",
            1000.0 / median
        );
    }
}

/// What the machine is doing, for the bars' lines.
fn load() -> String {
    std::fs::read_to_string("/proc/loadavg").unwrap_or_else(|_| "unknown".into())
}

#[test]
#[ignore = "release: cargo test -p ryueda-app --release --test frames -- --ignored --nocapture"]
fn the_conflict_panel_draws_within_2_s_and_a_resolve_within_1_s() {
    let _one = common::one_at_a_time();
    if !gpu() {
        return;
    }
    let (_top, board) = common::merging("bar-conflicts", common::OURS, common::THEIRS);
    // Warm: the daemon has worked the merge out once; the bar is the window's.
    agent(&board, "merge.status", json!({}));
    let clash = |a: &App| {
        a.opened().and_then(|w| w.merge.as_ref()).is_some_and(|m| {
            !m.resolving && m.conflicts().iter().any(|c| c["kind"] == "refdes_clash")
        })
    };
    let start = Instant::now();
    let mut h = window(&board, egui::vec2(1400.0, 900.0));
    while !clash(h.state()) {
        assert!(
            start.elapsed() < bar(Duration::from_secs(2)) * 5,
            "no status"
        );
        h.step();
    }
    h.step();
    let drawn = start.elapsed();
    assert!(h.query_by_label("Conflicts").is_some());
    let row = {
        let m = h.state().opened().unwrap().merge.as_ref().unwrap();
        rows(&m.status)
            .into_iter()
            .find(|r| r.kind == "refdes_clash")
            .unwrap()
    };
    let t = Instant::now();
    h.state_mut()
        .opened_mut()
        .unwrap()
        .resolve(Batch::one(&row, &row.actions[0]));
    let busy = |a: &App| {
        a.opened()
            .and_then(|w| w.merge.as_ref())
            .is_some_and(|m| m.resolving || m.conflict(&row.id).is_some())
    };
    while busy(h.state()) {
        assert!(
            t.elapsed() < bar(Duration::from_secs(1)) * 5,
            "no new status"
        );
        h.step();
    }
    let resolved = t.elapsed();
    eprintln!(
        "the breakout's merge: the panel drawn {drawn:?} after the window opened, a resolve's status in {resolved:?} (load {})",
        load().trim()
    );
    assert!(drawn <= bar(Duration::from_secs(2)), "{drawn:?}");
    assert!(resolved <= bar(Duration::from_secs(1)), "{resolved:?}");
}
