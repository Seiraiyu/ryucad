//! The Settings dialog (project settings, kept in `ryucad.json` and Git)
//! and the Preferences dialog (this user's, kept on this machine). Both
//! change values only through `config.set`, and show where each comes
//! from.

use crate::core::workspace::Workspace;
use eframe::egui;
use serde_json::{Value, json};

/// Where a setting's value comes from, for people.
fn source(ws: &Workspace, key: &str) -> &'static str {
    match ws.settings.all.iter().find(|s| s.key == key) {
        Some(s) if s.default => "default",
        Some(s) if s.scope == "project" => "this project",
        Some(_) => "your preferences",
        None => "",
    }
}

/// A dialog window, first placed at the top of `view` (the 3D view's
/// rectangle), clear of the menus and panels.
pub fn window(title: &str, view: egui::Rect) -> egui::Window<'_> {
    egui::Window::new(title)
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(view.center_top() + egui::vec2(0.0, 16.0))
}

fn about(ws: &Workspace, key: &str) -> String {
    ws.settings
        .all
        .iter()
        .find(|s| s.key == key)
        .map_or_else(String::new, |s| s.about.clone())
}

/// A setting chosen from a list.
fn choice(ui: &mut egui::Ui, ws: &mut Workspace, key: &str, label: &str, options: &[&str]) {
    let current = ws.settings.text(key, "").to_owned();
    let name = crate::ui::form::label(ui, label, false).on_hover_text(about(ws, key));
    let mut picked = None;
    egui::ComboBox::from_id_salt(key)
        .selected_text(&current)
        .show_ui(ui, |ui| {
            for o in options {
                if ui.selectable_label(current == *o, *o).clicked() && current != *o {
                    picked = Some(*o);
                }
            }
        })
        .response
        .labelled_by(name.id);
    ui.weak(source(ws, key));
    ui.end_row();
    if let Some(o) = picked {
        ws.set_config(key, json!(o));
    }
}

/// A setting that is on or off. Returns whether it was changed.
fn toggle(ui: &mut egui::Ui, ws: &mut Workspace, key: &str, label: &str) -> bool {
    let mut on = ws
        .settings
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let name = crate::ui::form::label(ui, label, false).on_hover_text(about(ws, key));
    let changed = ui.checkbox(&mut on, "").labelled_by(name.id).changed();
    if changed {
        ws.set_config(key, json!(on));
    }
    ui.weak(source(ws, key));
    ui.end_row();
    changed
}

/// A whole number in a range, sent when the drag ends.
fn number(ui: &mut egui::Ui, ws: &mut Workspace, key: &str, label: &str, range: (i64, i64)) {
    let id = ui.make_persistent_id(("setting", key));
    let stored = ws
        .settings
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or(range.0);
    let mut n = ui.data(|d| d.get_temp::<i64>(id)).unwrap_or(stored);
    let name = crate::ui::form::label(ui, label, false).on_hover_text(about(ws, key));
    let r = ui
        .add(egui::DragValue::new(&mut n).range(range.0..=range.1))
        .labelled_by(name.id);
    if r.changed() || r.dragged() {
        ui.data_mut(|d| d.insert_temp(id, n));
    }
    let done = (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && n != stored;
    if done {
        ws.set_config(key, json!(n));
    }
    if !r.dragged() && !r.has_focus() && !r.changed() {
        ui.data_mut(|d| d.remove::<i64>(id));
    }
    ui.weak(source(ws, key));
    ui.end_row();
}

/// A line of text, sent when it is entered or loses focus changed.
/// Returns whether it was sent.
fn text(ui: &mut egui::Ui, ws: &mut Workspace, key: &str, label: &str, hint: &str) -> bool {
    let id = ui.make_persistent_id(("setting", key));
    let stored = ws.settings.text(key, "").to_owned();
    let mut t = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or(stored.clone());
    let name = crate::ui::form::label(ui, label, false).on_hover_text(about(ws, key));
    let r = ui
        .add(egui::TextEdit::singleline(&mut t).hint_text(hint))
        .labelled_by(name.id);
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(id, t.clone()));
    }
    let sent = r.lost_focus() && t.trim() != stored;
    if sent {
        ws.set_config(key, json!(t.trim()));
    }
    if !r.has_focus() {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    ui.weak(source(ws, key));
    ui.end_row();
    sent
}

