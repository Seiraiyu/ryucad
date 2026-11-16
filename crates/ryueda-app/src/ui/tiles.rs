//! ryueda's tiles (design 519, The window) and what each draws.

use crate::core::lists::Finding;
use crate::core::window::Window;
use crate::ui::canvas::{self, Kept};
use eframe::egui;
use egui_tiles::TileId;
use ryu_app::layout::{Dir, Home, Node, Spec, tabs};
use ryu_app::shell::Pane;
use std::time::Instant;

const TILES: [(&str, &str); 9] = [
    ("navigator", "Navigator"),
    ("canvas", "Sheet"),
    ("properties", "Properties"),
    ("findings", "Findings"),
    ("instructions", "Instructions"),
    ("activity", "Activity"),
    ("changes", "Changes"),
    ("history", "History"),
    ("conflicts", "Conflicts"),
];

fn home(id: &str) -> Home {
    match id {
        "navigator" => Home::Left,
        "canvas" => Home::Centre,
        "properties" | "conflicts" => Home::Right,
        _ => Home::Bottom,
    }
}

fn default() -> Node {
    Node::Split(
        Dir::Horizontal,
        vec![
            (tabs(&["navigator"]), 260.0),
            (
                Node::Split(
                    Dir::Vertical,
                    vec![
                        (tabs(&["canvas"]), 600.0),
                        (
                            tabs(&["findings", "instructions", "activity", "changes", "history"]),
                            200.0,
                        ),
                    ],
                ),
                1000.0,
            ),
            (tabs(&["properties"]), 280.0),
        ],
    )
}

pub const SPEC: Spec = Spec {
    tiles: &TILES,
    centre: "canvas",
    home,
    default,
    id: "ryueda tiles",
};

/// What the tiles need besides the window: the canvas's cut outlines and
/// the lists' filters.
#[derive(Default)]
pub struct Local {
    pub kept: Kept,
    pub component_filter: String,
    pub net_filter: String,
    /// A layout area's pick in its group's Apply.
    pub areas: std::collections::BTreeMap<String, crate::core::merge::Pick>,
}

pub struct Behavior<'a> {
    pub w: &'a mut Window,
    pub local: &'a mut Local,
    pub now: Instant,
    /// A reference to copy, when Copy reference was chosen on a row.
    pub copy: &'a mut Option<String>,
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _: TileId, pane: &mut Pane) -> egui_tiles::UiResponse {
        egui::Frame::NONE
            .inner_margin(4.0)
            .show(ui, |ui| match *pane {
                "canvas" => canvas::show(ui, self.w, &mut self.local.kept, self.now),
                "navigator" => navigator(ui, self.w, self.local),
                "properties" => properties(ui, self.w, self.copy),
                "findings" => findings(ui, self.w),
                "instructions" => instructions(ui, self.w),
                "activity" => activity(ui, self.w),
                "changes" => changes(ui, self.w),
                "history" => history(ui, self.w, self.copy),
                "conflicts" => conflicts(ui, self.w, self.local, self.copy),
                _ => {}
            });
        egui_tiles::UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        SPEC.title(pane).unwrap_or(pane).into()
    }

    /// Every tile has its tab, as ryucad-app's: its name for screen
    /// readers and the window tests, and a handle to move it by.
    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }
}

