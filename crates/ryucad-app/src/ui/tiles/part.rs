//! The Part tile: the part's PDM properties, and where it is used.

use crate::core::workspace::Workspace;
use crate::ui::view::View;
use eframe::egui;

/// One property's control, by its type: a line or a box of text, a
/// choice, or a checkbox; labelled by `name` for accessibility.
pub(crate) fn prop_control(ui: &mut egui::Ui, prop: &mut crate::core::props::Prop, name: egui::Id) {
    use crate::core::props::Control;
    let f = &mut prop.field;
    let r = match &prop.control {
        Control::Text { multiline: true } => ui.add(
            egui::TextEdit::multiline(&mut f.text)
                .desired_rows(2)
                .desired_width(150.0),
        ),
        Control::Boolean => {
            let mut on = f.text == "true";
            let r = ui.checkbox(&mut on, "");
            if r.changed() {
                f.text = on.to_string();
            }
            r
        }
        Control::Enum(values) => {
            let shown = if f.text.is_empty() {
                "(not set)".to_owned()
            } else {
                f.text.clone()
            };
            egui::ComboBox::from_id_salt(("property", &f.name))
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.text, String::new(), "(not set)");
                    for v in values {
                        ui.selectable_value(&mut f.text, v.clone(), v);
                    }
                })
                .response
        }
        _ => ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(150.0)),
    };
    let r = r.labelled_by(name);
    if f.dirty() {
        r.on_hover_text(format!(
            "was {}",
            if f.read.is_empty() {
                "not set"
            } else {
                &f.read
            }
        ));
    }
}

/// The part the Part section shows: the chosen feature's part, else the
/// first.
pub fn section_part(ws: &Workspace, view: &View) -> Option<String> {
    view.chosen
        .as_ref()
        .map(|k| k.0.clone())
        .or_else(|| ws.model.parts.keys().next().cloned())
}

/// The Part section: the chosen feature's part (or the first part), its
/// release state and every property, each with a control for its type,
/// marked required (`*`) or revising, its faults beside it. Apply sends
/// `props.set` against the revision they were read at. With PDM not set
/// up, Set up PDM (`pdm.init`) gives the project its ID.
pub fn part_section(ui: &mut egui::Ui, ws: &mut Workspace, view: &View) {
    let part = section_part(ws, view);
    ws.show_props(part.as_deref());
    if ws.pdm_config.is_some() && !ws.pdm_ready() {
        ui.weak("PDM isn't set up: the project has no ID for release records yet.");
        if ui.button("Set up PDM").clicked() {
            ws.pdm_init();
        }
    }
    let badge = part.as_deref().and_then(|p| ws.states.badge(p));
    let Some(p) = &mut ws.props else {
        return;
    };
    ui.heading(format!("Properties of {}", p.part));
    if let Some(b) = badge {
        ui.weak(format!("Release state: {b}"));
    }
    if !p.changed.is_empty() {
        let frame =
            egui::Frame::group(ui.style()).fill(ui.visuals().warn_fg_color.gamma_multiply(0.15));
        frame.show(ui, |ui| {
            ui.label("Someone changed these properties while you edited them. They now read:");
            for c in &p.changed {
                ui.label(format!("{}: {} (you typed {})", c.name, c.theirs, c.yours));
            }
            if ui.button("OK").clicked() {
                p.changed.clear();
            }
        });
    }
    let error = ui.visuals().error_fg_color;
    egui::Grid::new("properties").num_columns(3).show(ui, |ui| {
        for prop in &mut p.props {
            let star = if prop.required { " *" } else { "" };
            let name = ui.label(format!("{}{star}", prop.field.name));
            prop_control(ui, prop, name.id);
            if prop.revising {
                ui.weak("revising").on_hover_text(
                    "A release records it: changing it is work towards the next revision",
                );
            } else {
                ui.label("");
            }
            ui.end_row();
            for fault in &prop.faults {
                ui.label("");
                ui.colored_label(error, fault);
                ui.end_row();
            }
        }
    });
    if let Some(e) = &p.error {
        ui.colored_label(error, &e.message);
    }
    let (dirty, saving) = (p.dirty(), p.saving);
    let mut save = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(dirty && !saving, egui::Button::new("Apply properties"))
            .clicked();
        if ui
            .add_enabled(dirty && !saving, egui::Button::new("Revert properties"))
            .clicked()
        {
            for prop in &mut p.props {
                prop.field.text.clone_from(&prop.field.read);
            }
            p.error = None;
        }
        if saving {
            ui.spinner();
        }
    });
    if save {
        ws.save_props();
    }
}
