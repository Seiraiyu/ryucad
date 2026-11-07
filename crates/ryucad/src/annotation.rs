//! Annotations (design 374): a person's requests to the agent, kept in
//! `annotations/` (see `doc::annotation` for the file), with their
//! derived states, their proposed changes (see [`propose`]) and the
//! requests that list, make, answer and close them. Nothing here acts on
//! an annotation by itself: every change is a request someone sent.

pub mod propose;

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

/// How an agent works through annotations: written where agents read
/// (`ryucad annotations --help`, the app's Driving ryucad from an agent
/// window), never enforced. ryucad does nothing with an annotation by
/// itself.
pub const ETIQUETTE: &str = "\
Annotations are a person's requests to you, pinned to the model; some carry a proposed change.
When you start work, run `ryucad annotations list --open` and read what is open.
Ask the person before acting on annotations they didn't ask you about.
Claim one before working it: `ryucad annotations claim <id>`.
A proposed change is a revision: `ryucad diff --to annotation:<id> --model`, `ryucad render --rev annotation:<id> --compare working`.
Take the change if you agree with it (`ryucad annotations apply <id>`), or make your own edit.
Reply saying what you did: `ryucad annotations reply <id> TEXT`.
Close it: `ryucad annotations resolve <id> --note TEXT`, or `ryucad annotations reject <id> --reason TEXT`.
Wait for the next one with `ryucad watch --topic annotations`.";

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

/// Whether `a`'s change is in `files`: every file it writes is so, or an
/// annotation whose change stands on it (its `after`, at any depth) is
/// applied, having written its files over `a`'s.
pub fn applied(all: &BTreeMap<Id, Annotation>, a: &Annotation, files: &Files) -> bool {
    fn go(
        all: &BTreeMap<Id, Annotation>,
        a: &Annotation,
        files: &Files,
        seen: &mut BTreeSet<Id>,
    ) -> bool {
        let Some(c) = &a.change else { return false };
        if !seen.insert(a.id.clone()) {
            return false;
        }
        is_in(files, c)
            || all.values().any(|x| {
                x.change.as_ref().is_some_and(|xc| xc.after.contains(&a.id))
                    && go(all, x, files, seen)
            })
    }
    go(all, a, files, &mut BTreeSet::new())
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
        if x.change.is_none() {
            return;
        }
        let c = x.change.as_ref().expect("a change");
        if !x.status.is_open() || applied(all, x, files) {
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
        let all = readable(all);
        let applied = applied(&all, a, &working);
        d.applied = Some(applied);
        if !applied {
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

/// Who is making a write: the connection's participant (its name and
/// kind), else the agent `named` (`--as` with no daemon), else `agent`.
pub fn author(live: Option<&crate::live::Live>, me: Option<&str>, named: Option<&str>) -> By {
    if let (Some(l), Some(me)) = (live, me)
        && let Ok(p) = l.get(me)
    {
        return By {
            participant: p.name.clone(),
            kind: match p.kind {
                crate::live::Kind::Human => AuthorKind::Human,
                crate::live::Kind::Agent => AuthorKind::Agent,
            },
        };
    }
    By::agent(named.unwrap_or(crate::live::DEFAULT_AGENT))
}

/// Pins as a request gives them, each checked against `session`: a
/// reference must read and resolve (its own error otherwise), a bare
/// point's part exist.
pub fn pins_from(session: &mut Session, args: &[crate::proto::PinArg]) -> Result<Vec<Pin>> {
    let unit = session.project().manifest.default_unit;
    let point = |p: &[Value; 3], at: usize| -> Result<[f64; 3]> {
        let mut out = [0.0; 3];
        for (i, v) in p.iter().enumerate() {
            out[i] = crate::units::Quantity::from_json(v, unit)
                .map_err(|e| e.with("pointer", format!("/pins/{at}/point/{i}")))?
                .meters();
        }
        Ok(out)
    };
    let mut out = Vec::new();
    for (i, a) in args.iter().enumerate() {
        out.push(match (&a.reference, &a.part, &a.point) {
            (Some(r), None, p) => {
                crate::entity::EntityRef::parse(r)?.resolve(session)?;
                Pin::Entity {
                    reference: r.clone(),
                    point_m: p.as_ref().map(|p| point(p, i)).transpose()?,
                }
            }
            (None, Some(part), Some(p)) => {
                let id = crate::server::part_id(part)?;
                session.project().part(&id)?;
                Pin::Point {
                    part: id,
                    point_m: point(p, i)?,
                }
            }
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    "a pin is a ref (with an optional point), or a part and a point",
                )
                .with("pointer", format!("/pins/{i}")));
            }
        });
    }
    Ok(out)
}

