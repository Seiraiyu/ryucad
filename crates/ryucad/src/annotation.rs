//! Annotations (design 374): a person's requests to the agent, kept in
//! `annotations/` (see `doc::annotation` for the file), with their
//! derived states, their proposed changes (see [`propose`]) and the
//! requests that list, make, answer and close them. Nothing here acts on
//! an annotation by itself: every change is a request someone sent.

use crate::activity::Files;
use crate::doc::annotation::check_id;
pub use crate::doc::annotation::{
    Annotation, AuthorKind, By, Change, Pin, Reply, Resolution, Status,
};
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Set, annotation IDs are a fixed sequence from it: the same requests
/// give the same IDs in any process, in a daemon or not (tests).
pub const ID_SEED_VAR: &str = "RYUCAD_ID_SEED";

/// Set to a time (`YYYY-MM-DDTHH:MM:SSZ`), what the clock reads (tests).
pub const NOW_VAR: &str = "RYUCAD_NOW";

const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// A new annotation ID that `taken` doesn't refuse.
pub fn new_id(taken: impl Fn(&str) -> bool) -> Result<Id> {
    new_id_from(std::env::var(ID_SEED_VAR).ok().as_deref(), taken)
}

/// [`new_id`] with the seed given: 50 bits, random or the seed's
/// sequence (BLAKE3 of `ryucad.annotation.id.1`, the seed and a counter),
/// as 10 base32 characters, as ryueda's design IDs are.
pub fn new_id_from(seed: Option<&str>, taken: impl Fn(&str) -> bool) -> Result<Id> {
    for k in 0u64.. {
        let bits = match seed {
            Some(seed) => {
                let mut h = blake3::Hasher::new();
                h.update(b"ryucad.annotation.id.1\0");
                h.update(seed.as_bytes());
                h.update(&k.to_le_bytes());
                u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().expect("8 bytes"))
            }
            None => {
                let mut b = [0u8; 8];
                getrandom::fill(&mut b)
                    .map_err(|e| Error::new(Code::Internal, format!("no random source: {e}")))?;
                u64::from_le_bytes(b)
            }
        };
        let bits = bits & ((1 << 50) - 1);
        let text: String = (0..10)
            .map(|i| BASE32[((bits >> (45 - 5 * i)) & 31) as usize] as char)
            .collect();
        if !taken(&text) {
            return check_id(&text);
        }
    }
    unreachable!("some ID is free")
}

/// Now, UTC to the second: `YYYY-MM-DDTHH:MM:SSZ`, or `RYUCAD_NOW`.
pub fn now() -> String {
    now_from(std::env::var(NOW_VAR).ok().as_deref())
}

pub fn now_from(fixed: Option<&str>) -> String {
    if let Some(t) = fixed.filter(|t| crate::doc::annotation::is_time(t)) {
        return t.to_owned();
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    format!("{}Z", crate::export::utc(secs))
}

/// Every annotation the files hold, by ID: each read, or why it doesn't.
/// A file whose name isn't an annotation's path is doctor's
/// (`unknown_file`) and not listed.
pub fn read_all(files: &Files) -> BTreeMap<Id, Result<Annotation>> {
    let prefix = format!("{}/", crate::doc::annotation::ANNOTATIONS);
    files
        .range(prefix.clone()..)
        .take_while(|(p, _)| p.starts_with(&prefix))
        .filter(|(p, _)| crate::doc::annotation::is_annotation_path(p))
        .map(|(p, bytes)| {
            let id = p[prefix.len()..p.len() - ".json".len()].to_owned();
            let id = check_id(&id).expect("an annotation's path");
            let read = std::str::from_utf8(bytes)
                .map_err(|_| Error::new(Code::InvalidJson, format!("{p} isn't UTF-8")))
                .and_then(crate::doc::canonical::parse)
                .and_then(|v| Annotation::from_json(&v))
                .and_then(|a| match a.id == id {
                    true => Ok(a),
                    false => Err(Error::new(
                        Code::InvalidDocument,
                        format!("{p} holds annotation {}", a.id),
                    )
                    .with("pointer", "/id")),
                })
                .map_err(|e| e.with("file", p));
            (id, read)
        })
        .collect()
}

/// The annotations that read, by ID.
pub fn readable(all: &BTreeMap<Id, Result<Annotation>>) -> BTreeMap<Id, Annotation> {
    all.iter()
        .filter_map(|(id, a)| Some((id.clone(), a.as_ref().ok()?.clone())))
        .collect()
}

/// What an annotation is, besides what was recorded: the pins that don't
/// resolve in the working files (each `{pin, ref, code, was?,
/// removed_by?, candidates?}`), the files its change touches that aren't
/// the bytes it saw (`{path, base, now}`), and whether its change is in
/// the working files (`None` without a change).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Derived {
    pub outdated: Vec<Value>,
    pub stale: Vec<Value>,
    pub applied: Option<bool>,
}

