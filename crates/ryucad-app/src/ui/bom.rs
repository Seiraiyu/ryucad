//! The BOM tab, drawn from `ws.bom` (`core::bom`): the BOM of the part
//! the window shows, indented or flat, with the CSV's columns, its
//! findings above it and each line's problem beside it. Choosing a line
//! selects its occurrences and frames them. An indented line's item
//! number is a button: pressed, it is typed and set (`bom.renumber`).
//! While the window compares, lines are painted as the compare view
//! paints faces: added new, changed cells with their old value, removed
//! lines in the removed colour.
//!
//! Each panel is one function taking the `Ui` it draws in, so the bottom
//! panel's tab can become a tile of its own without changing it.

use crate::core::bom::{BomTab, Line, Mark};
use crate::core::workspace::Workspace;
use crate::ui::style;
use crate::ui::view::View;
use eframe::egui::{self, RichText};
use ryucad::live::SelectMode;

/// A line's name for people and accessibility: `BOM line 1.2: bolt`, or in
/// the flat BOM `BOM line: bolt A`.
pub fn line_name(line: &Line, level: &str) -> String {
    if line.depth == 0 {
        format!("BOM line: {} {}", line.part, line.rev)
    } else {
        format!("BOM line {level}: {}", line.part)
    }
}

/// The BOM tab.
pub fn tab(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    let mut flat = ws.bom.flat;
    let mut focus = None;
    ui.horizontal(|ui| {
        match &ws.bom.root {
            Some(r) => ui.strong(format!("BOM of {r}")),
            None => ui.strong("BOM"),
        };
        ui.checkbox(&mut flat, "Flat")
            .on_hover_text("Every part bought or made at the bottom, with its total");
        let any = ws.bom.root.is_some();
        if ui
            .add_enabled(any, egui::Button::new("Export CSV…"))
            .on_hover_text("Write this BOM as CSV, as `ryucad bom --out` does")
            .clicked()
        {
            ws.open_bom_export();
        }
    });
    if flat != ws.bom.flat {
        ws.set_bom_flat(flat);
    }
    let error = ui.visuals().error_fg_color;
    if !ws.bom.roots.is_empty() {
        ui.label("The project has several root assemblies: show one for its BOM.");
        ui.horizontal_wrapped(|ui| {
            for r in &ws.bom.roots {
                if ui.button(format!("Show {r}")).clicked() {
                    focus = Some(r.clone());
                }
            }
        });
    } else if let Some(e) = &ws.bom.error {
        ui.colored_label(error, &e.message);
    } else if ws.bom.root.is_none() {
        ui.spinner();
    }
    if let Some(r) = focus {
        ws.set_focus(Some(r));
        return;
    }
    if let Some(c) = &ws.compare {
        match (ws.bom.summary(), &ws.bom.diff_error) {
            (Some(s), _) => ui.label(format!("Compared with {}: {s}", c.rev)),
            (None, Some(e)) => {
                ui.colored_label(error, format!("No BOM at {}: {}", c.rev, e.message))
            }
            (None, None) => ui.spinner(),
        };
    }
    for f in &ws.bom.findings {
        ui.colored_label(ui.visuals().warn_fg_color, f);
    }
    if let Some(c) = &ws.bom.changed {
        let mut ok = false;
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(ui.visuals().warn_fg_color, c);
            ok = ui.button("OK").clicked();
        });
        if ok {
            ws.bom.changed = None;
        }
    }
    if let Some(e) = &ws.bom.edit_error {
        ui.colored_label(error, e);
    }
    let mut chose: Option<Vec<String>> = None;
    // The item number typed, apart from the lines it is typed over.
    let mut typed = ws.bom.editing.as_ref().map(|e| e.text.clone());
    let (mut start, mut set, mut cancel) = (None, false, false);
    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        let t = &ws.bom;
        egui::Grid::new("bom").striped(true).show(ui, |ui| {
            for c in &t.columns {
                ui.strong(c);
            }
            ui.end_row();
            let level_at = t.columns.iter().position(|c| c == "level");
            for line in t.shown() {
                let level = level_at.map_or("", |i| line.cells[i].as_str());
                let chosen = t.chosen.as_ref() == Some(&line.occurrences);
                let colour = match &line.mark {
                    Some(Mark::Added) => Some(style::color32(style::NEW)),
                    Some(Mark::Removed) => Some(style::color32(style::REMOVED)),
                    _ => None,
                };
                if let Some(colour) = colour {
                    for text in &line.cells {
                        let what = if line.mark == Some(Mark::Removed) {
                            "removed"
                        } else {
                            "added"
                        };
                        ui.label(RichText::new(text).color(colour))
                            .on_hover_text(format!("{what} since the compared revision"));
                    }
                    ui.end_row();
                    continue;
                }
                for (i, (c, text)) in t.columns.iter().zip(&line.cells).enumerate() {
                    // A changed cell says what it was; an item number stays
                    // editable.
                    let was = match &line.mark {
                        Some(Mark::Changed(old)) => old.get(&i),
                        _ => None,
                    };
                    if let Some(was) = was
                        && c != "item"
                    {
                        let shown = format!("{text} (was {was})");
                        ui.label(RichText::new(shown).color(style::color32(style::CHANGED)));
                        continue;
                    }
                    match c.as_str() {
                        "level" => {
                            let indent = "    ".repeat(line.depth.saturating_sub(1));
                            ui.label(format!("{indent}{text}"));
                        }
                        "item" if t.is_editing(line) => {
                            let name = format!(
                                "Item of {} in {}",
                                line.part,
                                BomTab::editable(line).unwrap_or("")
                            );
                            ui.horizontal(|ui| {
                                let buffer = typed.get_or_insert_with(String::new);
                                let r =
                                    ui.add(egui::TextEdit::singleline(buffer).desired_width(48.0));
                                r.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, &name)
                                });
                                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    set = true;
                                }
                                if ui
                                    .add_enabled(!t.saving, egui::Button::new("Set item"))
                                    .clicked()
                                {
                                    set = true;
                                }
                                if ui.button("Cancel").clicked() {
                                    cancel = true;
                                }
                                if t.saving {
                                    ui.spinner();
                                }
                            });
                        }
                        "item" if BomTab::editable(line).is_some() && !t.saving => {
                            let name = format!(
                                "Item of {} in {}",
                                line.part,
                                BomTab::editable(line).unwrap_or("")
                            );
                            let shown = if text.is_empty() {
                                "–"
                            } else {
                                text.as_str()
                            };
                            let shown = match was {
                                Some(w) => RichText::new(format!("{shown} (was {w})"))
                                    .color(style::color32(style::CHANGED)),
                                None => RichText::new(shown),
                            };
                            let r = ui
                                .add(egui::Button::new(shown).frame(false))
                                .on_hover_text("Change this item number");
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, &name)
                            });
                            if r.clicked() {
                                start = Some(line.clone());
                            }
                        }
                        "part" => {
                            let r = ui.selectable_label(chosen, text.as_str());
                            let name = line_name(line, level);
                            r.widget_info(|| {
                                egui::WidgetInfo::selected(egui::Role::Button, true, chosen, &name)
                            });
                            if r.clicked() {
                                chose = Some(line.occurrences.clone());
                            }
                        }
                        _ if line.bought => {
                            ui.label(RichText::new(text).weak())
                                .on_hover_text("Inside an assembly bought whole");
                        }
                        _ => {
                            ui.label(text);
                        }
                    }
                }
                if let Some(p) = &line.problem {
                    ui.colored_label(error, p);
                }
                ui.end_row();
            }
        });
    });
    if let (Some(text), Some(e)) = (typed, ws.bom.editing.as_mut()) {
        e.text = text;
    }
    if let Some(line) = start {
        ws.bom.edit(&line);
    }
    if set {
        ws.save_item();
    }
    if cancel {
        ws.bom.editing = None;
        ws.bom.edit_error = None;
    }
    if let Some(occurrences) = chose {
        choose(ws, view, occurrences);
    }
}