fn tree_node(ui: &mut egui::Ui, w: &mut Window, n: &crate::core::nav::Node) {
    let id = ui.make_persistent_id(("nav", &n.block, &n.layer));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            let here =
                w.shown.block.as_deref() == Some(n.block.as_str()) && w.shown.layer == n.layer;
            let diff = w.scene.as_ref().and_then(|s| s.paper.diff);
            let label = match diff.filter(|_| w.nav.block_changed(&n.block)) {
                Some(d) => {
                    let mut job = egui::text::LayoutJob::default();
                    let style = ui.style().clone();
                    egui::RichText::new(&n.label).append_to(
                        &mut job,
                        &style,
                        egui::FontSelection::Default,
                        egui::Align::Center,
                    );
                    egui::RichText::new(" ●").color(d.changed).append_to(
                        &mut job,
                        &style,
                        egui::FontSelection::Default,
                        egui::Align::Center,
                    );
                    egui::WidgetText::from(job)
                }
                None => egui::WidgetText::from(n.label.as_str()),
            };
            if ui.selectable_label(here, label).clicked() {
                w.show(Some(n.block.clone()), None, n.layer.clone());
            }
            if let Some(k) = w
                .merge
                .as_ref()
                .and_then(|m| m.blocks().get(&n.block).copied())
            {
                ui.colored_label(ui.visuals().warn_fg_color, format!("● {k}"))
                    .on_hover_text(format!("{k} to settle"));
            }
        })
        .body(|ui| {
            let sheets = w.nav.sheets(&n.block).to_vec();
            let sel_net = w
                .selection
                .last()
                .cloned()
                .filter(|r| w.kind_of(r) == "net");
            let reach = sel_net
                .as_ref()
                .map(|r| w.nav.reach(&n.block, r))
                .unwrap_or_default();
            let marks = w
                .merge
                .as_ref()
                .map(|m| m.sheets(&n.block, &w.nav))
                .unwrap_or_default();
            for (sid, name) in sheets {
                let here = w.shown.block.as_deref() == Some(n.block.as_str())
                    && w.shown.sheet.as_deref() == Some(sid.as_str());
                let mark = if reach.contains(&sid) && !here {
                    format!("{name} ●")
                } else {
                    name.clone()
                };
                let changed = w.nav.changed(&n.block, &sid);
                let diff = w.scene.as_ref().and_then(|s| s.paper.diff);
                let text = match (changed, diff) {
                    (Some(_), Some(d)) => {
                        let mut job = egui::text::LayoutJob::default();
                        let style = ui.style().clone();
                        egui::RichText::new(&mark).append_to(
                            &mut job,
                            &style,
                            egui::FontSelection::Default,
                            egui::Align::Center,
                        );
                        egui::RichText::new(" ●").color(d.changed).append_to(
                            &mut job,
                            &style,
                            egui::FontSelection::Default,
                            egui::Align::Center,
                        );
                        egui::WidgetText::from(job)
                    }
                    _ => egui::WidgetText::from(mark),
                };
                let mut r = match marks.get(&sid) {
                    None => ui.selectable_label(here, text),
                    Some(k) => {
                        ui.horizontal(|ui| {
                            let r = ui.selectable_label(here, text);
                            ui.colored_label(ui.visuals().warn_fg_color, format!("● {k}"))
                                .on_hover_text(format!("{k} to settle on this sheet"));
                            r
                        })
                        .inner
                    }
                };
                if let Some(c) = changed {
                    r = r.on_hover_text(format!(
                        "{} added, {} changed, {} moved, {} removed",
                        c.added, c.changed, c.moved, c.removed
                    ));
                }
                if r.clicked() {
                    w.show(Some(n.block.clone()), Some(sid.clone()), n.layer.clone());
                }
                for r in w.nav.regions(&n.block, &sid).to_vec() {
                    let label = r
                        .rsplit_once("region:")
                        .map_or(r.as_str(), |(_, x)| x)
                        .to_owned();
                    if ui.small_button(format!("  {label}")).clicked() {
                        w.go_to(&r);
                    }
                }
            }
            let diff = w.scene.as_ref().and_then(|s| s.paper.diff);
            for (sid, name) in w.nav.removed(&n.block).to_vec() {
                let text = egui::RichText::new(&name).strikethrough();
                let text = match diff {
                    Some(d) => text.color(d.removed),
                    None => text,
                };
                if ui
                    .selectable_label(false, text)
                    .on_hover_text("Only the compared revision has this sheet: shown as it drew it")
                    .clicked()
                {
                    w.show_removed(n.block.clone(), sid.clone());
                }
            }
            if let Some(r) = &sel_net
                && reach.len() > 1
            {
                ui.weak(format!("{r} reaches {} sheets", reach.len()));
            }
            for c in &n.children {
                tree_node(ui, w, c);
            }
        });
}

