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

    /// The step of an instance path that reaches copy `key` of instance
    /// pattern `pattern`: `<pattern>:<key>`. Not an ID a file may name a
    /// feature by.
    pub fn copy(pattern: &Id, key: &CopyKey) -> Id {
        Id(step_text(pattern, Some(key)))
    }

    /// The key an assembly's item numbers give another project's part:
    /// `<project ID>:<part>`. Not an ID a file may name a part by.
    pub fn foreign(project: &ProjectId, part: &Id) -> Id {
        Id(format!("{project}:{}", part.0))
    }

    /// The project and part of a key [`Id::foreign`] made.
    pub fn foreign_of(&self) -> Option<(ProjectId, Id)> {
        let (project, part) = self.0.split_once(':')?;
        Some((
            ProjectId::parse(project).ok()?,
            Id::parse(part, "part").ok()?,
        ))
    }

    /// The pattern and key of a step [`Id::copy`] made.
    pub fn copy_of(&self) -> Option<(Id, CopyKey)> {
        if !self.0.contains(':') {
            return None;
        }
        parse_step(&self.0, "")
            .ok()
            .and_then(|(id, c)| Some((id, c?)))
    }

    /// The key of solid `rest` of the part an insert `insert` reads:
    /// `<insert>/<rest>` (design 521). Not an ID a file may name a
    /// feature by: [`Id::parse`] refuses `/`, and [`Id::solid_key`] reads
    /// one.
    pub fn solid_path(insert: &Id, rest: &Id) -> Id {
        Id(format!("{}/{}", insert.0, rest.0))
    }

    /// Reads a solid's key: a founder's ID, or an inserted solid's
    /// `<insert>/<source key>`, every segment an ID. A key without `/` is
    /// exactly what [`Id::parse`] reads, so files naming solids by their
    /// founders read and write as before.
    pub fn solid_key(s: &str, what: &str) -> Result<Id> {
        if !s.contains('/') {
            return Id::parse(s, what);
        }
        for segment in s.split('/') {
            Id::parse(segment, what).map_err(|e| e.with("id", s))?;
        }
        Ok(Id(s.to_owned()))
    }

    /// Whether it is an inserted solid's key.
    pub fn is_path(&self) -> bool {
        self.0.contains('/')
    }

    /// The feature of its part a solid's key names first: the founder,
    /// or the insert that founded it. Itself for any other ID.
    pub fn founder_str(&self) -> &str {
        self.0.split_once('/').map_or(&self.0, |(f, _)| f)
    }

    /// [`Id::founder_str`] as an ID.
    pub fn founder(&self) -> Id {
        Id(self.founder_str().to_owned())
    }

    /// The source's key below the insert: `b/c` of `a/b/c`; `None` for a
    /// founder's key.
    pub fn rest(&self) -> Option<Id> {
        self.0.split_once('/').map(|(_, r)| Id(r.to_owned()))
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

/// An ID orders, compares and hashes as its text, so maps keyed by IDs
/// answer `&str` lookups (a solid key's founder, `Id::founder_str`).
impl core::borrow::Borrow<str> for Id {
    fn borrow(&self) -> &str {
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

    /// The step of an instance path that reaches copy `key` of instance
    /// pattern `pattern`: `<pattern>:<key>`. Not an ID a file may name a
    /// feature by.
    pub fn copy(pattern: &Id, key: &CopyKey) -> Id {
        Id(step_text(pattern, Some(key)))
    }

    /// The pattern and key of a step [`Id::copy`] made.
    pub fn copy_of(&self) -> Option<(Id, CopyKey)> {
        if !self.0.contains(':') {
            return None;
        }
        parse_step(&self.0, "")
            .ok()
            .and_then(|(id, c)| Some((id, c?)))
    }
}

/// Reads a project ID as [`ProjectId::parse`] checks it.
impl<'de> Deserialize<'de> for ProjectId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> core::result::Result<ProjectId, D::Error> {
        let s = String::deserialize(d)?;
        ProjectId::parse(&s).map_err(|e| serde::de::Error::custom(e.message))
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

/// Which copy of an instance pattern's seed a path step names (design
/// 521): an index (`<pattern>:<i>`, `<pattern>:<i>:<j>` past the first
/// row), or the sketch point that drives it (`<pattern>:<point>`). A
/// point whose ID is a canonical number is that index, so a key reads
/// back as it was made ([`CopyKey::point`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CopyKey {
    Index([u32; 2]),
    Point(Id),
}

/// `t` as a number written canonically: `7`, never `07` or `+7`.
fn canonical(t: &str) -> Option<u32> {
    t.parse::<u32>().ok().filter(|i| i.to_string() == t)
}

impl CopyKey {
    /// The key of the copy at sketch point `id`.
    pub fn point(id: &Id) -> CopyKey {
        match canonical(id.as_str()) {
            Some(i) => CopyKey::Index([i, 0]),
            None => CopyKey::Point(id.clone()),
        }
    }

    /// The step's text after the pattern's ID: `2`, `2:1` or `p7`.
    pub fn text(&self) -> String {
        match self {
            CopyKey::Index([i, 0]) => i.to_string(),
            CopyKey::Index([i, j]) => format!("{i}:{j}"),
            CopyKey::Point(p) => p.to_string(),
        }
    }

    /// As errors and reports give it: `[i, j]`, or the point's ID.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            CopyKey::Index(i) => serde_json::json!(i),
            CopyKey::Point(p) => serde_json::json!(p),
        }
    }

    /// As copies' hashes take it: `[i, j]` as they always have, or the
    /// point's ID.
    pub fn hash_text(&self) -> String {
        match self {
            CopyKey::Index(i) => format!("{i:?}"),
            CopyKey::Point(p) => p.to_string(),
        }
    }

    /// Its index, for a key that is one.
    pub fn index(&self) -> Option<[u32; 2]> {
        match self {
            CopyKey::Index(i) => Some(*i),
            CopyKey::Point(_) => None,
        }
    }
}

