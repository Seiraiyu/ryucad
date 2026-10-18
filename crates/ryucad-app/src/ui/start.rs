//! The start window: recent projects with thumbnails, and Open. A folder
//! that isn't a project is offered as a new one.

use crate::core::link::Wake;
use crate::core::recent::{self, Thumbnails};
use eframe::egui;
use ryucad::client::Daemon;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What the start window asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Open the project in or above this folder.
    Open(PathBuf),
    /// Make this folder a project, then open it.
    Create(PathBuf),
}

pub struct Start {
    pub recent: Vec<PathBuf>,
    pub thumbnails: Thumbnails,
    textures: BTreeMap<PathBuf, egui::TextureHandle>,
    /// A folder that was opened but isn't a project.
    pub not_a_project: Option<PathBuf>,
    pub error: Option<String>,
}

impl Start {
    pub fn new(daemon: &Daemon, wake: &Wake) -> Start {
        let recent = recent::load();
        let mut thumbnails = Thumbnails::default();
        thumbnails.request(daemon, &recent, wake);
        Start {
            recent,
            thumbnails,
            textures: BTreeMap::new(),
            not_a_project: None,
            error: None,
        }
    }

    fn texture(&mut self, ctx: &egui::Context, root: &PathBuf) -> Option<egui::TextureHandle> {
        if let Some(t) = self.textures.get(root) {
            return Some(t.clone());
        }
        let Some(Ok((w, h, px))) = self.thumbnails.done.get(root) else {
            return None;
        };
        let image = egui::ColorImage::from_rgba_unmultiplied([*w as usize, *h as usize], px);
        let name = format!("thumbnail {}", root.display());
        let t = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
        self.textures.insert(root.clone(), t.clone());
        Some(t)
    }
}

/// Asks the OS for a folder.
pub fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Open a ryucad project folder")
        .pick_folder()
}

pub fn show(ui: &mut egui::Ui, start: &mut Start) -> Option<Action> {
    start.thumbnails.poll();
    let mut action = None;
    egui::CentralPanel::default().show(ui, |ui| {
        ui.heading("ryucad");
        ui.label("A ryucad project is a folder. Open one to watch, point at and adjust what agents build.");
        if ui.button("Open…").clicked()
            && let Some(dir) = pick_folder()
        {
            action = Some(Action::Open(dir));
        }
        if let Some(e) = &start.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        if let Some(dir) = start.not_a_project.clone() {
            ui.group(|ui| {
                ui.label(format!("{} isn't a ryucad project.", dir.display()));
                ui.horizontal(|ui| {
                    if ui.button("Create a project here").clicked() {
                        action = Some(Action::Create(dir.clone()));
                    }
                    if ui.button("Choose another folder…").clicked()
                        && let Some(d) = pick_folder()
                    {
                        action = Some(Action::Open(d));
                    }
                });
            });
        }
        ui.separator();
        ui.heading("Recent projects");
        if start.recent.is_empty() {
            ui.weak("None yet");
        }
        let size = egui::vec2(recent::SIZE.0 as f32, recent::SIZE.1 as f32);
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Rows as wide as the window allows.
            let cell = size.x + ui.spacing().item_spacing.x;
            let per_row = ((ui.available_width() + ui.spacing().item_spacing.x) / cell).floor().max(1.0) as usize;
            let recent = start.recent.clone();
            let names = recent::names(&recent);
            let all: Vec<(PathBuf, String)> = recent.into_iter().zip(names).collect();
            for row in all.chunks(per_row) {
                ui.horizontal(|ui| {
                    for (root, name) in row {
                        if thumbnail(ui, start, root, name, size) {
                            action = Some(Action::Open(root.clone()));
                        }
                    }
                });
            }
        });
    });
    action
}

/// One recent project: its thumbnail and name. Returns whether it was
/// clicked.
fn thumbnail(
    ui: &mut egui::Ui,
    start: &mut Start,
    root: &PathBuf,
    name: &str,
    size: egui::Vec2,
) -> bool {
    let texture = start.texture(ui.ctx(), root);
    ui.vertical(|ui| {
        ui.set_width(size.x);
        // The same space whether the thumbnail is there yet or not, so
        // nothing moves under the mouse when it comes.
        let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
        match texture {
            Some(t) => {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().image(t.id(), rect, uv, egui::Color32::WHITE);
            }
            None => {
                ui.painter()
                    .rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
            }
        }
        let clicked = r.on_hover_text(root.display().to_string()).clicked();
        ui.link(name).clicked() || clicked
    })
    .inner
}
