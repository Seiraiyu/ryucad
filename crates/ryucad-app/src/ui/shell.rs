//! The window's tiles (design 199, Tiles and layout): an egui_tiles tree
//! of panes by tile id (`core::layout::TILES`), dragged between tab bars
//! and split by their edges as egui_tiles allows. Every tile but the
//! viewport can be closed; View > Panels opens a closed one at its
//! default place. The tree is the `ui.layout` preference (`core::layout`),
//! read when the window opens and saved at most once a second.

use crate::core::layout::{self, Dir, Home, Node};
use crate::ui::app::Opened;
use eframe::egui;
use egui_tiles::{Container, Linear, LinearDir, Tile, TileId, Tiles, Tree};

/// A pane is its tile's id.
pub type Pane = &'static str;

/// The tile ids, as `'static` strs.
fn pane(id: &str) -> Option<Pane> {
    layout::TILES
        .iter()
        .find(|(t, _)| *t == id)
        .map(|(t, _)| *t)
}

/// The tiles' egui id: one per window, so their state (tab scrolling,
/// drags) survives the tree being made again.
pub fn tree_id() -> egui::Id {
    egui::Id::unique("ryucad tiles")
}

/// The tree of a saved layout.
pub fn tree(root: &Node) -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let root = insert(&mut tiles, root);
    let mut t = Tree::new(tree_id(), root, tiles);
    // A layout saved with a one-pixel tile (#531) opens with it usable.
    fair_shares(&mut t);
    t
}

/// How small a share may be against its siblings' mean before it is taken
/// for one egui_tiles gave a dropped tile rather than one a person chose.
const LEAST_SHARE: f32 = 0.05;

/// Gives every child of a split whose share is under [`LEAST_SHARE`] of
/// its siblings' mean that mean. egui_tiles gives a tile dropped into a
/// split a share of 1, which beside the default layout's pixel-like
/// shares (1040 and 280) is a pane a pixel wide with no tab bar left to
/// drag it back by (#531).
pub fn fair_shares(tree: &mut Tree<Pane>) {
    for (_, tile) in tree.tiles.iter_mut() {
        let Tile::Container(Container::Linear(l)) = tile else {
            continue;
        };
        if l.children.len() < 2 {
            continue;
        }
        let shares: Vec<f32> = l.children.iter().map(|c| l.shares[*c]).collect();
        let total: f32 = shares.iter().sum();
        let small: Vec<TileId> = l
            .children
            .iter()
            .zip(&shares)
            .filter(|&(_, s)| {
                let mean = (total - s) / (shares.len() - 1) as f32;
                *s < LEAST_SHARE * mean
            })
            .map(|(c, _)| *c)
            .collect();
        if small.is_empty() || small.len() == shares.len() {
            continue;
        }
        let kept: Vec<f32> = l
            .children
            .iter()
            .zip(&shares)
            .filter(|(c, _)| !small.contains(c))
            .map(|(_, s)| *s)
            .collect();
        let mean = kept.iter().sum::<f32>() / kept.len() as f32;
        for c in small {
            l.shares.set_share(c, mean);
        }
    }
}

fn insert(tiles: &mut Tiles<Pane>, n: &Node) -> TileId {
    match n {
        Node::Tile(id) => tiles.insert_pane(pane(id).expect("read layouts hold only known tiles")),
        Node::Split(dir, children) => {
            let ids: Vec<TileId> = children.iter().map(|(c, _)| insert(tiles, c)).collect();
            let dir = match dir {
                Dir::Horizontal => LinearDir::Horizontal,
                Dir::Vertical => LinearDir::Vertical,
            };
            let mut linear = Linear::new(dir, ids.clone());
            for (id, (_, share)) in ids.iter().zip(children) {
                linear.shares.set_share(*id, *share);
            }
            tiles.insert_container(Container::Linear(linear))
        }
        Node::Tabs(children, active) => {
            let ids: Vec<TileId> = children.iter().map(|c| insert(tiles, c)).collect();
            let mut tabs = egui_tiles::Tabs::new(ids.clone());
            if let Some(a) = ids.get(*active) {
                tabs.set_active(*a);
            }
            tiles.insert_container(Container::Tabs(tabs))
        }
    }
}

