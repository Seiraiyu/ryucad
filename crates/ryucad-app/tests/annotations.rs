//! Annotating from the window, against real daemons (design 374): the
//! window lists and follows annotations, proposes in Annotate, draws its
//! person's draft and follows the agent's replies and changes.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::modes::Mode;
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);

fn open(p: &Project, mode: Mode) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    ws.set_mode(mode);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.annotations.loaded
        && ws.model.meshes.len() == 2));
    ws
}

#[test]
fn the_window_lists_and_follows_annotations() {
    let p = Project::new().with_part();
    let face = format!("p/base/{}", common::face_up(&p, "p", "base", 1.0));
    let made = p.ok(&["annotations", "add", &face, "--comment", "Round this"]);
    let id = made["annotation"]["id"].as_str().unwrap().to_owned();
    let mut ws = open(&p, Mode::Annotate);
    assert_eq!(ws.annotations.all[&id].comment, "Round this");
    // The window makes one; the agent sees it.
    ws.create_annotation(Vec::new(), "And this".into(), None);
    assert!(ws.wait_until(WAIT, |ws| ws.annotations.all.len() == 2));
    let mine = ws
        .annotations
        .all
        .values()
        .find(|e| e.comment == "And this")
        .unwrap()
        .id
        .clone();
    assert_eq!(ws.annotations.all[&mine].by, "Dana");
    // The agent replies and resolves: the window follows.
    p.ok(&["annotations", "reply", &mine, "On it"]);
    p.ok(&["annotations", "resolve", &mine, "--note", "Done"]);
    assert!(ws.wait_until(WAIT, |ws| ws.annotations.all[&mine].status == "done"));
    assert_eq!(ws.annotations.all[&mine].replies[0].text, "On it");
    // Deleted from outside: gone here too.
    p.ok(&["annotations", "delete", &id]);
    assert!(ws.wait_until(WAIT, |ws| !ws.annotations.all.contains_key(&id)));
}

/// The hash of the boss's body as the window draws it.
fn drawn(ws: &Workspace) -> Option<String> {
    ws.model
        .parts
        .get("p")?
        .solids
        .iter()
        .find(|s| s.id == "boss")?
        .hash
        .clone()
}

fn set(ws: &mut Workspace, name: &str, text: &str) {
    let f = ws.form.as_mut().unwrap();
    f.params.iter_mut().find(|x| x.name == name).unwrap().text = text.into();
}

#[test]
fn the_window_draws_and_reads_its_persons_draft() {
    let p = Project::new().with_part();
    let mut ws = open(&p, Mode::Annotate);
    let working = drawn(&ws).unwrap();
    ws.open_form(Some(("p".into(), "boss".into())));
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    set(&mut ws, "radius", "4 mm");
    ws.save_form();
    // The draft is drawn: the boss as suggested, the files as they were.
    assert!(ws.wait_until(WAIT, |ws| ws.model_rev().is_some() && ws.idle()));
    assert!(ws.wait_until(WAIT, |ws| drawn(ws).is_some_and(|h| h != working)));
    // The draft is painted as a compare against the agent's model.
    assert!(ws.wait_until(WAIT, |ws| ws.compare.as_ref().is_some_and(|c| c.loaded)));
    let c = ws.compare.as_ref().unwrap();
    assert_eq!(c.rev, "working");
    assert_eq!(c.to.as_deref(), Some("draft:Dana"));
    assert!(
        c.features
            .contains_key(&("p".to_owned(), "boss".to_owned())),
        "{:?}",
        c.features
    );
    // A second suggestion stands on the first: the form read the draft.
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.form.as_ref().is_some_and(|f| {
            f.params
                .iter()
                .any(|x| x.name == "radius" && x.read == "4 mm")
        })));
    set(&mut ws, "height", "9 mm");
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| ws.annotations.draft(ws.my_name()).len() == 2));
    // Show agent's model: the working files.
    ws.set_show_agent(true);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && drawn(ws).as_deref() == Some(working.as_str())));
    ws.set_show_agent(false);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && drawn(ws).is_some_and(|h| h != working)));
    // Choosing a suggestion compares with it, and pauses the draft.
    let first = ws.annotations.draft("Dana")[0].clone();
    ws.choose_annotation(Some(&first));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model_rev().is_none()
            && ws
                .compare
                .as_ref()
                .is_some_and(|c| c.rev == format!("annotation:{first}"))
    }));
    ws.choose_annotation(None);
    assert!(ws.wait_until(WAIT, |ws| ws.model_rev().is_some()
        && ws.compare.as_ref().is_some_and(|c| c.to.is_some())));
}

