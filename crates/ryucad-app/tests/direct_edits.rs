//! Direct edits in the window against a real daemon (design 445, The
//! app): a delete's, blend delete's and resize's reports read, a chosen
//! one painting what it modified and made, and their forms and the
//! tweaks' two checkboxes editing as an agent's `feature.edit` would.

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

fn hex(r: &str) -> String {
    r.rsplit('/').next().unwrap().to_owned()
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
    assert!(ws.wait_until(WAIT, |ws| {
        ws.forms.form.as_ref().is_some_and(|f| f.feature == id)
    }));
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
fn direct_edits_are_read_and_a_chosen_one_paints_what_it_modified_and_made() {
    let p = Project::new().with_direct_edits();
    let top = p.box_face([0, 0, 1]);
    let round = p.faces_of("f", "cylinder")[0].clone();
    let mut ws = open(&p);
    let part = ws.part("p").unwrap().clone();
    let info = |f: &str| {
        part.feature(f)
            .unwrap()
            .tweak
            .clone()
            .unwrap_or_else(|| panic!("{f}"))
    };
    assert_eq!(
        ["r", "db", "x"].map(|f| info(f).kind),
        ["resize_blend", "delete_blend", "delete_face"]
    );
    // The delete: its seed found the bore, which went; the hosts it
    // capped were modified.
    let x = info("x");
    assert_eq!(x.seeds.len(), 1, "{x:?}");
    let bore = x.seeds[0].faces.clone();
    assert!(!bore.is_empty(), "{x:?}");
    assert!(x.deleted.unwrap().faces >= bore.len() as u64, "{x:?}");
    assert!(x.modified.unwrap().faces >= 1, "{x:?}");
    // The blend delete took nothing below 1 mm; the resize remade the
    // fillet at 3 mm.
    assert!(info("db").blends.is_empty());
    let r = info("r");
    assert_eq!(
        r.blends.iter().map(|b| b.text()).collect::<Vec<_>>(),
        ["fillet 4 mm → 3 mm"],
        "{r:?}"
    );
    // Chosen, the delete paints the top it healed, not the bore it took.
    ws.choose(Some(key("x")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.made.as_ref().is_some_and(|m| m.feature == key("x"))
    }));
    let made = ws.made.clone().unwrap();
    let painted = &made.faces[&key("b")];
    assert!(painted.contains(&hex(&top)), "{painted:?}");
    for f in &bore {
        assert!(!painted.contains(&hex(f)), "{f} in {painted:?}");
    }
    // Chosen, the resize paints the fillet it remade.
    ws.choose(Some(key("r")));
    assert!(ws.wait_until(WAIT, |ws| {
        ws.made.as_ref().is_some_and(|m| m.feature == key("r"))
    }));
    let made = ws.made.clone().unwrap();
    assert!(made.faces[&key("b")].contains(&hex(&round)), "{made:?}");
}

#[test]
fn a_deletes_heal_a_blend_deletes_size_and_a_resizes_radius_are_edited_from_their_forms() {
    let p = Project::new().with_direct_edits();
    let mut ws = open(&p);
    let mut f = form(&mut ws, "x");
    let Some(TweakForm::DeleteFace(x)) = &mut f.tweak else {
        panic!("a delete's form")
    };
    assert_eq!(
        (x.heal.read.as_str(), x.seeds.read.len(), x.faces.read.len()),
        ("cap", 1, 0)
    );
    x.heal.chosen = "auto".into();
    save(&mut ws, f);
    let shown = p.ok(&["feature", "show", "p", "x"]);
    assert_eq!(shown["heal"], "auto", "{shown}");
    let mut f = form(&mut ws, "db");
    let Some(TweakForm::DeleteBlend(d)) = &mut f.tweak else {
        panic!("a blend delete's form")
    };
    assert_eq!(
        (d.below.read.as_str(), d.tolerance.read.as_str()),
        ("1 mm", "")
    );
    d.below.text = "0.5 mm".into();
    save(&mut ws, f);
    let shown = p.ok(&["feature", "show", "p", "db"]);
    assert_eq!(shown["below"]["value"], "0.5 mm", "{shown}");
    let mut f = form(&mut ws, "r");
    let Some(TweakForm::ResizeBlend(r)) = &mut f.tweak else {
        panic!("a resize's form")
    };
    let (k, radius) = &mut r.groups[0].sizes[0];
    assert_eq!((k.as_str(), radius.read.as_str()), ("radius", "3 mm"));
    radius.text = "2 mm".into();
    save(&mut ws, f);
    let shown = p.ok(&["feature", "show", "p", "r"]);
    assert_eq!(shown["groups"][0]["radius"]["value"], "2 mm", "{shown}");
    // The rebuild's report follows: the fillet remade at 2 mm.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.part("p")
            .and_then(|p| p.feature("r")?.tweak.clone())
            .is_some_and(|t| t.blends.first().is_some_and(|b| b.size == "2 mm"))
    }));
}

#[test]
fn a_tweaks_topology_and_blends_are_edited_from_its_form() {
    let p = Project::new().with_tweaks();
    let mut ws = open(&p);
    let mut m = form(&mut ws, "m");
    let Some(TweakForm::MoveFace(x)) = &mut m.tweak else {
        panic!("a move's form")
    };
    assert!(!x.retrim.topology.read && !x.retrim.blends.read);
    x.retrim.topology.chosen = true;
    x.retrim.blends.chosen = true;
    save(&mut ws, m);
    let shown = p.ok(&["feature", "show", "p", "m"]);
    assert_eq!(
        (shown["topology"].as_str(), shown["blends"].as_str()),
        (Some("change"), Some("rebuild")),
        "{shown}"
    );
    // The row says so once it has rebuilt.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.part("p")
            .and_then(|p| p.feature("m")?.tweak.clone())
            .is_some_and(|t| t.change && t.rebuild)
    }));
    // Turned back, the file is ryucad 17's again.
    let mut m = form(&mut ws, "m");
    let Some(TweakForm::MoveFace(x)) = &mut m.tweak else {
        panic!("a move's form")
    };
    assert!(x.retrim.topology.read && x.retrim.blends.read);
    x.retrim.topology.chosen = false;
    x.retrim.blends.chosen = false;
    save(&mut ws, m);
    let file = std::fs::read_to_string(p.root.join("parts/p/features/m.json")).unwrap();
    assert!(
        !file.contains("topology") && !file.contains("blends"),
        "{file}"
    );
}
