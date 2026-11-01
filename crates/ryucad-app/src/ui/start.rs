//! The start window (design 199, Start screen; `design/app/start.html`):
//! recent projects as cards with their thumbnails (`core::thumbs`; a
//! missing one shows the project's initial), Open…, New project… and
//! Open sample…, and the card on driving ryucad from an agent. A folder
//! that isn't a project is offered as a new one, and a typed folder path
//! opens without the OS's dialog (no desktop portal on Linux).

use crate::core::thumbs::{self, Thumbnail};
use crate::ui::icon::{self, Icon};
use crate::ui::style::{self, Contrast, Look, Palette};
use eframe::egui::{self, RichText};
use ryucad::error::Error;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What the start window asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Open the project in or above this folder.
    Open(PathBuf),
    /// Make this folder a project, then open it.
    Create(PathBuf),
    /// Ask the OS for a folder, then do this with it.
    Pick(Pick),
    Quit,
}

/// What a folder the OS gives is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Open the project in it.
    Open,
    /// Make it a new project (it must be empty).
    New,
    /// Write the sample into it and open that (it must be empty).
    Sample,
}

impl Pick {
    /// The OS dialog's title.
    pub fn title(self) -> &'static str {
        match self {
            Pick::Open => "Open a ryucad project folder",
            Pick::New => "Choose an empty folder for the new project",
            Pick::Sample => "Choose an empty folder for the sample project",
        }
    }
}

/// The widths the mockup gives the agent card, the gap beside it and the
/// screen's margins, in points.
const AGENT_WIDTH: f32 = 380.0;
const GAP: f32 = 32.0;
const MARGIN: [i8; 2] = [56, 40];
/// A card's thumbnail is this tall; cards come three to a row.
const THUMB_HEIGHT: f32 = 150.0;
const PER_ROW: usize = 3;

pub struct Start {
    pub recent: Vec<PathBuf>,
    /// Each recent project's thumbnail, read when the screen first shows
    /// (`None`: no thumbnail, the initial shows).
    pub thumbnails: BTreeMap<PathBuf, Option<Thumbnail>>,
    textures: BTreeMap<PathBuf, egui::TextureHandle>,
    /// A folder that was opened but isn't a project.
    pub not_a_project: Option<PathBuf>,
    /// Why the last thing asked of the screen failed, shown as a card.
    pub error: Option<Error>,
    /// A folder typed or pasted in: for when the OS has no folder dialog
    /// (no desktop portal on Linux), or the path is at hand.
    pub typed: String,
    look: Look,
    contrast: Contrast,
}

impl Start {
    pub fn new() -> Start {
        let user = |k: &str| {
            ryucad::config::user(k)
                .as_str()
                .unwrap_or_default()
                .to_owned()
        };
        Start {
            recent: crate::core::recent::load(),
            thumbnails: BTreeMap::new(),
            textures: BTreeMap::new(),
            not_a_project: None,
            error: None,
            typed: String::new(),
            look: Look::parse(&user("ui.style")),
            contrast: Contrast::parse(&user("ui.contrast")),
        }
    }

    /// Reads every recent project's thumbnail not read yet: files in the
    /// cache folder, made by the windows that had them open.
    fn read_thumbnails(&mut self) {
        let Some(folder) = thumbs::folder() else {
            return;
        };
        for root in &self.recent {
            self.thumbnails
                .entry(root.clone())
                .or_insert_with(|| thumbs::read(&thumbs::path_in(&folder, root)));
        }
    }

    fn texture(&mut self, ctx: &egui::Context, root: &PathBuf) -> Option<egui::TextureHandle> {
        if let Some(t) = self.textures.get(root) {
            return Some(t.clone());
        }
        let t = self.thumbnails.get(root)?.as_ref()?;
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [t.size.0 as usize, t.size.1 as usize],
            &t.rgba,
        );
        let name = format!("thumbnail {}", root.display());
        let t = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
        self.textures.insert(root.clone(), t.clone());
        Some(t)
    }
}

impl Default for Start {
    fn default() -> Start {
        Start::new()
    }
}

/// Asks the OS for a folder, in a dialog of the window `parent`: when it
/// closes the window has the focus again, so the next click isn't spent
/// on bringing it back.
pub fn pick_folder(parent: &eframe::Frame, why: Pick) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(why.title())
        .set_parent(parent)
        .pick_folder()
}

