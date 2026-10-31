//! The Tree tile (design 199, Tree): a search field and filters over
//! `core::tree`'s rows, each row an icon, today's text and its state's
//! badges, kept to 20 points; the keyboard moves through it while it has
//! focus.

use crate::core::model::BodyKey;
use crate::core::tree::{Act, Key, Row, Shown, Source, Tint};
pub use crate::core::tree::{assembly_text, mark_text, part_text, row_of, row_text};
use crate::core::workspace::Workspace;
use crate::ui::icon::{self, Icon};
use crate::ui::style::{self, Palette};
use crate::ui::view::View;
use eframe::egui::{self, Color32, Sense, text::LayoutJob};

/// A row's height at `ui.scale` 1.0.
pub const ROW: f32 = 20.0;
/// An icon's size in a row.
const ICON: f32 = 16.0;
/// How far each level is indented.
const INDENT: f32 = 16.0;

/// The colour a tint is in this look.
fn tint(p: &Palette, t: Tint) -> Color32 {
    style::rgb(match t {
        Tint::Plain => p.text,
        Tint::Accent => p.accent,
        Tint::Weak => p.text_weak,
        Tint::Danger => p.danger,
        Tint::Warn => p.warning,
        Tint::Info => p.info,
        Tint::Ok => p.ok,
    })
}

/// An icon by its name, else a question mark.
fn icon_named(name: &str) -> Icon {
    crate::ui::toolbar::icon_of(name).unwrap_or(Icon::QUESTION)
}

/// The tree's keyboard focus: one widget over the rows, so arrows reach
/// it rather than moving egui's focus between rows.
fn focus_id(ui: &egui::Ui) -> egui::Id {
    ui.scope_id().with("tree focus")
}

/// Whether a stable name of `body` starts with the hex query `q`.
fn names(ws: &Workspace, body: &BodyKey, q: &str) -> bool {
    let Some(m) = ws.model.meshes.get(body).and_then(|m| m.mesh.as_ref().ok()) else {
        return false;
    };
    let hex = ryucad::mesh::name_hex;
    m.faces.iter().any(|f| hex(&f.name).starts_with(q))
        || m.edges.iter().any(|(n, _)| hex(n).starts_with(q))
        || m.vertices.iter().any(|(n, _)| hex(n).starts_with(q))
}

/// The search field, its count and the filters.
fn search(ui: &mut egui::Ui, view: &mut View, shown: usize) {
    let tree = &mut view.tree;
    let keys = view
        .commands
        .keys
        .get("tree.find")
        .cloned()
        .unwrap_or_default();
    ui.horizontal(|ui| {
        icon::show(
            ui,
            Icon::MAGNIFYING_GLASS,
            14.0,
            ui.visuals().weak_text_color(),
        );
        let searching = !tree.query.is_empty();
        let room = if searching { 90.0 } else { 70.0 };
        let edit = egui::TextEdit::singleline(&mut tree.query)
            .hint_text(format!("Find in tree  {keys}"))
            .desired_width((ui.available_width() - room).max(60.0));
        let r = ui.add(edit);
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::SearchInput, true, "Find in tree"));
        if std::mem::take(&mut view.tree_find) {
            r.request_focus();
        }
        // Down from the field goes into the rows.
        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            view.tree_take_focus = true;
        }
        if searching {
            ui.weak(format!("{shown} rows"));
            let clear = icon::button(
                ui,
                Icon::X,
                12.0,
                "Clear search",
                "Clear search",
                false,
                Ok(()),
            );
            if clear.clicked() {
                tree.query.clear();
            }
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (label, on, tip) in [
            (
                "Failing",
                &mut tree.failing,
                "Only what failed, and what holds it",
            ),
            ("Changed", &mut tree.changed, "Only what a compare marks"),
            ("Hidden", &mut tree.hidden, "Only what is hidden"),
        ] {
            let b = egui::Button::selectable(*on, egui::RichText::new(label).small());
            if ui.add(b).on_hover_text(tip).clicked() {
                *on = !*on;
            }
        }
    });
}

