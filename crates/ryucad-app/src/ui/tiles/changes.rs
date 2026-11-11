//! The Changes tile: what a compare finds in the chosen feature.

use crate::core::workspace::Workspace;
use crate::ui::view::View;
use eframe::egui::{self, RichText};

/// In a compare, what changed in the feature chosen in the tree since the
/// revision compared with, after its part's PDM and the parameters of its
/// part and of the project: a change nothing reads, weak, with
/// `(read by nothing)`.
pub fn changes(ui: &mut egui::Ui, ws: &Workspace, view: &View) {
    let (Some(c), Some(key)) = (&ws.compare, &view.chosen) else {
        return;
    };
    ui.heading(format!("Changes since {}", c.rev));
    // The part's properties, records and state, beside its features'.
    if let Some(m) = c.pdm.get(&key.0) {
        ui.label(RichText::new(format!("Part {}", key.0)).strong());
        for l in m.marks.iter().chain(&m.lines) {
            ui.label(l);
        }
    }
    let lists = [
        (format!("Parameters of {}", key.0), c.params.get(&key.0)),
        ("Project parameters".to_owned(), Some(&c.project_params)),
    ];
    for (title, list) in lists {
        let Some(list) = list.filter(|l| !l.is_empty()) else {
            continue;
        };
        ui.label(RichText::new(title).strong());
        for p in list {
            if p.unused {
                ui.weak(format!("{} (read by nothing)", p.line));
            } else {
                ui.label(&p.line);
            }
        }
    }
    match c.features.get(key) {
        Some(m) if !m.lines.is_empty() => {
            for l in &m.lines {
                ui.label(l);
            }
        }
        Some(m) => {
            ui.label(m.change.as_str());
        }
        None if !c.loaded => {
            ui.spinner();
        }
        None => {
            ui.weak(format!("{} is as it was", key.1));
        }
    }
    ui.separator();
}
