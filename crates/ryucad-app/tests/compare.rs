//! The window in a compare, against a real daemon and a real repository:
//! what an agent's `ryucad compare` starts, the diff it reads and the
//! ghosts it fetches.

mod common;

use common::{Project, ryucad};
use ryucad::error::Code;
use ryucad_app::core::compare::{Blink, Change, FaceMark, Front};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap()
}

/// [`Project::with_part`] committed in a repository of its own, then
/// changed: the box is longer, the cylinder gone, a sphere added.
fn changed() -> Project {
    let p = Project::new().with_part().committed();
    p.ok(&["feature", "edit", "p", "base", "--param", "x=30"]);
    p.ok(&["feature", "delete", "p", "boss"]);
    p.ok(&[
        "feature", "add", "p", "sphere", "--id", "ball", "--param", "radius=2",
    ]);
    p
}

fn key(f: &str) -> (String, String) {
    ("p".to_owned(), f.to_owned())
}

/// The compare has its diff and every ghost it wants.
fn ghosted(ws: &Workspace) -> bool {
    ws.idle()
        && ws
            .compare
            .as_ref()
            .is_some_and(|c| c.loaded && c.ghosts.len() == c.wanted.len())
}

#[test]
fn an_agent_puts_the_window_into_a_compare() {
    let p = changed();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    // The one human attached is the window: the agent's compare goes there.
    let r = p.ok(&["compare", "HEAD"]);
    assert_eq!(r["participant"], ws.me());
    assert!(ws.wait_until(WAIT, ghosted));
    let c = ws.compare.as_ref().unwrap();
    assert_eq!(c.rev, "HEAD");
    assert_eq!(c.commit.as_deref(), r["compare"]["commit"].as_str());
    assert_eq!(c.features[&key("base")].change, Change::Changed);
    assert_eq!(c.features[&key("boss")].change, Change::Removed);
    assert_eq!(c.features[&key("ball")].change, Change::Added);
    assert_eq!(c.removed["p"], ["boss"]);
    // The box's end faces moved; its sides changed; its top took a new
    // name only where the cylinder no longer stands.
    assert!(
        c.faces[&key("base")]
            .values()
            .any(|m| *m == FaceMark::Changed)
    );
    assert!(c.new_bodies.contains(&key("ball")));
    // Ghosts: the box and the cylinder as HEAD has them.
    let ghosts: Vec<&(String, String)> = c.ghosts.keys().collect();
    assert_eq!(ghosts, [&key("base"), &key("boss")]);
    assert!(c.gone_bodies.contains(&key("boss")));
    assert_ne!(
        c.ghosts[&key("base")].hash,
        ws.model.meshes[&key("base")].hash
    );
    // An edit made meanwhile is diffed again.
    p.ok(&["feature", "edit", "p", "base", "--param", "x=20"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ghosted(ws)
            && ws
                .compare
                .as_ref()
                .is_some_and(|c| !c.features.contains_key(&key("base")))
    }));
    assert!(
        !ws.compare
            .as_ref()
            .unwrap()
            .ghosts
            .contains_key(&key("base"))
    );
    // The window ends it itself.
    ws.set_compare(None);
    assert!(ws.wait_until(WAIT, |ws| ws.compare.is_none()));
    assert_eq!(p.ok(&["compare"])["compare"], serde_json::Value::Null);
}

#[test]
fn swapped_the_revision_is_drawn_and_its_ghost_is_the_working_files() {
    let p = changed();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    p.ok(&["compare", "HEAD"]);
    assert!(ws.wait_until(WAIT, ghosted));
    ws.swap_compare();
    // HEAD's model: the box and the cylinder; the working files' box and
    // sphere ghosted.
    assert!(ws.wait_until(WAIT, |ws| {
        ghosted(ws)
            && ws.model.meshes.contains_key(&key("boss"))
            && ws
                .compare
                .as_ref()
                .is_some_and(|c| c.swapped.as_deref() == Some("HEAD"))
    }));
    assert_eq!(ws.model_rev().as_deref(), Some("HEAD"));
    let c = ws.compare.as_ref().unwrap();
    assert_eq!(
        (c.rev.as_str(), c.to.as_deref(), c.named()),
        ("working", Some("HEAD"), "HEAD")
    );
    assert!(c.new_bodies.contains(&key("boss")));
    let ghosts: Vec<&(String, String)> = c.ghosts.keys().collect();
    assert_eq!(ghosts, [&key("ball"), &key("base")]);
    assert!(c.gone_bodies.contains(&key("ball")));
    // An agent's write lands in the working files: the ghost follows,
    // and HEAD's model stays.
    let before = c.wanted[&key("base")].clone();
    p.ok(&["feature", "edit", "p", "base", "--param", "x=25"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ghosted(ws)
            && ws
                .compare
                .as_ref()
                .is_some_and(|c| c.wanted.get(&key("base")).is_some_and(|h| *h != before))
    }));
    assert!(ws.model.meshes.contains_key(&key("boss")));
    // HEAD alone: nothing painted, nothing ghosted.
    ws.compare_only(Front::Rev);
    assert!(ws.painted().is_none());
    assert_eq!(ws.model_rev().as_deref(), Some("HEAD"));
    // B with one side alone: the working files alone.
    ws.swap_compare();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle() && !ws.model.meshes.contains_key(&key("boss"))
    }));
    assert_eq!(ws.model_rev(), None);
    assert!(ws.painted().is_none());
    ws.compare_both();
    assert!(ws.painted().is_some());
    // Stopping the compare starts the blink again.
    p.ok(&["compare", "--off"]);
    assert!(ws.wait_until(WAIT, |ws| ws.compare.is_none()));
    assert_eq!(ws.blink, Blink::default());
}