/// A row's text as drawn: its id in the text colour, the rest weak, the
/// query's match marked.
fn text_job(
    ui: &egui::Ui,
    row: &Row,
    mark: Option<(usize, usize)>,
    colour: Color32,
    weak: Color32,
) -> LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let split = row
        .text
        .find(" (")
        .or_else(|| row.text.find(": "))
        .unwrap_or(row.text.len());
    let mut job = LayoutJob::default();
    let marked = ui.visuals().selection.bg_fill.gamma_multiply(0.6);
    // Pieces at the id's end and the match's ends.
    let mut cuts = vec![0, split, row.text.len()];
    if let Some((a, b)) = mark {
        cuts.extend([a, b]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b || !row.text.is_char_boundary(a) || !row.text.is_char_boundary(b) {
            continue;
        }
        let in_mark = mark.is_some_and(|(x, y)| a >= x && b <= y);
        let fmt = egui::TextFormat {
            font_id: font.clone(),
            color: if a < split { colour } else { weak },
            background: if in_mark {
                marked
            } else {
                Color32::TRANSPARENT
            },
            ..Default::default()
        };
        job.append(&row.text[a..b], 0.0, fmt);
    }
    job
}

/// What happened on one row this frame.
#[derive(Default)]
struct Did {
    clicked: bool,
    caret: bool,
    act: Option<Act>,
    run: Option<(&'static str, String)>,
    /// The keyboard's menu on this row: drawn (`Some`), open or not.
    keyed: Option<bool>,
}

/// Draws one row.
#[allow(clippy::too_many_arguments)]
fn row_ui(
    ui: &mut egui::Ui,
    ws: &Workspace,
    view: &View,
    row: &Row,
    shown: &Shown,
    open: bool,
    narrowing: bool,
    focused: bool,
) -> Did {
    let mut did = Did::default();
    let p = view.look.palette(ui.visuals().dark_mode);
    let chosen = row.choose.is_some() && view.chosen == row.choose;
    let cursor = focused && view.tree.cursor.as_deref() == Some(row.id.as_str());
    let width = ui.available_width().max(120.0);
    let (rect, r) = ui.allocate_exact_size(egui::vec2(width, ROW), Sense::click());
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, chosen, &row.text));
    let visible = ui.is_rect_visible(rect);
    let painter = ui.painter_at(rect.expand2(egui::vec2(0.0, 1.0)));
    if visible {
        if chosen {
            painter.rect_filled(rect, 0.0, style::rgb(p.select_bg));
        } else if r.hovered() {
            painter.rect_filled(rect, 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        if cursor {
            painter.rect_stroke(
                rect.shrink(0.5),
                0.0,
                egui::Stroke::new(1.0, style::rgb(p.focus)),
                egui::StrokeKind::Inside,
            );
        }
    }
    let mut x = rect.min.x + 4.0 + row.depth as f32 * INDENT;
    let mid = rect.center().y;
    // The caret: open or close what is under the row.
    if row.children && !narrowing {
        let c = egui::Rect::from_center_size(egui::pos2(x + 6.0, mid), egui::vec2(12.0, ROW));
        let cr = ui.interact(c, ui.scope_id().with(("caret", &row.id)), Sense::click());
        let name = format!("{} {}", if open { "Collapse" } else { "Expand" }, row.text);
        cr.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        if visible {
            let i = if open {
                Icon::CARET_DOWN
            } else {
                Icon::CARET_RIGHT
            };
            icon::paint(
                ui.painter(),
                egui::Rect::from_center_size(c.center(), egui::vec2(11.0, 11.0)),
                i,
                style::rgb(p.text_weak),
            );
        }
        did.caret = cr.clicked();
    }
    x += 14.0;
    // An instance's or a copy's Show box.
    if row.hideable
        && let Some(reference) = &row.reference
    {
        let b =
            egui::Rect::from_min_size(egui::pos2(x, rect.min.y + 1.0), egui::vec2(18.0, ROW - 2.0));
        let mut shown_now = !row.hidden;
        let label = format!("Show {reference}");
        let cb = ui.put(b, egui::Checkbox::without_text(&mut shown_now));
        cb.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::CheckBox, true, shown_now, &label)
        });
        let cb = cb.on_hover_text(if shown_now {
            "Shown: click to hide (Tab in the view, Space in the tree)"
        } else {
            "Hidden: click to show (Shift+Tab in the view, Space in the tree)"
        });
        if cb.changed() {
            did.act = Some(if shown_now {
                Act::Show(reference.clone())
            } else {
                Act::Hide(reference.clone())
            });
        }
        x += 20.0;
    }
    let colour = if chosen {
        style::rgb(p.select_fg)
    } else if let Some(c) = row.colour {
        style::color32(c)
    } else if row.failed && row.tint == Tint::Danger {
        style::rgb(p.danger)
    } else if row.weak {
        style::rgb(p.text_weak)
    } else {
        style::rgb(p.text)
    };
    let weak = if chosen || row.colour.is_some() || row.failed {
        colour
    } else {
        style::rgb(p.text_weak)
    };
    let icon_rect =
        egui::Rect::from_center_size(egui::pos2(x + ICON / 2.0, mid), egui::vec2(ICON, ICON));
    if visible {
        let t = if row.weak && row.tint == Tint::Plain {
            tint(&p, Tint::Weak)
        } else {
            tint(&p, row.tint)
        };
        icon::paint(ui.painter(), icon_rect, icon_named(row.icon), t);
    }
    x += ICON + 5.0;
    // A solid's row stands out, as a strong label does.
    let colour = if row.strong && !chosen && !row.failed && !row.weak {
        ui.visuals().strong_text_color()
    } else {
        colour
    };
    let job = text_job(ui, row, shown.mark, colour, weak);
    let galley = ui.painter().layout_job(job);
    let text_w = galley.size().x;
    if visible {
        ui.painter()
            .galley(egui::pos2(x, mid - galley.size().y / 2.0), galley, colour);
    }
    x += text_w + 5.0;
    for (k, b) in row.badges.iter().enumerate() {
        let br = egui::Rect::from_center_size(egui::pos2(x + 7.0, mid), egui::vec2(14.0, 14.0));
        if visible {
            icon::paint(ui.painter(), br, icon_named(b.icon), tint(&p, b.tint));
        }
        ui.interact(
            br,
            ui.scope_id().with(("badge", &row.id, k)),
            Sense::hover(),
        )
        .on_hover_text(&b.tip);
        x += 17.0;
    }
    let r = match &row.tip {
        Some(t) => r.on_hover_text(t),
        None => r,
    };
    did.clicked = r.clicked();
    // The registry's commands for the row, and a solid's Hide or Show.
    // A menu the keyboard opened stays that popup until it closes (a
    // context menu is another popup, so it would never show).
    let keyed = view.tree_menu.as_deref() == Some(row.id.as_str());
    if row.menu.is_some() || row.solid {
        let popup = if keyed {
            let opening = view.tree_menu_opening;
            egui::Popup::menu(&r).open_memory(opening.then_some(egui::SetOpenCommand::Bool(true)))
        } else {
            egui::Popup::context_menu(&r)
        };
        let shown = popup.show(|ui| {
            let reference = row.reference.clone().unwrap_or_default();
            if row.solid {
                let (label, act) = if row.hidden {
                    ("Show", Act::Show(reference.clone()))
                } else {
                    ("Hide", Act::Hide(reference.clone()))
                };
                if ui.button(label).clicked() {
                    did.act = Some(act);
                    ui.close();
                }
            }
            if let Some(kind) = row.menu {
                use crate::core::command::Context;
                if let Some((id, _)) = crate::ui::commands::context_menu(
                    ui,
                    &view.commands,
                    Context::Row(kind),
                    Some(&reference),
                ) {
                    did.run = Some((id, reference.clone()));
                }
            }
        });
        if keyed {
            did.keyed = Some(shown.is_some());
        }
    }
    let _ = ws;
    did
}