/// A path as a card writes it: the home folder as `~`.
pub fn tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home.map(PathBuf::from) {
        Some(h) if !h.as_os_str().is_empty() => match path.strip_prefix(&h) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        _ => path.display().to_string(),
    }
}

pub fn show(ui: &mut egui::Ui, start: &mut Start) -> Option<Action> {
    start.read_thumbnails();
    let p = start.look.colours(ui.visuals().dark_mode, start.contrast);
    let mut action = None;
    egui::Panel::top("start menu").show(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                for (title, a) in [
                    ("Open…", Action::Pick(Pick::Open)),
                    ("New project…", Action::Pick(Pick::New)),
                    ("Open sample…", Action::Pick(Pick::Sample)),
                    ("Quit", Action::Quit),
                ] {
                    if ui.button(title).clicked() {
                        action = Some(a);
                    }
                }
            });
        });
    });
    egui::Panel::bottom("start status").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.weak(format!(
                "ryucad {}",
                ryucad::proto::VersionInfo::current().ryucad
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let look = match start.contrast {
                    Contrast::High => "High contrast",
                    Contrast::Normal => start.look.title(),
                };
                let theme = if ui.visuals().dark_mode {
                    "dark"
                } else {
                    "light"
                };
                ui.weak(format!("{look} · {theme}"));
            });
        });
    });
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(style::rgb(p.app))
                .inner_margin(egui::Margin::symmetric(MARGIN[0], MARGIN[1])),
        )
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let left = (ui.available_width() - AGENT_WIDTH - GAP).max(320.0);
                ui.vertical(|ui| {
                    ui.set_width(left);
                    projects(ui, start, &p, &mut action);
                });
                ui.add_space(GAP);
                ui.vertical(|ui| {
                    ui.set_width(AGENT_WIDTH);
                    agent_card(ui, start, &p);
                });
            });
        });
    action
}

/// The left column: the title, the buttons, a folder that isn't a
/// project, an error, the folder path and the recent projects.
fn projects(ui: &mut egui::Ui, start: &mut Start, p: &Palette, action: &mut Option<Action>) {
    ui.label(RichText::new("ryucad").size(22.0).strong());
    ui.weak("CAD an agent drives, and you watch, point, judge and nudge.");
    ui.add_space(20.0);
    ui.horizontal(|ui| {
        let buttons = [
            (Icon::FOLDER_OPEN, "Open…", Pick::Open, true),
            (Icon::FOLDER_SIMPLE_PLUS, "New project…", Pick::New, false),
            (Icon::PACKAGE, "Open sample…", Pick::Sample, false),
        ];
        for (glyph, title, why, primary) in buttons {
            if labelled(ui, p, glyph, title, primary).clicked() {
                *action = Some(Action::Pick(why));
            }
        }
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let name = ui.weak("Folder path");
        let field = ui
            .add(egui::TextEdit::singleline(&mut start.typed).desired_width(420.0))
            .labelled_by(name.id);
        let typed = start.typed.trim();
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui
            .add_enabled(!typed.is_empty(), egui::Button::new("Open folder"))
            .clicked()
            || entered)
            && !typed.is_empty()
        {
            *action = Some(Action::Open(PathBuf::from(typed)));
        }
    });
    if let Some(e) = start.error.clone()
        && error_card(ui, p, &e)
    {
        start.error = None;
    }
    if let Some(dir) = start.not_a_project.clone() {
        ui.add_space(8.0);
        card_frame(p).show(ui, |ui| {
            ui.label(format!("{} isn't a ryucad project.", dir.display()));
            ui.horizontal(|ui| {
                if ui.button("Create a project here").clicked() {
                    *action = Some(Action::Create(dir.clone()));
                }
                if ui.button("Choose another folder…").clicked() {
                    *action = Some(Action::Pick(Pick::Open));
                }
            });
        });
    }
    ui.add_space(28.0);
    ui.label(
        RichText::new("RECENT PROJECTS")
            .size(11.0)
            .strong()
            .color(style::rgb(p.text_weak)),
    );
    ui.add_space(10.0);
    if start.recent.is_empty() {
        ui.weak("None yet: open a project, start a new one, or open the sample.");
        return;
    }
    let recent = start.recent.clone();
    let names = crate::core::recent::names(&recent);
    let spacing = 16.0;
    let width = ((ui.available_width() - spacing * (PER_ROW - 1) as f32) / PER_ROW as f32).floor();
    egui::ScrollArea::vertical()
        .id_salt("recent projects")
        .auto_shrink(false)
        .show(ui, |ui| {
            let all: Vec<(PathBuf, String)> = recent.into_iter().zip(names).collect();
            for row in all.chunks(PER_ROW) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = spacing;
                    for (root, name) in row {
                        if card(ui, start, p, root, name, width) {
                            *action = Some(Action::Open(root.clone()));
                        }
                    }
                });
                ui.add_space(spacing);
            }
        });
}

