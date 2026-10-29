//! Running the registry's commands (`core::command`) against the window:
//! what each one's [`State`] reads, what [`run`] does, the choices a
//! submenu lists, and the keymap's presses ([`keys`]). The menus, the
//! toolbar, the context menus and the palette all come here, so a
//! command does the same thing however it is reached.

use crate::core::command::{self, Context, State};
use crate::core::keys::{Chord, Keymap};
use crate::ui::app::Opened;
use crate::ui::menu::Action;
use eframe::egui;
use ryucad::live::SelectMode;
use ryucad::view::StandardView;
use serde_json::json;

/// What the commands' availability reads now.
pub fn state(o: &mut Opened) -> State {
    let chosen_import = o.view.chosen.as_ref().is_some_and(|(p, f)| {
        o.ws.model
            .parts
            .get(p)
            .and_then(|p| p.feature(f))
            .is_some_and(|f| f.import.is_some())
    });
    State {
        selected: o.ws.myself().map_or(0, |m| m.selection.len()),
        chosen: o.view.chosen.is_some(),
        chosen_import,
        one_shown: o.ws.model.shown().len() == 1,
        assembly_shown: o.ws.shown_assembly().is_some(),
        planar_face: o.view.selected_face(&o.ws).is_some(),
        weighable: o.ws.mass_target().is_some(),
        comparing: o.ws.compare.as_ref().map(|c| c.rev.clone()),
        edges: o.view.edges,
        sketches: o.view.sketches,
        nav_cube: o.view.cube,
        triad: o.view.triad,
        origin: o.view.origin,
        measuring: o.ws.measure.on,
        selector: o.view.selector.is_some(),
    }
}

/// One choice of a submenu command.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// What [`choose`] takes.
    pub key: String,
    pub label: String,
    /// Chosen now (a selectable label's state).
    pub on: bool,
    pub enabled: bool,
}

fn choice(key: impl Into<String>, label: impl Into<String>, on: bool) -> Choice {
    Choice {
        key: key.into(),
        label: label.into(),
        on,
        enabled: true,
    }
}

/// The choices a [`command::Kind::Choices`] command lists now.
pub fn choices(o: &Opened, id: &str) -> Vec<Choice> {
    match id {
        "file.open_recent" => o
            .recent
            .iter()
            .filter(|p| **p != o.ws.root)
            .map(|p| choice(p.display().to_string(), p.display().to_string(), false))
            .collect(),
        "view.named" => {
            o.ws.views
                .views
                .iter()
                .map(|v| Choice {
                    enabled: v.camera.is_ok(),
                    ..choice(&v.id, &v.id, false)
                })
                .collect()
        }
        "view.quality" => {
            let now = o.ws.settings.text("display.quality", "normal");
            ["coarse", "normal", "fine"]
                .iter()
                .map(|q| choice(*q, *q, now == *q))
                .collect()
        }
        "view.show" => {
            let focus = o.ws.model.focus.as_deref();
            let mut out = vec![choice("", "The project's model", focus.is_none())];
            out.extend(o.ws.model.parts.values().map(|p| {
                let label = if p.assembly {
                    format!("{} (assembly)", p.id)
                } else {
                    p.id.clone()
                };
                choice(&p.id, label, focus == Some(p.id.as_str()))
            }));
            out
        }
        _ => Vec::new(),
    }
}

/// Runs a choice of a submenu command.
pub fn choose(o: &mut Opened, id: &str, key: &str) -> Option<Action> {
    match id {
        "file.open_recent" => return Some(Action::Open(Some(key.into()))),
        "view.named" => o.view.restore_view(&mut o.ws, key),
        "view.quality" => {
            if o.ws.settings.text("display.quality", "normal") != key {
                o.ws.set_config("display.quality", json!(key));
            }
        }
        "view.show" => o.ws.set_focus((!key.is_empty()).then(|| key.to_owned())),
        _ => {}
    }
    None
}

