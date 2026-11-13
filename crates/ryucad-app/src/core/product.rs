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
