//! Assemblies in the window against a real daemon: an assembly's
//! occurrences drawn where its instances put them, the project's one root
//! assembly shown by default, and a focus an agent sets followed.

mod common;

use common::{Project, mated, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

/// A bolt and an assembly placing it twice.
fn assembled() -> Project {
    let p = Project::new();
    p.ok(&["part", "create", "bolt"]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=3",
        "--param",
        "height=20",
    ]);
    p.ok(&["part", "create", "frame", "--assembly"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b1", "--of", "bolt",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b2", "--of", "bolt", "--origin", "40,0,0",
    ]);
    p
}

fn key(part: &str, f: &str) -> (String, String) {
    (part.to_owned(), f.to_owned())
}

#[test]
fn the_window_draws_an_assembly_where_it_places_its_parts() {
    let p = assembled();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.model.meshes.contains_key(&key("frame", "b1/shank"))
            && ws.model.meshes.contains_key(&key("frame", "b2/shank"))
    }));
    let frame = ws.part("frame").unwrap();
    assert!(frame.assembly);
    assert_eq!(frame.features[1].instance.as_ref().unwrap().part, "bolt");
    // One mesh for both bolts, fetched once; the second drawn 40 mm along
    // x from the first.
    assert!(Arc::ptr_eq(
        &ws.model.meshes[&key("frame", "b1/shank")],
        &ws.model.meshes[&key("frame", "b2/shank")]
    ));
    let at = |k: &str| ws.model.pose(&key("frame", k)).origin;
    let (a, b) = (at("b1/shank"), at("b2/shank"));
    assert!((b[0] - a[0] - 0.04).abs() < 1e-9, "{a:?} {b:?}");
    let fetched = ws.fetched;
    // The frame is the one root: it is what the window draws.
    assert_eq!(ws.model.shown(), ["frame"]);
    let drawn: Vec<_> = ws.model.drawn().map(|(k, ..)| k.clone()).collect();
    assert_eq!(drawn, [key("frame", "b1/shank"), key("frame", "b2/shank")]);
    // An agent shows the human the bolt alone.
    p.ok(&["focus", "bolt"]);
    assert!(ws.wait_until(WAIT, |ws| ws.model.focus.as_deref() == Some("bolt")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model.meshes.contains_key(&key("bolt", "shank"))
    }));
    let drawn: Vec<_> = ws.model.drawn().map(|(k, ..)| k.clone()).collect();
    assert_eq!(drawn, [key("bolt", "shank")]);
    assert_eq!(
        ws.fetched, fetched,
        "the bolt's own mesh is the same body's"
    );
    // A move fetches nothing: the bolt is drawn where it is now.
    ws.set_focus(None);
    p.ok(&["assembly", "move", "frame", "b2", "--translate", "0,10,0"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.model.focus.is_none()
            && (ws.model.pose(&key("frame", "b2/shank")).origin[1] - 0.01).abs() < 1e-9
    }));
    assert_eq!(ws.fetched, fetched);
}

#[test]
fn the_form_edits_an_instance() {
    let p = assembled();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("frame").is_some()));
    ws.open_form(Some(key("frame", "b2")));
    assert!(ws.wait_until(WAIT, |ws| ws.forms.form.is_some()));
    let f = ws.forms.form.as_mut().unwrap();
    assert_eq!(f.ty, "instance");
    assert_eq!(f.origin[0].read, "40 mm");
    assert_eq!(f.rev.as_ref().unwrap().read, "working");
    f.origin[1].text = "15 mm".into();
    f.fixed.as_mut().unwrap().chosen = true;
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.forms
            .form
            .as_ref()
            .is_some_and(|f| !f.saving && !f.dirty())
    }));
    let shown = p.ok(&["feature", "show", "frame", "b2"]);
    assert_eq!(
        shown["pose"]["origin_m"],
        serde_json::json!([0.04, 0.015, 0])
    );
    assert_eq!(shown["fixed"], true);
    // The tree says so.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.part("frame")
            .and_then(|p| p.feature("b2"))
            .and_then(|f| f.instance.as_ref())
            .is_some_and(|i| i.fixed)
    }));
}

