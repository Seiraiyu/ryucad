//! ryuplm's tiles (design 938, The window) and what each draws.

use crate::core::launch;
use crate::core::lists::BomLine;
use crate::core::marks::{Mark, Marking, mark, mark_item};
use crate::core::rows::Row;
use crate::core::tiles::TILES;
use crate::core::window::{STATES, Window};
use eframe::egui::{self, Color32};
use egui_tiles::TileId;
use ryu_app::layout::{Dir, Home, Node, Spec, tabs};
use ryu_app::shell::Pane;
use ryu_doc::error::{Code, Error};
use std::time::Instant;

/// The tabs below Items, in their order.
pub const BOTTOM: [&str; 8] = [
    "bom",
    "where_used",
    "findings",
    "changes",
    "releases",
    "imports",
    "documents",
    "activity",
];

fn home(id: &str) -> Home {
    match id {
        "navigator" => Home::Left,
        "items" => Home::Centre,
        "item" => Home::Right,
        _ => Home::Bottom,
    }
}

fn default() -> Node {
    Node::Split(
        Dir::Horizontal,
        vec![
            (tabs(&["navigator"]), 240.0),
            (
                Node::Split(
                    Dir::Vertical,
                    vec![(tabs(&["items"]), 560.0), (tabs(&BOTTOM), 240.0)],
                ),
                900.0,
            ),
            (tabs(&["item"]), 340.0),
        ],
    )
}

pub const SPEC: Spec = Spec {
    tiles: &TILES,
    centre: "items",
    home,
    default,
    id: "ryuplm tiles",
};

/// What the tiles keep between frames besides the window.
#[derive(Default)]
pub struct Local {
    pub search: String,
    /// Find (Mod+F) asked for the search box.
    pub focus_search: bool,
    pub doc_search: String,
    /// A row's menu asked for this tab.
    pub open_tab: Option<&'static str>,
}

pub struct Behavior<'a> {
    pub w: &'a mut Window,
    pub local: &'a mut Local,
    pub now: Instant,
    /// Text to copy, when a Copy was chosen.
    pub copy: &'a mut Option<String>,
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _: TileId, pane: &mut Pane) -> egui_tiles::UiResponse {
        self.w.shows(pane);
        egui::Frame::NONE.inner_margin(4.0).show(ui, |ui| {
            if self.w.lost {
                ui.disable();
            }
            match *pane {
                "navigator" => navigator(ui, self.w),
                "items" => items(ui, self.w, self.local, self.now, self.copy),
                "item" => item(ui, self.w, self.copy),
                "bom" => bom(ui, self.w, self.now, self.copy),
                "where_used" => where_used(ui, self.w),
                "findings" => findings(ui, self.w),
                "changes" => changes(ui, self.w),
                "releases" => releases(ui, self.w, self.copy),
                "imports" => imports(ui, self.w, self.copy),
                "documents" => documents(ui, self.w, self.local, self.copy),
                "activity" => activity(ui, self.w),
                _ => {}
            }
        });
        egui_tiles::UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        SPEC.title(pane).unwrap_or(pane).into()
    }

    /// Every tile has its tab, as ryucad-app's and ryueda-app's: its name
    /// for screen readers and the window tests.
    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }
}

const ROW: f32 = 20.0;

/// An error code as the CLI writes it (`item_not_found`).
pub fn code_word(c: Code) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{c:?}"))
}

/// A tile's newest answer failed: why, over what it keeps.
fn failed(ui: &mut egui::Ui, e: Option<&Error>) {
    if let Some(e) = e {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("{} ({})", e.message, code_word(e.code)),
        );
    }
}

/// What fills an empty tile: the command that would, with Copy.
fn command(ui: &mut egui::Ui, copy: &mut Option<String>, says: &str, cmd: String) {
    ui.weak(says);
    ui.horizontal(|ui| {
        ui.monospace(&cmd);
        if ui
            .small_button("Copy")
            .on_hover_text("Copy the command")
            .clicked()
        {
            *copy = Some(cmd.clone());
        }
    });
}

fn marking(w: &Window, now: Instant, background: Color32) -> Marking<'_> {
    Marking {
        me: w.me(),
        mine: &w.selection,
        hover: w.hover.as_deref(),
        people: &w.people,
        flash: &w.flash,
        now,
        background,
    }
}

