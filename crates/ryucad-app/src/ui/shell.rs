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
    Tree::new(tree_id(), root, tiles)
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
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _: TileId, pane: &mut Pane) -> egui_tiles::UiResponse {
        crate::ui::tiles::show(ui, self.o, self.gpu, pane);
        egui_tiles::UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        layout::title(pane).unwrap_or(pane).into()
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
    fn on_edit(&mut self, _: egui_tiles::EditAction) {
        self.edited = true;
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
