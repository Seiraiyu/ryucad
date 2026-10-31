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
    /// Another project's use: its folder on disk and its name.
    pub folder: Option<String>,
    pub project_name: Option<String>,
    /// Its repository on the review service (`owner/name`), for an
    /// organization's use.
    pub repo: Option<String>,
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
    /// Other projects' uses, in their checkouts on disk: working rows,
    /// then released ones.
    pub others: Vec<Use>,
    /// The other checkouts searched (the `pdm.dependents` preference's).
    pub searched: Vec<String>,
    /// The organization's uses, from the review service, once answered.
    pub org: Option<Result<Vec<Use>, Error>>,
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
            folder: u["folder"].as_str().map(str::to_owned),
            project_name: u["name"].as_str().map(str::to_owned),
            repo: u["repo"].as_str().map(str::to_owned),
        })
        .collect()
}

impl WhereUsed {
    /// A `bom.where_used` answer.
    pub fn from_answer(part: &str, v: &Value) -> WhereUsed {
        let (working, mut others): (Vec<Use>, Vec<Use>) = uses(&v["working"])
            .into_iter()
            .partition(|u| u.folder.is_none());
        let (released, theirs): (Vec<Use>, Vec<Use>) = uses(&v["released"])
            .into_iter()
            .partition(|u| u.folder.is_none());
        others.extend(theirs);
        WhereUsed {
            part: part.to_owned(),
            working,
            released,
            others,
            searched: v["dependents"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| d.as_str().map(str::to_owned))
                .collect(),
            org: None,
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

    /// The review service's organization-wide answer, working and
    /// released rows together.
    pub fn org_rows(v: &Value) -> Vec<Use> {
        let mut rows = uses(&v["working"]);
        rows.extend(uses(&v["released"]));
        rows
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
        let own = match (&self.label, &self.state) {
            (Some(l), Some(s)) => format!("{} {l}, {s} ({})", self.assembly, self.count),
            _ => format!("{} ({})", self.assembly, self.count),
        };
        match self.project_name.as_ref().or(self.repo.as_ref()) {
            Some(p) => format!("{p}: {own}"),
            None => own,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn other_projects_uses_are_told_apart() {
        let v = json!({"schema": 1, "part": "bolt", "problems": [
                {"code": "not_found", "message": "/x can't be read", "details": {"folder": "/x"}}],
            "dependents": ["/work/rig"],
            "working": [
                {"assembly": "frame", "count": 1, "uses": []},
                {"assembly": "rig", "count": 2, "uses": [], "project": "cc",
                 "folder": "/work/rig", "name": "rig"}],
            "released": [{"assembly": "rig", "label": "A", "state": "released", "count": 2,
                          "uses": [], "project": "cc", "folder": "/work/rig", "name": "rig"}]});
        let w = WhereUsed::from_answer("bolt", &v);
        assert_eq!(w.working.len(), 1);
        assert_eq!(w.others.len(), 2);
        assert_eq!(w.others[0].text(), "rig: rig (2)");
        assert_eq!(w.others[1].text(), "rig: rig A, released (2)");
        assert_eq!(w.others[0].folder.as_deref(), Some("/work/rig"));
        assert_eq!(w.searched, ["/work/rig"]);
        assert_eq!(w.problems, ["/x can't be read"]);
    }

    #[test]
    fn an_organizations_uses_name_their_repositories() {
        let v = json!({"schema": 1, "part": "bolt", "problems": [],
            "working": [{"assembly": "rig", "count": 2, "uses": [],
                         "repo": "acme/rig", "commit": "c"}],
            "released": []});
        let u = WhereUsed::org_rows(&v);
        assert_eq!(u[0].text(), "acme/rig: rig (2)");
        assert_eq!(u[0].repo.as_deref(), Some("acme/rig"));
    }

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