/// The shared cache: its server, whether this machine writes to it, the
/// token it is sent (kept in a file of its own, never with the
/// preferences) and what it has done.
fn shared_cache(ui: &mut egui::Ui, ws: &mut Workspace) {
    ui.label("Shared cache")
        .on_hover_text("A server your team shares results and meshes through, so what one machine built no other builds again");
    crate::ui::form::plain_grid("shared cache", 3)
        .num_columns(3)
        .show(ui, |ui| {
            let mut changed = text(
                ui,
                ws,
                "cache.remote",
                "Shared cache server",
                "https://cache.example.com",
            );
            changed |= toggle(ui, ws, "cache.remote_write", "Send what I build");
            if changed {
                ws.read_cache();
            }
            let id = ui.make_persistent_id("cache token");
            let mut token = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default();
            let name = ui.label("Token");
            let r = ui
                .add(egui::TextEdit::singleline(&mut token).password(true))
                .labelled_by(name.id);
            if r.changed() {
                ui.data_mut(|d| d.insert_temp(id, token.clone()));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!token.trim().is_empty(), egui::Button::new("Save token"))
                    .clicked()
                {
                    ws.login_cache(&token);
                    ui.data_mut(|d| d.remove::<String>(id));
                }
                let stored = ws
                    .cache
                    .as_ref()
                    .is_some_and(|c| c["remote"]["token"] == true)
                    || ws
                        .cache_token
                        .as_ref()
                        .is_some_and(|t| t["token"] == "file");
                if ui
                    .add_enabled(stored, egui::Button::new("Forget token"))
                    .clicked()
                {
                    ws.logout_cache();
                }
            });
            ui.end_row();
        });
    ui.weak(shared_text(ws.cache.as_ref()));
    if let Some(t) = &ws.cache_token {
        ui.weak(token_text(t));
    }
}

/// What the shared cache has done, as the Preferences window says it.
pub fn shared_text(stats: Option<&Value>) -> String {
    let Some(r) = stats.map(|s| &s["remote"]) else {
        return "reading…".to_owned();
    };
    if r["enabled"] != true {
        return match (r["url"].as_str(), r["note"].as_str()) {
            (Some(url), Some(note)) => format!("{url} isn't used: {note}"),
            _ => "None set: this machine builds what it lacks itself".to_owned(),
        };
    }
    let n = |k: &str| r[k].as_u64().unwrap_or_default();
    let state = match r["problem"]["message"].as_str() {
        Some(m) => format!("skipped for now: {m}"),
        None if r["write"] == true => "reading and sending".to_owned(),
        None => "reading only".to_owned(),
    };
    format!(
        "{}, {state}: {} read, {} sent, {} bad",
        r["url"].as_str().unwrap_or_default(),
        n("hits"),
        n("puts"),
        n("bad")
    )
}

/// What storing or removing the token did, as the Preferences window says
/// it.
pub fn token_text(answer: &Value) -> String {
    if let Some(m) = answer["error"]["message"].as_str() {
        return format!("Not saved: {m}");
    }
    if answer["token"].is_null() {
        return "No token stored".to_owned();
    }
    let removed = answer["removed"] == true;
    let source = match answer["overridden_by"].as_str() {
        Some(var) if removed => format!("Removed; {var} is still sent"),
        Some(var) => format!("Saved, but {var} is sent instead"),
        None => "Saved".to_owned(),
    };
    match (
        &answer["server"]["write"],
        answer["server"]["error"]["message"].as_str(),
    ) {
        (Value::Bool(true), _) => format!("{source}: the server lets it read and write"),
        (Value::Bool(false), _) => format!("{source}: the server lets it read only"),
        (_, Some(m)) => format!("{source}; the server says: {m}"),
        _ => source,
    }
}

