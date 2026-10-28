//! Where used, in the Part section: every assembly in the working files
//! that reaches the chosen part, and every official release of an
//! assembly that does, with its state, as `bom.where_used` gives them
//! (from the files, at each release's commit).

use ryucad::error::Error;
use serde_json::Value;

/// One assembly that uses the part.
#[derive(Clone, Debug, PartialEq)]
pub struct Use {
    pub assembly: String,
    /// The release, for a released assembly.
    pub label: Option<String>,
    /// `released`, `superseded` or `obsolete`, for a released assembly.
    pub state: Option<String>,
    /// How many one of the assembly holds.
    pub count: u64,
    /// Each way down to the part: `frame > sub: 4 at working`.
    pub paths: Vec<String>,
}

/// What the Part section shows of where a part is used.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WhereUsed {
    pub part: String,
    pub working: Vec<Use>,
    pub released: Vec<Use>,
    /// Why no released assembly was searched (no release branch).
    pub note: Option<String>,
    /// Releases that couldn't be read.
    pub problems: Vec<String>,
    pub error: Option<Error>,
}

fn uses(v: &Value) -> Vec<Use> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|u| Use {
            assembly: u["assembly"].as_str().unwrap_or_default().to_owned(),
            label: u["label"].as_str().map(str::to_owned),
            state: u["state"].as_str().map(str::to_owned),
            count: u["count"].as_u64().unwrap_or(0),
            paths: u["uses"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|w| {
                    let via: Vec<&str> = w["via"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    format!(
                        "{}: {} at {}",
                        via.join(" > "),
                        w["count"],
                        crate::core::bom::rev_text(&w["rev"])
                    )
                })
                .collect(),
        })
        .collect()
}

impl WhereUsed {
    /// A `bom.where_used` answer.
    pub fn from_answer(part: &str, v: &Value) -> WhereUsed {
        WhereUsed {
            part: part.to_owned(),
            working: uses(&v["working"]),
            released: uses(&v["released"]),
            note: v["released_note"].as_str().map(str::to_owned),
            problems: v["problems"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["message"].as_str().map(str::to_owned))
                .collect(),
            error: None,
        }
    }

    pub fn failed(part: &str, e: Error) -> WhereUsed {
        WhereUsed {
            part: part.to_owned(),
            error: Some(e),
            ..WhereUsed::default()
        }
    }
}

impl Use {
    /// The use as the section lists it: `frame (5)`, or for a release
    /// `frame A, superseded (5)`.
    pub fn text(&self) -> String {
        match (&self.label, &self.state) {
            (Some(l), Some(s)) => format!("{} {l}, {s} ({})", self.assembly, self.count),
            _ => format!("{} ({})", self.assembly, self.count),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uses_now_and_in_releases_read_as_people_say_them() {
        let v = json!({"schema": 1, "part": "bolt", "problems": [],
            "working": [{"assembly": "frame", "count": 5, "uses": [
                {"via": ["frame", "sub"], "rev": "working", "count": 4},
                {"via": ["frame"], "rev": {"label": "A"}, "count": 1}]}],
            "released": [{"assembly": "frame", "label": "A", "state": "superseded",
                          "commit": "c", "count": 4, "uses": [
                {"via": ["frame", "sub"], "rev": {"label": "A"}, "count": 4}]}]});
        let w = WhereUsed::from_answer("bolt", &v);
        assert_eq!(w.working[0].text(), "frame (5)");
        assert_eq!(
            w.working[0].paths,
            ["frame > sub: 4 at working", "frame: 1 at A"]
        );
        assert_eq!(w.released[0].text(), "frame A, superseded (4)");
        assert_eq!(w.note, None);
        let none = WhereUsed::from_answer(
            "bolt",
            &json!({"working": [], "released": [], "released_note": "no release branch"}),
        );
        assert_eq!(none.note.as_deref(), Some("no release branch"));
    }
}
