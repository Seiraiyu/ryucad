//! Entity references: one text form for a face, edge or vertex everywhere
//! (selections, highlights, renders, the app's "copy reference"):
//! `<part>/<feature>/<name>`, with `<name>` the 32 hex digits of the
//! entity's stable name, as `inspect` prints it. In a sketch, which has no
//! body, the third part is a sketch entity's ID: `<part>/<sketch>/<entity>`.
//!
//! In an assembly an entity is reached through the instances that place
//! it: `<assembly>/<instance>[/<instance>…]/<solid>/<name>`, and a whole
//! occurrence (or every occurrence a subassembly's instance places) is
//! `<assembly>/<instance>[/<instance>…]`.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde::{Serialize, Serializer};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityRef {
    pub part: Id,
    /// In an assembly, the instances that reach the entity, from the
    /// assembly's own; empty in a part.
    pub path: Vec<Id>,
    /// The solid's founder, or the sketch; for a whole occurrence, the
    /// last instance of the path.
    pub feature: Id,
    /// 32 lowercase hex digits, or a sketch entity's ID; empty for a whole
    /// occurrence.
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

/// What a reference resolved to: an entity of a feature's body, a
/// sketch's entity (`point`, `line`, `arc` or `circle`), or occurrences of
/// an assembly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Body(EntityKind),
    Sketch(&'static str),
    Occurrence,
}

impl Target {
    /// `face`, `edge` or `vertex`, the sketch entity's type, or
    /// `occurrence`.
    pub fn as_str(self) -> &'static str {
        match self {
            Target::Body(k) => k.as_str(),
            Target::Sketch(t) => t,
            Target::Occurrence => "occurrence",
        }
    }
}

