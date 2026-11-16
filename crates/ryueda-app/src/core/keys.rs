//! ryueda's keymap: its table ([`crate::core::command::COMMANDS`]),
//! then `ui.keys`. ryueda has no navigation presets.

pub use ryu_app::keys::{Chord, Conflict, captured, consume, is_modifier};

pub type Keymap = ryu_app::keys::Keymap<crate::core::product::Eda>;
