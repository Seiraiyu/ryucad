//! The panels around the viewport: the feature tree, the participants bar
//! and the status line.

use crate::core::compare::{Change, Mark};
use crate::core::model::{BodyKey, Part};
use crate::core::workspace::Workspace;
use crate::ui::style;
use crate::ui::view::View;
use eframe::egui::{self, RichText};
use ryucad::live::Kind;

/// An error as one line for people.
pub fn message(e: &serde_json::Value) -> String {
    e["message"].as_str().unwrap_or("failed").to_owned()
}

/// A feature's line in the tree: its ID and type, a sketch's solve state,
/// and why it failed or how many faces weren't meshed.
pub fn row_text(f: &crate::core::model::Feature, faults: usize) -> String {
    let ty = match (&f.ty, &f.operation) {
        (Some(t), Some(op)) => format!("{t}, {op}"),
        (Some(t), None) => t.clone(),
        (None, _) => "?".to_owned(),
    };
    let kind = match &f.sketch {
        Some(s) if s.state == "under" => format!("{ty}, under, {} dof", s.dof),
        Some(s) => format!("{ty}, {}", s.state),
        None => ty,
    };
    let stale = if f.sketch.as_ref().is_some_and(|s| s.stale) {
        ", stale"
    } else {
        ""
    };
    let text = format!("{} ({kind}{stale})", f.id);
    match &f.error {
        Some(e) => match e["code"].as_str() {
            Some(code) => format!("{text}: failed ({code})"),
            None => format!("{text}: failed"),
        },
        None if faults > 0 => format!("{text}: {faults} unmeshed"),
        None => text,
    }
}

/// How a feature differs in a compare, after its line in the tree:
/// ` [changed, moved, failed at HEAD (rebuild_failed)]`; empty when it
/// doesn't.
pub fn mark_text(m: &Mark, rev: &str) -> String {
    let mut says = Vec::new();
    if m.change != Change::Same {
        says.push(m.change.as_str().to_owned());
    }
    if m.moved {
        says.push("moved".to_owned());
    }
    if let Some(code) = &m.failed_there {
        says.push(format!("failed at {rev} ({code})"));
    }
    if says.is_empty() {
        String::new()
    } else {
        format!(" [{}]", says.join(", "))
    }
}

/// A part's line in the tree: its ID, its release state (`p (B
/// modified, C proposed)`) once known, and in a compare the records it
/// adds (`[proposes C]`).
pub fn part_text(id: &str, badge: Option<&str>, marks: &[String]) -> String {
    let mut s = id.to_owned();
    if let Some(b) = badge {
        s.push_str(&format!(" ({b})"));
    }
    if !marks.is_empty() {
        s.push_str(&format!(" [{}]", marks.join(", ")));
    }
    s
}

/// The colour a compare gives a feature's line.
fn mark_colour(m: &Mark) -> style::Rgb {
    match m.change {
        Change::Added => style::NEW,
        Change::Removed => style::REMOVED,
        _ => style::CHANGED,
    }
}

/// One feature's line in the tree: its status, how a compare marks it,
/// and under it what it reads. Clicking it chooses it, or unchooses it.
fn feature_row(
    ui: &mut egui::Ui,
    ws: &Workspace,
    view: &View,
    part: &Part,
    f: &crate::core::model::Feature,
    choose: &mut Option<Option<BodyKey>>,
) {
    let key = (part.id.clone(), f.id.clone());
    let mark = ws.compare.as_ref().and_then(|c| {
        let m = c.features.get(&key)?;
        Some((mark_text(m, &c.rev), mark_colour(m)))
    });
    let text = match &mark {
        Some((t, c)) => RichText::new(row_text(f, 0) + t).color(style::color32(*c)),
        None => RichText::new(row_text(f, 0)),
    };
    let chosen = view.chosen.as_ref() == Some(&key);
    let r = ui.selectable_label(chosen, text);
    let r = match &f.error {
        Some(e) => r.on_hover_text(message(e)),
        None => r,
    };
    if r.clicked() {
        *choose = Some(if chosen { None } else { Some(key) });
    }
    if !f.uses.is_empty() {
        ui.indent(("uses", &part.id, &f.id), |ui| {
            ui.weak(format!("uses {}", f.uses.join(", ")));
        });
    }
}