/// Writes `a` into `annotations/` through the session and takes it in.
pub fn save(session: &mut Session, a: &Annotation) -> Result<()> {
    let at = crate::doc::annotation::path(session.root(), &a.id);
    session.write_json(&at, &a.to_json())?;
    session.reload()
}

/// Every ID the files hold or take, readable or not.
pub fn taken(session: &Session) -> BTreeSet<String> {
    read_all(&session.snapshot())
        .keys()
        .map(|id| id.to_string())
        .collect()
}

/// Annotation `id` as the files hold it, checked against `expect` (its
/// file's BLAKE3): `annotation_not_found`, its file's own error, or
/// `conflict`.
pub fn one(session: &Session, id: &str, expect: Option<&str>) -> Result<Annotation> {
    let missing = || {
        Error::new(Code::AnnotationNotFound, format!("no annotation {id}")).with("annotation", id)
    };
    let id = check_id(id).map_err(|_| missing())?;
    let rel = crate::doc::annotation::rel(&id);
    let files = session.snapshot();
    let bytes = files.get(&rel).ok_or_else(missing)?;
    if let Some(e) = expect
        && hash(bytes) != e
    {
        return Err(Error::new(
            Code::Conflict,
            format!("annotation {id} changed since you read it; read it again and retry"),
        )
        .with("annotation", &id)
        .with("expected", e)
        .with("actual", hash(bytes)));
    }
    read_all(&files).remove(&id).expect("listed")
}

fn closed(a: &Annotation) -> bool {
    matches!(a.status, Status::Done | Status::Rejected)
}

/// Writes `a` and answers it as listings give it.
fn written(session: &mut Session, a: &Annotation) -> Result<Value> {
    save(session, a)?;
    let all = read_all(&session.snapshot());
    Ok(json!({"annotation": entry(session, &all, a)}))
}

fn nonempty(text: &str, what: &str) -> Result<String> {
    let t = text.trim();
    if t.is_empty() {
        return Err(Error::new(Code::InvalidParam, format!("give the {what}")).with("param", what));
    }
    Ok(t.to_owned())
}

/// `annotation.create`. Pins resolve against the author's draft, what
/// their window shows.
pub fn create(
    session: &mut Session,
    by: By,
    pins: &[crate::proto::PinArg],
    comment: &str,
    view: Option<crate::doc::views::ViewFile>,
) -> Result<Value> {
    let comment = nonempty(comment, "comment")?;
    let mut drafted = session.capturing(propose::draft(session, &by))?;
    let pins = pins_from(&mut drafted, pins)?;
    let taken = taken(session);
    let a = Annotation {
        id: new_id(|s| taken.contains(s))?,
        by,
        at: now(),
        status: Status::Open,
        claimed_by: None,
        pins,
        comment,
        view,
        change: None,
        replies: Default::default(),
        resolution: None,
    };
    written(session, &a)
}