/// The saved layout of a tree.
pub fn node(tree: &Tree<Pane>) -> Option<Node> {
    fn of(tiles: &Tiles<Pane>, id: TileId) -> Option<Node> {
        match tiles.get(id)? {
            Tile::Pane(p) => Some(Node::Tile((*p).to_owned())),
            Tile::Container(Container::Tabs(t)) => {
                let kids: Vec<(TileId, Node)> = t
                    .children
                    .iter()
                    .filter_map(|c| Some((*c, of(tiles, *c)?)))
                    .collect();
                let active = t
                    .active
                    .and_then(|a| kids.iter().position(|(c, _)| *c == a))
                    .unwrap_or(0);
                (!kids.is_empty())
                    .then(|| Node::Tabs(kids.into_iter().map(|k| k.1).collect(), active))
            }
            Tile::Container(Container::Linear(l)) => {
                let dir = match l.dir {
                    LinearDir::Horizontal => Dir::Horizontal,
                    LinearDir::Vertical => Dir::Vertical,
                };
                let kids: Vec<(Node, f32)> = l
                    .children
                    .iter()
                    .filter_map(|c| Some((of(tiles, *c)?, l.shares[*c])))
                    .collect();
                match kids.len() {
                    0 => None,
                    1 => kids.into_iter().next().map(|k| k.0),
                    _ => Some(Node::Split(dir, kids)),
                }
            }
            // The window never makes grids; one dragged into being is
            // saved as tabs.
            Tile::Container(Container::Grid(g)) => {
                let kids: Vec<Node> = g.children().filter_map(|c| of(tiles, *c)).collect();
                (!kids.is_empty()).then_some(Node::Tabs(kids, 0))
            }
        }
    }
    of(&tree.tiles, tree.root?)
}

/// The tile of a pane, if it is open.
pub fn find(tree: &Tree<Pane>, id: &str) -> Option<TileId> {
    tree.tiles
        .iter()
        .find(|(_, t)| matches!(t, Tile::Pane(p) if *p == id))
        .map(|(tid, _)| *tid)
}

/// Whether a pane is open.
pub fn is_open(tree: &Tree<Pane>, id: &str) -> bool {
    find(tree, id).is_some()
}

/// Shows `id`: opens it at its default place if it is closed (in the tab
/// group holding another tile of that place, else beside the viewport),
/// and makes it the active tab.
pub fn open(tree: &mut Tree<Pane>, id: &str) {
    let Some(p) = pane(id) else { return };
    if let Some(t) = find(tree, p) {
        tree.make_active(|tid, _| tid == t);
        return;
    }
    let home = layout::home(p);
    let neighbour = layout::TILES
        .iter()
        .map(|(t, _)| *t)
        .filter(|t| *t != p && layout::home(t) == home)
        .find_map(|t| find(tree, t));
    let group = neighbour
        .or_else(|| find(tree, "viewport"))
        .and_then(|t| tree.tiles.parent_of(t))
        .filter(|g| {
            matches!(
                tree.tiles.get(*g),
                Some(Tile::Container(Container::Tabs(_)))
            )
        });
    let new = tree.tiles.insert_pane(p);
    match group {
        Some(g) if neighbour.is_some() || home == Home::Centre => {
            if let Some(Tile::Container(c)) = tree.tiles.get_mut(g) {
                c.add_child(new);
            }
        }
        _ => {
            // A place with no tab group open: a new group beside the root.
            let tabs = tree.tiles.insert_tab_tile(vec![new]);
            match tree.root {
                Some(root) => {
                    let dir = match home {
                        Home::Bottom => LinearDir::Vertical,
                        _ => LinearDir::Horizontal,
                    };
                    let children = match home {
                        Home::Left => vec![tabs, root],
                        _ => vec![root, tabs],
                    };
                    let mut linear = Linear::new(dir, children);
                    linear.shares.set_share(root, 3.0);
                    tree.root = Some(tree.tiles.insert_container(Container::Linear(linear)));
                }
                None => tree.root = Some(tabs),
            }
        }
    }
    tree.make_active(|tid, _| tid == new);
}

