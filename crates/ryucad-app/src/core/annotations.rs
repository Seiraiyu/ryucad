//! The project's annotations as the window holds them (design 374): read
//! with `annotation.list`, kept current from `annotation.changed`, each
//! with its derived states (`outdated`, `stale`, `applied`); the
//! Annotations tile's filters, the markers' numbers and places, and the
//! person's own open annotations and draft. No egui.

use crate::core::model::Pose;
use serde_json::Value;
use std::collections::BTreeMap;

/// A pin: an entity's reference, a bare point on a part, or both.
#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub reference: Option<String>,
    /// The part (or assembly) it is on: the reference's first step, or a
    /// bare point's part.
    pub part: String,
    /// In meters: in the frame of the part at the reference's end (an
    /// occurrence's own part), or the bare point's part's.
    pub point: Option<[f64; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub id: String,
    pub by: String,
    pub agent: bool,
    pub at: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    pub by: String,
    pub note: Option<String>,
    pub applied: bool,
}

/// One annotation, as a listing gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub by: String,
    pub agent: bool,
    pub at: String,
    /// `open`, `in_progress`, `done` or `rejected`.
    pub status: String,
    pub claimed_by: Option<String>,
    pub pins: Vec<Pin>,
    pub comment: String,
    /// The camera its author looked through.
    pub view: Option<ryucad::view::Camera>,
    /// The method its proposed change would send, if it carries one.
    pub change: Option<String>,
    pub rough: bool,
    pub replies: Vec<Reply>,
    pub resolution: Option<Resolution>,
    /// Each pin that no longer resolves, in words.
    pub outdated: Vec<String>,
    pub stale: bool,
    pub applied: bool,
    /// The listing's JSON, whole.
    pub raw: Value,
}

fn author(v: &Value) -> (String, bool) {
    (
        v["participant"].as_str().unwrap_or_default().to_owned(),
        v["kind"] == "agent",
    )
}

fn three(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

impl Entry {
    pub fn from_json(v: &Value) -> Option<Entry> {
        let (by, agent) = author(&v["by"]);
        let pins = v["pins"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let reference = p["ref"].as_str().map(str::to_owned);
                let part = match &reference {
                    Some(r) => r.split('/').next()?.to_owned(),
                    None => p["part"].as_str()?.to_owned(),
                };
                Some(Pin {
                    reference,
                    part,
                    point: three(&p["point_m"]),
                })
            })
            .collect();
        let mut replies: Vec<Reply> = v["replies"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(id, r)| {
                let (by, agent) = author(&r["by"]);
                Reply {
                    id: id.clone(),
                    by,
                    agent,
                    at: r["at"].as_str().unwrap_or_default().to_owned(),
                    text: r["text"].as_str().unwrap_or_default().to_owned(),
                }
            })
            .collect();
        replies.sort_by(|a, b| (&a.at, &a.id).cmp(&(&b.at, &b.id)));
        let outdated = v["outdated_pins"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| {
                let why = match o["removed_by"].as_str() {
                    Some(f) => format!("removed by {f}"),
                    None => o["code"].as_str().unwrap_or("unresolved").replace('_', " "),
                };
                format!("{}: {why}", o["ref"].as_str().unwrap_or_default())
            })
            .collect();
        // A listing gives the view without the stored file's header.
        let view = v["view"].as_object().and_then(|o| {
            let mut o = o.clone();
            o.insert("kind".into(), ryucad::doc::views::KIND.into());
            o.insert("format".into(), 1.into());
            let f = ryucad::doc::views::ViewFile::from_json(&Value::Object(o)).ok()?;
            Some(ryucad::view::Camera::from(&f))
        });
        Some(Entry {
            id: v["id"].as_str()?.to_owned(),
            by,
            agent,
            at: v["at"].as_str().unwrap_or_default().to_owned(),
            status: v["status"].as_str().unwrap_or("open").to_owned(),
            claimed_by: v["claimed_by"]["participant"].as_str().map(str::to_owned),
            pins,
            comment: v["comment"].as_str().unwrap_or_default().to_owned(),
            view,
            change: v["change"]["request"]["method"].as_str().map(str::to_owned),
            rough: v["change"]["rough"] == true,
            replies,
            resolution: v["resolution"].is_object().then(|| Resolution {
                by: author(&v["resolution"]["by"]).0,
                note: v["resolution"]["note"].as_str().map(str::to_owned),
                applied: v["resolution"]["applied"] == true,
            }),
            outdated,
            stale: v["stale"] == true,
            applied: v["applied"] == true,
            raw: v.clone(),
        })
    }

    /// `open` or `in_progress`.
    pub fn is_open(&self) -> bool {
        matches!(self.status.as_str(), "open" | "in_progress")
    }

    /// Whether it is one of `me`'s suggestions the draft lays: open, a
    /// human's, carrying a change not yet applied that still applies.
    pub fn drafted_by(&self, me: &str) -> bool {
        self.is_open()
            && !self.agent
            && self.by == me
            && self.change.is_some()
            && !self.applied
            && !self.stale
    }
}

