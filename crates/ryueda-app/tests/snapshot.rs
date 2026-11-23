//! The canvas as lavapipe draws it, in both themes, within a tolerance
//! (design 519, Testing approach), and every control named for
//! accesskit (ryucad 11.E's rule).

mod common;

use common::{agent, breakout, breakout_in_git, daemon, gpu, one_at_a_time};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryueda_app::ui::app::App;
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

const W: f32 = 1200.0;
const H: f32 = 800.0;
/// A channel further off than this makes a pixel differ.
const CHANNEL: u8 = 24;
/// At most this share of pixels may differ.
const SHARE: f64 = 0.01;

fn loaded(root: &Path, dark: bool) -> Harness<'static, App> {
    loaded_block(root, dark, "breakout")
}

fn loaded_block(root: &Path, dark: bool, block: &str) -> Harness<'static, App> {
    let root = root.to_owned();
    let mut h = Harness::builder()
        .with_size(egui::vec2(W, H))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, daemon()));
    h.ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    let want = if dark { "dark" } else { "light" };
    let end = Instant::now() + Duration::from_secs(30);
    while !h
        .state()
        .opened()
        .and_then(|w| w.scene.as_ref())
        .is_some_and(|s| s.theme == want && s.block == block)
        || !h.state().opened().is_some_and(|w| w.idle())
    {
        assert!(Instant::now() < end, "the sheet never drew");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    // Flashes off: a picture mustn't catch a fade.
    h.state_mut().opened_mut().unwrap().flash_on = false;
    h.state_mut().opened_mut().unwrap().flash.clear();
    for _ in 0..4 {
        h.step();
    }
    h
}

fn looks_as_it_did(img: &image::RgbaImage, name: &str) {
    if let Some(why) = differs(img, name) {
        panic!("{why}");
    }
}

/// Why `img` doesn't look as snapshot `name` did (its `.new.png` written
/// beside it), or `None` when it does, isn't compared here, or was saved.
fn differs(img: &image::RgbaImage, name: &str) -> Option<String> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/snapshots/{name}-lavapipe.png"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        img.save(&path).unwrap();
        return None;
    }
    let (adapter, _, _) = ryueda_app::ui::smoke::headless().unwrap();
    let info = adapter.get_info();
    if !format!("{} {}", info.name, info.driver).contains("llvmpipe") {
        let drawn = img
            .pixels()
            .filter(|p| p.0[..3] != img.get_pixel(0, 0).0[..3])
            .count();
        assert!(drawn > 20_000, "{} drew {drawn} pixels", info.name);
        eprintln!(
            "{}: drawn, not compared (the picture is lavapipe's)",
            info.name
        );
        return None;
    }
    let want = image::open(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .to_rgba8();
    assert_eq!(want.dimensions(), img.dimensions(), "{name}");
    let differ = img
        .pixels()
        .zip(want.pixels())
        .filter(|(a, b)| a.0.iter().zip(b.0).any(|(x, y)| x.abs_diff(y) > CHANNEL))
        .count();
    let limit = (f64::from(img.width() * img.height()) * SHARE) as usize;
    if differ <= limit {
        return None;
    }
    img.save(path.with_extension("new.png")).unwrap();
    Some(format!(
        "{differ} pixels differ (at most {limit}); see {name}-lavapipe.new.png"
    ))
}

#[test]
fn the_canvas_looks_as_it_did_in_both_themes() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("snapshot");
    for (dark, name) in [(false, "canvas-light"), (true, "canvas-dark")] {
        let mut h = loaded(&root, dark);
        let img = h.render().unwrap();
        looks_as_it_did(&img, name);
    }
}

#[test]
fn every_control_is_named() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("names");
    let h = loaded(&root, false);
    for label in [
        "Schematic sheet",
        "Navigator",
        "Sheet",
        "Properties",
        "Findings",
        "Instructions",
        "Activity",
        "Edit",
        "View",
        "Tools",
        "Run check",
        "Breakout (breakout)",
        "Changes",
        "History",
    ] {
        assert!(h.query_by_label(label).is_some(), "nothing named {label}");
    }
    let unnamed = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::Button
                && n.label().is_none_or(|l| l.trim().is_empty())
        })
        .count();
    assert_eq!(unnamed, 0, "unnamed buttons");
}

/// The compare bar's controls are named too (plan 13.A).
#[test]
fn every_compare_control_is_named() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout_in_git("names-compare");
    let mut h = loaded(&root, false);
    agent(&root, "compare.set", json!({"rev": "HEAD"}));
    let end = Instant::now() + Duration::from_secs(30);
    while !h
        .state()
        .opened()
        .and_then(|w| w.scene.as_ref())
        .is_some_and(|s| s.against.is_some())
    {
        assert!(Instant::now() < end, "the compare never drew");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    h.step();
    for label in ["Both", "Working", "HEAD", "Stop"] {
        assert!(h.query_by_label(label).is_some(), "nothing named {label}");
    }
    let unnamed = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::Button
                && n.label().is_none_or(|l| l.trim().is_empty())
        })
        .count();
    assert_eq!(unnamed, 0, "unnamed buttons");
}