/// The project's settings: only what rebuilding or understanding the
/// model needs.
pub fn settings(ctx: &egui::Context, ws: &mut Workspace, open: &mut bool, view: egui::Rect) {
    window("Project settings", view)
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Saved in this project's ryucad.json, so they travel with it in Git.");
            crate::ui::form::plain_grid("project settings", 3)
                .num_columns(3)
                .show(ui, |ui| {
                    choice(
                        ui,
                        ws,
                        "default_unit",
                        "Default unit",
                        &["mm", "cm", "m", "um", "in", "ft"],
                    );
                });
            ui.weak(
                "Bare numbers in feature files take this unit; values already written keep theirs.",
            );
        });
}

/// This user's preferences, on this machine.
pub fn preferences(ctx: &egui::Context, ws: &mut Workspace, open: &mut bool, view: egui::Rect) {
    window("Preferences", view)
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Saved for you on this machine; projects don't carry them.");
            for w in ws.settings.warnings.clone() {
                ui.colored_label(ui.visuals().warn_fg_color, w);
            }
            crate::ui::form::plain_grid("preferences", 3)
                .num_columns(3)
                .show(ui, |ui| {
                    choice(
                        ui,
                        ws,
                        "ui.style",
                        "Style",
                        &["drafting", "graphite", "studio"],
                    );
                    choice(ui, ws, "ui.theme", "Theme", &["system", "light", "dark"]);
                    choice(
                        ui,
                        ws,
                        "ui.navigation",
                        "Mouse navigation",
                        &["ryucad", "solidworks", "onshape"],
                    );
                    choice(
                        ui,
                        ws,
                        "display.quality",
                        "Display quality",
                        &["coarse", "normal", "fine"],
                    );
                    toggle(ui, ws, "display.edges", "Draw edges");
                    toggle(ui, ws, "display.sketches", "Draw sketches");
                    toggle(ui, ws, "display.nav_cube", "Navigation cube");
                    toggle(ui, ws, "display.triad", "Axis triad");
                    toggle(ui, ws, "display.origin", "Origin axes and planes");
                    number(
                        ui,
                        ws,
                        "display.mesh_cache_mib",
                        "Mesh cache (MiB)",
                        (16, 65536),
                    );
                    number(
                        ui,
                        ws,
                        "daemon.poll_ms",
                        "Check files every (ms)",
                        (50, 5000),
                    );
                });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Disk cache");
                ui.weak(cache_text(ws.cache.as_ref()));
                let enabled = ws.cache.as_ref().is_some_and(|c| c["enabled"] == true);
                if ui
                    .add_enabled(enabled, egui::Button::new("Clear cache"))
                    .on_hover_text("Results and meshes every project on this machine reuses; anything cleared is rebuilt when next needed")
                    .clicked()
                {
                    ws.clear_cache();
                }
            });
            ui.separator();
            shared_cache(ui, ws);
        });
}

/// What the disk cache holds, as the Preferences window says it.
pub fn cache_text(stats: Option<&serde_json::Value>) -> String {
    let Some(s) = stats else {
        return "reading…".to_owned();
    };
    if s["enabled"] != true {
        return "none on this machine".to_owned();
    }
    let mib = |b: &serde_json::Value| b.as_u64().unwrap_or_default() as f64 / f64::from(1 << 20);
    format!(
        "{} entries, {:.1} MiB of {:.0} MiB",
        s["entries"].as_u64().unwrap_or_default(),
        mib(&s["bytes"]),
        mib(&s["max_bytes"])
    )
}

/// Tools > Check interference: the pairs of the shown assembly's solids
/// that share a volume, each with a Show button (the pair's touched
/// faces, to frame), how many only touch, and those that couldn't be
/// checked. Returns the faces to frame when Show is pressed.
pub fn interference(
    ctx: &egui::Context,
    ws: &mut Workspace,
    open: &mut bool,
    view: egui::Rect,
) -> Option<Vec<String>> {
    let mut show = None;
    let mut again = false;
    window("Interference", view)
        .open(open)
        .default_width(420.0)
        .show(ctx, |ui| {
            let Some(i) = &ws.interference else {
                ui.label("Show an assembly, then check it.");
                return;
            };
            ui.label(format!("Assembly {}", i.part));
            again = ui.button("Check again").clicked();
            ui.separator();
            if !i.loaded {
                ui.spinner();
                return;
            }
            if let Some(e) = &i.error {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
                return;
            }
            if i.pairs.is_empty() {
                ui.label("No instances interfere.");
            }
            for p in &i.pairs {
                ui.horizontal(|ui| {
                    ui.label(p.label(&i.part));
                    if ui.small_button("Show").clicked() {
                        show = Some(p.faces.clone());
                    }
                });
            }
            if i.touching > 0 {
                ui.label(format!("{} pair(s) only touch.", i.touching));
            }
            for (a, b) in &i.unchecked {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("{a} and {b} couldn't be checked"),
                );
            }
            ui.weak("Volumes are estimated from meshes.");
        });
    if again {
        ws.check_interference();
    }
    show
}

