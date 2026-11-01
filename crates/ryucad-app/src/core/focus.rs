//! Which tile has the keyboard (design 199, Accessibility): F6 and
//! Shift+F6 move it from tile to tile in the order the tiles are drawn,
//! a click puts it where it lands, and it follows the widget Tab gives
//! the keyboard to (the tile it is drawn in). Only a tile reached by the keyboard shows its ring,
//! as a browser's `:focus-visible` does. With the viewport's, its own
//! keys work (the arrows orbit, Shift and the arrows pan, + and - zoom).

use eframe::egui::{Pos2, Rect};

/// A tile's id, as `core::layout::TILES` names it.
pub type Tile = &'static str;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Focus {
    /// The tile with the keyboard.
    pub tile: Option<Tile>,
    /// It came by the keyboard: its ring shows.
    pub ring: bool,
    /// F6 has just moved it: its first control takes the keyboard at the
    /// next Tab.
    pub entered: bool,
    /// The tiles drawn this frame, in order, with where.
    drawing: Vec<(Tile, Rect)>,
    /// Those of the last whole frame.
    drawn: Vec<(Tile, Rect)>,
}

impl Focus {
    /// A frame begins: the tiles it draws are noted afresh.
    pub fn begin(&mut self) {
        self.drawn = std::mem::take(&mut self.drawing);
        // A tile closed since is let go.
        if let Some(t) = self.tile
            && !self.drawn.is_empty()
            && !self.drawn.iter().any(|(d, _)| *d == t)
        {
            self.tile = None;
            self.ring = false;
        }
    }

    /// Tile `id` was drawn in `rect` this frame.
    pub fn drew(&mut self, id: Tile, rect: Rect) {
        self.drawing.push((id, rect));
    }

    /// The tiles of the last whole frame, in order.
    pub fn tiles(&self) -> impl Iterator<Item = Tile> + '_ {
        self.drawn.iter().map(|(t, _)| *t)
    }

    /// Where tile `id` was last drawn.
    pub fn rect(&self, id: Tile) -> Option<Rect> {
        self.drawn.iter().find(|(t, _)| *t == id).map(|(_, r)| *r)
    }

    /// The tile at `p`, if any.
    pub fn at(&self, p: Pos2) -> Option<Tile> {
        self.drawn
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(t, _)| *t)
    }

    /// F6 (`back`: Shift+F6): the next tile drawn after the one with the
    /// keyboard, round to the first; with none, the first (the last).
    pub fn step(&mut self, back: bool) -> Option<Tile> {
        let order: Vec<Tile> = self.tiles().collect();
        if order.is_empty() {
            return None;
        }
        let n = order.len();
        let next = match self.tile.and_then(|t| order.iter().position(|o| *o == t)) {
            Some(i) if back => order[(i + n - 1) % n],
            Some(i) => order[(i + 1) % n],
            None if back => order[n - 1],
            None => order[0],
        };
        self.tile = Some(next);
        self.ring = true;
        self.entered = true;
        Some(next)
    }

    /// A press at `p`: the tile it lands in has the keyboard, without its
    /// ring.
    pub fn clicked(&mut self, p: Pos2) {
        if let Some(t) = self.at(p) {
            self.tile = Some(t);
        }
        self.ring = false;
        self.entered = false;
    }

    /// A widget of tile `id` took the keyboard (Tab moved it there, or a
    /// click): the tile has it too, keeping the ring as it was.
    pub fn follow(&mut self, id: Tile) {
        if self.tile != Some(id) {
            self.tile = Some(id);
            self.entered = false;
        }
    }

    /// Whether `id` has the keyboard.
    pub fn has(&self, id: Tile) -> bool {
        self.tile == Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn drawn(f: &mut Focus, tiles: &[Tile]) {
        f.begin();
        for (i, t) in tiles.iter().enumerate() {
            let x = i as f32 * 100.0;
            f.drew(t, Rect::from_min_max(pos2(x, 0.0), pos2(x + 100.0, 100.0)));
        }
        f.begin();
    }

    #[test]
    fn f6_goes_round_the_tiles_in_their_order() {
        let mut f = Focus::default();
        assert_eq!(f.step(false), None);
        drawn(&mut f, &["tree", "viewport", "properties", "activity"]);
        assert_eq!(f.step(false), Some("tree"));
        assert!(f.ring && f.entered);
        assert_eq!(f.step(false), Some("viewport"));
        assert_eq!(f.step(false), Some("properties"));
        assert_eq!(f.step(false), Some("activity"));
        assert_eq!(f.step(false), Some("tree"));
        // Shift+F6 goes back, round to the last.
        assert_eq!(f.step(true), Some("activity"));
        let mut g = Focus::default();
        drawn(&mut g, &["tree", "viewport"]);
        assert_eq!(g.step(true), Some("viewport"));
    }

    #[test]
    fn a_click_takes_the_keyboard_without_the_ring() {
        let mut f = Focus::default();
        drawn(&mut f, &["tree", "viewport", "properties"]);
        f.step(false);
        f.clicked(pos2(150.0, 50.0));
        assert_eq!(f.tile, Some("viewport"));
        assert!(!f.ring && !f.entered);
        // Off every tile (a menu, the status line), nothing moves.
        f.clicked(pos2(150.0, 500.0));
        assert_eq!(f.tile, Some("viewport"));
        // F6 from there goes on from it, ring and all.
        assert_eq!(f.step(false), Some("properties"));
        assert!(f.ring);
    }

    #[test]
    fn the_keyboard_s_tile_follows_tab_and_lets_go_of_a_closed_tile() {
        let mut f = Focus::default();
        drawn(&mut f, &["tree", "viewport", "properties"]);
        f.step(false);
        f.follow("properties");
        assert_eq!(f.tile, Some("properties"));
        assert!(f.ring);
        assert!(f.has("properties") && !f.has("tree"));
        // The Properties tile closed: nothing has the keyboard.
        drawn(&mut f, &["tree", "viewport"]);
        assert_eq!(f.tile, None);
        assert!(!f.ring);
    }
}