fn navigator(ui: &mut egui::Ui, w: &mut Window, local: &mut Local) {
    let designs: Vec<String> = w.nav.design_ids().map(str::to_owned).collect();
    egui::ScrollArea::vertical()
        .id_salt("navigator")
        .show(ui, |ui| {
            for d in &designs {
                let tree = w.nav.tree(d);
                tree_node(ui, w, &tree);
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut local.component_filter)
                    .hint_text("Components: designator, value, MPN"),
            );
            let design = w
                .shown
                .design
                .clone()
                .or_else(|| designs.first().cloned())
                .unwrap_or_default();
            for row in w.nav.components(&design, &local.component_filter) {
                let text = format!("{}  {}  {}", row.refdes, row.value, row.mpn);
                let on = w.selection.contains(&row.reference);
                if ui.selectable_label(on, text).clicked() {
                    w.go_to(&row.reference);
                }
            }
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut local.net_filter).hint_text("Nets"));
            let block = w.shown.block.clone().unwrap_or_default();
            for row in w.nav.nets(&block, &local.net_filter) {
                let on = w.selection.contains(&row.reference);
                if ui.selectable_label(on, &row.name).clicked() {
                    w.go_to(&row.reference);
                }
            }
        });
}

fn properties(ui: &mut egui::Ui, w: &mut Window, copy: &mut Option<String>) {
    let Some(p) = w.props.clone() else {
        ui.weak("Nothing selected. Click a part, pin, net or region.");
        return;
    };
    // The reference wraps, and its button sits under it, so neither is
    // cut off in a narrow tile.
    ui.add(egui::Label::new(egui::RichText::new(&p.reference).monospace()).wrap());
    if ui.button("Copy reference").clicked() {
        *copy = Some(p.reference.clone());
    }
    egui::Grid::new("properties").striped(true).show(ui, |ui| {
        for (k, v) in &p.rows {
            ui.label(k);
            ui.label(v);
            ui.end_row();
        }
    });
    if !p.pins.is_empty() {
        ui.separator();
        for (pin, net) in &p.pins {
            ui.label(if net.is_empty() {
                pin.clone()
            } else {
                format!("{pin} → {net}")
            });
        }
    }
    if !p.naming.is_empty() {
        ui.separator();
        ui.strong("Instructions naming it");
        for t in &p.naming {
            ui.label(t);
        }
    }
    if let Some(c) = &w.changes {
        let lines = c.lines_of(&p.reference);
        if !lines.is_empty() {
            ui.separator();
            ui.strong(format!("Changes since {}", c.rev));
            for l in &lines {
                ui.label(l);
            }
        }
    }
}

fn changes(ui: &mut egui::Ui, w: &mut Window) {
    let Some(c) = w.changes.clone() else {
        ui.weak("Not comparing. An agent shows you its work with:");
        ui.horizontal(|ui| {
            ui.monospace("ryueda compare HEAD");
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text("ryueda compare HEAD".into());
            }
        });
        return;
    };
    if !c.loaded() {
        ui.spinner();
    }
    egui::ScrollArea::vertical()
        .id_salt("changes")
        .show(ui, |ui| {
            for (block, rows) in &c.blocks {
                ui.strong(block);
                for (i, r) in rows.iter().enumerate() {
                    ui.push_id((block, i), |ui| {
                        ui.horizontal(|ui| {
                            let s = crate::core::changes::Changes::show(r);
                            let ok = s != crate::core::changes::Show::Nowhere;
                            if ui
                                .add_enabled(ok, egui::Button::new("Show").small())
                                .clicked()
                            {
                                w.show_change(&s);
                            }
                            ui.strong(&r.verb);
                            ui.monospace(&r.reference);
                            if let Some(n) = &r.name {
                                ui.label(n);
                            }
                        });
                        for l in &r.lines {
                            ui.label(format!("    {l}"));
                        }
                    });
                }
            }
            if !c.electrical.is_empty() {
                ui.separator();
                ui.strong("Electrical");
                for (i, e) in c.electrical.iter().enumerate() {
                    ui.push_id(("electrical", i), |ui| {
                        ui.horizontal(|ui| {
                            let b = ui.add_enabled(
                                e.reference.is_some(),
                                egui::Button::new("Show").small(),
                            );
                            if b.clicked()
                                && let Some(r) = &e.reference
                            {
                                w.go_to(r);
                            }
                            ui.label(&e.text);
                        });
                    });
                }
            }
        });
}