#[test]
fn a_part_asked_for_as_the_drawn_revision_moves_is_asked_for_again() {
    let p = Project::new().with_part();
    let mut ws = open(&p, Mode::Annotate);
    let working = drawn(&ws).unwrap();
    ws.open_form(Some(("p".into(), "boss".into())));
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    set(&mut ws, "radius", "4 mm");
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.model_rev().is_some()
        && drawn(ws).is_some_and(|h| h != working)));
    // The agent's model drawn, the part not yet loaded (a lazily loaded
    // project's row opened): its first report is asked at the working
    // files, and the drawn revision moves back to the draft before the
    // answer comes, which drops it.
    ws.set_show_agent(true);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model_rev().is_none()));
    ws.model.parts.remove("p");
    ws.load_part("p");
    ws.set_show_agent(false);
    // It is asked for again at the draft, not left loading.
    assert!(
        ws.wait_until(WAIT, |ws| ws.idle()
            && ws.loading.is_empty()
            && drawn(ws).is_some_and(|h| h != working)),
        "still loading {:?}",
        ws.loading
    );
}

fn radius_on_disk(p: &Project) -> String {
    p.ok(&["feature", "show", "p", "boss"])["params"]["radius"]["value"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Design 374's "When this stage is done", the window's half; the
/// agent's from the CLI.
#[test]
fn a_person_annotates_and_the_agent_works_through_it() {
    let p = Project::new().with_part();
    let mut ws = open(&p, Mode::Annotate);
    // A comment pinned to the block's top.
    let face = format!("p/base/{}", common::face_up(&p, "p", "base", 1.0));
    let pin = ryucad::proto::PinArg {
        reference: Some(face),
        part: None,
        point: Some(ryucad_app::core::annotations::point_arg([
            0.015, 0.006, 0.005,
        ])),
    };
    ws.create_annotation(
        vec![pin],
        "Move this hole 5 mm towards the edge".into(),
        None,
    );
    // A radius suggested from the form.
    ws.open_form(Some(("p".into(), "boss".into())));
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    set(&mut ws, "radius", "2 mm");
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| ws.annotations.all.len() == 2 && ws.idle()));
    // The agent sees two open annotations, one carrying a change.
    let list = p.ok(&["annotations", "list", "--open"]);
    let all = list["annotations"].as_array().unwrap();
    assert_eq!(all.len(), 2);
    let id = |change: bool| {
        all.iter()
            .find(|a| a["change"].is_null() != change)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (change, comment) = (id(true), id(false));
    let d = p.ok(&[
        "diff",
        "--from",
        "working",
        "--to",
        &format!("annotation:{change}"),
        "--model",
    ]);
    assert_eq!(d["parts"]["p"]["features"]["boss"]["status"], "changed");
    // It applies the radius and replies; it does the comment itself.
    p.ok(&["annotations", "apply", &change]);
    p.ok(&["annotations", "reply", &comment, "Moved it in the sketch."]);
    p.ok(&["annotations", "resolve", &comment, "--note", "Done"]);
    // The window follows: replies toasted, the model changed, no draft.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.annotations.all[&comment].status == "done"
            && ws.annotations.all[&change].status == "done"
    }));
    assert!(
        ws.toasts
            .shown()
            .iter()
            .any(|t| t.text.starts_with("replied on"))
    );
    assert!(ws.wait_until(WAIT, |ws| ws.model_rev().is_none() && ws.idle()));
    assert_eq!(radius_on_disk(&p), "2 mm");
}

#[test]
fn view_refuses_and_direct_edit_writes() {
    let p = Project::new().with_part();
    let mut ws = open(&p, Mode::View);
    ws.create_annotation(Vec::new(), "Hello".into(), None);
    assert!(ws.wait_until(WAIT, |ws| !ws.errors.is_empty()));
    assert_eq!(
        ws.errors.last().unwrap().code,
        ryucad::error::Code::ParticipantMode
    );
    assert!(
        p.ok(&["annotations", "list"])["annotations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    ws.set_mode(Mode::Edit);
    ws.open_form(Some(("p".into(), "boss".into())));
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    set(&mut ws, "radius", "5 mm");
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())
    }));
    assert_eq!(radius_on_disk(&p), "5 mm");
}