/// A panel's lines: label, then value, in a grid.
fn line_grid(ui: &mut egui::Ui, id: &str, lines: &[(String, String)]) {
    egui::Grid::new(id)
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            for (label, value) in lines {
                ui.label(label);
                ui.label(value);
                ui.end_row();
            }
        });
}

/// Tools > Measure, while on: what the picks measured, with Copy. Esc or
/// closing it leaves the tool.
pub fn measure(ctx: &egui::Context, ws: &mut Workspace, view: egui::Rect) {
    let mut open = ws.measure.on;
    if !open {
        return;
    }
    let mut copy = None;
    window("Measure", view)
        .open(&mut open)
        .default_width(360.0)
        .show(ctx, |ui| {
            let t = &ws.measure;
            if t.picks.is_empty() {
                ui.label("Pick a face, edge or vertex: its properties. Pick a second: the distance and angle between them.");
                return;
            }
            for p in &t.picks {
                ui.monospace(p);
            }
            ui.separator();
            if let Some(e) = &t.error {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
                return;
            }
            let Some(r) = &t.reading else {
                ui.spinner();
                return;
            };
            line_grid(ui, "measure lines", &r.lines);
            if ui.button("Copy").clicked() {
                copy = Some(t.text());
            }
        });
    if let Some(text) = copy {
        ctx.copy_text(text);
    }
    if !open {
        ws.measure.set(false);
    }
}

/// Tools > Appearance: the colour and opacity of what is selected (a
/// face, its solid or its part; an occurrence's solid or the occurrences
/// above it), each shown on the model as it is dragged and written once
/// when it is let go (or typed), one undoable change each; the part's
/// material's colour; and Clear, which takes the entry out.
pub fn appearance(ctx: &egui::Context, ws: &mut Workspace, view: egui::Rect) {
    use crate::core::appearance::Preview;
    let mut open = ws.appearance.open;
    if !open {
        return;
    }
    // What to do once the window is drawn: preview, write a field, clear.
    let mut preview = None;
    let mut write = None;
    let mut clear = false;
    let released = !ctx.input(|i| i.pointer.any_down());
    window("Appearance", view)
        .open(&mut open)
        .default_width(340.0)
        .show(ctx, |ui| {
            let e = &mut ws.appearance;
            if e.targets.is_empty() {
                ui.label("Select a face, a solid or an occurrence, or show one part.");
                return;
            }
            let chosen = e.chosen;
            egui::ComboBox::from_label("Colour what")
                .selected_text(e.targets[chosen].clone())
                .show_ui(ui, |ui| {
                    for (k, t) in e.targets.clone().iter().enumerate() {
                        ui.selectable_value(&mut e.chosen, k, t);
                    }
                });
            if e.chosen != chosen {
                e.dragging = None;
            }
            ui.horizontal(|ui| {
                let name = ui.label("Colour");
                let r = egui::color_picker::color_edit_button_srgb(ui, &mut e.colour)
                    .labelled_by(name.id);
                if r.changed() {
                    e.typed = e.hex();
                    preview = Some(Preview {
                        colour: Some(e.colour.map(|c| f32::from(c) / 255.0)),
                        opacity: None,
                    });
                }
                let hex = ui.label("Hex");
                let r = ui
                    .add(egui::TextEdit::singleline(&mut e.typed).desired_width(80.0))
                    .labelled_by(hex.id);
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && e.take_typed()
                {
                    write = Some("colour");
                }
            });
            let name = ui.label("Opacity");
            let r = ui
                .add(egui::Slider::new(&mut e.opacity, 0.05..=1.0))
                .labelled_by(name.id);
            if r.changed() {
                preview = Some(Preview {
                    colour: None,
                    opacity: Some(e.opacity),
                });
            }
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                write = Some("opacity");
            }
            ui.horizontal(|ui| {
                if ui.button("Use material colour").clicked() {
                    write = Some("material");
                }
                if ui.button("Clear").clicked() {
                    clear = true;
                }
            });
            if let Some(err) = &e.error {
                ui.colored_label(ui.visuals().error_fg_color, &err.message);
            }
            ui.weak("Colours are kept in Git, in the part's appearance.json: ryucad colour.");
        });
    if let Some(p) = preview {
        ws.preview_appearance(p);
    }
    // A colour dragged in the picker is written when the pointer is let go.
    if write.is_none() && released && ws.appearance.dragging.is_some_and(|d| d.colour.is_some()) {
        write = Some("colour");
    }
    if let Some(field) = write {
        ws.write_appearance(field);
    }
    if clear {
        ws.clear_appearance();
    }
    if !open {
        ws.close_appearance();
    }
}

