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