#[test]
fn a_reconnected_window_is_in_view_again() {
    let p = Project::new().with_part();
    let mut ws = open(&p, Mode::Edit);
    p.ok(&["daemon", "stop"]);
    assert!(ws.wait_until(WAIT, |ws| ws.lost));
    // Reconnecting is the window's frames' work, as tests/reconnect.rs
    // drives it.
    let t = std::time::Instant::now();
    while ws.lost {
        assert!(t.elapsed() < WAIT, "timed out");
        ws.pump();
        ws.tick(std::time::Instant::now());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(ws.mode, Mode::View);
    // And the daemon says so.
    assert!(ws.wait_until(WAIT, |ws| ws.myself().is_some_and(|m| m.mode == Mode::View)));
}

#[test]
fn a_freehand_drag_in_annotate_is_a_rough_suggestion_with_a_comment_to_edit() {
    let p = common::two_bolts();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    ws.set_mode(Mode::Annotate);
    assert!(ws.wait_until(WAIT, |w| w.idle()
        && w.annotations.loaded
        && w.model.middles("frame").contains_key("b2")));
    ws.drag_to("frame", "b2", [0.01, 0.0, 0.0]);
    assert!(ws.wait_until(WAIT, |w| w.idle()));
    ws.drop_drag();
    assert!(ws.wait_until(WAIT, |w| w.annotation_ui.editing.is_some()));
    let listed = p.ok(&["annotations", "list"]);
    let a = &listed["annotations"][0];
    assert_eq!(a["change"]["rough"], true, "{a}");
    assert_eq!(a["comment"], "Move about here", "{a}");
    assert_eq!(a["change"]["request"]["method"], "assembly.move", "{a}");
    let (id, text) = ws.annotation_ui.editing.clone().unwrap();
    assert_eq!(
        (id.as_str(), text.as_str()),
        (a["id"].as_str().unwrap(), "Move about here")
    );
    assert_eq!(ws.wants_tile, Some("annotations"));
    assert_eq!(ws.chosen_annotation.as_deref(), Some(id.as_str()));
    // Nothing moved.
    let x = p.ok(&["feature", "show", "frame", "b2"])["pose"]["origin_m"][0]
        .as_f64()
        .unwrap();
    assert!((x - 0.04).abs() < 1e-12, "{x}");
}

/// Looking straight down at `p`'s block (20 × 10 × 5 mm at the origin):
/// the view's plane is the model's xy about (10, 5) mm.
fn from_above() -> ryucad::view::Camera {
    ryucad::view::Camera {
        eye: [0.01, 0.005, 1.0],
        target: [0.01, 0.005, 0.0025],
        up: [0.0, 1.0, 0.0],
        projection: ryucad::view::Projection::Orthographic { height: 0.03 },
    }
}

/// An annotation's marks as `annotation.list` gives them.
fn listed_marks(p: &Project, id: &str) -> serde_json::Value {
    let list = p.ok(&["annotations", "list"]);
    list["annotations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == id)
        .unwrap()["marks"]
        .clone()
}

#[test]
fn a_markup_is_posted_with_what_its_marks_touch_and_edited_again() {
    use ryucad_app::core::markup::Tool;
    let p = Project::new().with_part();
    let top = format!("p/base/{}", common::face_up(&p, "p", "base", 1.0));
    let mut ws = open(&p, Mode::Annotate);
    ws.open_markup(from_above()).unwrap();
    let m = ws.markup.as_mut().unwrap();
    assert_eq!(m.part, "p");
    // An arrow across the block's top, clear of the boss.
    m.press([0.005, 0.002]);
    m.drag([0.007, 0.004], false);
    m.release(1e-4);
    // A stroke on the block's top face, in its part's frame.
    m.choose(Tool::PenFaces);
    let key = ("p".to_owned(), "base".to_owned());
    m.press_face(key.clone(), "p/base".into(), [0.012, 0.002, 0.005]);
    m.drag_face(Some((key.clone(), [0.016, 0.002, 0.005])));
    m.drag_face(Some((key, [0.018, 0.006, 0.005])));
    m.release(1e-4);
    // Post wants words: a mark says where.
    ws.post_markup();
    assert!(ws.markup.as_ref().unwrap().note.is_some());
    ws.markup.as_mut().unwrap().comment = "Round this edge 2 mm".into();
    ws.post_markup();
    assert!(ws.wait_until(WAIT, |ws| ws.markup.is_none()
        && ws.annotations.all.len() == 1));
    let id = ws.annotations.all.keys().next().unwrap().clone();
    assert_eq!(ws.chosen_annotation.as_deref(), Some(id.as_str()));
    // The daemon worked out what each touches: the block's top.
    let marks = listed_marks(&p, &id);
    assert_eq!(marks["kinds"], serde_json::json!(["arrow", "stroke"]));
    let list = marks["list"].as_array().unwrap();
    assert_eq!(list[0]["touches"][0]["ref"], top.as_str(), "{marks}");
    assert_eq!(list[0]["touches"][1]["ref"], top.as_str(), "{marks}");
    assert_eq!(list[1]["on"], "p/base");
    assert_eq!(list[1]["touches"][0]["ref"], top.as_str(), "{marks}");
    // The window draws its face stroke, and its marker at its arrow's tail.
    let e = &ws.annotations.all[&id];
    assert_eq!(e.mark_count(), 2);
    let strokes = ws
        .annotations
        .face_strokes(&["p"], |on| ws.model.placed_on(on), None);
    assert_eq!(strokes.len(), 1);
    assert_eq!(strokes[0].points.len(), 3);
    let at = ws.annotations.markers(
        &["p"],
        |k| ws.model.pose(k),
        |on| ws.model.placed_on(on),
        None,
    );
    assert_eq!(at.len(), 1);
    assert_eq!(ws.face_strokes(true), strokes, "the window draws it");
    // Its author edits it: the bar reopens at its view with its marks; the
    // stroke undone, Save sends the arrow alone.
    ws.edit_markup(&id).unwrap();
    let m = ws.markup.as_mut().unwrap();
    assert_eq!((m.marks.len(), m.camera.clone()), (2, from_above()));
    assert_eq!(m.editing.as_deref(), Some(id.as_str()));
    // While the bar edits it, the window draws the bar's marks only: the
    // listing's stroke and marker are left out, so an undo shows.
    assert_eq!(ws.face_strokes(true), strokes, "the bar's copy, once");
    assert!(
        ws.annotations
            .markers(
                &["p"],
                |k| ws.model.pose(k),
                |on| ws.model.placed_on(on),
                ws.markup_editing(),
            )
            .is_empty()
    );
    let m = ws.markup.as_mut().unwrap();
    m.undo();
    assert!(
        ws.face_strokes(true).is_empty(),
        "the stroke undone is gone"
    );
    ws.post_markup();
    assert!(ws.wait_until(WAIT, |ws| ws.markup.is_none()
        && ws.annotations.all[&id].mark_count() == 1));
    assert_eq!(listed_marks(&p, &id)["kinds"], serde_json::json!(["arrow"]));
}

#[test]
fn an_agents_marks_from_the_cli_are_the_windows_to_draw() {
    let p = Project::new().with_part();
    p.ok(&[
        "view",
        "save",
        "p",
        "above",
        "--eye",
        "10,5,1000",
        "--target",
        "10,5,2.5",
        "--up",
        "0,1,0",
        "--ortho",
        "30 mm",
    ]);
    let mut ws = open(&p, Mode::Annotate);
    let stroke = r#"{"part": "p", "list": [{"kind": "stroke", "in": "face", "on": "p/base",
        "points_m": [[0.012, 0.002, 0.005], [0.018, 0.006, 0.005]]}]}"#;
    let a = p.ok(&[
        "annotations",
        "add",
        "--comment",
        "here",
        "--model",
        "p",
        "--of",
        "above",
        "--marks",
        stroke,
        "--arrow",
        "-5,-3:-3,-1",
    ]);
    let id = a["annotation"]["id"].as_str().unwrap().to_owned();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.annotations
            .all
            .get(&id)
            .is_some_and(|e| e.mark_count() == 2)
    }));
    let e = &ws.annotations.all[&id];
    assert!(e.view.is_some());
    assert_eq!(
        ws.annotations
            .face_strokes(&["p"], |on| ws.model.placed_on(on), None)
            .len(),
        1
    );
    // Resolved, its face strokes go with its pins.
    p.ok(&["annotations", "resolve", &id, "--note", "done"]);
    assert!(ws.wait_until(WAIT, |ws| !ws.annotations.all[&id].is_open()));
    assert!(
        ws.annotations
            .face_strokes(&["p"], |on| ws.model.placed_on(on), None)
            .is_empty()
    );
}