/// One step of an instance path as text: an instance's ID, or a copy
/// `<pattern>:<key>` ([`CopyKey::text`]).
pub fn step_text(id: &Id, copy: Option<&CopyKey>) -> String {
    match copy {
        None => id.to_string(),
        Some(k) => format!("{id}:{}", k.text()),
    }
}

/// Reads one step of an instance path: an ID, or a copy's.
pub fn parse_step(s: &str, at: &str) -> Result<(Id, Option<CopyKey>)> {
    let mut parts = s.split(':');
    let head = parts.next().unwrap_or_default();
    let id = Id::parse(head, "instance").map_err(|e| e.with("pointer", at))?;
    let rest: Vec<&str> = parts.collect();
    let copy = match rest.as_slice() {
        [] => return Ok((id, None)),
        [i] => match canonical(i) {
            Some(i) => Some(CopyKey::Index([i, 0])),
            None => Id::parse(i, "point").ok().map(CopyKey::Point),
        },
        [i, j] => canonical(i)
            .zip(canonical(j))
            .filter(|(_, j)| *j != 0)
            .map(|(i, j)| CopyKey::Index([i, j])),
        _ => None,
    };
    match copy {
        Some(CopyKey::Index([0, 0])) => Err(invalid_step(
            at,
            "copy 0 of a pattern is its seed: name the seed",
        )),
        Some(c) => Ok((id, Some(c))),
        None => Err(invalid_step(
            at,
            "a pattern's copy is <pattern>:<i>, <pattern>:<i>:<j> past its first row, or <pattern>:<point>",
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
    fn copy_steps_are_indices_or_points() {
        let id = |s: &str| Id::parse(s, "id").unwrap();
        for (text, key) in [
            ("ring:2", CopyKey::Index([2, 0])),
            ("ring:2:1", CopyKey::Index([2, 1])),
            ("ring:p7", CopyKey::Point(id("p7"))),
            ("ring:07", CopyKey::Point(id("07"))),
        ] {
            let (p, k) = parse_step(text, "").unwrap();
            assert_eq!((p.as_str(), k.as_ref()), ("ring", Some(&key)), "{text}");
            assert_eq!(step_text(&p, Some(&key)), text);
            assert_eq!(Id::copy(&p, &key).copy_of(), Some((p.clone(), key)));
        }
        // A point named by a canonical number is that index.
        assert_eq!(CopyKey::point(&id("3")), CopyKey::Index([3, 0]));
        assert_eq!(CopyKey::point(&id("p3")), CopyKey::Point(id("p3")));
        for bad in [
            "ring:0",
            "ring:0:0",
            "ring:2:0",
            "ring:+1",
            "ring:P7",
            "ring:1:2:3",
        ] {
            assert!(parse_step(bad, "/x").is_err(), "{bad}");
        }
        assert_eq!(CopyKey::Index([2, 1]).hash_text(), "[2, 1]");
        assert_eq!(CopyKey::Point(id("p7")).to_json(), serde_json::json!("p7"));
        assert_eq!(CopyKey::Index([2, 0]).to_json(), serde_json::json!([2, 0]));
    }

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
        let p: ProjectId = serde_json::from_value(serde_json::json!("0".repeat(32))).unwrap();
        assert_eq!(p.as_str(), "0".repeat(32));
        assert!(serde_json::from_value::<ProjectId>(serde_json::json!("x")).is_err());
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

#[cfg(test)]
mod solid_key_tests {
    use super::*;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    #[test]
    fn a_solid_key_is_a_founder_or_an_inserts_path() {
        // A founder's key is its ID, as it always was.
        let k = Id::solid_key("board", "feature").unwrap();
        assert_eq!(k, id("board"));
        assert!(!k.is_path());
        assert_eq!(k.founder_str(), "board");
        assert_eq!(k.rest(), None);
        // An inserted solid's: the insert, then the source's own key.
        let k = Id::solid_key("board/u1", "feature").unwrap();
        assert!(k.is_path());
        assert_eq!(k.founder_str(), "board");
        assert_eq!(k.founder(), id("board"));
        assert_eq!(k.rest().unwrap().as_str(), "u1");
        assert_eq!(Id::solid_path(&id("board"), &id("u1")), k);
        // Nested inserts: a/b/c is insert a's solid b/c.
        let k = Id::solid_key("a/b/c", "feature").unwrap();
        assert_eq!(k.rest().unwrap().as_str(), "b/c");
        assert_eq!(Id::solid_path(&id("a"), &k.rest().unwrap()), k);
        // Every segment is an ID; Id::parse still refuses a path.
        for bad in ["", "/", "a/", "/a", "a//b", "a/B", "a/con", "A"] {
            let e = Id::solid_key(bad, "feature").unwrap_err();
            assert_eq!(e.code, Code::InvalidId, "{bad:?}");
        }
        assert_eq!(
            Id::parse("a/b", "feature").unwrap_err().code,
            Code::InvalidId
        );
        // A key borrows as its text, so maps keyed by Id answer &str.
        let m: std::collections::BTreeSet<Id> = [id("board")].into();
        assert!(m.contains("board"));
    }
}
