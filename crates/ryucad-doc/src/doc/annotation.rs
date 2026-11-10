//! Annotations: `annotations/<id>.json` at the project's root, one file
//! each, a person's (or an agent's) words about the model, pinned to
//! entities or points, optionally carrying a proposed change (a write
//! that wasn't made). They count in no revision: nothing a rebuild makes
//! reads them, and an agent's `--expect` never conflicts with one.
//!
//! ```json
//! {"kind": "ryucad.annotation", "format": 1, "id": "k3v7q2m7xa",
//!  "by": {"participant": "dave (window 1)", "kind": "human"},
//!  "at": "2026-11-05T14:02:11Z", "status": "open", "claimed_by": null,
//!  "pins": [{"ref": "bracket/base/9f1c…", "point_m": [0.012, 0, 0.004]}],
//!  "comment": "Move this hole 5 mm towards the edge",
//!  "view": {"eye_m": […], "target_m": […], "up": […],
//!           "projection": "perspective", "fov_deg": 40},
//!  "change": null, "replies": {}, "resolution": null}
//! ```
//!
//! Format 2 adds `marks`, redline marks drawn in `view` (see
//! `doc::redline`); a file with none is written at format 1.
//!
//! Derived states (`outdated`, `stale`, `applied`) are never written:
//! `ryucad::annotation` works them out from the model.

use super::files::{Obj, header, invalid, number};
use super::id::Id;
use super::redline::Marks;
use super::sketch::clean;
use super::views::ViewFile;
use crate::error::{Code, Error, Result};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const KIND: &str = "ryucad.annotation";

/// The newest format this build reads: 2 holds marks (design 423); a file
/// with none is written at 1.
pub const FORMAT: u64 = 2;

/// The folder of the project's annotations, beside the manifest.
pub const ANNOTATIONS: &str = "annotations";

/// Whether an annotation was made by a person (a window) or an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorKind {
    Human,
    Agent,
}

impl AuthorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthorKind::Human => "human",
            AuthorKind::Agent => "agent",
        }
    }
}

/// Who made something: the participant's name and kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct By {
    pub participant: String,
    pub kind: AuthorKind,
}

impl By {
    pub fn agent(name: &str) -> By {
        By {
            participant: name.to_owned(),
            kind: AuthorKind::Agent,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"participant": self.participant, "kind": self.kind.as_str()})
    }

    fn from_json(v: &Value, at: &str) -> Result<By> {
        let mut o = Obj::new(v, at)?;
        let participant = o
            .take("participant")?
            .as_str()
            .filter(|s| !s.trim().is_empty() && !s.contains('\n'))
            .ok_or_else(|| invalid(&format!("{at}/participant"), "a participant's name"))?
            .to_owned();
        let kind = match o.take("kind")?.as_str() {
            Some("human") => AuthorKind::Human,
            Some("agent") => AuthorKind::Agent,
            _ => {
                return Err(invalid(
                    &format!("{at}/kind"),
                    "expected \"human\" or \"agent\"",
                ));
            }
        };
        o.finish()?;
        Ok(By { participant, kind })
    }
}

/// What someone recorded of an annotation. `outdated`, `stale` and
/// `applied` are derived, never a status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Open,
    InProgress,
    Done,
    Rejected,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::InProgress => "in_progress",
            Status::Done => "done",
            Status::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "open" => Status::Open,
            "in_progress" => Status::InProgress,
            "done" => Status::Done,
            "rejected" => Status::Rejected,
            _ => return None,
        })
    }

    /// `open` or `in_progress`: what `--open` lists.
    pub fn is_open(self) -> bool {
        matches!(self, Status::Open | Status::InProgress)
    }
}

/// Where an annotation points: an entity reference as requests write it,
/// with the clicked point in that part's frame if any, or a bare point in
/// a part's or assembly's frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Pin {
    Entity {
        reference: String,
        point_m: Option<[f64; 3]>,
    },
    Point {
        part: Id,
        point_m: [f64; 3],
    },
}

