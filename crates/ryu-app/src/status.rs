//! The status line's parts every product shows at its right: the look
//! and theme, and the daemon's connection (design 199, Feedback). Drawn
//! right to left, in the order a product calls them. Moved from
//! ryucad-app's `ui::panels` by ryueda 3.C.

use crate::icon::{self, Icon};
use crate::style::{Contrast, Look};

/// `Drafting · light`, or `High contrast · dark`.
pub fn look(ui: &mut egui::Ui, look: Look, contrast: Contrast) {
    ui.weak(format!(
        "{} · {}",
        match contrast {
            Contrast::High => "High contrast",
            Contrast::Normal => look.title(),
        },
        if ui.visuals().dark_mode {
            "dark"
        } else {
            "light"
        }
    ));
}

/// `daemon 0.9.0` with a plug, or `reconnecting` while it is lost.
pub fn connection(ui: &mut egui::Ui, lost: bool, version: &str) {
    let (glyph, text) = match lost {
        true => (Icon::PLUGS, "reconnecting".to_owned()),
        false => (Icon::PLUGS_CONNECTED, format!("daemon {version}")),
    };
    ui.weak(text);
    icon::show(ui, glyph, 14.0, ui.visuals().weak_text_color());
}
