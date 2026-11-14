use super::route::Effect;
use super::*;
use std::collections::BTreeSet;

/// A body of an inspect answer: faces by name and origin, edges by the
/// faces they bound.
fn body(solid: &str, faces: &[(&str, &str)], edges: &[(&str, [&str; 2])]) -> Value {
    json!({
        "solid": solid,
        "faces": faces.iter().map(|(n, f)| json!({"name": n, "origin": {"feature": f}}))
            .collect::<Vec<_>>(),
        "edges": edges.iter().map(|(n, fs)| json!({"name": n, "faces": fs}))
            .collect::<Vec<_>>(),
    })
}

#[test]
fn a_shells_rims_are_the_edges_between_made_and_unmade_faces() {
    let key = ("p".to_owned(), "sh".to_owned());
    // An open shell: walls `w1`, `w2` and floor `fl` the shell made, the
    // outside `o1`, `o2` the box's, the opening's edges `r1`, `r2`
    // between them; `e1` joins two made faces, `e2` two unmade ones.
    let open = body(
        "b",
        &[
            ("w1", "sh"),
            ("w2", "sh"),
            ("fl", "sh"),
            ("o1", "b"),
            ("o2", "b"),
        ],
        &[
            ("r1", ["w1", "o1"]),
            ("r2", ["o2", "w2"]),
            ("e1", ["w1", "fl"]),
            ("e2", ["o1", "o2"]),
        ],
    );
    // A closed shell: its cavity's faces meet only each other.
    let cavity = body(
        "c",
        &[("i1", "sh"), ("i2", "sh"), ("o3", "c")],
        &[("e3", ["i1", "i2"]), ("e4", ["o3", "o3"])],
    );
    // A body the shell made nothing of.
    let other = body("d", &[("x", "d")], &[("e5", ["x", "x"])]);
    let v = json!({"bodies": [open, cavity, other]});
    let m = Made::from_inspect(&key, "r1", &v);
    let names = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<BTreeSet<_>>();
    let b = ("p".to_owned(), "b".to_owned());
    let c = ("p".to_owned(), "c".to_owned());
    assert_eq!(m.faces[&b], names(&["w1", "w2", "fl"]));
    assert_eq!(m.faces[&c], names(&["i1", "i2"]));
    assert!(!m.faces.contains_key(&("p".to_owned(), "d".to_owned())));
    assert_eq!(m.rims[&b], names(&["r1", "r2"]));
    // The cavity has no opening, so no rim.
    assert!(!m.rims.contains_key(&c));
    assert_eq!(m.rims.len(), 1);
    assert_eq!((m.feature, m.revision.as_str()), (key, "r1"));
    assert!(m.lines.is_empty() && m.walls.is_empty());
}

#[test]
fn a_fills_sides_a_sews_seams_and_a_thickens_walls_are_found_by_origin() {
    let names = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<BTreeSet<_>>();
    let edge = |n: &str, origin: Value| json!({"name": n, "faces": ["a"], "origin": origin});
    // A fill: its patch and its sides, laminar, from its own origins.
    let fill = json!({"bodies": [{"solid": "f",
        "faces": [{"name": "a", "origin": {"feature": "f", "fill": "patch"}}],
        "edges": [edge("s1", json!({"feature": "f", "side": 0})),
                  edge("s2", json!({"feature": "f", "side": 1, "piece": 0})),
                  edge("i1", json!({"feature": "f"}))]}]});
    let m = Made::from_inspect(&("p".into(), "f".into()), "r", &fill);
    let f = ("p".to_owned(), "f".to_owned());
    assert_eq!(m.faces[&f], names(&["a"]));
    assert_eq!(m.lines[&f], names(&["s1", "s2"]));
    // A sew: the sheets' faces keep their own origins, only the edges
    // it sewed are its.
    let sew = json!({"bodies": [{"solid": "sw",
        "faces": [{"name": "a", "origin": {"feature": "top"}}],
        "edges": [edge("e1", json!({"feature": "sw", "sewn": ["x", "y"]})),
                  edge("e2", json!({"feature": "top"}))]}]});
    let m = Made::from_inspect(&("p".into(), "sw".into()), "r", &sew);
    let sw = ("p".to_owned(), "sw".to_owned());
    assert!(m.faces.is_empty());
    assert_eq!(m.lines[&sw], names(&["e1"]));
    // A thicken: its walls apart from its sides' faces.
    let thicken = json!({"bodies": [{"solid": "t",
        "faces": [{"name": "w", "origin": {"feature": "t", "wall_of": ["e"]}},
                  {"name": "s", "origin": {"feature": "t", "front_of": "a"}}],
        "edges": []}]});
    let m = Made::from_inspect(&("p".into(), "t".into()), "r", &thicken);
    let t = ("p".to_owned(), "t".to_owned());
    assert_eq!(m.faces[&t], names(&["w", "s"]));
    assert_eq!(m.walls[&t], names(&["w"]));
}

