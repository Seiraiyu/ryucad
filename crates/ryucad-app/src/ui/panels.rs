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
    use crate::core::model::MateInfo;
    let ty = match (&f.ty, &f.operation, &f.instance) {
        // An instance says what it places, and what its mates leave it.
        (_, _, Some(i)) => {
            let freedom = match &i.solve {
                _ if i.fixed => ", fixed".to_owned(),
                Some((state, _)) if state == "failing" => ", failing".to_owned(),
                Some((_, dof)) => format!(", {dof} dof"),
                None => String::new(),
            };
            format!("{} @ {}{freedom}", i.part, i.rev)
        }
        // A mate its form and what it joins, and a redundant or
        // conflicting one says so.
        _ if f.mate.is_some() => {
            let m = f.mate.as_ref().expect("a mate");
            let state = match m.state.as_deref() {
                Some("redundant" | "conflicting") => {
                    format!(", {}", m.state.as_deref().unwrap_or_default())
                }
                _ => String::new(),
            };
            format!(
                "{}: {} -> {}{state}",
                m.form,
                MateInfo::path_of(&m.a),
                MateInfo::path_of(&m.b)
            )
        }
        // An instance pattern how many copies it places.
        (Some(t), _, None) if t == "instance_pattern" && f.error.is_none() => {
            format!("{t}, {} copies", f.copies.len())
        }
        (Some(t), Some(op), None) => format!("{t}, {op}"),
        (Some(t), None, None) => t.clone(),
        (None, _, None) => "?".to_owned(),
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

/// What an assembly's line adds: ` (assembly)`, and when it has mates
/// its solve: ` (assembly, under, 2 dof)`, `well`, `over`, `unsolved`, and
/// `stale` while its stored poses don't hold.
pub fn assembly_text(solve: Option<&crate::core::model::Solve>) -> String {
    let Some(s) = solve else {
        return " (assembly)".to_owned();
    };
    let state = match (s.converged, s.state.as_str()) {
        (false, _) => "unsolved".to_owned(),
        (true, "under") => format!("under, {} dof", s.dof),
        (true, state) => state.to_owned(),
    };
    let stale = if s.stale { ", stale" } else { "" };
    format!(" (assembly, {state}{stale})")
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
/// What a tree row asks of the window's visibility: by the reference
/// `visibility.set` takes.
#[derive(Clone, Debug, PartialEq)]
pub enum Seen {
    Hide(String),
    Show(String),
    Isolate(String),
    All,
}

/// A checkbox that shows or hides what `reference` reaches, named for
/// screen readers as `Show <reference>`.
fn visibility_box(ui: &mut egui::Ui, ws: &Workspace, reference: &str, act: &mut Option<Seen>) {
    let mut shown = !ws.model.all_hidden(reference);
    let label = format!("Show {reference}");
    let r = ui.add(egui::Checkbox::without_text(&mut shown));
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, shown, &label));
    let r = r.on_hover_text(if shown {
        "Shown: click to hide (Tab in the view)"
    } else {
        "Hidden: click to show (Shift+Tab in the view)"
    });
    if r.changed() {
        *act = Some(if shown {
            Seen::Show(reference.to_owned())
        } else {
            Seen::Hide(reference.to_owned())
        });
    }
}

