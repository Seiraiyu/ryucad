//! Keymap presets: the default keys of the view and measure commands, one
//! table per `ui.navigation` style (`ryucad`, `solidworks`, `onshape`).
//! Changing the style changes the table. Each command has an ID
//! (`view.front`, `measure.tool`) and a title, so the command registry
//! that comes with phase 11 can read these tables as its named default
//! bindings, and a person's own rebinds override them there. Until then
//! the window runs them itself (`ui::app`) and the menus show them.
//! Nothing here binds a key outside these commands.

use super::nav::Navigation;
use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};
use ryucad::view::StandardView;

/// A command a preset binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    View(StandardView),
    /// Look along the selected planar face's normal.
    NormalTo,
    /// Zoom to fit the model.
    Fit,
    /// The spacebar view selector, at the pointer.
    Selector,
    /// Tools > Measure, on or off.
    Measure,
}

impl Command {
    /// The registry's ID.
    pub fn id(self) -> String {
        match self {
            Command::View(v) => format!("view.{}", v.name()),
            Command::NormalTo => "view.normal_to".into(),
            Command::Fit => "view.fit".into(),
            Command::Selector => "view.selector".into(),
            Command::Measure => "measure.tool".into(),
        }
    }

    /// The title a menu or palette shows.
    pub fn title(self) -> String {
        match self {
            Command::View(StandardView::Iso) => "Isometric".into(),
            Command::View(v) => {
                let mut name = v.name().replace('_', " ");
                if let Some(first) = name.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                name
            }
            Command::NormalTo => "Normal to".into(),
            Command::Fit => "Zoom to fit".into(),
            Command::Selector => "View selector".into(),
            Command::Measure => "Measure".into(),
        }
    }
}

/// One preset entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub command: Command,
    pub shortcut: KeyboardShortcut,
}

const fn bind(command: Command, modifiers: Modifiers, key: Key) -> Binding {
    Binding {
        command,
        shortcut: KeyboardShortcut::new(modifiers, key),
    }
}

const NONE: Modifiers = Modifiers::NONE;
const CMD: Modifiers = Modifiers::COMMAND;
const SHIFT: Modifiers = Modifiers::SHIFT;

/// ryucad's own: FreeCAD's number keys for the standard views (0 iso,
/// 1 front, 2 top, 3 right, 4 back, 5 bottom, 6 left), N normal to.
const RYUCAD: [Binding; 11] = [
    bind(Command::View(StandardView::Iso), NONE, Key::Num0),
    bind(Command::View(StandardView::Front), NONE, Key::Num1),
    bind(Command::View(StandardView::Top), NONE, Key::Num2),
    bind(Command::View(StandardView::Right), NONE, Key::Num3),
    bind(Command::View(StandardView::Back), NONE, Key::Num4),
    bind(Command::View(StandardView::Bottom), NONE, Key::Num5),
    bind(Command::View(StandardView::Left), NONE, Key::Num6),
    bind(Command::NormalTo, NONE, Key::N),
    bind(Command::Fit, NONE, Key::F),
    bind(Command::Selector, NONE, Key::Space),
    bind(Command::Measure, NONE, Key::M),
];

/// SolidWorks's: Ctrl+1 to Ctrl+6 front, back, left, right, top, bottom,
/// Ctrl+7 isometric, Ctrl+8 normal to.
const SOLIDWORKS: [Binding; 11] = [
    bind(Command::View(StandardView::Front), CMD, Key::Num1),
    bind(Command::View(StandardView::Back), CMD, Key::Num2),
    bind(Command::View(StandardView::Left), CMD, Key::Num3),
    bind(Command::View(StandardView::Right), CMD, Key::Num4),
    bind(Command::View(StandardView::Top), CMD, Key::Num5),
    bind(Command::View(StandardView::Bottom), CMD, Key::Num6),
    bind(Command::View(StandardView::Iso), CMD, Key::Num7),
    bind(Command::NormalTo, CMD, Key::Num8),
    bind(Command::Fit, NONE, Key::F),
    bind(Command::Selector, NONE, Key::Space),
    bind(Command::Measure, NONE, Key::M),
];