/// `annotation.list`.
pub fn list(
    session: &mut Session,
    status: &[String],
    part: Option<&str>,
    by: Option<&str>,
    open: bool,
) -> Result<Value> {
    let wanted = status
        .iter()
        .map(|s| {
            Status::parse(s).ok_or_else(|| {
                Error::new(
                    Code::InvalidParam,
                    format!("no status {s:?}: open, in_progress, done or rejected"),
                )
                .with("status", s)
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let all = read_all(&session.snapshot());
    let mut faults = Vec::new();
    let mut shown: Vec<&Annotation> = Vec::new();
    for (id, a) in &all {
        let a = match a {
            Ok(a) => a,
            Err(e) => {
                faults.push(json!({"id": id, "error": e}));
                continue;
            }
        };
        let in_part = |p: &str| {
            a.pins.iter().any(|x| x.part() == p)
                || a.change.as_ref().is_some_and(|c| {
                    c.writes
                        .keys()
                        .any(|w| w.starts_with(&format!("parts/{p}/")))
                })
        };
        if (!wanted.is_empty() && !wanted.contains(&a.status))
            || (open && !a.status.is_open())
            || by.is_some_and(|b| a.by.participant != b)
            || part.is_some_and(|p| !in_part(p))
        {
            continue;
        }
        shown.push(a);
    }
    shown.sort_by(|x, y| (&y.at, &y.id).cmp(&(&x.at, &x.id)));
    let shown: Vec<Annotation> = shown.into_iter().cloned().collect();
    let entries: Vec<Value> = shown.iter().map(|a| entry(session, &all, a)).collect();
    let mut v = json!({"schema": 1, "annotations": entries});
    if !faults.is_empty() {
        v["faults"] = json!(faults);
    }
    Ok(v)
}

/// Where `render --annotations` draws pins on `part`: each open
/// annotation's pins that carry a point and lie on `part` (or on one of
/// its occurrences, placed by `poses`, keyed by instance path), with the
/// annotation's number among the open ones, oldest first, from 1. A pin
/// with no point is the app's to draw at its entity.
pub fn markers(
    files: &Files,
    part: &str,
    poses: &BTreeMap<String, ryukei::geom::Rigid>,
) -> Vec<([f64; 3], String)> {
    let all = readable(&read_all(files));
    let mut open: Vec<&Annotation> = all.values().filter(|a| a.status.is_open()).collect();
    open.sort_by(|a, b| (&a.at, &a.id).cmp(&(&b.at, &b.id)));
    let mut out = Vec::new();
    for (n, a) in open.iter().enumerate() {
        for pin in &a.pins {
            let at = match pin {
                Pin::Point { part: p, point_m } if p.as_str() == part => Some(*point_m),
                Pin::Entity {
                    reference,
                    point_m: Some(p),
                } => {
                    let steps: Vec<&str> = reference.split('/').collect();
                    if steps.first() != Some(&part) {
                        None
                    } else if steps.len() > 3 {
                        // `<asm>/<path>…/<solid>/<name>`: the point is in
                        // the occurrence's own part's frame.
                        let path = steps[1..steps.len() - 2].join("/");
                        poses.get(&path).map(|r| {
                            let q = r.apply_point(ryukei::geom::Point3::new(p[0], p[1], p[2]));
                            [q.x, q.y, q.z]
                        })
                    } else {
                        Some(*p)
                    }
                }
                _ => None,
            };
            if let Some(at) = at {
                out.push((at, (n + 1).to_string()));
            }
        }
    }
    out
}

/// `annotation.show`: the annotation, each pin resolved (its kind and
/// what it names now, or why not) and each file its change touches.
pub fn show(session: &mut Session, id: &str) -> Result<Value> {
    let a = one(session, id, None)?;
    let all = read_all(&session.snapshot());
    let mut pins = Vec::new();
    for pin in &a.pins {
        let mut v = json!({"ref": pin_text(pin)});
        if let Pin::Entity { reference, .. } = pin {
            match crate::entity::EntityRef::parse(reference).and_then(|r| r.resolve(session)) {
                Ok(t) => {
                    v["kind"] = json!(t.as_str());
                    let part = pin.part().to_owned();
                    let req = crate::proto::Request::Inspect {
                        part,
                        feature: None,
                        entity: Some(reference.clone()),
                        rev: None,
                    };
                    if let Ok(now) = crate::server::dispatch(session, None, None, req) {
                        v["now"] = now;
                    }
                }
                Err(e) => v["error"] = json!(e),
            }
        }
        pins.push(v);
    }
    let working = session.snapshot();
    let files: Vec<Value> = a
        .change
        .iter()
        .flat_map(|c| c.base.iter())
        .map(|(p, b)| {
            json!({"path": p, "base": b, "now": working.get(p).map(|x| hash(x)),
                             "deletes": a.change.as_ref().expect("a change").writes[p].is_none()})
        })
        .collect();
    Ok(json!({"annotation": entry(session, &all, &a), "pins": pins, "files": files}))
}

/// `annotation.reply`.
pub fn reply(
    session: &mut Session,
    id: &str,
    text: &str,
    expect: Option<&str>,
    by: By,
) -> Result<Value> {
    let mut a = one(session, id, expect)?;
    let text = nonempty(text, "text")?;
    let rid = a.next_reply();
    a.replies.insert(
        rid,
        Reply {
            by,
            at: now(),
            text,
        },
    );
    written(session, &a)
}

/// `annotation.claim`: `in_progress`, claimed by the caller.
pub fn claim(session: &mut Session, id: &str, expect: Option<&str>, by: By) -> Result<Value> {
    let mut a = one(session, id, expect)?;
    if let Some(c) = &a.claimed_by
        && *c != by
    {
        return Err(claimed(&a, c));
    }
    if closed(&a) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("annotation {id} is {}: reopen it first", a.status.as_str()),
        )
        .with("annotation", id));
    }
    a.status = Status::InProgress;
    a.claimed_by = Some(by);
    written(session, &a)
}

fn claimed(a: &Annotation, c: &By) -> Error {
    Error::new(
        Code::AnnotationClaimed,
        format!("annotation {} is claimed by {}", a.id, c.participant),
    )
    .with("annotation", &a.id)
    .with("claimed_by", c.to_json())
}

/// `annotation.resolve` and `annotation.reject` (`reason` given).
pub fn close(
    session: &mut Session,
    id: &str,
    rejected: Option<&str>,
    note: Option<&str>,
    expect: Option<&str>,
    by: By,
) -> Result<Value> {
    let mut a = one(session, id, expect)?;
    if closed(&a) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("annotation {id} is {} already", a.status.as_str()),
        )
        .with("annotation", id));
    }
    let note = match rejected {
        Some(reason) => Some(nonempty(reason, "reason")?),
        None => note
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned),
    };
    a.status = if rejected.is_some() {
        Status::Rejected
    } else {
        Status::Done
    };
    a.claimed_by = None;
    a.resolution = Some(Resolution {
        by,
        at: now(),
        note,
        applied: false,
    });
    written(session, &a)
}

