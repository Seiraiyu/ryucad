//! ryupdm in the window against a real daemon: release states, part
//! properties and the Releases tab, through the same requests an agent
//! sends.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.part("p").is_some()));
    ws
}

/// A part `p` with a box and a cylinder, numbered 100, committed on `main`.
fn numbered() -> Project {
    let p = Project::new().with_part();
    p.ok(&["pdm", "init"]);
    p.ok(&["props", "set", "p", "number=100"]);
    p.committed()
}

fn badge(ws: &Workspace) -> Option<String> {
    ws.states.badge("p")
}

#[test]
fn badges_follow_proposals_and_the_release_branch() {
    let p = numbered();
    let mut ws = open(&p);
    assert_eq!(badge(&ws).as_deref(), Some("in work"));
    p.ok(&["release", "propose", "p"]);
    assert!(ws.wait_until(WAIT, |ws| badge(ws).as_deref()
        == Some("in work, A proposed")));
    // Official once main holds it; work on it is work towards B.
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "release p A"]);
    assert!(ws.wait_until(WAIT, |ws| badge(ws).as_deref() == Some("A")));
    p.ok(&["feature", "edit", "p", "base", "--param", "x=30"]);
    assert!(ws.wait_until(WAIT, |ws| badge(ws).as_deref() == Some("A modified")));
}

/// Opens `p` with the Part section showing part `p`.
fn with_props(p: &Project) -> Workspace {
    let mut ws = open(p);
    ws.show_props(Some("p"));
    assert!(ws.wait_until(WAIT, |ws| ws.props.is_some()));
    ws
}

fn field<'a>(ws: &'a mut Workspace, name: &str) -> &'a mut ryucad_app::core::edit::Field {
    &mut ws
        .props
        .as_mut()
        .unwrap()
        .props
        .iter_mut()
        .find(|r| r.field.name == name)
        .unwrap()
        .field
}

fn number_on_disk(p: &Project) -> serde_json::Value {
    let got = p.ok(&["props", "show", "p"]);
    got["properties"][0]["value"].clone()
}

#[test]
fn a_property_saves_through_props_set() {
    let p = numbered();
    let mut ws = with_props(&p);
    assert_eq!(field(&mut ws, "number").read, "100");
    field(&mut ws, "number").text = "101".into();
    field(&mut ws, "name").text = "Bracket".into();
    ws.save_props();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.props
            .as_ref()
            .is_some_and(|p| !p.saving && !p.dirty() && p.props[0].field.read == "101")
    }));
    assert_eq!(number_on_disk(&p), "101");
    let log = p.ok(&["activity"]);
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["method"], "props.set");
    assert_eq!(last["participant"]["id"], ws.me());
}

#[test]
fn a_stale_save_conflicts_and_shows_what_changed() {
    let p = numbered();
    let mut ws = with_props(&p);
    field(&mut ws, "number").text = "101".into();
    p.ok(&["props", "set", "p", "number=102"]);
    ws.save_props();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.props.as_ref().is_some_and(|p| !p.changed.is_empty())
    }));
    let c = &ws.props.as_ref().unwrap().changed[0];
    assert_eq!((c.theirs.as_str(), c.yours.as_str()), ("102", "101"));
    assert_eq!(number_on_disk(&p), "102");
}

#[test]
fn a_value_the_settings_refuse_keeps_what_was_typed() {
    let p = numbered();
    p.ok(&[
        "pdm",
        "configure",
        "--property",
        r#"finish={"type": "enum", "values": ["anodized", "bare"]}"#,
    ]);
    let mut ws = with_props(&p);
    field(&mut ws, "finish").text = "red".into();
    ws.save_props();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.props.as_ref().is_some_and(|p| p.error.is_some())
    }));
    let e = ws.props.as_ref().unwrap().error.as_ref().unwrap();
    assert_eq!(e.code, ryucad::error::Code::PropertyInvalid);
    assert_eq!(field(&mut ws, "finish").text, "red");
}

#[test]
fn pdm_is_set_up_from_the_window() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.pdm_config.is_some()));
    assert!(!ws.pdm_ready());
    ws.pdm_init();
    assert!(ws.wait_until(WAIT, |ws| ws.pdm_ready()));
    let config = p.ok(&["pdm", "config"]);
    assert!(config["project"].is_string());
    assert_eq!(config["configured"], true);
}

/// The Releases tab of `p`, read.
fn with_releases(p: &Project) -> Workspace {
    let mut ws = open(p);
    ws.load_releases();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.releases.list.is_some()));
    ws
}

fn lines(ws: &Workspace) -> Vec<String> {
    ws.releases
        .records("p")
        .into_iter()
        .map(ryucad_app::core::releases::record_line)
        .collect()
}

