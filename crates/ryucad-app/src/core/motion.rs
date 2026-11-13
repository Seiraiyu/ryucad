//! The window's motions, apart from egui (design 529, The app): the
//! motions of the assembly it shows (`motion.list`), the one its
//! participant shows (`participant.changed`) and how far through, the
//! frames fetched once per motion and assembly revision
//! (`motion.frames`), from which a slider or Play moves the model at
//! once ([`Frames::offsets`]); the last sweep's marks on the slider and
//! its collisions; and each drive's range as edited.

use ryucad::explode::Offset;
use ryucad::playback::Frame;
use ryucad::proto::Request;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

/// How long Play takes from start to end (and as long back).
pub const PERIOD: Duration = Duration::from_secs(4);

/// The motion a participant shows: an assembly's, how far through, and
/// whether it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub part: String,
    pub motion: String,
    pub fraction: f64,
    pub playing: bool,
}

impl Shown {
    /// As `participant.changed` gives it; `None` for none.
    pub fn from_json(v: &Value) -> Option<Shown> {
        Some(Shown {
            part: v["part"].as_str()?.to_owned(),
            motion: v["motion"].as_str()?.to_owned(),
            fraction: v["fraction"].as_f64()?,
            playing: v["playing"].as_bool().unwrap_or(false),
        })
    }
}

/// One motion as `motion.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    /// Its drives as the file writes them.
    pub drives: Value,
    pub steps: u64,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

/// A motion's frames (`motion.frames`): its assembly, ID and the
/// assembly's revision they were solved at, and the frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    pub part: String,
    pub motion: String,
    pub revision: String,
    pub frames: Vec<Frame>,
}

impl Frames {
    pub fn from_json(v: &Value) -> Option<Frames> {
        Some(Frames {
            part: v["part"].as_str()?.to_owned(),
            motion: v["motion"].as_str()?.to_owned(),
            revision: v["revision"].as_str()?.to_owned(),
            frames: v["frames"]
                .as_array()?
                .iter()
                .map(Frame::from_json)
                .collect::<Option<Vec<_>>>()?,
        })
    }

    /// The frame nearest `fraction` (clamped to `[0, 1]`).
    pub fn index_at(&self, fraction: f64) -> usize {
        let n = self.frames.len().max(1);
        let f = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0)
        };
        ((f * (n - 1) as f64).round() as usize).min(n - 1)
    }

    pub fn at(&self, fraction: f64) -> Option<&Frame> {
        self.frames.get(self.index_at(fraction))
    }

    /// Where the frame nearest `fraction` draws each occurrence of
    /// `paths` that it moves, by path.
    pub fn offsets<'a>(
        &self,
        fraction: f64,
        paths: impl Iterator<Item = &'a str>,
    ) -> BTreeMap<String, Offset> {
        let Some(f) = self.at(fraction) else {
            return BTreeMap::new();
        };
        paths
            .filter_map(|p| Some((p.to_owned(), *f.offset_of(p)?)))
            .collect()
    }

    /// Each drive's value at the frame nearest `fraction`, as the tile
    /// lists it: `hinge: 4.00°`, `drives/1: 12.5 mm`.
    pub fn values_text(&self, fraction: f64) -> Vec<String> {
        let Some(f) = self.at(fraction) else {
            return Vec::new();
        };
        f.values
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| {
                let mut parts = Vec::new();
                if let Some(a) = v["angle_deg"].as_f64() {
                    parts.push(format!("{a:.2}°"));
                }
                if let Some(d) = v["distance"].as_str() {
                    parts.push(d.to_owned());
                }
                format!("{k}: {}", parts.join(", "))
            })
            .collect()
    }
}

/// Where Play is, `elapsed` after it started from `start`: forward to the
/// end, back to the start, and again, [`PERIOD`] each way.
pub fn ping_pong(start: f64, elapsed: Duration) -> f64 {
    let u = (start.clamp(0.0, 1.0) + elapsed.as_secs_f64() / PERIOD.as_secs_f64()) % 2.0;
    if u <= 1.0 { u } else { 2.0 - u }
}

