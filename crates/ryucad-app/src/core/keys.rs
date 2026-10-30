//! Chords (design 199, Command registry > Keymap): modifiers plus a key,
//! written `Mod+Shift+Z`, where `Mod` is Cmd on macOS and Ctrl elsewhere
//! and `Ctrl` is Ctrl everywhere. The syntax is `ryucad::config`'s
//! (`chord_ok`, the `ui.keys` preference's check), so the daemon and the
//! window read chords alike. [`Keymap`] is the effective map: the
//! registry's defaults, then the navigation style's preset, then
//! `ui.keys`.

use crate::core::command::{self, COMMANDS};
use crate::core::nav::Navigation;
use eframe::egui::{Key, KeyboardShortcut, Modifiers};
use ryucad::config::{MODIFIERS, chord_ok};
use serde_json::{Map, Value};

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

/// The effective keymap: each command's chords from the registry, then
/// the navigation style's preset, then `ui.keys`. Ids in `ui.keys` this
/// build doesn't know are kept (a newer build's bindings survive an older
/// one) and ignored; chords it can't read are dropped with a note.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    /// Every command's chords, in registry order.
    binds: Vec<(&'static str, Vec<Chord>)>,
    /// `ui.keys` as it was read, unknown ids and all: what a rebind
    /// writes back, changed.
    user: Map<String, Value>,
    /// What couldn't be used, for the status line.
    pub notes: Vec<String>,
}

/// A chord another command already has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub chord: Chord,
    /// The command that has it.
    pub with: &'static str,
}

impl Keymap {
    pub fn new(style: Navigation, user: &Value) -> Keymap {
        let mut binds: Vec<(&'static str, Vec<Chord>)> = COMMANDS
            .iter()
            .map(|c| {
                (
                    c.id,
                    c.keys.iter().filter_map(|k| Chord::parse(k)).collect(),
                )
            })
            .collect();
        let set = |id: &str, chords: Vec<Chord>, binds: &mut Vec<(&'static str, Vec<Chord>)>| {
            if let Some(b) = binds.iter_mut().find(|b| b.0 == id) {
                b.1 = chords;
            }
        };
        let mut preset: Vec<(&str, Vec<Chord>)> = Vec::new();
        for (id, chord) in crate::core::keymap::preset(style) {
            match preset.iter_mut().find(|p| p.0 == *id) {
                Some(p) => p.1.extend(Chord::parse(chord)),
                None => preset.push((id, Chord::parse(chord).into_iter().collect())),
            }
        }
        for (id, chords) in preset {
            set(id, chords, &mut binds);
        }
        let mut notes = Vec::new();
        let user = user.as_object().cloned().unwrap_or_default();
        for (id, chords) in &user {
            let Some(list) = chords.as_array() else {
                notes.push(format!("ui.keys: {id}'s keys aren't a list"));
                continue;
            };
            let mut read = Vec::new();
            for c in list {
                match c.as_str().and_then(Chord::parse) {
                    Some(c) => read.push(c),
                    None => notes.push(format!(
                        "ui.keys: {id} has a chord this build can't read: {c}"
                    )),
                }
            }
            set(id, read, &mut binds);
        }
        Keymap { binds, user, notes }
    }

    /// A command's chords; none for an id the registry doesn't have.
    pub fn chords(&self, id: &str) -> &[Chord] {
        self.binds
            .iter()
            .find(|b| b.0 == id)
            .map_or(&[], |b| b.1.as_slice())
    }

    /// Every command and its chords, in registry order.
    pub fn all(&self) -> impl Iterator<Item = (&'static str, &[Chord])> {
        self.binds.iter().map(|(id, c)| (*id, c.as_slice()))
    }

    /// Whether `ui.keys` sets this command's keys.
    pub fn overridden(&self, id: &str) -> bool {
        self.user.contains_key(id)
    }

    /// The command other than `id` whose chords hold `chord` where their
    /// scopes meet, on this OS (`mac`).
    pub fn taken(&self, id: &str, chord: &Chord, mac: bool) -> Option<&'static str> {
        self.holders(id, chord, mac).next()
    }

    /// Every other command with `chord` where its scope meets `id`'s, in
    /// the keymap's order.
    fn holders<'a>(
        &'a self,
        id: &'a str,
        chord: &'a Chord,
        mac: bool,
    ) -> impl Iterator<Item = &'static str> + 'a {
        let scope = command::get(id).map(|c| c.scope);
        self.binds.iter().filter_map(move |(other, chords)| {
            let meets =
                scope.is_some_and(|s| command::get(other).is_some_and(|c| c.scope.meets(s)));
            (*other != id && meets && chords.iter().any(|c| c.same(chord, mac))).then_some(*other)
        })
    }

    /// Every pair of commands sharing a chord where their scopes meet.
    pub fn conflicts(&self, mac: bool) -> Vec<(&'static str, &'static str, Chord)> {
        let mut out = Vec::new();
        for (i, (a, chords)) in self.binds.iter().enumerate() {
            for c in chords {
                for (b, _) in &self.binds[i + 1..] {
                    if self.holders(a, c, mac).any(|h| h == *b) {
                        out.push((*a, *b, *c));
                    }
                }
            }
        }
        out
    }

    /// `ui.keys` with `id` bound to `chords`, or the first chord another
    /// command has, before anything is written.
    pub fn rebind(&self, id: &str, chords: &[Chord], mac: bool) -> Result<Value, Conflict> {
        for c in chords {
            if let Some(with) = self.taken(id, c, mac) {
                return Err(Conflict { chord: *c, with });
            }
        }
        let mut user = self.user.clone();
        user.insert(
            id.to_owned(),
            chords.iter().map(|c| Value::from(c.text())).collect(),
        );
        Ok(Value::Object(user))
    }

    /// `ui.keys` without `id`: its keys back to the default.
    pub fn reset(&self, id: &str) -> Value {
        let mut user = self.user.clone();
        user.remove(id);
        Value::Object(user)
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
