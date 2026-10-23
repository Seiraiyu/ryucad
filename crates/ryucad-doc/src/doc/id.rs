//! Part and feature IDs. An ID is a file or directory name and the seed of
//! every stable name under it, so it must be portable and never change.

use crate::error::{Code, Error, Result};
use serde::{Serialize, Serializer};
use std::fmt;

/// Names Windows reserves for devices, in any case and with any extension.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// A validated ID: `^[a-z0-9][a-z0-9_-]{0,62}$`, not a reserved Windows
/// name. Lowercase only, so IDs that differ by case can't collide on a
/// case-insensitive filesystem.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(String);

impl Id {
    /// Checks `s` as the ID of a `what` (`"part"` or `"feature"`).
    pub fn parse(s: &str, what: &str) -> Result<Id> {
        let b = s.as_bytes();
        let ok_first = b
            .first()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let ok_rest = b
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-');
        if !(ok_first && ok_rest && b.len() <= 63) {
            return Err(Error::new(
                Code::InvalidId,
                format!(
                    "bad {what} ID {s:?}: use 1 to 63 of a-z, 0-9, _ and -, starting with a letter or digit"
                ),
            )
            .with("id", s));
        }
        if RESERVED.contains(&s) {
            return Err(Error::new(
                Code::InvalidId,
                format!("bad {what} ID {s:?}: reserved on Windows"),
            )
            .with("id", s));
        }
        Ok(Id(s.to_owned()))
    }

    /// `<prefix>-<n>` for the smallest `n` from 1 that `taken` refuses.
    pub fn generate(prefix: &str, taken: impl Fn(&str) -> bool) -> Id {
        (1u64..)
            .map(|n| format!("{prefix}-{n}"))
            .find(|s| !taken(s))
            .map(Id)
            .expect("some n is free")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> core::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_portable_names() {
        for s in ["a", "0", "base_plate", "box-1", "a-b_c9", &"x".repeat(63)] {
            assert_eq!(Id::parse(s, "feature").unwrap().as_str(), s);
        }
    }

    #[test]
    fn rejects_everything_else() {
        for s in [
            "",
            "-a",
            "_a",
            "A",
            "Base",
            "a b",
            "a.b",
            "a/b",
            "é",
            &"x".repeat(64),
            "con",
            "nul",
            "com1",
            "lpt9",
        ] {
            let e = Id::parse(s, "part").unwrap_err();
            assert_eq!(e.code, Code::InvalidId, "{s:?}");
            assert_eq!(e.details["id"], s);
        }
        // Only the exact device names are reserved.
        assert!(Id::parse("console", "part").is_ok());
        assert!(Id::parse("com10", "part").is_ok());
    }

    #[test]
    fn generated_ids_fill_the_first_gap() {
        let taken = ["box-1", "box-2", "box-4"];
        assert_eq!(
            Id::generate("box", |s| taken.contains(&s)).as_str(),
            "box-3"
        );
        assert_eq!(Id::generate("torus", |_| false).as_str(), "torus-1");
    }
}