/// A mark the last sweep puts on the slider: a colliding step (red), a
/// blocked one (grey), and the refined contact (a line).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Contact(f64),
    Collision(f64),
    Blocked(f64),
}

/// A pair a sweep found colliding: first where (`t`, the refined contact
/// for the first collision), and the faces both sides touch there.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepPair {
    pub a: String,
    pub b: String,
    pub t: f64,
    pub faces: Vec<String>,
}

impl SweepPair {
    /// As the tile lists it: both solids under the assembly.
    pub fn label(&self, part: &str) -> String {
        let short = |s: &str| {
            s.strip_prefix(part)
                .and_then(|r| r.strip_prefix('/'))
                .unwrap_or(s)
                .to_owned()
        };
        format!("{} and {}", short(&self.a), short(&self.b))
    }
}

/// The last sweep of a motion (`assembly.sweep` with interference):
/// each step's fraction, whether it held and the faces its collisions
/// touch; the refined contact; each colliding pair. Stale once the
/// assembly changes: still listed, painted no more.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sweep {
    pub part: String,
    pub motion: String,
    pub loaded: bool,
    pub error: Option<String>,
    pub stale: bool,
    pub steps: Vec<(f64, bool, Vec<String>)>,
    pub contact: Option<f64>,
    pub pairs: Vec<SweepPair>,
}

impl Sweep {
    /// Asked for `motion` of `part`; the answer hasn't come.
    pub fn asked(part: &str, motion: &str) -> Sweep {
        Sweep {
            part: part.to_owned(),
            motion: motion.to_owned(),
            ..Sweep::default()
        }
    }

    pub fn from_json(v: &Value) -> Sweep {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
        let faces = |c: &Value| -> Vec<String> {
            ["faces_a", "faces_b"]
                .iter()
                .flat_map(|k| c[*k].as_array().into_iter().flatten())
                .map(text)
                .collect()
        };
        let contact = v["first_collision"]["contact"]["t"].as_f64();
        let first = (
            text(&v["first_collision"]["pair"][0]),
            text(&v["first_collision"]["pair"][1]),
        );
        let mut steps = Vec::new();
        let mut pairs: Vec<SweepPair> = Vec::new();
        for s in v["steps"].as_array().into_iter().flatten() {
            let t = s["t"].as_f64().unwrap_or_default();
            let mut touched = Vec::new();
            for c in s["collisions"].as_array().into_iter().flatten() {
                let (a, b) = (text(&c["pair"][0]), text(&c["pair"][1]));
                let f = faces(c);
                touched.extend(f.iter().cloned());
                if !pairs.iter().any(|p| p.a == a && p.b == b) {
                    let at = match contact {
                        Some(x) if (a.clone(), b.clone()) == first => x,
                        _ => t,
                    };
                    pairs.push(SweepPair {
                        a,
                        b,
                        t: at,
                        faces: f,
                    });
                }
            }
            steps.push((t, s["state"] == "holds", touched));
        }
        Sweep {
            part: text(&v["part"]),
            motion: text(&v["motion"]),
            loaded: true,
            error: None,
            stale: false,
            steps,
            contact,
            pairs,
        }
    }

    /// The slider's marks, in order along it.
    pub fn marks(&self) -> Vec<Mark> {
        let mut out: Vec<Mark> = self.contact.into_iter().map(Mark::Contact).collect();
        for (t, holds, faces) in &self.steps {
            if !holds {
                out.push(Mark::Blocked(*t));
            } else if !faces.is_empty() {
                out.push(Mark::Collision(*t));
            }
        }
        let at = |m: &Mark| match *m {
            Mark::Contact(t) | Mark::Collision(t) | Mark::Blocked(t) => t,
        };
        out.sort_by(|x, y| at(x).total_cmp(&at(y)));
        out
    }