#[test]
fn markup_is_refused_in_view_and_a_refusal_keeps_the_marks() {
    let p = Project::new().with_part();
    let mut ws = open(&p, Mode::Annotate);
    ws.open_markup(from_above()).unwrap();
    let m = ws.markup.as_mut().unwrap();
    m.press([0.0, 0.0]);
    m.drag([0.002, 0.0], false);
    m.release(1e-4);
    m.comment = "here".into();
    // The window's mode turns to View under it: the daemon refuses Post.
    ws.set_mode(Mode::View);
    ws.post_markup();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.markup
            .as_ref()
            .is_some_and(|m| !m.posting && m.note.is_some())
    }));
    assert_eq!(ws.markup.as_ref().unwrap().marks.len(), 1, "kept");
    assert!(
        p.ok(&["annotations", "list"])["annotations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// #730: a chosen annotation's view marks are drawn only while the part
/// they were made on is shown.
#[test]
fn a_chosen_annotations_marks_go_when_another_part_is_shown() {
    let p = Project::new().with_part();
    p.ok(&["part", "create", "q"]);
    p.ok(&[
        "feature", "add", "q", "box", "--id", "b", "--param", "x=5", "--param", "y=5", "--param",
        "z=5",
    ]);
    p.ok(&[
        "view",
        "save",
        "p",
        "above",
        "--eye",
        "10,5,1000",
        "--target",
        "10,5,2.5",
        "--up",
        "0,1,0",
        "--ortho",
        "30 mm",
    ]);
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    ws.set_mode(Mode::Annotate);
    ws.set_focus(Some("p".into()));
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.annotations.loaded
        && ws.model.focus.as_deref() == Some("p")));
    let stroke = r#"{"part": "p", "list": [{"kind": "stroke", "in": "face", "on": "p/base",
        "points_m": [[0.012, 0.002, 0.005], [0.018, 0.006, 0.005]]}]}"#;
    let a = p.ok(&[
        "annotations",
        "add",
        "--comment",
        "here",
        "--model",
        "p",
        "--of",
        "above",
        "--marks",
        stroke,
        "--arrow",
        "-5,-3:-3,-1",
    ]);
    let id = a["annotation"]["id"].as_str().unwrap().to_owned();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.annotations
            .all
            .get(&id)
            .is_some_and(|e| e.mark_count() == 2)
    }));
    ws.choose_annotation(Some(&id));
    assert!(ws.view_marks().is_some());
    ws.set_focus(Some("q".into()));
    assert!(ws.wait_until(WAIT, |ws| ws.model.focus.as_deref() == Some("q")));
    assert!(ws.view_marks().is_none(), "p's marks drawn over q");
    ws.set_focus(Some("p".into()));
    assert!(ws.wait_until(WAIT, |ws| ws.model.focus.as_deref() == Some("p")));
    assert!(ws.view_marks().is_some());
}

