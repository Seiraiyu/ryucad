//! Sheet metal in the window against a real daemon (design 346, phase 7):
//! the tree's rows, the Part tile's bend table, a chosen unfold painting
//! its bend lines, the flat view laying the solid flat with its bend
//! lines and notes, and a flange's and a base flange's forms editing as
//! an agent's `feature.edit` would.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::edit::{Form, SheetMetalForm};
use ryucad_app::core::sheet::Shown;
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

#[test]
fn a_sheet_metal_part_is_listed_tabled_painted_and_laid_flat() {
    let p = Project::new().with_sheet_metal();
    let mut ws = open(&p);
    let part = ws.part("p").unwrap().clone();
    let kinds: Vec<_> = ["plate", "f", "u"]
        .iter()
        .map(|f| {
            part.feature(f)
                .unwrap()
                .sheet
                .as_ref()
                .unwrap()
                .kind
                .clone()
        })
        .collect();
    assert_eq!(kinds, ["base_flange", "flange", "unfold"]);
    // The Part tile's table, of the solid the base flange founds: shown
    // flat, its one bend unfolded, so no bend left to list.
    let solid = ws.sheet_solid("p", None).unwrap();
    assert_eq!(solid, key("plate"));
    ws.show_sheet(&solid);
    let revision = part.revision.clone();
    assert!(ws.wait_until(WAIT, |ws| ws.sheets.table(&solid, &revision).is_some()));
    let Some(Shown::Table(t)) = ws.sheets.table(&solid, &revision).cloned() else {
        panic!("a table")
    };
    assert_eq!((t.state.as_str(), t.thickness_m), ("flat", 0.0015));
    assert_eq!(t.allowance.as_deref(), Some("K 0.44"));
    assert_eq!(t.unfolded.len(), 1);
    // A chosen unfold paints its bend line's edges, the flat in force
    // being its own.
    ws.choose(Some(key("u")));
    assert!(ws.wait_until(WAIT, |ws| !ws.sheet_paint(&key("u")).1.is_empty()));
    let (faces, edges) = ws.sheet_paint(&key("u"));
    assert!(faces.is_empty());
    assert!(edges.iter().all(|e| e.starts_with("p/plate/")), "{edges:?}");
    // The flat view: the solid's flat body in place of its mesh, its one
    // bend line with its note.
    let candidate = ws.flat_candidate(None).unwrap();
    assert_eq!(candidate, key(""));
    ws.show_flat_view(Some(candidate));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model.flat.is_some() && ws.flat_view.as_ref().is_some_and(|v| v.lines.is_some())
    }));
    let (k, body) = ws.model.flat.clone().unwrap();
    assert_eq!(k, key("plate"));
    assert_eq!(ws.model.mesh_of(&k).unwrap().hash, body.hash);
    let v = ws.flat_view.clone().unwrap();
    assert_eq!(v.solid, "plate");
    let lines = v.lines.unwrap();
    assert_eq!(
        (lines.from.as_str(), lines.unfold.as_deref()),
        ("unfold", Some("u"))
    );
    assert_eq!(lines.lines.len(), 1);
    assert!(
        lines.lines[0].note.ends_with("K0.44"),
        "{}",
        lines.lines[0].note
    );
    // Off again: the folded body's own mesh.
    ws.show_flat_view(None);
    assert!(ws.model.flat.is_none());
}

#[test]
fn a_flat_view_refused_at_a_new_revision_draws_the_folded_body_again() {
    let p = Project::new().with_sheet_metal();
    let mut ws = open(&p);
    let candidate = ws.flat_candidate(None).unwrap();
    ws.show_flat_view(Some(candidate));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model.flat.is_some() && ws.flat_view.as_ref().is_some_and(|v| v.lines.is_some())
    }));
    let (k, flat) = ws.model.flat.clone().unwrap();
    // A deduction longer than the bend: the solid lays flat no more, so
    // the view's re-read at the new revision is refused.
    p.ok(&["feature", "edit", "p", "plate", "--deduction", "100mm"]);
    assert!(ws.wait_until(WAIT, |ws| {
        ws.flat_view.as_ref().is_some_and(|v| v.error.is_some())
    }));
    // The stale flat is gone, body and bend lines, and the folded body is
    // drawn in its place.
    let v = ws.flat_view.clone().unwrap();
    assert!(v.lines.is_none(), "{:?}", v.lines);
    assert!(ws.model.flat.is_none());
    assert!(ws.wait_until(WAIT, |ws| {
        ws.model.mesh_of(&k).is_some_and(|m| m.hash != flat.hash)
    }));
}

#[test]
fn a_flanges_angle_and_a_base_flanges_k_factor_are_edited_from_their_forms() {
    let p = Project::new().with_sheet_metal();
    let mut ws = open(&p);
    let mut f = form(&mut ws, "f");
    let Some(SheetMetalForm::Flange(x)) = &mut f.sheet else {
        panic!("a flange's form")
    };
    assert!(x.inherits("radius"));
    assert_eq!(x.angle.read, "90 deg");
    x.angle.text = "100 deg".into();
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
    let shown = p.ok(&["feature", "show", "p", "f"]);
    assert_eq!(shown["angle"]["value"], "100 deg", "{shown}");
    let mut b = form(&mut ws, "plate");
    let Some(SheetMetalForm::BaseFlange(x)) = &mut b.sheet else {
        panic!("a base flange's form")
    };
    x.allowance.value.text = "0.5".into();
    ws.forms.form = Some(b);
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws
                .forms
                .form
                .as_ref()
                .is_some_and(|f| !f.saving && !f.dirty())
    }));
    let shown = p.ok(&["feature", "show", "p", "plate"]);
    assert_eq!(shown["allowance"]["k_factor"]["value"], "0.5", "{shown}");
}