/// Chooses a tree row's feature, as a click does: its form opens, and a
/// sketch frames itself, an instance everything it places, anything else
/// its solid. Choosing the chosen row with Ctrl/Cmd unchooses it.
pub(crate) fn choose(
    ws: &mut Workspace,
    view: &mut View,
    key: Option<BodyKey>,
    gone: bool,
    frame: bool,
) {
    view.chosen = key;
    view.marked.clear();
    if gone {
        ws.open_form(None);
        ws.choose(None);
        return;
    }
    ws.open_form(view.chosen.clone());
    ws.choose(view.chosen.clone());
    if frame && let Some(k) = view.chosen.clone() {
        let bodies = ws.model.framed(&k);
        view.fit_all(ws, &bodies);
    }
}

/// The keys the tree takes this frame, while it has the keyboard.
fn keys(ui: &egui::Ui) -> Vec<Key> {
    // By each press's own modifiers, as the window's keymap reads them.
    ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => match key {
                    egui::Key::F10 if modifiers.shift_only() => Some(Key::Menu),
                    _ if modifiers.any() => None,
                    egui::Key::ArrowUp => Some(Key::Up),
                    egui::Key::ArrowDown => Some(Key::Down),
                    egui::Key::ArrowLeft => Some(Key::Left),
                    egui::Key::ArrowRight => Some(Key::Right),
                    egui::Key::Enter => Some(Key::Enter),
                    egui::Key::Space => Some(Key::Space),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    })
}

