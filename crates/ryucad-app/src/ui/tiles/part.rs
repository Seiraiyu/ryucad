//! The Part tile: the part's PDM properties, and where it is used.

use crate::core::workspace::Workspace;
use crate::ui::form::{self, Banner};
use crate::ui::icon::Icon;
use crate::ui::view::View;
use eframe::egui;

/// One property's control, by its type: a line or a box of text, a
/// choice, or a checkbox; labelled by `name` for accessibility. Returns
/// whether Enter or Esc left a line of it.
pub(crate) fn prop_control(
    ui: &mut egui::Ui,
    prop: &mut crate::core::props::Prop,
    name: egui::Id,
) -> (bool, bool) {
    use crate::core::props::Control;
    let f = &mut prop.field;
    let mut line = false;
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
        _ => {
            line = true;
            ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(150.0))
        }
    };
    let r = r.labelled_by(name);
    let keys = if line && r.lost_focus() {
        ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        })
    } else {
        (false, false)
    };
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
    keys
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
    let badge = part.as_deref().and_then(|p| {
        let b = ws.states.badge(p)?;
        Some(match ws.releases.approved_by(p) {
            Some(a) => format!("{b}, {a}"),
            None => b,
        })
    });
    let Some(p) = &mut ws.props else {
        return;
    };
    form::heading(
        ui,
        Some(Icon::PACKAGE),
        &format!("Properties of {}", p.part),
        p.dirty(),
    );
    if let Some(b) = badge {
        ui.weak(format!("Release state: {b}"));
    }
    match form::banner(ui, &format!("{}'s properties", p.part), &p.changed) {
        Some(Banner::Reload) => p.changed.clear(),
        Some(Banner::KeepMine) => {
            for c in std::mem::take(&mut p.changed) {
                if let Some(prop) = p.props.iter_mut().find(|x| x.field.name == c.name) {
                    prop.field.text = c.yours;
                }
            }
        }
        None => {}
    }
    if let Some(e) = &p.error {
        form::error(ui, e);
    }
    let keys = form::grid(ui, "properties", |rows| {
        for prop in &mut p.props {
            let star = if prop.required { " *" } else { "" };
            let label = format!("{}{star}", prop.field.name);
            let dirty = prop.field.dirty();
            let revising = prop.revising;
            let entered = rows.custom_dirty(&label, dirty, |ui, name| {
                ui.horizontal(|ui| {
                    let entered = prop_control(ui, prop, name);
                    if revising {
                        ui.weak("revising").on_hover_text(
                            "A release records it: changing it is work towards the next revision",
                        );
                    }
                    entered
                })
                .inner
            });
            rows.keys.apply |= entered.0;
            rows.keys.revert |= entered.1;
            for fault in &prop.faults {
                rows.note(fault);
            }
        }
    });
    let (dirty, saving) = (p.dirty(), p.saving);
    let foot = form::foot(ui, dirty, saving, ("Apply properties", "Revert properties"));
    if (foot.revert || keys.revert) && dirty {
        for prop in &mut p.props {
            prop.field.text.clone_from(&prop.field.read);
        }
        p.error = None;
    }
    if (foot.apply || keys.apply) && dirty && !saving {
        ws.save_props();
    }
}