/// Runs `id`: from a menu, the toolbar or a key with no `target`, from a
/// context menu with the reference of what was right-clicked. A command
/// that can't run now does nothing. Returns what it asks of the app.
pub fn run(o: &mut Opened, ctx: &egui::Context, id: &str, target: Option<&str>) -> Option<Action> {
    if command::enabled(id, &state(o)).is_err() && target.is_none() {
        return None;
    }
    if let Some(v) = id.strip_prefix("view.").and_then(StandardView::named) {
        o.view.standard(&mut o.ws, v);
        return None;
    }
    let toggle = |o: &mut Opened, key: &str, on: bool| o.ws.set_config(key, json!(!on));
    match id {
        "file.open" => return Some(Action::Open(None)),
        "file.import_step" => o.ws.import = Some(crate::core::step::Import::new()),
        "file.update_import" => {
            let file = o.view.chosen.as_ref().and_then(|(p, f)| {
                Some(
                    o.ws.model
                        .parts
                        .get(p)?
                        .feature(f)?
                        .import
                        .as_ref()?
                        .file
                        .clone(),
                )
            });
            if let Some(f) = file {
                o.ws.import = Some(crate::core::step::Import::update(&f));
            }
        }
        "file.export_step" => {
            if let [one] = o.ws.model.shown().as_slice() {
                o.ws.export = Some(crate::core::step::Export::new(one));
            }
        }
        "file.close" => return Some(Action::Close),
        "file.quit" => return Some(Action::Quit),
        "edit.undo" => o.ws.undo(None),
        "edit.redo" => o.ws.redo(None),
        "edit.copy_reference" => {
            let text = match target {
                Some(t) => t.to_owned(),
                None => {
                    o.ws.myself()
                        .map(|m| m.selection.join("\n"))
                        .unwrap_or_default()
                }
            };
            ctx.copy_text(text);
        }
        "edit.select" => {
            if let Some(t) = target {
                o.ws.select(vec![t.to_owned()], SelectMode::Replace);
            }
        }
        // Esc closes the view selector first, then leaves the Measure
        // tool, then clears the selection and the tree's choice with it.
        "edit.clear_selection" => {
            if o.view.selector.is_some() {
                o.view.selector = None;
            } else if o.ws.measure.on {
                o.ws.measure.set(false);
            } else {
                if o.ws.myself().is_some_and(|m| !m.selection.is_empty()) {
                    o.ws.select(Vec::new(), SelectMode::Replace);
                }
                o.view.unchoose(&mut o.ws);
            }
        }
        "edit.preferences" => {
            o.dialogs.preferences = true;
            o.ws.read_cache();
        }
        "view.normal_to" => o.view.normal_to(&mut o.ws),
        // At the pointer over the viewport, else its middle; again closes it.
        "view.selector" => {
            let at = ctx.input(|i| i.pointer.latest_pos());
            let at = at.filter(|p| o.view.rect.contains(*p));
            o.view.toggle_selector(at.unwrap_or(o.view.rect.center()));
        }
        "view.fit" => o.view.fit(&mut o.ws, None),
        "view.fit_selection" => {
            let selection =
                o.ws.myself()
                    .map(|m| m.selection.clone())
                    .unwrap_or_default();
            o.view.fit_entities(&mut o.ws, &selection);
        }
        "view.edges" => toggle(o, "display.edges", o.view.edges),
        "view.sketches" => toggle(o, "display.sketches", o.view.sketches),
        "view.nav_cube" => toggle(o, "display.nav_cube", o.view.cube),
        "view.triad" => toggle(o, "display.triad", o.view.triad),
        "view.origin" => toggle(o, "display.origin", o.view.origin),
        "view.isolate" => {
            if let Some(t) = target {
                o.ws.set_visibility(Vec::new(), Vec::new(), vec![t.to_owned()], false);
            }
        }
        "view.show_all" => {
            o.ws.set_visibility(Vec::new(), Vec::new(), Vec::new(), true)
        }
        "view.compare_head" => o.ws.set_compare(Some("HEAD".to_owned())),
        "view.compare_with" => o.compare_with = Some(String::new()),
        "view.compare_stop" => o.ws.set_compare(None),
        "measure.tool" => {
            let on = !o.ws.measure.on;
            o.ws.measure.set(on);
        }
        "measure.mass" => {
            if let Some(of) = o.ws.mass_target() {
                o.ws.weigh(&of);
                o.dialogs.mass = true;
            }
        }
        "measure.clearance" => {
            o.ws.check_clearance();
            o.dialogs.clearance = true;
        }
        "tools.interference" => {
            o.ws.check_interference();
            o.dialogs.interference = true;
        }
        // Mod+K again closes it.
        "palette.open" => {
            o.palette = match o.palette {
                Some(_) => None,
                None => Some(crate::ui::palette::Palette::default()),
            };
        }
        "project.settings" => o.dialogs.settings = true,
        "project.doctor" => {
            o.dialogs.doctor = true;
            o.ws.diagnose();
        }
        "help.agents" => o.dialogs.agents = true,
        "help.about" => o.dialogs.about = true,
        _ => {}
    }
    None
}

