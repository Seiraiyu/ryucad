//! Who else is attached, and what each selects, highlights, says and
//! shows: `ryu_app::people`', with what a ryueda window shows
//! ([`SheetView`]).

use serde_json::Value;

pub use ryu_app::people::Highlight;

/// Where a participant's window looks (`view.set`, `view.show`), read
/// from their JSON: `design`, `block`, `sheet`, `layer`, `camera` and
/// `shown_by`, each `None` where unset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetView {
    pub design: Option<String>,
    pub block: Option<String>,
    pub sheet: Option<String>,
    pub layer: Option<String>,
    /// `(centre_nm, zoom_nm_per_pt)`.
    pub camera: Option<([i64; 2], i64)>,
    pub shown_by: Option<String>,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

impl ryu_app::people::View for SheetView {
    fn from_json(v: &Value) -> Self {
        let camera = (|| {
            let c = &v["camera"];
            let x = c["centre_nm"][0].as_i64()?;
            let y = c["centre_nm"][1].as_i64()?;
            let z = c["zoom_nm_per_pt"].as_i64()?;
            Some(([x, y], z))
        })();
        SheetView {
            design: text(&v["design"]),
            block: text(&v["block"]),
            sheet: text(&v["sheet"]),
            layer: text(&v["layer"]),
            camera,
            shown_by: text(&v["shown_by"]),
        }
    }
    /// A participant can be followed once their window shows a sheet.
    fn followable(&self) -> bool {
        self.block.is_some() && self.sheet.is_some()
    }
}

pub type Person = ryu_app::people::Person<SheetView>;
pub type People = ryu_app::people::People<SheetView>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_person_s_sheet_view_reads_through() {
        let mut p = People::default();
        p.reset(&json!([
            {"id": "p-1", "kind": "human", "name": "Dana", "selection": ["top/r1"], "highlights": [],
             "block": "top", "sheet": "s1", "layer": null, "design": "d",
             "camera": {"centre_nm": [10, 20], "zoom_nm_per_pt": 30000}, "shown_by": "p-2"},
            {"id": "p-2", "kind": "agent", "name": "agent", "selection": [], "highlights": []},
        ]));
        let dana = p.get("p-1").unwrap();
        assert_eq!(dana.sheet.as_deref(), Some("s1"));
        assert_eq!(dana.camera, Some(([10, 20], 30_000)));
        assert_eq!(dana.shown_by.as_deref(), Some("p-2"));
        use ryu_app::people::View;
        assert!(dana.view.followable() && !p.get("p-2").unwrap().view.followable());
    }
}
