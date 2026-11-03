//! The owner's files as a source: a root `cox-files:<set>[@vN]` names a
//! `cox files` set, pulled at split time (see [`materialize`]). Design 277,
//! "12.C: The owner's STEP files".

/// The prefix of a `cox-files:` root.
pub const PREFIX: &str = "cox-files:";

/// A `cox-files:<set>[@vN]` root: the set, and the version it is pinned at
/// (`None`: the latest, for exploratory runs only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub set: String,
    pub version: Option<u32>,
}

impl Root {
    /// `None` for a root of another form; else the root, or why it doesn't
    /// read.
    pub fn parse(root: &str) -> Option<Result<Root, String>> {
        let rest = root.strip_prefix(PREFIX)?;
        let (set, version) = match rest.split_once('@') {
            Some((set, v)) => match v.strip_prefix('v').and_then(|n| n.parse::<u32>().ok()) {
                Some(n) if n > 0 => (set, Some(n)),
                _ => return Some(Err(format!("{root}: a version is written @v1, @v2, …"))),
            },
            None => (rest, None),
        };
        let named = !set.is_empty()
            && !set.starts_with('.')
            && set
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        if !named {
            return Some(Err(format!("{root}: not a set's name")));
        }
        Some(Ok(Root {
            set: set.to_owned(),
            version,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_read_with_and_without_a_version() {
        assert_eq!(Root::parse("crates/x"), None);
        assert_eq!(
            Root::parse("cox-files:step-files@v2"),
            Some(Ok(Root {
                set: "step-files".into(),
                version: Some(2)
            }))
        );
        assert_eq!(
            Root::parse("cox-files:step-files"),
            Some(Ok(Root {
                set: "step-files".into(),
                version: None
            }))
        );
        for bad in [
            "cox-files:",
            "cox-files:a@2",
            "cox-files:a@v0",
            "cox-files:a@vx",
            "cox-files:a b",
            "cox-files:../a",
        ] {
            assert!(Root::parse(bad).unwrap().is_err(), "{bad}");
        }
    }
}