/// A solid in the tree: its founder, its state and mesh faults, and its
/// chain of features in order; a boolean's consumed tools under it.
fn solid_group(
    ui: &mut egui::Ui,
    ws: &Workspace,
    view: &View,
    part: &Part,
    solid: &str,
    tool: bool,
    choose: &mut Option<Option<BodyKey>>,
) {
    let shown = part.solid(solid);
    let faults = ws
        .model
        .meshes
        .get(&(part.id.clone(), solid.to_owned()))
        .and_then(|m| m.mesh.as_ref().ok())
        .map_or(0, |m| m.faults.len());
    let mut title = if tool {
        format!("{solid} (tool, consumed)")
    } else {
        format!("{solid} (solid)")
    };
    if let Some(code) = shown
        .and_then(|s| s.error.as_ref())
        .and_then(|e| e["code"].as_str())
    {
        title.push_str(&format!(": failed ({code}), showing its last good state"));
    } else if faults > 0 {
        title.push_str(&format!(": {faults} unmeshed"));
    }
    let header = egui::CollapsingHeader::new(RichText::new(title).strong())
        .id_salt(("solid", &part.id, solid))
        .default_open(!tool)
        .show(ui, |ui| {
            let chain = part
                .features
                .iter()
                .filter(|f| f.solids.iter().any(|s| s == solid));
            for f in chain {
                feature_row(ui, ws, view, part, f, choose);
                for t in &f.consumes {
                    ui.indent(("tools", &part.id, &f.id, t), |ui| {
                        solid_group(ui, ws, view, part, t, true, choose);
                    });
                }
            }
        });
    if faults > 0 {
        header.header_response.on_hover_text(format!(
            "{faults} face(s) couldn't be meshed: the kernel left them out"
        ));
    }
}

/// Parts with their solids and sketches in order: each solid with its
/// state, mesh faults and chain of features, each feature with its
/// rebuild status and under it what it reads. Choosing a feature paints
/// what it made and frames its solid, or paints its sketch. In a compare,
/// features are marked by how they differ, and those only the other side
/// has follow, to be chosen for their changes.
pub fn tree(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    // Names are IDs: never broken across lines.
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    ui.heading("Parts");
    if ws.model.parts.is_empty() {
        ui.weak("No parts yet");
    }
    let mut choose = None;
    // A feature only a compare's other side has: chosen for its changes,
    // with no form and nothing to frame.
    let mut gone = None;
    for part in ws.model.parts.values() {
        // Damage doctor found in the part's files.
        let prefix = format!("parts/{}/", part.id);
        let damage: Vec<String> = ws
            .doctor
            .as_ref()
            .and_then(|d| d["issues"].as_array())
            .into_iter()
            .flatten()
            .filter(|i| {
                i["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|f| f.as_str().is_some_and(|f| f.starts_with(&prefix)))
            })
            .map(|i| i["message"].as_str().unwrap_or_default().to_owned())
            .collect();
        let marks = ws
            .compare
            .as_ref()
            .and_then(|c| c.pdm.get(&part.id))
            .map_or(&[][..], |m| m.marks.as_slice());
        let state = ws.states.get(&part.id);
        let badge = state.map(crate::core::pdm::badge);
        let text = part_text(&part.id, badge.as_deref(), marks);
        let title = match &part.error {
            Some(_) => RichText::new(format!("{text} (broken)")).color(ui.visuals().warn_fg_color),
            None if !marks.is_empty() => RichText::new(text).color(style::color32(style::NEW)),
            None => RichText::new(text),
        };
        let header = egui::CollapsingHeader::new(title)
            .id_salt(("part", &part.id))
            .default_open(true)
            .show(ui, |ui| {
                for f in &part.features {
                    match f.solids.as_slice() {
                        // A sketch, or a file that doesn't load.
                        [] => feature_row(ui, ws, view, part, f, &mut choose),
                        // A solid shows where its founder is listed; what
                        // acts on solids shows in them, a consumed solid
                        // under its boolean.
                        [s] if *s == f.id && f.operation.is_none() && part.solid(s).is_some() => {
                            solid_group(ui, ws, view, part, s, false, &mut choose);
                        }
                        _ => {}
                    }
                }
                let removed = ws.compare.as_ref().and_then(|c| c.removed.get(&part.id));
                for id in removed.into_iter().flatten() {
                    let key = (part.id.clone(), id.clone());
                    let chosen = view.chosen.as_ref() == Some(&key);
                    let text = RichText::new(format!("{id} [removed]"))
                        .color(style::color32(style::REMOVED));
                    if ui.selectable_label(chosen, text).clicked() {
                        gone = Some(if chosen { None } else { Some(key) });
                    }
                }
            });
        let why = state.and_then(crate::core::pdm::reason);
        match (&part.error, why) {
            (Some(e), _) => {
                header.header_response.on_hover_text(message(e));
            }
            (None, Some(w)) => {
                header
                    .header_response
                    .on_hover_text(format!("release state unknown: {w}"));
            }
            (None, None) => {}
        }
        if !damage.is_empty() {
            let badge = format!("{} doctor issue(s)", damage.len());
            ui.colored_label(ui.visuals().warn_fg_color, badge)
                .on_hover_text(damage.join("\n"));
        }
    }
    if let Some(c) = choose {
        view.chosen = c;
        view.marked.clear();
        ws.open_form(view.chosen.clone());
        ws.choose(view.chosen.clone());
        // A sketch frames itself; anything else its solid.
        if let Some(k) = view.chosen.clone() {
            let body = ws.model.solid_of(&k).unwrap_or(k);
            view.fit(ws, Some(&body));
        }
    }
    if let Some(c) = gone {
        view.chosen = c;
        ws.open_form(None);
        ws.choose(None);
    }
}

/// In a compare, what changed in the feature chosen in the tree since the
/// revision compared with.
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

/// Everyone attached, what they say they are doing, and "Follow" for
/// anyone with a camera.
pub fn people(ui: &mut egui::Ui, ws: &mut Workspace) {
    ui.horizontal_wrapped(|ui| {
        let me = ws.me().to_owned();
        let mut follow = None;
        for p in ws.people.all.values() {
            let colour = if p.id == me {
                style::SELECTED
            } else {
                style::participant(&p.id)
            };
            let kind = match p.kind {
                Kind::Human => "",
                Kind::Agent => " [agent]",
            };
            let you = if p.id == me { " (you)" } else { "" };
            ui.label(RichText::new(format!("{}{kind}{you}", p.name)).color(style::color32(colour)));
            if let Some((text, _)) = &p.status {
                ui.weak(text);
            }
            if p.id != me && p.camera.is_some() {
                let on = ws.following.as_deref() == Some(p.id.as_str());
                if ui.selectable_label(on, "Follow").clicked() {
                    follow = Some(if on { None } else { Some(p.id.clone()) });
                }
            }
            ui.separator();
        }
        if let Some(f) = follow {
            ws.following = f;
        }
    });
}

/// A compare: the revision, what differs and the colours that say so,
/// and Stop; or, while one is typed (`typed`), the revision to compare
/// with.
pub fn compare_bar(ui: &mut egui::Ui, ws: &mut Workspace, typed: &mut Option<String>) {
    ui.horizontal_wrapped(|ui| {
        if let Some(text) = typed {
            ui.label("Compare with");
            let edit = egui::TextEdit::singleline(text)
                .hint_text("HEAD, a branch or a commit")
                .desired_width(180.0);
            let r = ui.add(edit);
            r.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "revision to compare with")
            });
            let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Start comparing").clicked() || go) && !text.trim().is_empty() {
                ws.set_compare(Some(text.trim().to_owned()));
                *typed = None;
            } else if ui.button("Cancel").clicked() {
                *typed = None;
            }
            return;
        }
        let Some(c) = &ws.compare else { return };
        let commit = c
            .commit
            .as_deref()
            .map_or(String::new(), |c| format!(" ({})", &c[..c.len().min(10)]));
        ui.label(format!("Comparing with {}{commit}", c.rev));
        ui.separator();
        match &c.error {
            Some(e) => {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
            }
            None if !c.loaded => {
                ui.spinner();
            }
            None => {
                let (added, removed, changed) = c.counts();
                for (n, what, colour) in [
                    (added, "added", style::NEW),
                    (removed, "removed", style::REMOVED),
                    (changed, "changed", style::CHANGED),
                ] {
                    let text = RichText::new(format!("{n} {what}")).color(style::color32(colour));
                    ui.label(text);
                }
            }
        }
        ui.separator();
        if ui.button("Stop comparing").clicked() {
            ws.set_compare(None);
        }
    });
}