/// #776: choosing an annotation made on a part that isn't shown shows
/// that part, so its marks are drawn.
#[test]
fn choosing_an_annotation_on_another_part_shows_that_part() {
    let p = Project::new().with_part();
    p.ok(&["part", "create", "q"]);
    p.ok(&[
        "feature", "add", "q", "box", "--id", "b", "--param", "x=5", "--param", "y=5", "--param",
        "z=5",
    ]);
    p.ok(&[
        "view",
        "save",
        "p",
        "above",
        "--eye",
        "10,5,1000",
        "--target",
        "10,5,2.5",
        "--up",
        "0,1,0",
        "--ortho",
        "30 mm",
    ]);
    let stroke = r#"{"part": "p", "list": [{"kind": "stroke", "in": "face", "on": "p/base",
        "points_m": [[0.012, 0.002, 0.005], [0.018, 0.006, 0.005]]}]}"#;
    let a = p.ok(&[
        "annotations",
        "add",
        "--comment",
        "here",
        "--model",
        "p",
        "--of",
        "above",
        "--marks",
        stroke,
    ]);
    let id = a["annotation"]["id"].as_str().unwrap().to_owned();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    ws.set_focus(Some("q".into()));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.model.focus.as_deref() == Some("q")
            && ws
                .annotations
                .all
                .get(&id)
                .is_some_and(|e| e.mark_count() == 1)
    }));
    ws.choose_annotation(Some(&id));
    assert!(
        ws.wait_until(WAIT, |ws| ws.model.focus.as_deref() == Some("p")),
        "choosing it didn't show p"
    );
    assert!(ws.view_marks().is_some());
}
