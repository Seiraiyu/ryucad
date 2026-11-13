//! A window's tiles (design 199, Tiles and layout): an egui_tiles tree
//! of panes by tile id, made from a [`crate::layout::Spec`]'s layout and
//! read back into one. Every tile but the spec's centre can be closed;
//! a closed one opens at its home. What each tile draws is the product's
//! (its `egui_tiles::Behavior`). Moved from ryucad-app by ryueda 3.C.

use crate::layout::{Dir, Home, Node, Spec};
use egui_tiles::{Container, Linear, LinearDir, Tile, TileId, Tiles, Tree};

/// A pane is its tile's id.
pub type Pane = &'static str;

/// The tile ids, as `'static` strs.
fn pane(spec: &Spec, id: &str) -> Option<Pane> {
    spec.tiles.iter().find(|(t, _)| *t == id).map(|(t, _)| *t)
}

/// The tiles' egui id: one per window, so their state (tab scrolling,
/// drags) survives the tree being made again.
pub fn tree_id(spec: &Spec) -> egui::Id {
    egui::Id::unique(spec.id)
}

/// The tree of a saved layout.
pub fn tree(spec: &Spec, root: &Node) -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let root = insert(spec, &mut tiles, root);
    let mut t = Tree::new(tree_id(spec), root, tiles);
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

fn insert(spec: &Spec, tiles: &mut Tiles<Pane>, n: &Node) -> TileId {
    match n {
        Node::Tile(id) => {
            tiles.insert_pane(pane(spec, id).expect("read layouts hold only known tiles"))
        }
        Node::Split(dir, children) => {
            let ids: Vec<TileId> = children
                .iter()
                .map(|(c, _)| insert(spec, tiles, c))
                .collect();
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
            let ids: Vec<TileId> = children.iter().map(|c| insert(spec, tiles, c)).collect();
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
/// group holding another tile of that place, else beside the centre),
/// and makes it the active tab.
pub fn open(spec: &Spec, tree: &mut Tree<Pane>, id: &str) {
    let Some(p) = pane(spec, id) else { return };
    if let Some(t) = find(tree, p) {
        tree.make_active(|tid, _| tid == t);
        return;
    }
    let home = (spec.home)(p);
    let neighbour = spec
        .tiles
        .iter()
        .map(|(t, _)| *t)
        .filter(|t| *t != p && (spec.home)(t) == home)
        .find_map(|t| find(tree, t));
    let group = neighbour
        .or_else(|| find(tree, spec.centre))
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
pub fn open_first(spec: &Spec, tree: &mut Tree<Pane>, id: &str) {
    open(spec, tree, id);
    let Some(t) = find(tree, id) else { return };
    if let Some(g) = tree.tiles.parent_of(t)
        && let Some(Tile::Container(Container::Tabs(tabs))) = tree.tiles.get_mut(g)
    {
        tabs.children.retain(|c| *c != t);
        tabs.children.insert(0, t);
    }
}

/// Closes `id` if it is open (never the spec's centre).
pub fn close(spec: &Spec, tree: &mut Tree<Pane>, id: &str) {
    if id == spec.centre {
        return;
    }
    if let Some(t) = find(tree, id) {
        tree.remove_recursively(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tests::TOY;

    #[test]
    fn a_toy_layout_becomes_a_tree_opens_at_home_and_keeps_its_centre() {
        let d = (TOY.default)();
        let mut t = tree(&TOY, &d);
        assert_eq!(node(&t), Some(d));
        open(&TOY, &mut t, "properties");
        assert!(is_open(&t, "properties"));
        close(&TOY, &mut t, "canvas");
        assert!(is_open(&t, "canvas"), "the centre never closes");
        close(&TOY, &mut t, "properties");
        assert!(!is_open(&t, "properties"));
    }
}
