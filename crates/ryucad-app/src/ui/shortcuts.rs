//! The Shortcuts tile (design 199, Help menu > Keyboard shortcuts): every
//! command by category with its keys, a search, each command's keys
//! rebound by pressing a chord, and Reset per command and for all. A
//! chord another command has where their scopes meet is refused before
//! anything is written, naming that command. What it writes is the
//! `ui.keys` preference, through `config.set`.

use crate::core::command::{self, Category, Scope};
use crate::core::keys::Chord;
use crate::ui::app::Opened;
use eframe::egui;
use serde_json::json;

/// The tile's own state.
#[derive(Clone, Debug, Default)]
pub struct Shortcuts {
    pub search: String,
    /// The command whose next chord is being pressed.
    pub capturing: Option<&'static str>,
    /// Why the last chord pressed was refused.
    pub refused: Option<String>,
}

/// Whether a command matches the search, by title, id or keys' text.
fn matches(search: &str, c: &command::Command, keys: &str) -> bool {
    let s = search.trim().to_lowercase();
    s.is_empty()
        || c.title.to_lowercase().contains(&s)
        || c.id.contains(&s)
        || keys.to_lowercase().contains(&s)
}

/// What the press of `key` with `modifiers` does while `id` waits for
/// its chord: Esc cancels, Backspace unbinds, anything else binds that
/// chord or is refused.
fn pressed(
    o: &mut Opened,
    ctx: &egui::Context,
    id: &'static str,
    key: egui::Key,
    m: egui::Modifiers,
) {
    let s = &mut o.shortcuts;
    if key == egui::Key::Escape && !m.any() {
        s.capturing = None;
        return;
    }
    let mac = crate::ui::commands::mac(ctx);
    let chords = if key == egui::Key::Backspace && !m.any() {
        Vec::new()
    } else {
        vec![Chord::pressed(key, m, mac)]
    };
    match o.keymap.rebind(id, &chords, mac) {
        Ok(keys) => {
            o.ws.set_config("ui.keys", keys);
            o.shortcuts.capturing = None;
            o.shortcuts.refused = None;
        }
        Err(c) => {
            let title = command::get(c.with).map_or(c.with, |x| x.title);
            let text = ctx.format_shortcut(&c.chord.shortcut(mac));
            o.shortcuts.refused = Some(format!(
                "{text} is already {title} ({}). Choose another chord, or unbind that one first.",
                c.with
            ));
        }
    }
}

/// The tile.
pub fn show(ui: &mut egui::Ui, o: &mut Opened) {
    // A chord pressed for the command waiting for one.
    if let Some(id) = o.shortcuts.capturing {
        let press = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        if let Some((key, m)) = press {
            pressed(o, ui.ctx(), id, key, m);
        }
    }
    let style = o.ws.settings.text("ui.navigation", "ryucad").to_owned();
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut o.shortcuts.search)
                .hint_text("Find a command or a key")
                .desired_width(240.0),
        );
        r.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::SearchInput, true, "Search shortcuts")
        });
        ui.weak(format!(
            "Navigation keys follow {style} (Preferences › Navigation)"
        ));
        let any = o.keymap.all().any(|(id, _)| o.keymap.overridden(id));
        if ui
            .add_enabled(any, egui::Button::new("Reset all"))
            .clicked()
        {
            o.ws.set_config("ui.keys", json!({}));
            o.shortcuts.refused = None;
        }
    });
    ui.separator();
    let mut reset = None;
    let mut capture = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let groups: Vec<(String, Vec<&'static command::Command>)> = Category::ALL
                .iter()
                .map(|cat| {
                    let list = command::COMMANDS
                        .iter()
                        .filter(|c| {
                            c.category == *cat
                                && c.scope == Scope::Window
                                && c.kind != command::Kind::Choices
                        })
                        .collect();
                    (cat.title().to_uppercase(), list)
                })
                .chain(std::iter::once((
                    Scope::Viewport.title().to_uppercase(),
                    command::COMMANDS
                        .iter()
                        .filter(|c| c.scope == Scope::Viewport)
                        .collect(),
                )))
                .collect();
            for (heading, list) in groups {
                let rows: Vec<(&command::Command, String)> = list
                    .into_iter()
                    .map(|c| {
                        (
                            c,
                            crate::ui::commands::keys_text(ui.ctx(), &o.keymap, c.id)
                                .unwrap_or_default(),
                        )
                    })
                    .filter(|(c, k)| matches(&o.shortcuts.search, c, k))
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.weak(heading);
                egui::Grid::new(("shortcuts", rows[0].0.id))
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        for (c, keys) in rows {
                            ui.label(c.title);
                            ui.monospace(egui::RichText::new(c.id).weak());
                            let waiting = o.shortcuts.capturing == Some(c.id);
                            let text = if waiting {
                                "Press a chord… Esc cancels · Backspace unbinds".to_owned()
                            } else if keys.is_empty() {
                                "none".to_owned()
                            } else {
                                keys
                            };
                            let r = ui.add(egui::Button::selectable(
                                waiting,
                                egui::RichText::new(text).monospace(),
                            ));
                            let name = format!("Keys of {}", c.title);
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, &name)
                            });
                            if r.clicked() {
                                capture = Some(c.id);
                            }
                            if o.keymap.overridden(c.id) {
                                if ui.small_button("Reset").clicked() {
                                    reset = Some(c.id);
                                }
                            } else {
                                ui.label("");
                            }
                            ui.end_row();
                            if waiting && let Some(why) = &o.shortcuts.refused {
                                ui.label("");
                                ui.colored_label(ui.visuals().error_fg_color, why);
                                ui.end_row();
                            }
                        }
                    });
            }
        });
    if let Some(id) = capture {
        o.shortcuts.capturing = Some(id);
        o.shortcuts.refused = None;
    }
    if let Some(id) = reset {
        let keys = o.keymap.reset(id);
        o.ws.set_config("ui.keys", keys);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_reads_titles_ids_and_keys() {
        let fit = command::get("view.fit").unwrap();
        assert!(matches("zoom", fit, "F"));
        assert!(matches("view.fit", fit, "F"));
        assert!(matches("", fit, ""));
        assert!(matches(
            "ctrl+k",
            command::get("edit.undo").unwrap(),
            "Ctrl+K"
        ));
        assert!(!matches("undo", fit, "F"));
    }
}
