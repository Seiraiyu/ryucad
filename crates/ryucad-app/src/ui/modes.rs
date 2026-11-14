//! The window's mode, shown (design 374, Modes): a bar under the toolbar
//! and a 3-point frame round the viewport in the mode's colour while it
//! annotates or edits directly, saying so, with the count of the
//! person's open annotations, Show agent's model and the way back to
//! View. View shows neither.

use crate::core::modes::{Gate, Mode};
use crate::core::workspace::Workspace;
use crate::ui::icon::{self, Icon};
use crate::ui::style::{self, Palette};
use eframe::egui::{self, RichText};

/// What the bar says: its lead, the rest and its button's name.
pub fn words(mode: Mode) -> Option<(&'static str, &'static str, &'static str)> {
    match mode {
        Mode::View => None,
        Mode::Annotate => Some((
            "Annotating:",
            "your changes are suggestions for the agent",
            "Stop annotating",
        )),
        Mode::Edit => Some(("Direct edit:", "you are changing the model", "Stop editing")),
    }
}

/// The mode's colour: `None` in View.
pub fn colour(mode: Mode, p: &Palette) -> Option<u32> {
    match mode {
        Mode::View => None,
        Mode::Annotate => Some(p.annotate),
        Mode::Edit => Some(p.direct),
    }
}

/// The bar across the window, under the toolbar.
pub fn bar(ui: &mut egui::Ui, ws: &mut Workspace, p: &Palette) {
    let (Some((lead, rest, stop)), Some(c)) = (words(ws.session.mode), colour(ws.session.mode, p))
    else {
        return;
    };
    let glyph = if ws.session.mode == Mode::Annotate {
        Icon::NOTE_PENCIL
    } else {
        Icon::PENCIL_SIMPLE
    };
    ui.horizontal(|ui| {
        icon::show(ui, glyph, 14.0, style::rgb(c));
        ui.label(RichText::new(lead).strong().color(style::rgb(c)));
        ui.label(rest);
        if ws.session.mode == Mode::Annotate {
            let n = open_mine(ws);
            ui.weak(match n {
                1 => "1 open annotation of yours".to_owned(),
                n => format!("{n} open annotations of yours"),
            });
            draft_toggle(ui, ws);
        }
        if ui.button(stop).clicked() {
            ws.set_mode(Mode::View);
        }
    });
}

/// How many of the window's person's annotations are open.
fn open_mine(ws: &Workspace) -> usize {
    ws.annotations.open_mine(ws.my_name())
}

/// Show agent's model, or Show my suggestions, while the person has a
/// draft.
fn draft_toggle(ui: &mut egui::Ui, ws: &mut Workspace) {
    if ws.annotations.draft(ws.my_name()).is_empty() {
        return;
    }
    if ws.myself().is_some_and(|m| m.compare.is_some()) {
        ui.weak("Comparing: stop comparing to see your suggestions");
        return;
    }
    let (text, to) = if ws.show_agent {
        ("Show my suggestions", false)
    } else {
        ("Show agent's model", true)
    };
    if ui.button(text).clicked() {
        ws.set_show_agent(to);
    }
}

/// The frame round the viewport, inside its rect.
pub fn frame(ui: &egui::Ui, ws: &Workspace, rect: egui::Rect, p: &Palette) {
    if let Some(c) = colour(ws.session.mode, p) {
        ui.painter().rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(3.0, style::rgb(c)),
            egui::StrokeKind::Inside,
        );
    }
}

/// A button for a control that writes by `method`: disabled where the
/// window's mode doesn't allow it, its tooltip saying which mode would,
/// and saying in Annotate that it suggests.
pub fn button(
    ui: &mut egui::Ui,
    ws: &Workspace,
    method: &str,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    gated(ui, &ws.control(method), egui::Button::new(text))
}

/// [`button`] for any widget, by its gate.
pub fn gated(ui: &mut egui::Ui, gate: &Gate, widget: impl egui::Widget) -> egui::Response {
    match gate {
        Gate::Send => ui.add(widget),
        Gate::Propose => ui
            .add(widget)
            .on_hover_text("Sent to the agent as a suggestion (Annotate)"),
        Gate::Refuse(why) => ui.add_enabled(false, widget).on_disabled_hover_text(*why),
    }
}

/// [`gated`] for a widget the control also enables by its own rule.
pub fn gated_if(
    ui: &mut egui::Ui,
    gate: &Gate,
    enabled: bool,
    widget: impl egui::Widget,
) -> egui::Response {
    match gate {
        Gate::Refuse(why) => ui.add_enabled(false, widget).on_disabled_hover_text(*why),
        Gate::Propose => ui
            .add_enabled(enabled, widget)
            .on_hover_text("Sent to the agent as a suggestion (Annotate)"),
        Gate::Send => ui.add_enabled(enabled, widget),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mode_has_its_words_and_colour() {
        let p = crate::ui::style::Look::Drafting.palette(false);
        assert!(words(Mode::View).is_none());
        let (lead, rest, stop) = words(Mode::Annotate).unwrap();
        assert_eq!(lead, "Annotating:");
        assert!(rest.contains("suggestions for the agent"));
        assert_eq!(stop, "Stop annotating");
        let (lead, rest, stop) = words(Mode::Edit).unwrap();
        assert_eq!((lead, stop), ("Direct edit:", "Stop editing"));
        assert!(rest.contains("changing the model"));
        assert_eq!(colour(Mode::Annotate, &p), Some(p.annotate));
        assert_eq!(colour(Mode::Edit, &p), Some(p.direct));
        assert_eq!(colour(Mode::View, &p), None);
    }

    #[test]
    fn a_refused_control_is_disabled_and_says_why() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::{NodeT, Queryable};
        for (gate, enabled) in [
            (Gate::Send, true),
            (Gate::Propose, true),
            (Gate::Refuse("Direct edit does this."), false),
        ] {
            let g = gate.clone();
            let mut h = Harness::new_ui(move |ui| {
                gated(ui, &g, egui::Button::new("Apply"));
                crate::ui::form::foot(ui, true, false, ("Save", "Revert"), &g);
            });
            h.run();
            for name in ["Apply", "Save"] {
                assert_eq!(
                    !h.get_by_label(name).accesskit_node().is_disabled(),
                    enabled,
                    "{name} with {gate:?}"
                );
            }
            // Revert only puts back what was read: never gated.
            assert!(!h.get_by_label("Revert").accesskit_node().is_disabled());
        }
    }
}