    /// The faces to paint at `fraction`: those the step nearest it
    /// touches; none while stale.
    pub fn faces_at(&self, fraction: f64) -> Vec<String> {
        if self.stale {
            return Vec::new();
        }
        self.steps
            .iter()
            .min_by(|x, y| (x.0 - fraction).abs().total_cmp(&(y.0 - fraction).abs()))
            .map(|(_, _, f)| f.clone())
            .unwrap_or_default()
    }
}

/// A drive's range as the tile edits it: drive `drive`'s `key` (`angle`,
/// `distance`, `rotate`, `translate`) from `from` to `to`, as the file
/// writes them.
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub drive: usize,
    pub key: &'static str,
    pub from: String,
    pub to: String,
}

/// Every range of `drives` (as a motion file writes them), in order.
pub fn ranges(drives: &Value) -> Vec<Range> {
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let mut out = Vec::new();
    for (i, d) in drives.as_array().into_iter().flatten().enumerate() {
        for key in ["angle", "distance", "rotate", "translate"] {
            if d[key].is_object() {
                out.push(Range {
                    drive: i,
                    key,
                    from: text(&d[key]["from"]),
                    to: text(&d[key]["to"]),
                });
            }
        }
    }
    out
}

/// `motion.save` of motion `id` of `part`: `drives` with each of
/// `ranges` laid over them, and `steps` as typed; `Err` with the reason
/// when the steps aren't a whole number.
pub fn save(
    part: &str,
    id: &str,
    drives: &Value,
    ranges: &[Range],
    steps: &str,
    revision: Option<String>,
) -> Result<Request, String> {
    let steps: u32 = steps
        .trim()
        .parse()
        .map_err(|_| format!("steps is a whole number, 2 to 1,000: {steps:?}"))?;
    let mut drives = drives.clone();
    for r in ranges {
        let at = &mut drives[r.drive][r.key];
        at["from"] = json!(r.from.trim());
        at["to"] = json!(r.to.trim());
    }
    Ok(Request::MotionSave {
        part: part.to_owned(),
        motion: id.to_owned(),
        drives,
        steps: Some(steps),
        expected_revision: revision,
    })
}

/// What the Motion tile and the viewport know.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Motions {
    /// The assembly listed, and its revision when listed.
    pub part: Option<String>,
    pub revision: Option<String>,
    pub list: Vec<Entry>,
    /// What the participant shows.
    pub shown: Option<Shown>,
    /// The shown motion's frames, once fetched.
    pub frames: Option<Frames>,
    /// A fraction the slider holds until the session says it.
    pub sliding: Option<f64>,
    /// Where Play started: when, and from what fraction.
    pub started: Option<(std::time::Instant, f64)>,
    /// The last sweep.
    pub sweep: Option<Sweep>,
    /// The shown motion's ranges and steps as typed, and which motion
    /// they are of.
    pub editing: Option<(String, Vec<Range>, String)>,
}

impl Motions {
    /// Takes a `motion.list` answer for `part`.
    pub fn listed(&mut self, part: &str, v: &Value) {
        self.part = Some(part.to_owned());
        self.revision = v["revisions"][part].as_str().map(str::to_owned);
        self.list = v["motions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| Entry {
                id: e["motion"].as_str().unwrap_or_default().to_owned(),
                drives: e["drives"].clone(),
                steps: e["steps"].as_u64().unwrap_or(11),
                warnings: e["warnings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| w["message"].as_str().map(str::to_owned))
                    .collect(),
                error: e["error"]["message"].as_str().map(str::to_owned),
            })
            .collect();
    }

    /// The shown motion of `part`, if it is shown and listed.
    pub fn current(&self, part: &str) -> Option<&Entry> {
        let s = self.shown.as_ref().filter(|s| s.part == part)?;
        self.list.iter().find(|e| e.id == s.motion)
    }