/// A value of a conflict's side, short enough for the panel.
fn side(v: &serde_json::Value) -> String {
    let text = match v {
        serde_json::Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    match text.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text,
    }
}

/// The merge Git is in the middle of: each conflict with what it
/// concerns, its sides and who caused it, and a button per choice and
/// planned repair (`merge.resolve`); `merge_broke` conflicts are fixed in
/// the model. "Show" chooses the feature and marks the entities it
/// concerns.
pub fn merge(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    let Some(m) = &ws.merge else { return };
    ui.heading(format!("{} in progress", m.step()));
    ui.label(m.summary());
    ui.weak(
        "ryucad never stages or commits: once everything is settled, git add the files and commit.",
    );
    if let Some(e) = m.error.as_ref().or(m.failed.as_ref()) {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
    }
    let mut act: Option<(String, String, Option<String>)> = None;
    let mut show: Option<serde_json::Value> = None;
    for c in m.conflicts() {
        let id = c["id"].as_str().unwrap_or_default();
        let concerns = &c["concerns"];
        egui::CollapsingHeader::new(crate::core::merge::title(c))
            .id_salt(("conflict", id))
            .show(ui, |ui| {
                ui.label(c["message"].as_str().unwrap_or_default());
                ui.weak(format!(
                    "caused by {}",
                    c["cause"].as_str().unwrap_or("both")
                ));
                for k in ["base", "ours", "theirs"] {
                    if let Some(v) = c.get(k) {
                        ui.label(format!("{k}: {}", side(v)));
                    }
                }
                if let Some(h) = crate::core::merge::hint(c) {
                    ui.weak(h);
                }
                ui.horizontal_wrapped(|ui| {
                    let choices = c["choices"].as_array().into_iter().flatten();
                    for choice in choices.filter_map(|x| x.as_str()) {
                        let label = match choice {
                            "ours" => "Take ours",
                            "theirs" => "Take theirs",
                            // A value is typed: the CLI's `--value`.
                            _ => continue,
                        };
                        if ui
                            .add_enabled(!m.resolving, egui::Button::new(label))
                            .clicked()
                        {
                            act = Some((id.to_owned(), choice.to_owned(), None));
                        }
                    }
                    for r in c["repairs"].as_array().into_iter().flatten() {
                        let summary = r["summary"].as_str().unwrap_or("Repair");
                        let button = egui::Button::new(summary);
                        if ui.add_enabled(!m.resolving, button).clicked() {
                            let rid = r["id"].as_str().map(str::to_owned);
                            act = Some((id.to_owned(), "repair".to_owned(), rid));
                        }
                    }
                    if concerns["feature"].is_string() && ui.button("Show").clicked() {
                        show = Some(concerns.clone());
                    }
                });
            });
    }
    let unstaged = m.unstaged();
    if !unstaged.is_empty() {
        ui.label(format!(
            "Settled, waiting for git add: {}",
            unstaged.join(", ")
        ));
    }
    for f in m.status["already_failing"].as_array().into_iter().flatten() {
        ui.weak(format!(
            "{}/{} already failed before the merge ({})",
            f["part"].as_str().unwrap_or_default(),
            f["feature"].as_str().unwrap_or_default(),
            f["error"].as_str().unwrap_or_default()
        ));
    }
    if m.resolving {
        ui.spinner();
    }
    ui.separator();
    if let Some((id, choice, repair)) = act {
        ws.resolve(&id, &choice, repair.as_deref());
    }
    if let Some(c) = show {
        let part = c["part"].as_str().unwrap_or_default().to_owned();
        let feature = c["feature"].as_str().unwrap_or_default().to_owned();
        view.marked = c["entities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| Some(format!("{part}/{feature}/{}", e.as_str()?)))
            .collect();
        let key = (part, feature);
        view.chosen = Some(key.clone());
        ws.open_form(Some(key.clone()));
        view.fit(ws, Some(&key));
    }
}

/// The connection, what agents are doing, and the model's state.
pub fn status(ui: &mut egui::Ui, ws: &Workspace) {
    ui.horizontal(|ui| {
        if ws.lost {
            let why = ws
                .lost_why
                .as_ref()
                .map_or(String::new(), |e| format!(": {}", e.message));
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Reconnecting to the daemon{why}"),
            );
        } else {
            ui.label("Connected");
        }
        ui.separator();
        let busy: Vec<String> = ws
            .people
            .others(ws.me())
            .filter_map(|p| p.status.as_ref().map(|(s, _)| format!("{}: {s}", p.name)))
            .collect();
        if !busy.is_empty() {
            ui.label(busy.join(" · "));
            ui.separator();
        }
        let features: usize = ws.model.parts.values().map(|p| p.features.len()).sum();
        let failed: usize = ws
            .model
            .parts
            .values()
            .flat_map(|p| &p.features)
            .filter(|f| f.error.is_some())
            .count();
        ui.label(format!(
            "{} part(s), {features} feature(s)",
            ws.model.parts.len()
        ));
        if failed > 0 {
            ui.colored_label(ui.visuals().error_fg_color, format!("{failed} failed"));
        }
        if !ws.idle() {
            ui.spinner();
        }
        if crate::ui::platform::software() {
            ui.separator();
            ui.weak("Software rendering");
        }
        if let Some(m) = &ws.merge {
            ui.separator();
            ui.colored_label(ui.visuals().warn_fg_color, m.summary());
        }
        if let Some(e) = ws.shown_error(std::time::Instant::now()) {
            ui.separator();
            ui.colored_label(ui.visuals().error_fg_color, &e.message);
        }
    });
}