/// A commit's time as `YYYY-MM-DD` (UTC), from seconds since the Unix
/// epoch, in integers.
fn date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn history(ui: &mut egui::Ui, w: &mut Window, copy: &mut Option<String>) {
    let Some(h) = w.history.clone() else {
        ui.spinner();
        return;
    };
    if h.not_a_repo() {
        ui.weak(
            "This project's history is Git's: make its folder a repository, and commits show here.",
        );
        ui.horizontal(|ui| {
            ui.monospace("git init");
            if ui.small_button("Copy").clicked() {
                *copy = Some("git init".into());
            }
        });
        return;
    }
    if let Some(e) = &h.error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Can't read the history: {e}"),
        );
        return;
    }
    let compared = w.blink.of.clone();
    let mut pick: Option<Option<String>> = None;
    egui::ScrollArea::vertical()
        .id_salt("history")
        .show(ui, |ui| {
            if h.uncommitted > 0 {
                ui.label(format!("{} uncommitted change(s)", h.uncommitted));
            }
            egui::Grid::new("history")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    for c in &h.commits {
                        ui.monospace(&c.short);
                        ui.label(&c.author);
                        ui.weak(date(c.time));
                        ui.label(&c.subject);
                        let on = compared.as_deref() == Some(c.id.as_str());
                        if ui
                            .selectable_label(on, "Compare")
                            .on_hover_text("Compare the working files with this commit")
                            .clicked()
                        {
                            pick = Some((!on).then(|| c.id.clone()));
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(rev) = pick {
        w.set_compare(rev);
    }
}

fn show_button(ui: &mut egui::Ui, w: &mut Window, refs: &[String], label: &str) {
    // Always drawn, so a list's Show buttons line up with its rows; with
    // nothing to show, disabled.
    let b = ui.add_enabled(!refs.is_empty(), egui::Button::new("Show").small());
    let b = if refs.is_empty() {
        b.on_disabled_hover_text("nothing to show")
    } else {
        b.on_hover_text(label)
    };
    if b.clicked() {
        w.go_to(&refs[0]);
        w.selection = refs.to_vec();
    }
}

fn findings(ui: &mut egui::Ui, w: &mut Window) {
    ui.horizontal(|ui| {
        let running = w.checked == Some(false);
        if ui
            .add_enabled(
                !running,
                egui::Button::new(if running { "Checking…" } else { "Run check" }),
            )
            .clicked()
        {
            w.run_check();
        }
        if let Some(s) = w.scene.as_ref().and_then(|s| s.score) {
            ui.weak(format!("Sheet score {s}"));
        }
    });
    let all: Vec<Finding> = w.findings.all();
    egui::ScrollArea::vertical()
        .id_salt("findings")
        .show(ui, |ui| {
            for (i, f) in all.iter().enumerate() {
                ui.push_id(i, |ui| {
                    ui.horizontal(|ui| {
                        let refs: Vec<String> = f.refs.iter().map(|r| w.nav.local(r)).collect();
                        show_button(ui, w, &refs, &f.code);
                        ui.strong(format!("{} {}", f.severity, f.code));
                        ui.label(&f.message);
                    });
                    if !f.what.is_empty() {
                        ui.collapsing("What, why, fix", |ui| {
                            ui.label(&f.what);
                            ui.label(&f.why);
                            ui.monospace(&f.fix);
                        });
                    }
                });
            }
        });
}

fn instructions(ui: &mut egui::Ui, w: &mut Window) {
    let list = w.instructions.clone();
    egui::ScrollArea::vertical()
        .id_salt("instructions")
        .show(ui, |ui| {
            for i in &list {
                ui.horizontal(|ui| {
                    show_button(ui, w, &i.refs, &i.text);
                    ui.monospace(&i.text);
                    ui.strong(i.state_text());
                    if let Some(why) = &i.why {
                        ui.weak(why);
                    }
                });
            }
        });
}

fn activity(ui: &mut egui::Ui, w: &mut Window) {
    ui.horizontal(|ui| {
        if ui.button("Undo").clicked() {
            w.undo();
        }
        if ui.button("Redo").clicked() {
            w.redo();
        }
    });
    let list = w.activity.clone();
    egui::ScrollArea::vertical()
        .id_salt("activity")
        .show(ui, |ui| {
            for e in &list {
                ui.label(format!("{}  {}", e.seq, e.text));
            }
        });
}

fn conflicts(ui: &mut egui::Ui, w: &mut Window, local: &mut Local, copy: &mut Option<String>) {
    use crate::core::merge::{Batch, Pick, Row, groups, rows};
    let Some(m) = w.merge.clone() else {
        ui.weak("No merge in progress.");
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("{} in progress", m.step()));
        ui.label(m.summary());
        if w.resolve_busy() {
            ui.spinner();
        }
    });
    if let Some(e) = &m.error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!(
                "Can't tell what is left: {} ({})",
                e.message,
                canvas::code_word(e.code)
            ),
        );
    }
    if w.unloaded {
        ui.weak("ryueda.json holds this merge's record, so the project doesn't open: settle it here (written, not logged, as the CLI does), and the sheet opens.");
    }
    if let Some(e) = w.last_resolve().cloned()
        && ui
            .button(format!("Undo: {}", e.text))
            .on_hover_text("Undo the last resolve, as Activity's Undo does")
            .clicked()
    {
        w.undo();
    }
    let busy = w.resolve_busy();
    let (singles, groups) = groups(rows(&m.status));
    let mut send: Option<Batch> = None;
    let mut show: Option<(Row, Option<&'static str>)> = None;
    egui::ScrollArea::vertical()
        .id_salt("conflicts")
        .show(ui, |ui| {
            for g in &groups {
                ui.push_id(("areas", &g.block), |ui| {
                    ui.group(|ui| {
                        ui.strong(format!(
                            "layout_conflict in {}: {} areas, settled together",
                            g.block,
                            g.areas.len()
                        ));
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new("Relay out every area (recommended)"),
                            )
                            .clicked()
                        {
                            send = Some(Batch::relayout_all(g));
                        }
                        for a in &g.areas {
                            ui.push_id(&a.id, |ui| {
                                ui.label(&a.message);
                                ui.horizontal_wrapped(|ui| {
                                    let pick = local.areas.entry(a.id.clone()).or_default();
                                    ui.radio_value(pick, Pick::Relayout, "Relay out");
                                    ui.radio_value(pick, Pick::Ours, "Ours");
                                    ui.radio_value(pick, Pick::Theirs, "Theirs");
                                    shows(ui, a, &mut show);
                                });
                                if let Some(e) = m.refused_for(&a.id) {
                                    refused(ui, e);
                                }
                            });
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Apply")).clicked() {
                            send = Some(Batch::areas(g, &local.areas));
                        }
                    });
                });
            }
            for r in &singles {
                conflict_row(ui, r, &m, busy, &mut show, &mut send, copy);
            }
            finish(ui, &m, copy);
        });
    if let Some(b) = send {
        w.resolve(b);
    }
    match show {
        Some((r, None)) => w.show_conflict(&r),
        Some((r, Some(side))) => w.show_side(&r, side),
        None => {}
    }
}

