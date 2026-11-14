//! The tile layout as the `ui.layout` preference keeps it (design 199,
//! Tiles and layout): format 1, a tree of tiles by id, splits with their
//! shares and tabs with the active one. It is saved per user, never per
//! project. Reading drops tiles this build doesn't have, puts back the
//! viewport if it went, and takes a layout that can't be read (or of a
//! newer format) as the default, with a note. `ui::shell` turns it into
//! egui_tiles' tree and back.

use serde_json::Value;

pub use ryu_app::layout::{Dir, FORMAT, Home, Node, Read};
use ryu_app::layout::{Spec, tabs};

/// ryucad's tiles, to the shell.
pub const SPEC: Spec = Spec {
    tiles: &TILES,
    centre: "viewport",
    home,
    default,
    id: "ryucad tiles",
};

/// Every tile, by id, with its tab title: the order View > Panels lists
/// them in.
pub const TILES: [(&str, &str); 20] = [
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
    ("deps", "Dependencies"),
    ("shortcuts", "Shortcuts"),
    ("section", "Section"),
    ("explode", "Explode"),
    ("motion", "Motion"),
    ("equations", "Equations"),
    ("annotations", "Annotations"),
    ("drawings", "Drawings"),
];

/// A tile's title.
pub fn title(id: &str) -> Option<&'static str> {
    SPEC.title(id)
}

/// Reads `ui.layout` ([`Spec::read`]).
pub fn read(v: &Value) -> Read {
    SPEC.read(v)
}

/// A tile's place in the default layout.
pub fn home(id: &str) -> Home {
    match id {
        "tree" => Home::Left,
        "viewport" | "shortcuts" | "drawings" => Home::Centre,
        "properties" | "parameters" | "part" | "conflicts" | "changes" | "section" | "explode"
        | "motion" | "annotations" => Home::Right,
        _ => Home::Bottom,
    }
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
                            tabs(&["activity", "history", "releases", "branches", "bom", "deps"]),
                            170.0,
                        ),
                    ],
                ),
                1320.0,
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_app::layout::tile;
    use serde_json::json;

    #[test]
    fn dependencies_sit_among_the_bottom_tabs() {
        assert_eq!(title("deps"), Some("Dependencies"));
        assert_eq!(home("deps"), Home::Bottom);
        let Node::Split(_, top) = default() else {
            panic!()
        };
        let Node::Split(_, right) = &top[1].0 else {
            panic!()
        };
        let Node::Tabs(bottom, _) = &right[1].0 else {
            panic!()
        };
        assert_eq!(bottom.last(), Some(&Node::Tile("deps".into())));
    }

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
            let on_demand = [
                "conflicts",
                "shortcuts",
                "section",
                "explode",
                "motion",
                "equations",
                "annotations",
                "drawings",
            ]
            .contains(&id);
            assert_eq!(d.tiles().contains(&id), !on_demand, "{id}");
        }
    }

    #[test]
    fn the_motion_tile_opens_on_the_right_beside_explode() {
        assert_eq!(title("motion"), Some("Motion"));
        assert_eq!(home("motion"), Home::Right);
        assert!(!default().tiles().contains(&"motion"), "opened on demand");
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