/// A row choosing one of a list, by name.
fn choice_row(ui: &mut egui::Ui, c: &mut crate::core::edit::Choice) {
    ui.label(&c.name);
    let r = egui::ComboBox::from_id_salt(("choice", &c.name))
        .selected_text(&c.chosen)
        .show_ui(ui, |ui| {
            for o in &c.options {
                ui.selectable_value(&mut c.chosen, o.clone(), o);
            }
        });
    let r = r.response;
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, &c.name));
    if c.dirty() {
        r.on_hover_text(format!("was {}", c.read));
    }
    ui.end_row();
}

/// Some of a list, each a checkbox, in the order picked.
fn picks(ui: &mut egui::Ui, p: &mut crate::core::edit::Picks) {
    ui.label(format!("{}: {}", p.name, p.chosen.join(", ")));
    ui.horizontal_wrapped(|ui| {
        for o in p.options.clone() {
            let mut on = p.chosen.contains(&o);
            if ui.checkbox(&mut on, &o).changed() {
                p.toggle(&o);
            }
        }
    });
}

fn field_row(ui: &mut egui::Ui, f: &mut crate::core::edit::Field, label: &str) {
    ui.label(label);
    let edit = egui::TextEdit::singleline(&mut f.text).desired_width(110.0);
    let r = ui.add(edit);
    if f.dirty() {
        r.on_hover_text(format!("was {}", f.read));
    }
    ui.end_row();
}

