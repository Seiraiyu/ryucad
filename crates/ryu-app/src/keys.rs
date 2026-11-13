//! Chords (design 199, Command registry > Keymap): modifiers plus a key,
//! written `Mod+Shift+Z`, where `Mod` is Cmd on macOS and Ctrl elsewhere
//! and `Ctrl` is Ctrl everywhere. The syntax is `ryu_session::chords`',
//! which a product's daemon checks `ui.keys` with, so the daemon and the
//! window read chords alike. [`Keymap`] is the effective map: the
//! registry's defaults, then the preset's, then `ui.keys`. Moved from
//! ryucad-app by ryueda 3.C.

use crate::command::{Command, Scope};
use egui::{Key, KeyboardShortcut, Modifiers};
use ryu_session::chords::{MODIFIERS, chord_ok};
use serde_json::{Map, Value};
use std::marker::PhantomData;

/// One preset entry: a command id and its chord.
pub type Binding = (&'static str, &'static str);

/// A product's registry and its presets (ryucad's navigation styles).
pub trait Registry: 'static {
    /// What picks a preset.
    type Style: Copy;
    /// What the registry's context menus target.
    type Context: Copy + 'static;
    fn commands() -> &'static [Command<Self::Context>];
    fn preset(style: Self::Style) -> &'static [Binding];
}

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

    /// Whether it types nothing into a text field: a modified chord, or a
    /// function key (F6 leaves a field for the next tile).
    pub fn types_nothing(&self) -> bool {
        self.modified()
            || self
                .key
                .name()
                .strip_prefix('F')
                .is_some_and(|n| n.parse::<u8>().is_ok())
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

/// Whether `key` is a modifier's own key (Shift, Control, Alt or Super,
/// either side): egui reports their presses as keys of their own, which a
/// chord being captured must wait past (#385).
pub fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

/// The press a chord being captured takes from a frame's events: the
/// first key pressed that isn't a modifier's own, with the modifiers held
/// (#385: Ctrl+Shift+F was taken as `ControlLeft`).
pub fn captured(events: &[egui::Event]) -> Option<(Key, Modifiers)> {
    events.iter().find_map(|e| match e {
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } if !is_modifier(*key) => Some((*key, *modifiers)),
        _ => None,
    })
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

/// The effective keymap: each command's chords from the registry, then
/// the preset, then `ui.keys`. Ids in `ui.keys` this build doesn't know
/// are kept (a newer build's bindings survive an older one) and ignored;
/// chords it can't read are dropped with a note.
pub struct Keymap<R: Registry> {
    /// Every command, its scope and its chords, in registry order.
    binds: Vec<(&'static str, Scope, Vec<Chord>)>,
    /// `ui.keys` as it was read, unknown ids and all: what a rebind
    /// writes back, changed.
    user: Map<String, Value>,
    /// What couldn't be used, for the status line.
    pub notes: Vec<String>,
    registry: PhantomData<fn() -> R>,
}

impl<R: Registry> Clone for Keymap<R> {
    fn clone(&self) -> Self {
        Keymap {
            binds: self.binds.clone(),
            user: self.user.clone(),
            notes: self.notes.clone(),
            registry: PhantomData,
        }
    }
}

impl<R: Registry> std::fmt::Debug for Keymap<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keymap")
            .field("binds", &self.binds)
            .field("user", &self.user)
            .field("notes", &self.notes)
            .finish()
    }
}

impl<R: Registry> PartialEq for Keymap<R> {
    fn eq(&self, o: &Self) -> bool {
        self.binds == o.binds && self.user == o.user && self.notes == o.notes
    }
}

/// A chord another command already has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub chord: Chord,
    /// The command that has it.
    pub with: &'static str,
}

