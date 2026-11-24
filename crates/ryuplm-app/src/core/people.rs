//! Who else is attached, and what each selects, highlights, says and
//! shows: `ryu_app::people`', with what a ryuplm window shows
//! ([`ItemView`]).

use ryuplm::view::List;
use serde_json::Value;

pub use ryu_app::people::Highlight;

/// Where a participant's window looks (`view.set`, `view.show`), read
/// from their JSON: `item`, `tab`, `list` and `shown_by`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemView {
    pub item: Option<String>,
    pub tab: Option<String>,
    pub list: List,
    pub shown_by: Option<String>,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

impl ryu_app::people::View for ItemView {
    fn from_json(v: &Value) -> Self {
        ItemView {
            item: text(&v["item"]),
            tab: text(&v["tab"]),
            list: serde_json::from_value(v["list"].clone()).unwrap_or_default(),
            shown_by: text(&v["shown_by"]),
        }
    }
    /// A participant can be followed once their window has set a view.
    fn followable(&self) -> bool {
        self.shown_by.is_some()
    }
}

pub type Person = ryu_app::people::Person<ItemView>;
pub type People = ryu_app::people::People<ItemView>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_person_s_item_view_reads_through() {
        let mut p = People::default();
        p.reset(&json!([
            {"id": "p-1", "kind": "human", "name": "Dana", "selection": ["item/k3v9q2m8x4c1p"],
             "highlights": [], "item": "k3v9q2m8x4c1p", "tab": "bom",
             "list": {"text": "bolt", "sort": "number"}, "shown_by": "p-2"},
            {"id": "p-2", "kind": "agent", "name": "agent", "selection": [], "highlights": [],
             "item": null, "tab": null, "list": {}, "shown_by": null},
        ]));
        let dana = p.get("p-1").unwrap();
        assert_eq!(dana.item.as_deref(), Some("k3v9q2m8x4c1p"));
        assert_eq!(dana.tab.as_deref(), Some("bom"));
        assert_eq!(dana.list.text.as_deref(), Some("bolt"));
        assert_eq!(dana.shown_by.as_deref(), Some("p-2"));
        use ryu_app::people::View;
        assert!(dana.view.followable() && !p.get("p-2").unwrap().view.followable());
    }
}