/// The parameters of the chosen feature's part (or the first part), each
/// editable, with a row to add one. Apply sends `param.set` against the
/// revision they were read at.
pub fn parameters(ui: &mut egui::Ui, ws: &mut Workspace, view: &View) {
    let part = view
        .chosen
        .as_ref()
        .map(|k| k.0.clone())
        .or_else(|| ws.model.parts.keys().next().cloned());
    ws.show_params(part.as_deref());
    let Some(p) = &mut ws.params else {
        return;
    };
    ui.heading(format!("Parameters of {}", p.part));
    if !p.changed.is_empty() {
        let frame =
            egui::Frame::group(ui.style()).fill(ui.visuals().warn_fg_color.gamma_multiply(0.15));
        frame.show(ui, |ui| {
            ui.label("Someone changed these parameters while you edited them. They now read:");
            for c in &p.changed {
                ui.label(format!("{}: {} (you typed {})", c.name, c.theirs, c.yours));
            }
            if ui.button("OK").clicked() {
                p.changed.clear();
            }
        });
    }
    if p.params.is_empty() {
        ui.weak("No parameters yet");
    }
    egui::Grid::new("parameters").num_columns(3).show(ui, |ui| {
        for row in &mut p.params {
            let name = ui.label(&row.field.name);
            let edit = egui::TextEdit::singleline(&mut row.field.text).desired_width(110.0);
            let r = ui.add(edit).labelled_by(name.id);
            if row.field.dirty() {
                r.on_hover_text(format!("was {}", row.field.read));
            }
            let shown = match &row.error {
                Some(_) => ui.colored_label(ui.visuals().error_fg_color, "fails"),
                None => ui.weak(&row.value),
            };
            match (&row.error, row.used_by.is_empty()) {
                (Some(e), _) => {
                    shown.on_hover_text(e);
                }
                (None, false) => {
                    shown.on_hover_text(format!("used by {}", row.used_by.join(", ")));
                }
                (None, true) => {}
            }
            ui.end_row();
        }
        let add = egui::TextEdit::singleline(&mut p.new_name)
            .hint_text("new name")
            .desired_width(70.0);
        let named = ui.add(add).on_hover_text("A new parameter's name");
        named.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new parameter's name")
        });
        let value = egui::TextEdit::singleline(&mut p.new_value)
            .hint_text("value")
            .desired_width(110.0);
        let valued = ui.add(value).on_hover_text("A new parameter's expression");
        valued.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new parameter's value")
        });
        ui.end_row();
    });
    if let Some(e) = &p.error {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
    }
    let (dirty, saving) = (p.dirty(), p.saving);
    let mut save = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(dirty && !saving, egui::Button::new("Apply parameters"))
            .clicked();
        if ui
            .add_enabled(dirty && !saving, egui::Button::new("Revert parameters"))
            .clicked()
        {
            for row in &mut p.params {
                row.field.text.clone_from(&row.field.read);
            }
            p.new_name.clear();
            p.new_value.clear();
            p.error = None;
        }
        if saving {
            ui.spinner();
        }
    });
    if save {
        ws.save_params();
    }
}

/// One property's control, by its type: a line or a box of text, a
/// choice, or a checkbox; labelled by `name` for accessibility.
fn prop_control(ui: &mut egui::Ui, prop: &mut crate::core::props::Prop, name: egui::Id) {
    use crate::core::props::Control;
    let f = &mut prop.field;
    let r = match &prop.control {
        Control::Text { multiline: true } => ui.add(
            egui::TextEdit::multiline(&mut f.text)
                .desired_rows(2)
                .desired_width(150.0),
        ),
        Control::Boolean => {
            let mut on = f.text == "true";
            let r = ui.checkbox(&mut on, "");
            if r.changed() {
                f.text = on.to_string();
            }
            r
        }
        Control::Enum(values) => {
            let shown = if f.text.is_empty() {
                "(not set)".to_owned()
            } else {
                f.text.clone()
            };
            egui::ComboBox::from_id_salt(("property", &f.name))
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.text, String::new(), "(not set)");
                    for v in values {
                        ui.selectable_value(&mut f.text, v.clone(), v);
                    }
                })
                .response
        }
        _ => ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(150.0)),
    };
    let r = r.labelled_by(name);
    if f.dirty() {
        r.on_hover_text(format!(
            "was {}",
            if f.read.is_empty() {
                "not set"
            } else {
                &f.read
            }
        ));
    }
}