/// Tools > Mass properties: the weighed solid, part or assembly's values,
/// each part's material and mass, and the warnings. Weigh again takes
/// the selection (or what is shown) anew.
pub fn mass(ctx: &egui::Context, ws: &mut Workspace, open: &mut bool, view: egui::Rect) {
    let mut again = None;
    window("Mass properties", view)
        .open(open)
        .default_width(380.0)
        .show(ctx, |ui| {
            let Some(m) = &ws.mass else {
                ui.label("Select a solid, or show a part or assembly.");
                return;
            };
            ui.label(format!("Of {}", m.of));
            if ui.button("Weigh again").clicked() {
                again = ws.mass_target();
            }
            ui.separator();
            if !m.loaded {
                ui.spinner();
                return;
            }
            if let Some(e) = &m.error {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
                return;
            }
            line_grid(ui, "mass lines", &m.lines);
            if !m.parts.is_empty() {
                ui.separator();
                line_grid(ui, "mass parts", &m.parts);
            }
            for w in &m.warnings {
                ui.colored_label(ui.visuals().warn_fg_color, w);
            }
            ui.weak("Materials and their densities are in materials.json: ryucad materials set.");
        });
    if let Some(of) = again {
        ws.weigh(&of);
    }
}

/// Tools > Clearance: the shown assembly's pairs of solids, closest
/// first, each with a Show button (both nearest points, to frame); those
/// that couldn't be measured. Returns the points to frame when Show is
/// pressed.
pub fn clearance(
    ctx: &egui::Context,
    ws: &mut Workspace,
    open: &mut bool,
    view: egui::Rect,
) -> Option<Vec<[f64; 3]>> {
    let mut show = None;
    let mut again = false;
    window("Clearance", view)
        .open(open)
        .default_width(440.0)
        .show(ctx, |ui| {
            let Some(c) = &mut ws.clearance else {
                ui.label("Show an assembly, then measure its clearance.");
                return;
            };
            ui.label(format!("Assembly {}", c.part));
            again = ui.button("Measure again").clicked();
            ui.separator();
            if !c.loaded {
                ui.spinner();
                return;
            }
            if let Some(e) = &c.error {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
                return;
            }
            if c.gaps.is_empty() {
                ui.label("No instances come within a hundredth of the assembly's size.");
            }
            for (k, g) in c.gaps.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(g.label(&c.part));
                    if ui.small_button("Show").clicked() {
                        show = Some((k, g.points.to_vec()));
                    }
                });
            }
            for (a, b) in &c.unchecked {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("{a} and {b} couldn't be measured"),
                );
            }
            ui.weak("Pairs at 0 touch or interfere: Tools > Check interference says which.");
        });
    if again {
        ws.check_clearance();
    }
    let (k, points) = show?;
    if let Some(c) = &mut ws.clearance {
        c.shown = Some(k);
    }
    Some(points)
}
