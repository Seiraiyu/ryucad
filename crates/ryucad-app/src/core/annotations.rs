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
    /// Its redline marks (design 423), as the file has them, touches and
    /// all; a listing that doesn't hold them reads as none.
    pub marks: Option<ryucad::redline::Marks>,
    /// Each mark's touch that no longer resolves, in words.
    pub outdated_marks: Vec<String>,
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
    /// What a screen reader says of its row (#749): number, author,
    /// state, comment.
    pub fn spoken(&self, number: Option<usize>) -> String {
        let tag = number.map_or_else(|| "·".to_owned(), |n| format!("#{n}"));
        format!(
            "{tag} by {}, {}: {}",
            self.by,
            self.status.replace('_', " "),
            self.comment
        )
    }

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
        // A listing gives marks as `{part, count, kinds, list}`: the file's
        // `{part, list}` and a summary.
        let marks = v["marks"].as_object().and_then(|m| {
            let file = serde_json::json!({"part": m.get("part")?, "list": m.get("list")?});
            ryucad::redline::Marks::from_json(&file, "/marks", true).ok()
        });
        let outdated_marks = v["outdated_marks"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| {
                let why = match o["removed_by"].as_str() {
                    Some(f) => format!("removed by {f}"),
                    None => o["code"].as_str().unwrap_or("unresolved").replace('_', " "),
                };
                format!(
                    "mark {}: {}: {why}",
                    o["mark"].as_u64().map_or(0, |m| m + 1),
                    o["ref"].as_str().unwrap_or_default()
                )
            })
            .collect();
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
            marks,
            outdated_marks,
            raw: v.clone(),
        })
    }

    /// How many marks it carries.
    pub fn mark_count(&self) -> usize {
        self.marks.as_ref().map_or(0, |m| m.list.len())
    }

    /// The plane of the view its marks were drawn in.
    pub fn plane(&self) -> Option<ryucad::redline::Plane> {
        self.view.as_ref().map(ryucad::redline::Plane::of)
    }

    /// Where its first mark's first point is in the shown model's frame: a
    /// view mark's on its plane, a face stroke's placed by `place` (the
    /// pose of the occurrence or solid it names).
    pub fn first_point(&self, place: &impl Fn(&str) -> Pose) -> Option<[f64; 3]> {
        use ryucad::redline::Anchor;
        let m = self.marks.as_ref()?.list.first()?;
        match &m.anchor {
            Anchor::View(xy) => Some(self.plane()?.point(*xy.first()?)),
            Anchor::Face { on, points_m } => Some(place(on).apply(*points_m.first()?)),
        }
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
    /// The annotation chosen in the tile or by its marker.
    pub chosen_annotation: Option<String>,
    /// Annotate's Show agent's model: the working files drawn though the
    /// person has a draft.
    pub show_agent: bool,
    /// The Annotations tile's filter and what is typed in it.
    pub annotation_filter: crate::core::annotations::Filter,
    pub annotation_ui: crate::core::annotations::Typed,
    /// The Markup bar, while it is open (design 423).
    pub markup: Option<crate::core::markup::Markup>,
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
    /// `Model::pose` gives it); an open annotation with marks and no pin
    /// stands at its first mark's first point (design 423, When), a face
    /// stroke's placed by `place` (the pose of what its `on` names).
    pub fn markers(
        &self,
        shown: &[&str],
        pose: impl Fn(&(String, String)) -> Pose,
        place: impl Fn(&str) -> Pose,
        skip: Option<&str>,
    ) -> Vec<Marker> {
        let mut out = Vec::new();
        for (i, e) in self.open_oldest_first().into_iter().enumerate() {
            // The annotation the Markup bar edits is drawn by the bar; the
            // others keep their numbers.
            if Some(e.id.as_str()) == skip {
                continue;
            }
            let marker = |pin: usize, at: At| Marker {
                id: e.id.clone(),
                pin,
                number: i + 1,
                at,
                outdated: !e.outdated.is_empty() || !e.outdated_marks.is_empty(),
                by: e.by.clone(),
                comment: e.comment.clone(),
            };
            if e.pins.is_empty()
                && let Some(m) = &e.marks
                && shown.contains(&m.part.as_str())
                && let Some(p) = e.first_point(&place)
            {
                out.push(marker(0, At::Point(p)));
            }
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
                out.push(marker(k, at));
            }
        }
        out
    }

    /// The open annotations' face strokes on the parts `shown`, each in
    /// its author's colour, placed by `place`: drawn whenever pins are
    /// (design 423, When).
    pub fn face_strokes(
        &self,
        shown: &[&str],
        place: impl Fn(&str) -> Pose,
        skip: Option<&str>,
    ) -> Vec<Stroke> {
        let mut out = Vec::new();
        for e in self.open_oldest_first() {
            // The annotation the Markup bar edits: the bar draws its marks.
            if Some(e.id.as_str()) == skip {
                continue;
            }
            let Some(m) = e
                .marks
                .as_ref()
                .filter(|m| shown.contains(&m.part.as_str()))
            else {
                continue;
            };
            out.extend(face_strokes_of(m, &e.by, &place));
        }
        out
    }
}