#[test]
fn a_compare_looks_as_it_did_in_both_themes() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout_in_git("snapshot-compare");
    // Both themes compared before either fails, so a change draws both
    // anew (`.new.png`, uploaded by CI) in one run.
    let mut failed = Vec::new();
    for (dark, name) in [(false, "compare-light"), (true, "compare-dark")] {
        let mut h = loaded(&root, dark);
        agent(&root, "compare.set", json!({"rev": "HEAD"}));
        let end = Instant::now() + Duration::from_secs(30);
        while !h
            .state()
            .opened()
            .is_some_and(|w| w.idle() && w.scene.as_ref().is_some_and(|s| s.against.is_some()))
        {
            assert!(Instant::now() < end, "the compare never drew");
            h.step();
            std::thread::sleep(Duration::from_millis(20));
        }
        h.state_mut().opened_mut().unwrap().flash.clear();
        for _ in 0..4 {
            h.step();
        }
        let img = h.render().unwrap();
        failed.extend(differs(&img, name));
        agent(&root, "compare.set", json!({"off": true}));
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
fn a_halo_reads_on_white_paper() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = breakout("snapshot-halo");
    let mut h = loaded(&root, false);
    let w = h.state_mut().opened_mut().unwrap();
    w.flash
        .start("breakout/r1", Instant::now() + Duration::from_secs(3600));
    for _ in 0..4 {
        h.step();
    }
    let img = h.render().unwrap();
    looks_as_it_did(&img, "halo-light");
}

/// The breakout's merge in the window: the Conflicts tile open, light.
#[test]
fn the_conflict_panel_looks_as_it_did_and_is_named() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let (_top, board) = common::merging("snapshot-conflicts", common::OURS, common::THEIRS);
    let root = board.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(W, H))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, daemon()));
    h.ctx.set_theme(egui::Theme::Light);
    let end = Instant::now() + Duration::from_secs(60);
    while !h.state().opened().is_some_and(|w| {
        w.idle()
            && w.merge
                .as_ref()
                .is_some_and(|m| m.conflicts().iter().any(|c| c["kind"] == "refdes_clash"))
    }) {
        assert!(Instant::now() < end, "the merge never showed");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    h.state_mut().opened_mut().unwrap().flash_on = false;
    h.state_mut().opened_mut().unwrap().flash.clear();
    for _ in 0..4 {
        h.step();
    }
    let img = h.render().unwrap();
    looks_as_it_did(&img, "conflicts-light");
    for label in ["Conflicts", "Renumber (recommended)"] {
        assert!(h.query_by_label(label).is_some(), "nothing named {label}");
    }
    for label in ["Show", "Show ours", "Show theirs", "Copy"] {
        assert!(
            h.query_all_by_label(label).next().is_some(),
            "nothing named {label}"
        );
    }
    let unnamed = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::Button
                && n.label().is_none_or(|l| l.trim().is_empty())
        })
        .count();
    assert_eq!(unnamed, 0, "unnamed buttons");
}

/// Design 688's visual review: the breakout's and the RP2040's compare
/// against HEAD and the breakout's merge, light and dark, as one contact
/// sheet (`review-13b.png` in `RYUEDA_REVIEW_DIR`), for `cox visual`.
#[test]
#[ignore = "RYUEDA_REVIEW_DIR=<dir> cargo test -p ryueda-app --test snapshot review_pictures -- --ignored --test-threads=1"]
fn review_pictures() {
    let _one = one_at_a_time();
    assert!(gpu(), "the review needs an adapter");
    let dir =
        std::path::PathBuf::from(std::env::var("RYUEDA_REVIEW_DIR").expect("RYUEDA_REVIEW_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let wait = |h: &mut Harness<'static, App>, done: &dyn Fn(&App) -> bool| {
        let end = Instant::now() + Duration::from_secs(120);
        while !done(h.state()) {
            assert!(Instant::now() < end, "never drew");
            h.step();
            std::thread::sleep(Duration::from_millis(20));
        }
        h.state_mut().opened_mut().unwrap().flash.clear();
        for _ in 0..4 {
            h.step();
        }
    };
    let mut shots = Vec::new();
    for (root, block) in [
        (breakout_in_git("review-breakout"), "breakout"),
        (common::rp2040_in_git("review-rp2040"), "mitayi"),
    ] {
        for dark in [false, true] {
            let mut h = loaded_block(&root, dark, block);
            agent(&root, "compare.set", json!({"rev": "HEAD"}));
            wait(&mut h, &|a: &App| {
                a.opened().is_some_and(|w| {
                    w.idle() && w.scene.as_ref().is_some_and(|s| s.against.is_some())
                })
            });
            shots.push(h.render().unwrap());
            agent(&root, "compare.set", json!({"off": true}));
        }
    }
    let (_top, board) = common::merging("review-merge", common::OURS, common::THEIRS);
    for dark in [false, true] {
        let root = board.clone();
        let mut h = Harness::builder()
            .with_size(egui::vec2(W, H))
            .wgpu()
            .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, daemon()));
        h.ctx.set_theme(if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        wait(&mut h, &|a: &App| {
            a.opened().is_some_and(|w| {
                w.idle()
                    && w.merge
                        .as_ref()
                        .is_some_and(|m| m.conflicts().iter().any(|c| c["kind"] == "refdes_clash"))
            })
        });
        shots.push(h.render().unwrap());
    }
    // Light on the left, dark on the right; a row each.
    let (w, h) = shots[0].dimensions();
    let mut sheet = image::RgbaImage::new(w * 2, h * 3);
    for (i, img) in shots.iter().enumerate() {
        let (x, y) = (w * (i as u32 % 2), h * (i as u32 / 2));
        image::imageops::overlay(&mut sheet, img, i64::from(x), i64::from(y));
    }
    sheet.save(dir.join("review-13b.png")).unwrap();
}
