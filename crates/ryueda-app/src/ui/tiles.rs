//! ryueda's tiles (design 519, The window) and what each draws.

use crate::core::lists::Finding;
use crate::core::window::Window;
use crate::ui::canvas::{self, Kept};
use eframe::egui;
use egui_tiles::TileId;
use ryu_app::layout::{Dir, Home, Node, Spec, tabs};
use ryu_app::shell::Pane;
use std::time::Instant;

const TILES: [(&str, &str); 6] = [
    ("navigator", "Navigator"),
    ("canvas", "Sheet"),
    ("properties", "Properties"),
    ("findings", "Findings"),
    ("instructions", "Instructions"),
    ("activity", "Activity"),
];

fn home(id: &str) -> Home {
    match id {
        "navigator" => Home::Left,
        "canvas" => Home::Centre,
        "properties" => Home::Right,
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
                        (tabs(&["findings", "instructions", "activity"]), 200.0),
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
                _ => {}
            });
        egui_tiles::UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        SPEC.title(pane).unwrap_or(pane).into()
    }
}

fn tree_node(ui: &mut egui::Ui, w: &mut Window, n: &crate::core::nav::Node) {
    let id = ui.make_persistent_id(("nav", &n.block, &n.layer));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            let here =
                w.shown.block.as_deref() == Some(n.block.as_str()) && w.shown.layer == n.layer;
            if ui.selectable_label(here, &n.label).clicked() {
                w.show(Some(n.block.clone()), None, n.layer.clone());
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
            for (sid, name) in sheets {
                let here = w.shown.block.as_deref() == Some(n.block.as_str())
                    && w.shown.sheet.as_deref() == Some(sid.as_str());
                let mark = if reach.contains(&sid) && !here {
                    format!("{name} ●")
                } else {
                    name.clone()
                };
                if ui.selectable_label(here, mark).clicked() {
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
    ui.horizontal(|ui| {
        ui.monospace(&p.reference);
        if ui.button("Copy reference").clicked() {
            *copy = Some(p.reference.clone());
        }
    });
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
                "findings",
                "instructions",
                "navigator",
                "properties"
            ]
        );
        assert_eq!(SPEC.read(&d.to_value()).root, d);
        assert_eq!(SPEC.centre, "canvas");
        assert_eq!((SPEC.home)("findings"), Home::Bottom);
    }
}