#[test]
fn the_tree_says_what_mates_leave_and_a_mate_paints_its_faces() {
    let p = mated();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws
                .part("frame")
                .is_some_and(|f| f.feature("ring").is_some())
    }));
    let frame = ws.part("frame").unwrap();
    let solve = frame.solve.as_ref().unwrap();
    assert!(solve.converged && !solve.stale, "{solve:?}");
    let pin = frame.feature("pin").unwrap().instance.as_ref().unwrap();
    assert_eq!(pin.solve, Some(("under".to_owned(), 1)));
    let m = frame.feature("m1").unwrap().mate.as_ref().unwrap();
    assert_eq!(
        (m.form.as_str(), m.state.as_deref()),
        ("revolute", Some("well"))
    );
    assert_eq!(
        frame.feature("ring").unwrap().copies,
        ["ring:1/pin", "ring:2/pin", "ring:3/pin"]
    );
    // The copies are drawn, under the pattern.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model
            .meshes
            .contains_key(&key("frame", "ring:2/pin/shank"))
    }));
    // Chosen, the mate paints the faces its connectors are on.
    ws.choose(Some(key("frame", "m1")));
    let made = ws.made.as_ref().expect("the mate's faces");
    let bodies: Vec<&String> = made.faces.keys().map(|(_, b)| b).collect();
    assert_eq!(bodies, ["base/slab", "pin/shank"]);
}

#[test]
fn the_window_finds_which_instances_interfere_and_goes_stale_on_edits() {
    let p = assembled();
    // A third bolt 4 mm from the first: their shanks, 3 mm in radius,
    // overlap.
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b3", "--of", "bolt", "--origin", "4,0,0",
    ]);
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.model.meshes.contains_key(&key("frame", "b3/shank"))));
    assert_eq!(ws.shown_assembly().as_deref(), Some("frame"));
    ws.check_interference();
    let loaded =
        |ws: &Workspace| ws.idle() && ws.assembly.interference.as_ref().is_some_and(|i| i.loaded);
    assert!(ws.wait_until(WAIT, loaded));
    let i = ws.assembly.interference.as_ref().unwrap();
    assert_eq!(i.pairs.len(), 1, "{i:?}");
    assert_eq!(
        (i.pairs[0].a.as_str(), i.pairs[0].b.as_str()),
        ("frame/b1/shank", "frame/b3/shank")
    );
    assert!(i.faces().all(|f| f.starts_with("frame/b")));
    // Moved clear, the list goes stale and paints nothing. It isn't asked
    // again on its own (design 525): the old answer stays listed.
    p.ok(&["assembly", "move", "frame", "b3", "--translate", "20mm,0,0"]);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.assembly.interference.as_ref().is_some_and(|i| i.stale)));
    let i = ws.assembly.interference.as_ref().unwrap();
    assert!(i.loaded, "{i:?}");
    assert_eq!(i.pairs.len(), 1, "{i:?}");
    assert_eq!(i.faces().count(), 0, "{i:?}");
    // Check again asks anew, and finds it clear.
    ws.check_interference();
    assert!(ws.wait_until(WAIT, |ws| {
        loaded(ws)
            && ws
                .assembly
                .interference
                .as_ref()
                .is_some_and(|i| !i.stale && i.pairs.is_empty())
    }));
}

#[test]
fn the_window_hides_as_its_own_request_answers() {
    let p = assembled();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("frame").is_some()));
    // #143: with the notification lost, the answer alone hides it.
    ws.deaf = true;
    ws.set_visibility(vec!["frame/b2".into()], Vec::new(), Vec::new(), false);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    let hidden: Vec<String> = ws.model.hidden.iter().cloned().collect();
    assert_eq!(hidden, ["frame/b2"]);
}

