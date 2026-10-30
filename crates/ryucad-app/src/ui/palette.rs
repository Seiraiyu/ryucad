//! The command palette (design 199, Command registry > Palette): Mod+K
//! or Mod+Shift+P opens a field over the window with the commands that
//! match what is typed, recent ones first, each with its icon and keys,
//! and its reason greyed when it can't run. Typing `@` searches the model
//! instead; Enter goes to what is chosen and frames it, as choosing it in
//! the tree does. Up and Down move, Enter runs, Esc closes.

use crate::core::command::{self, Command};
use crate::core::palette::{self, Place};
use crate::ui::app::Opened;
use crate::ui::icon;
use crate::ui::menu::Action;
use eframe::egui;

/// The palette while it is open.
#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub query: String,
    /// The highlighted row.
    pub at: usize,
    /// The field has been given the keyboard.
    focused: bool,
}

/// The palette's rows now.
enum Rows {
    Commands(Vec<&'static Command>),
    Places(Vec<Place>),
}

impl Rows {
    fn len(&self) -> usize {
        match self {
            Rows::Commands(c) => c.len(),
            Rows::Places(p) => p.len(),
        }
    }
}

fn rows(o: &Opened, query: &str) -> Rows {
    match query.strip_prefix('@') {
        Some(q) => {
            let pdm = palette::pdm_names(&o.ws.bom);
            Rows::Places(palette::places(q, &o.ws.model, &pdm))
        }
        None => Rows::Commands(palette::commands(query, &o.recent_commands)),
    }
}

/// How many rows show at once.
const SHOWN: usize = 12;

/// Draws the palette if it is open; Enter runs, Esc or a click outside
/// closes.
pub fn show(ctx: &egui::Context, o: &mut Opened) -> Option<Action> {
    let mut p = o.palette.take()?;
    let rows = rows(o, &p.query);
    let (mut go, mut close) = (false, false);
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
            p.at = (p.at + 1).min(rows.len().saturating_sub(1));
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
            p.at = p.at.saturating_sub(1);
        }
        go = i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
        close = i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
    });
    let view = o.view.rect;
    let width = 600.0_f32.min(view.width() - 20.0).max(240.0);
    let mut clicked = None;
    let area = egui::Area::new(egui::Id::unique("command palette"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(view.center().x - width / 2.0, view.top() + 8.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    icon::show(ui, icon::Icon::COMMAND, 16.0, ui.visuals().weak_text_color());
                    let edit = egui::TextEdit::singleline(&mut p.query)
                        .hint_text("Type a command, or @ to find a part or feature")
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY);
                    let r = ui.add(edit);
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Search commands")
                    });
                    if !p.focused {
                        r.request_focus();
                        p.focused = true;
                    }
                    if r.changed() {
                        p.at = 0;
                    }
                });
                ui.separator();
                let shown = &o.view.commands;
                let first = p.at.saturating_sub(SHOWN - 1);
                match &rows {
                    Rows::Commands(list) => {
                        for (i, c) in list.iter().enumerate().skip(first).take(SHOWN) {
                            let enabled = command::enabled(c.id, &shown.state);
                            let mut text = format!("{}  {}", c.category.title(), c.title);
                            if let Err(why) = enabled {
                                text.push_str(" — ");
                                text.push_str(why);
                            }
                            let r = ui.horizontal(|ui| {
                                let colour = if enabled.is_ok() {
                                    ui.visuals().text_color()
                                } else {
                                    ui.visuals().weak_text_color()
                                };
                                match c.icon.and_then(crate::ui::toolbar::icon_of) {
                                    Some(ic) => {
                                        icon::show(ui, ic, 16.0, colour);
                                    }
                                    None => {
                                        ui.add_space(16.0 + ui.spacing().item_spacing.x);
                                    }
                                }
                                let r = ui.add_enabled(
                                    enabled.is_ok(),
                                    egui::Button::selectable(i == p.at, egui::RichText::new(text).color(colour))
                                        .shortcut_text(shown.keys.get(c.id).cloned().unwrap_or_default()),
                                );
                                if r.clicked() {
                                    clicked = Some(i);
                                }
                            });
                            let _ = r;
                        }
                        if list.is_empty() {
                            ui.weak("No command matches");
                        }
                    }
                    Rows::Places(list) => {
                        for (i, place) in list.iter().enumerate().skip(first).take(SHOWN) {
                            if ui.selectable_label(i == p.at, &place.label).clicked() {
                                clicked = Some(i);
                            }
                        }
                        if list.is_empty() {
                            ui.weak("Nothing in the model matches");
                        }
                    }
                }
                ui.separator();
                ui.weak("↑ ↓ move · Enter run · Esc close · @ finds parts, features, solids and instances");
            });
        });
    if area.response.clicked_elsewhere() && clicked.is_none() {
        close = true;
    }
    let pick = clicked.or(go.then_some(p.at));
    if close && pick.is_none() {
        return None;
    }
    let Some(i) = pick else {
        o.palette = Some(p);
        return None;
    };
    match rows {
        Rows::Commands(list) => {
            let c = list.get(i)?;
            if command::enabled(c.id, &o.view.commands.state).is_err() {
                o.palette = Some(p);
                return None;
            }
            palette::ran(&mut o.recent_commands, c.id);
            crate::ui::commands::run(o, ctx, c.id, None)
        }
        Rows::Places(list) => {
            let place = list.get(i)?;
            go_to(o, place);
            None
        }
    }
}

/// Chooses and frames what `place` names, as choosing it in the tree
/// does: a feature opens its form, a part is shown.
fn go_to(o: &mut Opened, place: &Place) {
    match &place.feature {
        Some(f) => {
            let key = (place.part.clone(), f.clone());
            o.view.chosen = Some(key.clone());
            o.ws.open_form(Some(key.clone()));
            o.ws.choose(Some(key.clone()));
            let bodies = o.ws.model.framed(&key);
            o.view.fit_all(&mut o.ws, &bodies);
        }
        None => o.ws.set_focus(Some(place.part.clone())),
    }
}