/// What a toast says of one of the person's annotations someone closed:
/// `applied` (a suggestion whose change went in), `resolved` or
/// `rejected`, then its comment's start, else the number it had while
/// open (#744), else nothing more.
pub fn closed_text(e: &Entry, number: Option<usize>) -> String {
    let what = match e.status.as_str() {
        "done" if e.resolution.as_ref().is_some_and(|r| r.applied) => "applied",
        "done" => "resolved",
        _ => "rejected",
    };
    let short: String = e.comment.trim().chars().take(40).collect();
    match (short.is_empty(), number) {
        (false, _) => format!("{what} “{short}”"),
        (true, Some(n)) => format!("{what} #{n}"),
        (true, None) => what.to_owned(),
    }
}

/// A face stroke as the window draws it: in the shown model's frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub by: String,
    pub points: Vec<[f64; 3]>,
}

/// `marks`' face strokes, each placed by `place`.
pub fn face_strokes_of(
    marks: &ryucad::redline::Marks,
    by: &str,
    place: &impl Fn(&str) -> Pose,
) -> Vec<Stroke> {
    marks
        .list
        .iter()
        .filter_map(|m| match &m.anchor {
            ryucad::redline::Anchor::Face { on, points_m } => {
                let at = place(on);
                Some(Stroke {
                    by: by.to_owned(),
                    points: points_m.iter().map(|p| at.apply(*p)).collect(),
                })
            }
            ryucad::redline::Anchor::View(_) => None,
        })
        .collect()
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
    fn a_rows_name_says_who_and_what() {
        let e = Entry::from_json(&entry(
            "aaaaaaaaaa",
            "dana (window 1)",
            "2026-11-05T14:00:00Z",
            "in_progress",
        ))
        .unwrap();
        assert_eq!(
            e.spoken(Some(9)),
            "#9 by dana (window 1), in progress: about aaaaaaaaaa"
        );
    }

    #[test]
    fn a_closing_is_told_by_its_comment_else_by_its_number() {
        let mut a = sample();
        let numbers = a.numbers();
        // #744: an applied suggestion without a comment or a note.
        let mut applied = a.all["bbbbbbbbbb"].raw.clone();
        applied["comment"] = json!("");
        applied["status"] = json!("done");
        applied["resolution"] = json!({"by": {"participant": "agent", "kind": "agent"},
                                       "at": "2026-11-05T17:00:00Z", "applied": true});
        let mut rejected = a.all["aaaaaaaaaa"].raw.clone();
        rejected["status"] = json!("rejected");
        rejected["resolution"] = json!({"by": {"participant": "agent", "kind": "agent"},
                                        "at": "2026-11-05T17:00:00Z", "note": "no"});
        let news = a.changed(
            &json!({"annotations": [applied, rejected]}),
            "dana (window 1)",
        );
        assert_eq!(news.closed.len(), 2);
        let text = |id: &str| closed_text(&a.all[id], numbers.get(id).copied());
        assert_eq!(text("bbbbbbbbbb"), "applied #3");
        assert_eq!(text("aaaaaaaaaa"), "rejected “about aaaaaaaaaa”");
        // No number known and no comment: the verb alone, never empty quotes.
        assert_eq!(closed_text(&a.all["bbbbbbbbbb"], None), "applied");
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
        let m = a.markers(&["p"], |_| moved, |_| Pose::IDENTITY, None);
        assert_eq!(m.len(), 1);
        assert_eq!(
            (m[0].number, m[0].at.clone()),
            (2, At::Point([1.01, 0.0, 0.0]))
        );
        // An occurrence's pin with no point stands on its entity.
        let m = a.markers(&["a"], |_| Pose::IDENTITY, |_| Pose::IDENTITY, None);
        assert_eq!(
            m[0].at,
            At::Entity("a/i1/s/0123456789abcdef0123456789abcdef".into())
        );
        assert!(m[0].outdated);
    }

    /// An annotation with marks and no pin, as a listing gives it.
    fn marked() -> Value {
        let mut e = entry(
            "ffffffffff",
            "dana (window 1)",
            "2026-11-05T19:00:00Z",
            "open",
        );
        e["view"] = json!({"eye_m": [0, -1, 0], "target_m": [0, 0, 0], "up": [0, 0, 1],
                           "projection": "orthographic", "height_m": 0.1});
        e["marks"] = json!({"part": "p", "count": 2, "kinds": ["arrow", "stroke"], "list": [
            {"kind": "arrow", "in": "view", "xy_m": [[0.01, 0.02], [0.03, 0.02]],
             "touches": [{"at": "tip", "ref": "p/base/0123456789abcdef0123456789abcdef"}]},
            {"kind": "stroke", "in": "face", "on": "p/base",
             "points_m": [[0, 0, 0.005], [0.01, 0, 0.005]], "touches": []}]});
        e["outdated"] = json!(true);
        e["outdated_marks"] = json!([{"mark": 0, "touch": 0, "at": "tip",
            "ref": "p/base/0123456789abcdef0123456789abcdef", "code": "reference_unresolved",
            "removed_by": "cut1"}]);
        e
    }

    #[test]
    fn marks_are_read_and_stand_where_they_were_drawn() {
        let mut a = Annotations::default();
        a.reset(&json!({"schema": 1, "annotations": [marked()]}));
        let e = &a.all["ffffffffff"];
        assert_eq!(e.mark_count(), 2);
        assert_eq!(
            e.outdated_marks,
            ["mark 1: p/base/0123456789abcdef0123456789abcdef: removed by cut1"]
        );
        // Seen from -y, the plane's x is the model's x and its y the
        // model's z: the arrow's tail is at (0.01, 0, 0.02).
        let at = e.first_point(&|_| Pose::IDENTITY).unwrap();
        assert!(
            at.iter()
                .zip([0.01, 0.0, 0.02])
                .all(|(a, b)| (a - b).abs() < 1e-12)
        );
        // No pin: its marker stands at its first mark, outdated by a touch.
        let m = a.markers(&["p"], |_| Pose::IDENTITY, |_| Pose::IDENTITY, None);
        assert_eq!(m.len(), 1);
        assert!(m[0].outdated);
        assert!(
            a.markers(&["q"], |_| Pose::IDENTITY, |_| Pose::IDENTITY, None)
                .is_empty()
        );
        // Its face stroke, placed by what it was drawn on.
        let up = Pose {
            origin: [0.0, 0.0, 1.0],
            ..Pose::IDENTITY
        };
        let strokes = a.face_strokes(
            &["p"],
            |on| {
                assert_eq!(on, "p/base");
                up
            },
            None,
        );
        assert_eq!(
            strokes,
            vec![Stroke {
                by: "dana (window 1)".into(),
                points: vec![[0.0, 0.0, 1.005], [0.01, 0.0, 1.005]],
            }]
        );
        // While the Markup bar edits it, the bar draws its marks: the
        // listing's are left out, stroke and marker.
        let editing = Some("ffffffffff");
        assert!(a.face_strokes(&["p"], |_| up, editing).is_empty());
        assert!(a.markers(&["p"], |_| up, |_| up, editing).is_empty());
        // Closed, it draws no stroke and stands at no marker.
        let mut done = marked();
        done["status"] = json!("done");
        a.changed(&json!({"annotations": [done]}), "dana (window 1)");
        assert!(a.face_strokes(&["p"], |_| up, None).is_empty());
        assert!(a.markers(&["p"], |_| up, |_| up, None).is_empty());
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