/// Opens `id` as [`open`] does, as the first tab of its group.
pub fn open_first(tree: &mut Tree<Pane>, id: &str) {
    open(tree, id);
    let Some(t) = find(tree, id) else { return };
    if let Some(g) = tree.tiles.parent_of(t)
        && let Some(Tile::Container(Container::Tabs(tabs))) = tree.tiles.get_mut(g)
    {
        tabs.children.retain(|c| *c != t);
        tabs.children.insert(0, t);
    }
}

/// Closes `id` if it is open (never the viewport).
pub fn close(tree: &mut Tree<Pane>, id: &str) {
    if id == "viewport" {
        return;
    }
    if let Some(t) = find(tree, id) {
        tree.remove_recursively(t);
    }
}

/// What the tiles draw with: the window and the GPU.
pub struct Behavior<'a> {
    pub o: &'a mut Opened,
    pub gpu: Option<&'a eframe::egui_wgpu::RenderState>,
    /// A tab was closed or moved, or a split resized: the layout is saved.
    pub edited: bool,
    /// A tile was dropped: its new split's shares are made fair
    /// ([`fair_shares`]).
    pub dropped: bool,
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _: TileId, pane: &mut Pane) -> egui_tiles::UiResponse {
        let rect = ui.max_rect();
        self.o.focus.drew(pane, rect);
        let has = self.o.focus.has(pane);
        // Tab into the tile F6 reached: its first control takes it.
        if has && std::mem::take(&mut self.o.enter) {
            ui.memory_mut(|m| m.move_focus(egui::FocusDirection::Next));
        }
        // The keyboard moved while this tile drew (Tab, or a click, gave
        // one of its widgets the keyboard): the tile has it.
        let before = ui.memory(|m| m.focused());
        crate::ui::tiles::show(ui, self.o, self.gpu, pane);
        let after = ui.memory(|m| m.focused());
        if after.is_some() && after != before {
            self.o.focus.follow(pane);
        }
        // The keyboard's ring, on the tile's frame, only when the keyboard
        // brought it there.
        if has && self.o.focus.ring {
            let p = self.o.view.palette(ui.visuals().dark_mode);
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                0.0,
                egui::Stroke::new(2.0, crate::ui::style::rgb(p.focus)),
                egui::StrokeKind::Inside,
            );
        }
        egui_tiles::UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        layout::title(pane).unwrap_or(pane).into()
    }

    /// A tab: egui_tiles 0.18's default `tab_ui`, but for its close
    /// button, which is named for screen readers as `Close <tab>` rather
    /// than `Close` (#389). Copied from egui_tiles 0.18.0's
    /// `src/behavior.rs` (MIT OR Apache-2.0, Copyright (c) 2023 Rerun
    /// Technologies AB), its debug logging left out.
    fn tab_ui(
        &mut self,
        tiles: &mut Tiles<Pane>,
        ui: &mut egui::Ui,
        id: egui::Id,
        tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Response {
        use egui::{Role, Sense, Stroke, TextStyle, Vec2, WidgetInfo, vec2};
        let text = self.tab_title_for_tile(tiles, tile_id);
        let title = text.text().to_owned();
        let close_btn_size = Vec2::splat(self.close_button_outer_size());
        let close_btn_left_padding = 4.0;
        let font_id = TextStyle::Button.resolve(ui.style());
        let galley = text.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, font_id);

        let x_margin = self.tab_title_spacing(ui.visuals());

        let button_width = galley.size().x
            + 2.0 * x_margin
            + f32::from(state.closable) * (close_btn_left_padding + close_btn_size.x);
        let (_, tab_rect) = ui.allocate_space(vec2(button_width, ui.available_height()));

        let draggable = self.is_tile_draggable(tiles, tile_id);
        let sense = if draggable {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let tab_response = ui.interact(tab_rect, id, sense);
        let tab_response = if draggable {
            tab_response.on_hover_cursor(self.tab_hover_cursor_icon())
        } else {
            tab_response
        };
        tab_response.widget_info(|| {
            WidgetInfo::selected(Role::Button, ui.is_enabled(), state.active, galley.text())
        });

        if ui.is_rect_visible(tab_rect) && !state.is_being_dragged {
            let bg_color = self.tab_bg_color(ui.visuals(), tiles, tile_id, state);
            let stroke = self.tab_outline_stroke(ui.visuals(), tiles, tile_id, state);
            ui.painter().rect(
                tab_rect.shrink(0.5),
                0.0,
                bg_color,
                stroke,
                egui::StrokeKind::Inside,
            );
            if state.active {
                ui.painter().hline(
                    tab_rect.x_range(),
                    tab_rect.bottom(),
                    Stroke::new(stroke.width + 1.0, bg_color),
                );
            }
            let text_color = self.tab_text_color(ui.visuals(), tiles, tile_id, state);
            let text_position = egui::Align2::LEFT_CENTER
                .align_size_within_rect(galley.size(), tab_rect.shrink(x_margin))
                .min;
            ui.painter().galley(text_position, galley, text_color);

            if state.closable {
                let close_btn_rect = egui::Align2::RIGHT_CENTER
                    .align_size_within_rect(close_btn_size, tab_rect.shrink(x_margin));
                let close_btn_id = ui.auto_id_with("tab_close_btn");
                let close_btn_response = ui
                    .interact(close_btn_rect, close_btn_id, Sense::click_and_drag())
                    .on_hover_cursor(egui::CursorIcon::Default);
                // The one change: the button names its tab (#389).
                close_btn_response.widget_info(|| {
                    WidgetInfo::labeled(Role::Button, ui.is_enabled(), format!("Close {title}"))
                });
                let visuals = ui.style().interact(&close_btn_response);
                let rect = close_btn_rect
                    .shrink(self.close_button_inner_margin())
                    .expand(visuals.expansion);
                let stroke = visuals.fg_stroke;
                ui.painter()
                    .line_segment([rect.left_top(), rect.right_bottom()], stroke);
                ui.painter()
                    .line_segment([rect.right_top(), rect.left_bottom()], stroke);
                if (close_btn_response.clicked()
                    || tab_response.clicked_by(egui::PointerButton::Middle))
                    && self.on_tab_close(tiles, tile_id)
                {
                    tiles.remove(tile_id);
                    // egui_tiles says nothing of a close (no `on_edit`):
                    // the layout is saved all the same (#409).
                    self.edited = true;
                }
            }
        }

        self.on_tab_button(tiles, tile_id, tab_response)
    }

    fn is_tab_closable(&self, tiles: &Tiles<Pane>, tile: TileId) -> bool {
        !matches!(tiles.get(tile), Some(Tile::Pane(p)) if *p == "viewport")
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }

    /// A tab's context menu: Close (not the viewport's) and Reset
    /// layout, run once the frame is drawn.
    fn on_tab_button(
        &mut self,
        tiles: &mut Tiles<Pane>,
        tile: TileId,
        response: egui::Response,
    ) -> egui::Response {
        if let Some(Tile::Pane(p)) = tiles.get(tile) {
            let pane = *p;
            let shown = &self.o.view.commands;
            let mut ran = None;
            response.context_menu(|ui| {
                ran = crate::ui::commands::context_menu(
                    ui,
                    shown,
                    crate::core::command::Context::Tab,
                    Some(pane),
                    None,
                );
            });
            if ran.is_some() {
                self.o.view.ran = ran;
            }
        }
        response
    }

    fn tab_bar_height(&self, _: &egui::Style) -> f32 {
        24.0
    }

    fn gap_width(&self, _: &egui::Style) -> f32 {
        1.0
    }

    /// Any change (a tab chosen, moved or closed, a split resized) is
    /// saved.
    fn on_edit(&mut self, action: egui_tiles::EditAction) {
        self.edited = true;
        if matches!(action, egui_tiles::EditAction::TileDropped) {
            self.dropped = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_becomes_a_tree_and_back() {
        let d = layout::default();
        let t = tree(&d);
        assert_eq!(node(&t), Some(d));
    }

    /// A tile dropped into a split whose shares are pixel-like (1040 and
    /// 280) is given egui_tiles' share of 1: one pixel. It gets its
    /// siblings' mean instead, after a drop and when a layout saved so is
    /// read (#531).
    #[test]
    fn a_tile_dropped_into_a_split_gets_a_usable_share() {
        let tabs = |t: &str| (Node::Tabs(vec![Node::Tile(t.into())], 0), 0.0);
        let mut kids = vec![
            tabs("viewport"),
            (Node::Tile("activity".into()), 1.0),
            tabs("properties"),
        ];
        kids[0].1 = 1040.0;
        kids[2].1 = 280.0;
        let t = tree(&Node::Split(Dir::Horizontal, kids));
        let Some(Node::Split(_, kids)) = node(&t) else {
            panic!()
        };
        let shares: Vec<f32> = kids.iter().map(|k| k.1).collect();
        assert_eq!(shares, [1040.0, 660.0, 280.0]);
        // Shares a person dragged stay as they are.
        let mut t = tree(&layout::default());
        let before = node(&t);
        fair_shares(&mut t);
        assert_eq!(node(&t), before);
    }

    #[test]
    fn opening_a_closed_tile_puts_it_at_its_place() {
        let mut t = tree(&layout::default());
        close(&mut t, "history");
        assert!(!is_open(&t, "history"));
        open(&mut t, "history");
        // Back in the bottom tabs, beside the activity, and showing.
        let n = node(&t).unwrap();
        let bottom = match &n {
            Node::Split(_, c) => match &c[1].0 {
                Node::Split(_, c) => c[1].0.clone(),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        };
        match bottom {
            Node::Tabs(kids, active) => {
                assert!(kids.contains(&Node::Tile("history".into())));
                assert_eq!(kids[active], Node::Tile("history".into()));
            }
            other => panic!("{other:?}"),
        }
        // Conflicts opens first among the right tabs.
        open_first(&mut t, "conflicts");
        match node(&t).unwrap() {
            Node::Split(_, c) => match &c[1].0 {
                Node::Split(_, c) => match &c[0].0 {
                    Node::Split(_, c) => match &c[1].0 {
                        Node::Tabs(kids, active) => {
                            assert_eq!(kids[0], Node::Tile("conflicts".into()));
                            assert_eq!(*active, 0);
                        }
                        other => panic!("{other:?}"),
                    },
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        // Shortcuts opens beside the viewport; the viewport never closes.
        open(&mut t, "shortcuts");
        close(&mut t, "viewport");
        assert!(is_open(&t, "viewport"));
        assert!(is_open(&t, "shortcuts"));
    }

    #[test]
    fn a_tile_whose_place_is_gone_gets_a_group_of_its_own() {
        let mut t = tree(&Node::Tabs(vec![Node::Tile("viewport".into())], 0));
        open(&mut t, "tree");
        open(&mut t, "activity");
        let n = node(&t).unwrap();
        assert_eq!(n.tiles(), ["tree", "viewport", "activity"]);
    }
}