/// Parts with their solids and sketches in order, an assembly's
/// instances, mates and copies, under a search and filters. Choosing a
/// feature paints what it made and frames its solid, or paints its
/// sketch. In a compare, features are marked by how they differ, and
/// those only the other side has follow, to be chosen for their changes.
pub fn tree(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    let pdm = crate::core::palette::pdm_names(&ws.bom);
    let rows = {
        let src = Source {
            model: &ws.model,
            compare: ws.compare.as_ref(),
            states: &ws.states,
            doctor: ws.doctor.as_ref(),
            merge: ws.merge.as_ref().map(|m| &m.status),
            pdm: &pdm,
            whole: ws.model.listed.len() <= crate::core::workspace::LOAD_WHOLE,
        };
        crate::core::tree::rows(&src)
    };
    let shown = view.tree.shown(&rows, &|b, q| names(ws, b, q));
    search(ui, view, shown.len());
    ui.separator();
    if ws.model.parts.is_empty() && ws.model.listed.is_empty() {
        ui.weak("No parts yet");
    } else if shown.is_empty() {
        ui.weak("Nothing matches");
    }
    let narrowing = view.tree.narrowing();
    let id = focus_id(ui);
    let mut acts: Vec<Act> = Vec::new();
    let mut run = None;
    let mut toggled: Vec<(String, bool)> = Vec::new();
    let scroll = egui::ScrollArea::both()
        .id_salt("tree rows")
        .auto_shrink(false)
        .show(ui, |ui| {
            // Names are IDs: never broken across lines.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.spacing_mut().item_spacing.y = 0.0;
            let focused = ui.memory(|m| m.has_focus(id));
            for s in &shown {
                let row = &rows[s.index];
                let open = narrowing || view.tree.is_open(row);
                let did = row_ui(ui, ws, view, row, s, open, narrowing, focused);
                if did.caret || (did.clicked && row.choose.is_none() && row.children && !narrowing)
                {
                    toggled.push((row.id.clone(), !view.tree.is_open(row)));
                }
                if did.clicked || did.caret {
                    view.tree.cursor = Some(row.id.clone());
                    view.tree_take_focus = true;
                }
                if did.clicked
                    && let Some(k) = &row.choose
                {
                    let chosen = view.chosen.as_ref() == Some(k);
                    // The release's own modifiers, as egui's clicks are
                    // judged at it.
                    let command = ui.input(|i| {
                        i.modifiers.command || i.events.iter().any(|e| {
                            matches!(e, egui::Event::PointerButton { pressed: false, modifiers, .. }
                                    if modifiers.command)
                        })
                    });
                    match (chosen, command) {
                        (true, true) => acts.push(Act::Choose {
                            key: (String::new(), String::new()),
                            gone: false,
                        }),
                        (true, false) => {}
                        (false, _) => acts.push(Act::Choose {
                            key: k.clone(),
                            gone: row.gone,
                        }),
                    }
                }
                if let Some(a) = did.act {
                    acts.push(a);
                }
                if did.run.is_some() {
                    run = did.run;
                }
                if let Some(open) = did.keyed {
                    view.tree_menu_opening = false;
                    if !open {
                        view.tree_menu = None;
                    }
                }
                // A part or subassembly opened loads.
                if open && let Some(l) = &row.load {
                    ws.want(l);
                }
            }
        });
    // The rows hold the keyboard: arrows, Enter, Space and the menu key.
    let area = ui.interact(scroll.inner_rect, id, Sense::focusable_noninteractive());
    area.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Tree, true, "Tree rows"));
    view.tree_focus = Some(id);
    if std::mem::take(&mut view.tree_take_focus) {
        area.request_focus();
    }
    if area.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    tab: false,
                    escape: false,
                },
            );
        });
        for k in keys(ui) {
            match view.tree.key(&rows, &shown, k) {
                Some(Act::Menu(_)) => {
                    view.tree_menu = view.tree.cursor.clone();
                    view.tree_menu_opening = true;
                }
                Some(a) => acts.push(a),
                None => {}
            }
        }
    }
    for (row, open) in toggled {
        view.tree.set_open(&row, open);
    }
    ws.load_wanted();
    for a in acts {
        match a {
            Act::Choose { key, gone } => {
                let key = (!key.0.is_empty()).then_some(key);
                choose(ws, view, key, gone, true);
            }
            Act::Hide(r) => ws.set_visibility(vec![r], Vec::new(), Vec::new(), false),
            Act::Show(r) => ws.set_visibility(Vec::new(), vec![r], Vec::new(), false),
            Act::Menu(_) => {}
        }
    }
    if let Some((id, r)) = run {
        view.ran = Some((id, Some(r)));
    }
}