/// Which annotations the tile shows by status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Show {
    #[default]
    Open,
    All,
    Done,
    Rejected,
}

/// The tile's filter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub show: Show,
    /// Only this window's person's.
    pub mine: bool,
    /// Only those pinned to, or changing, this part.
    pub part: Option<String>,
}

/// Where a marker stands.
#[derive(Clone, Debug, PartialEq)]
pub enum At {
    /// A point in the shown model's frame.
    Point([f64; 3]),
    /// The middle of this entity's mesh points (a pin with no point).
    Entity(String),
}

/// A pin drawn over the viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub id: String,
    /// The pin's place among its annotation's pins.
    pub pin: usize,
    pub number: usize,
    pub at: At,
    pub outdated: bool,
    pub by: String,
    pub comment: String,
}

/// What an `annotation.changed` meant to the window's person.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct News {
    /// Replies someone else added to the person's annotations.
    pub replies: Vec<(String, Reply)>,
    /// The person's annotations someone else closed: ID, status, note.
    pub closed: Vec<(String, String, Option<String>)>,
}

/// Every annotation the window knows.
#[derive(Clone, Debug, Default)]
pub struct Annotations {
    pub all: BTreeMap<String, Entry>,
    pub loaded: bool,
    /// Files that don't read, as the listing gives them.
    pub faults: Vec<Value>,
    /// Bumped whenever anything here changes, so views redraw.
    pub generation: u64,
}

impl Annotations {
    /// Takes an `annotation.list` answer.
    pub fn reset(&mut self, list: &Value) {
        self.all = list["annotations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Entry::from_json)
            .map(|e| (e.id.clone(), e))
            .collect();
        self.faults = list["faults"].as_array().cloned().unwrap_or_default();
        self.loaded = true;
        self.generation += 1;
    }

    /// Takes an `annotation.changed` notification, and says what it
    /// meant to `me` (this window's participant name).
    pub fn changed(&mut self, params: &Value, me: &str) -> News {
        let mut news = News::default();
        for v in params["annotations"].as_array().into_iter().flatten() {
            let Some(e) = Entry::from_json(v) else {
                continue;
            };
            if let Some(was) = self.all.get(&e.id)
                && e.by == me
                && !e.agent
            {
                for r in &e.replies {
                    if r.by != me && !was.replies.iter().any(|x| x.id == r.id) {
                        news.replies.push((e.id.clone(), r.clone()));
                    }
                }
                if was.is_open()
                    && !e.is_open()
                    && e.resolution.as_ref().is_some_and(|r| r.by != me)
                {
                    let note = e.resolution.as_ref().and_then(|r| r.note.clone());
                    news.closed.push((e.id.clone(), e.status.clone(), note));
                }
            }
            self.all.insert(e.id.clone(), e);
        }
        for id in params["removed"].as_array().into_iter().flatten() {
            if let Some(id) = id.as_str() {
                self.all.remove(id);
            }
        }
        self.generation += 1;
        news
    }

    fn open_oldest_first(&self) -> Vec<&Entry> {
        let mut open: Vec<&Entry> = self.all.values().filter(|e| e.is_open()).collect();
        open.sort_by(|a, b| (&a.at, &a.id).cmp(&(&b.at, &b.id)));
        open
    }

    /// Each open annotation's number: its place among the open ones,
    /// oldest first, from 1, as `render --annotations` numbers them.
    pub fn numbers(&self) -> BTreeMap<String, usize> {
        self.open_oldest_first()
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), i + 1))
            .collect()
    }

    /// How many of `me`'s annotations are open.
    pub fn open_mine(&self, me: &str) -> usize {
        self.all
            .values()
            .filter(|e| e.is_open() && e.by == me)
            .count()
    }

    /// The IDs of `me`'s suggestions the draft lays, oldest first.
    pub fn draft(&self, me: &str) -> Vec<String> {
        self.open_oldest_first()
            .into_iter()
            .filter(|e| e.drafted_by(me))
            .map(|e| e.id.clone())
            .collect()
    }

    /// What the tile lists under `f`: open ones first, then the rest,
    /// each newest first.
    pub fn shown(&self, f: &Filter, me: &str) -> Vec<&Entry> {
        let mut out: Vec<&Entry> = self
            .all
            .values()
            .filter(|e| match f.show {
                Show::Open => e.is_open(),
                Show::All => true,
                Show::Done => e.status == "done",
                Show::Rejected => e.status == "rejected",
            })
            .filter(|e| !f.mine || e.by == me)
            .filter(|e| {
                f.part.as_deref().is_none_or(|p| {
                    e.pins.iter().any(|x| x.part == p)
                        || e.raw["change"]["writes"].as_object().is_some_and(|w| {
                            w.keys().any(|k| k.starts_with(&format!("parts/{p}/")))
                        })
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.is_open()
                .cmp(&a.is_open())
                .then_with(|| (&b.at, &b.id).cmp(&(&a.at, &a.id)))
        });
        out
    }

    /// The open annotations' pins on the parts `shown`, each placed by
    /// `pose` (an occurrence's body's, `(part, "<path>/<solid>")`, as
    /// `Model::pose` gives it).
    pub fn markers(&self, shown: &[&str], pose: impl Fn(&(String, String)) -> Pose) -> Vec<Marker> {
        let mut out = Vec::new();
        for (i, e) in self.open_oldest_first().into_iter().enumerate() {
            for (k, pin) in e.pins.iter().enumerate() {
                if !shown.contains(&pin.part.as_str()) {
                    continue;
                }
                let at = match (&pin.reference, pin.point) {
                    (Some(r), Some(p)) => {
                        let steps: Vec<&str> = r.split('/').collect();
                        let body = steps[1..steps.len().saturating_sub(1).max(1)].join("/");
                        At::Point(pose(&(pin.part.clone(), body)).apply(p))
                    }
                    (Some(r), None) => At::Entity(r.clone()),
                    (None, Some(p)) => At::Point(p),
                    (None, None) => continue,
                };
                out.push(Marker {
                    id: e.id.clone(),
                    pin: k,
                    number: i + 1,
                    at,
                    outdated: !e.outdated.is_empty(),
                    by: e.by.clone(),
                    comment: e.comment.clone(),
                });
            }
        }
        out
    }
}

/// The comment box: what it pins, what is typed and the camera it keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct Compose {
    pub pins: Vec<ryucad::proto::PinArg>,
    pub text: String,
    pub camera: Option<ryucad::view::Camera>,
}