/// The context menu of a row that can be hidden: isolating it, and
/// showing everything again.
fn visibility_menu(r: &egui::Response, reference: &str, act: &mut Option<Seen>) {
    r.context_menu(|ui| {
        if ui.button("Isolate").clicked() {
            *act = Some(Seen::Isolate(reference.to_owned()));
            ui.close();
        }
        if ui.button("Show everything").clicked() {
            *act = Some(Seen::All);
            ui.close();
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn feature_row(
    ui: &mut egui::Ui,
    ws: &Workspace,
    view: &View,
    part: &Part,
    f: &crate::core::model::Feature,
    choose: &mut Option<Option<BodyKey>>,
    act: &mut Option<Seen>,
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
    // An instance can be hidden and isolated.
    let reference = format!("{}/{}", part.id, f.id);
    let hideable = part.assembly && f.instance.is_some();
    let r = if hideable {
        let r = ui
            .horizontal(|ui| {
                visibility_box(ui, ws, &reference, act);
                ui.selectable_label(chosen, text)
            })
            .inner;
        visibility_menu(&r, &reference, act);
        r
    } else {
        ui.selectable_label(chosen, text)
    };
    let r = match (&f.error, &f.mate) {
        (Some(e), _) => r.on_hover_text(message(e)),
        // A mate's connectors in full, as an agent would give them.
        (None, Some(m)) => r.on_hover_text(format!("a = {}\nb = {}", m.a, m.b)),
        (None, None) => r,
    };
    if let Some(c) = toggle(ui, &r, chosen, key) {
        *choose = Some(c);
    }
    if !f.uses.is_empty() {
        ui.indent(("uses", &part.id, &f.id), |ui| {
            ui.weak(format!("uses {}", f.uses.join(", ")));
        });
    }
    // A subassembly's own instances, as the window knows that assembly.
    if let Some(i) = &f.instance {
        placed(ui, ws, &i.part, &format!("{}/{}", part.id, f.id), 1);
    }
    // A pattern's copies, under it, each to hide or isolate.
    if !f.copies.is_empty() {
        ui.indent(("copies", &part.id, &f.id), |ui| {
            for c in &f.copies {
                let reference = format!("{}/{c}", part.id);
                let r = ui
                    .horizontal(|ui| {
                        visibility_box(ui, ws, &reference, act);
                        ui.weak(c)
                    })
                    .inner;
                visibility_menu(&r, &reference, act);
            }
        });
    }
}

/// The instances of assembly `id` (placed at `at`), under the instance
/// that places it, to any depth: read-only lines.
fn placed(ui: &mut egui::Ui, ws: &Workspace, id: &str, at: &str, depth: usize) {
    let Some(sub) = ws.model.parts.get(id).filter(|p| p.assembly) else {
        return;
    };
    // A cycle fails its instances; this only guards the drawing.
    if depth > 16 {
        return;
    }
    ui.indent(("placed", at), |ui| {
        for f in &sub.features {
            ui.weak(row_text(f, 0));
            if let Some(i) = &f.instance {
                placed(ui, ws, &i.part, &format!("{at}/{}", f.id), depth + 1);
            }
        }
    });
}

/// A solid in the tree: its founder, its state and mesh faults, and its
/// chain of features in order; a boolean's consumed tools under it.
#[allow(clippy::too_many_arguments)]
fn solid_group(
    ui: &mut egui::Ui,
    ws: &Workspace,
    view: &View,
    part: &Part,
    solid: &str,
    tool: bool,
    choose: &mut Option<Option<BodyKey>>,
    act: &mut Option<Seen>,
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
    let reference = format!("{}/{solid}", part.id);
    let hidden = ws.model.all_hidden(&reference);
    if hidden {
        title.push_str(" (hidden)");
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
                feature_row(ui, ws, view, part, f, choose, act);
                for t in &f.consumes {
                    ui.indent(("tools", &part.id, &f.id, t), |ui| {
                        solid_group(ui, ws, view, part, t, true, choose, act);
                    });
                }
            }
        });
    // A solid shown can be hidden or isolated, a hidden one shown.
    if !tool {
        header.header_response.context_menu(|ui| {
            let (label, seen) = if hidden {
                ("Show", Seen::Show(reference.clone()))
            } else {
                ("Hide", Seen::Hide(reference.clone()))
            };
            if ui.button(label).clicked() {
                *act = Some(seen);
                ui.close();
            }
            if ui.button("Isolate").clicked() {
                *act = Some(Seen::Isolate(reference.clone()));
                ui.close();
            }
            if ui.button("Show everything").clicked() {
                *act = Some(Seen::All);
                ui.close();
            }
        });
    }
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
    let mut act = None;
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
        let mut text = part_text(&part.id, badge.as_deref(), marks);
        if part.assembly {
            text.push_str(&assembly_text(part.solve.as_ref()));
        }
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
                        [] => feature_row(ui, ws, view, part, f, &mut choose, &mut act),
                        // A solid shows where its founder is listed; what
                        // acts on solids shows in them, a consumed solid
                        // under its boolean.
                        [s] if *s == f.id && f.operation.is_none() && part.solid(s).is_some() => {
                            solid_group(ui, ws, view, part, s, false, &mut choose, &mut act);
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
                    let r = ui.selectable_label(chosen, text);
                    if let Some(c) = toggle(ui, &r, chosen, key) {
                        gone = Some(c);
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
    match act {
        Some(Seen::Hide(r)) => ws.set_visibility(vec![r], Vec::new(), Vec::new(), false),
        Some(Seen::Show(r)) => ws.set_visibility(Vec::new(), vec![r], Vec::new(), false),
        Some(Seen::Isolate(r)) => ws.set_visibility(Vec::new(), Vec::new(), vec![r], false),
        Some(Seen::All) => ws.set_visibility(Vec::new(), Vec::new(), Vec::new(), true),
        None => {}
    }
}

/// What a click on a tree row chooses: the row, or with Ctrl/Cmd on the
/// chosen row nothing. A plain click on the chosen row keeps it, so a
/// second click never throws away what its form holds.
fn toggle(
    ui: &egui::Ui,
    r: &egui::Response,
    chosen: bool,
    key: BodyKey,
) -> Option<Option<BodyKey>> {
    if !r.clicked() {
        return None;
    }
    // The release's own modifiers, as egui's clicks are judged at it.
    let command = ui.input(|i| {
        i.modifiers.command
            || i.events.iter().any(|e| {
                matches!(e, egui::Event::PointerButton { pressed: false, modifiers, .. }
                    if modifiers.command)
            })
    });
    match (chosen, command) {
        (true, true) => Some(None),
        (true, false) => None,
        (false, _) => Some(Some(key)),
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

/// The last eight hex digits of a stable name or reference, after what it
/// is: enough to tell a feature's edges apart.
fn short(kind: &str, r: &str) -> String {
    format!("{kind} {}", &r[r.len().saturating_sub(8)..])
}

/// A fillet's or chamfer's groups: each one's form, sizes and face, and
/// its edges, each a checkbox to take it out (adding one is an agent's
/// work); then whether its edges grow along tangent chains.
fn blend(ui: &mut egui::Ui, b: &mut crate::core::edit::BlendForm) {
    let many = b.groups.len() > 1;
    for (i, g) in b.groups.iter_mut().enumerate() {
        if many {
            ui.label(egui::RichText::new(format!("group {}", i + 1)).strong());
        }
        egui::Grid::new(("blend group", i))
            .num_columns(2)
            .show(ui, |ui| {
                if let Some(form) = &g.form {
                    ui.label("form");
                    ui.label(form);
                    ui.end_row();
                }
                for (_, f) in &mut g.sizes {
                    let name = f.name.clone();
                    field_row(ui, f, &name);
                }
                if let Some(face) = &g.face {
                    ui.label("face");
                    ui.monospace(short("face", face)).on_hover_text(face);
                    ui.end_row();
                }
            });
        let e = &mut g.edges;
        ui.label(format!(
            "{}: {} of {}",
            e.name,
            e.chosen.len(),
            e.options.len()
        ));
        ui.horizontal_wrapped(|ui| {
            for o in e.options.clone() {
                let mut on = e.chosen.contains(&o);
                if ui
                    .checkbox(&mut on, short("edge", &o))
                    .on_hover_text(&o)
                    .changed()
                {
                    e.toggle(&o);
                }
            }
        });
    }
    ui.checkbox(&mut b.propagate.chosen, "propagate along tangent chains");
}

/// A hole's form as read, its sizes, its depth (blind with its length,
/// through all, or up to its target, shown), and its points, each a
/// checkbox to take it out (adding one is an agent's work).
fn hole(ui: &mut egui::Ui, h: &mut crate::core::edit::HoleForm) {
    egui::Grid::new("hole").num_columns(2).show(ui, |ui| {
        ui.label("form");
        ui.label(&h.form);
        ui.end_row();
        for (_, f) in &mut h.sizes {
            let name = f.name.clone();
            field_row(ui, f, &name);
        }
        choice_row(ui, &mut h.depth);
        match h.depth.chosen.as_str() {
            "blind" => field_row(ui, &mut h.length, "depth"),
            "up_to" => {
                ui.label("up to");
                let t = h.target.clone().unwrap_or_default();
                ui.monospace(short("face", &t)).on_hover_text(&t);
                ui.end_row();
            }
            _ => {}
        }
    });
    let p = &mut h.points;
    if p.chosen.is_empty() {
        ui.label("points: every point no curve uses");
    } else {
        ui.label(format!("{}: {}", p.name, p.chosen.join(", ")));
    }
    ui.horizontal_wrapped(|ui| {
        for o in p.options.clone() {
            let mut on = p.chosen.contains(&o);
            if ui.checkbox(&mut on, &o).changed() {
                p.toggle(&o);
            }
        }
    });
}

/// A pattern's form, directions, axis or plane as read, its spacings,
/// counts and angle, whether it runs against its directions, its skips
/// as typed, and its seeds, each a checkbox to take it out.
fn pattern(ui: &mut egui::Ui, p: &mut crate::core::edit::PatternForm) {
    egui::Grid::new("pattern").num_columns(2).show(ui, |ui| {
        ui.label("form");
        ui.label(&p.form);
        ui.end_row();
        for (k, r) in &p.refs {
            ui.label(k);
            if r.contains('/') {
                ui.monospace(short("entity", r)).on_hover_text(r);
            } else {
                ui.label(r);
            }
            ui.end_row();
        }
        for (_, f) in &mut p.sizes {
            let name = f.name.clone();
            field_row(ui, f, &name);
        }
        if p.form != "mirror" {
            field_row(ui, &mut p.skip, "skip");
        }
    });
    for (k, f) in &mut p.reverse {
        let label = match k.as_str() {
            "reverse" => "against the direction",
            _ => "against the second direction",
        };
        ui.checkbox(&mut f.chosen, label);
    }
    picks(ui, &mut p.seeds);
}

/// A connector as one short line: its instance path, then its entity's
/// last name digits or `origin`; the whole text on hover.
fn connector_text(c: &str) -> String {
    use crate::core::model::MateInfo;
    let path = MateInfo::path_of(c);
    match MateInfo::face_of(c) {
        Some((_, name)) => format!("{path} : {}", &name[name.len() - 8..]),
        None if c.split('/').next_back().is_some_and(|n| n.len() == 32) => {
            format!("{path} : {}", &c[c.len() - 8..])
        }
        None => format!("{path} : origin"),
    }
}

/// A mate's form, its connectors as read (changing them is an agent's
/// work), the angle and distance its form may drive, and each
/// connector's offset.
fn mate(ui: &mut egui::Ui, m: &mut crate::core::edit::MateForm) {
    egui::Grid::new("mate").num_columns(2).show(ui, |ui| {
        choice_row(ui, &mut m.form);
        for (side, c) in &m.connectors {
            ui.label(side);
            ui.monospace(connector_text(c)).on_hover_text(c);
            ui.end_row();
        }
        let (angle, distance) = m.takes();
        if angle {
            field_row(ui, &mut m.angle, "angle (empty: free)");
        }
        if distance {
            field_row(ui, &mut m.distance, "distance (empty: free)");
        }
    });
    for (side, o) in [("a", &mut m.a), ("b", &mut m.b)] {
        ui.collapsing(format!("Offset of {side}"), |ui| {
            egui::Grid::new(("offset", side))
                .num_columns(2)
                .show(ui, |ui| {
                    for (k, f) in ["x", "y", "z"].iter().zip(o.origin.iter_mut()) {
                        field_row(ui, f, &format!("along {k}"));
                    }
                    field_row(ui, &mut o.rotate, "turn about z");
                });
            ui.checkbox(&mut o.flip.chosen, "turned over");
        });
    }
}

/// The width of a text field in a grid's column: a grid gives a column
/// other than its last only the width it had last frame, at least 40
/// points, so a field there asking for more never gets it (`add_sized`
/// takes it). A share of the panel, within limits.
fn field_width(ui: &egui::Ui, share: f32, min: f32, max: f32) -> f32 {
    (ui.available_width() * share).clamp(min, max)
}

/// Adds a single line text field `width` wide, in a grid cell or not.
fn sized(ui: &mut egui::Ui, edit: egui::TextEdit<'_>, width: f32) -> egui::Response {
    let height = ui.spacing().interact_size.y;
    ui.add_sized([width, height], edit)
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
    let (name_w, value_w) = (
        field_width(ui, 0.3, 70.0, 160.0),
        field_width(ui, 0.45, 110.0, 280.0),
    );
    egui::Grid::new("parameters").num_columns(3).show(ui, |ui| {
        for row in &mut p.params {
            let name = ui.label(&row.field.name);
            let edit = egui::TextEdit::singleline(&mut row.field.text);
            let r = sized(ui, edit, value_w).labelled_by(name.id);
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
        let add = egui::TextEdit::singleline(&mut p.new_name).hint_text("new name");
        let named = sized(ui, add, name_w).on_hover_text("A new parameter's name");
        named.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new parameter's name")
        });
        let value = egui::TextEdit::singleline(&mut p.new_value).hint_text("value");
        let valued = sized(ui, value, value_w).on_hover_text("A new parameter's expression");
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
        for c in [
            &mut form.operation,
            &mut form.op,
            &mut form.target,
            &mut form.rev,
        ]
        .into_iter()
        .flatten()
        {
            choice_row(ui, c);
        }
        if let Some(f) = &mut form.fixed {
            ui.label("");
            ui.checkbox(&mut f.chosen, "fixed where it is");
            ui.end_row();
        }
    });
    if let Some(b) = &mut form.blend {
        blend(ui, b);
    }
    if let Some(h) = &mut form.hole {
        hole(ui, h);
    }
    if let Some(p) = &mut form.pattern {
        pattern(ui, p);
    }
    if let Some(m) = &mut form.mate {
        mate(ui, m);
    }
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
    Releases,
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

/// The activity log, newest first, with undo and redo per entry; the
/// project's Git history, read-only; and the release records of `part`
/// (the chosen feature's part, else the first).
pub fn journal(ui: &mut egui::Ui, ws: &mut Workspace, tab: &mut Tab, part: Option<&str>) {
    // A short tab still fills the panel, so the view keeps its size.
    ui.set_min_size(ui.available_size());
    ui.horizontal(|ui| {
        ui.selectable_value(tab, Tab::Activity, "Activity");
        if ui.selectable_value(tab, Tab::History, "History").clicked() {
            ws.load_history();
        }
        if ui
            .selectable_value(tab, Tab::Releases, "Releases")
            .clicked()
        {
            ws.load_releases();
        }
    });
    ui.separator();
    match tab {
        Tab::Activity => activity(ui, ws),
        Tab::History => history(ui, ws),
        Tab::Releases => releases(ui, ws, part),
    }
}

/// `part`'s release records, the checks' findings about it, and Propose…,
/// Obsolete… and Sync tags…, each opening a dialog to confirm.
fn releases(ui: &mut egui::Ui, ws: &mut Workspace, part: Option<&str>) {
    use crate::core::releases::{Confirm, record_line};
    let Some(part) = part else {
        ui.weak("No parts yet");
        return;
    };
    let badge = ws.states.badge(part);
    let t = &mut ws.releases;
    if t.list.is_none() {
        ui.spinner();
        return;
    }
    let mut dry = false;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(format!("Releases of {part}"));
                if let Some(b) = &badge {
                    ui.weak(b);
                }
            });
            if !t.in_repository() {
                ui.weak("This project isn't in a Git repository: no record can become official.");
            }
            let records = t.records(part);
            if records.is_empty() {
                ui.weak("No release records yet");
            }
            for r in records {
                ui.label(record_line(r));
            }
            let obsoletable = t.obsoletable(part);
            ui.horizontal(|ui| {
                let idle = !t.busy;
                if ui
                    .add_enabled(idle, egui::Button::new("Propose…"))
                    .on_hover_text("Write the record of this part's next revision")
                    .clicked()
                {
                    dry = true;
                }
                let obsolete = egui::Button::new("Obsolete…");
                if ui
                    .add_enabled(idle && obsoletable.is_some(), obsolete)
                    .on_hover_text("Make this part's latest official revision obsolete")
                    .clicked()
                    && let Some(label) = obsoletable.clone()
                {
                    t.note.clear();
                    t.confirm = Some(Confirm::Obsolete {
                        part: part.to_owned(),
                        label,
                    });
                }
                if ui
                    .add_enabled(idle, egui::Button::new("Sync tags…"))
                    .on_hover_text("Create and move the local ryupdm/ tags to match the records")
                    .clicked()
                {
                    t.confirm = Some(Confirm::Sync);
                }
                if t.busy {
                    ui.spinner();
                }
            });
            if let Some(d) = &t.done {
                ui.label(d);
            }
            if let Some(e) = &t.error {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
            }
            let findings = t.findings_of(part);
            if !findings.is_empty() {
                ui.strong("Checks");
                for f in findings {
                    ui.label(format!(
                        "{} {}: {}",
                        f["severity"].as_str().unwrap_or_default(),
                        f["code"].as_str().unwrap_or_default(),
                        f["message"].as_str().unwrap_or_default()
                    ));
                }
            }
        });
    if dry {
        ws.releases.note.clear();
        ws.propose_dry(part);
    }
}

/// The Releases tab's dialogs, placed at the top of `view`: Propose
/// (the label it takes, every reason it can't yet, a note), Obsolete
/// and Sync tags (what it will write), each confirmed or cancelled.
pub fn release_dialogs(ctx: &egui::Context, ws: &mut Workspace, view: egui::Rect) {
    use crate::core::releases::{Confirm, note};
    let t = &mut ws.releases;
    // What a dialog's button asks for: a proposal with its note, or a
    // confirmed obsolete record or sync.
    let mut act: Option<(Confirm, Option<String>)> = None;
    let mut propose: Option<(String, Option<String>)> = None;
    if let Some((part, dry)) = t.proposal.clone() {
        let mut open = true;
        crate::ui::dialogs::window(&format!("Propose {part}"), view)
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                let label = dry["label"].as_str().unwrap_or("?");
                ui.label(format!("It will be revision {label}."));
                let problems = dry["problems"].as_array().cloned().unwrap_or_default();
                if !problems.is_empty() {
                    ui.label("It can't be proposed yet:");
                    for p in &problems {
                        ui.colored_label(ui.visuals().error_fg_color, message(p));
                    }
                }
                let name = ui.label("Note");
                ui.add(egui::TextEdit::multiline(&mut t.note).desired_rows(2))
                    .labelled_by(name.id);
                ui.horizontal(|ui| {
                    let ready = dry["ready"] == true && !t.busy;
                    if ui
                        .add_enabled(ready, egui::Button::new("Propose"))
                        .clicked()
                    {
                        propose = Some((part.clone(), note(&t.note)));
                    }
                    if ui.button("Cancel").clicked() {
                        t.proposal = None;
                    }
                });
            });
        if !open {
            t.proposal = None;
        }
    }
    if let Some(c) = t.confirm.clone() {
        let title = match &c {
            Confirm::Obsolete { part, label } => format!("Make {part} {label} obsolete"),
            Confirm::Sync => "Sync tags".to_owned(),
        };
        let to_sync = t.to_sync();
        let mut open = true;
        crate::ui::dialogs::window(&title, view)
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                let go = match &c {
                    Confirm::Obsolete { part, label } => {
                        ui.label(format!(
                            "This writes an obsolete record for {part} {label}; it becomes official when the release branch holds it."
                        ));
                        let name = ui.label("Note");
                        ui.add(egui::TextEdit::multiline(&mut t.note).desired_rows(2))
                            .labelled_by(name.id);
                        "Make obsolete"
                    }
                    Confirm::Sync => {
                        if to_sync.is_empty() {
                            ui.label("Every tag is where its release is: nothing to do.");
                        }
                        for (tag, what) in &to_sync {
                            ui.label(format!("{what} {tag}"));
                        }
                        ui.weak("Only local tags: nothing is pushed or deleted.");
                        "Sync"
                    }
                };
                ui.horizontal(|ui| {
                    let can = !t.busy && !(c == Confirm::Sync && to_sync.is_empty());
                    if ui.add_enabled(can, egui::Button::new(go)).clicked() {
                        act = Some((c.clone(), note(&t.note)));
                    }
                    if ui.button("Cancel").clicked() {
                        t.confirm = None;
                    }
                });
            });
        if !open {
            t.confirm = None;
        }
    }
    if let Some((part, n)) = propose {
        ws.propose(&part, n);
    }
    match act {
        Some((Confirm::Obsolete { part, label }, n)) => ws.obsolete(&part, &label, n),
        Some((Confirm::Sync, _)) => ws.sync_tags(),
        None => {}
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
    // Commits made in a terminal change no project file: while the tab
    // shows, it reads the history again now and then.
    let next = ws.history_shown(std::time::Instant::now());
    ui.ctx().request_repaint_after(next);
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
            instance: None,
            mate: None,
            copies: Vec::new(),
        }
    }

    #[test]
    fn assembly_features_say_what_their_mates_leave() {
        use crate::core::model::{MateInfo, Placed, Solve};
        let mut pin = feature("instance");
        pin.id = "pin".into();
        pin.instance = Some(Placed {
            part: "bolt".into(),
            rev: "working".into(),
            fixed: false,
            solve: Some(("under".into(), 2)),
        });
        assert_eq!(row_text(&pin, 0), "pin (bolt @ working, 2 dof)");
        pin.instance.as_mut().unwrap().fixed = true;
        assert_eq!(row_text(&pin, 0), "pin (bolt @ working, fixed)");
        let mut m = feature("mate");
        m.mate = Some(MateInfo {
            form: "revolute".into(),
            a: "frame/base/slab/0123456789abcdef0123456789abcdef".into(),
            b: "frame/ring:2/pin".into(),
            state: Some("redundant".into()),
        });
        assert_eq!(
            row_text(&m, 0),
            "f (revolute: base -> ring:2/pin, redundant)"
        );
        let mut ring = feature("instance_pattern");
        ring.copies = vec!["ring:1/pin".into(), "ring:2/pin".into()];
        assert_eq!(row_text(&ring, 0), "f (instance_pattern, 2 copies)");
        let solve = |state: &str, dof, converged, stale| Solve {
            state: state.into(),
            dof,
            converged,
            stale,
        };
        assert_eq!(assembly_text(None), " (assembly)");
        assert_eq!(
            assembly_text(Some(&solve("under", 2, true, true))),
            " (assembly, under, 2 dof, stale)"
        );
        assert_eq!(
            assembly_text(Some(&solve("over", 0, false, false))),
            " (assembly, unsolved)"
        );
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
