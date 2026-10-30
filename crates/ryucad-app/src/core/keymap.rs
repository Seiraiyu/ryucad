//! Keymap presets: the default keys of the view and measure commands, one
//! table per `ui.navigation` style (`ryucad`, `solidworks`, `onshape`),
//! by command id. The registry (`core::command`) gives every other
//! command its keys; a preset's table replaces those of the commands it
//! names, and `ui.keys` replaces both ([`crate::core::keys::Keymap`]).
//! Changing the style changes the table. Nothing here binds a key outside
//! these commands.

use super::nav::Navigation;
use eframe::egui;

/// One preset entry: a command id and its chord.
pub type Binding = (&'static str, &'static str);

/// ryucad's own: FreeCAD's number keys for the standard views (0 iso,
/// 1 front, 2 top, 3 right, 4 back, 5 bottom, 6 left), N normal to.
const RYUCAD: [Binding; 11] = [
    ("view.front_top_right", "0"),
    ("view.front", "1"),
    ("view.top", "2"),
    ("view.right", "3"),
    ("view.back", "4"),
    ("view.bottom", "5"),
    ("view.left", "6"),
    ("view.normal_to", "N"),
    ("view.fit", "F"),
    ("view.selector", "Space"),
    ("measure.tool", "M"),
];

/// SolidWorks's: Ctrl+1 to Ctrl+6 front, back, left, right, top, bottom,
/// Ctrl+7 isometric, Ctrl+8 normal to.
const SOLIDWORKS: [Binding; 11] = [
    ("view.front", "Mod+1"),
    ("view.back", "Mod+2"),
    ("view.left", "Mod+3"),
    ("view.right", "Mod+4"),
    ("view.top", "Mod+5"),
    ("view.bottom", "Mod+6"),
    ("view.front_top_right", "Mod+7"),
    ("view.normal_to", "Mod+8"),
    ("view.fit", "F"),
    ("view.selector", "Space"),
    ("measure.tool", "M"),
];

/// Onshape's: Shift+1 to Shift+7 front, back, left, right, top, bottom,
/// isometric, N normal to.
const ONSHAPE: [Binding; 11] = [
    ("view.front", "Shift+1"),
    ("view.back", "Shift+2"),
    ("view.left", "Shift+3"),
    ("view.right", "Shift+4"),
    ("view.top", "Shift+5"),
    ("view.bottom", "Shift+6"),
    ("view.front_top_right", "Shift+7"),
    ("view.normal_to", "N"),
    ("view.fit", "F"),
    ("view.selector", "Space"),
    ("measure.tool", "M"),
];

/// The preset of a navigation style.
pub fn preset(style: Navigation) -> &'static [Binding] {
    match style {
        Navigation::Ryucad => &RYUCAD,
        Navigation::Solidworks => &SOLIDWORKS,
        Navigation::Onshape => &ONSHAPE,
    }
}

/// Takes a press of `s` out of this frame's input: by the key it types,
/// or by where it sits on the keyboard (Shift+1 types `!` on most
/// layouts, which no shortcut on 1 would otherwise see).
pub fn consume(i: &mut egui::InputState, s: &egui::KeyboardShortcut) -> bool {
    if i.consume_shortcut(s) {
        return true;
    }
    let mut found = false;
    i.events.retain(|e| match e {
        egui::Event::Key {
            physical_key: Some(k),
            pressed: true,
            modifiers,
            ..
        } if !found && *k == s.logical_key && modifiers.matches_logically(s.modifiers) => {
            found = true;
            false
        }
        _ => true,
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keys::{Chord, Keymap};
    use eframe::egui::{Key, KeyboardShortcut, Modifiers};

    const STYLES: [Navigation; 3] = [
        Navigation::Ryucad,
        Navigation::Solidworks,
        Navigation::Onshape,
    ];

    #[test]
    fn every_preset_names_registry_commands_once_and_conflicts_with_nothing() {
        for style in STYLES {
            let p = preset(style);
            for (i, (id, chord)) in p.iter().enumerate() {
                assert!(crate::core::command::get(id).is_some(), "{style:?} {id}");
                assert!(Chord::parse(chord).is_some(), "{style:?} {chord}");
                assert!(
                    p[i + 1..].iter().all(|(other, _)| other != id),
                    "{style:?} {id}"
                );
            }
            // With the registry's own keys, on both kinds of OS.
            for mac in [false, true] {
                let k = Keymap::new(style, &serde_json::json!({}));
                assert_eq!(
                    k.conflicts(mac),
                    Vec::<(&str, &str, Chord)>::new(),
                    "{style:?}"
                );
            }
        }
    }

    #[test]
    fn the_presets_keys_are_as_the_styles_have_them() {
        let k = |style, id| {
            Keymap::new(style, &serde_json::json!({}))
                .chords(id)
                .to_vec()
        };
        assert_eq!(k(Navigation::Ryucad, "view.front"), [Chord::key(Key::Num1)]);
        assert_eq!(
            k(Navigation::Solidworks, "view.front_top_right"),
            [Chord::command(Key::Num7)]
        );
        assert_eq!(
            k(Navigation::Solidworks, "view.normal_to"),
            [Chord::command(Key::Num8)]
        );
        assert_eq!(
            k(Navigation::Onshape, "view.back"),
            [Chord::key(Key::Num2).shift()]
        );
        for style in STYLES {
            assert_eq!(k(style, "view.selector"), [Chord::key(Key::Space)]);
            assert_eq!(k(style, "measure.tool"), [Chord::key(Key::M)]);
        }
    }

    #[test]
    fn shift_and_a_digit_is_found_by_where_the_key_is() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: Key::Exclamationmark,
            physical_key: Some(Key::Num1),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::SHIFT,
        });
        let mut found = false;
        let mut out = ctx.run_pass(input, |ctx| {
            found =
                ctx.input_mut(|i| consume(i, &KeyboardShortcut::new(Modifiers::SHIFT, Key::Num1)));
        });
        out.textures_delta.clear();
        assert!(found);
    }
}