impl Pin {
    pub fn to_json(&self) -> Value {
        let three = |a: &[f64; 3]| a.map(|x| number(clean(x)));
        match self {
            Pin::Entity { reference, point_m } => {
                let mut v = json!({"ref": reference});
                if let Some(p) = point_m {
                    v["point_m"] = json!(three(p));
                }
                v
            }
            Pin::Point { part, point_m } => json!({"part": part, "point_m": three(point_m)}),
        }
    }

    /// The part the pin is in.
    pub fn part(&self) -> &str {
        match self {
            Pin::Entity { reference, .. } => reference.split('/').next().unwrap_or_default(),
            Pin::Point { part, .. } => part.as_str(),
        }
    }

    fn from_json(v: &Value, at: &str) -> Result<Pin> {
        let mut o = Obj::new(v, at)?;
        let point = |o: &mut Obj, required: bool| -> Result<Option<[f64; 3]>> {
            let p = match o.take_opt("point_m") {
                None if !required => return Ok(None),
                None => {
                    return Err(invalid(
                        &format!("{at}/point_m"),
                        "missing field \"point_m\"",
                    ));
                }
                Some(p) => p,
            };
            three(&p, &format!("{at}/point_m")).map(Some)
        };
        let pin = match o.take_opt("ref") {
            Some(r) => {
                let reference = r
                    .as_str()
                    .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
                    .ok_or_else(|| invalid(&format!("{at}/ref"), "an entity reference"))?
                    .to_owned();
                Pin::Entity {
                    reference,
                    point_m: point(&mut o, false)?,
                }
            }
            None => {
                let part = o
                    .take("part")?
                    .as_str()
                    .and_then(|s| Id::parse(s, "part").ok())
                    .ok_or_else(|| invalid(&format!("{at}/part"), "a part's ID"))?;
                Pin::Point {
                    part,
                    point_m: point(&mut o, true)?.expect("required"),
                }
            }
        };
        o.finish()?;
        Ok(pin)
    }
}

/// A write that wasn't made: the request, the BLAKE3 of each file it
/// touches as the proposer's draft had it (`None` for a file it
/// creates), each file's content (`None` for a deletion) and the
/// proposer's earlier open annotations it stands on.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub request: Value,
    pub rough: bool,
    pub base: BTreeMap<String, Option<String>>,
    pub writes: BTreeMap<String, Option<Value>>,
    pub after: Vec<Id>,
}

impl Change {
    pub fn to_json(&self) -> Value {
        json!({
            "request": self.request,
            "rough": self.rough,
            "base": self.base,
            "writes": self.writes,
            "after": self.after,
        })
    }

    fn from_json(v: &Value, at: &str) -> Result<Change> {
        let mut o = Obj::new(v, at)?;
        let request = o.take("request")?;
        let method_ok = request["method"].as_str().is_some_and(|m| !m.is_empty());
        if !method_ok || !request.as_object().is_some_and(|r| r.len() <= 2) {
            return Err(invalid(
                &format!("{at}/request"),
                "a request: {\"method\", \"params\"}",
            ));
        }
        let rough = o
            .take("rough")?
            .as_bool()
            .ok_or_else(|| invalid(&format!("{at}/rough"), "expected true or false"))?;
        let ptr = |key: &str, path: &str| format!("{at}/{key}/{}", escape(path));
        let mut base = BTreeMap::new();
        for (path, h) in object(&o.take("base")?, &format!("{at}/base"))? {
            check_path(path).map_err(|m| invalid(&ptr("base", path), m))?;
            let hash = match h {
                Value::Null => None,
                Value::String(s) if is_hash(s) => Some(s.clone()),
                _ => return Err(invalid(&ptr("base", path), "a BLAKE3 (64 hex) or null")),
            };
            base.insert(path.clone(), hash);
        }
        let mut writes = BTreeMap::new();
        for (path, w) in object(&o.take("writes")?, &format!("{at}/writes"))? {
            check_path(path).map_err(|m| invalid(&ptr("writes", path), m))?;
            if !base.contains_key(path) {
                return Err(invalid(&ptr("writes", path), "a write needs its base"));
            }
            writes.insert(path.clone(), (!w.is_null()).then(|| w.clone()));
        }
        if writes.is_empty() {
            return Err(invalid(
                &format!("{at}/writes"),
                "a change writes something",
            ));
        }
        let after = o
            .take("after")?
            .as_array()
            .ok_or_else(|| invalid(&format!("{at}/after"), "a list of annotation IDs"))?
            .iter()
            .enumerate()
            .map(|(i, x)| {
                x.as_str()
                    .and_then(|s| check_id(s).ok())
                    .ok_or_else(|| invalid(&format!("{at}/after/{i}"), "an annotation ID"))
            })
            .collect::<Result<Vec<_>>>()?;
        o.finish()?;
        Ok(Change {
            request,
            rough,
            base,
            writes,
            after,
        })
    }
}