/// Onshape's: Shift+1 to Shift+7 front, back, left, right, top, bottom,
/// isometric, N normal to.
const ONSHAPE: [Binding; 11] = [
    bind(Command::View(StandardView::Front), SHIFT, Key::Num1),
    bind(Command::View(StandardView::Back), SHIFT, Key::Num2),
    bind(Command::View(StandardView::Left), SHIFT, Key::Num3),
    bind(Command::View(StandardView::Right), SHIFT, Key::Num4),
    bind(Command::View(StandardView::Top), SHIFT, Key::Num5),
    bind(Command::View(StandardView::Bottom), SHIFT, Key::Num6),
    bind(Command::View(StandardView::Iso), SHIFT, Key::Num7),
    bind(Command::NormalTo, NONE, Key::N),
    bind(Command::Fit, NONE, Key::F),
    bind(Command::Selector, NONE, Key::Space),
    bind(Command::Measure, NONE, Key::M),
];

/// The preset of a navigation style.
pub fn preset(style: Navigation) -> &'static [Binding] {
    match style {
        Navigation::Ryucad => &RYUCAD,
        Navigation::Solidworks => &SOLIDWORKS,
        Navigation::Onshape => &ONSHAPE,
    }
}

/// The key a style's preset gives `command`, if any.
pub fn shortcut(style: Navigation, command: Command) -> Option<KeyboardShortcut> {
    preset(style)
        .iter()
        .find(|b| b.command == command)
        .map(|b| b.shortcut)
}

/// Takes a press of `s` out of this frame's input: by the key it types,
/// or by where it sits on the keyboard (Shift+1 types `!` on most
/// layouts, which no shortcut on 1 would otherwise see).
pub fn consume(i: &mut egui::InputState, s: &KeyboardShortcut) -> bool {
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

    #[test]
    fn every_preset_binds_its_keys_once() {
        for style in [
            Navigation::Ryucad,
            Navigation::Solidworks,
            Navigation::Onshape,
        ] {
            let p = preset(style);
            for (i, a) in p.iter().enumerate() {
                for b in &p[i + 1..] {
                    assert_ne!(a.shortcut, b.shortcut, "{style:?}: {a:?} and {b:?}");
                    assert_ne!(a.command, b.command, "{style:?}");
                }
                // None takes the window's own keys.
                for own in [
                    crate::ui::menu::UNDO,
                    crate::ui::menu::REDO,
                    crate::ui::menu::REDO_Y,
                    crate::ui::menu::CLOSE,
                    crate::ui::menu::CLEAR_SELECTION,
                ] {
                    assert_ne!(a.shortcut, own, "{style:?}");
                }
            }
            // Each has the six faces, iso, normal to, fit, the selector and
            // measure.
            for c in [
                Command::View(StandardView::Front),
                Command::View(StandardView::Back),
                Command::View(StandardView::Left),
                Command::View(StandardView::Right),
                Command::View(StandardView::Top),
                Command::View(StandardView::Bottom),
                Command::View(StandardView::Iso),
                Command::NormalTo,
                Command::Fit,
                Command::Selector,
                Command::Measure,
            ] {
                assert!(shortcut(style, c).is_some(), "{style:?} {c:?}");
            }
            assert_eq!(
                shortcut(style, Command::Selector),
                Some(KeyboardShortcut::new(NONE, Key::Space))
            );
        }
        assert_eq!(
            shortcut(Navigation::Solidworks, Command::View(StandardView::Iso)),
            Some(KeyboardShortcut::new(CMD, Key::Num7))
        );
        assert_eq!(
            shortcut(Navigation::Solidworks, Command::NormalTo),
            Some(KeyboardShortcut::new(CMD, Key::Num8))
        );
        assert_eq!(
            shortcut(Navigation::Ryucad, Command::View(StandardView::Front)),
            Some(KeyboardShortcut::new(NONE, Key::Num1))
        );
    }

    #[test]
    fn commands_have_registry_ids_and_titles() {
        assert_eq!(Command::View(StandardView::Front).id(), "view.front");
        assert_eq!(
            Command::View(StandardView::named("front_top_right").unwrap()).id(),
            "view.front_top_right"
        );
        assert_eq!(Command::Selector.id(), "view.selector");
        assert_eq!(Command::Measure.id(), "measure.tool");
        assert_eq!(Command::View(StandardView::Iso).title(), "Isometric");
        assert_eq!(Command::View(StandardView::Dimetric).title(), "Dimetric");
        assert_eq!(Command::NormalTo.title(), "Normal to");
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
            found = ctx.input_mut(|i| consume(i, &KeyboardShortcut::new(SHIFT, Key::Num1)));
        });
        out.textures_delta.clear();
        assert!(found);
    }
}
