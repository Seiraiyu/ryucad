//! Surfaces in the window against a real daemon (design 388, phase 9):
//! the model knows which bodies are sheets and how many boundaries they
//! have; a chosen fill's sides, a sew's seams and a thicken's walls are
//! found to paint; the surface toggle, a fill's tolerance and a thicken's
//! front edit as an agent's `feature.edit` would.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::edit::{Form, SurfaceForm};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(120);

fn key(f: &str) -> (String, String) {
    ("p".to_owned(), f.to_owned())
}

/// The window, once the model and every body's mesh have come.
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
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    ws.form.clone().unwrap()
}

/// Saves `f` and waits until it is written.
fn save(ws: &mut Workspace, f: Form) {
    ws.form = Some(f);
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()
        && ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())));
}

#[test]
fn sheets_are_known_and_what_surfaces_made_is_found() {
    let p = Project::new().with_surfaces();
    let mut ws = open(&p);
    let part = ws.part("p").unwrap().clone();
    for id in ["w", "f", "sw"] {
        assert!(ws.model.is_sheet(&key(id)), "{id}");
    }
    for id in ["b", "c", "t"] {
        assert!(!ws.model.is_sheet(&key(id)), "{id}");
    }
    assert_eq!(part.solid("w").unwrap().boundaries, Some(1));
    assert_eq!(part.solid("sw").unwrap().boundaries, Some(0));
    // A new sew and a thicken found their bodies.
    assert!(part.founders().contains(&"sw".to_owned()));
    assert!(part.founders().contains(&"t".to_owned()));
    // A chosen fill: its face, and its four sides to dash.
    ws.choose(Some(key("f")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.made.as_ref().is_some_and(|m| m.feature == key("f"))
    }));
    let made = ws.made.clone().unwrap();
    assert!(!made.faces[&key("f")].is_empty());
    assert_eq!(made.lines[&key("f")].len(), 4, "{made:?}");
    // A chosen sew: no face of its own, the twelve edges it sewed.
    ws.choose(Some(key("sw")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.made.as_ref().is_some_and(|m| m.feature == key("sw"))
    }));
    let made = ws.made.clone().unwrap();
    assert!(made.faces.is_empty(), "{made:?}");
    assert_eq!(made.lines[&key("sw")].len(), 12, "{made:?}");
    // A chosen thicken: its four walls among the faces it made (its front
    // and walls; its back, at zero, keeps the sheet's face and name).
    ws.choose(Some(key("t")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.made.as_ref().is_some_and(|m| m.feature == key("t"))
    }));
    let made = ws.made.clone().unwrap();
    assert_eq!(made.walls[&key("t")].len(), 4, "{made:?}");
    assert_eq!(made.faces[&key("t")].len(), 5, "{made:?}");
}

#[test]
fn a_fills_tolerance_a_thickens_front_and_a_walls_surface_are_edited_from_their_forms() {
    let p = Project::new().with_surfaces();
    let mut ws = open(&p);
    let mut f = form(&mut ws, "f");
    let Some(SurfaceForm::Fill(x)) = &mut f.surface_form else {
        panic!("a fill's form")
    };
    assert_eq!(x.sides.len(), 4);
    assert_eq!(x.tolerance.read, "0.001 mm");
    x.tolerance.text = "0.002 mm".into();
    save(&mut ws, f);
    let shown = p.ok(&["feature", "show", "p", "f"]);
    assert_eq!(shown["tolerance"]["value"], "0.002 mm", "{shown}");
    let mut t = form(&mut ws, "t");
    let Some(SurfaceForm::Thicken(x)) = &mut t.surface_form else {
        panic!("a thicken's form")
    };
    assert_eq!(
        (x.sheet.as_deref(), x.front.read.as_str()),
        (Some("xt"), "2 mm")
    );
    x.front.text = "3 mm".into();
    save(&mut ws, t);
    let shown = p.ok(&["feature", "show", "p", "t"]);
    assert_eq!(shown["front"]["value"], "3 mm", "{shown}");
    // The wall reads as a surface, its operation new.
    let w = form(&mut ws, "w");
    assert!(w.surface.as_ref().unwrap().read);
    assert!(w.operation.as_ref().is_none_or(|o| o.read == "new"));
}