    /// The fraction drawn now at `now`: Play's, else the slider's, else
    /// the session's.
    pub fn fraction(&self, now: std::time::Instant) -> Option<f64> {
        let s = self.shown.as_ref()?;
        if let Some((at, from)) = self.started {
            return Some(ping_pong(from, now.saturating_duration_since(at)));
        }
        Some(self.sliding.unwrap_or(s.fraction))
    }

    /// Whether the window plays the motion now.
    pub fn playing(&self) -> bool {
        self.started.is_some()
    }

    /// Whether the frames held are the shown motion's at the listed
    /// revision.
    pub fn frames_current(&self) -> bool {
        match (&self.shown, &self.frames) {
            (Some(s), Some(f)) => {
                f.part == s.part
                    && f.motion == s.motion
                    && self.revision.as_deref().is_none_or(|r| r == f.revision)
            }
            _ => false,
        }
    }

    /// Where the shown motion draws each of `paths` of `part` at `now`.
    pub fn offsets<'a>(
        &self,
        part: &str,
        paths: impl Iterator<Item = &'a str>,
        now: std::time::Instant,
    ) -> BTreeMap<String, Offset> {
        match (&self.frames, self.fraction(now)) {
            (Some(f), Some(x)) if self.frames_current() && f.part == part => f.offsets(x, paths),
            _ => BTreeMap::new(),
        }
    }

    /// The last sweep's faces at the fraction drawn, when it is the shown
    /// motion's.
    pub fn collision_faces(&self, now: std::time::Instant) -> Vec<String> {
        match (&self.sweep, &self.shown, self.fraction(now)) {
            (Some(w), Some(s), Some(x)) if w.part == s.part && w.motion == s.motion => {
                w.faces_at(x)
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Frames of `two` lifted 0 to 30 mm along z, 4 frames, the last
    /// blocked (kept where the third left it).
    fn frames() -> Frames {
        let off = |z: f64| json!({"origin_m": [0.0, 0.0, z], "z": [0, 0, 1], "x": [1, 0, 0]});
        Frames::from_json(&json!({"part": "asm", "motion": "lift", "revision": "r1", "frames": [
            {"index": 0, "t": 0.0, "state": "holds", "values": {"drives/0": {"distance": "0 mm"}},
             "offsets": {}},
            {"index": 1, "t": 0.333, "state": "holds", "values": {"drives/0": {"distance": "10 mm"}},
             "offsets": {"two": off(0.01)}},
            {"index": 2, "t": 0.667, "state": "holds", "values": {"drives/0": {"distance": "20 mm"}},
             "offsets": {"two": off(0.02)}},
            {"index": 3, "t": 1.0, "state": "blocked", "values": {"drives/0": {"distance": "30 mm"}},
             "offsets": {"two": off(0.02)}}]}))
        .unwrap()
    }

    #[test]
    fn frames_move_occurrences_under_their_body_and_the_slider_picks_one() {
        let f = frames();
        assert_eq!(
            (f.part.as_str(), f.motion.as_str(), f.revision.as_str()),
            ("asm", "lift", "r1")
        );
        assert_eq!(f.index_at(0.0), 0);
        assert_eq!(
            f.index_at(0.5),
            2,
            "0.5 of 3 rounds to 2 (1.5 rounds away from zero)"
        );
        assert_eq!(f.index_at(0.4), 1);
        assert_eq!(f.index_at(1.0), 3);
        assert_eq!(f.index_at(7.0), 3);
        let paths = ["one/base", "two/base", "two/inner/deep"];
        let at = f.offsets(0.4, paths.iter().copied());
        assert_eq!(at.len(), 2, "both occurrences under two, nothing of one");
        assert_eq!(at["two/inner/deep"].apply([0.0; 3]), [0.0, 0.0, 0.01]);
        assert!(f.offsets(0.0, paths.iter().copied()).is_empty());
        assert!(!f.at(1.0).unwrap().holds);
        assert_eq!(f.values_text(0.4), ["drives/0: 10 mm"]);
    }

    #[test]
    fn play_goes_back_and_forth() {
        use std::time::Duration;
        let p = PERIOD.as_secs_f64();
        let at = |start: f64, secs: f64| ping_pong(start, Duration::from_secs_f64(secs));
        assert!((at(0.0, 0.5 * p) - 0.5).abs() < 1e-9);
        assert!((at(0.0, 1.5 * p) - 0.5).abs() < 1e-9, "on its way back");
        assert!((at(0.8, 0.4 * p) - 0.8).abs() < 1e-9, "to the end and back");
        assert!((at(0.0, 2.0 * p)).abs() < 1e-9, "a whole loop");
        assert_eq!(at(1.0, 0.0), 1.0);
    }

    #[test]
    fn a_sweep_marks_the_slider_and_lists_its_collisions() {
        let face = |s: &str| json!([format!("asm/{s}/body/aa")]);
        let s = Sweep::from_json(&json!({"part": "asm", "motion": "lift", "revision": "r1",
            "steps": [
              {"index": 0, "t": 0.0, "state": "holds", "collisions": []},
              {"index": 1, "t": 0.5, "state": "holds", "collisions": [
                  {"pair": ["asm/two/body", "asm/lid/body"], "volume_m3": 1e-9,
                   "faces_a": face("two"), "faces_b": face("lid")}]},
              {"index": 2, "t": 1.0, "state": "blocked", "conflicting": [["m1"]]}],
            "first_collision": {"index": 1, "t": 0.5, "pair": ["asm/two/body", "asm/lid/body"],
                                "contact": {"t": 0.4375, "bracket": [0.43, 0.4375]}},
            "first_blocked": {"index": 2}}));
        assert_eq!(
            s.marks(),
            [
                Mark::Contact(0.4375),
                Mark::Collision(0.5),
                Mark::Blocked(1.0)
            ]
        );
        assert_eq!(s.pairs.len(), 1);
        let p = &s.pairs[0];
        assert_eq!(p.label("asm"), "two/body and lid/body");
        assert_eq!(p.t, 0.4375, "shown at the refined contact");
        assert_eq!(p.faces, ["asm/two/body/aa", "asm/lid/body/aa"]);
        // The faces painted are those of the step nearest the slider.
        assert_eq!(s.faces_at(0.55), ["asm/two/body/aa", "asm/lid/body/aa"]);
        assert!(s.faces_at(0.1).is_empty());
        let mut stale = s.clone();
        stale.stale = true;
        assert!(stale.faces_at(0.5).is_empty());
    }

    #[test]
    fn ranges_are_edited_and_saved_as_the_file_writes_them() {
        let drives = json!([
            {"mate": "hinge", "angle": {"from": "-6 deg", "to": "6 deg"}},
            {"instances": ["p1"], "rotate": {"axis": {"origin": [0, 0, 0], "direction": "z"},
                                              "from": "0 deg", "to": "90 deg"},
             "translate": {"direction": "z", "from": "0 mm", "to": "5 mm"}}]);
        let mut r = ranges(&drives);
        assert_eq!(
            r.iter()
                .map(|x| (x.drive, x.key, x.from.as_str(), x.to.as_str()))
                .collect::<Vec<_>>(),
            [
                (0, "angle", "-6 deg", "6 deg"),
                (1, "rotate", "0 deg", "90 deg"),
                (1, "translate", "0 mm", "5 mm")
            ]
        );
        r[1].to = "45 deg".into();
        let req = save("asm", "twist", &drives, &r, "13", Some("r1".into())).unwrap();
        let p = req.params();
        assert_eq!(p["drives"][1]["rotate"]["to"], "45 deg");
        assert_eq!(
            p["drives"][1]["rotate"]["axis"]["direction"], "z",
            "the rest kept"
        );
        assert_eq!(p["steps"], 13);
        assert_eq!(p["expected_revision"], "r1");
        assert!(save("asm", "twist", &drives, &r, "lots", None).is_err());
    }
}
