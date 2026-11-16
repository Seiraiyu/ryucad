//! ryueda's command table: every action the window offers, once, by an
//! id `ui.keys` names. Menus, the toolbar, context menus, the palette and
//! the keymap are built from it.

use ryu_app::command::{Category, Command, cmd};

/// What a context menu is offered on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Here {
    /// Something picked on the canvas.
    Canvas,
    /// A row of a list (navigator, findings, instructions).
    Row,
}

pub static COMMANDS: &[Command<Here>] = &[
    cmd("view.fit", "Fit sheet", Category::View)
        .icon("arrows-out")
        .keys(&["F"])
        .viewport()
        .toolbar(),
    cmd("view.next_sheet", "Next sheet", Category::View)
        .keys(&["PageDown"])
        .toolbar(),
    cmd("view.prev_sheet", "Previous sheet", Category::View)
        .keys(&["PageUp"])
        .toolbar(),
    cmd("view.go_to", "Go to reference…", Category::View)
        .icon("magnifying-glass")
        .keys(&["Mod+G"])
        .in_text(),
    cmd("view.layer_frames", "Region frames", Category::View).toggle(),
    cmd("view.layer_notes", "Notes", Category::View).toggle(),
    cmd("view.layer_intent", "Intent", Category::View).toggle(),
    cmd("view.layer_pin_numbers", "Pin numbers", Category::View).toggle(),
    cmd(
        "view.follow_shows",
        "Follow what agents show",
        Category::View,
    )
    .toggle(),
    cmd("view.flash_changes", "Flash changes", Category::View).toggle(),
    cmd("view.theme", "Theme", Category::View).choices(),
    cmd("edit.copy_ref", "Copy reference", Category::Edit)
        .keys(&["Mod+Shift+C"])
        .context(&[Here::Canvas, Here::Row]),
    cmd("edit.clear_selection", "Clear selection", Category::Edit)
        .keys(&["Escape"])
        .viewport(),
    cmd("edit.undo", "Undo", Category::Edit)
        .icon("arrow-u-up-left")
        .keys(&["Mod+Z"])
        .in_text()
        .toolbar(),
    cmd("edit.redo", "Redo", Category::Edit)
        .icon("arrow-u-up-right")
        .keys(&["Mod+Shift+Z", "Mod+Y"])
        .in_text()
        .toolbar(),
    cmd("tools.palette", "Command palette", Category::Tools)
        .keys(&["Mod+K"])
        .in_text(),
    cmd("tools.check", "Run check", Category::Tools).icon("check-circle"),
];

/// A place the palette's `@` offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub title: String,
    /// A reference path, or `block:<id>` for a block's first sheet.
    pub reference: String,
}

/// What `@query` finds, best first: the design's designators (with their
/// values), the shown block's nets, and the blocks.
pub fn places(
    nav: &crate::core::nav::Navigator,
    design: &str,
    block: &str,
    query: &str,
) -> Vec<Place> {
    use ryu_app::palette::score;
    let mut found: Vec<(i32, Place)> = Vec::new();
    for r in nav.components(design, "") {
        let title = format!("{} {}", r.refdes, r.value).trim().to_owned();
        if let Some(s) = score(query, &title) {
            found.push((
                s,
                Place {
                    title,
                    reference: r.reference,
                },
            ));
        }
    }
    for n in nav.nets(block, "") {
        if let Some(s) = score(query, &n.name) {
            found.push((
                s,
                Place {
                    title: format!("net {}", n.name),
                    reference: n.reference,
                },
            ));
        }
    }
    let tree = nav.tree(design);
    let mut blocks = std::collections::BTreeSet::new();
    let mut stack = vec![&tree];
    while let Some(n) = stack.pop() {
        blocks.insert(n.block.clone());
        stack.extend(n.children.iter());
    }
    for b in blocks.into_iter().filter(|b| !b.is_empty()) {
        if let Some(s) = score(query, &b) {
            found.push((
                s,
                Place {
                    title: format!("block {b}"),
                    reference: format!("block:{b}"),
                },
            ));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
    found.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keys::Keymap;
    use crate::core::nav::Navigator;
    use serde_json::json;

    #[test]
    fn every_command_is_named_once_and_no_keys_conflict() {
        let mut ids: Vec<&str> = COMMANDS.iter().map(|c| c.id).collect();
        for c in COMMANDS {
            assert!(ryu_session::chords::command_id_ok(c.id), "{}", c.id);
            assert!(!c.title.is_empty());
            for k in c.keys {
                assert!(ryu_app::keys::Chord::parse(k).is_some(), "{}: {k}", c.id);
            }
        }
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "an id is listed twice");
        for mac in [false, true] {
            let k = Keymap::new((), &json!({}));
            assert!(k.conflicts(mac).is_empty(), "{:?}", k.conflicts(mac));
        }
        for want in [
            "view.fit",
            "view.next_sheet",
            "view.prev_sheet",
            "view.go_to",
            "view.layer_frames",
            "view.layer_notes",
            "view.layer_intent",
            "view.layer_pin_numbers",
            "view.follow_shows",
            "view.flash_changes",
            "view.theme",
            "edit.copy_ref",
            "edit.clear_selection",
            "edit.undo",
            "edit.redo",
            "tools.palette",
            "tools.check",
        ] {
            assert!(ryu_app::command::get(COMMANDS, want).is_some(), "{want}");
        }
    }

    #[test]
    fn at_searches_designators_nets_and_blocks() {
        let mut n = Navigator::default();
        n.designs(&json!({"designs": [{"id": "d", "name": "Breakout"}]}));
        n.netlist(
            "d",
            &json!({"root": "top", "occurrences": [
            {"refdes": "U3", "value": "BME280", "of": "x", "block": "sensor", "component": "u3"}]}),
        );
        n.block(
            &json!({"reference": "top", "instances": [{"id": "s1", "of": "sensor"}],
            "nets": [{"id": "sda", "name": "SDA"}]}),
        );
        n.block(&json!({"reference": "sensor", "instances": [], "nets": []}));
        let u = places(&n, "d", "top", "u3");
        assert_eq!(
            u[0],
            Place {
                title: "U3 BME280".into(),
                reference: "sensor/u3".into()
            }
        );
        assert_eq!(places(&n, "d", "top", "sda")[0].reference, "top/sda");
        assert_eq!(
            places(&n, "d", "top", "sens")[0],
            Place {
                title: "block sensor".into(),
                reference: "block:sensor".into()
            }
        );
    }
}