/// The Part section: the chosen feature's part (or the first part), its
/// release state and every property, each with a control for its type,
/// marked required (`*`) or revising, its faults beside it. Apply sends
/// `props.set` against the revision they were read at. With PDM not set
/// up, Set up PDM (`pdm.init`) gives the project its ID.
pub fn part_section(ui: &mut egui::Ui, ws: &mut Workspace, view: &View) {
    let part = view
        .chosen
        .as_ref()
        .map(|k| k.0.clone())
        .or_else(|| ws.model.parts.keys().next().cloned());
    ws.show_props(part.as_deref());
    if ws.pdm_config.is_some() && !ws.pdm_ready() {
        ui.weak("PDM isn't set up: the project has no ID for release records yet.");
        if ui.button("Set up PDM").clicked() {
            ws.pdm_init();
        }
    }
    let badge = part.as_deref().and_then(|p| ws.states.badge(p));
    let Some(p) = &mut ws.props else {
        return;
    };
    ui.heading(format!("Properties of {}", p.part));
    if let Some(b) = badge {
        ui.weak(format!("Release state: {b}"));
    }
    if !p.changed.is_empty() {
        let frame =
            egui::Frame::group(ui.style()).fill(ui.visuals().warn_fg_color.gamma_multiply(0.15));
        frame.show(ui, |ui| {
            ui.label("Someone changed these properties while you edited them. They now read:");
            for c in &p.changed {
                ui.label(format!("{}: {} (you typed {})", c.name, c.theirs, c.yours));
            }
            if ui.button("OK").clicked() {
                p.changed.clear();
            }
        });
    }
    let error = ui.visuals().error_fg_color;
    egui::Grid::new("properties").num_columns(3).show(ui, |ui| {
        for prop in &mut p.props {
            let star = if prop.required { " *" } else { "" };
            let name = ui.label(format!("{}{star}", prop.field.name));
            prop_control(ui, prop, name.id);
            if prop.revising {
                ui.weak("revising").on_hover_text(
                    "A release records it: changing it is work towards the next revision",
                );
            } else {
                ui.label("");
            }
            ui.end_row();
            for fault in &prop.faults {
                ui.label("");
                ui.colored_label(error, fault);
                ui.end_row();
            }
        }
    });
    if let Some(e) = &p.error {
        ui.colored_label(error, &e.message);
    }
    let (dirty, saving) = (p.dirty(), p.saving);
    let mut save = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(dirty && !saving, egui::Button::new("Apply properties"))
            .clicked();
        if ui
            .add_enabled(dirty && !saving, egui::Button::new("Revert properties"))
            .clicked()
        {
            for prop in &mut p.props {
                prop.field.text.clone_from(&prop.field.read);
            }
            p.error = None;
        }
        if saving {
            ui.spinner();
        }
    });
    if save {
        ws.save_props();
    }
}

/// A sketch dimension's value, edited in place. Apply sends `sketch.edit`
/// against the revision it was read at.
fn dimension(ui: &mut egui::Ui, ws: &mut Workspace) {
    let Some(form) = &mut ws.dimension else {
        return;
    };
    ui.heading(&form.constraint);
    ui.label(format!(
        "{} in sketch {} of part {}",
        form.ty, form.sketch, form.part
    ));
    if let Some(c) = &form.changed {
        let frame =
            egui::Frame::group(ui.style()).fill(ui.visuals().warn_fg_color.gamma_multiply(0.15));
        let mut ok = false;
        frame.show(ui, |ui| {
            ui.label("Someone changed this dimension while you edited it. It now reads:");
            ui.label(format!("{} (you typed {})", c.theirs, c.yours));
            ok = ui.button("OK").clicked();
        });
        if ok {
            form.changed = None;
        }
    }
    ui.separator();
    egui::Grid::new("dimension").num_columns(2).show(ui, |ui| {
        field_row(ui, &mut form.value, "value");
    });
    if let Some(e) = &form.error {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
    }
    let (dirty, saving) = (form.dirty(), form.saving);
    let mut save = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(dirty && !saving, egui::Button::new("Apply"))
            .clicked();
        if ui
            .add_enabled(dirty && !saving, egui::Button::new("Revert"))
            .clicked()
        {
            form.value.text.clone_from(&form.value.read);
            form.error = None;
        }
        if saving {
            ui.spinner();
        }
    });
    if save {
        ws.save_dimension();
    }
}

