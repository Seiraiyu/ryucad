//! The tile layout as the `ui.layout` preference keeps it (design 199,
//! Tiles and layout): format 1, a tree of tiles by id, splits with their
//! shares and tabs with the active one, saved per user, never per
//! project. A product gives its tiles as a [`Spec`]. Reading drops tiles
//! the spec doesn't have, puts back its centre (ryucad's viewport) if it
//! went, and takes a layout that can't be read (or of a newer format) as
//! the default, with a note. Moved from ryucad-app by ryueda 3.C.

use serde_json::{Value, json};

/// The format this build writes.
pub const FORMAT: u64 = 1;

/// A product's tiles.
#[derive(Clone, Copy)]
pub struct Spec {
    /// Every tile by id, with its tab title, in the order View > Panels
    /// lists them.
    pub tiles: &'static [(&'static str, &'static str)],
    /// The tile every layout has, which never closes.
    pub centre: &'static str,
    /// Where a tile goes when it is opened and isn't in the layout.
    pub home: fn(&str) -> Home,
    /// The default layout.
    pub default: fn() -> Node,
    /// The salt of the tiles' egui id.
    pub id: &'static str,
}

impl Spec {
    /// A tile's title.
    pub fn title(&self, id: &str) -> Option<&'static str> {
        self.tiles
            .iter()
            .find(|(t, _)| *t == id)
            .map(|(_, title)| *title)
    }

    /// Reads `ui.layout`: `{}` (or nothing) is the default; one that can't
    /// be read, or of a newer format, is the default with a note. Unknown
    /// tiles and repeats are dropped, the centre put back where it went
    /// missing, empty splits and tabs pruned.
    pub fn read(&self, v: &Value) -> Read {
        let default = |note: Option<String>| Read {
            root: (self.default)(),
            note,
        };
        let Some(m) = v.as_object() else {
            return default(Some(
                "The saved layout isn't an object; using the default.".into(),
            ));
        };
        if m.is_empty() {
            return default(None);
        }
        match m.get("format").and_then(Value::as_u64) {
            Some(FORMAT) => {}
            Some(f) if f > FORMAT => {
                return default(Some(format!(
                    "The saved layout is format {f}, newer than this build reads; using the default."
                )));
            }
            _ => {
                return default(Some(
                    "The saved layout has no format; using the default.".into(),
                ));
            }
        }
        let mut seen = Vec::new();
        let Some(mut root) = m.get("root").and_then(|r| self.node(r, &mut seen)) else {
            return default(Some(
                "The saved layout can't be read; using the default.".into(),
            ));
        };
        if !seen.iter().any(|t| t == self.centre) {
            root = match root {
                Node::Split(d, mut c) => {
                    c.insert(0, (tabs(&[self.centre]), 1.0));
                    Node::Split(d, c)
                }
                other => Node::Split(
                    Dir::Horizontal,
                    vec![(tabs(&[self.centre]), 3.0), (other, 1.0)],
                ),
            };
        }
        Read { root, note: None }
    }

    /// A node, keeping only this spec's tiles, each once.
    fn node(&self, v: &Value, seen: &mut Vec<String>) -> Option<Node> {
        if let Some(id) = v.get("tile").and_then(Value::as_str) {
            if self.title(id).is_none() || seen.iter().any(|s| s == id) {
                return None;
            }
            seen.push(id.to_owned());
            return Some(Node::Tile(id.to_owned()));
        }
        if let Some(dir) = v.get("split").and_then(Value::as_str) {
            let dir = match dir {
                "horizontal" => Dir::Horizontal,
                "vertical" => Dir::Vertical,
                _ => return None,
            };
            let children = v.get("children")?.as_array()?;
            let shares = v.get("shares").and_then(Value::as_array);
            let c: Vec<(Node, f32)> = children
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let share = shares
                        .and_then(|s| s.get(i))
                        .and_then(Value::as_f64)
                        .filter(|s| s.is_finite() && *s > 0.0)
                        .unwrap_or(1.0) as f32;
                    Some((self.node(c, seen)?, share))
                })
                .collect();
            return match c.len() {
                0 => None,
                1 => c.into_iter().next().map(|(n, _)| n),
                _ => Some(Node::Split(dir, c)),
            };
        }
        if let Some(children) = v.get("tabs").and_then(Value::as_array) {
            let active = v.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;
            let c: Vec<Node> = children.iter().filter_map(|c| self.node(c, seen)).collect();
            if c.is_empty() {
                return None;
            }
            let active = active.min(c.len() - 1);
            return Some(Node::Tabs(c, active));
        }
        None
    }
}