/// A file's BLAKE3, as a change's base records it.
pub fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// The bytes a change writes to a file: its JSON in canonical form,
/// exactly what `write_json` writes; `None` for a deletion.
pub fn bytes_of(write: &Option<Value>) -> Option<Vec<u8>> {
    write
        .as_ref()
        .map(|v| crate::doc::canonical::to_string(v).into_bytes())
}

/// Whether every file `change` writes is already so in `files`.
pub fn is_in(files: &Files, change: &Change) -> bool {
    change
        .writes
        .iter()
        .all(|(p, w)| files.get(p).map(|b| b.to_vec()) == bytes_of(w))
}

/// `files` with `change`'s writes laid over them.
pub fn lay(files: &mut Files, change: &Change) {
    for (p, w) in &change.writes {
        match bytes_of(w) {
            Some(b) => files.insert(p.clone(), b.into()),
            None => files.remove(p),
        };
    }
}

/// The open annotations `a`'s change stands on whose changes aren't in
/// `files` yet, every one its `after` names at any depth, each once, in
/// the order they are laid (each after those it stands on).
pub fn chain<'a>(
    all: &'a BTreeMap<Id, Annotation>,
    a: &Annotation,
    files: &Files,
) -> Vec<&'a Annotation> {
    let mut out: Vec<&Annotation> = Vec::new();
    let mut seen = BTreeSet::new();
    fn visit<'a>(
        all: &'a BTreeMap<Id, Annotation>,
        id: &Id,
        files: &Files,
        seen: &mut BTreeSet<Id>,
        out: &mut Vec<&'a Annotation>,
    ) {
        if !seen.insert(id.clone()) {
            return;
        }
        let Some(x) = all.get(id) else { return };
        let Some(c) = &x.change else { return };
        if !x.status.is_open() || is_in(files, c) {
            return;
        }
        for before in &c.after {
            visit(all, before, files, seen, out);
        }
        out.push(x);
    }
    if let Some(c) = &a.change {
        for before in &c.after {
            visit(all, before, files, &mut seen, &mut out);
        }
    }
    out
}

/// `a`'s derived states against the working files.
pub fn derive(
    session: &mut Session,
    all: &BTreeMap<Id, Result<Annotation>>,
    a: &Annotation,
) -> Derived {
    let mut d = Derived::default();
    for (i, pin) in a.pins.iter().enumerate() {
        let fault = match pin {
            Pin::Entity { reference, .. } => crate::entity::EntityRef::parse(reference)
                .and_then(|r| r.resolve(session).map(drop))
                .err(),
            Pin::Point { part, .. } => session.project().part(part).err(),
        };
        if let Some(e) = fault {
            let mut v = json!({"pin": i, "ref": pin_text(pin), "code": e.code});
            for key in ["was", "removed_by", "candidates"] {
                if let Some(x) = e.details.get(key) {
                    v[key] = x.clone();
                }
            }
            d.outdated.push(v);
        }
    }
    if let Some(c) = &a.change {
        let working = session.snapshot();
        let applied = is_in(&working, c);
        d.applied = Some(applied);
        if !applied {
            let all = readable(all);
            let mut seen = working.clone();
            for before in chain(&all, a, &working) {
                lay(&mut seen, before.change.as_ref().expect("in a chain"));
            }
            for (path, base) in &c.base {
                let now = seen.get(path).map(|b| hash(b));
                if now != *base {
                    d.stale
                        .push(json!({"path": path, "base": base, "now": now}));
                }
            }
        }
    }
    d
}

/// A pin as text: its reference, or `<part>@x,y,z`.
pub fn pin_text(pin: &Pin) -> String {
    match pin {
        Pin::Entity { reference, .. } => reference.clone(),
        Pin::Point { part, point_m } => {
            format!("{part}@{},{},{}", point_m[0], point_m[1], point_m[2])
        }
    }
}