/// A row's mark under it: a stripe at its left edge in the mark's colour
/// (the cue colour-blind eyes and the contrast rule read) and a tint.
fn paint_mark(ui: &egui::Ui, rect: egui::Rect, m: Option<&Mark>) {
    if let Some(m) = m {
        ui.painter()
            .rect_filled(rect, 0.0, m.colour.gamma_multiply(0.18));
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(4.0, rect.height())),
            0.0,
            m.colour,
        );
    }
}

/// One clickable row of painted columns, named for accesskit (`label`):
/// its response.
fn row(
    ui: &mut egui::Ui,
    cols: &[(f32, String)],
    label: &str,
    selected: bool,
    m: Option<&Mark>,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, ROW), egui::Sense::click());
    paint_mark(ui, rect, m);
    if resp.hovered() {
        ui.painter().rect_stroke(
            rect,
            0.0,
            ui.visuals().widgets.hovered.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let colour = ui.visuals().text_color();
    let mut x = rect.left() + 8.0;
    for (w, text) in cols {
        let clip = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(*w - 6.0, ROW));
        ui.painter().with_clip_rect(clip).text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            font.clone(),
            colour,
        );
        x += w;
    }
    let label = label.to_owned();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &label));
    resp
}

fn navigator(ui: &mut egui::Ui, w: &mut Window) {
    let f = w.rows.filter.clone();
    let count = |w: &Window, k: &str| {
        w.counts
            .get(k)
            .map_or_else(|| "…".to_owned(), |n| n.to_string())
    };
    let classes: Vec<String> = w
        .info
        .value
        .as_ref()
        .map(|v| ryu_app::people::strings(&v["classes"]))
        .unwrap_or_default();
    let mut chosen = None;
    egui::ScrollArea::vertical()
        .id_salt("navigator")
        .show(ui, |ui| {
            let all =
                f.class.is_none() && f.state.is_none() && f.faults.is_none() && f.source.is_none();
            if ui
                .selectable_label(all, format!("All items ({})", count(w, "count:all")))
                .clicked()
            {
                chosen = Some(crate::core::rows::Filter {
                    text: f.text.clone(),
                    sort: f.sort.clone(),
                    ..Default::default()
                });
            }
            let mut pick =
                |ui: &mut egui::Ui,
                 on: bool,
                 text: String,
                 set: &dyn Fn(&mut crate::core::rows::Filter)| {
                    if ui.selectable_label(on, text).clicked() {
                        let mut g = f.clone();
                        set(&mut g);
                        chosen = Some(g);
                    }
                };
            ui.strong("Classes");
            for c in &classes {
                let on = f.class.as_deref() == Some(c.as_str());
                let t = format!("{c} ({})", count(w, &format!("count:class:{c}")));
                pick(ui, on, t, &|g| g.class = (!on).then(|| c.clone()));
            }
            ui.strong("States");
            for s in STATES {
                let on = f.state.as_deref() == Some(s);
                let t = format!(
                    "{} ({})",
                    s.replace('_', " "),
                    count(w, &format!("count:state:{s}"))
                );
                pick(ui, on, t, &|g| g.state = (!on).then(|| s.to_owned()));
            }
            ui.strong("Findings");
            for (k, title) in [("errors", "With errors"), ("warnings", "With warnings")] {
                let on = f.faults.as_deref() == Some(k);
                let t = format!("{title} ({})", count(w, &format!("count:faults:{k}")));
                pick(ui, on, t, &|g| g.faults = (!on).then(|| k.to_owned()));
            }
            ui.strong("Imports");
            for i in w.imports.value.clone().unwrap_or_default() {
                let on = f.source.as_deref() == Some(i.name.as_str());
                let t = format!("{} ({} items)", i.name, i.items);
                pick(ui, on, t, &|g| g.source = (!on).then(|| i.name.clone()));
            }
        });
    if let Some(g) = chosen {
        w.set_filter(g);
    }
}

const COLUMNS: [(&str, Option<&str>, f32); 7] = [
    ("Number", Some("number"), 130.0),
    ("Name", Some("name"), 170.0),
    ("Class", Some("class"), 100.0),
    ("State", None, 90.0),
    ("Unit", None, 50.0),
    ("Make or buy", None, 90.0),
    ("Findings", None, 70.0),
];

