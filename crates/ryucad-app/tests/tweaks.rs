//! Face tweaks in the window against a real daemon (design 400, phase
//! 6): a draft's, move's and replacement's reports read, a chosen tweak
//! painting the faces it modified, and their forms editing as an agent's
//! `feature.edit` would.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::edit::{Form, TweakForm};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(120);

fn key(f: &str) -> (String, String) {
    ("p".to_owned(), f.to_owned())
}

/// The window, once the model and every solid's mesh have come.
fn open(p: &Project) -> Workspace {
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.part("p").is_some_and(|p| {
                !p.solids.is_empty()
                    && p.solids
                        .iter()
                        .all(|s| ws.model.meshes.contains_key(&key(&s.id)))
            })
    }));
    ws
}

/// The form of `id`, once `feature.show` has answered.
fn form(ws: &mut Workspace, id: &str) -> Form {
    ws.open_form(Some(key(id)));
    assert!(ws.wait_until(WAIT, |ws| ws.forms.form.is_some()));
    ws.forms.form.clone().unwrap()
}

/// Saves `f` and waits for the window to settle.
fn save(ws: &mut Workspace, f: Form) {
    ws.forms.form = Some(f);
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws
                .forms
                .form
                .as_ref()
                .is_some_and(|f| !f.saving && !f.dirty())
    }));
}

#[test]
fn tweaks_are_read_and_a_chosen_one_paints_what_it_modified() {
    let p = Project::new().with_tweaks();
    let east = p.box_face([1, 0, 0]);
    let mut ws = open(&p);
    let part = ws.part("p").unwrap().clone();
    let kinds: Vec<_> = ["d", "m", "r"]
        .iter()
        .map(|f| {
            part.feature(f)
                .unwrap()
                .tweak
                .as_ref()
                .unwrap()
                .kind
                .clone()
        })
        .collect();
    assert_eq!(kinds, ["draft", "move_face", "replace_face"]);
    // The draft: its neutral plane (xy, pulled along +z) and each wall's
    // angle, exact.
    let d = part.feature("d").unwrap().tweak.clone().unwrap();
    let plane = d.neutral.unwrap();
    assert_eq!((plane.origin, plane.normal), ([0.0; 3], [0.0, 0.0, 1.0]));
    assert_eq!(d.pull, Some([0.0, 0.0, 1.0]));
    assert_eq!(
        d.angles.iter().map(|a| a.text()).collect::<Vec<_>>(),
        ["2 deg", "2 deg"]
    );
    assert!(!d.varies());
    // An offset has no line to draw.
    assert!(
        part.feature("m")
            .unwrap()
            .tweak
            .as_ref()
            .unwrap()
            .line
            .is_none()
    );
    // Chosen, the draft paints both walls it drafted, by their names.
    ws.choose(Some(key("d")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model
            .made
            .as_ref()
            .is_some_and(|m| m.feature == key("d"))
    }));
    let made = ws.model.made.clone().unwrap();
    let walls = &made.faces[&key("b")];
    assert_eq!(walls.len(), 2, "{walls:?}");
    assert!(walls.contains(east.rsplit('/').next().unwrap()));
}

#[test]
fn a_drafts_angle_and_a_moves_distance_are_edited_from_their_forms() {
    let p = Project::new().with_tweaks();
    let mut ws = open(&p);
    let mut f = form(&mut ws, "d");
    let Some(TweakForm::Draft(d)) = &mut f.tweak else {
        panic!("a draft's form")
    };
    assert_eq!(
        (d.angle.read.as_str(), d.hinge.as_str()),
        ("2 deg", "neutral xy")
    );
    d.angle.text = "3 deg".into();
    save(&mut ws, f);
    let shown = p.ok(&["feature", "show", "p", "d"]);
    assert_eq!(shown["angle"]["value"], "3 deg", "{shown}");
    // The rebuild's angles follow.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.part("p")
            .and_then(|p| p.feature("d")?.tweak.clone())
            .is_some_and(|t| t.angles.iter().all(|a| a.text() == "3 deg"))
    }));
    let mut m = form(&mut ws, "m");
    let Some(TweakForm::MoveFace(x)) = &mut m.tweak else {
        panic!("a move's form")
    };
    assert_eq!(x.by, "offset");
    x.distance.as_mut().unwrap().text = "2 mm".into();
    x.tangent.chosen = false;
    save(&mut ws, m);
    let shown = p.ok(&["feature", "show", "p", "m"]);
    assert_eq!(shown["distance"]["value"], "2 mm", "{shown}");
    assert_eq!(shown["tangent"], false, "{shown}");
}