#[test]
fn a_release_is_proposed_synced_and_made_obsolete_from_the_tab() {
    let p = numbered();
    let mut ws = with_releases(&p);
    assert!(ws.releases.in_repository());
    assert!(lines(&ws).is_empty());
    // The dialog's dry run: the label it takes, ready.
    ws.propose_dry("p");
    assert!(ws.wait_until(WAIT, |ws| ws.releases.proposal.is_some()));
    let (part, dry) = ws.releases.proposal.clone().unwrap();
    assert_eq!(
        (part.as_str(), &dry["label"], &dry["ready"]),
        ("p", &serde_json::json!("A"), &serde_json::json!(true))
    );
    ws.propose("p", Some("first".into()));
    assert!(ws.wait_until(WAIT, |ws| {
        !ws.releases.busy && ws.releases.records("p").len() == 1
    }));
    assert_eq!(ws.releases.done.as_deref(), Some("wrote releases/p/A.json"));
    assert!(ws.releases.proposal.is_none());
    assert!(
        lines(&ws)[0].starts_with("A release, proposed: first ("),
        "{:?}",
        lines(&ws)
    );
    // Official on main: its tag is missing until synced.
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "release p A"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle() && ws.releases.to_sync() == [("ryupdm/p/A".to_owned(), "create")]
    }));
    assert!(
        ws.releases
            .findings_of("p")
            .iter()
            .any(|f| f["code"] == "release_tag_missing")
    );
    ws.sync_tags();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle() && !ws.releases.busy && ws.releases.to_sync().is_empty()
    }));
    assert_eq!(
        ws.releases.done.as_deref(),
        Some("tags: 1 created, 0 moved")
    );
    assert!(
        ws.releases.findings_of("p").is_empty(),
        "{:?}",
        ws.releases.findings
    );
    assert!(
        lines(&ws)[0].contains(", tag ok: first"),
        "{:?}",
        lines(&ws)
    );
    // Made obsolete: only its latest official revision can be.
    assert_eq!(ws.releases.obsoletable("p").as_deref(), Some("A"));
    ws.obsolete("p", "A", Some("replaced".into()));
    assert!(ws.wait_until(WAIT, |ws| {
        !ws.releases.busy && ws.releases.records("p").len() == 2
    }));
    assert_eq!(ws.releases.obsoletable("p"), None);
    assert!(
        lines(&ws)[1].starts_with("A obsolete, proposed: replaced"),
        "{:?}",
        lines(&ws)
    );
    // A refusal is said, not lost.
    ws.obsolete("p", "A", None);
    assert!(ws.wait_until(WAIT, |ws| ws.releases.error.is_some()));
    assert_eq!(
        ws.releases.error.as_ref().unwrap().code,
        ryucad::error::Code::ReleasePending
    );
}

#[test]
fn a_part_that_isnt_ready_says_why_before_proposing() {
    let p = Project::new().with_part().committed();
    let mut ws = with_releases(&p);
    ws.propose_dry("p");
    assert!(ws.wait_until(WAIT, |ws| ws.releases.proposal.is_some()));
    let dry = &ws.releases.proposal.as_ref().unwrap().1;
    assert_eq!(dry["ready"], false);
    assert_eq!(dry["problems"][0]["details"]["property"], "number", "{dry}");
}

#[test]
fn a_number_a_merge_duplicated_reaches_the_conflict_panel() {
    let p = numbered();
    p.ok(&["part", "create", "q"]);
    p.ok(&["part", "create", "s"]);
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "two more parts"]);
    p.git(&["checkout", "-q", "-b", "theirs"]);
    p.ok(&["props", "set", "q", "number=300"]);
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "theirs: q is 300"]);
    p.git(&["checkout", "-q", "main"]);
    p.ok(&["props", "set", "s", "number=300"]);
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "ours: s is 300"]);
    assert!(p.try_git(&["merge", "--no-commit", "--no-ff", "theirs"]));
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.merge.is_some()));
    let m = ws.merge.as_ref().unwrap();
    let titles: Vec<String> = m
        .conflicts()
        .iter()
        .map(ryucad_app::core::merge::title)
        .collect();
    assert_eq!(
        titles,
        ["pdm_broke in q number", "pdm_broke in s number"],
        "{}",
        m.status
    );
    // Settled in the Part section, as a person would: the merge is clean.
    ws.show_props(Some("s"));
    assert!(ws.wait_until(WAIT, |ws| ws.props.is_some()));
    field(&mut ws, "number").text = "301".into();
    ws.save_props();
    assert!(ws.wait_until(WAIT, |ws| ws.merge.as_ref().is_some_and(|m| m.clean())));
}

#[test]
fn typing_right_after_setting_up_pdm_is_kept() {
    // #73: the properties read again after `pdm.init` replaced the form.
    let p = Project::new().with_part();
    let mut ws = with_props(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.pdm_config.is_some()));
    ws.pdm_init();
    // Typed before the answer, and the read after it, land.
    field(&mut ws, "number").text = "100".into();
    assert!(ws.wait_until(WAIT, |ws| ws.pdm_ready() && ws.idle()));
    assert_eq!(field(&mut ws, "number").text, "100");
    ws.save_props();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.props
            .as_ref()
            .is_some_and(|p| !p.saving && !p.dirty() && p.props[0].field.read == "100")
    }));
    assert_eq!(number_on_disk(&p), "100");
}