/// A reply, by its ID (`r1`, `r2`, … one past the largest).
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub by: By,
    pub at: String,
    pub text: String,
}

/// How a `done` or `rejected` annotation was closed.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    pub by: By,
    pub at: String,
    /// A rejection's reason (required), or a note.
    pub note: Option<String>,
    /// Whether `annotation.apply` closed it.
    pub applied: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub id: Id,
    pub by: By,
    pub at: String,
    pub status: Status,
    /// Who is working it: set exactly while `in_progress`.
    pub claimed_by: Option<By>,
    pub pins: Vec<Pin>,
    pub comment: String,
    pub view: Option<ViewFile>,
    /// Redline marks (design 423), drawn in `view`; `None` writes the file
    /// at format 1 with no `marks` key, as stage 19 did.
    pub marks: Option<Marks>,
    pub change: Option<Change>,
    pub replies: BTreeMap<String, Reply>,
    pub resolution: Option<Resolution>,
}

/// An annotation's ID: 10 characters of RFC 4648's base32, lower case.
pub fn check_id(text: &str) -> Result<Id> {
    let ok = text.len() == 10
        && text
            .bytes()
            .all(|b| b"abcdefghijklmnopqrstuvwxyz234567".contains(&b));
    if !ok {
        return Err(Error::new(
            Code::InvalidId,
            format!("{text:?} isn't an annotation ID: 10 base32 characters"),
        )
        .with("id", text));
    }
    Id::parse(text, "annotation")
}

/// An annotation's path relative to the project, with `/`.
pub fn rel(id: &Id) -> String {
    format!("{ANNOTATIONS}/{id}.json")
}

/// An annotation's file in the project at `root`.
pub fn path(root: &Path, id: &Id) -> PathBuf {
    root.join(ANNOTATIONS).join(format!("{id}.json"))
}

/// Whether `rel` is an annotation's path (`annotations/<id>.json`).
pub fn is_annotation_path(rel: &str) -> bool {
    rel.strip_prefix(&format!("{ANNOTATIONS}/"))
        .and_then(|n| n.strip_suffix(".json"))
        .is_some_and(|id| check_id(id).is_ok())
}

/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn is_time(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

fn is_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A path a change may write: relative, `/`-separated, no `.` or `..`,
/// not an annotation.
fn check_path(p: &str) -> std::result::Result<(), &'static str> {
    let steps_ok = !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
    if !steps_ok {
        return Err("a path relative to the project");
    }
    if p.starts_with(&format!("{ANNOTATIONS}/")) {
        return Err("a change never writes an annotation");
    }
    if !is_proposable_path(p) {
        return Err(
            "a file a model edit writes: ryucad.json, ryucad.lock, materials.json, parameters.json, ryupdm.json or parts/**/*.json",
        );
    }
    Ok(())
}

