//! Solids in the window against a real daemon: one mesh per solid as the
//! model shows it, features grouped under the solids they act on, and a
//! chosen feature painted by the faces it made.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn key(f: &str) -> (String, String) {
    ("p".to_owned(), f.to_owned())
}

/// The window, once the model and every solid's mesh have come.
fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.part("p").is_some_and(|p| {
                p.solids
                    .iter()
                    .all(|s| ws.model.meshes.contains_key(&key(&s.id)))
            })
    }));
    ws
}

#[test]
fn the_window_shows_solids() {
    let p = Project::new().with_part();
    // A slot cut across the base, and the boss joined to it.
    p.ok(&[
        "feature",
        "add",
        "p",
        "box",
        "--id",
        "slot",
        "--operation",
        "cut",
        "--target",
        "base",
        "--param",
        "x=4",
        "--param",
        "y=20",
        "--param",
        "z=2",
        "--origin",
        "8,-5,4",
    ]);
    p.ok(&[
        "feature", "add", "p", "boolean", "--id", "whole", "--op", "unite", "--target", "base",
        "--tool", "boss",
    ]);
    let mut ws = open(&p);
    // One solid, and its mesh; the boss is the base's now.
    let part = ws.part("p").unwrap();
    assert_eq!(part.solids.len(), 1);
    assert_eq!(part.solids[0].features, ["base", "slot", "whole"]);
    assert_eq!(ws.model.meshes.len(), 1);
    assert!(ws.model.meshes.contains_key(&key("base")));
    let slot = part.feature("slot").unwrap();
    assert_eq!(slot.operation.as_deref(), Some("cut"));
    assert_eq!(part.feature("whole").unwrap().consumes, ["boss"]);
    // A feature shows in its solid.
    assert_eq!(ws.model.solid_of(&key("slot")), Some(key("base")));
    assert_eq!(ws.model.solid_of(&key("boss")), None);
    // Choosing the slot paints the faces it made: its walls and floor, in
    // the base.
    ws.choose(Some(key("slot")));
    assert!(ws.wait_until(WAIT, |ws| ws.made.is_some()));
    let made = ws.made.clone().unwrap();
    assert_eq!(made.feature, key("slot"));
    let faces = &made.faces[&key("base")];
    assert_eq!(faces.len(), 3, "{faces:?}");
    // An edit moves what it made with it.
    p.ok(&[
        "feature", "edit", "p", "slot", "--param", "z=10", "--origin", "8,-5,-1",
    ]);
    let before = made.revision.clone();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.made.as_ref().is_some_and(|m| m.revision != before)
    }));
    // Through the slot: its floor is gone, its walls go right through.
    assert_eq!(ws.made.as_ref().unwrap().faces[&key("base")].len(), 2);
    ws.choose(None);
    assert!(ws.made.is_none());
}