/// A button with an icon before its title, the accent filling the
/// primary one, named by its title.
fn labelled(
    ui: &mut egui::Ui,
    p: &Palette,
    glyph: Icon,
    title: &str,
    primary: bool,
) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let (fg, fill, stroke) = if primary {
        // White on the accent; high contrast's yellow takes black.
        let fg = if p.select_fg == 0x000000 {
            0x000000
        } else {
            0xffffff
        };
        (style::rgb(fg), style::rgb(p.accent), style::rgb(p.accent))
    } else {
        (
            style::rgb(p.text),
            style::rgb(p.frame),
            style::rgb(p.border_strong),
        )
    };
    let galley = ui.painter().layout_no_wrap(title.to_owned(), font, fg);
    let pad = egui::vec2(12.0, 0.0);
    let size = egui::vec2(
        pad.x * 2.0 + 14.0 + 6.0 + galley.size().x,
        ui.spacing().interact_size.y,
    );
    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, title));
    if ui.is_rect_visible(rect) {
        let radius = ui.visuals().widgets.inactive.corner_radius;
        let fill = if r.hovered() && !primary {
            style::rgb(p.frame_hover)
        } else {
            fill
        };
        ui.painter().rect(
            rect,
            radius,
            fill,
            egui::Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
        let icon = egui::Rect::from_center_size(
            egui::pos2(rect.left() + pad.x + 7.0, rect.center().y),
            egui::Vec2::splat(14.0),
        );
        icon::paint(ui.painter(), icon, glyph, fg);
        let at = egui::pos2(icon.right() + 6.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(at, galley, fg);
        if r.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(2.0),
                radius,
                egui::Stroke::new(2.0, style::rgb(p.focus)),
                egui::StrokeKind::Outside,
            );
        }
    }
    r
}

/// The frame of a card: raised, a strong border, the look's corners.
fn card_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(style::rgb(p.raised))
        .stroke(egui::Stroke::new(1.0, style::rgb(p.border_strong)))
        .inner_margin(egui::Margin::symmetric(12, 10))
}

/// What failed, as an error card: its title, code and message. Returns
/// whether it was closed.
fn error_card(ui: &mut egui::Ui, p: &Palette, e: &Error) -> bool {
    let mut close = false;
    ui.add_space(8.0);
    card_frame(p).show(ui, |ui| {
        ui.horizontal(|ui| {
            icon::show(ui, Icon::X_CIRCLE, 18.0, style::rgb(p.danger));
            let title = crate::core::feedback::title(e.code);
            ui.label(RichText::new(title).strong().color(style::rgb(p.danger)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = icon::button(ui, Icon::X, 14.0, "Close", "Close", false, Ok(())).clicked();
                let code = serde_json::to_value(e.code).unwrap_or_default();
                crate::ui::feedback::chip(ui, code.as_str().unwrap_or_default());
            });
        });
        ui.label(&e.message);
    });
    close
}