/// A tile.
pub fn tile(id: &str) -> Node {
    Node::Tile(id.to_owned())
}

/// Tabs of these tiles, the first showing.
pub fn tabs(ids: &[&str]) -> Node {
    Node::Tabs(ids.iter().map(|i| tile(i)).collect(), 0)
}

/// Which way a split lays its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Side by side.
    Horizontal,
    /// One above another.
    Vertical,
}

/// A node of the layout.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Tile(String),
    /// Children and their shares of the room.
    Split(Dir, Vec<(Node, f32)>),
    /// Tabs and the index of the one showing.
    Tabs(Vec<Node>, usize),
}

/// Where a tile goes when it is opened and isn't in the layout: the
/// default layout's tab group holding it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Home {
    Left,
    Centre,
    Right,
    Bottom,
}

impl Node {
    /// The tiles under this node, in order.
    pub fn tiles(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.walk(&mut |id| out.push(id));
        out
    }

    fn walk<'a>(&'a self, f: &mut impl FnMut(&'a str)) {
        match self {
            Node::Tile(id) => f(id),
            Node::Split(_, c) => c.iter().for_each(|(n, _)| n.walk(f)),
            Node::Tabs(c, _) => c.iter().for_each(|n| n.walk(f)),
        }
    }

    /// The JSON `ui.layout` holds for this root.
    pub fn to_value(&self) -> Value {
        fn node(n: &Node) -> Value {
            match n {
                Node::Tile(id) => json!({"tile": id}),
                Node::Split(d, c) => json!({
                    "split": match d { Dir::Horizontal => "horizontal", Dir::Vertical => "vertical" },
                    "children": c.iter().map(|(n, _)| node(n)).collect::<Vec<_>>(),
                    "shares": c.iter().map(|(_, s)| *s).collect::<Vec<_>>(),
                }),
                Node::Tabs(c, active) => json!({
                    "tabs": c.iter().map(node).collect::<Vec<_>>(),
                    "active": active,
                }),
            }
        }
        json!({"format": FORMAT, "root": node(self)})
    }
}

/// How `ui.layout` read.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub root: Node,
    /// Why the default was used, for one note in the status line.
    pub note: Option<String>,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const TOY_TILES: [(&str, &str); 4] = [
        ("canvas", "Canvas"),
        ("navigator", "Navigator"),
        ("findings", "Findings"),
        ("properties", "Properties"),
    ];

    fn toy_home(id: &str) -> Home {
        match id {
            "navigator" => Home::Left,
            "canvas" => Home::Centre,
            "properties" => Home::Right,
            _ => Home::Bottom,
        }
    }

    fn toy_default() -> Node {
        Node::Split(
            Dir::Horizontal,
            vec![(tabs(&["navigator"]), 1.0), (tabs(&["canvas"]), 3.0)],
        )
    }

    pub(crate) const TOY: Spec = Spec {
        tiles: &TOY_TILES,
        centre: "canvas",
        home: toy_home,
        default: toy_default,
        id: "toy tiles",
    };

    #[test]
    fn a_product_s_layout_reads_round_trips_and_keeps_its_centre() {
        let d = toy_default();
        assert_eq!(
            TOY.read(&d.to_value()),
            Read {
                root: d.clone(),
                note: None
            }
        );
        assert_eq!(TOY.read(&json!({})).root, d);
        assert_eq!(TOY.title("findings"), Some("Findings"));
        assert_eq!(TOY.title("viewport"), None);
        // An unknown tile goes; the centre comes back.
        let r = TOY.read(&json!({"format": 1, "root": {"tile": "findings"}}));
        assert_eq!(r.root.tiles(), ["canvas", "findings"]);
        let r = TOY.read(&json!({"format": 2}));
        assert!(r.note.unwrap().contains("newer"));
    }
}
