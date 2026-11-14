//! Chords and the keymap are `ryu_app::keys`' (ryueda 3.C). ryucad's
//! keymap is its registry ([`crate::core::command::COMMANDS`]), then the
//! navigation style's preset ([`crate::core::keymap`]), then `ui.keys`.

pub use ryu_app::keys::{Chord, Conflict, captured, is_modifier};

/// ryucad's effective keymap.
pub type Keymap = ryu_app::keys::Keymap<crate::core::product::Ryucad>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::nav::Navigation;
    use eframe::egui::Key;

    #[test]
    fn defaults_then_the_preset_then_ui_keys() {
        use serde_json::json;
        let k = Keymap::new(Navigation::Ryucad, &json!({}));
        assert_eq!(k.chords("edit.undo"), [Chord::command(Key::Z)]);
        assert_eq!(k.chords("view.front"), [Chord::key(Key::Num1)]);
        assert!(!k.overridden("edit.undo"));
        let user = json!({
            "edit.undo": ["Alt+Backspace"],
            "view.front": [],
            "stage12.thing": ["Mod+J"],
            "view.fit": ["Mod+Zed", "G"],
        });
        let k = Keymap::new(Navigation::Ryucad, &user);
        assert_eq!(
            k.chords("edit.undo"),
            [Chord::parse("Alt+Backspace").unwrap()]
        );
        assert!(k.chords("view.front").is_empty());
        assert_eq!(k.chords("view.fit"), [Chord::key(Key::G)]);
        assert_eq!(k.notes.len(), 1, "{:?}", k.notes);
        assert!(k.chords("stage12.thing").is_empty());
        // A rebind keeps what this build doesn't know.
        let v = k
            .rebind("edit.redo", &[Chord::parse("Mod+R").unwrap()], false)
            .unwrap();
        assert_eq!(v["stage12.thing"], json!(["Mod+J"]));
        assert_eq!(v["edit.redo"], json!(["Mod+R"]));
        assert_eq!(k.reset("edit.undo").get("edit.undo"), None);
    }

    #[test]
    fn a_chord_another_command_has_is_refused_by_scope() {
        use serde_json::json;
        let k = Keymap::new(Navigation::Ryucad, &json!({}));
        let close = Chord::command(Key::W);
        assert_eq!(
            k.rebind("view.front_top_right", &[close], false),
            Err(Conflict {
                chord: close,
                with: "file.close"
            })
        );
        // Tab is the viewport's: a window command can't take it, and
        // another viewport command can't either.
        let tab = Chord::key(Key::Tab);
        assert_eq!(k.taken("view.fit", &tab, false), Some("view.hide_under"));
        assert_eq!(
            k.taken("view.show_under", &tab, false),
            Some("view.hide_under")
        );
        // Off macOS, Ctrl+Z is Mod+Z.
        let ctrl_z = Chord::parse("Ctrl+Z").unwrap();
        assert_eq!(k.taken("view.fit", &ctrl_z, false), Some("edit.undo"));
        assert_eq!(k.taken("view.fit", &ctrl_z, true), None);
    }

    #[test]
    fn conflicts_name_every_pair_sharing_a_chord() {
        use serde_json::json;
        // Three commands on Mod+W: file.close's default, and two more.
        let user = json!({"view.fit": ["Mod+W"], "view.front": ["Mod+W"]});
        let k = Keymap::new(Navigation::Ryucad, &user);
        let w = Chord::command(Key::W);
        let mut pairs: Vec<[&str; 2]> = k
            .conflicts(false)
            .into_iter()
            .filter(|(_, _, c)| c.same(&w, false))
            .map(|(a, b, _)| {
                let mut p = [a, b];
                p.sort_unstable();
                p
            })
            .collect();
        pairs.sort_unstable();
        assert_eq!(
            pairs,
            [
                ["file.close", "view.fit"],
                ["file.close", "view.front"],
                ["view.fit", "view.front"],
            ]
        );
    }
}