/// The chosen feature's parameters and placement, edited in place. Apply
/// sends `feature.edit` against the revision the form was read at. A
/// dimension chosen by its label shows instead.
pub fn properties(ui: &mut egui::Ui, ws: &mut Workspace) {
    if ws.dimension.is_some() {
        dimension(ui, ws);
        return;
    }
    let Some(form) = &mut ws.form else {
        ui.weak("Choose a feature in the tree to see and edit it");
        return;
    };
    ui.heading(&form.feature);
    ui.label(format!("{} in part {}", form.ty, form.part));
    if !form.changed.is_empty() {
        let frame =
            egui::Frame::group(ui.style()).fill(ui.visuals().warn_fg_color.gamma_multiply(0.15));
        frame.show(ui, |ui| {
            ui.label("Someone changed this feature while you edited it. It now reads:");
            for c in &form.changed {
                ui.label(format!("{}: {} (you typed {})", c.name, c.theirs, c.yours));
            }
            if ui.button("OK").clicked() {
                form.changed.clear();
            }
        });
    }
    ui.separator();
    egui::Grid::new("params").num_columns(2).show(ui, |ui| {
        for f in &mut form.params {
            let name = f.name.clone();
            field_row(ui, f, &name);
        }
        if let Some(e) = &mut form.extent {
            choice_row(ui, &mut e.ty);
            if matches!(e.ty.chosen.as_str(), "blind" | "symmetric") {
                field_row(ui, &mut e.length, "length");
            }
            if e.ty.chosen == "through_all" {
                ui.label("");
                ui.checkbox(&mut e.both.chosen, "both ways");
                ui.end_row();
            }
        }
        for c in [&mut form.operation, &mut form.op, &mut form.target]
            .into_iter()
            .flatten()
        {
            choice_row(ui, c);
        }
    });
    // What it acts on: none for a new solid.
    let new = form.operation.as_ref().is_some_and(|o| o.chosen == "new");
    for p in [&mut form.targets, &mut form.tools].into_iter().flatten() {
        if !new {
            picks(ui, p);
        }
    }
    if form.placed() {
        ui.collapsing("Placement", |ui| {
            egui::Grid::new("placement").num_columns(2).show(ui, |ui| {
                for (axis, fields) in [
                    ("origin", &mut form.origin),
                    ("z axis", &mut form.z),
                    ("x axis", &mut form.x),
                ] {
                    for (k, f) in ["x", "y", "z"].iter().zip(fields.iter_mut()) {
                        field_row(ui, f, &format!("{axis} {k}"));
                    }
                }
            });
        });
    }
    if let Some(e) = &form.error {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
    }
    let (dirty, saving) = (form.dirty(), form.saving);
    let mut save = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(dirty && !saving, egui::Button::new("Apply"))
            .clicked();
        if ui
            .add_enabled(dirty && !saving, egui::Button::new("Revert"))
            .clicked()
        {
            form.revert();
        }
        if saving {
            ui.spinner();
        }
    });
    let key = (form.part.clone(), form.feature.clone());
    if save {
        ws.save_form();
    }
    names(ui, ws, &key);
}

/// The stable names of the faces, edges and vertices of a feature's
/// solid as the model shows it, to copy.
fn names(ui: &mut egui::Ui, ws: &Workspace, key: &(String, String)) {
    let Some(key) = ws.model.solid_of(key) else {
        return;
    };
    let key = &key;
    let Some(mesh) = ws.model.meshes.get(key).and_then(|m| m.mesh.as_ref().ok()) else {
        return;
    };
    ui.collapsing("Stable names", |ui| {
        let reference = |n: &[u8; 16]| format!("{}/{}/{}", key.0, key.1, ryucad::mesh::name_hex(n));
        let mut rows: Vec<(&str, String)> = Vec::new();
        rows.extend(mesh.faces.iter().map(|f| ("face", reference(&f.name))));
        rows.extend(mesh.edges.iter().map(|(n, _)| ("edge", reference(n))));
        rows.extend(mesh.vertices.iter().map(|(n, _)| ("vertex", reference(n))));
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(ui, |ui| {
                for (kind, r) in rows {
                    ui.horizontal(|ui| {
                        ui.label(kind);
                        ui.monospace(&r[r.len() - 32..]);
                        if ui.small_button("Copy").clicked() {
                            ui.ctx().copy_text(r.clone());
                        }
                    });
                }
            });
    });
}

/// The bottom panel's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Activity,
    History,
}

/// How long ago `ms` (Unix milliseconds) was, for people.
pub fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => format!("{s} s ago"),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// The activity log, newest first, with undo and redo per entry; and the
/// project's Git history, read-only.
pub fn journal(ui: &mut egui::Ui, ws: &mut Workspace, tab: &mut Tab) {
    // A short tab still fills the panel, so the view keeps its size.
    ui.set_min_size(ui.available_size());
    ui.horizontal(|ui| {
        ui.selectable_value(tab, Tab::Activity, "Activity");
        if ui.selectable_value(tab, Tab::History, "History").clicked() {
            ws.load_history();
        }
    });
    ui.separator();
    match tab {
        Tab::Activity => activity(ui, ws),
        Tab::History => history(ui, ws),
    }
}