#[test]
fn a_tweaks_made_faces_are_the_faces_it_modified() {
    let key = ("p".to_owned(), "m".to_owned());
    // The tweak's own inspect: every face keeps its origin, the box's.
    let v = json!({"bodies": [{"feature": "m", "solid": "b",
        "faces": [{"name": "t", "origin": {"feature": "b"}},
                  {"name": "s", "origin": {"feature": "b"}}],
        "edges": [{"name": "e", "faces": ["t", "s"]}]}],
        "move_face": {"modified": {"faces": 1, "edges": 1, "vertices": 0,
                                   "references": ["p/b/e", "p/b/t"]}}});
    let m = Made::from_inspect(&key, "r1", &v);
    let b = ("p".to_owned(), "b".to_owned());
    assert_eq!(m.faces[&b], BTreeSet::from(["t".to_owned()]));
    assert!(m.rims.is_empty());
}

#[test]
fn a_deletes_made_faces_are_the_hosts_it_healed_and_what_it_made() {
    let key = ("p".to_owned(), "x".to_owned());
    let v = json!({"bodies": [{"feature": "x", "solid": "b",
        "faces": [{"name": "t", "origin": {"feature": "b"}},
                  {"name": "c", "origin": {"feature": "x"}},
                  {"name": "s", "origin": {"feature": "b"}}],
        "edges": []}],
        "delete_face": {
            "modified": {"faces": 1, "edges": 0, "vertices": 0, "references": ["p/b/t"]},
            "created": {"faces": 1, "edges": 0, "vertices": 0, "references": ["p/b/c"]},
            "deleted": {"faces": 2, "edges": 0, "vertices": 0,
                        "references": ["p/b/h1", "p/b/h2"]}}});
    let m = Made::from_inspect(&key, "r1", &v);
    let b = ("p".to_owned(), "b".to_owned());
    assert_eq!(
        m.faces[&b],
        BTreeSet::from(["c".to_owned(), "t".to_owned()])
    );
    assert!(m.rims.is_empty() && m.lines.is_empty() && m.walls.is_empty());
}

/// An answer as the link gives one.
fn answered(result: Value) -> Result<Answer> {
    Ok(Answer {
        result,
        binary: None,
        raw: None,
    })
}

/// Design 541's typed handles: a form answer opens the form it was asked
/// for, in the forms sub-state, with nothing left for the workspace.
#[test]
fn a_form_answer_reaches_the_forms_that_asked() {
    let mut forms = forms::Forms {
        form_for: Some(("p".to_owned(), "f1".to_owned())),
        ..Default::default()
    };
    let show = json!({"id": "f1", "type": "box", "revision": "1", "params": {"width": "10 mm"}});
    let mut fx = Effects::default();
    forms.answer(
        forms::Asked::Form("p".to_owned(), "f1".to_owned(), None),
        answered(show),
        &Model::default(),
        &crate::core::pdm::States::default(),
        &mut fx,
    );
    let f = forms.form.expect("the form asked for opens");
    assert_eq!(
        (f.part.as_str(), f.feature.as_str(), f.ty.as_str()),
        ("p", "f1", "box")
    );
    assert!(fx.recorded().is_empty(), "{:?}", fx.recorded());
}