fn row_cols(r: &Row) -> Vec<(f32, String)> {
    let findings = match (r.errors, r.warnings) {
        (0, 0) => String::new(),
        (e, 0) => format!("{e} err"),
        (0, w) => format!("{w} warn"),
        (e, w) => format!("{e} err {w} warn"),
    };
    vec![
        (COLUMNS[0].2, r.number.clone()),
        (COLUMNS[1].2, r.name.clone().unwrap_or_default()),
        (COLUMNS[2].2, r.class.clone().unwrap_or_default()),
        (COLUMNS[3].2, r.state.replace('_', " ")),
        (COLUMNS[4].2, r.unit.clone()),
        (COLUMNS[5].2, r.make_buy.clone().unwrap_or_default()),
        (COLUMNS[6].2, findings),
    ]
}

/// What a row's menu asked for.
enum Act {
    Pick(String, bool),
    Open(String),
    Tab(String, &'static str),
    Expand(String, bool),
}

fn row_menu(
    resp: &egui::Response,
    id: &str,
    number: &str,
    copy: &mut Option<String>,
    acts: &mut Vec<Act>,
) {
    resp.context_menu(|ui| {
        if ui.button("Copy reference").clicked() {
            *copy = Some(format!("item/{id}"));
            ui.close();
        }
        if ui.button("Copy number").clicked() {
            *copy = Some(number.to_owned());
            ui.close();
        }
        if ui.button("Show in BOM").clicked() {
            acts.push(Act::Tab(id.to_owned(), "bom"));
            ui.close();
        }
        if ui.button("Where used").clicked() {
            acts.push(Act::Tab(id.to_owned(), "where_used"));
            ui.close();
        }
    });
}

fn apply(w: &mut Window, local: Option<&mut Local>, acts: Vec<Act>) {
    let mut tab = None;
    for a in acts {
        match a {
            Act::Pick(r, add) => w.pick(Some(r), add),
            Act::Open(id) => w.open_item(Some(id)),
            Act::Tab(id, t) => {
                w.open_item(Some(id));
                tab = Some(t);
            }
            Act::Expand(id, open) => w.expand(&id, open),
        }
    }
    if let (Some(l), Some(t)) = (local, tab) {
        l.open_tab = Some(t);
    }
}

fn items(
    ui: &mut egui::Ui,
    w: &mut Window,
    local: &mut Local,
    now: Instant,
    copy: &mut Option<String>,
) {
    let bg = ui.visuals().panel_fill;
    let r = ui.add(
        egui::TextEdit::singleline(&mut local.search)
            .hint_text("Find: a number, a previous number, a name or a description")
            .desired_width(f32::INFINITY),
    );
    if std::mem::take(&mut local.focus_search) {
        r.request_focus();
    }
    if r.changed() {
        let mut f = w.rows.filter.clone();
        let t = local.search.trim();
        f.text = (!t.is_empty()).then(|| t.to_owned());
        w.set_filter(f);
    }
    failed(
        ui,
        w.rows
            .error
            .then(|| Error::new(Code::Daemon, "the list didn't read"))
            .as_ref(),
    );
    let sort = w.rows.filter.sort.clone();
    let mut resort = None;
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        for (title, key, width) in COLUMNS {
            match key {
                Some(k) => {
                    let on = sort.as_deref() == Some(k);
                    let t = if on {
                        format!("{title} ↓")
                    } else {
                        title.to_owned()
                    };
                    if ui
                        .add_sized(
                            [width - 6.0, ROW],
                            egui::Button::new(t).selected(on).frame(false),
                        )
                        .on_hover_text(format!("Sort by {}", title.to_lowercase()))
                        .clicked()
                    {
                        resort = Some((!on).then(|| k.to_owned()));
                    }
                }
                None => {
                    ui.add_sized(
                        [width - 6.0, ROW],
                        egui::Label::new(egui::RichText::new(title).strong()),
                    );
                }
            }
        }
    });
    if let Some(s) = resort {
        let mut f = w.rows.filter.clone();
        f.sort = s;
        w.set_filter(f);
    }
    let total = w.rows.total.unwrap_or(0);
    if w.rows.total == Some(0) && w.rows.filter == Default::default() {
        command(
            ui,
            copy,
            "No items yet. An agent or the CLI makes them:",
            "ryuplm item create <NUMBER> --name <NAME>".into(),
        );
        return;
    }
    let mut acts = Vec::new();
    let mut hover = None;
    egui::ScrollArea::vertical()
        .id_salt("items")
        .auto_shrink([false, false])
        .show_rows(ui, ROW, total, |ui, range| {
            w.rows.view(range.start, range.len());
            for i in range {
                let Some(r) = w.rows.get(i).cloned() else {
                    ui.add_sized([ui.available_width(), ROW], egui::Label::new("…"));
                    continue;
                };
                let m = mark_item(&r.id, &marking(w, now, bg));
                let label = format!("{} {}", r.number, r.name.clone().unwrap_or_default());
                let selected = w.item.as_deref() == Some(r.id.as_str());
                let resp = row(ui, &row_cols(&r), label.trim(), selected, m.as_ref());
                if resp.clicked() {
                    let add = ui.input(|i| i.modifiers.command);
                    acts.push(Act::Pick(r.reference(), add));
                    acts.push(Act::Open(r.id.clone()));
                }
                if resp.hovered() {
                    hover = Some(r.reference());
                }
                row_menu(&resp, &r.id, &r.number, copy, &mut acts);
            }
        });
    w.hover = hover;
    apply(w, Some(local), acts);
}