#[test]
fn a_window_compares_with_any_revision() {
    let p = changed();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    ws.set_compare(Some("main".into()));
    assert!(ws.wait_until(WAIT, ghosted));
    assert_eq!(ws.compare.as_ref().unwrap().counts(), (1, 1, 1));
    // A revision that doesn't resolve is refused, and the compare stays.
    ws.set_compare(Some("nowhere".into()));
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && !ws.errors.is_empty()));
    assert_eq!(ws.errors.last().unwrap().code, Code::RevNotFound);
    assert_eq!(ws.compare.as_ref().unwrap().rev, "main");
}

#[test]
fn a_proposal_and_property_changes_show_in_a_compare() {
    let p = Project::new().with_part();
    p.ok(&["pdm", "init"]);
    p.ok(&["props", "set", "p", "number=100"]);
    let p = p.committed();
    p.ok(&["props", "set", "p", "number=101"]);
    p.ok(&["release", "propose", "p"]);
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    ws.set_compare(Some("HEAD".into()));
    assert!(ws.wait_until(WAIT, ghosted));
    let m = &ws.compare.as_ref().unwrap().pdm["p"];
    assert_eq!(m.marks, ["proposes A"]);
    assert_eq!(
        m.lines,
        [
            "number: 100 → 101 (revising)",
            "state: in work → in work, A proposed"
        ]
    );
}

#[test]
fn an_assemblys_occurrences_are_painted_and_ghosted_where_they_were() {
    // Two bolts placed in `frame`, committed; then one moved and the
    // bolt's head made bigger, so both differ (issue #109).
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
        "feature", "add", "frame", "instance", "--id", "b1", "--of", "bolt", "--fixed",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b2", "--of", "bolt", "--origin", "40,0,0",
    ]);
    let p = p.committed();
    p.ok(&["assembly", "move", "frame", "b2", "--translate", "0,10mm,0"]);
    p.ok(&["feature", "edit", "bolt", "shank", "--param", "radius=4"]);
    p.ok(&["focus", "frame"]);
    let mut ws = open(&p);
    let k = |s: &str| ("frame".to_owned(), format!("{s}/shank"));
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.model.meshes.contains_key(&k("b1"))));
    p.ok(&["compare", "HEAD"]);
    assert!(ws.wait_until(WAIT, ghosted));
    let c = ws.compare.as_ref().unwrap();
    // Both bolts' faces differ, and both are ghosted: b1 where it is, b2
    // where it was, 10 mm off.
    for b in ["b1", "b2"] {
        assert!(!c.faces[&k(b)].is_empty(), "{b}: {:?}", c.faces);
        assert!(c.ghosts.contains_key(&k(b)), "{b}: {:?}", c.ghosts.keys());
    }
    assert_eq!(c.ghost_pose(&k("b1")).origin, [0.0; 3]);
    assert_eq!(c.ghost_pose(&k("b2")).origin, [0.04, 0.0, 0.0]);
    assert_ne!(c.ghosts[&k("b2")].hash, ws.model.meshes[&k("b2")].hash);
}

#[test]
fn writes_wait_while_the_revision_is_drawn() {
    use ryucad_app::core::compare::SWAPPED;
    use ryucad_app::core::modes::Gate;
    let p = changed();
    let mut ws = open(&p);
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    p.ok(&["compare", "HEAD"]);
    assert!(ws.wait_until(WAIT, ghosted));
    assert_eq!(ws.control("feature.edit"), Gate::Send);
    ws.swap_compare();
    assert_eq!(ws.control("feature.edit"), Gate::Refuse(SWAPPED));
    assert_eq!(ws.control("annotation.create"), Gate::Refuse(SWAPPED));
    // A write sent anyway is answered here with the reason, never sent
    // (the daemon would have said nothing_to_undo).
    ws.undo(None);
    let e = ws.errors.last().unwrap();
    assert_eq!(e.code, Code::ParticipantMode);
    assert!(e.message.ends_with(SWAPPED), "{}", e.message);
    ws.swap_compare();
    assert_eq!(ws.control("feature.edit"), Gate::Send);
}