/// `annotation.reopen`: `open` again, its resolution kept as a reply.
pub fn reopen(session: &mut Session, id: &str, expect: Option<&str>) -> Result<Value> {
    let mut a = one(session, id, expect)?;
    let Some(r) = a.resolution.take() else {
        return Err(
            Error::new(Code::OperationNoEffect, format!("annotation {id} is open"))
                .with("annotation", id),
        );
    };
    let word = if a.status == Status::Rejected {
        "rejected"
    } else {
        "resolved"
    };
    let text = match &r.note {
        Some(n) => format!("{word}: {n}"),
        None => word.to_owned(),
    };
    let rid = a.next_reply();
    a.replies.insert(
        rid,
        Reply {
            by: r.by,
            at: r.at,
            text,
        },
    );
    a.status = Status::Open;
    written(session, &a)
}

/// Refuses unless `by` may change or delete `a` as its author: open and
/// unclaimed (`annotation_claimed` otherwise), and `by` its author
/// (`invalid_param`).
fn authors(a: &Annotation, by: &By) -> Result<()> {
    if a.by != *by {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "only its author, {}, changes annotation {}",
                a.by.participant, a.id
            ),
        )
        .with("annotation", &a.id)
        .with("by", a.by.to_json()));
    }
    if let Some(c) = &a.claimed_by {
        return Err(claimed(a, c));
    }
    if a.status != Status::Open {
        return Err(Error::new(
            Code::AnnotationClaimed,
            format!(
                "annotation {} is {}: it can't be changed",
                a.id,
                a.status.as_str()
            ),
        )
        .with("annotation", &a.id));
    }
    Ok(())
}