fn activity(ui: &mut egui::Ui, ws: &mut Workspace) {
    let mut undo = None;
    let mut redo = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            if ws.activity.entries.is_empty() {
                ui.weak("Nothing has changed since the daemon started");
            }
            egui::Grid::new("activity")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    for e in ws.activity.entries.iter().rev() {
                        let seq = e["seq"].as_u64().unwrap_or_default();
                        ui.weak(ago(e["time_ms"].as_u64().unwrap_or_default()));
                        let who = e["participant"]["name"]
                            .as_str()
                            .unwrap_or("outside ryucad");
                        ui.label(who);
                        ui.label(ws.activity.describe(e));
                        let files: Vec<String> = e["files"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|f| {
                                format!(
                                    "{} {}",
                                    f["change"].as_str().unwrap_or(""),
                                    f["path"].as_str().unwrap_or("")
                                )
                            })
                            .collect();
                        ui.label(format!("{} file(s)", files.len()))
                            .on_hover_text(files.join("\n"));
                        if crate::core::activity::can_undo(e) {
                            if ui.small_button("Undo").clicked() {
                                undo = Some(seq);
                            }
                        } else if crate::core::activity::can_redo(e) {
                            if ui.small_button("Redo").clicked() {
                                redo = Some(seq);
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(s) = undo {
        ws.undo(Some(s));
    }
    if let Some(s) = redo {
        ws.redo(Some(s));
    }
}

fn history(ui: &mut egui::Ui, ws: &mut Workspace) {
    if ui.small_button("Refresh").clicked() {
        ws.load_history();
    }
    let Some(h) = &ws.history else {
        ui.spinner();
        return;
    };
    if let Some(e) = h["error"].as_str() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Can't read the history: {e}"),
        );
        return;
    }
    if h["repository"] == false {
        ui.weak("This project isn't in a Git repository, so it has no history");
        return;
    }
    let compared = ws.compare.as_ref().map(|c| c.rev.clone());
    let mut pick = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let uncommitted = h["uncommitted"].as_array().map_or(0, Vec::len);
            if uncommitted > 0 {
                ui.label(format!("{uncommitted} uncommitted change(s)"));
            }
            egui::Grid::new("history")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    for c in h["commits"].as_array().into_iter().flatten() {
                        let id = c["id"].as_str().unwrap_or_default();
                        ui.monospace(&id[..id.len().min(10)]);
                        ui.label(c["author"].as_str().unwrap_or_default());
                        ui.weak(ago(c["time"].as_u64().unwrap_or_default() * 1000));
                        ui.label(c["subject"].as_str().unwrap_or_default());
                        let on = compared.as_deref() == Some(id);
                        if ui
                            .selectable_label(on, "Compare")
                            .on_hover_text("Compare the working files with this commit")
                            .clicked()
                        {
                            pick = Some((!on).then(|| id.to_owned()));
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(rev) = pick {
        ws.set_compare(rev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Feature, SketchState};
    use serde_json::json;

    fn feature(ty: &str) -> Feature {
        Feature {
            id: "f".into(),
            ty: Some(ty.into()),
            hash: None,
            sketch: None,
            uses: Vec::new(),
            operation: None,
            solids: Vec::new(),
            consumes: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn marks_say_how_a_feature_differs() {
        let mark = |change, moved, failed: Option<&str>| Mark {
            change,
            moved,
            failed_there: failed.map(str::to_owned),
            lines: Vec::new(),
        };
        assert_eq!(
            mark_text(&mark(Change::Changed, true, None), "HEAD"),
            " [changed, moved]"
        );
        assert_eq!(
            mark_text(
                &mark(Change::Rebuilt, false, Some("rebuild_failed")),
                "main"
            ),
            " [rebuilt, failed at main (rebuild_failed)]"
        );
        assert_eq!(mark_text(&mark(Change::Same, false, None), "HEAD"), "");
        assert_eq!(
            mark_text(&mark(Change::Same, true, None), "HEAD"),
            " [moved]"
        );
    }

    #[test]
    fn parts_say_their_release_state_and_proposals() {
        assert_eq!(part_text("p", None, &[]), "p");
        assert_eq!(part_text("p", Some("in work"), &[]), "p (in work)");
        assert_eq!(
            part_text("p", Some("A modified, B proposed"), &["proposes B".into()]),
            "p (A modified, B proposed) [proposes B]"
        );
    }

    #[test]
    fn rows_say_state_and_failures() {
        assert_eq!(row_text(&feature("box"), 0), "f (box)");
        assert_eq!(row_text(&feature("box"), 2), "f (box): 2 unmeshed");
        let mut s = feature("sketch");
        s.sketch = Some(SketchState {
            state: "under".into(),
            dof: 3,
            stale: false,
        });
        assert_eq!(row_text(&s, 0), "f (sketch, under, 3 dof)");
        s.sketch = Some(SketchState {
            state: "well".into(),
            dof: 0,
            stale: true,
        });
        assert_eq!(row_text(&s, 0), "f (sketch, well, stale)");
        let mut e = feature("extrude");
        e.error = Some(json!({"code": "dependency_failed", "message": "s failed"}));
        assert_eq!(row_text(&e, 0), "f (extrude): failed (dependency_failed)");
    }
}