fn item(ui: &mut egui::Ui, w: &mut Window, copy: &mut Option<String>) {
    failed(ui, w.card.error.as_ref());
    let Some(c) = w.card.value.clone() else {
        ui.weak(if w.item.is_none() {
            "Choose an item in Items, or an agent shows you one."
        } else {
            "Reading…"
        });
        return;
    };
    ui.heading(
        format!("{} {}", c.number, c.name.clone().unwrap_or_default())
            .trim()
            .to_owned(),
    );
    ui.horizontal(|ui| {
        let state = match &c.label {
            Some(l) => format!("{} ({l})", c.state.replace('_', " ")),
            None => c.state.replace('_', " "),
        };
        ui.label(state);
        if ui.small_button("Copy reference").clicked() {
            *copy = Some(c.reference());
        }
        if ui.small_button("Copy number").clicked() {
            *copy = Some(c.number.clone());
        }
    });
    let mut open = None;
    egui::ScrollArea::vertical().id_salt("item").show(ui, |ui| {
        egui::Grid::new("fields").num_columns(2).show(ui, |ui| {
            for (k, v) in [
                ("Number", Some(c.number.clone())),
                ("Name", c.name.clone()),
                ("Description", c.description.clone()),
                ("Class", c.class.clone()),
                ("Unit", Some(c.unit.clone())),
                ("Make or buy", c.make_buy.clone()),
                ("ID", Some(c.id.clone())),
            ] {
                ui.label(k);
                ui.label(v.unwrap_or_default());
                ui.end_row();
            }
        });
        if !c.properties.is_empty() {
            ui.strong("Properties");
            egui::Grid::new("properties").num_columns(3).show(ui, |ui| {
                for p in &c.properties {
                    ui.label(&p.name);
                    ui.label(&p.value);
                    ui.weak(p.kind.as_deref().unwrap_or("undeclared"));
                    ui.end_row();
                }
            });
        }
        ui.strong("AML");
        if c.aml.is_empty() {
            ui.weak("No manufacturers");
        }
        for a in &c.aml {
            let star = if a.preferred { "preferred · " } else { "" };
            ui.label(format!("{} {} ({star}{})", a.manufacturer, a.mpn, a.status));
        }
        ui.strong("Representations");
        if c.links.is_empty() {
            ui.weak("Not linked to a ryucad part or a ryueda design");
        }
        for (k, l) in c.links.iter().enumerate() {
            ui.push_id(k, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{} {} in {}", l.product, l.part, l.project));
                    match &l.folder {
                        Some(f) => ui.weak(f),
                        None => ui.colored_label(ui.visuals().warn_fg_color, "not found"),
                    };
                    let can = l.folder.is_some() && launch::installed(&l.product);
                    let b = ui.add_enabled(can, egui::Button::new("Open").small());
                    let b = if can {
                        b.on_hover_text(format!("{} open", l.product))
                    } else {
                        b.on_disabled_hover_text(format!(
                            "{} isn't installed, or the project wasn't found",
                            l.product
                        ))
                    };
                    if b.clicked() {
                        open = Some((l.product.clone(), l.folder.clone().unwrap_or_default()));
                    }
                });
            });
        }
        if !c.previous.is_empty() {
            ui.strong("Previous numbers");
            ui.label(c.previous.join(", "));
        }
        if !c.provenance.is_empty() {
            ui.strong("Provenance");
            for (import, key) in &c.provenance {
                ui.label(format!("{import}, key {key}"));
            }
        }
        ui.strong("Documents");
        ui.label(match c.documents.len() {
            0 => "None".to_owned(),
            1 => "1 document (the Documents tab)".to_owned(),
            n => format!("{n} documents (the Documents tab)"),
        });
        if !c.faults.is_empty() {
            ui.strong("Findings");
            for f in &c.faults {
                ui.label(format!("{} {}: {}", f.severity, f.code, f.message));
            }
        }
    });
    if let Some((product, folder)) = open
        && let Err(e) = launch::representation(&product, &folder)
    {
        w.toasts.error("Open", &e, 0, Instant::now());
    }
}

