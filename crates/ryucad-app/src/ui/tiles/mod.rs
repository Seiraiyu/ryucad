//! What each tile draws (`core::layout::TILES`): the panels that were
//! fixed round the viewport, now one module each.

pub mod activity;
pub mod branches;
pub mod changes;
pub mod conflicts;
pub mod history;
pub mod parameters;
pub mod part;
pub mod properties;
pub mod releases;
pub mod tree;

use crate::ui::app::Opened;
use crate::ui::panels;
use eframe::egui;

/// Draws tile `id`'s contents.
pub fn show(
    ui: &mut egui::Ui,
    o: &mut Opened,
    gpu: Option<&eframe::egui_wgpu::RenderState>,
    id: &str,
) {
    // A short tile still fills its room.
    ui.set_min_size(ui.available_size());
    let part = o
        .view
        .chosen
        .as_ref()
        .map(|k| k.0.clone())
        .or_else(|| o.ws.model.parts.keys().next().cloned());
    let scroll = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
        egui::ScrollArea::vertical()
            .id_salt(("tile", ui.scope_id()))
            .auto_shrink(false)
            .show(ui, |ui| add(ui));
    };
    match id {
        "viewport" => {
            // The compare bar stays above the viewport, inside its tile.
            if o.compare_with.is_some() || o.ws.compare.is_some() {
                panels::compare_bar(ui, &mut o.ws, &mut o.compare_with);
                ui.separator();
            }
            o.view.show(ui, &mut o.ws, gpu);
        }
        "tree" => scroll(ui, &mut |ui| tree::tree(ui, &mut o.ws, &mut o.view)),
        "properties" => scroll(ui, &mut |ui| properties::properties(ui, &mut o.ws)),
        "parameters" => scroll(ui, &mut |ui| parameters::parameters(ui, &mut o.ws, &o.view)),
        "part" => scroll(ui, &mut |ui| {
            part::part_section(ui, &mut o.ws, &o.view);
            ui.separator();
            crate::ui::bom::where_used(ui, &mut o.ws, &o.view);
        }),
        "conflicts" => scroll(ui, &mut |ui| conflicts::merge(ui, &mut o.ws, &mut o.view)),
        "changes" => scroll(ui, &mut |ui| {
            if o.ws.compare.is_none() {
                ui.weak("Not comparing: View > Compare with HEAD shows what changed.");
            }
            changes::changes(ui, &o.ws, &o.view);
        }),
        "activity" => activity::activity(ui, &mut o.ws),
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
        "shortcuts" => crate::ui::shortcuts::show(ui, o),
        _ => {}
    }
}
