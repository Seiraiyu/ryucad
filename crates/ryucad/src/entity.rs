//! Entity references: one text form for a face, edge or vertex everywhere
//! (selections, highlights, renders, the app's "copy reference"):
//! `<part>/<feature>/<name>`, with `<name>` the 32 hex digits of the
//! entity's stable name, as `inspect` prints it.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde::{Serialize, Serializer};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityRef {
    pub part: Id,
    pub feature: Id,
    /// 32 lowercase hex digits.
    pub name: String,
}

/// What kind of entity a reference resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Face,
    Edge,
    Vertex,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntityKind::Face => "face",
            EntityKind::Edge => "edge",
            EntityKind::Vertex => "vertex",
        }
    }
}

impl EntityRef {
    pub fn parse(s: &str) -> Result<EntityRef> {
        let bad = |why: &str| {
            Error::new(
                Code::InvalidParam,
                format!("bad entity reference {s:?}: {why}; use <part>/<feature>/<name>"),
            )
            .with("entity", s)
        };
        let mut it = s.split('/');
        let (Some(part), Some(feature), Some(name), None) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            return Err(bad("expected three parts separated by /"));
        };
        let hex = name.len() == 32
            && name
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
        if !hex {
            return Err(bad("the name is 32 lowercase hex digits"));
        }
        Ok(EntityRef {
            part: Id::parse(part, "part")?,
            feature: Id::parse(feature, "feature")?,
            name: name.to_owned(),
        })
    }

    /// What the reference names in the current rebuild, or `not_found` with
    /// how far it resolved. Never a nearest match.
    pub fn resolve(&self, session: &mut Session) -> Result<EntityKind> {
        let missing = |what: &str| {
            Error::new(Code::NotFound, format!("{self} doesn't resolve: {what}"))
                .with("entity", self.to_string())
        };
        if !session.project().parts.contains_key(&self.part) {
            return Err(missing("no such part"));
        }
        let built = session.rebuilt(&self.part)?;
        let Some((_, feature)) = built.features.iter().find(|(id, _)| *id == self.feature) else {
            return Err(missing("no such feature").with("part", &self.part));
        };
        let Ok(b) = feature else {
            return Err(missing("the feature failed to rebuild")
                .with("part", &self.part)
                .with("feature", &self.feature));
        };
        let Some(b) = b.body() else {
            return Err(missing("the feature is a sketch, which has no body")
                .with("part", &self.part)
                .with("feature", &self.feature));
        };
        let body = &b.body;
        let hit = |n: String| n == self.name;
        if body
            .faces()
            .any(|f| hit(body.face(f).expect("live").name.to_string()))
        {
            return Ok(EntityKind::Face);
        }
        if body
            .edges()
            .any(|e| hit(body.edge(e).expect("live").name.to_string()))
        {
            return Ok(EntityKind::Edge);
        }
        if body
            .vertices()
            .any(|v| hit(body.vertex(v).expect("live").name.to_string()))
        {
            return Ok(EntityKind::Vertex);
        }
        Err(missing("no such entity in the feature's body")
            .with("part", &self.part)
            .with("feature", &self.feature))
    }
}

impl fmt::Display for EntityRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.part, self.feature, self.name)
    }
}

impl Serialize for EntityRef {
    fn serialize<S: Serializer>(&self, s: S) -> core::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// Parses every reference, then checks that each resolves: the error lists
/// every one that doesn't.
pub fn parse_all(session: &mut Session, refs: &[String]) -> Result<Vec<EntityRef>> {
    let parsed = refs
        .iter()
        .map(|s| EntityRef::parse(s))
        .collect::<Result<Vec<_>>>()?;
    let unresolved: Vec<String> = parsed
        .iter()
        .filter(|r| r.resolve(session).is_err())
        .map(ToString::to_string)
        .collect();
    if !unresolved.is_empty() {
        return Err(Error::new(
            Code::NotFound,
            format!("{} reference(s) don't resolve", unresolved.len()),
        )
        .with("unresolved", unresolved));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    const NAME: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn parses_and_prints() {
        let r = EntityRef::parse(&format!("p/b/{NAME}")).unwrap();
        assert_eq!((r.part.as_str(), r.feature.as_str()), ("p", "b"));
        assert_eq!(r.to_string(), format!("p/b/{NAME}"));
        assert_eq!(json!(r), json!(format!("p/b/{NAME}")));
        for bad in [
            "p/b",
            "p/b/c/d",
            &format!("p/b/{}", NAME.to_uppercase()),
            &format!("p/b/{}", &NAME[1..]),
        ] {
            assert_eq!(
                EntityRef::parse(bad).unwrap_err().code,
                Code::InvalidParam,
                "{bad}"
            );
        }
        let e = EntityRef::parse(&format!("P/b/{NAME}")).unwrap_err();
        assert_eq!(e.code, Code::InvalidId);
    }

    #[test]
    fn resolves_exactly_or_not_at_all() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 1, "y": 2, "z": 3}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "p"})).unwrap()["bodies"][0].clone();
        for (kind, key) in [
            (EntityKind::Face, "faces"),
            (EntityKind::Edge, "edges"),
            (EntityKind::Vertex, "vertices"),
        ] {
            let name = body[key][0]["name"].as_str().unwrap();
            let r = EntityRef::parse(&format!("p/b/{name}")).unwrap();
            assert_eq!(r.resolve(&mut s).unwrap(), kind);
        }
        for missing in [
            format!("q/b/{NAME}"),
            format!("p/c/{NAME}"),
            format!("p/b/{NAME}"),
        ] {
            let e = EntityRef::parse(&missing)
                .unwrap()
                .resolve(&mut s)
                .unwrap_err();
            assert_eq!(e.code, Code::NotFound, "{missing}");
        }
        let face = format!("p/b/{}", body["faces"][0]["name"].as_str().unwrap());
        let e = parse_all(&mut s, &[face.clone(), format!("p/b/{NAME}")]).unwrap_err();
        assert_eq!(e.details["unresolved"], json!([format!("p/b/{NAME}")]));
        assert_eq!(parse_all(&mut s, &[face]).unwrap().len(), 1);
    }
}