fn line_cols(l: &BomLine, depth: usize) -> Vec<(f32, String)> {
    vec![
        (230.0, format!("{}{}", "   ".repeat(depth), l.title())),
        (60.0, l.quantity.clone()),
        (50.0, l.unit.clone().unwrap_or_default()),
        (50.0, l.find.clone().unwrap_or_default()),
        (120.0, l.refdes.clone().unwrap_or_default()),
        (70.0, l.from.clone()),
        (140.0, l.problem.clone().unwrap_or_default()),
    ]
}

#[allow(clippy::too_many_arguments)]
fn line_row(
    ui: &mut egui::Ui,
    w: &Window,
    parent: &str,
    l: &BomLine,
    depth: usize,
    now: Instant,
    copy: &mut Option<String>,
    acts: &mut Vec<Act>,
) {
    let bg = ui.visuals().panel_fill;
    let reference = l.reference(parent);
    let m = reference
        .as_deref()
        .and_then(|r| mark(r, &marking(w, now, bg)))
        .or_else(|| {
            l.child
                .as_deref()
                .and_then(|c| mark(c, &marking(w, now, bg)))
        });
    let child = l
        .child
        .as_ref()
        .and_then(|c| c.strip_prefix("item/"))
        .map(str::to_owned);
    ui.horizontal(|ui| {
        match &child {
            Some(id) => {
                let open = w.children.contains_key(id);
                let b = ui
                    .small_button(if open { "-" } else { "+" })
                    .on_hover_text("Its own lines");
                b.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        true,
                        format!("Lines of {}", l.title()),
                    )
                });
                if b.clicked() {
                    acts.push(Act::Expand(id.clone(), !open));
                }
            }
            None => {
                ui.add_space(22.0);
            }
        }
        let selected = reference.as_ref().is_some_and(|r| w.selection.contains(r));
        let resp = row(ui, &line_cols(l, depth), &l.title(), selected, m.as_ref());
        if resp.clicked()
            && let Some(r) = &reference
        {
            acts.push(Act::Pick(r.clone(), ui.input(|i| i.modifiers.command)));
        }
        if resp.double_clicked()
            && let Some(id) = &child
        {
            acts.push(Act::Open(id.clone()));
        }
        if let Some(id) = &child {
            row_menu(&resp, id, l.number.as_deref().unwrap_or(""), copy, acts);
        }
    });
    if let Some(id) = &child
        && let Some(g) = w.children.get(id)
    {
        if let Some(e) = &g.error {
            ui.colored_label(ui.visuals().error_fg_color, &e.message);
        }
        for c in g.value.iter().flatten() {
            line_row(ui, w, id, c, depth + 1, now, copy, acts);
        }
    }
}

