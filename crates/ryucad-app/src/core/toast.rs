//! Toasts (design 199): what other participants change, the window's own
//! failures, the daemon lost and found again, and a long request of the
//! window's finishing while it isn't focused. At most [`SHOWN`] at once,
//! newest at the bottom; each goes after [`LIFE`] unless hovered, and
//! errors stay until closed. `ui.notify` says which show. They are
//! `ryu_app::toast`'s (ryueda 3.C); here are ryucad's words for a change.
//! Pure: `ui::feedback` draws them.

use serde_json::Value;
use std::collections::BTreeSet;

pub use ryu_app::toast::{Action, BATCH, Kind, LIFE, LOST, Notify, SHOWN, Toast};

/// ryucad's toasts, in its words ([`crate::core::product::Ryucad`]).
pub type Toasts = ryu_app::toast::Toasts<crate::core::product::Ryucad>;

/// An activity method in words: `feature.edit` is `edited`.
pub fn verb(method: &str, kind: &Value) -> String {
    match kind.as_str() {
        Some("undo") => return "undid".into(),
        Some("redo") => return "redid".into(),
        _ => {}
    }
    match method {
        "feature.add" => "added",
        "feature.edit" => "edited",
        "feature.delete" => "deleted",
        "feature.move" => "reordered",
        "part.create" => "created",
        "part.delete" => "deleted",
        "param.set" | "param.delete" | "param.rename" => "set parameters",
        "sketch.edit" => "edited",
        "assembly.move" => "moved",
        "import" => "imported",
        "merge.resolve" => "resolved",
        "appearance.set" | "appearance.clear" => "coloured",
        "view.save" | "view.delete" | "view.rename" => "saved views",
        "doctor.repair" => "repaired",
        "config.set" => "changed settings",
        m => m,
    }
    .to_owned()
}

/// The features an entry's files are: `parts/<part>/features/<f>.json`,
/// as `(part, feature)`.
pub fn features_of(entry: &Value) -> Vec<(String, String)> {
    entry["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let path = f["path"].as_str()?;
            let rest = path.strip_prefix("parts/")?;
            let (part, file) = rest.split_once("/features/")?;
            let feature = file.strip_suffix(".json")?;
            Some((part.to_owned(), feature.to_owned()))
        })
        .collect()
}

