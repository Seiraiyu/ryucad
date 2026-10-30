//! Editing a feature from the property panel's form, against a real
//! daemon: the same `feature.edit` an agent sends, guarded the same way.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open(p: &Project) -> Workspace {
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && ws.model.meshes.len() == 2));
    ws.open_form(Some(("p".into(), "boss".into())));
    assert!(ws.wait_until(WAIT, |ws| ws.form.is_some()));
    ws
}

fn radius(ws: &mut Workspace) -> &mut ryucad_app::core::edit::Field {
    let form = ws.form.as_mut().unwrap();
    form.params.iter_mut().find(|f| f.name == "radius").unwrap()
}

fn radius_on_disk(p: &Project) -> String {
    p.ok(&["feature", "show", "p", "boss"])["params"]["radius"]["value"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_save_is_a_feature_edit() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    assert_eq!(radius(&mut ws).read, "3 mm");
    radius(&mut ws).text = "0.2 in".into();
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form.as_ref().is_some_and(|f| !f.saving && !f.dirty())
    }));
    assert_eq!(radius_on_disk(&p), "0.2 in");
    assert_eq!(radius(&mut ws).read, "0.2 in");
    // The window's edit is in the activity log, as this window's, and an
    // agent can undo it.
    let log = p.ok(&["activity"]);
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["method"], "feature.edit");
    assert_eq!(last["participant"]["id"], ws.me());
    p.ok(&["undo", "--entry", &last["seq"].to_string()]);
    assert_eq!(radius_on_disk(&p), "3 mm");
}

#[test]
fn a_stale_save_conflicts_and_shows_what_changed() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    radius(&mut ws).text = "4 mm".into();
    // An agent edits the same feature first.
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=5"]);
    ws.save_form();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form.as_ref().is_some_and(|f| !f.changed.is_empty())
    }));
    let form = ws.form.as_ref().unwrap();
    assert_eq!(form.changed[0].name, "radius");
    assert_eq!(
        (
            form.changed[0].theirs.as_str(),
            form.changed[0].yours.as_str()
        ),
        ("5 mm", "4 mm")
    );
    // Nothing was overwritten.
    assert_eq!(radius_on_disk(&p), "5 mm");
    assert_eq!(radius(&mut ws).text, "5 mm");
}

#[test]
fn a_bad_value_keeps_what_was_typed() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    radius(&mut ws).text = "3 parsecs".into();
    ws.save_form();
    // The refusal lands on the field it names.
    assert!(ws.wait_until(WAIT, |ws| {
        ws.form
            .as_ref()
            .is_some_and(|f| f.params.iter().any(|x| x.error.is_some()))
    }));
    let form = ws.form.as_ref().unwrap();
    assert_eq!(form.error, None);
    assert!(form.dirty());
    assert!(radius(&mut ws).error.is_some());
    assert_eq!(radius(&mut ws).text, "3 parsecs");
}

#[test]
fn an_untouched_form_follows_the_files() {
    let p = Project::new().with_part();
    let mut ws = open(&p);
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=6"]);
    assert!(ws.wait_until(WAIT, |ws| ws.form.as_ref().is_some_and(|f| {
        f.params
            .iter()
            .any(|f| f.name == "radius" && f.read == "6 mm")
    })));
}
