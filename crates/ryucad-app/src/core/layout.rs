//! The tile layout as the `ui.layout` preference keeps it (design 199,
//! Tiles and layout): format 1, a tree of tiles by id, splits with their
//! shares and tabs with the active one. It is saved per user, never per
//! project. Reading drops tiles this build doesn't have, puts back the
//! viewport if it went, and takes a layout that can't be read (or of a
//! newer format) as the default, with a note. `ui::shell` turns it into
//! egui_tiles' tree and back.

use serde_json::{Value, json};

/// The format this build writes.
pub const FORMAT: u64 = 1;

/// Every tile, by id, with its tab title: the order View > Panels lists
/// them in.
pub const TILES: [(&str, &str); 15] = [
    ("viewport", "Viewport"),
    ("tree", "Tree"),
    ("properties", "Properties"),
    ("parameters", "Parameters"),
    ("part", "Part"),
    ("conflicts", "Conflicts"),
    ("changes", "Changes"),
    ("activity", "Activity"),
    ("history", "History"),
    ("releases", "Releases"),
    ("branches", "Branches"),
    ("bom", "BOM"),
    ("shortcuts", "Shortcuts"),
    ("section", "Section"),
    ("explode", "Explode"),
];

/// A tile's title.
pub fn title(id: &str) -> Option<&'static str> {
    TILES
        .iter()
        .find(|(t, _)| *t == id)
        .map(|(_, title)| *title)
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

/// A tile's place in the default layout.
pub fn home(id: &str) -> Home {
    match id {
        "tree" => Home::Left,
        "viewport" | "shortcuts" => Home::Centre,
        "properties" | "parameters" | "part" | "conflicts" | "changes" | "section" | "explode" => {
            Home::Right
        }
        _ => Home::Bottom,
    }
}

fn tile(id: &str) -> Node {
    Node::Tile(id.to_owned())
}

fn tabs(ids: &[&str]) -> Node {
    Node::Tabs(ids.iter().map(|i| tile(i)).collect(), 0)
}

/// The default layout, as 11.A's mockups draw it at 1600×1000: the tree
/// down the left (280), the viewport (1040) with the right tabs (280)
/// beside it, and the bottom tabs (170) under both.
pub fn default() -> Node {
    Node::Split(
        Dir::Horizontal,
        vec![
            (tabs(&["tree"]), 280.0),
            (
                Node::Split(
                    Dir::Vertical,
                    vec![
                        (
                            Node::Split(
                                Dir::Horizontal,
                                vec![
                                    (tabs(&["viewport"]), 1040.0),
                                    (
                                        tabs(&["properties", "parameters", "part", "changes"]),
                                        280.0,
                                    ),
                                ],
                            ),
                            752.0,
                        ),
                        (
                            tabs(&["activity", "history", "releases", "branches", "bom"]),
                            170.0,
                        ),
                    ],
                ),
                1320.0,
            ),
        ],
    )
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

/// Reads `ui.layout`: `{}` (or nothing) is the default; one that can't be
/// read, or of a newer format, is the default with a note. Unknown tiles
/// and repeats are dropped, the viewport put back where it went missing,
/// empty splits and tabs pruned.
pub fn read(v: &Value) -> Read {
    let default = |note: Option<String>| Read {
        root: default(),
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
    let Some(mut root) = m.get("root").and_then(|r| node(r, &mut seen)) else {
        return default(Some(
            "The saved layout can't be read; using the default.".into(),
        ));
    };
    if !seen.iter().any(|t| t == "viewport") {
        root = match root {
            Node::Split(d, mut c) => {
                c.insert(0, (tabs(&["viewport"]), 1.0));
                Node::Split(d, c)
            }
            other => Node::Split(
                Dir::Horizontal,
                vec![(tabs(&["viewport"]), 3.0), (other, 1.0)],
            ),
        };
    }
    Read { root, note: None }
}

/// A node, keeping only tiles this build has, each once.
fn node(v: &Value, seen: &mut Vec<String>) -> Option<Node> {
    if let Some(id) = v.get("tile").and_then(Value::as_str) {
        if title(id).is_none() || seen.iter().any(|s| s == id) {
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
                Some((node(c, seen)?, share))
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
        let c: Vec<Node> = children.iter().filter_map(|c| node(c, seen)).collect();
        if c.is_empty() {
            return None;
        }
        let active = active.min(c.len() - 1);
        return Some(Node::Tabs(c, active));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_round_trips() {
        let d = default();
        let r = read(&d.to_value());
        assert_eq!(
            r,
            Read {
                root: d.clone(),
                note: None
            }
        );
        assert_eq!(read(&json!({})).root, d);
        assert_eq!(read(&json!({})).note, None);
        // Every tile but the ones opened on demand is in it.
        for (id, _) in TILES {
            let on_demand = ["conflicts", "shortcuts", "section", "explode"].contains(&id);
            assert_eq!(d.tiles().contains(&id), !on_demand, "{id}");
        }
    }

    #[test]
    fn unknown_tiles_go_and_the_viewport_comes_back() {
        let v = json!({"format": 1, "root": {"split": "vertical",
            "children": [{"tabs": [{"tile": "tree"}, {"tile": "stage12"}, {"tile": "tree"}]},
                         {"tile": "activity"}],
            "shares": [2, 1]}});
        let r = read(&v);
        assert_eq!(r.note, None);
        assert_eq!(r.root.tiles(), ["viewport", "tree", "activity"]);
        match &r.root {
            Node::Split(Dir::Vertical, c) => assert_eq!(c[1].1, 2.0),
            other => panic!("{other:?}"),
        }
        // One tile alone, no viewport.
        let r = read(&json!({"format": 1, "root": {"tile": "history"}}));
        assert_eq!(r.root.tiles(), ["viewport", "history"]);
    }

    #[test]
    fn layouts_it_can_t_read_are_the_default_with_a_note() {
        for v in [
            json!({"format": 2, "root": {"tile": "tree"}}),
            json!({"root": {"tile": "tree"}}),
            json!({"format": 1, "root": {"tile": "nope"}}),
            json!({"format": 1}),
            json!([]),
        ] {
            let r = read(&v);
            assert_eq!(r.root, default(), "{v}");
            assert!(r.note.is_some(), "{v}");
        }
        assert!(read(&json!({"format": 2})).note.unwrap().contains("newer"));
    }

    #[test]
    fn the_active_tab_is_kept_and_clamped() {
        let v = json!({"format": 1, "root": {"tabs": [{"tile": "viewport"}, {"tile": "tree"}], "active": 9}});
        assert_eq!(
            read(&v).root,
            Node::Tabs(vec![tile("viewport"), tile("tree")], 1)
        );
    }
}
