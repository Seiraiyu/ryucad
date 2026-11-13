//! ryucad as the shell's product (ryueda 3.C): the marker through which
//! `ryu_app`'s generic keymap, toasts and link take ryucad's registry,
//! words and requests. ryucad-app's modules name the shell's types with
//! it (`core::keys::Keymap` is `ryu_app::keys::Keymap<Ryucad>`).

use crate::core::command::{COMMANDS, Command, Context};
use crate::core::nav::Navigation;
use ryu_app::keys::Binding;

/// ryucad, to the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ryucad;

impl ryu_app::keys::Registry for Ryucad {
    type Style = Navigation;
    type Context = Context;
    fn commands() -> &'static [Command] {
        COMMANDS
    }
    fn preset(style: Navigation) -> &'static [Binding] {
        crate::core::keymap::preset(style)
    }
}

impl ryu_app::toast::Words for Ryucad {
    const OBJECTS: &'static str = "features";
    const GROUPS: &'static str = "parts";
    fn quiet(entry: &serde_json::Value) -> bool {
        // Annotations' own writes make toasts of their own (`annotation`)
        // for their authors and show in the tile; applying one writes the
        // model, and is a change like any.
        entry["method"]
            .as_str()
            .is_some_and(|m| m.starts_with("annotation.") && m != "annotation.apply")
    }
    fn verb(method: &str, kind: &serde_json::Value) -> String {
        crate::core::toast::verb(method, kind)
    }
    fn objects(entry: &serde_json::Value) -> Vec<(String, String)> {
        crate::core::toast::features_of(entry)
    }
    fn groups(entry: &serde_json::Value) -> std::collections::BTreeSet<String> {
        crate::core::toast::parts_of(entry)
    }
    fn undoable(entry: &serde_json::Value) -> bool {
        crate::core::activity::can_undo(entry)
    }
}