/// The Part section's Where used: the assemblies in the working files
/// that reach its part (Show shows one), then every official release of
/// an assembly that does, with its state. Each says how many one holds,
/// and each way down to the part on hover.
pub fn where_used(ui: &mut egui::Ui, ws: &mut Workspace, view: &View) {
    let part = crate::ui::panels::section_part(ws, view);
    ws.show_where_used(part.as_deref());
    let Some(w) = &ws.where_used else {
        return;
    };
    ui.heading(format!("Where {} is used", w.part));
    if let Some(e) = &w.error {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
        return;
    }
    let mut show = None;
    if w.working.is_empty() {
        ui.weak("No assembly in the working files uses it");
    }
    for u in &w.working {
        ui.horizontal(|ui| {
            ui.label(u.text()).on_hover_text(u.paths.join("\n"));
            if ui.small_button(format!("Show {}", u.assembly)).clicked() {
                show = Some(u.assembly.clone());
            }
        });
    }
    ui.strong("Released");
    match &w.note {
        Some(n) => {
            ui.weak(n);
        }
        None if w.released.is_empty() => {
            ui.weak("No released assembly uses it");
        }
        None => {}
    }
    for u in &w.released {
        ui.label(u.text()).on_hover_text(u.paths.join("\n"));
    }
    for p in &w.problems {
        ui.colored_label(ui.visuals().warn_fg_color, p);
    }
    if let Some(a) = show {
        ws.set_focus(Some(a));
    }
}

/// Export CSV…, while `ws.bom_export` is open: where to write the BOM as
/// the tab shows it, indented or flat.
pub fn export(
    ctx: &egui::Context,
    ws: &mut Workspace,
    browse: &mut Option<crate::ui::step::Browse>,
    view: egui::Rect,
) {
    let root = ws.bom.root.clone().unwrap_or_default();
    let Some(e) = &mut ws.bom_export else {
        return;
    };
    let (mut open, mut go) = (true, false);
    crate::ui::dialogs::window("Export BOM as CSV", view)
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            let shape = if e.flat { "flat" } else { "indented" };
            ui.label(format!(
                "The {shape} BOM of {root}, as the working files have it"
            ));
            let how = crate::ui::step::Browse::SaveCsv;
            let entered = crate::ui::step::file_row(ui, "Save as", &mut e.path, browse, how);
            let typed = !e.path.trim().is_empty();
            go = ui
                .add_enabled(typed && !e.waiting, egui::Button::new("Export"))
                .clicked()
                || (entered && typed && !e.waiting);
            if e.waiting {
                ui.spinner();
            }
            if let Some(err) = &e.error {
                ui.colored_label(ui.visuals().error_fg_color, &err.message);
            }
            if let Some((path, n)) = &e.done {
                ui.label(format!("Wrote {n} bytes to {path}"));
            }
        });
    if go {
        ws.run_bom_export();
    }
    if !open {
        ws.bom_export = None;
    }
}

/// Chooses a line: its occurrences become this window's selection, and
/// the view frames them.
fn choose(ws: &mut Workspace, view: &mut View, occurrences: Vec<String>) {
    ws.bom.chosen = Some(occurrences.clone());
    ws.select(occurrences.clone(), SelectMode::Replace);
    view.fit_entities(ws, &occurrences);
}
