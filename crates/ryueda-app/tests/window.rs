//! The window against real daemons (design 519, Testing approach): a
//! sheet opened from the navigator, a pick read by the agent, an agent's
//! show followed, a finding's Show, and Undo, over the sensor breakout.

mod common;

use common::{agent, breakout, daemon, gpu, one_at_a_time};
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

/// The screen point of an owner's box's middle on the canvas.
fn on_sheet(h: &Harness<'static, App>, reference: &str) -> egui::Pos2 {
    let rect = h.get_by_label("Schematic sheet").rect();
    let w = h.state().opened().unwrap();
    let sc = w.scene.as_ref().unwrap();
    let b = sc.owners[sc.owner(reference).unwrap()].boxed.unwrap();
    let cam = w.camera.unwrap();
    let px = [f64::from(rect.width()), f64::from(rect.height())];
    let q = cam.to_screen([(b[0][0] + b[1][0]) / 2.0, (b[0][1] + b[1][1]) / 2.0], px);
    rect.left_top() + egui::vec2(q[0] as f32, q[1] as f32)
}

fn click_at(h: &mut Harness<'static, App>, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    h.step();
}

#[test]
fn a_sheet_opens_from_the_navigator() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("navigator");
    let mut h = window(&root);
    frames(&mut h, |a| shown(a).is_some_and(|(b, _)| b == "breakout"));
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.nav.wanted_blocks().is_empty() && w.nav.design_ids().count() == 1)
    });
    h.step();
    h.get_by_label("s1 (sensor)").click();
    frames(&mut h, |a| shown(a).is_some_and(|(b, _)| b == "sensor"));
    assert_eq!(
        h.state().opened().unwrap().shown.layer.as_deref(),
        Some("s1")
    );
}

#[test]
fn a_pick_is_read_by_the_agent() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("pick");
    let mut h = window(&root);
    frames(&mut h, |a| {
        shown(a).is_some_and(|(b, _)| b == "breakout")
            && a.opened().is_some_and(|w| w.camera.is_some())
    });
    // The resistor's body is on the root block's sheet.
    let at = on_sheet(&h, "breakout/r1");
    click_at(&mut h, at);
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| w.selection == ["breakout/r1"])
    });
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        h.step();
        let got = agent(&root, "selection.get", json!({}));
        if got["selection"] == json!(["breakout/r1"]) {
            break;
        }
        assert!(Instant::now() < end, "the agent never saw the pick: {got}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Properties read the component.
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.props.as_ref())
            .is_some_and(|p| p.rows.iter().any(|(k, _)| k == "Designator"))
    });
}

#[test]
fn an_agent_shows_the_person_what_it_means() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("show");
    let mut h = window(&root);
    frames(&mut h, |a| shown(a).is_some());
    agent(&root, "view.show", json!({"ref": "sensor/c1"}));
    frames(&mut h, |a| {
        shown(a).is_some_and(|(b, _)| b == "sensor")
            && a.opened().is_some_and(|w| w.selection == ["sensor/c1"])
    });
    // ui.follow_shows off: the next show is left.
    h.state_mut().opened_mut().unwrap().follow_shows = false;
    agent(&root, "view.show", json!({"ref": "breakout/r1"}));
    for _ in 0..25 {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(shown(h.state()).unwrap().0, "sensor");
}

#[test]
fn a_finding_s_show_frames_its_part() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("finding");
    // A resistor on no net: ERC finds its pins unconnected.
    agent(
        &root,
        "component.add",
        json!({"block": "breakout", "of": "r_4k7", "id": "r9"}),
    );
    let mut h = window(&root);
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| {
            w.findings.all().iter().any(|f| {
                f.refs
                    .iter()
                    .any(|r| w.nav.local(r).starts_with("breakout/r9"))
            })
        })
    });
    h.step();
    let w = h.state().opened().unwrap();
    let i = w
        .findings
        .all()
        .iter()
        .position(|f| {
            f.refs
                .iter()
                .any(|r| w.nav.local(r).starts_with("breakout/r9"))
        })
        .unwrap();
    // The bottom tile scrolls: bring the row's button into view first.
    h.get_all_by_label("Show").nth(i).unwrap().scroll_to_me();
    for _ in 0..4 {
        h.step();
    }
    h.get_all_by_label("Show").nth(i).unwrap().click();
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.selection.iter().any(|r| r.starts_with("breakout/r9")))
    });
}

#[test]
fn undo_goes_through_the_daemon() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("undo");
    let mut h = window(&root);
    frames(&mut h, |a| shown(a).is_some());
    let before = h
        .state()
        .opened()
        .unwrap()
        .scene
        .as_ref()
        .unwrap()
        .revision
        .clone();
    agent(
        &root,
        "component.edit",
        json!({"component": "breakout/r1", "value": "47k"}),
    );
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.scene.as_ref())
            .is_some_and(|s| s.revision != before)
    });
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.activity.first().is_some_and(|e| e.undo))
    });
    h.get_by_label("Undo").click();
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.scene.as_ref())
            .is_some_and(|s| s.revision == before)
    });
}