/// [`common::two_bolts`] (b1 fixed, b2 40 mm along x) with motions `lift`
/// (b2 up 30 mm in 4 steps) and `slam` (b2 40 mm along -x, into b1, in 11
/// steps).
fn with_motions() -> Project {
    let p = common::two_bolts();
    p.ok(&[
        "motion",
        "save",
        "frame",
        "lift",
        "--steps",
        "4",
        "--drives",
        r#"[{"instances": ["b2"], "translate": {"direction": "z", "from": "0 mm", "to": "30 mm"}}]"#,
    ]);
    p.ok(&[
        "motion",
        "save",
        "frame",
        "slam",
        "--steps",
        "11",
        "--drives",
        r#"[{"instances": ["b2"], "translate": {"direction": [-1, 0, 0], "from": "0 mm", "to": "40 mm"}}]"#,
    ]);
    p
}

fn shown_of(p: &Project) -> serde_json::Value {
    p.ok(&["participants"])["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["kind"] == "human")
        .map(|x| x["motion"].clone())
        .unwrap()
}

#[test]
fn the_window_shows_plays_and_follows_a_participants_motion() {
    let p = with_motions();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    let b2 = key("frame", "b2/shank");
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.contains_key(&b2)));
    let z = |ws: &Workspace| ws.model.pose(&b2).origin[2];
    let rest = z(&ws);
    // An agent shows it at its end: b2 drawn 30 mm up, once its frames come.
    p.ok(&[
        "motion",
        "show",
        "lift",
        "--part",
        "frame",
        "--fraction",
        "1",
    ]);
    assert!(ws.wait_until(WAIT, |ws| (z(ws) - rest - 0.03).abs() < 1e-9));
    assert_eq!(ws.assembly.motions.list.len(), 2);
    assert_eq!(
        ws.assembly.motions.frames.as_ref().unwrap().frames.len(),
        120
    );
    // The slider moves it at once, to its nearest frame, and is written.
    ws.slide_motion(0.5);
    let want = 0.03 * 60.0 / 119.0;
    assert!((z(&ws) - rest - want).abs() < 1e-9, "{}", z(&ws) - rest);
    ws.show_motion(None, Some(0.5), Some(false), false);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws
                .myself()
                .and_then(|m| m.motion.clone())
                .is_some_and(|m| m.fraction == 0.5)
    }));
    // Play moves it on its own, and says so to the session.
    ws.play_motion(true);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    assert_eq!(shown_of(&p)["playing"], true);
    let before = z(&ws);
    std::thread::sleep(Duration::from_millis(400));
    ws.tick(std::time::Instant::now());
    assert_ne!(z(&ws), before, "Play moved it");
    ws.play_motion(false);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && !ws.assembly.motions.playing()));
    assert_eq!(shown_of(&p)["playing"], false);
    // An agent's fraction is followed.
    p.ok(&["motion", "show", "--fraction", "0"]);
    assert!(ws.wait_until(WAIT, |ws| (z(ws) - rest).abs() < 1e-12));
    // Nothing is drawn moved once it is off.
    p.ok(&[
        "motion",
        "show",
        "lift",
        "--part",
        "frame",
        "--fraction",
        "1",
    ]);
    assert!(ws.wait_until(WAIT, |ws| z(ws) > rest + 0.029));
    p.ok(&["motion", "show", "--off"]);
    assert!(ws.wait_until(WAIT, |ws| z(ws) == rest && ws.model.exploded.is_none()));
    // An explode shown takes the motion away, and the reverse.
    p.ok(&[
        "explode",
        "save",
        "frame",
        "apart",
        "--steps",
        r#"[{"instances": ["b2"], "translate": [0, 0, 10]}]"#,
    ]);
    p.ok(&[
        "motion",
        "show",
        "lift",
        "--part",
        "frame",
        "--fraction",
        "1",
    ]);
    assert!(ws.wait_until(WAIT, |ws| z(ws) > rest + 0.029));
    p.ok(&["explode", "show", "apart"]);
    assert!(ws.wait_until(WAIT, |ws| (z(ws) - rest - 0.01).abs() < 1e-9));
    assert!(ws.assembly.motions.shown.is_none());
    // Edited from the window: the motion's range written.
    p.ok(&["motion", "show", "lift", "--part", "frame"]);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.assembly.motions.current("frame").is_some()));
    ws.assembly.motions.editing = Some((
        "lift".into(),
        vec![ryucad_app::core::motion::Range {
            drive: 0,
            key: "translate",
            from: "0 mm".into(),
            to: "50 mm".into(),
        }],
        "6".into(),
    ));
    ws.save_motion();
    let file = p.root.join("parts/frame/motions/lift.json");
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && std::fs::read_to_string(&file).unwrap().contains("50 mm")));
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["steps"], 6, "{written}");
}