/// Whether this OS's Mod is Cmd.
pub fn mac(ctx: &egui::Context) -> bool {
    ctx.os() == egui::os::OperatingSystem::Mac
}

/// What the window's commands look like this frame, for the menus drawn
/// where only the workspace and the view are at hand (the tree's and the
/// viewport's context menus): their availability and their keys' text.
#[derive(Clone, Debug, Default)]
pub struct Shown {
    pub state: State,
    pub keys: std::collections::HashMap<&'static str, String>,
}

/// This frame's [`Shown`].
pub fn shown(o: &mut Opened, ctx: &egui::Context) -> Shown {
    let keys = command::COMMANDS
        .iter()
        .filter_map(|c| Some((c.id, keys_text(ctx, &o.keymap, c.id)?)))
        .collect();
    Shown {
        state: state(o),
        keys,
    }
}

/// The keys `id` has, as this OS writes them (`Ctrl+Shift+Z`, `⇧⌘Z`).
pub fn keys_text(ctx: &egui::Context, keymap: &Keymap, id: &str) -> Option<String> {
    let mac = mac(ctx);
    let mut texts: Vec<String> = Vec::new();
    for c in keymap.chords(id) {
        let t = ctx.format_shortcut(&c.shortcut(mac));
        if !texts.contains(&t) {
            texts.push(t);
        }
    }
    (!texts.is_empty()).then(|| texts.join(", "))
}

/// The window's keys this frame: each command's chords, the most
/// modified first (so Mod+Shift+Z is redo before Mod+Z is undo). A text
/// field keeps its keys but for commands that fire in text; Esc is a
/// menu's or popup's while one is open (`popup_open`: whether one was
/// when the frame began, since the menus have closed it by now). The
/// viewport's commands are the view's (`View::take_keys`).
pub fn keys(o: &mut Opened, ctx: &egui::Context, popup_open: bool) -> Option<Action> {
    let mac = mac(ctx);
    let typing = ctx.memory(|m| m.focused().is_some());
    let popup = popup_open || egui::Popup::is_any_open(ctx);
    let mut binds: Vec<(&'static str, Chord)> = o
        .keymap
        .all()
        .flat_map(|(id, chords)| chords.iter().map(move |c| (id, *c)))
        .filter(|(id, _)| command::get(id).is_some_and(|c| c.scope == command::Scope::Window))
        .collect();
    let weight = |c: &Chord| {
        [c.command, c.ctrl, c.alt, c.shift]
            .iter()
            .filter(|m| **m)
            .count()
    };
    binds.sort_by_key(|(_, c)| std::cmp::Reverse(weight(c)));
    for (id, chord) in binds {
        let c = command::get(id).expect("bound commands are registered");
        if (typing && !c.in_text) || (chord.key == egui::Key::Escape && popup) {
            continue;
        }
        // The palette has the keyboard while it is open (egui lets go of
        // a field's focus on Esc before anyone sees the key): only its own
        // keys, which close it.
        if o.palette.is_some() && id != "palette.open" {
            continue;
        }
        // A key whose command can't run now is left for someone else.
        if command::enabled(id, &state(o)).is_err() {
            continue;
        }
        let s = chord.shortcut(mac);
        if ctx.input_mut(|i| crate::core::keymap::consume(i, &s))
            && let Some(a) = run(o, ctx, id, None)
        {
            return Some(a);
        }
    }
    None
}

/// A context menu's entries for `what` (`target`, the reference
/// right-clicked): returns the command chosen, for the window to run with
/// it once the frame is drawn.
pub fn context_menu(
    ui: &mut egui::Ui,
    shown: &Shown,
    what: Context,
    target: Option<&str>,
) -> Option<(&'static str, Option<String>)> {
    let mut chosen = None;
    for c in command::context(what) {
        let mut b = egui::Button::new(c.title);
        if let Some(k) = shown.keys.get(c.id) {
            b = b.shortcut_text(k.as_str());
        }
        let enabled = target.is_some() || command::enabled(c.id, &shown.state).is_ok();
        if ui.add_enabled(enabled, b).clicked() {
            chosen = Some((c.id, target.map(str::to_owned)));
            ui.close();
        }
    }
    chosen
}