/// The parts an entry's files are in: `parts/<part>/…`.
pub fn parts_of(entry: &Value) -> BTreeSet<String> {
    entry["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let rest = f["path"].as_str()?.strip_prefix("parts/")?;
            Some(rest.split_once('/')?.0.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::error::{Code, Error};
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn entry(seq: u64, who: &str, method: &str, files: &[&str]) -> Value {
        let files: Vec<Value> = files.iter().map(|p| json!({"path": p})).collect();
        json!({"seq": seq, "kind": "change", "method": method, "undone_by": null,
               "participant": {"id": who, "kind": "agent", "name": "agent"}, "files": files})
    }

    /// #732: connected again, the lost toast goes, however often it was
    /// lost; the window's own errors stay.
    #[test]
    fn the_lost_toast_goes_once_connected_again() {
        let t0 = Instant::now();
        let mut t = Toasts::default();
        let e = Error::new(Code::UnitMismatch, "length takes a length, not an angle");
        t.error("Unit mismatch", &e, 0, t0);
        for _ in 0..2 {
            t.lost(t0);
            assert_eq!(t.shown().last().unwrap().text, LOST);
            t.found(t0);
            let texts: Vec<&str> = t.shown().iter().map(|t| t.text.as_str()).collect();
            assert!(!texts.contains(&LOST), "{texts:?}");
            assert_eq!(texts.first(), Some(&"Unit mismatch"));
            assert_eq!(texts.last(), Some(&"Connected to the daemon again"));
        }
        let mut quiet = Toasts::with(Notify::Errors);
        quiet.lost(t0);
        quiet.found(t0);
        assert!(quiet.shown().iter().all(|t| t.text != LOST));
    }

    #[test]
    fn an_agent_s_reply_on_my_annotation_is_a_toast() {
        let mut t = Toasts::default();
        let now = Instant::now();
        t.annotation(
            "agent",
            true,
            "replied on #2",
            "Moved it 5 mm.",
            "aaaaaaaaaa",
            now,
        );
        let shown = t.shown();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].who.as_deref(), Some("agent"));
        assert_eq!(shown[0].text, "replied on #2");
        assert_eq!(shown[0].detail.as_deref(), Some("Moved it 5 mm."));
        assert_eq!(
            shown[0].actions,
            vec![Action::Annotation("aaaaaaaaaa".into())]
        );
        // ui.notify = errors keeps them out.
        let mut quiet = Toasts::with(Notify::Errors);
        quiet.annotation("agent", true, "replied on #2", "x", "aaaaaaaaaa", now);
        assert!(quiet.shown().is_empty());
    }

    #[test]
    fn annotation_writes_make_no_change_toast() {
        let mut t = Toasts::default();
        let e = |method| entry(7, "a1", method, &["annotations/aaaaaaaaaa.json"]);
        t.activity(&e("annotation.reply"), "me", Instant::now());
        assert!(t.shown().is_empty());
        t.activity(&e("annotation.apply"), "me", Instant::now());
        assert_eq!(t.shown().len(), 1, "applying writes the model");
    }

    /// #426: a change of no feature names its part.
    #[test]
    fn a_part_created_names_it() {
        let mut t = Toasts::default();
        let e = entry(1, "p-2", "part.create", &["parts/bracket/part.json"]);
        t.activity(&e, "p-1", Instant::now());
        assert_eq!(t.shown()[0].text, "created bracket");
    }

    #[test]
    fn an_agents_changes_within_a_second_are_one_toast_with_show_and_undo() {
        let t0 = Instant::now();
        let mut t = Toasts::default();
        for (i, f) in ["f1", "c1", "e1"].iter().enumerate() {
            let e = entry(
                i as u64 + 1,
                "p-2",
                "feature.edit",
                &[&format!("parts/bracket/features/{f}.json")],
            );
            t.activity(&e, "p-1", t0 + Duration::from_millis(100 * i as u64));
        }
        let shown = t.shown();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].who.as_deref(), Some("agent"));
        assert_eq!(shown[0].text, "edited 3 features in bracket");
        assert_eq!(
            shown[0].actions,
            [
                Action::Show(vec![
                    "bracket/c1".into(),
                    "bracket/e1".into(),
                    "bracket/f1".into()
                ]),
                Action::Undo(vec![3, 2, 1]),
            ]
        );
        // A second later, a new toast.
        let e = entry(
            4,
            "p-2",
            "assembly.move",
            &["parts/frame/features/pin.json"],
        );
        t.activity(&e, "p-1", t0 + Duration::from_secs(2));
        assert_eq!(t.shown()[1].text, "moved pin in frame");
    }

    #[test]
    fn the_windows_own_changes_and_outside_ones_make_no_toast() {
        let mut t = Toasts::default();
        t.activity(&entry(1, "p-1", "feature.edit", &[]), "p-1", Instant::now());
        let outside = json!({"seq": 2, "kind": "external", "files": []});
        t.activity(&outside, "p-1", Instant::now());
        assert!(t.shown().is_empty());
    }

    #[test]
    fn toasts_go_after_their_life_unless_hovered_and_errors_stay() {
        let t0 = Instant::now();
        let mut t = Toasts::default();
        t.note("Connected to the daemon again", false, t0);
        let e = Error::new(Code::UnitMismatch, "length takes a length, not an angle");
        t.error("Unit mismatch", &e, 0, t0);
        let id = t.shown()[0].id;
        t.hover(id, t0 + Duration::from_secs(5));
        t.expire(t0 + LIFE);
        assert_eq!(t.shown().len(), 2);
        t.expire(t0 + Duration::from_secs(5) + LIFE);
        assert_eq!(t.shown().len(), 1);
        assert_eq!(t.shown()[0].kind, Kind::Error);
        assert_eq!(t.due(t0), None);
        let id = t.shown()[0].id;
        t.close(id);
        assert!(t.shown().is_empty());
    }

    #[test]
    fn at_most_four_show_and_notify_chooses_which() {
        let t0 = Instant::now();
        let mut t = Toasts::default();
        for i in 0..6 {
            t.note(&format!("n{i}"), false, t0);
        }
        let texts: Vec<&str> = t.shown().iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["n2", "n3", "n4", "n5"]);
        let mut t = Toasts::with(Notify::Errors);
        t.activity(&entry(1, "p-2", "feature.edit", &[]), "p-1", t0);
        t.note("Connected", false, t0);
        t.note("Lost the daemon", true, t0);
        assert_eq!(t.shown().len(), 1);
        t.notify = Notify::Off;
        t.note("Lost the daemon", true, t0);
        assert_eq!(t.shown().len(), 1);
        assert_eq!(Notify::parse(&json!("off")), Notify::Off);
        assert_eq!(Notify::parse(&json!("loud")), Notify::All);
    }
}