/// The files a proposable request may write (`annotation::PROPOSABLE`):
/// the project's root JSON files and the JSON files under `parts/`. Never
/// Git's folder, `.gitattributes` or an imported file, whatever a file
/// arriving by a merge or written by hand says.
pub fn is_proposable_path(p: &str) -> bool {
    use super::{lock::LOCK, materials::MATERIALS, project};
    const ROOT: [&str; 5] = [
        project::MANIFEST,
        LOCK,
        MATERIALS,
        project::PARAMETERS,
        project::PDM_CONFIG,
    ];
    ROOT.contains(&p)
        || p.strip_prefix("parts/").is_some_and(|rest| {
            rest.ends_with(".json")
                && rest.contains('/')
                && rest.split('/').all(|s| !s.starts_with('.'))
        })
}

/// A key as a JSON pointer step.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn object<'a>(v: &'a Value, at: &str) -> Result<&'a Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| invalid(at, "expected a JSON object"))
}

fn three(v: &Value, at: &str) -> Result<[f64; 3]> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or_else(|| invalid(at, "expected three numbers"))?;
    let mut out = [0.0; 3];
    for (i, x) in a.iter().enumerate() {
        out[i] = clean(
            x.as_f64()
                .filter(|x| x.is_finite())
                .ok_or_else(|| invalid(&format!("{at}/{i}"), "expected a number"))?,
        );
    }
    Ok(out)
}

fn time(v: Value, at: &str) -> Result<String> {
    v.as_str()
        .filter(|s| is_time(s))
        .map(str::to_owned)
        .ok_or_else(|| invalid(at, "a time: YYYY-MM-DDTHH:MM:SSZ"))
}

fn text(v: Value, at: &str) -> Result<String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid(at, "expected text"))
}

impl Annotation {
    pub fn to_json(&self) -> Value {
        let view = self.view.as_ref().map(|v| {
            let mut j = v.to_json();
            if let Some(o) = j.as_object_mut() {
                o.remove("kind");
                o.remove("format");
            }
            j
        });
        let replies: Map<String, Value> = self
            .replies
            .iter()
            .map(|(id, r)| {
                (
                    id.clone(),
                    json!({"by": r.by.to_json(), "at": r.at, "text": r.text}),
                )
            })
            .collect();
        let mut v = json!({
            "kind": KIND,
            "format": 1,
            "id": self.id,
            "by": self.by.to_json(),
            "at": self.at,
            "status": self.status.as_str(),
            "claimed_by": self.claimed_by.as_ref().map(By::to_json),
            "pins": self.pins.iter().map(Pin::to_json).collect::<Vec<_>>(),
            "comment": self.comment,
            "view": view,
            "change": self.change.as_ref().map(Change::to_json),
            "replies": replies,
            "resolution": self.resolution.as_ref().map(|r| json!({
                "by": r.by.to_json(), "at": r.at, "note": r.note, "applied": r.applied,
            })),
        });
        if let Some(m) = &self.marks {
            v["format"] = json!(FORMAT);
            v["marks"] = m.to_json();
        }
        v
    }