#[test]
fn a_sweep_from_the_window_marks_and_lists_what_collides() {
    let p = with_motions();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("frame").is_some()));
    p.ok(&["motion", "show", "slam", "--part", "frame"]);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.assembly.motions.frames_current()));
    ws.sweep_motion();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.assembly.motions.sweep.as_ref().is_some_and(|w| w.loaded)));
    let w = ws.assembly.motions.sweep.clone().unwrap();
    assert!(w.error.is_none(), "{w:?}");
    assert_eq!(w.pairs.len(), 1, "{w:?}");
    assert_eq!(w.pairs[0].label("frame"), "b1/shank and b2/shank");
    // Shanks of radius 3 mm meet when b2 has moved 34 mm of 40.
    assert!((w.pairs[0].t - 0.85).abs() < 0.01, "{}", w.pairs[0].t);
    assert!(
        w.marks()
            .iter()
            .any(|m| matches!(m, ryucad_app::core::motion::Mark::Contact(_)))
    );
    // At the contact the faces are painted; at the start they aren't.
    ws.jump_motion(1.0);
    assert!(
        !ws.assembly
            .motions
            .collision_faces(std::time::Instant::now())
            .is_empty()
    );
    ws.jump_motion(0.0);
    assert!(
        ws.assembly
            .motions
            .collision_faces(std::time::Instant::now())
            .is_empty()
    );
    // A change to the assembly makes it stale: listed, painted no more.
    p.ok(&["assembly", "move", "frame", "b2", "--translate", "0,10,0"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.assembly.motions.sweep.as_ref().is_some_and(|w| w.stale)
    }));
    ws.jump_motion(1.0);
    assert!(
        ws.assembly
            .motions
            .collision_faces(std::time::Instant::now())
            .is_empty()
    );
}

#[test]
fn a_mate_says_its_value_and_its_limits_are_edited_in_the_form() {
    let p = mated();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.part("frame").is_some_and(|f| f.feature("m1").is_some())));
    let m1 = |ws: &Workspace| {
        ws.part("frame")
            .unwrap()
            .feature("m1")
            .unwrap()
            .mate
            .clone()
            .unwrap()
    };
    assert!(
        m1(&ws).value.as_deref().is_some_and(|v| v.ends_with('°')),
        "{:?}",
        m1(&ws)
    );
    assert_eq!(m1(&ws).limits, Default::default());
    ws.open_form(Some(key("frame", "m1")));
    assert!(ws.wait_until(WAIT, |ws| ws.forms.form.is_some()));
    let f = ws.forms.form.as_mut().unwrap().mate.as_mut().unwrap();
    f.limits[0].text = "-10 deg".into();
    f.limits[1].text = "35 deg".into();
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.forms
            .form
            .as_ref()
            .is_some_and(|f| !f.saving && !f.dirty())
    }));
    let shown = p.ok(&["feature", "show", "frame", "m1"]);
    assert_eq!(shown["limits"]["angle"]["min"], "-10 deg", "{shown}");
    assert_eq!(shown["limits"]["angle"]["max"], "35 deg", "{shown}");
    // The report's limits reach the window, for its arcs.
    assert!(ws.wait_until(WAIT, |ws| m1(ws).limits.angle.is_some()));
    let (lo, hi) = m1(&ws).limits.angle.unwrap();
    assert!((lo.unwrap() + 10f64.to_radians()).abs() < 1e-12);
    assert!((hi.unwrap() - 35f64.to_radians()).abs() < 1e-12);
}
