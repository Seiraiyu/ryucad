//! The History tab (design 688): `history.log`'s commits, each with
//! Compare, and how many working files differ from `HEAD`.

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    /// The id's first seven, as Git shortens it.
    pub short: String,
    pub author: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
    pub subject: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    pub commits: Vec<Commit>,
    pub uncommitted: usize,
    /// The refusal's code, when `history.log` was refused.
    pub error: Option<String>,
}

impl History {
    pub fn read(v: &Value) -> History {
        let commits = v["commits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let id = c["id"].as_str()?.to_owned();
                Some(Commit {
                    short: id.chars().take(7).collect(),
                    id,
                    author: c["author"].as_str().unwrap_or_default().to_owned(),
                    time: c["time"].as_i64().unwrap_or(0),
                    subject: c["subject"].as_str().unwrap_or_default().to_owned(),
                })
            })
            .collect();
        History {
            commits,
            uncommitted: v["uncommitted"].as_array().map_or(0, Vec::len),
            error: None,
        }
    }

    pub fn failed(code: &str) -> History {
        History {
            error: Some(code.to_owned()),
            ..History::default()
        }
    }

    pub fn not_a_repo(&self) -> bool {
        self.error.as_deref() == Some("not_a_git_repo")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn commits_read_newest_first_with_what_is_uncommitted() {
        let h = History::read(&json!({
            "commits": [
                {"id": "0123456789abcdef0123456789abcdef01234567", "author": "Test", "time": 1767225600, "subject": "laid out", "files": []},
                {"id": "89abcdef0123456789abcdef0123456789abcdef", "author": "Test", "time": 1767225500, "subject": "styled", "files": []}
            ],
            "uncommitted": [{"path": "blocks/breakout/components/r3.json", "change": "created"}]
        }));
        assert_eq!(h.commits.len(), 2);
        assert_eq!(h.commits[0].short, "0123456");
        assert_eq!(h.commits[0].subject, "laid out");
        assert_eq!(h.uncommitted, 1);
        assert_eq!(h.error, None);
        let e = History::failed("not_a_git_repo");
        assert!(e.commits.is_empty() && e.not_a_repo());
    }
}