    pub fn from_json(v: &Value) -> Result<Annotation> {
        let format = v["format"].as_u64().unwrap_or(1);
        let mut o = header(v, KIND, FORMAT)?;
        let id = o
            .take("id")?
            .as_str()
            .and_then(|s| check_id(s).ok())
            .ok_or_else(|| invalid("/id", "an annotation ID: 10 base32 characters"))?;
        let by = By::from_json(&o.take("by")?, "/by")?;
        let at = time(o.take("at")?, "/at")?;
        let status = o
            .take("status")?
            .as_str()
            .and_then(Status::parse)
            .ok_or_else(|| invalid("/status", "expected open, in_progress, done or rejected"))?;
        let claimed_by = match o.take_opt("claimed_by") {
            None | Some(Value::Null) => None,
            Some(c) => Some(By::from_json(&c, "/claimed_by")?),
        };
        if (status == Status::InProgress) != claimed_by.is_some() {
            return Err(invalid("/claimed_by", "set exactly while in_progress"));
        }
        let pins = o
            .take("pins")?
            .as_array()
            .ok_or_else(|| invalid("/pins", "a list of pins"))?
            .iter()
            .enumerate()
            .map(|(i, p)| Pin::from_json(p, &format!("/pins/{i}")))
            .collect::<Result<Vec<_>>>()?;
        let comment = text(o.take("comment")?, "/comment")?;
        let view = match o.take_opt("view") {
            None | Some(Value::Null) => None,
            Some(mut v) => {
                if let Some(m) = v.as_object_mut() {
                    m.insert("kind".into(), json!(super::views::KIND));
                    m.insert("format".into(), json!(1));
                }
                Some(ViewFile::from_json(&v).map_err(|e| {
                    let p = e.details["pointer"].as_str().unwrap_or("").to_owned();
                    e.with("pointer", format!("/view{p}"))
                })?)
            }
        };
        let change = match o.take_opt("change") {
            None | Some(Value::Null) => None,
            Some(c) => Some(Change::from_json(&c, "/change")?),
        };
        let marks = match o.take_opt("marks") {
            None | Some(Value::Null) => None,
            Some(_) if format < 2 => {
                return Err(invalid("/marks", "marks need format 2 of the file"));
            }
            Some(m) => Some(Marks::from_json(&m, "/marks", true)?),
        };
        if marks.is_some() && view.is_none() {
            return Err(invalid("/view", "marks need the view they were drawn in"));
        }
        if comment.trim().is_empty() && change.is_none() {
            return Err(invalid("/comment", "a comment, unless it carries a change"));
        }
        let mut replies = BTreeMap::new();
        for (rid, r) in object(&o.take("replies")?, "/replies")? {
            let at = format!("/replies/{}", escape(rid));
            let numbered = rid.strip_prefix('r').is_some_and(|n| {
                !n.starts_with('0') && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
            });
            if !numbered {
                return Err(invalid(&at, "a reply's ID: r1, r2, …"));
            }
            let mut ro = Obj::new(r, &at)?;
            let reply = Reply {
                by: By::from_json(&ro.take("by")?, &format!("{at}/by"))?,
                at: time(ro.take("at")?, &format!("{at}/at"))?,
                text: text(ro.take("text")?, &format!("{at}/text"))?,
            };
            ro.finish()?;
            replies.insert(rid.clone(), reply);
        }
        let resolution = match o.take_opt("resolution") {
            None | Some(Value::Null) => None,
            Some(r) => {
                let mut ro = Obj::new(&r, "/resolution")?;
                let res = Resolution {
                    by: By::from_json(&ro.take("by")?, "/resolution/by")?,
                    at: time(ro.take("at")?, "/resolution/at")?,
                    note: match ro.take("note")? {
                        Value::Null => None,
                        n => Some(text(n, "/resolution/note")?),
                    },
                    applied: ro
                        .take("applied")?
                        .as_bool()
                        .ok_or_else(|| invalid("/resolution/applied", "expected true or false"))?,
                };
                ro.finish()?;
                Some(res)
            }
        };
        let closed = matches!(status, Status::Done | Status::Rejected);
        if closed != resolution.is_some() {
            return Err(invalid("/resolution", "set exactly while done or rejected"));
        }
        if status == Status::Rejected
            && resolution
                .as_ref()
                .and_then(|r| r.note.as_deref())
                .is_none_or(|n| n.trim().is_empty())
        {
            return Err(invalid("/resolution/note", "a rejection gives its reason"));
        }
        o.finish()?;
        Ok(Annotation {
            id,
            by,
            at,
            status,
            claimed_by,
            pins,
            comment,
            view,
            marks,
            change,
            replies,
            resolution,
        })
    }