type Binds = Vec<(&'static str, Scope, Vec<Chord>)>;

impl<R: Registry> Keymap<R> {
    pub fn new(style: R::Style, user: &Value) -> Self {
        let mut binds: Binds = R::commands()
            .iter()
            .map(|c| {
                (
                    c.id,
                    c.scope,
                    c.keys.iter().filter_map(|k| Chord::parse(k)).collect(),
                )
            })
            .collect();
        let set = |id: &str, chords: Vec<Chord>, binds: &mut Binds| {
            if let Some(b) = binds.iter_mut().find(|b| b.0 == id) {
                b.2 = chords;
            }
        };
        let mut preset: Vec<(&str, Vec<Chord>)> = Vec::new();
        for (id, chord) in R::preset(style) {
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
        Keymap {
            binds,
            user,
            notes,
            registry: PhantomData,
        }
    }

    /// A command's chords; none for an id the registry doesn't have.
    pub fn chords(&self, id: &str) -> &[Chord] {
        self.binds
            .iter()
            .find(|b| b.0 == id)
            .map_or(&[], |b| b.2.as_slice())
    }

    /// Every command and its chords, in registry order.
    pub fn all(&self) -> impl Iterator<Item = (&'static str, &[Chord])> {
        self.binds.iter().map(|(id, _, c)| (*id, c.as_slice()))
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
    /// the keymap's order. A command's scope is the registry's: the first
    /// entry with its id, as `command::get` finds it.
    fn holders<'a>(
        &'a self,
        id: &'a str,
        chord: &'a Chord,
        mac: bool,
    ) -> impl Iterator<Item = &'static str> + 'a {
        let scope_of = |x: &str| self.binds.iter().find(|b| b.0 == x).map(|b| b.1);
        let scope = scope_of(id);
        self.binds.iter().filter_map(move |(other, _, chords)| {
            let meets = scope.is_some_and(|s| scope_of(other).is_some_and(|o| o.meets(s)));
            (*other != id && meets && chords.iter().any(|c| c.same(chord, mac))).then_some(*other)
        })
    }

    /// Every pair of commands sharing a chord where their scopes meet.
    pub fn conflicts(&self, mac: bool) -> Vec<(&'static str, &'static str, Chord)> {
        let mut out = Vec::new();
        for (i, (a, _, chords)) in self.binds.iter().enumerate() {
            for c in chords {
                for (b, _, _) in &self.binds[i + 1..] {
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
    use crate::command::Category;

    /// #385: Ctrl+Shift+F arrives as the modifiers' own presses, then F;
    /// the chord captured is Mod+Shift+F, which the daemon takes.
    #[test]
    fn a_chord_is_captured_past_its_modifiers_own_keys() {
        use egui::Event;
        let press = |key: Key, modifiers: Modifiers| Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        };
        let ctrl = Modifiers::CTRL | Modifiers::COMMAND;
        let events = [
            press(Key::ControlLeft, ctrl),
            press(Key::ShiftLeft, ctrl | Modifiers::SHIFT),
            press(Key::F, ctrl | Modifiers::SHIFT),
        ];
        let (key, m) = captured(&events).unwrap();
        let c = Chord::pressed(key, m, false);
        assert_eq!(c.text(), "Mod+Shift+F");
        assert!(chord_ok(&c.text()));
        // Only modifiers so far: nothing is captured yet.
        assert_eq!(captured(&events[..2]), None);
        assert!(is_modifier(Key::SuperRight) && !is_modifier(Key::Z));
    }

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
        for name in ryu_session::chords::key_names() {
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
        // A function key types nothing, Shift or not; a letter does.
        assert!(Chord::parse("F6").unwrap().types_nothing());
        assert!(Chord::parse("Shift+F6").unwrap().types_nothing());
        assert!(!Chord::parse("F").unwrap().types_nothing());
        assert!(!Chord::parse("Shift+Equals").unwrap().types_nothing());
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Toy;

    const TOY: &[Command<()>] = &[
        crate::command::cmd("edit.undo", "Undo", Category::Edit).keys(&["Mod+Z"]),
        crate::command::cmd("view.fit", "Fit", Category::View).keys(&["F"]),
        crate::command::cmd("view.pan_left", "Pan left", Category::View)
            .keys(&["Left"])
            .focused(),
    ];

    impl Registry for Toy {
        type Style = bool;
        type Context = ();
        fn commands() -> &'static [Command<()>] {
            TOY
        }
        fn preset(other: bool) -> &'static [Binding] {
            if other { &[("view.fit", "G")] } else { &[] }
        }
    }

    #[test]
    fn a_keymap_is_the_registry_then_the_preset_then_ui_keys() {
        use serde_json::json;
        let k = Keymap::<Toy>::new(false, &json!({}));
        assert_eq!(k.chords("view.fit"), [Chord::key(Key::F)]);
        // A window key and a viewport key meet; Mod+Z is undo's.
        assert_eq!(
            k.taken("view.pan_left", &Chord::command(Key::Z), false),
            Some("edit.undo")
        );
        let k = Keymap::<Toy>::new(
            true,
            &json!({"edit.undo": ["Alt+Backspace"], "x.y": ["Mod+J"]}),
        );
        assert_eq!(k.chords("view.fit"), [Chord::key(Key::G)]);
        assert_eq!(
            k.chords("edit.undo"),
            [Chord::parse("Alt+Backspace").unwrap()]
        );
        assert!(k.overridden("edit.undo") && !k.overridden("view.fit"));
        assert_eq!(
            k.taken("view.pan_left", &Chord::command(Key::Z), false),
            None
        );
        let v = k.rebind("view.fit", &[Chord::key(Key::H)], false).unwrap();
        assert_eq!(v["x.y"], json!(["Mod+J"]));
        assert_eq!(k.conflicts(false), []);
    }
}
