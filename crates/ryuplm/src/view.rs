//! What a participant's window shows besides its selection (ryu-session's
//! `Product::View` for ryuplm, design 938's live session): the open item,
//! which of its tabs, and the Items list's filter. Session state, never
//! written to the catalogue.

use crate::error::{Code, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The bottom tabs a view may name, as the window's tiles are named.
pub const TABS: [&str; 8] = [
    "bom",
    "where_used",
    "findings",
    "changes",
    "releases",
    "imports",
    "documents",
    "activity",
];

/// The Items list's filter, as `item list` takes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct List {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// `errors` or `warnings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faults: Option<String>,
    /// An import mapping's name: items it made or updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `id`, `number`, `name` or `class`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
}

/// What a participant shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The open item's ID.
    pub item: Option<String>,
    /// Which bottom tab, one of [`TABS`].
    pub tab: Option<String>,
    pub list: List,
    /// Who set the view last: the participant itself (`view.set`) or the
    /// one whose `view.show` moved it.
    pub shown_by: Option<String>,
}

fn bad(param: &str, value: &str, allowed: &[&str]) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("{param} is {}, not {value:?}", allowed.join(", ")),
    )
    .with("param", param)
    .with("allowed", allowed)
}

/// Refuses a tab the window doesn't have.
pub fn check_tab(tab: &str) -> Result<()> {
    if TABS.contains(&tab) {
        Ok(())
    } else {
        Err(bad("tab", tab, &TABS))
    }
}

impl List {
    /// Refuses a sort or a fault filter `item list` would refuse. The
    /// text, class and state are filters: one nothing matches lists
    /// nothing.
    pub fn check(&self) -> Result<()> {
        use crate::listing::Sort;
        if let Some(s) = &self.sort
            && Sort::parse(s).is_none()
        {
            return Err(bad("list.sort", s, &Sort::NAMES));
        }
        if let Some(f) = &self.faults
            && f != "errors"
            && f != "warnings"
        {
            return Err(bad("list.faults", f, &["errors", "warnings"]));
        }
        Ok(())
    }
}

/// What a show of `reference` (as the session keeps it) moves a view
/// to: an item opens it; a line opens its item at the BOM tab; an import
/// opens the Imports tab and leaves the item.
pub fn shown_at(reference: &str) -> (Option<String>, Option<&'static str>) {
    if reference.starts_with("import/") {
        return (None, Some("imports"));
    }
    match crate::refs::Ref::parse(reference) {
        Some(crate::refs::Ref::Item(i)) => (Some(i.to_string()), None),
        Some(crate::refs::Ref::Line(i, _)) => (Some(i.to_string()), Some("bom")),
        None => (None, None),
    }
}

/// What a participant's JSON says of its view: `item`, `tab`, `list`
/// and `shown_by`, null where unset.
pub fn to_json(v: &View) -> Value {
    json!({"item": v.item, "tab": v.tab, "list": v.list, "shown_by": v.shown_by})
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_session::live::{Kind, Live};
    use serde_json::json;

    #[test]
    fn a_participants_view_is_in_its_json() {
        let mut live = Live::<crate::Plm>::new();
        let dana = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        let v = &mut live.get_mut(&dana).unwrap().view;
        v.item = Some("k3v9q2m8x4c1p".into());
        v.tab = Some("bom".into());
        v.list.sort = Some("number".into());
        v.shown_by = Some(dana.clone());
        let p = &live.list()["participants"][0];
        assert_eq!(p["item"], "k3v9q2m8x4c1p");
        assert_eq!(p["tab"], "bom");
        assert_eq!(p["list"], json!({"sort": "number"}));
        assert_eq!(p["shown_by"], json!(dana));
    }

    #[test]
    fn tabs_and_filters_are_checked() {
        assert!(check_tab("bom").is_ok() && check_tab("documents").is_ok());
        assert_eq!(
            check_tab("scene").unwrap_err().code,
            crate::error::Code::InvalidParam
        );
        let bad = List {
            sort: Some("colour".into()),
            ..Default::default()
        };
        assert_eq!(bad.check().unwrap_err().details["param"], "list.sort");
        let bad = List {
            faults: Some("many".into()),
            ..Default::default()
        };
        assert_eq!(bad.check().unwrap_err().details["param"], "list.faults");
        assert!(
            List {
                sort: Some("name".into()),
                faults: Some("errors".into()),
                ..Default::default()
            }
            .check()
            .is_ok()
        );
    }

    #[test]
    fn a_show_opens_an_item_or_a_line_s_bom_or_the_imports() {
        assert_eq!(
            shown_at("item/k3v9q2m8x4c1p"),
            (Some("k3v9q2m8x4c1p".into()), None)
        );
        assert_eq!(
            shown_at("item/k3v9q2m8x4c1p/line/0000000000001"),
            (Some("k3v9q2m8x4c1p".into()), Some("bom"))
        );
        assert_eq!(shown_at("import/erp"), (None, Some("imports")));
    }
}
