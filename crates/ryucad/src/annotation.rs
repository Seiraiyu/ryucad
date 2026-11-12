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
Some annotations carry marks: read them in words with `ryucad annotations show <id>` and look at them with `ryucad render --annotation <id> --out a.png`. A mark says where, not exactly how far: ask, or use the measured points, before reading a size off an arrow.
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
    /// Marks' touches that don't resolve now, by mark and touch.
    pub outdated_marks: Vec<Value>,
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
    for (i, m) in a.marks.iter().flat_map(|m| m.list.iter()).enumerate() {
        for (j, t) in m.touches.iter().flatten().enumerate() {
            if let Err(e) = crate::entity::EntityRef::parse(&t.reference)
                .and_then(|r| r.resolve(session).map(drop))
            {
                let mut v = json!({"mark": i, "touch": j, "at": t.at.as_str(),
                                   "ref": t.reference, "code": e.code});
                for key in ["was", "removed_by", "candidates"] {
                    if let Some(x) = e.details.get(key) {
                        v[key] = x.clone();
                    }
                }
                d.outdated_marks.push(v);
            }
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

/// An annotation as listings give it: its file's fields (its marks as
/// `{part, count, kinds, list}`: the window draws them), and `outdated`,
/// `stale` (with `outdated_pins`, `outdated_marks` and `stale_files` when true) and, with a
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
    o.insert(
        "outdated".into(),
        json!(!d.outdated.is_empty() || !d.outdated_marks.is_empty()),
    );
    if !d.outdated.is_empty() {
        o.insert("outdated_pins".into(), json!(d.outdated));
    }
    if !d.outdated_marks.is_empty() {
        o.insert("outdated_marks".into(), json!(d.outdated_marks));
    }
    o.insert("stale".into(), json!(!d.stale.is_empty()));
    if !d.stale.is_empty() {
        o.insert("stale_files".into(), json!(d.stale));
    }
    if let Some(applied) = d.applied {
        o.insert("applied".into(), json!(applied));
    }
    if let Some(m) = &a.marks {
        o.insert(
            "marks".into(),
            json!({"part": m.part, "count": m.list.len(),
                   "kinds": m.list.iter().map(|k| k.kind.as_str()).collect::<Vec<_>>(),
                   "list": m.list.iter().map(crate::redline::Mark::to_json).collect::<Vec<_>>()}),
        );
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
                let id = crate::part::part_id(part)?;
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

/// What a pick pass leaves out beside the view's own section planes:
/// the author's participant's hidden set, when the request came from one.
#[derive(Clone, Debug, Default)]
pub struct PickFrom {
    pub hidden: std::collections::BTreeSet<String>,
}

/// Marks as a request gives them, read in `drafted` (the author's draft)
/// and seen through `view`: shorthand marks read in the default unit,
/// touches given checked to resolve, the rest worked out by one pick
/// pass of the view over the draft.
pub fn marks_from(
    drafted: &mut Session,
    arg: &crate::proto::MarksArg,
    view: Option<&crate::doc::views::ViewFile>,
    from: &PickFrom,
) -> Result<crate::doc::redline::Marks> {
    use crate::doc::redline::{Anchor, Kind, Mark, Marks};
    let unit = drafted.project().manifest.default_unit;
    let list = json!({"part": arg.part, "list": arg.list});
    let mut marks = Marks::from_json(&list, "/marks", false)?;
    drafted.project().part(&marks.part)?;
    for (i, s) in arg.short.iter().enumerate() {
        let bad = |why: &str| {
            Error::new(
                Code::InvalidParam,
                format!("--{} {:?}: {why}", s.kind, s.at),
            )
            .with("pointer", format!("/marks/short/{i}"))
        };
        let q = |t: &str| -> Result<f64> {
            crate::units::Quantity::parse(t.trim(), unit)
                .map(|q| q.meters())
                .map_err(|_| bad("each coordinate is a length"))
        };
        let xy = |t: &str| -> Result<[f64; 2]> {
            let (x, y) = t.split_once(',').ok_or_else(|| bad("a point is X,Y"))?;
            Ok([q(x)?, q(y)?])
        };
        let kind = Kind::parse(&s.kind)
            .filter(|k| *k != Kind::Stroke)
            .ok_or_else(|| bad("line, arrow, ellipse, rectangle or text"))?;
        let (points, text) = if kind == Kind::Text {
            let (p, t) =
                s.at.split_once('=')
                    .ok_or_else(|| bad("text is X,Y=TEXT"))?;
            (vec![xy(p)?], Some(t.to_owned()))
        } else {
            let (a, b) =
                s.at.split_once(':')
                    .ok_or_else(|| bad("two points: X1,Y1:X2,Y2"))?;
            (vec![xy(a)?, xy(b)?], None)
        };
        let mark = Mark {
            kind,
            anchor: Anchor::View(points),
            text,
            touches: None,
        };
        // Read back as the file reads it, so a shorthand keeps the same
        // limits a mark given whole keeps.
        let at = format!("/marks/list/{}", marks.list.len());
        marks.list.push(
            Marks::from_json(
                &json!({"part": marks.part, "list": [mark.to_json()]}),
                "/marks",
                false,
            )
            .map_err(|e| e.with("pointer", at))?
            .list
            .remove(0),
        );
    }
    if marks.list.len() > crate::doc::redline::MAX_MARKS {
        return Err(crate::doc::files::invalid(
            "/marks/list",
            "a list of at most 64 marks",
        ));
    }
    let Some(view) = view else {
        return Err(crate::doc::files::invalid(
            "/view",
            "marks need the view they were drawn in: give a camera, --view or --of",
        ));
    };
    for m in &marks.list {
        for t in m.touches.iter().flatten() {
            crate::entity::EntityRef::parse(&t.reference)?.resolve(drafted)?;
        }
    }
    if marks.list.iter().any(|m| m.touches.is_none()) {
        let camera = crate::view::Camera::from(view);
        let sections = view
            .sections
            .iter()
            .map(|s| crate::section::Plane {
                origin_m: s.origin_m,
                normal: s.normal,
            })
            .collect();
        let ask = crate::scene::RenderAsk::pick(
            marks.part.as_str(),
            camera.clone(),
            sections,
            from.hidden.clone(),
        );
        let scene = crate::scene::render_scene(drafted, None, Vec::new(), ask)?;
        let plane = crate::redline::Plane::of(&camera);
        let (w, h) = crate::redline::pick_size(&plane, &camera, &marks.list);
        let picked = crate::render::pick(&scene.g, &camera, w, h)?;
        let seen = crate::render::View::new(&camera, w, h);
        for m in &mut marks.list {
            if m.touches.is_none() {
                m.touches = Some(crate::redline::touches(
                    &picked,
                    &seen,
                    &plane,
                    m,
                    &scene.poses,
                ));
            }
        }
    }
    Ok(marks)
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
    Annotation::from_json(&a.to_json())?;
    save(session, a)?;
    let all = read_all(&session.snapshot());
    let mut v = json!({"annotation": entry(session, &all, a)});
    if a.marks.is_some() {
        v["marks"] = json!(crate::redline::words(session, a)?);
    }
    Ok(v)
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
    marks: Option<&crate::proto::MarksArg>,
    from: &PickFrom,
) -> Result<Value> {
    let comment = nonempty(comment, "comment")?;
    let mut drafted = session.capturing(propose::draft(session, &by))?;
    let pins = pins_from(&mut drafted, pins)?;
    let marks = marks
        .map(|m| marks_from(&mut drafted, m, view.as_ref(), from))
        .transpose()?;
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
        marks,
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

/// Where a pin with a point is drawn on `part`: its point, placed by its
/// occurrence's pose (`poses`, by instance path) when it names one; `None`
/// for a pin with no point, or one on another part.
pub fn pin_on(
    pin: &Pin,
    part: &str,
    poses: &BTreeMap<String, ryukei::geom::Rigid>,
) -> Option<[f64; 3]> {
    match pin {
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
    }
}

/// Each open annotation's number: its place among the open ones,
/// oldest first, from 1, as `render --annotations` and the app number
/// them.
pub fn numbers(files: &Files) -> BTreeMap<Id, usize> {
    let all = readable(&read_all(files));
    let mut open: Vec<&Annotation> = all.values().filter(|a| a.status.is_open()).collect();
    open.sort_by(|a, b| (&a.at, &a.id).cmp(&(&b.at, &b.id)));
    open.iter()
        .enumerate()
        .map(|(n, a)| (a.id.clone(), n + 1))
        .collect()
}

/// Where `render --annotations` draws pins on `part`: each open
/// annotation's pins that carry a point and lie on `part` (or on one of
/// its occurrences, placed by `poses`, keyed by instance path), with the
/// annotation's number among the open ones ([`numbers`]), but `skip`'s
/// (the annotation `render --annotation` draws itself). A pin with no
/// point is the app's to draw at its entity.
pub fn markers(
    files: &Files,
    part: &str,
    poses: &BTreeMap<String, ryukei::geom::Rigid>,
    skip: Option<&Id>,
) -> Vec<([f64; 3], String)> {
    let all = readable(&read_all(files));
    let numbers = numbers(files);
    let mut out = Vec::new();
    let mut open: Vec<(&usize, &Annotation)> = numbers
        .iter()
        .filter(|(id, _)| Some(*id) != skip)
        .filter_map(|(id, n)| all.get(id).map(|a| (n, a)))
        .collect();
    open.sort_by_key(|(n, _)| **n);
    for (n, a) in open {
        for pin in &a.pins {
            if let Some(at) = pin_on(pin, part, poses) {
                out.push((at, n.to_string()));
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
    let marks = crate::redline::words(session, &a)?;
    Ok(
        json!({"annotation": entry(session, &all, &a), "pins": pins, "files": files,
              "marks": marks}),
    )
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
#[allow(clippy::too_many_arguments)] // one per field a request edits
pub fn edit(
    session: &mut Session,
    id: &str,
    comment: Option<&str>,
    pins: Option<&[crate::proto::PinArg]>,
    marks: Option<&crate::proto::MarksArg>,
    no_marks: bool,
    from: &PickFrom,
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
    if no_marks {
        a.marks = None;
    }
    if let Some(m) = marks {
        let mut drafted = session.capturing(propose::draft(session, &by))?;
        a.marks = Some(marks_from(&mut drafted, m, a.view.as_ref(), from)?);
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
    use crate::redline::{Anchor, At, Touch};
    use crate::testing::{call, fresh};
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
        let m = markers(&s.snapshot(), "plate", &BTreeMap::new(), None);
        assert_eq!(
            m,
            vec![
                ([0.01, 0.0, 0.005], "1".to_owned()),
                ([0.0, 0.01, 0.0], "2".to_owned())
            ]
        );
        assert!(markers(&s.snapshot(), "other", &BTreeMap::new(), None).is_empty());
        // `render --annotation` draws its own: the others keep their numbers.
        let skip = Id::parse("aaaaaaaaaa", "annotation").unwrap();
        assert_eq!(
            markers(&s.snapshot(), "plate", &BTreeMap::new(), Some(&skip)),
            vec![([0.0, 0.01, 0.0], "2".to_owned())]
        );
        let n = numbers(&s.snapshot());
        assert_eq!(n.len(), 3, "the open ones");
        assert_eq!(n[&skip], 1);
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
                // A test module in a file of its own (`server/tests.rs`)
                // and the tests' harness are test code throughout.
                if rel.ends_with("/tests.rs") || rel == "testing.rs" {
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
    /// A 60 × 40 × 5 mm plate with two through holes (at x 20 and 30,
    /// y 20) and a boss on its top (at x 45, y 20, 6 mm tall), and a
    /// view from a corner above it.
    pub(crate) fn drilled() -> (
        tempfile::TempDir,
        crate::session::Session,
        crate::doc::views::ViewFile,
    ) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "plate"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "plate", "type": "box", "id": "slab",
             "params": {"x": 60, "y": 40, "z": 5}}),
        )
        .unwrap();
        for (id, x) in [("h1", 20), ("h2", 30)] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "plate", "type": "cylinder", "id": id,
                 "operation": "cut", "targets": ["slab"], "params": {"radius": 3, "height": 9},
                 "placement": {"origin": [x, 20, -2]}}),
            )
            .unwrap();
        }
        call(
            &mut s,
            "feature.add",
            json!({"part": "plate", "type": "cylinder", "id": "boss",
             "operation": "join", "targets": ["slab"], "params": {"radius": 4, "height": 6},
             "placement": {"origin": [45, 20, 5]}}),
        )
        .unwrap();
        let camera = crate::view::Camera {
            eye: [0.12, -0.10, 0.09],
            target: [0.03, 0.02, 0.0025],
            up: [0.0, 0.0, 1.0],
            projection: crate::view::Projection::Orthographic { height: 0.08 },
        };
        (dir, s, camera.to_view_file())
    }

    /// Where model point `p` (metres) is in `view`'s plane.
    pub(crate) fn on(view: &crate::doc::views::ViewFile, p: [f64; 3]) -> [f64; 2] {
        crate::redline::Plane::of(&crate::view::Camera::from(view)).xy(p)
    }

    /// The feature that made entity `r` of the plate, as `inspect` says.
    fn made_by(s: &mut crate::session::Session, r: &str) -> String {
        let v = call(s, "inspect", json!({"part": "plate", "entity": r})).unwrap();
        v["entity"]["origin"]["feature"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn create_marked(
        s: &mut crate::session::Session,
        view: &crate::doc::views::ViewFile,
        list: Value,
    ) -> Result<Value> {
        let marks = crate::proto::MarksArg {
            part: "plate".into(),
            list: serde_json::from_value(list).unwrap(),
            short: Vec::new(),
        };
        create(
            s,
            By::agent("agent"),
            &[],
            "look",
            Some(view.clone()),
            Some(&marks),
            &Default::default(),
        )
    }

    #[test]
    fn each_kind_touches_what_lies_under_it() {
        let (_dir, mut s, view) = drilled();
        let boss_top = on(&view, [0.045, 0.02, 0.011]);
        let plate_top = on(&view, [0.008, 0.008, 0.005]);
        // A hole's wall seen from the corner: the far side of h1's bore,
        // just under the top.
        let wall = on(&view, [0.020, 0.023, 0.004]);
        let (h1, h2) = (
            on(&view, [0.017, 0.017, 0.005]),
            on(&view, [0.033, 0.023, 0.005]),
        );
        let a = create_marked(&mut s, &view, json!([
            {"kind": "arrow", "in": "view", "xy_m": [boss_top, plate_top]},
            {"kind": "line", "in": "view", "xy_m": [plate_top, wall]},
            {"kind": "ellipse", "in": "view", "xy_m": [[h1[0] - 0.002, h1[1] - 0.002], [h2[0] + 0.002, h2[1] + 0.002]]},
            {"kind": "line", "in": "view", "xy_m": [[0.3, 0.3], [0.31, 0.3]]},
            {"kind": "text", "in": "view", "xy_m": [[0, 0]], "text": "2 mm"}
        ])).unwrap();
        let list = &a["annotation"]["marks"];
        assert_eq!(list["count"], 5);
        let file = read_all(&s.snapshot())
            .into_values()
            .next()
            .unwrap()
            .unwrap();
        let marks = file.marks.unwrap().list;
        let t = |i: usize| marks[i].touches.clone().unwrap();
        assert_eq!(t(0)[0].at, At::Tail);
        assert_eq!(made_by(&mut s, &t(0)[0].reference), "boss");
        assert_eq!(t(0)[1].at, At::Tip);
        assert_eq!(made_by(&mut s, &t(0)[1].reference), "slab");
        assert_eq!(
            made_by(&mut s, &t(1)[1].reference),
            "h1",
            "the line's end on a bore's wall"
        );
        let inside: Vec<String> = t(2).iter().map(|x| made_by(&mut s, &x.reference)).collect();
        assert!(
            inside.contains(&"h1".to_owned()) && inside.contains(&"h2".to_owned()),
            "{inside:?}"
        );
        assert!(
            t(3).is_empty(),
            "a mark over the background touches nothing"
        );
        assert!(t(4).is_empty(), "text touches nothing");
    }

    #[test]
    fn a_face_stroke_lies_on_the_faces_under_its_points() {
        let (_dir, mut s, view) = drilled();
        let a = create_marked(
            &mut s,
            &view,
            json!([
                {"kind": "stroke", "in": "face", "on": "plate/slab",
                 "points_m": [[0.005, 0.005, 0.005], [0.015, 0.005, 0.005], [0.025, 0.005, 0.005]]}
            ]),
        )
        .unwrap();
        let file = read_all(&s.snapshot())
            .into_values()
            .next()
            .unwrap()
            .unwrap();
        let t = file.marks.unwrap().list[0].touches.clone().unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(made_by(&mut s, &t[0].reference), "slab");
        assert!(
            a["marks"][0]["touches"][0]["ref"].is_string(),
            "the answer reads the marks"
        );
    }

    #[test]
    fn touches_given_are_kept_and_checked() {
        let (_dir, mut s, view) = drilled();
        let face = call(&mut s, "inspect", json!({"part": "plate"})).unwrap()["bodies"][0]["faces"]
            [0]["name"]
            .as_str()
            .unwrap()
            .to_owned();
        let given = format!("plate/slab/{face}");
        create_marked(
            &mut s,
            &view,
            json!([
                {"kind": "line", "in": "view", "xy_m": [[0, 0], [0.01, 0]],
                 "touches": [{"at": "end", "ref": given}]}
            ]),
        )
        .unwrap();
        let file = read_all(&s.snapshot())
            .into_values()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(
            file.marks.unwrap().list[0].touches.clone().unwrap(),
            vec![Touch {
                at: At::End,
                reference: given
            }]
        );
        let e = create_marked(
            &mut s,
            &view,
            json!([
                {"kind": "line", "in": "view", "xy_m": [[0, 0], [0.01, 0]],
                 "touches": [{"at": "end", "ref": "plate/slab/00000000000000000000000000000000"}]}
            ]),
        )
        .unwrap_err();
        // The resolver's own refusal: a name the session never built is not
        // found, one it built and lost is unresolved.
        assert!(
            matches!(e.code, Code::NotFound | Code::ReferenceUnresolved),
            "{e:?}"
        );
    }

    #[test]
    fn marks_need_a_view_and_keep_to_their_limits() {
        let (_dir, mut s, _view) = drilled();
        let marks = crate::proto::MarksArg {
            part: "plate".into(),
            list: vec![json!({"kind": "line", "in": "view", "xy_m": [[0, 0], [0.01, 0]]})],
            short: Vec::new(),
        };
        let e = create(
            &mut s,
            By::agent("agent"),
            &[],
            "look",
            None,
            Some(&marks),
            &Default::default(),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].clone()),
            (Code::InvalidDocument, json!("/view"))
        );
        let (_dir, mut s, view) = drilled();
        let e = create_marked(
            &mut s,
            &view,
            json!(vec![
                json!({"kind": "text", "in": "view", "xy_m": [[0, 0]], "text": "x"});
                65
            ]),
        )
        .unwrap_err();
        assert_eq!(e.details["pointer"], "/marks/list");
        let e = create_marked(
            &mut s,
            &view,
            json!([{"kind": "line", "in": "view", "xy_m": [[0, 0], [0.01, 0]]}])
                .as_array()
                .unwrap()
                .iter()
                .cloned()
                .map(|mut m| {
                    m["kind"] = json!("cloud");
                    m
                })
                .collect::<Value>(),
        )
        .unwrap_err();
        assert_eq!(e.details["pointer"], "/marks/list/0/kind");
    }

    #[test]
    fn shorthand_marks_read_in_the_default_unit() {
        let (_dir, mut s, view) = drilled();
        let marks = crate::proto::MarksArg {
            part: "plate".into(),
            list: Vec::new(),
            short: vec![
                crate::proto::ShortMark {
                    kind: "arrow".into(),
                    at: "-20,11:9,-4".into(),
                },
                crate::proto::ShortMark {
                    kind: "text".into(),
                    at: "20,15=2 mm".into(),
                },
            ],
        };
        create(
            &mut s,
            By::agent("agent"),
            &[],
            "look",
            Some(view.clone()),
            Some(&marks),
            &Default::default(),
        )
        .unwrap();
        let file = read_all(&s.snapshot())
            .into_values()
            .next()
            .unwrap()
            .unwrap();
        let m = file.marks.unwrap();
        // One multiplication from millimetres, as every quantity.
        let Anchor::View(xy) = &m.list[0].anchor else {
            panic!()
        };
        for (p, w) in xy.iter().zip([[-0.02, 0.011], [0.009, -0.004]]) {
            assert!(
                (p[0] - w[0]).abs() < 1e-15 && (p[1] - w[1]).abs() < 1e-15,
                "{xy:?}"
            );
        }
        assert_eq!(m.list[1].text.as_deref(), Some("2 mm"));
        let bad = crate::proto::MarksArg {
            part: "plate".into(),
            list: Vec::new(),
            short: vec![crate::proto::ShortMark {
                kind: "arrow".into(),
                at: "1,2".into(),
            }],
        };
        let e = create(
            &mut s,
            By::agent("agent"),
            &[],
            "look",
            Some(view),
            Some(&bad),
            &Default::default(),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].clone()),
            (Code::InvalidParam, json!("/marks/short/0"))
        );
    }

    #[test]
    fn touches_are_read_from_the_authors_draft() {
        // A person proposes a lid on the plate's top; their mark's end
        // lands on it, which only their draft has.
        use crate::testing::call_live;
        let (_dir, mut s, view) = drilled();
        let mut live = crate::live::Live::new();
        let h = live
            .hello(crate::live::Kind::Human, Some("Dana"), None)
            .unwrap();
        call_live(
            &mut s,
            &mut live,
            Some(&h),
            "participant.mode",
            json!({"mode": "annotate"}),
        )
        .unwrap();
        call_live(
            &mut s,
            &mut live,
            Some(&h),
            "annotation.propose",
            json!({"request": {
            "method": "feature.add", "params": {"part": "plate", "type": "box", "id": "lid",
            "operation": "join", "targets": ["slab"], "params": {"x": 10, "y": 10, "z": 4},
            "placement": {"origin": [2, 2, 5]}}}}),
        )
        .unwrap();
        let end = on(&view, [0.007, 0.007, 0.009]);
        let a = call_live(&mut s, &mut live, Some(&h), "annotation.create", json!({
            "comment": "here",
            "camera": {"eye": ["120 mm", "-100 mm", "90 mm"], "target": ["30 mm", "20 mm", "2.5 mm"],
                       "up": [0, 0, 1], "projection": {"orthographic": {"height": "80 mm"}}},
            "marks": {"part": "plate", "list": [{"kind": "line", "in": "view", "xy_m": [[0.3, 0.3], end]}]}})).unwrap();
        let r = a["marks"][0]["touches"][0]["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        let name = live.get(&h).unwrap().name.clone();
        let v = call(
            &mut s,
            "inspect",
            json!({"part": "plate", "entity": r, "rev": format!("draft:{name}")}),
        )
        .unwrap();
        assert_eq!(v["entity"]["origin"]["feature"], "lid");
        // The working files have no lid: there the touch is outdated. The
        // proposal and the mark are made in one second and listed by time,
        // then by their random IDs, so find the mark by its own ID.
        let id = a["annotation"]["id"].clone();
        let e = list(&mut s, &[], None, None, false).unwrap();
        let mine = e["annotations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id)
            .unwrap();
        assert_eq!(mine["outdated"], true);
    }

    #[test]
    fn a_touch_that_no_longer_resolves_makes_its_annotation_outdated() {
        let (_dir, mut s, view) = drilled();
        let boss_top = on(&view, [0.045, 0.02, 0.011]);
        create_marked(
            &mut s,
            &view,
            json!([
                {"kind": "line", "in": "view", "xy_m": [[0.3, 0.3], boss_top]}
            ]),
        )
        .unwrap();
        let list = |s: &mut crate::session::Session| {
            list(s, &[], None, None, false).unwrap()["annotations"][0].clone()
        };
        assert_eq!(list(&mut s)["outdated"], false);
        call(
            &mut s,
            "feature.delete",
            json!({"part": "plate", "id": "boss"}),
        )
        .unwrap();
        let e = list(&mut s);
        assert_eq!(e["outdated"], true);
        let o = &e["outdated_marks"][0];
        assert_eq!(
            (o["mark"].clone(), o["touch"].clone(), o["at"].clone()),
            (json!(0), json!(0), json!("end"))
        );
        // The resolver's own code, as a pin's (`not_found` for a name
        // this session never learnt, `reference_unresolved` for one it did).
        assert!(
            ["not_found", "reference_unresolved"].contains(&o["code"].as_str().unwrap()),
            "{o}"
        );
        assert!(e.get("outdated_pins").is_none());
        // The marks themselves are as they were.
        let file = read_all(&s.snapshot())
            .into_values()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(
            file.marks.unwrap().list[0].anchor,
            Anchor::View(vec![[0.3, 0.3], boss_top])
        );
    }

    #[test]
    fn show_reads_each_mark_in_words() {
        let (_dir, mut s, view) = drilled();
        let boss_top = on(&view, [0.045, 0.02, 0.011]);
        let plate_top = on(&view, [0.008, 0.008, 0.005]);
        let a = create_marked(
            &mut s,
            &view,
            json!([
                {"kind": "arrow", "in": "view", "xy_m": [boss_top, plate_top]},
                {"kind": "rectangle", "in": "view", "xy_m": [[0, 0], [0.01, 0.005]]},
                {"kind": "stroke", "in": "face", "on": "plate/slab",
                 "points_m": [[0.005, 0.005, 0.005], [0.015, 0.005, 0.005]]},
                {"kind": "text", "in": "view", "xy_m": [[0, 0]], "text": "2 mm"}
            ]),
        )
        .unwrap();
        let id = a["annotation"]["id"].as_str().unwrap().to_owned();
        let v = show(&mut s, &id).unwrap();
        let m = &v["marks"];
        assert_eq!(m[0]["kind"], "arrow");
        assert_eq!(m[0]["in"], "view");
        assert_eq!(
            m[0]["points"][0]["plane"]["value"]
                .as_str()
                .unwrap()
                .matches("mm")
                .count(),
            2
        );
        assert!(m[0]["points"][1]["model"]["m"].is_array());
        // The arrow's length is measured in the view's plane, exactly.
        let d = [plate_top[0] - boss_top[0], plate_top[1] - boss_top[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        assert!((m[0]["length"]["m"].as_f64().unwrap() - len).abs() < 1e-12);
        assert_eq!(m[0]["touches"][0]["at"], "tail");
        assert_eq!(m[0]["touches"][0]["kind"], "face");
        assert_eq!(m[0]["touches"][0]["outdated"], false);
        assert_eq!(
            m[0]["touches"][0]["now"]["entity"]["origin"]["feature"],
            "boss"
        );
        assert_eq!(m[1]["box"]["plane"]["size"]["value"], "(10 mm, 5 mm)");
        assert_eq!(m[2]["in"], "face");
        assert_eq!(m[2]["on"], "plate/slab");
        assert_eq!(m[2]["count"], 2);
        assert_eq!(m[3]["text"], "2 mm");
        assert!(m[3]["touches"].as_array().unwrap().is_empty());
        // Listings give a summary, and the marks themselves for the window
        // to draw, as the file has them.
        assert_eq!(
            v["annotation"]["marks"]["kinds"],
            json!(["arrow", "rectangle", "stroke", "text"])
        );
        let id = v["annotation"]["id"].as_str().unwrap();
        let file = one(&s, id, None).unwrap();
        assert_eq!(
            v["annotation"]["marks"]["list"],
            file.marks.unwrap().to_json()["list"]
        );
    }

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
            marks: None,
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
            "annotations show",
            "render --annotation",
        ] {
            assert!(ETIQUETTE.contains(cmd), "{cmd}");
        }
        assert!(ETIQUETTE.contains("A mark says where"));
    }
}
