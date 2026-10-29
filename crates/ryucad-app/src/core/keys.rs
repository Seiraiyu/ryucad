//! Chords (design 199, Command registry > Keymap): modifiers plus a key,
//! written `Mod+Shift+Z`, where `Mod` is Cmd on macOS and Ctrl elsewhere
//! and `Ctrl` is Ctrl everywhere. The syntax is `ryucad::config`'s
//! (`chord_ok`, the `ui.keys` preference's check), so the daemon and the
//! window read chords alike.

use eframe::egui::{Key, KeyboardShortcut, Modifiers};
use ryucad::config::{MODIFIERS, chord_ok};

/// A key and its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    /// Cmd on macOS, Ctrl elsewhere.
    pub command: bool,
    /// Ctrl on every OS (macOS's Control).
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Chord {
    pub const fn key(key: Key) -> Chord {
        Chord {
            command: false,
            ctrl: false,
            alt: false,
            shift: false,
            key,
        }
    }

    pub const fn command(key: Key) -> Chord {
        Chord {
            command: true,
            ..Chord::key(key)
        }
    }

    pub const fn shift(self) -> Chord {
        Chord {
            shift: true,
            ..self
        }
    }

    /// Reads `Mod+Shift+Z`; anything `chord_ok` refuses is `None`.
    pub fn parse(s: &str) -> Option<Chord> {
        if !chord_ok(s) {
            return None;
        }
        let mut parts: Vec<&str> = s.split('+').collect();
        let key = Key::from_name(parts.pop()?)?;
        let has = |m: &str| parts.contains(&m);
        Some(Chord {
            command: has(MODIFIERS[0]),
            ctrl: has(MODIFIERS[1]),
            alt: has(MODIFIERS[2]),
            shift: has(MODIFIERS[3]),
            key,
        })
    }

    /// The text `parse` reads, modifiers in `MODIFIERS`' order.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (on, name) in [self.command, self.ctrl, self.alt, self.shift]
            .into_iter()
            .zip(MODIFIERS)
        {
            if on {
                out.push_str(name);
                out.push('+');
            }
        }
        out.push_str(self.key.name());
        out
    }

    /// Whether it has a modifier other than Shift: only such chords fire
    /// while a text field has the keyboard.
    pub fn modified(&self) -> bool {
        self.command || self.ctrl || self.alt
    }

    /// egui's shortcut for it on this OS (`mac`): `Mod` is egui's
    /// command; `Ctrl` is egui's ctrl on macOS and its command elsewhere,
    /// where the two are one key.
    pub fn shortcut(&self, mac: bool) -> KeyboardShortcut {
        let mut m = Modifiers::NONE;
        if self.command || (self.ctrl && !mac) {
            m = m.plus(Modifiers::COMMAND);
        }
        if self.ctrl && mac {
            m = m.plus(Modifiers::CTRL);
        }
        if self.alt {
            m = m.plus(Modifiers::ALT);
        }
        if self.shift {
            m = m.plus(Modifiers::SHIFT);
        }
        KeyboardShortcut::new(m, self.key)
    }

    /// The chord a key press made, as `Mod` where it can be.
    pub fn pressed(key: Key, m: Modifiers, mac: bool) -> Chord {
        Chord {
            command: m.command,
            ctrl: mac && m.ctrl,
            alt: m.alt,
            shift: m.shift,
            key,
        }
    }

    /// Whether this chord reads as `other` on this OS: off macOS, `Ctrl`
    /// and `Mod` are one key.
    pub fn same(&self, other: &Chord, mac: bool) -> bool {
        self.shortcut(mac) == other.shortcut(mac)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_read_and_print_both_ways() {
        for text in [
            "Mod+K",
            "Mod+Shift+Z",
            "F",
            "1",
            "Ctrl+Alt+Delete",
            "Shift+Tab",
            "Mod+Equals",
        ] {
            let c = Chord::parse(text).unwrap();
            assert_eq!(c.text(), text);
        }
        // Modifiers in any order read; they print in one.
        assert_eq!(Chord::parse("Shift+Mod+Z").unwrap().text(), "Mod+Shift+Z");
        for bad in ["Mod+Ctrl+K", "mod+K", "Mod+", "Hyper+X"] {
            assert_eq!(Chord::parse(bad), None, "{bad}");
        }
    }

    /// Every key the daemon's schema accepts is a key egui knows, by the
    /// same name.
    #[test]
    fn the_schema_s_keys_are_egui_s() {
        for name in ryucad::config::key_names() {
            let k = Key::from_name(&name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(k.name(), name);
        }
    }

    #[test]
    fn mod_is_command_and_ctrl_is_ctrl_on_mac_only() {
        let undo = Chord::parse("Mod+Z").unwrap();
        assert_eq!(
            undo.shortcut(false),
            KeyboardShortcut::new(Modifiers::COMMAND, Key::Z)
        );
        assert_eq!(
            undo.shortcut(true),
            KeyboardShortcut::new(Modifiers::COMMAND, Key::Z)
        );
        let ctrl = Chord::parse("Ctrl+Z").unwrap();
        assert_eq!(
            ctrl.shortcut(true),
            KeyboardShortcut::new(Modifiers::CTRL, Key::Z)
        );
        assert!(ctrl.same(&undo, false));
        assert!(!ctrl.same(&undo, true));
        let press = Chord::pressed(Key::Z, Modifiers::COMMAND, false);
        assert!(press.same(&undo, false));
        assert!(Chord::parse("Mod+K").unwrap().modified());
        assert!(!Chord::parse("Shift+K").unwrap().modified());
    }
}
