//! The Tree tile: every part, its solids and features, and an assembly's
//! instances, mates and copies, with what a compare marks.

use crate::core::compare::{Change, Mark};
use crate::core::model::{BodyKey, Part};
use crate::core::workspace::Workspace;
use crate::ui::panels::message;
use crate::ui::style;
use crate::ui::view::View;
use eframe::egui::{self, RichText};

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
                "{}: {} → {}{state}",
                m.form,
                MateInfo::path_of(&m.a),
                MateInfo::path_of(&m.b)
            )
        }
        // An import its file and product: `import: bracket.step /
        // BRK-100`, and its body when the product has several.
        _ if f.import.is_some() => {
            let i = f.import.as_ref().expect("an import");
            let mut s = format!("import: {}", i.file_name());
            if let Some((id, _, _)) = &i.product {
                s.push_str(&format!(" / {id}"));
            }
            if i.body > 0 {
                s.push_str(&format!(", body {}", i.body));
            }
            s
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
pub(crate) fn mark_colour(m: &Mark) -> style::Rgb {
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
    /// A context menu's command, with the row's reference.
    Run(&'static str, String),
}

/// A checkbox that shows or hides what `reference` reaches, named for
/// screen readers as `Show <reference>`.
pub(crate) fn visibility_box(
    ui: &mut egui::Ui,
    ws: &Workspace,
    reference: &str,
    act: &mut Option<Seen>,
) {
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

/// A row's context menu: the registry's commands for its kind (copying
/// its reference, framing it, and for a solid, an instance or a copy,
/// isolating it and showing everything), run with its reference.
pub(crate) fn row_menu(
    r: &egui::Response,
    view: &View,
    row: crate::core::command::Row,
    reference: &str,
    act: &mut Option<Seen>,
) {
    r.context_menu(|ui| {
        if let Some((id, _)) = row_items(ui, view, row, reference) {
            *act = Some(Seen::Run(id, reference.to_owned()));
        }
    });
}

/// The registry's items for a row of kind `row`, for a menu already open.
pub(crate) fn row_items(
    ui: &mut egui::Ui,
    view: &View,
    row: crate::core::command::Row,
    reference: &str,
) -> Option<(&'static str, Option<String>)> {
    use crate::core::command::Context;
    crate::ui::commands::context_menu(ui, &view.commands, Context::Row(row), Some(reference))
}

/// What kind of row a feature is.
pub fn row_of(f: &crate::core::model::Feature) -> crate::core::command::Row {
    use crate::core::command::Row;
    if f.instance.is_some() {
        Row::Instance
    } else if f.mate.is_some() {
        Row::Mate
    } else {
        match f.ty.as_deref() {
            Some("sketch") => Row::Sketch,
            Some("pattern" | "instance_pattern") => Row::Pattern,
            _ => Row::Feature,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn feature_row(
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
        ui.horizontal(|ui| {
            visibility_box(ui, ws, &reference, act);
            ui.selectable_label(chosen, text)
        })
        .inner
    } else {
        ui.selectable_label(chosen, text)
    };
    row_menu(&r, view, row_of(f), &reference, act);
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
                row_menu(&r, view, crate::core::command::Row::Copy, &reference, act);
            }
        });
    }
}

/// The instances of assembly `id` (placed at `at`), under the instance
/// that places it, to any depth: read-only lines. One the window hasn't
/// loaded is a closed row, loaded when opened.
pub(crate) fn placed(ui: &mut egui::Ui, ws: &Workspace, id: &str, at: &str, depth: usize) {
    if !ws.model.parts.contains_key(id) && ws.model.listed.get(id).is_some_and(|l| l.assembly) {
        ui.indent(("placed", at), |ui| {
            let r = egui::CollapsingHeader::new(RichText::new(format!("{id} (assembly)")).weak())
                .id_salt(("placed", at))
                .default_open(false)
                .show(ui, |ui| ui.weak("loading…"));
            if r.body_returned.is_some() {
                ws.want(id);
            }
        });
        return;
    }
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
pub(crate) fn solid_group(
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
    if let Some(failed) = shown.and_then(crate::core::model::Solid::failure) {
        title.push_str(&format!(": {failed}"));
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
            if let Some((id, _)) = row_items(ui, view, crate::core::command::Row::Solid, &reference)
            {
                *act = Some(Seen::Run(id, reference.clone()));
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
    // A project too large to load whole lists every part, its rows
    // closed: a part loads when its row opens.
    let whole = ws.model.listed.len() <= crate::core::workspace::LOAD_WHOLE;
    let unloaded: Vec<(&String, &crate::core::model::Listed)> = ws
        .model
        .listed
        .iter()
        .filter(|(id, _)| !ws.model.parts.contains_key(*id))
        .collect();
    for (id, l) in unloaded {
        let state = ws.states.get(id);
        let badge = state.map(crate::core::pdm::badge);
        let mut text = part_text(id, badge.as_deref(), &[]);
        if l.assembly {
            text.push_str(" (assembly)");
        }
        if let Some(n) = l.failed.filter(|n| *n > 0) {
            text.push_str(&format!(" ({n} failed)"));
        }
        // In a project loaded whole the row is only there until its part
        // loads, and mustn't leave its closed state to the part's row; in a
        // large one, a row opened stays open once its part loads.
        let salt = if whole { "listed" } else { "part" };
        let r = egui::CollapsingHeader::new(text)
            .id_salt((salt, id))
            .default_open(false)
            .show(ui, |ui| ui.weak("loading…"));
        row_menu(
            &r.header_response,
            view,
            crate::core::command::Row::Part,
            id,
            &mut act,
        );
        if r.body_returned.is_some() {
            ws.want(id);
        }
    }
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
            .default_open(whole)
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
        row_menu(
            &header.header_response,
            view,
            crate::core::command::Row::Part,
            &part.id,
            &mut act,
        );
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
    ws.load_wanted();
    if let Some(c) = choose {
        view.chosen = c;
        view.marked.clear();
        ws.open_form(view.chosen.clone());
        ws.choose(view.chosen.clone());
        // A sketch frames itself, an instance everything it places,
        // anything else its solid.
        if let Some(k) = view.chosen.clone() {
            let bodies = ws.model.framed(&k);
            view.fit_all(ws, &bodies);
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
        Some(Seen::Run(id, r)) => view.ran = Some((id, Some(r))),
        None => {}
    }
}

/// What a click on a tree row chooses: the row, or with Ctrl/Cmd on the
/// chosen row nothing. A plain click on the chosen row keeps it, so a
/// second click never throws away what its form holds.
pub(crate) fn toggle(
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
            import: None,
            form: None,
        }
    }

    #[test]
    fn imports_say_their_file_and_product() {
        use crate::core::model::ImportInfo;
        let mut f = feature("import");
        f.id = "body".into();
        f.import = Some(ImportInfo::from_json(&json!({
            "file": "imports/bracket.step", "blake3": "ab", "body": 0,
            "product": {"id": "BRK-100", "name": "Bracket"}})));
        assert_eq!(row_text(&f, 0), "body (import: bracket.step / BRK-100)");
        f.import = Some(ImportInfo::from_json(&json!({
            "file": "imports/loose.step", "blake3": "ab", "body": 2})));
        f.error = Some(json!({"code": "import_body_missing", "message": "no body 2"}));
        assert_eq!(
            row_text(&f, 0),
            "body (import: loose.step, body 2): failed (import_body_missing)"
        );
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
            pose: None,
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
            frames: None,
        });
        assert_eq!(
            row_text(&m, 0),
            "f (revolute: base → ring:2/pin, redundant)"
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