/// A part's report asked at a revision the window no longer draws is
/// dropped, and the part asked for again at the one it draws: the model
/// records the ask, the workspace sends it.
#[test]
fn a_stale_rebuild_answer_asks_the_part_again() {
    let mut model = Model::default();
    model.loading.insert("p".to_owned());
    let mut fx = Effects::default();
    model.answer(
        model::Asked::Rebuild("p".to_owned(), Some("draft:ann".to_owned())),
        answered(json!({"parts": [{"part": "p"}]})),
        Some(None),
        &mut fx,
    );
    assert!(!model.loading.contains("p"));
    assert!(
        matches!(fx.recorded(), [Effect::Then(Then::AskPart(p))] if p == "p"),
        "{:?}",
        fx.recorded()
    );
}

/// The doctor's report, answered while another was wanted, asks again
/// once taken: the session records it, as `answered` did after `answer`.
#[test]
fn a_doctor_answer_wanted_again_asks_again() {
    let mut session = session::Session {
        doctor_waiting: true,
        doctor_again: true,
        ..Default::default()
    };
    let mut fx = Effects::default();
    session.answer(
        session::Asked::Doctor,
        answered(json!({"findings": []})),
        "me",
        &mut fx,
    );
    assert!(!session.doctor_waiting && !session.doctor_again);
    assert!(
        matches!(fx.recorded(), [Effect::Then(Then::Diagnose)]),
        "{:?}",
        fx.recorded()
    );
}

/// Design 541: an answer for what a sub-state no longer shows is dropped,
/// and leaves the workspace nothing to do.
#[test]
fn an_answer_for_a_closed_sub_state_is_dropped() {
    // The panel moved on to another feature before the answer came.
    let mut forms = forms::Forms {
        form_for: Some(("p".to_owned(), "f2".to_owned())),
        ..Default::default()
    };
    let show = json!({"id": "f1", "type": "box", "revision": "1"});
    let mut fx = Effects::default();
    forms.answer(
        forms::Asked::Form("p".to_owned(), "f1".to_owned(), None),
        answered(show),
        &Model::default(),
        &crate::core::pdm::States::default(),
        &mut fx,
    );
    assert!(forms.form.is_none());
    assert!(fx.recorded().is_empty(), "{:?}", fx.recorded());
    // The parameter panel closed: its part's list is dropped.
    let mut parameters = parameters::Parameters::default();
    let mut fx = Effects::default();
    parameters.answer(
        parameters::Asked::Params("p".to_owned(), None),
        answered(json!({"parameters": []})),
        "mm",
        &mut fx,
    );
    assert!(parameters.params.is_none());
    assert!(fx.recorded().is_empty(), "{:?}", fx.recorded());
}

/// A failure no arm of a sub-state takes is the workspace's to show.
#[test]
fn a_failure_no_sub_state_takes_is_left_to_the_workspace() {
    let mut pdm = pdm::Pdm::default();
    let mut fx = Effects::default();
    pdm.answer(
        pdm::Asked::States,
        Err(Error::new(Code::Daemon, "gone")),
        &mut fx,
    );
    assert!(
        matches!(fx.recorded(), [Effect::Fail(e)] if e.message == "gone"),
        "{:?}",
        fx.recorded()
    );
}

/// Design 541: `Workspace::answer` only routes. Every arm but a
/// proposal's and a quiet request's hands the answer to the sub-state
/// that asked and does nothing else.
#[test]
fn the_workspace_answers_only_by_routing() {
    let src = include_str!("mod.rs");
    let start = src
        .find("    fn answer(&mut self, why: Route, answer: Result<Answer>)")
        .expect("Workspace::answer");
    let body = &src[start..];
    let body = &body[..body.find("\n    }\n").expect("its end")];
    let mut arms: Vec<String> = Vec::new();
    for line in body.lines() {
        // The match ends where `apply` follows it.
        if line == "        }" {
            break;
        }
        if line.starts_with("            Route::") {
            arms.push(line.trim().to_owned());
        } else if let Some(arm) = arms.last_mut() {
            arm.push_str(line.trim());
        }
    }
    assert_eq!(arms.len(), 16, "{arms:#?}");
    for arm in arms {
        if arm.starts_with("Route::Proposal") || arm.starts_with("Route::Quiet") {
            continue;
        }
        let compact: String = arm.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            compact.contains(".answer(a,answer,") && !compact.contains(';'),
            "an arm of Workspace::answer that does more than route: {arm}"
        );
    }
}