fn bom(ui: &mut egui::Ui, w: &mut Window, now: Instant, copy: &mut Option<String>) {
    let Some(parent) = w.item.clone() else {
        ui.weak("Open an item to see its BOM.");
        return;
    };
    failed(ui, w.bom.error.as_ref());
    let lines = w.bom.value.clone().unwrap_or_default();
    if lines.is_empty() {
        if w.bom.value.is_some() {
            let number = w
                .card
                .value
                .as_ref()
                .map_or_else(|| parent.clone(), |c| c.number.clone());
            command(
                ui,
                copy,
                "No lines. An agent or the CLI adds them:",
                format!("ryuplm bom add {number} <CHILD> <QUANTITY>"),
            );
        }
        return;
    }
    let mut acts = Vec::new();
    egui::ScrollArea::vertical().id_salt("bom").show(ui, |ui| {
        for (k, l) in lines.iter().enumerate() {
            ui.push_id(k, |ui| line_row(ui, w, &parent, l, 0, now, copy, &mut acts));
        }
    });
    apply(w, None, acts);
}

/// A Show button, always drawn so a list's buttons line up; disabled
/// when the row names nothing here.
fn show_button(ui: &mut egui::Ui, w: &mut Window, reference: Option<&str>, about: &str) {
    let b = ui.add_enabled(reference.is_some(), egui::Button::new("Show").small());
    let b = match reference {
        Some(_) => b.on_hover_text(about),
        None => b.on_disabled_hover_text("nothing to show here"),
    };
    if b.clicked()
        && let Some(r) = reference
    {
        w.go_to(r);
    }
}

fn where_used(ui: &mut egui::Ui, w: &mut Window) {
    if w.item.is_none() {
        ui.weak("Open an item to see where it is used.");
        return;
    }
    failed(ui, w.used.error.as_ref());
    let rows = w.used.value.clone().unwrap_or_default();
    if rows.is_empty() && w.used.value.is_some() {
        ui.weak("Nothing uses it, here or in the catalogues searched.");
    }
    let own = w
        .info
        .value
        .as_ref()
        .and_then(|v| v["project"].as_str().map(str::to_owned));
    egui::ScrollArea::vertical()
        .id_salt("where_used")
        .show(ui, |ui| {
            for (k, u) in rows.iter().enumerate() {
                ui.push_id(k, |ui| {
                    ui.horizontal(|ui| {
                        let here = own.as_deref() == Some(u.catalogue.as_str());
                        show_button(ui, w, here.then(|| u.reference()).as_deref(), &u.number);
                        ui.strong(&u.number);
                        ui.label(format!("via {} × {}", u.via, u.quantity));
                        ui.weak(
                            u.release
                                .as_deref()
                                .map_or_else(|| "working".to_owned(), |l| format!("released {l}")),
                        );
                        if !here {
                            ui.weak(u.folder.as_deref().unwrap_or(&u.catalogue));
                        }
                    });
                });
            }
        });
}

fn findings(ui: &mut egui::Ui, w: &mut Window) {
    let b = ui.add_enabled(
        !w.checking,
        egui::Button::new(if w.checking {
            "Checking…"
        } else {
            "Run check"
        }),
    );
    if b.clicked() {
        w.run_check();
    }
    failed(ui, w.findings.error.as_ref());
    let all = w.findings.value.clone().unwrap_or_default();
    if all.is_empty() && w.findings.value.is_some() {
        ui.weak("No findings.");
    }
    egui::ScrollArea::vertical()
        .id_salt("findings")
        .show(ui, |ui| {
            for (k, f) in all.iter().enumerate() {
                ui.push_id(k, |ui| {
                    ui.horizontal(|ui| {
                        show_button(ui, w, f.reference().as_deref(), &f.code);
                        ui.strong(format!("{} {}", f.severity, f.code));
                        if f.new {
                            ui.weak("new");
                        }
                        ui.label(&f.message);
                    });
                });
            }
        });
}