/// A pin's point as a request takes it: lengths in meters.
pub fn point_arg(p: [f64; 3]) -> [Value; 3] {
    p.map(|x| Value::from(format!("{x} m")))
}

/// What the tile has typed: a reply per annotation, and an annotation's
/// comment being edited.
#[derive(Clone, Debug, Default)]
pub struct Typed {
    pub replies: BTreeMap<String, String>,
    pub editing: Option<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(id: &str, by: &str, at: &str, status: &str) -> Value {
        json!({
            "id": id, "by": {"participant": by, "kind": "human"}, "at": at,
            "status": status, "claimed_by": null, "pins": [], "comment": format!("about {id}"),
            "view": null, "change": null, "replies": {}, "resolution": null,
            "outdated": false, "stale": false
        })
    }

    fn sample() -> Annotations {
        let mut mine = entry(
            "aaaaaaaaaa",
            "dana (window 1)",
            "2026-11-05T14:00:00Z",
            "open",
        );
        mine["pins"] =
            json!([{"ref": "p/base/0123456789abcdef0123456789abcdef", "point_m": [0.01, 0, 0]}]);
        let mut change = entry(
            "bbbbbbbbbb",
            "dana (window 1)",
            "2026-11-05T15:00:00Z",
            "open",
        );
        change["change"] = json!({"request": {"method": "feature.edit", "params": {}},
                                  "rough": false, "base": {}, "writes": {}, "after": []});
        let mut gone = entry(
            "cccccccccc",
            "erin (window 1)",
            "2026-11-05T13:00:00Z",
            "in_progress",
        );
        gone["claimed_by"] = json!({"participant": "agent", "kind": "agent"});
        gone["pins"] = json!([{"ref": "a/i1/s/0123456789abcdef0123456789abcdef"}]);
        gone["outdated"] = json!(true);
        gone["outdated_pins"] = json!([{"ref": "a/i1/s/0123456789abcdef0123456789abcdef",
                                        "code": "reference_unresolved", "removed_by": "cut1"}]);
        let mut done = entry(
            "dddddddddd",
            "dana (window 1)",
            "2026-11-05T12:00:00Z",
            "done",
        );
        done["resolution"] = json!({"by": {"participant": "agent", "kind": "agent"},
                                    "at": "2026-11-05T16:00:00Z", "note": "done", "applied": false});
        let mut a = Annotations::default();
        a.reset(&json!({"schema": 1, "annotations": [mine, change, gone, done]}));
        a
    }