/// Whether `s` is a stable name as references write it: 32 lowercase hex
/// digits.
fn is_name(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl EntityRef {
    /// Reads a reference. `<a>/<b>/<c>` is read as a part's entity; in an
    /// assembly, [`EntityRef::resolve`] reads it as the occurrence
    /// `b/c` instead ([`EntityRef::in_assembly`]).
    pub fn parse(s: &str) -> Result<EntityRef> {
        let bad = |why: &str| {
            Error::new(
                Code::InvalidParam,
                format!(
                    "bad entity reference {s:?}: {why}; use <part>/<feature>/<name>, \
                     <part>/<sketch>/<entity>, <assembly>/<instance>/…/<solid>/<name> \
                     or <assembly>/<instance>/…"
                ),
            )
            .with("entity", s)
        };
        let segments: Vec<&str> = s.split('/').collect();
        if segments.len() < 2 {
            return Err(bad("expected at least two parts separated by /"));
        }
        let part = Id::parse(segments[0], "part")?;
        let ids = |xs: &[&str]| -> Result<Vec<Id>> {
            xs.iter().map(|x| Id::parse(x, "instance")).collect()
        };
        let rest = &segments[1..];
        match rest {
            // An occurrence: an instance of an assembly.
            [one] => Ok(EntityRef {
                part,
                path: ids(rest)?,
                feature: Id::parse(one, "instance")?,
                name: String::new(),
            }),
            // A part's entity (or, in an assembly, an occurrence two deep).
            [feature, name] => {
                // A stable name's hex digits are a valid ID too: which it
                // is depends on whether the feature is a sketch.
                if Id::parse(name, "entity").is_err() {
                    return Err(bad(
                        "the name is 32 lowercase hex digits, or a sketch entity's ID",
                    ));
                }
                Ok(EntityRef {
                    part,
                    path: Vec::new(),
                    feature: Id::parse(feature, "feature")?,
                    name: (*name).to_owned(),
                })
            }
            // An entity of an occurrence, or a deeper occurrence.
            [path @ .., solid, name] if is_name(name) => Ok(EntityRef {
                part,
                path: ids(path)?,
                feature: Id::parse(solid, "feature")?,
                name: (*name).to_owned(),
            }),
            _ => Ok(EntityRef {
                part,
                path: ids(rest)?,
                feature: Id::parse(rest[rest.len() - 1], "instance")?,
                name: String::new(),
            }),
        }
    }

    /// Whether it names a whole occurrence rather than an entity.
    pub fn is_occurrence(&self) -> bool {
        self.name.is_empty()
    }

    /// The reference as an assembly reads it: `<a>/<b>/<c>` is the
    /// occurrence `b/c`, since an entity in an assembly is always reached
    /// through an instance and a solid.
    pub fn in_assembly(&self) -> EntityRef {
        if !self.path.is_empty() || self.is_occurrence() {
            return self.clone();
        }
        match Id::parse(&self.name, "instance") {
            Ok(last) => EntityRef {
                part: self.part.clone(),
                path: vec![self.feature.clone(), last.clone()],
                feature: last,
                name: String::new(),
            },
            Err(_) => self.clone(),
        }
    }

    /// What the reference names in the current rebuild, or `not_found` with
    /// how far it resolved. Never a nearest match.
    pub fn resolve(&self, session: &mut Session) -> Result<Target> {
        let missing = |what: &str| {
            Error::new(Code::NotFound, format!("{self} doesn't resolve: {what}"))
                .with("entity", self.to_string())
        };
        let Some(p) = session.project().parts.get(&self.part) else {
            return Err(missing("no such part"));
        };
        if crate::server::part_kind(p) == crate::doc::files::PartKind::Assembly {
            return self.in_assembly().resolve_in_assembly(session);
        }
        if !self.path.is_empty() || self.is_occurrence() {
            return Err(
                missing("the part is no assembly, so nothing in it is placed")
                    .with("part", &self.part),
            );
        }
        let solids = crate::solid::Solids::of(p);
        let built = session.rebuilt(&self.part)?;
        let Some((_, feature)) = built.features.iter().find(|(id, _)| *id == self.feature) else {
            return Err(missing("no such feature").with("part", &self.part));
        };
        if let Ok(b) = feature
            && let Some(sk) = b.sketch()
        {
            let e = Id::parse(&self.name, "entity")
                .ok()
                .and_then(|id| sk.sketch.entities.get(&id));
            return match e {
                Some(e) => Ok(Target::Sketch(e.geometry.type_name())),
                None => Err(missing("no such entity in the sketch")
                    .with("part", &self.part)
                    .with("feature", &self.feature)),
            };
        }
        // A face, edge or vertex is named by its solid's founder, and found
        // in the solid as the model shows it.
        let shown = built.shown(&solids);
        let Some(shown) = shown.iter().find(|s| *s.solid == self.feature) else {
            let what = match solids.consumer(&self.feature) {
                Some(by) => format!("the boolean {by} consumed its solid"),
                None => "the feature founds no solid; name a solid by the feature that founded it"
                    .to_owned(),
            };
            return Err(missing(&what)
                .with("part", &self.part)
                .with("feature", &self.feature)
                .with("solids", solids.solids_of(&self.feature)));
        };
        let Some((_, b)) = shown.state else {
            return Err(missing("the feature failed to rebuild")
                .with("part", &self.part)
                .with("feature", &self.feature));
        };
        if let Some(k) = kind_of(&b.body, &self.name) {
            return Ok(Target::Body(k));
        }
        Err(missing("no such entity in the feature's body")
            .with("part", &self.part)
            .with("feature", &self.feature))
    }
}

impl EntityRef {
    /// [`EntityRef::resolve`] in an assembly: the occurrences whose path
    /// starts with the reference's, or the entity of the one occurrence at
    /// its path.
    fn resolve_in_assembly(&self, session: &mut Session) -> Result<Target> {
        let missing = |what: &str| {
            Error::new(Code::NotFound, format!("{self} doesn't resolve: {what}"))
                .with("entity", self.to_string())
                .with("part", &self.part)
        };
        let model = session.model(&self.part, &crate::doc::assembly::Rev::Working)?;
        if self.is_occurrence() {
            return if model
                .occurrences
                .iter()
                .any(|o| o.path.starts_with(&self.path))
            {
                Ok(Target::Occurrence)
            } else {
                Err(missing("no instance at that path places anything"))
            };
        }
        let Some(o) = model.occurrences.iter().find(|o| o.path == self.path) else {
            return Err(missing("no occurrence at that path"));
        };
        let Some(solid) = o.solids.iter().find(|s| s.founder == self.feature) else {
            return Err(missing("its part shows no such solid").with(
                "solids",
                o.solids.iter().map(|s| &s.founder).collect::<Vec<_>>(),
            ));
        };
        match kind_of(&solid.built.body, &self.name) {
            Some(k) => Ok(Target::Body(k)),
            None => Err(missing("no such entity in the solid")),
        }
    }
}

/// What `name` names in `body`, if anything.
pub fn kind_of(body: &ryukei::topo::Body, name: &str) -> Option<EntityKind> {
    let hit = |n: String| n == name;
    if body
        .faces()
        .any(|f| hit(body.face(f).expect("live").name.to_string()))
    {
        return Some(EntityKind::Face);
    }
    if body
        .edges()
        .any(|e| hit(body.edge(e).expect("live").name.to_string()))
    {
        return Some(EntityKind::Edge);
    }
    if body
        .vertices()
        .any(|v| hit(body.vertex(v).expect("live").name.to_string()))
    {
        return Some(EntityKind::Vertex);
    }
    None
}

impl fmt::Display for EntityRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.part)?;
        for i in &self.path {
            write!(f, "/{i}")?;
        }
        if !self.is_occurrence() {
            write!(f, "/{}/{}", self.feature, self.name)?;
        }
        Ok(())
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
    let (unresolved, errors): (Vec<String>, Vec<Error>) = parsed
        .iter()
        .filter_map(|r| Some((r.to_string(), r.resolve(session).err()?)))
        .unzip();
    if !unresolved.is_empty() {
        return Err(Error::new(
            Code::NotFound,
            format!("{} reference(s) don't resolve", unresolved.len()),
        )
        .with("unresolved", unresolved)
        .with("errors", errors));
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
        // A sketch entity's ID.
        assert_eq!(EntityRef::parse("p/s/l1").unwrap().name, "l1");
        // In an assembly: occurrences, and entities reached through them.
        for (text, path, occurrence) in [
            ("a/i1".to_owned(), vec!["i1"], true),
            ("a/i1/i2/i3".to_owned(), vec!["i1", "i2", "i3"], true),
            (format!("a/i1/s/{NAME}"), vec!["i1"], false),
            (format!("a/i1/i2/s/{NAME}"), vec!["i1", "i2"], false),
        ] {
            let r = EntityRef::parse(&text).unwrap();
            assert_eq!(
                r.path.iter().map(Id::as_str).collect::<Vec<_>>(),
                path,
                "{text}"
            );
            assert_eq!(r.is_occurrence(), occurrence, "{text}");
            assert_eq!(r.to_string(), text);
        }
        // Three parts read as a part's entity, or in an assembly as an
        // occurrence two deep.
        let r = EntityRef::parse("a/i1/i2").unwrap();
        assert!(r.path.is_empty());
        let r = r.in_assembly();
        assert_eq!((r.path.len(), r.is_occurrence()), (2, true));
        assert_eq!(r.to_string(), "a/i1/i2");
        for bad in ["p", &format!("p/b/{}", NAME.to_uppercase()), "p/b/-x"] {
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
            assert_eq!(r.resolve(&mut s).unwrap(), Target::Body(kind));
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

    #[test]
    fn assemblies_resolve_occurrences_and_their_entities() {
        let (dir, mut s) = crate::assembly::tests::project();
        let r = dir.path();
        call(
            &mut s,
            "part.create",
            json!({"part": "pair", "kind": "assembly"}),
        )
        .unwrap();
        crate::assembly::tests::place_in(r, "pair", "left", "bolt", json!("working"), [0.0; 3]);
        crate::assembly::tests::place_in(
            r,
            "frame",
            "p1",
            "pair",
            json!("working"),
            [0.05, 0.0, 0.0],
        );
        crate::assembly::tests::place_in(r, "frame", "b1", "bolt", json!("working"), [0.0; 3]);
        s.refresh().unwrap();
        let body = call(&mut s, "inspect", json!({"part": "bolt"})).unwrap()["bodies"][0].clone();
        let face = body["faces"][0]["name"].as_str().unwrap().to_owned();
        let resolve = |s: &mut Session, text: &str| EntityRef::parse(text).unwrap().resolve(s);
        assert_eq!(resolve(&mut s, "frame/b1").unwrap(), Target::Occurrence);
        assert_eq!(resolve(&mut s, "frame/p1").unwrap(), Target::Occurrence);
        assert_eq!(
            resolve(&mut s, "frame/p1/left").unwrap(),
            Target::Occurrence
        );
        assert_eq!(
            resolve(&mut s, &format!("frame/b1/shank/{face}")).unwrap(),
            Target::Body(EntityKind::Face)
        );
        assert_eq!(
            resolve(&mut s, &format!("frame/p1/left/shank/{face}")).unwrap(),
            Target::Body(EntityKind::Face)
        );
        for missing in [
            "frame/b9".to_owned(),
            "frame/p1/right".to_owned(),
            format!("frame/b1/slab/{face}"),
            format!("frame/b1/shank/{NAME}"),
            format!("frame/p1/shank/{face}"),
            "bolt/b1".to_owned(),
        ] {
            assert_eq!(
                resolve(&mut s, &missing).unwrap_err().code,
                Code::NotFound,
                "{missing}"
            );
        }
        // Framing an occurrence frames it where the assembly puts it.
        let pts =
            crate::server::model_points(&mut s, &[EntityRef::parse("frame/p1/left").unwrap()])
                .unwrap();
        assert!(pts.iter().all(|p| (p[0] - 0.05).abs() <= 0.0031));
        // A selection takes them.
        let mut live = crate::live::Live::new();
        let v = crate::server::tests::call_live(
            &mut s,
            &mut live,
            None,
            "selection.set",
            json!({"entities": ["frame/p1", format!("frame/b1/shank/{face}")]}),
        )
        .unwrap();
        assert_eq!(v["selection"][0], "frame/p1");
    }
}
