//! Part and feature IDs, and the project's ID. A part or feature ID is a
//! file or directory name and the seed of every stable name under it, so
//! it must be portable and never change.

use crate::error::{Code, Error, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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

    /// The step of an instance path that reaches copy `index` of instance
    /// pattern `pattern`: `<pattern>:<i>`, or `<pattern>:<i>:<j>` past its
    /// first row. Not an ID a file may name a feature by.
    pub fn copy(pattern: &Id, index: [u32; 2]) -> Id {
        Id(step_text(pattern, Some(index)))
    }

    /// The pattern and index of a step [`Id::copy`] made.
    pub fn copy_of(&self) -> Option<(Id, [u32; 2])> {
        if !self.0.contains(':') {
            return None;
        }
        parse_step(&self.0, "")
            .ok()
            .and_then(|(id, c)| Some((id, c?)))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Id {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> core::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

/// Reads an ID as [`Id::parse`] checks it: a file whose keys or values
/// name objects by ID refuses one that breaks the rules.
impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> core::result::Result<Id, D::Error> {
        let s = String::deserialize(d)?;
        Id::parse(&s, "object").map_err(|e| serde::de::Error::custom(e.message))
    }
}

/// A project's identity: 128 random bits as 32 lowercase hex digits, in
/// the manifest. It survives moves and renames, and a fork keeps it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn parse(s: &str) -> Result<ProjectId> {
        let ok = s.len() == 32
            && s.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
        if !ok {
            return Err(Error::new(
                Code::InvalidId,
                format!("bad project ID {s:?}: use 32 lowercase hex digits"),
            )
            .with("id", s));
        }
        Ok(ProjectId(s.to_owned()))
    }

    /// The ID for 16 bytes: the caller draws them at random.
    pub fn from_bytes(bytes: [u8; 16]) -> ProjectId {
        ProjectId(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The step of an instance path that reaches copy `index` of instance
    /// pattern `pattern`: `<pattern>:<i>`, or `<pattern>:<i>:<j>` past its
    /// first row. Not an ID a file may name a feature by.
    pub fn copy(pattern: &Id, index: [u32; 2]) -> Id {
        Id(step_text(pattern, Some(index)))
    }

    /// The pattern and index of a step [`Id::copy`] made.
    pub fn copy_of(&self) -> Option<(Id, [u32; 2])> {
        if !self.0.contains(':') {
            return None;
        }
        parse_step(&self.0, "")
            .ok()
            .and_then(|(id, c)| Some((id, c?)))
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ProjectId {
    fn serialize<S: Serializer>(&self, s: S) -> core::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

/// One step of an instance path as text: an instance's ID, or a copy
/// `<pattern>:<i>` (`<pattern>:<i>:<j>` past the first row).
pub fn step_text(id: &Id, copy: Option<[u32; 2]>) -> String {
    match copy {
        None => id.to_string(),
        Some([i, 0]) => format!("{id}:{i}"),
        Some([i, j]) => format!("{id}:{i}:{j}"),
    }
}

/// Reads one step of an instance path: an ID, or a copy's.
pub fn parse_step(s: &str, at: &str) -> Result<(Id, Option<[u32; 2]>)> {
    let mut parts = s.split(':');
    let head = parts.next().unwrap_or_default();
    let id = Id::parse(head, "instance").map_err(|e| e.with("pointer", at))?;
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        return Ok((id, None));
    }
    let index = |t: &str| t.parse::<u32>().ok().filter(|_| !t.starts_with('+'));
    let copy = match rest.as_slice() {
        [i] => index(i).map(|i| [i, 0]),
        [i, j] => index(i).zip(index(j)).map(|(i, j)| [i, j]),
        _ => None,
    };
    match copy {
        Some([0, 0]) => Err(invalid_step(
            at,
            "copy 0 of a pattern is its seed: name the seed",
        )),
        Some(c) if step_text(&id, Some(c)) == s => Ok((id, Some(c))),
        _ => Err(invalid_step(
            at,
            "a pattern's copy is <pattern>:<i>, or <pattern>:<i>:<j> past its first row",
        )),
    }
}

fn invalid_step(at: &str, msg: &str) -> Error {
    Error::new(Code::InvalidDocument, msg).with("pointer", at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_deserialize_only_when_valid() {
        let id: Id = serde_json::from_value(serde_json::json!("g12")).unwrap();
        assert_eq!(id.as_str(), "g12");
        for bad in ["G1", "", "con", "a/b"] {
            assert!(
                serde_json::from_value::<Id>(serde_json::json!(bad)).is_err(),
                "{bad}"
            );
        }
        let m: std::collections::BTreeMap<Id, u8> =
            serde_json::from_value(serde_json::json!({"a": 1})).unwrap();
        assert_eq!(m.keys().next().unwrap().as_str(), "a");
    }

    #[test]
    fn project_ids_are_32_lowercase_hex() {
        let id = ProjectId::from_bytes([0xab; 16]);
        assert_eq!(id.as_str(), "ab".repeat(16));
        assert_eq!(ProjectId::parse(id.as_str()).unwrap(), id);
        for bad in [
            "",
            "AB".repeat(16).as_str(),
            &"a".repeat(31),
            &"g".repeat(32),
        ] {
            assert_eq!(ProjectId::parse(bad).unwrap_err().code, Code::InvalidId);
        }
    }

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