fn changes(ui: &mut egui::Ui, w: &mut Window) {
    if let Some(e) = &w.changes.error {
        if e.code == Code::NotAGitRepo {
            ui.weak("Changes are Git's: this catalogue isn't in a repository.");
            return;
        }
        failed(ui, Some(e));
    }
    let all = w.changes.value.clone().unwrap_or_default();
    if all.is_empty() && w.changes.value.is_some() {
        ui.weak("No changes since HEAD.");
    }
    egui::ScrollArea::vertical()
        .id_salt("changes")
        .show(ui, |ui| {
            for (k, c) in all.iter().enumerate() {
                ui.push_id(k, |ui| {
                    ui.horizontal(|ui| {
                        let r = (c.verb != "removed").then(|| c.reference());
                        show_button(ui, w, r.as_deref(), &c.number);
                        ui.strong(format!("{} {}", c.verb, c.number));
                        if let Some(b) = &c.bom {
                            ui.weak(b);
                        }
                    });
                    for l in c.lines.iter().take(8) {
                        ui.monospace(l);
                    }
                    if c.lines.len() > 8 {
                        ui.weak(format!("…and {} more", c.lines.len() - 8));
                    }
                });
            }
        });
}

fn releases(ui: &mut egui::Ui, w: &mut Window, copy: &mut Option<String>) {
    let mut all = w.releases_all;
    if ui.checkbox(&mut all, "Every item").changed() {
        w.set_releases_all(all);
    }
    failed(ui, w.releases.error.as_ref());
    let rows = w.releases.value.clone().unwrap_or_default();
    if rows.is_empty() && w.releases.value.is_some() {
        let number = w.card.value.as_ref().map(|c| c.number.clone());
        match number.filter(|_| !all) {
            Some(n) => command(
                ui,
                copy,
                "Never released. An agent or the CLI proposes it:",
                format!("ryuplm release propose {n}"),
            ),
            None => {
                ui.weak("No releases.");
            }
        }
    }
    egui::ScrollArea::vertical()
        .id_salt("releases")
        .show(ui, |ui| {
            for (k, r) in rows.iter().enumerate() {
                ui.push_id(k, |ui| {
                    ui.horizontal(|ui| {
                        let target = (r.kind == "item").then(|| format!("item/{}", r.part));
                        show_button(ui, w, target.as_deref(), &r.label);
                        ui.weak(&r.part);
                        ui.label(r.text());
                    });
                });
            }
        });
}

fn imports(ui: &mut egui::Ui, w: &mut Window, copy: &mut Option<String>) {
    failed(ui, w.imports.error.as_ref());
    let rows = w.imports.value.clone().unwrap_or_default();
    if rows.is_empty() && w.imports.value.is_some() {
        command(
            ui,
            copy,
            "No mappings. An agent or the CLI proposes one from a file:",
            "ryuplm import guess <FILE> --save <NAME>".into(),
        );
    }
    let mut listed = None;
    for (k, i) in rows.iter().enumerate() {
        ui.push_id(k, |ui| {
            ui.horizontal(|ui| {
                show_button(ui, w, Some(&i.reference()), &i.name);
                ui.strong(&i.name);
                ui.label(i.text());
                if ui.small_button("List its items").clicked() {
                    listed = Some(i.name.clone());
                }
            });
        });
    }
    if let Some(n) = listed {
        let mut f = w.rows.filter.clone();
        f.source = Some(n);
        w.set_filter(f);
    }
}

fn activity(ui: &mut egui::Ui, w: &mut Window) {
    let list = w.activity.value.clone().unwrap_or_default();
    let undo = list.iter().find(|e| e.undo).map(|e| e.text.clone());
    let redo = list.iter().find(|e| e.redo).map(|e| e.text.clone());
    ui.horizontal(|ui| {
        let t = undo
            .as_ref()
            .map_or_else(|| "Undo".to_owned(), |u| format!("Undo {u}"));
        if ui
            .add_enabled(undo.is_some(), egui::Button::new(t))
            .clicked()
        {
            w.undo();
        }
        let t = redo
            .as_ref()
            .map_or_else(|| "Redo".to_owned(), |r| format!("Redo {r}"));
        if ui
            .add_enabled(redo.is_some(), egui::Button::new(t))
            .clicked()
        {
            w.redo();
        }
    });
    failed(ui, w.activity.error.as_ref());
    egui::ScrollArea::vertical()
        .id_salt("activity")
        .show(ui, |ui| {
            for e in &list {
                ui.label(format!("{}  {}", e.seq, e.text));
            }
        });
}