    /// The ID the next reply takes: one past the largest.
    pub fn next_reply(&self) -> String {
        let n = self
            .replies
            .keys()
            .filter_map(|k| k.strip_prefix('r')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("r{}", n + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "kind": "ryucad.annotation", "format": 1, "id": "k3v7q2m7xa",
            "by": {"participant": "dave (window 1)", "kind": "human"},
            "at": "2026-11-05T14:02:11Z",
            "status": "open",
            "pins": [{"ref": "bracket/base/9f1c00000000000000000000000000aa",
                      "point_m": [0.012, 0, 0.004]},
                     {"part": "bracket", "point_m": [0, 0, 0.01]}],
            "comment": "Move this hole 5 mm towards the edge",
            "view": {"eye_m": [0.3, -0.3, 0.3], "target_m": [0, 0, 0], "up": [0, 0, 1],
                     "projection": "perspective", "fov_deg": 40},
            "change": {"request": {"method": "feature.edit",
                                   "params": {"part": "bracket", "id": "fillet1",
                                              "fields": {"radius": "3 mm"}}},
                       "rough": false,
                       "base": {"parts/bracket/features/fillet1.json": "ab".repeat(32)},
                       "writes": {"parts/bracket/features/fillet1.json": {"kind": "ryucad.feature"}},
                       "after": []},
            "replies": {"r1": {"by": {"participant": "agent", "kind": "agent"},
                               "at": "2026-11-05T14:05:00Z", "text": "Looking."}},
            "resolution": null
        })
    }

    #[test]
    fn an_annotation_reads_and_writes_back_the_same() {
        let a = Annotation::from_json(&sample()).unwrap();
        assert_eq!(a.id.as_str(), "k3v7q2m7xa");
        assert_eq!(a.by.kind, AuthorKind::Human);
        assert_eq!(a.pins.len(), 2);
        assert_eq!(a.status, Status::Open);
        assert!(a.change.as_ref().unwrap().after.is_empty());
        let back = Annotation::from_json(&a.to_json()).unwrap();
        assert_eq!(back, a);
        // `null` fields are written as null, so every file has every key.
        assert_eq!(a.to_json()["resolution"], Value::Null);
        assert_eq!(a.to_json()["claimed_by"], Value::Null);
    }

    #[test]
    fn faults_are_invalid_documents_at_their_pointer() {
        let bad = |edit: &dyn Fn(&mut Value), at: &str| {
            let mut v = sample();
            edit(&mut v);
            let e = Annotation::from_json(&v).unwrap_err();
            assert_eq!(e.code, Code::InvalidDocument, "{at}: {e:?}");
            assert_eq!(e.details["pointer"], at, "{e:?}");
        };
        bad(&|v| v["id"] = json!("Not-An-Id"), "/id");
        bad(&|v| v["id"] = json!("abc"), "/id");
        bad(&|v| v["at"] = json!("yesterday"), "/at");
        bad(&|v| v["status"] = json!("closed"), "/status");
        bad(&|v| v["by"]["kind"] = json!("robot"), "/by/kind");
        bad(&|v| v["pins"][0]["ref"] = json!(""), "/pins/0/ref");
        bad(
            &|v| v["pins"][1]["point_m"] = json!([0, 0]),
            "/pins/1/point_m",
        );
        bad(
            &|v| {
                v["replies"] = json!({"x1": {"by": {"participant": "a", "kind": "agent"},
                                              "at": "2026-11-05T14:05:00Z", "text": "t"}})
            },
            "/replies/x1",
        );
        bad(
            &|v| {
                v["change"] = Value::Null;
                v["comment"] = json!("");
            },
            "/comment",
        );
        bad(
            &|v| v["change"]["writes"] = json!({"../escape.json": null}),
            "/change/writes/..~1escape.json",
        );
        bad(
            &|v| v["change"]["writes"] = json!({"annotations/x.json": null}),
            "/change/writes/annotations~1x.json",
        );
        for path in [
            ".git/config",
            ".git/hooks/pre-commit",
            ".gitattributes",
            "imports/x.step",
            "imports/x.json",
            "parts/p/x.step",
            "parts/x.json",
            "parts/p/.hidden.json",
            "other.json",
        ] {
            bad(
                &|v| {
                    v["change"]["base"] = json!({path: null});
                    v["change"]["writes"] = json!({path: {}});
                },
                &format!("/change/base/{}", escape(path)),
            );
        }
        bad(
            &|v| {
                v["change"]["writes"] = json!({".git/config": {}});
            },
            "/change/writes/.git~1config",
        );
        bad(
            &|v| v["change"]["base"] = json!({"parts/p/part.json": "nothex"}),
            "/change/base/parts~1p~1part.json",
        );
        bad(
            &|v| {
                v["status"] = json!("rejected");
                v["resolution"] = json!({"by": {"participant": "a", "kind": "agent"},
                                     "at": "2026-11-05T14:05:00Z", "note": null,
                                     "applied": false});
            },
            "/resolution/note",
        );
        bad(&|v| v["status"] = json!("in_progress"), "/claimed_by");
        bad(&|v| v["extra"] = json!(1), "/extra");
    }