/// One recent project's card: its thumbnail (cut to the card's shape) or
/// initial, its name, path and when it was last open. Returns whether it
/// was clicked.
fn card(
    ui: &mut egui::Ui,
    start: &mut Start,
    p: &Palette,
    root: &PathBuf,
    name: &str,
    width: f32,
) -> bool {
    let texture = start.texture(ui.ctx(), root);
    let written = start
        .thumbnails
        .get(root)
        .and_then(Option::as_ref)
        .map(|t| t.written);
    // The name, the path and when: three lines under the thumbnail.
    let meta = 64.0;
    let (rect, r) =
        ui.allocate_exact_size(egui::vec2(width, THUMB_HEIGHT + meta), egui::Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect.expand(1.0));
        let radius = ui.visuals().widgets.inactive.corner_radius;
        let border = if r.hovered() || r.has_focus() {
            p.focus
        } else {
            p.border_strong
        };
        painter.rect_filled(rect, radius, style::rgb(p.raised));
        let thumb = egui::Rect::from_min_size(rect.min, egui::vec2(width, THUMB_HEIGHT));
        match texture {
            Some(t) => {
                // The middle of the render, at the card's shape.
                let image = t.size_vec2();
                let scale = (thumb.width() / image.x).max(thumb.height() / image.y);
                let shown = thumb.size() / scale / image;
                let uv = egui::Rect::from_center_size(egui::pos2(0.5, 0.5), shown);
                painter.image(t.id(), thumb, uv, egui::Color32::WHITE);
            }
            None => {
                painter.rect_filled(thumb, 0.0, style::rgb(p.viewport));
                painter.text(
                    thumb.center(),
                    egui::Align2::CENTER_CENTER,
                    thumbs::initial(name),
                    egui::FontId::proportional(44.0),
                    style::rgb(p.text_weak),
                );
            }
        }
        let x = rect.left() + 10.0;
        let mut y = thumb.bottom() + 8.0;
        let text = |painter: &egui::Painter, y: &mut f32, s: String, font: egui::FontId, c| {
            let g = painter.layout(s, font, c, width - 20.0);
            let h = g.size().y;
            painter.galley(egui::pos2(x, *y), g, c);
            *y += h + 1.0;
        };
        text(
            &painter,
            &mut y,
            name.to_owned(),
            egui::FontId::proportional(13.0),
            style::rgb(p.text),
        );
        text(
            &painter,
            &mut y,
            tilde(root),
            egui::FontId::monospace(11.0),
            style::rgb(p.text_weak),
        );
        if let Some(w) = written {
            text(
                &painter,
                &mut y,
                thumbs::opened(w, SystemTime::now()),
                egui::FontId::proportional(12.0),
                style::rgb(p.text_weak),
            );
        }
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0, style::rgb(border)),
            egui::StrokeKind::Inside,
        );
    }
    r.on_hover_text(root.display().to_string()).clicked()
}

/// The card on driving ryucad from an agent: the lines to give one for
/// the most recent project (`ui::help::agent_lines`), with Copy.
fn agent_card(ui: &mut egui::Ui, start: &Start, p: &Palette) {
    let root = start.recent.first();
    let lines = root.map_or_else(
        || "cd <your project folder>\nryucad rebuild\nryucad --help".to_owned(),
        |r| crate::ui::help::agent_lines(Path::new(&tilde(r))),
    );
    card_frame(p).show(ui, |ui| {
        ui.set_width(AGENT_WIDTH - 24.0);
        ui.horizontal(|ui| {
            icon::show(ui, Icon::TERMINAL_WINDOW, 18.0, style::rgb(p.accent));
            ui.label(RichText::new("Drive ryucad from an agent").strong());
        });
        let name = root
            .and_then(|r| crate::core::recent::names(std::slice::from_ref(r)).pop())
            .unwrap_or_else(|| "your project".to_owned());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(
                "Agents model through ryucad's CLI; this window shows what they do as they do \
                 it. Give your agent these lines for ",
            );
            ui.label(RichText::new(&name).strong());
            ui.label(":");
        });
        egui::Frame::new()
            .fill(style::rgb(p.frame))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&lines).monospace().size(12.0));
            });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if labelled(ui, p, Icon::COPY, "Copy", false).clicked() {
                ui.ctx().copy_text(lines.clone());
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_write_the_home_folder_as_a_tilde() {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        let Some(home) = home.map(PathBuf::from) else {
            return;
        };
        assert_eq!(
            tilde(&home.join("work").join("bracket")),
            format!("~/{}", Path::new("work").join("bracket").display())
        );
        assert_eq!(tilde(&home), "~");
        let elsewhere = Path::new("/opt/projects/gearbox");
        if !elsewhere.starts_with(&home) {
            assert_eq!(tilde(elsewhere), elsewhere.display().to_string());
        }
    }
}