fn refused(ui: &mut egui::Ui, e: &ryu_doc::error::Error) {
    ui.colored_label(
        ui.visuals().error_fg_color,
        format!("Refused ({}): {}", canvas::code_word(e.code), e.message),
    );
}

/// Show, and Show ours and Show theirs for an item in a block.
fn shows(
    ui: &mut egui::Ui,
    r: &crate::core::merge::Row,
    show: &mut Option<(crate::core::merge::Row, Option<&'static str>)>,
) {
    if ui
        .add_enabled(r.showable(), egui::Button::new("Show").small())
        .clicked()
    {
        *show = Some((r.clone(), None));
    }
    if r.block.is_some() {
        for (label, side) in [("Show ours", "ours"), ("Show theirs", "theirs")] {
            if ui
                .small_button(label)
                .on_hover_text("The sheet as that side's commit draws it, read only")
                .clicked()
            {
                *show = Some((r.clone(), Some(side)));
            }
        }
    }
}

fn conflict_row(
    ui: &mut egui::Ui,
    r: &crate::core::merge::Row,
    m: &crate::core::merge::Merge,
    busy: bool,
    show: &mut Option<(crate::core::merge::Row, Option<&'static str>)>,
    send: &mut Option<crate::core::merge::Batch>,
    copy: &mut Option<String>,
) {
    ui.push_id(&r.id, |ui| {
        ui.group(|ui| {
            ui.strong(&r.title);
            ui.label(&r.message);
            ui.weak(format!("cause: {} · {}", r.cause, r.files.join(", ")));
            if !r.refs.is_empty() {
                ui.weak(format!("concerns {}", r.refs.join(", ")));
            }
            for (side, short, full) in &r.values {
                ui.label(format!("{side}: {short}")).on_hover_text(full);
            }
            ui.horizontal_wrapped(|ui| shows(ui, r, show));
            for a in &r.actions {
                ui.horizontal_wrapped(|ui| {
                    let text = if a.recommended {
                        format!("{} (recommended)", a.label)
                    } else {
                        a.label.clone()
                    };
                    if ui.add_enabled(!busy, egui::Button::new(text)).clicked() {
                        *send = Some(crate::core::merge::Batch::one(r, a));
                    }
                    if let Some(first) = a.plan.first() {
                        ui.weak(first);
                    }
                });
                for l in a.plan.iter().skip(1) {
                    ui.weak(format!("    {l}"));
                }
            }
            if let Some(t) = &r.typed {
                ui.horizontal_wrapped(|ui| {
                    ui.weak("A value of your own:");
                    ui.monospace(t);
                    if ui.small_button("Copy").clicked() {
                        *copy = Some(t.clone());
                    }
                });
            }
            if let Some(h) = &r.hint {
                ui.weak(h);
            }
            if let Some(e) = m.refused_for(&r.id) {
                refused(ui, e);
            }
        });
    });
}

/// What waits for `git add`, what failed already, the warnings, and the
/// commands that finish the merge.
fn finish(ui: &mut egui::Ui, m: &crate::core::merge::Merge, copy: &mut Option<String>) {
    let line = |v: &serde_json::Value| {
        v["message"]
            .as_str()
            .map_or_else(|| v.to_string(), str::to_owned)
    };
    let settled = m.settled();
    if !settled.is_empty() {
        ui.separator();
        ui.strong("Waiting for git add");
        for f in &settled {
            ui.monospace(*f);
        }
    }
    let untracked = m.untracked();
    if !untracked.is_empty() {
        ui.strong("New files Git doesn't hold");
        for f in &untracked {
            ui.monospace(*f);
        }
    }
    for (title, key) in [
        ("Failing on a side already", "already_failing"),
        ("Warnings", "warnings"),
    ] {
        let list = m.status[key].as_array().cloned().unwrap_or_default();
        if !list.is_empty() {
            ui.strong(title);
            for x in &list {
                ui.label(line(x));
            }
        }
    }
    ui.separator();
    ui.weak("ryueda never stages or commits. Once everything is settled, in the project folder:");
    let mut add: Vec<&str> = settled;
    add.extend(untracked);
    let cmds = [
        (!add.is_empty()).then(|| format!("git add {}", add.join(" "))),
        Some("git commit".to_owned()),
    ];
    for c in cmds.into_iter().flatten() {
        ui.horizontal_wrapped(|ui| {
            ui.monospace(&c);
            if ui.small_button("Copy").clicked() {
                *copy = Some(c.clone());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_layout_has_every_tile_once_round_its_canvas() {
        let d = (SPEC.default)();
        let mut tiles = d.tiles();
        assert_eq!(tiles[0], "navigator");
        tiles.sort();
        assert_eq!(
            tiles,
            [
                "activity",
                "canvas",
                "changes",
                "findings",
                "history",
                "instructions",
                "navigator",
                "properties"
            ]
        );
        assert_eq!(SPEC.read(&d.to_value()).root, d);
        assert_eq!(SPEC.centre, "canvas");
        assert_eq!((SPEC.home)("findings"), Home::Bottom);
    }

    #[test]
    fn a_commit_s_date_is_its_utc_day() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_767_225_600), "2026-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
    }
}
