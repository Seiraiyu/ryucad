//! References to a face, edge or vertex of an earlier feature of the same
//! part, by stable name. In a file a reference is `{"feature", "name"}`;
//! in requests and output it is the entity reference text
//! `<part>/<feature>/<name>`, as `inspect` and selections give it.
//!
//! A reference stores nothing else: no position, no signature. If its name
//! is gone, the feature that holds it fails; it never lands on whatever
//! is nearest.

use super::files::{Obj, invalid};
use super::id::Id;
use crate::error::{Code, Error, Result};
use ryukei::name::Name;
use serde_json::{Value, json};

/// A face, edge or vertex of `feature`'s body.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reference {
    pub feature: Id,
    pub name: Name,
}

/// A stable name from its 32 lowercase hex digits.
pub fn parse_name(s: &str) -> Option<Name> {
    let hex = s.len() == 32 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
    hex.then(|| u128::from_str_radix(s, 16).ok().map(Name::from_u128))
        .flatten()
}

impl Reference {
    /// `{feature, name}`, or for an inserted solid's entity `{feature:
    /// <insert>, solid: <source key>, name}` (design 521).
    pub fn to_json(&self) -> Value {
        match self.feature.rest() {
            None => json!({"feature": self.feature, "name": self.name.to_string()}),
            Some(solid) => json!({"feature": self.feature.founder(), "solid": solid,
                                  "name": self.name.to_string()}),
        }
    }

    /// Reads `{"feature", "name"}`; `at` is its JSON pointer for errors.
    pub fn from_json(v: &Value, at: &str) -> Result<Reference> {
        let mut o = Obj::new(v, at)?;
        let f = o.take("feature")?;
        let n = o.take("name")?;
        let s = o.take_opt("solid");
        o.finish()?;
        let f = f
            .as_str()
            .ok_or_else(|| invalid(&format!("{at}/feature"), "feature is a feature's ID"))?;
        let feature =
            Id::parse(f, "feature").map_err(|e| e.with("pointer", format!("{at}/feature")))?;
        let name = n.as_str().and_then(parse_name).ok_or_else(|| {
            invalid(
                &format!("{at}/name"),
                "name is a stable name: 32 lowercase hex digits",
            )
        })?;
        let feature = match s {
            None => feature,
            Some(s) => {
                let rest = super::solid::SolidRef::from_json(&s, &format!("{at}/solid"))?;
                Id::solid_path(&feature, &rest)
            }
        };
        Ok(Reference { feature, name })
    }

    /// Reads the text form `<part>/<feature>/<name>`, or an inserted
    /// solid's `<part>/<insert>/<source solid>/<name>` (the last segment is
    /// always the name), which must name `part`: a feature refers only
    /// within its own part.
    pub fn parse(s: &str, part: &Id) -> Result<Reference> {
        let bad = |why: &str| {
            Error::new(
                Code::InvalidParam,
                format!("bad reference {s:?}: {why}; use <part>/<feature>/<name> from inspect"),
            )
            .with("reference", s)
        };
        let segments: Vec<&str> = s.split('/').collect();
        let [p, key @ .., n] = segments.as_slice() else {
            return Err(bad("expected three parts separated by /"));
        };
        if key.is_empty() {
            return Err(bad("expected three parts separated by /"));
        }
        if *p != part.as_str() {
            return Err(bad(&format!(
                "a feature of part {part} refers only within it"
            )));
        }
        let feature = Id::solid_key(&key.join("/"), "feature")?;
        let name = parse_name(n).ok_or_else(|| bad("the name is 32 lowercase hex digits"))?;
        Ok(Reference { feature, name })
    }

    /// The text form, in `part`.
    pub fn text(&self, part: &Id) -> String {
        format!("{part}/{}/{}", self.feature, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "0123456789abcdef0123456789abcdef";

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    #[test]
    fn file_and_text_forms() {
        let r = Reference::parse(&format!("p/base/{NAME}"), &id("p")).unwrap();
        assert_eq!(r.feature, id("base"));
        assert_eq!(r.name.to_string(), NAME);
        assert_eq!(r.text(&id("p")), format!("p/base/{NAME}"));
        let v = r.to_json();
        assert_eq!(v, json!({"feature": "base", "name": NAME}));
        assert_eq!(Reference::from_json(&v, "/face").unwrap(), r);
    }

    #[test]
    fn an_inserted_solids_reference_names_its_solid() {
        let r = Reference::parse(&format!("p/board/u1/{NAME}"), &id("p")).unwrap();
        assert_eq!(r.feature.as_str(), "board/u1");
        assert_eq!(r.text(&id("p")), format!("p/board/u1/{NAME}"));
        let v = r.to_json();
        assert_eq!(v, json!({"feature": "board", "solid": "u1", "name": NAME}));
        assert_eq!(Reference::from_json(&v, "/face").unwrap(), r);
        // Nested: a/b/c is insert a's solid b/c.
        let r = Reference::parse(&format!("p/a/b/c/{NAME}"), &id("p")).unwrap();
        assert_eq!(
            r.to_json(),
            json!({"feature": "a", "solid": "b/c", "name": NAME})
        );
        // A founder's reference is written as it always was.
        let r = Reference::parse(&format!("p/base/{NAME}"), &id("p")).unwrap();
        assert_eq!(r.to_json(), json!({"feature": "base", "name": NAME}));
        for (v, pointer) in [
            (
                json!({"feature": "board", "solid": 3, "name": NAME}),
                "/face/solid",
            ),
            (
                json!({"feature": "board", "solid": "U1", "name": NAME}),
                "/face/solid",
            ),
            (
                json!({"feature": "board/u1", "name": NAME}),
                "/face/feature",
            ),
        ] {
            let e = Reference::from_json(&v, "/face").unwrap_err();
            assert_eq!(e.details["pointer"], pointer, "{v}");
        }
    }

    #[test]
    fn bad_references() {
        for (s, code) in [
            (format!("q/base/{NAME}"), Code::InvalidParam),
            (
                format!("p/base/{}", NAME.to_uppercase()),
                Code::InvalidParam,
            ),
            ("p/base/l1".to_owned(), Code::InvalidParam),
            ("p/base".to_owned(), Code::InvalidParam),
            (format!("p/Base/{NAME}"), Code::InvalidId),
        ] {
            assert_eq!(
                Reference::parse(&s, &id("p")).unwrap_err().code,
                code,
                "{s}"
            );
        }
        for (v, pointer) in [
            (json!({"feature": "base"}), "/face/name"),
            (json!({"feature": "base", "name": "abc"}), "/face/name"),
            (json!({"feature": 3, "name": NAME}), "/face/feature"),
            (json!({"feature": "base", "name": NAME, "x": 1}), "/face/x"),
            (json!("base"), "/face"),
        ] {
            let e = Reference::from_json(&v, "/face").unwrap_err();
            assert_eq!(e.details["pointer"], pointer, "{v}");
        }
    }
}
