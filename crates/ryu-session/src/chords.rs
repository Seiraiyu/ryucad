//! The chord syntax of a window's `ui.keys` (design 199, Keys): what a
//! product's daemon checks the preference with and its window reads
//! chords by, so the two never disagree. Moved from `ryucad::config` by
//! ryueda 3.C, unchanged.

/// The most chords `ui.keys` binds to one command.
pub const MAX_CHORDS: usize = 8;

/// The modifiers a chord may name, in the order the app writes them: `Mod`
/// is Cmd on macOS and Ctrl elsewhere, `Ctrl` is Ctrl everywhere.
pub const MODIFIERS: [&str; 4] = ["Mod", "Ctrl", "Alt", "Shift"];

/// The keys named by more than one character (egui's names for them).
pub const NAMED_KEYS: [&str; 26] = [
    "Up",
    "Down",
    "Left",
    "Right",
    "Escape",
    "Tab",
    "Backspace",
    "Enter",
    "Insert",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Space",
    "Comma",
    "Minus",
    "Period",
    "Plus",
    "Equals",
    "Semicolon",
    "Backslash",
    "Slash",
    "OpenBracket",
    "CloseBracket",
    "Backtick",
];

/// Every key a chord may end in: A to Z, 0 to 9, F1 to F24 and
/// [`NAMED_KEYS`].
pub fn key_names() -> Vec<String> {
    let mut out: Vec<String> = ('A'..='Z').chain('0'..='9').map(String::from).collect();
    out.extend((1..=24).map(|n| format!("F{n}")));
    out.extend(NAMED_KEYS.iter().map(|k| (*k).to_owned()));
    out
}

/// A chord's text: modifiers from [`MODIFIERS`], each once and not `Mod`
/// with `Ctrl` (they are one key off macOS), then one of [`key_names`],
/// joined by `+`.
pub fn chord_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split('+').collect();
    let Some((key, mods)) = parts.split_last() else {
        return false;
    };
    let mut seen = [false; 4];
    for m in mods {
        let Some(i) = MODIFIERS.iter().position(|x| x == m) else {
            return false;
        };
        if std::mem::replace(&mut seen[i], true) {
            return false;
        }
    }
    !(seen[0] && seen[1]) && key_names().iter().any(|k| k == key)
}

/// A command id: dotted lower-case words, at least two (`edit.undo`,
/// `view.front_top_right`), at most 64 characters.
pub fn command_id_ok(id: &str) -> bool {
    id.len() <= 64
        && id.split('.').count() >= 2
        && id.split('.').all(|w| {
            w.starts_with(|c: char| c.is_ascii_lowercase())
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_read_as_ryucad_s_preferences_always_have() {
        for ok in [
            "Mod+K",
            "Mod+Shift+Z",
            "F",
            "1",
            "Ctrl+Alt+Delete",
            "Shift+Tab",
            "F24",
            "Mod+Equals",
        ] {
            assert!(chord_ok(ok), "{ok}");
        }
        for bad in [
            "Mod+Ctrl+K",
            "mod+K",
            "Mod+",
            "Hyper+X",
            "Shift+Shift+A",
            "",
        ] {
            assert!(!chord_ok(bad), "{bad}");
        }
        assert_eq!(key_names().len(), 26 + 10 + 24 + NAMED_KEYS.len());
    }

    #[test]
    fn command_ids_are_dotted_lower_case_words() {
        assert!(command_id_ok("edit.undo") && command_id_ok("view.front_top_right"));
        for bad in [
            "undo",
            "Edit.undo",
            "edit..undo",
            "edit.1x",
            &"a.".repeat(40),
        ] {
            assert!(!command_id_ok(bad), "{bad}");
        }
    }
}