fn doc_row(
    ui: &mut egui::Ui,
    w: &mut Window,
    c: &crate::core::documents::Card,
    copy: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        let busy = w.opening();
        let b = ui
            .add_enabled(!busy, egui::Button::new("Open").small())
            .on_hover_text(format!(
                "{} with this computer's program for {}",
                c.file_name, c.media_type
            ));
        b.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, !busy, format!("Open {}", c.number))
        });
        if b.clicked() {
            w.open_document(&c.id);
        }
        ui.strong(&c.number);
        ui.label(&c.title);
        ui.weak(format!(
            "{} · {} · {}",
            c.doc_type,
            crate::core::documents::size(c.size),
            c.storage
        ));
        if ui.small_button("Copy number").clicked() {
            *copy = Some(c.number.clone());
        }
    });
    for f in &c.faults {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("{} {}: {}", f.severity, f.code, f.message),
        );
    }
}

fn documents(ui: &mut egui::Ui, w: &mut Window, local: &mut Local, copy: &mut Option<String>) {
    let mut all = w.docs_all || w.item.is_none();
    ui.horizontal(|ui| {
        if ui.checkbox(&mut all, "Every document").changed() || (all && !w.docs_all) {
            w.set_docs(all, &local.doc_search);
        }
        if all {
            let r = ui.add(
                egui::TextEdit::singleline(&mut local.doc_search)
                    .hint_text("A number, a title or a description"),
            );
            if r.changed() {
                w.set_docs(true, &local.doc_search);
            }
        }
    });
    if all {
        failed(ui, w.documents.error.as_ref());
        let rows = w.documents.value.clone().unwrap_or_default();
        if rows.is_empty() && w.documents.value.is_some() {
            command(
                ui,
                copy,
                "No documents. An agent or the CLI adds them:",
                "ryuplm document add <FILE> --title <TITLE>".into(),
            );
        }
        egui::ScrollArea::vertical()
            .id_salt("documents")
            .show(ui, |ui| {
                for (k, d) in rows.iter().enumerate() {
                    ui.push_id(k, |ui| {
                        let open = w.doc_cards.contains_key(&d.id);
                        if ui.selectable_label(open, d.text()).clicked() {
                            w.doc_card(&d.id);
                        }
                        if let Some(c) = w.doc_cards.get(&d.id).and_then(|g| g.value.clone()) {
                            doc_row(ui, w, &c, copy);
                            for (id, number) in &c.items {
                                ui.horizontal(|ui| {
                                    show_button(ui, w, Some(&format!("item/{id}")), number);
                                    ui.label(number);
                                });
                            }
                        }
                    });
                }
            });
        return;
    }
    let Some(card) = w.card.value.clone() else {
        ui.weak("Reading…");
        return;
    };
    if card.documents.is_empty() {
        command(
            ui,
            copy,
            "No documents. An agent or the CLI attaches one:",
            format!("ryuplm document attach <DOCUMENT> {}", card.number),
        );
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("documents")
        .show(ui, |ui| {
            for (k, r) in card.documents.iter().enumerate() {
                ui.push_id(k, |ui| {
                    if r.contains(':') {
                        ui.weak(format!("{r} (another catalogue's)"));
                        return;
                    }
                    match w
                        .doc_cards
                        .get(r)
                        .map(|g| (g.value.clone(), g.error.clone()))
                    {
                        Some((Some(c), _)) => doc_row(ui, w, &c, copy),
                        Some((None, Some(e))) => {
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                format!("{r}: {}", e.message),
                            );
                        }
                        _ => {
                            ui.weak(format!("{r}…"));
                        }
                    }
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_layout_has_every_tile_once_round_its_items() {
        let d = (SPEC.default)();
        let mut tiles = d.tiles();
        assert_eq!(tiles[0], "navigator");
        tiles.sort();
        let mut all: Vec<&str> = crate::core::tiles::TILES
            .iter()
            .map(|(id, _)| *id)
            .collect();
        all.sort();
        assert_eq!(tiles, all);
        assert_eq!(SPEC.read(&d.to_value()).root, d);
        assert_eq!(SPEC.centre, "items");
        assert_eq!((SPEC.home)("findings"), Home::Bottom);
        assert_eq!((SPEC.home)("item"), Home::Right);
        for t in BOTTOM {
            assert!(ryuplm::view::TABS.contains(&t), "{t} is a view's tab");
        }
    }
}
