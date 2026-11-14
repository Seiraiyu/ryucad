//! ryueda's command table: every action the window offers, once, by an
//! id `ui.keys` names. Menus, the toolbar, context menus, the palette and
//! the keymap are built from it.

use ryu_app::command::{Category, Command, cmd};

/// What a context menu is offered on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Here {
    /// Something picked on the canvas.
    Canvas,
    /// A row of a list (navigator, findings, instructions).
    Row,
}

pub static COMMANDS: &[Command<Here>] = &[
    cmd("view.fit", "Fit sheet", Category::View).icon("corners-out").keys(&["F"]).viewport().toolbar(),
];