/// An annotation as listings give it: its file's fields, and `outdated`,
/// `stale` (with `outdated_pins` and `stale_files` when true) and, with a
/// change, `applied`.
pub fn entry(
    session: &mut Session,
    all: &BTreeMap<Id, Result<Annotation>>,
    a: &Annotation,
) -> Value {
    let d = derive(session, all, a);
    let mut v = a.to_json();
    let o = v.as_object_mut().expect("an object");
    o.remove("kind");
    o.remove("format");
    o.insert("outdated".into(), json!(!d.outdated.is_empty()));
    if !d.outdated.is_empty() {
        o.insert("outdated_pins".into(), json!(d.outdated));
    }
    o.insert("stale".into(), json!(!d.stale.is_empty()));
    if !d.stale.is_empty() {
        o.insert("stale_files".into(), json!(d.stale));
    }
    if let Some(applied) = d.applied {
        o.insert("applied".into(), json!(applied));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    /// A part `plate` with a box `slab`, and one of its faces' references.
    pub(crate) fn plate() -> (tempfile::TempDir, crate::session::Session, String) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "plate"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "plate", "type": "box", "id": "slab",
                                         "params": {"x": 40, "y": 20, "z": 5}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "plate"})).unwrap()["bodies"][0].clone();
        let face = format!("plate/slab/{}", body["faces"][0]["name"].as_str().unwrap());
        (dir, s, face)
    }

    /// Writes `a` into the project as a file, as a person's window would
    /// have, and takes it in.
    pub(crate) fn put(s: &mut crate::session::Session, a: &Annotation) {
        let at = crate::doc::annotation::path(s.root(), &a.id);
        crate::doc::project::write_json(&at, &a.to_json()).unwrap();
        s.reload().unwrap();
    }

    pub(crate) fn comment(id: &str, pins: Vec<Pin>) -> Annotation {
        Annotation {
            id: crate::doc::annotation::check_id(id).unwrap(),
            by: By::agent("agent"),
            at: "2026-11-05T14:02:11Z".into(),
            status: Status::Open,
            claimed_by: None,
            pins,
            comment: "look here".into(),
            view: None,
            change: None,
            replies: Default::default(),
            resolution: None,
        }
    }

    #[test]
    fn a_pin_whose_name_is_gone_is_outdated() {
        let (_dir, mut s, face) = plate();
        let gone = "plate/slab/00000000000000000000000000000000".to_owned();
        let a = comment(
            "aaaaaaaaaa",
            vec![
                Pin::Entity {
                    reference: face,
                    point_m: None,
                },
                Pin::Entity {
                    reference: gone.clone(),
                    point_m: None,
                },
            ],
        );
        put(&mut s, &a);
        let all = read_all(&s.snapshot());
        let d = derive(&mut s, &all, &a);
        assert_eq!(d.outdated.len(), 1, "{:?}", d.outdated);
        assert_eq!(d.outdated[0]["pin"], 1);
        assert_eq!(d.outdated[0]["ref"], gone);
        assert!(d.stale.is_empty());
        assert_eq!(d.applied, None);
        let e = entry(&mut s, &all, &a);
        assert_eq!(e["outdated"], true);
        assert_eq!(e["stale"], false);
        assert!(e.get("applied").is_none());
    }

    #[test]
    fn a_change_is_stale_once_its_file_moves_and_applied_once_it_is_there() {
        let (_dir, mut s, _) = plate();
        let rel = "parts/plate/features/slab.json".to_owned();
        let now = s.snapshot()[&rel].clone();
        let mut proposed: serde_json::Value = serde_json::from_slice(&now).unwrap();
        proposed["params"]["x"] = json!("50 mm");
        let mut a = comment("bbbbbbbbbb", vec![]);
        a.change = Some(Change {
            request: json!({"method": "feature.edit", "params": {}}),
            rough: false,
            base: [(rel.clone(), Some(blake3::hash(&now).to_hex().to_string()))].into(),
            writes: [(rel.clone(), Some(proposed.clone()))].into(),
            after: vec![],
        });
        put(&mut s, &a);
        let all = read_all(&s.snapshot());
        let d = derive(&mut s, &all, &a);
        assert!(d.stale.is_empty());
        assert_eq!(d.applied, Some(false));
        // The model moves under it: stale, naming the file.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "plate", "id": "slab",
                                          "params": {"y": 30}}),
        )
        .unwrap();
        let d = derive(&mut s, &all, &a);
        assert_eq!(d.stale.len(), 1);
        assert_eq!(d.stale[0]["path"], rel);
        // The file as the change writes it: applied, so not stale.
        crate::doc::project::write_json(&s.root().join(&rel), &proposed).unwrap();
        s.reload().unwrap();
        let all = read_all(&s.snapshot());
        let d = derive(&mut s, &all, &a);
        assert_eq!(d.applied, Some(true));
        assert!(d.stale.is_empty());
    }

    #[test]
    fn files_that_dont_read_are_listed_with_their_error() {
        let (_dir, mut s, _) = plate();
        let at = s.root().join("annotations/cccccccccc.json");
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, b"{\"kind\": \"ryucad.annotation\"}\n").unwrap();
        s.reload().unwrap();
        let all = read_all(&s.snapshot());
        assert_eq!(all.len(), 1);
        assert!(all.values().next().unwrap().is_err());
    }

    #[test]
    fn ids_are_random_or_the_seeds_sequence() {
        let a = new_id_from(None, |_| false).unwrap();
        let b = new_id_from(None, |_| false).unwrap();
        assert_ne!(a, b);
        assert!(crate::doc::annotation::check_id(a.as_str()).is_ok());
        let s1 = new_id_from(Some("t"), |_| false).unwrap();
        assert_eq!(new_id_from(Some("t"), |_| false).unwrap(), s1);
        // A taken ID is passed over to the seed's next.
        let s2 = new_id_from(Some("t"), |id| id == s1.as_str()).unwrap();
        assert_ne!(s2, s1);
    }

    #[test]
    fn the_clock_is_utc_to_the_second_or_ryucad_now() {
        assert!(crate::doc::annotation::is_time(&now_from(None)));
        assert_eq!(
            now_from(Some("2026-11-05T14:02:11Z")),
            "2026-11-05T14:02:11Z"
        );
        // A value that isn't a time is ignored.
        assert!(crate::doc::annotation::is_time(&now_from(Some("soon"))));
    }
}