/// `annotation.edit`.
pub fn edit(
    session: &mut Session,
    id: &str,
    comment: Option<&str>,
    pins: Option<&[crate::proto::PinArg]>,
    expect: Option<&str>,
    by: By,
) -> Result<Value> {
    let mut a = one(session, id, expect)?;
    authors(&a, &by)?;
    if let Some(c) = comment {
        a.comment = c.trim().to_owned();
        if a.comment.is_empty() && a.change.is_none() {
            return Err(Error::new(Code::InvalidParam, "give the comment").with("param", "comment"));
        }
    }
    if let Some(p) = pins {
        let mut drafted = session.capturing(propose::draft(session, &by))?;
        a.pins = pins_from(&mut drafted, p)?;
    }
    written(session, &a)
}

/// `annotation.delete`.
pub fn delete(session: &mut Session, id: &str, expect: Option<&str>, by: By) -> Result<Value> {
    let a = one(session, id, expect)?;
    if !closed(&a) {
        authors(&a, &by)?;
    }
    let at = crate::doc::annotation::path(session.root(), &a.id);
    session.remove_file(&at)?;
    session.reload()?;
    Ok(json!({"deleted": a.id}))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    #[test]
    fn markers_number_open_annotations_oldest_first() {
        let (_dir, mut s, face) = plate();
        let plate_id = || Id::parse("plate", "part").unwrap();
        let mut a = comment(
            "aaaaaaaaaa",
            vec![Pin::Entity {
                reference: face.clone(),
                point_m: Some([0.01, 0.0, 0.005]),
            }],
        );
        a.at = "2026-11-05T14:00:00Z".into();
        let mut b = comment(
            "bbbbbbbbbb",
            vec![Pin::Point {
                part: plate_id(),
                point_m: [0.0, 0.01, 0.0],
            }],
        );
        b.at = "2026-11-05T15:00:00Z".into();
        // No point: drawn by the app at its entity, not by renders.
        let mut c = comment(
            "cccccccccc",
            vec![Pin::Entity {
                reference: face,
                point_m: None,
            }],
        );
        c.at = "2026-11-05T16:00:00Z".into();
        let mut closed = comment(
            "dddddddddd",
            vec![Pin::Point {
                part: plate_id(),
                point_m: [1.0, 1.0, 1.0],
            }],
        );
        closed.at = "2026-11-05T12:00:00Z".into();
        closed.status = Status::Rejected;
        closed.resolution = Some(Resolution {
            by: By::agent("agent"),
            at: "2026-11-05T17:00:00Z".into(),
            note: Some("no".into()),
            applied: false,
        });
        for x in [&a, &b, &c, &closed] {
            put(&mut s, x);
        }
        let m = markers(&s.snapshot(), "plate", &BTreeMap::new());
        assert_eq!(
            m,
            vec![
                ([0.01, 0.0, 0.005], "1".to_owned()),
                ([0.0, 0.01, 0.0], "2".to_owned())
            ]
        );
        assert!(markers(&s.snapshot(), "other", &BTreeMap::new()).is_empty());
    }

    /// Every project write a handler makes goes through the session, so a
    /// request run in a capture session writes nothing (design 374). The
    /// free functions are left to code with no session: these files,
    /// none of which a proposable request reaches, and lines marked
    /// `// no session:`.
    #[test]
    fn proposable_handlers_write_through_the_session() {
        const FREE: [(&str, &str); 9] = [
            (
                "activity.rs",
                "undo and redo write their own entries' files",
            ),
            ("cli.rs", "the CLI's --out files"),
            ("drivers.rs", "Git's attributes, config and hooks"),
            ("review.rs", "the review client's renders"),
            ("export.rs", "an export's --out"),
            ("doctor.rs", "doctor's repairs"),
            ("validate.rs", "merge resolutions"),
            ("worktree.rs", "a worktree's recorded name"),
            ("import/plan.rs", "an import's files, written in parallel"),
        ];
        let free_names = [
            "write_json(",
            "write_atomic(",
            "write_atomic_with(",
            "remove_file(",
            "fs::write(",
            "remove_dir_all(",
        ];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut bad = Vec::new();
        let mut walk = vec![src.clone()];
        while let Some(dir) = walk.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                let rel = p
                    .strip_prefix(&src)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if p.is_dir() {
                    // The cache's entries and the benchmark's projects
                    // aren't project files a request writes.
                    if rel != "cache" && rel != "bench" {
                        walk.push(p);
                    }
                    continue;
                }
                if !rel.ends_with(".rs") || FREE.iter().any(|(f, _)| *f == rel) {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                // Up to the tests module, not a lone `#[cfg(test)]` item.
                let end = ["\n#[cfg(test)]\nmod ", "\n#[cfg(test)]\npub(crate) mod "]
                    .iter()
                    .filter_map(|m| text.find(m))
                    .min()
                    .unwrap_or(text.len());
                let body = &text[..end];
                let lines: Vec<&str> = body.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    let code = line.split("//").next().unwrap();
                    for name in free_names {
                        let mut from = 0;
                        while let Some(k) = code[from..].find(name) {
                            let at = from + k;
                            from = at + name.len();
                            let before = &code[..at];
                            let method = before.ends_with('.');
                            let defined = before.trim_end().ends_with("fn");
                            let marked = line.contains("// no session:")
                                || (i > 0 && lines[i - 1].contains("// no session:"));
                            if !(method || defined || marked) {
                                bad.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                            }
                        }
                    }
                }
            }
        }
        assert!(
            bad.is_empty(),
            "write through the session:\n{}",
            bad.join("\n")
        );
    }

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
    fn ask(s: &mut crate::session::Session, m: &str, p: Value) -> crate::error::Result<Value> {
        call(s, m, p)
    }

    #[test]
    fn an_annotation_goes_from_open_to_done_and_back() {
        let (_dir, mut s, face) = plate();
        let made = ask(
            &mut s,
            "annotation.create",
            json!({"pins": [{"ref": face, "point": [1, 2, 5]}],
                              "comment": "Round this edge", "as": "dave"}),
        )
        .unwrap();
        let id = made["annotation"]["id"].as_str().unwrap().to_owned();
        assert_eq!(
            made["annotation"]["by"],
            json!({"participant": "dave", "kind": "agent"})
        );
        assert_eq!(
            made["annotation"]["pins"][0]["point_m"],
            json!([0.001, 0.002, 0.005])
        );
        let l = ask(&mut s, "annotation.list", json!({"open": true})).unwrap();
        assert_eq!(l["annotations"].as_array().unwrap().len(), 1);
        // Replies number one past the largest.
        ask(
            &mut s,
            "annotation.reply",
            json!({"id": id, "text": "Which edge?"}),
        )
        .unwrap();
        let r = ask(
            &mut s,
            "annotation.reply",
            json!({"id": id, "text": "The top one."}),
        )
        .unwrap();
        assert!(r["annotation"]["replies"]["r2"].is_object());
        // One claim at a time.
        ask(&mut s, "annotation.claim", json!({"id": id, "as": "a1"})).unwrap();
        let e = ask(&mut s, "annotation.claim", json!({"id": id, "as": "a2"})).unwrap_err();
        assert_eq!(e.code, Code::AnnotationClaimed);
        assert_eq!(e.details["claimed_by"]["participant"], "a1");
        let e = ask(
            &mut s,
            "annotation.edit",
            json!({"id": id, "comment": "x", "as": "dave"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::AnnotationClaimed);
        let done = ask(
            &mut s,
            "annotation.resolve",
            json!({"id": id, "note": "Done: 2 mm", "as": "a1"}),
        )
        .unwrap();
        assert_eq!(done["annotation"]["status"], "done");
        assert!(done["annotation"]["claimed_by"].is_null());
        let back = ask(&mut s, "annotation.reopen", json!({"id": id})).unwrap();
        assert_eq!(back["annotation"]["status"], "open");
        assert!(back["annotation"]["resolution"].is_null());
        assert_eq!(
            back["annotation"]["replies"]["r3"]["text"],
            "resolved: Done: 2 mm"
        );
        let e = ask(
            &mut s,
            "annotation.reject",
            json!({"id": id, "reason": " "}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        ask(
            &mut s,
            "annotation.reject",
            json!({"id": id, "reason": "Not this part"}),
        )
        .unwrap();
        let shown = ask(&mut s, "annotation.show", json!({"id": id})).unwrap();
        assert_eq!(shown["annotation"]["status"], "rejected");
        assert_eq!(shown["pins"][0]["kind"], "face");
        // Anyone deletes a closed annotation.
        ask(
            &mut s,
            "annotation.delete",
            json!({"id": id, "as": "someone"}),
        )
        .unwrap();
        let e = ask(&mut s, "annotation.show", json!({"id": id})).unwrap_err();
        assert_eq!(e.code, Code::AnnotationNotFound);
    }

    #[test]
    fn only_the_author_edits_or_deletes_an_open_annotation() {
        let (_dir, mut s, face) = plate();
        let id = ask(
            &mut s,
            "annotation.create",
            json!({"pins": [{"ref": face}], "comment": "a", "as": "dave"}),
        )
        .unwrap()["annotation"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let e = ask(
            &mut s,
            "annotation.edit",
            json!({"id": id, "comment": "b", "as": "erin"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let e = ask(&mut s, "annotation.delete", json!({"id": id, "as": "erin"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let v = ask(
            &mut s,
            "annotation.edit",
            json!({"id": id, "comment": "b", "as": "dave"}),
        )
        .unwrap();
        assert_eq!(v["annotation"]["comment"], "b");
        // Guarded by its file's BLAKE3.
        let e = ask(
            &mut s,
            "annotation.reply",
            json!({"id": id, "text": "t", "expect": "0".repeat(64)}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        let e = ask(
            &mut s,
            "annotation.create",
            json!({"pins": [], "comment": "  "}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let e = ask(&mut s, "annotation.create",
                    json!({"pins": [{"ref": "plate/slab/00000000000000000000000000000000"}], "comment": "x"}))
            .unwrap_err();
        assert_ne!(e.code, Code::Internal);
        ask(&mut s, "annotation.delete", json!({"id": id, "as": "dave"})).unwrap();
    }

    #[test]
    fn list_filters_and_writes_nothing_into_any_revision() {
        let (_dir, mut s, face) = plate();
        let rev = s
            .project()
            .part(&crate::doc::id::Id::parse("plate", "part").unwrap())
            .unwrap()
            .revision
            .clone();
        let prev = s.project_revision().unwrap();
        ask(
            &mut s,
            "annotation.create",
            json!({"pins": [{"ref": face}], "comment": "a", "as": "dave"}),
        )
        .unwrap();
        ask(
            &mut s,
            "annotation.create",
            json!({"pins": [{"part": "plate", "point": [0, 0, 0]}],
                                                "comment": "b", "as": "erin"}),
        )
        .unwrap();
        let by = ask(&mut s, "annotation.list", json!({"by": "erin"})).unwrap();
        assert_eq!(by["annotations"].as_array().unwrap().len(), 1);
        let part = ask(&mut s, "annotation.list", json!({"part": "plate"})).unwrap();
        assert_eq!(part["annotations"].as_array().unwrap().len(), 2);
        let comments: BTreeSet<&str> = part["annotations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["comment"].as_str().unwrap())
            .collect();
        assert_eq!(comments, BTreeSet::from(["a", "b"]));
        let e = ask(&mut s, "annotation.list", json!({"status": ["closed"]})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(
            s.project()
                .part(&crate::doc::id::Id::parse("plate", "part").unwrap())
                .unwrap()
                .revision,
            rev
        );
        assert_eq!(s.project_revision().unwrap(), prev);
    }
    #[test]
    fn the_etiquette_names_the_commands_it_asks_for() {
        for cmd in [
            "annotations list --open",
            "annotations claim",
            "annotations reply",
            "annotations resolve",
            "annotations reject",
            "--reason",
        ] {
            assert!(ETIQUETTE.contains(cmd), "{cmd}");
        }
    }
}