    #[test]
    fn a_listing_reads_whole_annotations() {
        let a = sample();
        assert_eq!(a.all.len(), 4);
        let gone = &a.all["cccccccccc"];
        assert_eq!(gone.claimed_by.as_deref(), Some("agent"));
        assert_eq!(
            gone.outdated,
            vec!["a/i1/s/0123456789abcdef0123456789abcdef: removed by cut1"]
        );
        assert_eq!(a.all["bbbbbbbbbb"].change.as_deref(), Some("feature.edit"));
        assert_eq!(
            a.all["dddddddddd"]
                .resolution
                .as_ref()
                .unwrap()
                .note
                .as_deref(),
            Some("done")
        );
    }

    #[test]
    fn numbers_mine_and_the_draft() {
        let a = sample();
        // Open ones, oldest first, from 1.
        let n = a.numbers();
        assert_eq!(n["cccccccccc"], 1);
        assert_eq!(n["aaaaaaaaaa"], 2);
        assert_eq!(n["bbbbbbbbbb"], 3);
        assert!(!n.contains_key("dddddddddd"));
        assert_eq!(a.open_mine("dana (window 1)"), 2);
        assert_eq!(a.draft("dana (window 1)"), vec!["bbbbbbbbbb".to_owned()]);
        assert!(a.draft("erin (window 1)").is_empty());
    }

    #[test]
    fn the_tile_shows_open_first_then_newest() {
        let a = sample();
        let ids = |f: &Filter| {
            a.shown(f, "dana (window 1)")
                .iter()
                .map(|e| e.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(&Filter::default()),
            ["bbbbbbbbbb", "aaaaaaaaaa", "cccccccccc"]
        );
        let all = Filter {
            show: Show::All,
            ..Filter::default()
        };
        assert_eq!(
            ids(&all),
            ["bbbbbbbbbb", "aaaaaaaaaa", "cccccccccc", "dddddddddd"]
        );
        let mine = Filter {
            mine: true,
            show: Show::All,
            ..Filter::default()
        };
        assert_eq!(ids(&mine), ["bbbbbbbbbb", "aaaaaaaaaa", "dddddddddd"]);
        let part = Filter {
            part: Some("a".into()),
            ..Filter::default()
        };
        assert_eq!(ids(&part), ["cccccccccc"]);
    }

    #[test]
    fn markers_stand_on_what_is_shown() {
        let a = sample();
        let moved = Pose {
            origin: [1.0, 0.0, 0.0],
            ..Pose::IDENTITY
        };
        let m = a.markers(&["p"], |_| moved);
        assert_eq!(m.len(), 1);
        assert_eq!(
            (m[0].number, m[0].at.clone()),
            (2, At::Point([1.01, 0.0, 0.0]))
        );
        // An occurrence's pin with no point stands on its entity.
        let m = a.markers(&["a"], |_| Pose::IDENTITY);
        assert_eq!(
            m[0].at,
            At::Entity("a/i1/s/0123456789abcdef0123456789abcdef".into())
        );
        assert!(m[0].outdated);
    }

    #[test]
    fn a_change_tells_its_author_of_others_replies_and_closings() {
        let mut a = sample();
        let mut replied = a.all["aaaaaaaaaa"].raw.clone();
        replied["replies"] = json!({
            "r1": {"by": {"participant": "agent", "kind": "agent"}, "at": "2026-11-05T17:00:00Z", "text": "Moved it."},
        });
        replied["status"] = json!("done");
        replied["resolution"] = json!({"by": {"participant": "agent", "kind": "agent"},
                                       "at": "2026-11-05T17:00:00Z", "note": "moved", "applied": false});
        let news = a.changed(
            &json!({"annotations": [replied], "removed": ["cccccccccc"]}),
            "dana (window 1)",
        );
        assert_eq!(news.replies.len(), 1);
        assert_eq!(news.replies[0].1.text, "Moved it.");
        assert_eq!(
            news.closed,
            vec![(
                "aaaaaaaaaa".to_owned(),
                "done".to_owned(),
                Some("moved".to_owned())
            )]
        );
        assert!(!a.all.contains_key("cccccccccc"));
        // Another person's annotation tells this window nothing.
        let mut theirs = entry(
            "eeeeeeeeee",
            "erin (window 1)",
            "2026-11-05T18:00:00Z",
            "open",
        );
        a.changed(&json!({"annotations": [theirs.clone()]}), "dana (window 1)");
        theirs["replies"] = json!({"r1": {"by": {"participant": "agent", "kind": "agent"}, "at": "2026-11-05T18:00:01Z", "text": "ok"}});
        let news = a.changed(&json!({"annotations": [theirs]}), "dana (window 1)");
        assert!(news.replies.is_empty());
    }
}