    #[test]
    fn a_change_writes_the_files_model_edits_write() {
        for path in [
            "ryucad.json",
            "ryucad.lock",
            "materials.json",
            "parameters.json",
            "ryupdm.json",
            "parts/p/part.json",
            "parts/p/features/f.json",
            "parts/p/views/v.json",
        ] {
            let mut v = sample();
            v["change"]["base"] = json!({path: null});
            v["change"]["writes"] = json!({path: {}});
            Annotation::from_json(&v).unwrap_or_else(|e| panic!("{path}: {e:?}"));
        }
    }

    fn with_marks() -> Value {
        let mut v = sample();
        v["format"] = json!(2);
        v["marks"] = json!({"part": "bracket", "list": [
            {"kind": "arrow", "in": "view", "xy_m": [[0, 0], [0.01, 0]],
             "touches": [{"at": "tip", "ref": "bracket/base/9f1c00000000000000000000000000aa"}]}]});
        v
    }

    #[test]
    fn marks_are_format_2_and_none_are_format_1_byte_for_byte() {
        let a = Annotation::from_json(&with_marks()).unwrap();
        assert_eq!(a.marks.as_ref().unwrap().list.len(), 1);
        assert_eq!(a.to_json()["format"], 2);
        assert_eq!(a.to_json()["marks"], with_marks()["marks"]);
        assert_eq!(Annotation::from_json(&a.to_json()).unwrap(), a);
        // Without marks: format 1 and no `marks` key, as stage 19 wrote.
        let plain = Annotation::from_json(&sample()).unwrap();
        assert!(plain.marks.is_none());
        assert_eq!(plain.to_json()["format"], 1);
        assert!(plain.to_json().get("marks").is_none());
    }

    #[test]
    fn marks_need_format_2_and_a_view() {
        let mut v = with_marks();
        v["format"] = json!(1);
        let e = Annotation::from_json(&v).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].clone()),
            (Code::InvalidDocument, json!("/marks"))
        );
        let mut v = with_marks();
        v["view"] = Value::Null;
        let e = Annotation::from_json(&v).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].clone()),
            (Code::InvalidDocument, json!("/view"))
        );
        let mut v = with_marks();
        v["marks"]["list"][0]["kind"] = json!("cloud");
        let e = Annotation::from_json(&v).unwrap_err();
        assert_eq!(e.details["pointer"], "/marks/list/0/kind");
    }

    #[test]
    fn a_newer_format_is_refused() {
        let mut v = sample();
        v["format"] = json!(3);
        assert_eq!(
            Annotation::from_json(&v).unwrap_err().code,
            Code::FormatTooNew
        );
    }

    #[test]
    fn ids_are_ten_base32_characters() {
        assert!(check_id("k3v7q2m7xa").is_ok());
        assert!(check_id("k3v7q2m7x").is_err());
        assert!(check_id("k3v7q2m7x1").is_err()); // `1` isn't base32
        assert_eq!(
            rel(&check_id("k3v7q2m7xa").unwrap()),
            "annotations/k3v7q2m7xa.json"
        );
    }
}
