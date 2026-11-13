//! What each tile draws (`core::layout::TILES`): the panels that were
//! fixed round the viewport, now one module each.

pub mod activity;
pub mod annotations;
pub mod branches;
pub mod changes;
pub mod conflicts;
pub mod deps;
pub mod drawings;
pub mod equations;
pub mod explode;
pub mod history;
pub mod motion;
pub mod parameters;
pub mod part;
pub mod properties;
pub mod releases;
pub mod section;
pub mod tree;

use crate::ui::app::Opened;
use crate::ui::icon::{self, Icon};
use crate::ui::panels;
use eframe::egui;

/// What a tile with nothing to show says (design 199, Empty states): what
/// would fill it, how, and where an agent's CLI line fills it, that line
/// to copy. `more` adds a control (Releases' Set up PDM).
pub fn empty(
    ui: &mut egui::Ui,
    glyph: Icon,
    what: &str,
    how: &str,
    line: Option<&str>,
    more: impl FnOnce(&mut egui::Ui),
) {
    ui.vertical_centered(|ui| {
        ui.add_space(16.0);
        icon::show(ui, glyph, 28.0, ui.visuals().weak_text_color());
        ui.add_space(4.0);
        ui.label(what);
        ui.weak(how);
        if let Some(line) = line {
            ui.horizontal(|ui| {
                // Centred as one: the line and its Copy.
                let w = ui.fonts_mut(|f| {
                    f.layout_no_wrap(
                        line.to_owned(),
                        egui::FontId::monospace(12.0),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
                }) + 30.0;
                ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                crate::ui::feedback::chip(ui, line);
                if icon::button(ui, Icon::COPY, 14.0, "Copy", "Copy", false, Ok(())).clicked() {
                    ui.ctx().copy_text(line.to_owned());
                }
            });
        }
        more(ui);
    });
}

/// Draws tile `id`'s contents.
pub fn show(
    ui: &mut egui::Ui,
    o: &mut Opened,
    gpu: Option<&eframe::egui_wgpu::RenderState>,
    id: &str,
) {
    // A short tile still fills its room.
    ui.set_min_size(ui.available_size());
    // With the daemon lost nothing in a tile can be pressed; the viewport
    // keeps its wash and its camera.
    if o.ws.lost && id != "viewport" {
        ui.disable();
    }
    let part = o.ws.model.followed_part(o.view.chosen.as_ref());
    let scroll = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
        egui::ScrollArea::vertical()
            .id_salt(("tile", ui.scope_id()))
            .auto_shrink(false)
            .show(ui, |ui| add(ui));
    };
    // Tiles whose rows are wider than a narrow tile: they scroll across
    // too, rather than cut their controls off (#426).
    let scroll_both = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
        egui::ScrollArea::both()
            .id_salt(("tile", ui.scope_id()))
            .auto_shrink(false)
            .show(ui, |ui| add(ui));
    };
    match id {
        "viewport" => {
            // The compare bar stays above the viewport, inside its tile.
            // Not over a draft: the mode bar says what is drawn.
            if o.compare_with.is_some()
                || o.ws
                    .compare
                    .as_ref()
                    .is_some_and(|c| c.to.is_none() || c.swapped.is_some())
            {
                panels::compare_bar(ui, &mut o.ws, &mut o.compare_with);
                ui.separator();
            }
            o.view.show(ui, &mut o.ws, gpu);
            let p = o.view.palette(ui.ctx().theme() == egui::Theme::Dark);
            crate::ui::modes::frame(ui, &o.ws, o.view.rect, &p);
        }
        "tree" => tree::tree(ui, &mut o.ws, &mut o.view),
        "properties" => scroll(ui, &mut |ui| properties::properties(ui, &mut o.ws)),
        "parameters" => scroll(ui, &mut |ui| parameters::parameters(ui, &mut o.ws, &o.view)),
        "part" => scroll(ui, &mut |ui| {
            part::part_section(ui, &mut o.ws, &o.view);
            ui.separator();
            if part::sheet_section(ui, &mut o.ws, &o.view) {
                ui.separator();
            }
            crate::ui::bom::where_used(ui, &mut o.ws, &o.view);
        }),
        "conflicts" => scroll(ui, &mut |ui| conflicts::merge(ui, &mut o.ws, &mut o.view)),
        "changes" => scroll(ui, &mut |ui| {
            if o.ws.compare.is_none() {
                empty(
                    ui,
                    Icon::GIT_DIFF,
                    "Not comparing",
                    "View > Compare with HEAD shows what changed since the last commit.",
                    Some("ryucad compare HEAD"),
                    |_| {},
                );
            }
            changes::changes(ui, &o.ws, &o.view);
        }),
        "activity" => activity::activity(ui, o),
        "history" => history::history(ui, &mut o.ws),
        "releases" => {
            o.ws.open_releases();
            releases::releases(ui, &mut o.ws, part.as_deref());
        }
        "branches" => branches::branches(ui, &mut o.ws),
        "bom" => {
            o.ws.open_bom();
            crate::ui::bom::tab(ui, &mut o.ws, &mut o.view);
        }
        "deps" => scroll(ui, &mut |ui| deps::deps(ui, &mut o.ws)),
        "shortcuts" => crate::ui::shortcuts::show(ui, o),
        "section" => scroll_both(ui, &mut |ui| section::section(ui, &mut o.ws, &mut o.view)),
        "explode" => scroll_both(ui, &mut |ui| explode::explode(ui, &mut o.ws)),
        "motion" => scroll_both(ui, &mut |ui| motion::motion(ui, &mut o.ws, &mut o.view)),
        "equations" => scroll(ui, &mut |ui| equations::equations(ui, &mut o.ws)),
        "drawings" => drawings::drawings(ui, &mut o.ws, &mut o.view),
        "annotations" => scroll(ui, &mut |ui| {
            annotations::annotations(ui, &mut o.ws, &mut o.view)
        }),
        _ => {}
    }
}
