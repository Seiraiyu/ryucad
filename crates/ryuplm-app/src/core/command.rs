//! ryuplm's command table: every action the window offers, once, by an
//! id `ui.keys` names. Menus, the palette, context menus and the keymap
//! are built from it.

use ryu_app::command::{Category, Command, cmd};
use serde_json::Value;

/// What a context menu is offered on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Here {
    /// A row of a list (Items, BOM, Where used, Findings).
    Row,
}

pub static COMMANDS: &[Command<Here>] = &[
    cmd("view.find", "Find", Category::View)
        .icon("magnifying-glass")
        .keys(&["Mod+F"])
        .in_text(),
    cmd("view.go_to", "Go to item…", Category::View)
        .keys(&["Mod+G"])
        .in_text(),
    cmd("view.show_in_bom", "Show in BOM", Category::View).context(&[Here::Row]),
    cmd("view.where_used", "Where used", Category::View).context(&[Here::Row]),
    cmd(
        "view.follow_shows",
        "Follow what agents show",
        Category::View,
    )
    .toggle(),
    cmd("view.flash_changes", "Flash changes", Category::View).toggle(),
    cmd("view.theme", "Theme", Category::View).choices(),
    cmd("view.tile_navigator", "Navigator", Category::View).toggle(),
    cmd("view.tile_items", "Items", Category::View).toggle(),
    cmd("view.tile_item", "Item", Category::View).toggle(),
    cmd("view.tile_bom", "BOM", Category::View).toggle(),
    cmd("view.tile_where_used", "Where used panel", Category::View).toggle(),
    cmd("view.tile_findings", "Findings", Category::View).toggle(),
    cmd("view.tile_changes", "Changes", Category::View).toggle(),
    cmd("view.tile_releases", "Releases", Category::View).toggle(),
    cmd("view.tile_imports", "Imports", Category::View).toggle(),
    cmd("view.tile_documents", "Documents", Category::View).toggle(),
    cmd("view.tile_activity", "Activity", Category::View).toggle(),
    cmd("edit.copy_ref", "Copy reference", Category::Edit)
        .keys(&["Mod+Shift+C"])
        .context(&[Here::Row]),
    cmd("edit.copy_number", "Copy number", Category::Edit)
        .keys(&["Mod+Alt+C"])
        .context(&[Here::Row]),
    cmd("edit.clear_selection", "Clear selection", Category::Edit).keys(&["Escape"]),
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
    /// `item/<id>`.
    pub reference: String,
}

/// The places of an `item.list` answer, as it ordered them: `@query`
/// asks `item.list {text: query, limit: 12}` (by number, a previous
/// number, name or description), so Go to costs one request whatever the
/// catalogue's size.
pub fn places(v: &Value) -> Vec<Place> {
    v["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let id = r["id"].as_str()?;
            let number = r["number"].as_str()?;
            let title = match r["name"].as_str() {
                Some(n) => format!("{number} {n}"),
                None => number.to_owned(),
            };
            Some(Place {
                title,
                reference: format!("item/{id}"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keys::Keymap;
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
            "view.find",
            "view.go_to",
            "view.show_in_bom",
            "view.where_used",
            "view.follow_shows",
            "view.flash_changes",
            "view.theme",
            "edit.copy_ref",
            "edit.copy_number",
            "edit.clear_selection",
            "edit.undo",
            "edit.redo",
            "tools.palette",
            "tools.check",
        ] {
            assert!(ryu_app::command::get(COMMANDS, want).is_some(), "{want}");
        }
        // Every tile has its toggle.
        for (id, _) in crate::core::tiles::TILES {
            let toggle = format!("view.tile_{id}");
            assert!(COMMANDS.iter().any(|c| c.id == toggle), "{toggle}");
        }
        let km = Keymap::new((), &json!(null));
        assert_eq!(km.chords("view.find")[0].text(), "Mod+F");
    }

    #[test]
    fn at_lists_the_rows_an_item_list_answered() {
        let v = json!({"items": [
            {"id": "a0000000000001", "number": "100-0042", "name": "Bracket kit"},
            {"id": "a0000000000002", "number": "100-0043", "name": null},
        ]});
        assert_eq!(
            places(&v),
            [
                Place {
                    title: "100-0042 Bracket kit".into(),
                    reference: "item/a0000000000001".into()
                },
                Place {
                    title: "100-0043".into(),
                    reference: "item/a0000000000002".into()
                },
            ]
        );
    }
}
