//! Following the agents' work (#812, design 698 phase 7): with View >
//! Follow agents' work on (`view.follow_work`), the window keeps what
//! agents change in frame. Each change an agent makes, and each change of
//! what it selects and highlights, is a mark; the work box is the union
//! of the marks of the last [`WINDOW`], found again each frame from the
//! window's meshes and sketches as they come. When the box isn't inside
//! the view's inner [`INNER`], the camera eases out to take it in with
//! what is in view, keeping its direction and never zooming in, at most
//! once per `daemon.publish_max_ms`. A hand on the camera pauses it for
//! [`PAUSE`]; following a participant's camera, and the first framing of
//! a model (#673), come first. Pure: the viewport calls it.

use crate::core::model::{BodyKey, Model, Occurrence};
use crate::core::nav::Projector;
use crate::core::people::Person;
use crate::core::pick::Picker;
use crate::core::toast::{features_of, parts_of};
use ryucad::live::Kind;
use ryucad::view::Camera;
use serde_json::Value;
use std::time::{Duration, Instant};

/// How long a change counts as the agents' work: the window frames the
/// union of the last 5 s.
pub const WINDOW: Duration = Duration::from_secs(5);

/// How long a hand on the camera pauses following the work.
pub const PAUSE: Duration = Duration::from_secs(3);

/// The middle share of the view, each way, the work must lie in for the
/// camera to stay where it is.
pub const INNER: f64 = 0.9;

/// How often the camera may ease when `daemon.publish_max_ms` isn't
/// known: its default (`ryu_session::publish::MAX_MS`).
pub const EVERY: Duration = Duration::from_millis(1000);

/// A box in model space: its lowest and highest corners.
pub type Bounds = ([f64; 3], [f64; 3]);

/// What one mark names.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A part's feature: the solids it founds or acts on, wherever the
    /// window shows them, or the sketch it is.
    Feature(String, String),
    /// A part whose files changed but no feature's (its parameters, its
    /// properties): all its bodies.
    Part(String),
    /// What an agent selects and highlights: entities, and occurrences
    /// (`<asm>/<path>`) whole.
    Entities(Vec<String>),
}

/// One agent change, and the box it was last found to have.
#[derive(Clone, Debug)]
struct Mark {
    targets: Vec<Target>,
    /// When it last counted from: when it came, or when its box last
    /// changed (its meshes came after it).
    at: Instant,
    seen: Option<Bounds>,
}

/// The agents' recent work, a hand's pause and the last ease.
#[derive(Debug, Default)]
pub struct Work {
    marks: Vec<Mark>,
    paused_until: Option<Instant>,
    eased_at: Option<Instant>,
}

/// What [`Work::frame`] judges by: the camera (where a turn under way is
/// going, if one is), the viewport's aspect, whether the window follows a
/// participant's camera, and how often it may ease.
pub struct Sight<'a> {
    pub camera: &'a Camera,
    pub aspect: f64,
    pub following: bool,
    pub every: Duration,
}

impl Work {
    /// Takes an `activity.appended` entry: an agent's change, undo or redo
    /// is a mark of the features and parts its files are. Anyone else's
    /// (a person's, or a change made outside) isn't.
    pub fn activity(&mut self, entry: &Value, now: Instant) {
        if entry["participant"]["kind"] != "agent" {
            return;
        }
        let features = features_of(entry);
        let mut targets: Vec<Target> = features
            .iter()
            .map(|(p, f)| Target::Feature(p.clone(), f.clone()))
            .collect();
        targets.extend(
            parts_of(entry)
                .into_iter()
                .filter(|p| !features.iter().any(|(q, _)| q == p))
                .map(Target::Part),
        );
        self.mark(targets, now);
    }

    /// Participant `after` changed; `before` is what it pointed at before
    /// ([`pointing`]). An agent pointing at something new is a mark.
    pub fn pointed(&mut self, before: &[String], after: &Person, now: Instant) {
        let now_at = pointing(after);
        if after.kind != Kind::Agent || now_at.is_empty() || now_at == before {
            return;
        }
        self.mark(vec![Target::Entities(now_at)], now);
    }

    fn mark(&mut self, targets: Vec<Target>, now: Instant) {
        if !targets.is_empty() {
            self.marks.push(Mark {
                targets,
                at: now,
                seen: None,
            });
        }
    }

    /// A hand moved the camera: following the work pauses for [`PAUSE`].
    pub fn hand(&mut self, now: Instant) {
        self.paused_until = Some(now + PAUSE);
    }

    /// Whether there is work to keep in frame.
    pub fn pending(&self) -> bool {
        !self.marks.is_empty()
    }

    /// The work box now: the union of the boxes of the marks that still
    /// count, each found again from `model` and `picker`. A mark whose box
    /// changed counts again from `now`; one that hasn't for [`WINDOW`] goes.
    pub fn bounds(&mut self, now: Instant, model: &Model, picker: &Picker) -> Option<Bounds> {
        for m in &mut self.marks {
            let b = union_all(m.targets.iter().filter_map(|t| boxed(model, picker, t)));
            if b.is_some() && b != m.seen {
                m.seen = b;
                m.at = now;
            }
        }
        self.marks.retain(|m| now < m.at + WINDOW);
        self.marks.iter().fold(None, |all, m| union(all, m.seen))
    }

    /// Where the camera should ease to now, if anywhere: when `work` isn't
    /// inside the view's inner [`INNER`], the view that takes in the work
    /// and what is in view, in the same direction. Not while the window
    /// follows a participant's camera, within [`PAUSE`] of a hand, or
    /// within `sight.every` of the last ease (a burst eases once per
    /// publish).
    pub fn frame(&mut self, now: Instant, sight: &Sight, work: Option<Bounds>) -> Option<Camera> {
        let (lo, hi) = work?;
        let paused = self.paused_until.is_some_and(|t| now < t);
        let eased = self.eased_at.is_some_and(|t| now < t + sight.every);
        if sight.following || paused || eased || in_view(sight.camera, sight.aspect, lo, hi, INNER)
        {
            return None;
        }
        let mut points: Vec<[f64; 3]> = corners(lo, hi).to_vec();
        points.extend(view_corners(sight.camera, sight.aspect));
        let (plo, phi) = union_all(points.iter().map(|p| (*p, *p))).expect("twelve points");
        let middle = [0, 1, 2].map(|k| (plo[k] + phi[k]) / 2.0);
        let mut to = ryucad::view::refit_points(sight.camera, &points, sight.aspect);
        // The work inside the inner share, or the next publish would ease
        // out again: spread the points from their middle until it is.
        for _ in 0..4 {
            if in_view(&to, sight.aspect, lo, hi, INNER) {
                break;
            }
            for p in &mut points {
                *p = [0, 1, 2].map(|k| middle[k] + (p[k] - middle[k]) / INNER);
            }
            to = ryucad::view::refit_points(sight.camera, &points, sight.aspect);
        }
        self.eased_at = Some(now);
        Some(to)
    }

    /// How long until [`Work::frame`] may move the camera again while
    /// there is work: the window looks again then. `None` with no work.
    pub fn wake_in(&self, now: Instant, every: Duration) -> Option<Duration> {
        if self.marks.is_empty() {
            return None;
        }
        let later = [self.paused_until, self.eased_at.map(|t| t + every)]
            .into_iter()
            .flatten()
            .filter(|t| *t > now)
            .max();
        Some(later.map_or(Duration::from_millis(100), |t| t - now))
    }
}

/// What a participant points at: its selection, then its highlights'
/// entities, each once.
pub fn pointing(p: &Person) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in p
        .selection
        .iter()
        .chain(p.highlights.iter().flat_map(|h| h.entities.iter()))
    {
        if !out.contains(e) {
            out.push(e.clone());
        }
    }
    out
}

/// Whether the box `lo`..`hi` lies wholly inside the middle `share` of
/// the view each way (every corner in front of the eye).
pub fn in_view(camera: &Camera, aspect: f64, lo: [f64; 3], hi: [f64; 3], share: f64) -> bool {
    // The depth range doesn't matter to `ndc`.
    let pr = Projector::new(camera, aspect, camera.target, 0.0);
    corners(lo, hi).iter().all(|p| {
        pr.ndc(*p)
            .is_some_and(|n| n[0].abs() <= share && n[1].abs() <= share)
    })
}

/// The corners of what the view shows at its target's depth.
pub fn view_corners(camera: &Camera, aspect: f64) -> [[f64; 3]; 4] {
    let (r, u, _) = camera.basis();
    let hh = camera.half_height(camera.distance());
    let hw = hh * aspect;
    let at = |a: f64, b: f64| [0, 1, 2].map(|k| camera.target[k] + r[k] * a + u[k] * b);
    [at(-hw, -hh), at(hw, -hh), at(hw, hh), at(-hw, hh)]
}

/// The bodies and sketches a target names, by key, as the window shows
/// them: in the part itself, and in every shown assembly that places it.
/// Keys the window doesn't draw are harmless: they have no box.
pub fn bodies(model: &Model, t: &Target) -> Vec<BodyKey> {
    let mut out: Vec<BodyKey> = Vec::new();
    match t {
        Target::Feature(part, feature) => {
            // A sketch is drawn under its own key.
            out.push((part.clone(), feature.clone()));
            // The solids it founds or acts on; an assembly's instance's
            // solids are its occurrences' (`<path>/<founder>`).
            let founders: Option<Vec<String>> = model.parts.get(part).map(|p| {
                p.solids_of(feature)
                    .into_iter()
                    .map(|s| s.id.clone())
                    .collect()
            });
            out.extend(founders.iter().flatten().map(|s| (part.clone(), s.clone())));
            for (asm, o) in placed(model, part) {
                let names: Vec<String> = match founders.as_ref().filter(|f| !f.is_empty()) {
                    Some(f) => f.clone(),
                    // The part isn't loaded, or the feature founds nothing:
                    // the whole occurrence.
                    None => o.solids.iter().map(|(s, _)| s.clone()).collect(),
                };
                out.extend(
                    names
                        .into_iter()
                        .map(|s| (asm.to_owned(), format!("{}/{s}", o.path))),
                );
            }
        }
        Target::Part(part) => {
            if let Some(p) = model.parts.get(part) {
                out.extend(p.solids.iter().map(|s| (part.clone(), s.id.clone())));
            }
            for (asm, o) in placed(model, part) {
                out.extend(
                    o.solids
                        .iter()
                        .map(|(s, _)| (asm.to_owned(), format!("{}/{s}", o.path))),
                );
            }
        }
        Target::Entities(refs) => {
            for r in refs {
                out.extend(model.occurrence_bodies(r));
            }
        }
    }
    out
}

/// The occurrences of `part` in the assemblies the window shows, each
/// with its assembly.

fn placed<'a>(model: &'a Model, part: &str) -> Vec<(&'a str, &'a Occurrence)> {
    let mut out = Vec::new();
    for a in model.shown() {
        let Some(p) = model.parts.get(a).filter(|p| p.assembly && p.id != part) else {
            continue;
        };
        out.extend(
            p.occurrences
                .iter()
                .filter(|o| o.part == part)
                .map(|o| (p.id.as_str(), o)),
        );
    }
    out
}

/// A target's box: its bodies' and sketches', and its entities' own.

fn boxed(model: &Model, picker: &Picker, t: &Target) -> Option<Bounds> {
    let mut b = union_all(
        bodies(model, t)
            .iter()
            .filter_map(|k| picker.body_bounds(k)),
    );
    if let Target::Entities(refs) = t {
        b = union(b, picker.bounds_of(refs));
    }
    b
}

fn union(a: Option<Bounds>, b: Option<Bounds>) -> Option<Bounds> {
    match (a, b) {
        (Some((l1, h1)), Some((l2, h2))) => Some((
            [0, 1, 2].map(|k| l1[k].min(l2[k])),
            [0, 1, 2].map(|k| h1[k].max(h2[k])),
        )),
        (a, None) => a,
        (None, b) => b,
    }
}

fn union_all(boxes: impl IntoIterator<Item = Bounds>) -> Option<Bounds> {
    boxes.into_iter().fold(None, |all, b| union(all, Some(b)))
}

fn corners(lo: [f64; 3], hi: [f64; 3]) -> [[f64; 3]; 8] {
    core::array::from_fn(|i| [0, 1, 2].map(|k| if i >> k & 1 == 1 { hi[k] } else { lo[k] }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::mesh::{WireBody, WireMesh};
    use serde_json::json;

    /// A 1 cm cube's corners (no faces: only its box matters here).
    fn cube() -> WireBody {
        WireBody {
            feature: "c0/s".into(),
            hash: "h".into(),
            mesh: Ok(WireMesh {
                origin: [0.0; 3],
                chord: 0.0,
                angle: 0.0,
                positions: (0..8)
                    .map(|i| [0, 1, 2].map(|k| if i >> k & 1 == 1 { 0.01f32 } else { 0.0 }))
                    .collect(),
                corners: Vec::new(),
                triangles: Vec::new(),
                faces: Vec::new(),
                edges: Vec::new(),
                vertices: Vec::new(),
                faults: Vec::new(),
            }),
        }
    }

    /// The assembly `row` (shown: the project's one root) of two
    /// instances of part `cube` (one solid `s`), `c0` at x = 0 and `c1`
    /// at x = 1 m, their meshes fetched when `meshed`.
    fn row(meshed: bool) -> Model {
        let occurrences: Vec<Value> = [0.0, 1.0]
            .iter()
            .enumerate()
            .map(|(i, x)| {
                json!({"path": format!("c{i}"), "part": "cube",
                       "solids": [{"solid": "s", "hash": "h"}],
                       "pose": {"origin_m": [x, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}})
            })
            .collect();
        let mut m = Model::default();
        let want = m.apply_part(&json!({"part": "row", "revision": "r", "kind": "assembly",
            "features": [], "solids": [], "occurrences": occurrences}));
        m.ask(want);
        if meshed {
            assert!(m.take_mesh("row", cube()));
        }
        m
    }

    fn entry(kind: &str, files: &[&str]) -> Value {
        let files: Vec<Value> = files.iter().map(|p| json!({"path": p})).collect();
        json!({"seq": 1, "kind": "change", "method": "feature.edit", "undone_by": null,
               "participant": {"id": "p-1", "kind": kind, "name": kind}, "files": files})
    }

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-9)
    }

    #[test]
    fn an_agents_instance_edit_is_boxed_from_its_occurrence() {
        let m = row(true);
        let picker = Picker::new(&m);
        let t0 = Instant::now();
        let mut w = Work::default();
        // A person's change, or one made outside, is nobody's work to follow.
        w.activity(&entry("human", &["parts/row/features/c1.json"]), t0);
        w.activity(
            &json!({"kind": "external", "participant": null,
                           "files": [{"path": "parts/row/features/c1.json"}]}),
            t0,
        );
        assert_eq!(w.bounds(t0, &m, &picker), None);
        w.activity(&entry("agent", &["parts/row/features/c1.json"]), t0);
        let (lo, hi) = w.bounds(t0, &m, &picker).unwrap();
        assert!(
            near(lo, [1.0, 0.0, 0.0]) && near(hi, [1.01, 0.01, 0.01]),
            "{lo:?} {hi:?}"
        );
    }

    #[test]
    fn a_part_edited_under_an_assembly_is_boxed_where_it_is_placed() {
        let m = row(true);
        let picker = Picker::new(&m);
        let t0 = Instant::now();
        // A feature of the placed part: every occurrence of it.
        let mut w = Work::default();
        w.activity(&entry("agent", &["parts/cube/features/s.json"]), t0);
        let (lo, hi) = w.bounds(t0, &m, &picker).unwrap();
        assert!(
            near(lo, [0.0; 3]) && near(hi, [1.01, 0.01, 0.01]),
            "{lo:?} {hi:?}"
        );
        // A part file and no feature's: all its bodies too.
        let mut w = Work::default();
        w.activity(&entry("agent", &["parts/cube/part.json"]), t0);
        let (lo, hi) = w.bounds(t0, &m, &picker).unwrap();
        assert!(
            near(lo, [0.0; 3]) && near(hi, [1.01, 0.01, 0.01]),
            "{lo:?} {hi:?}"
        );
        assert_eq!(
            bodies(&m, &Target::Part("cube".into())),
            [
                ("row".to_owned(), "c0/s".to_owned()),
                ("row".to_owned(), "c1/s".to_owned())
            ]
        );
    }

    #[test]
    fn what_an_agent_points_at_is_its_work() {
        let m = row(true);
        let picker = Picker::new(&m);
        let t0 = Instant::now();
        let agent = Person::from_json(&json!({"id": "p-1", "kind": "agent", "name": "agent",
            "selection": [],
            "highlights": [{"style": "pending", "label": "next", "entities": ["row/c1"]}]}))
        .unwrap();
        assert_eq!(pointing(&agent), ["row/c1"]);
        let mut w = Work::default();
        // Nothing new: no mark.
        w.pointed(&["row/c1".to_owned()], &agent, t0);
        assert_eq!(w.bounds(t0, &m, &picker), None);
        w.pointed(&[], &agent, t0);
        let (lo, _) = w.bounds(t0, &m, &picker).unwrap();
        assert!(near(lo, [1.0, 0.0, 0.0]), "{lo:?}");
        // A person's is theirs.
        let human = Person::from_json(&json!({"id": "p-2", "kind": "human", "name": "Dana",
            "selection": ["row/c0"]}))
        .unwrap();
        let mut w = Work::default();
        w.pointed(&[], &human, t0);
        assert!(!w.pending());
    }

    #[test]
    fn a_sketch_is_boxed_by_its_drawing() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "p", "revision": "r", "kind": "part",
            "features": [{"id": "s1", "type": "sketch", "sketch": {"state": "well", "dof": 0}}],
            "solids": []}));
        let d = crate::core::sketch::Drawing::from_get(&json!({"revision": "r", "entities": {
            "l1": {"type": "line", "state": "well", "polyline": [[0.5, 0.0, 0.0], [0.6, 0.1, 0.0]]}}}));
        assert!(m.take_sketch(&("p".to_owned(), "s1".to_owned()), d));
        let picker = Picker::new(&m);
        let mut w = Work::default();
        let t0 = Instant::now();
        w.activity(&entry("agent", &["parts/p/features/s1.json"]), t0);
        let (lo, hi) = w.bounds(t0, &m, &picker).unwrap();
        assert!(
            near(lo, [0.5, 0.0, 0.0]) && near(hi, [0.6, 0.1, 0.0]),
            "{lo:?} {hi:?}"
        );
    }

    #[test]
    fn a_mark_counts_for_five_seconds_from_its_last_change() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut w = Work::default();
        w.activity(&entry("agent", &["parts/row/features/c1.json"]), t0);
        // Its meshes haven't come: no box, but it waits for them.
        let bare = row(false);
        assert_eq!(w.bounds(t0, &bare, &Picker::new(&bare)), None);
        assert!(w.pending());
        // They come 4 s later: it counts from then.
        let m = row(true);
        let picker = Picker::new(&m);
        assert!(w.bounds(t0 + s(4), &m, &picker).is_some());
        assert!(w.bounds(t0 + s(8), &m, &picker).is_some());
        assert_eq!(w.bounds(t0 + s(9), &m, &picker), None);
        assert!(!w.pending());
    }

    /// The camera of the first view on a 1 cm cube at the origin.
    fn on_cube() -> Camera {
        ryucad::view::frame(
            [0.0; 3],
            [0.01; 3],
            ryucad::view::StandardView::Iso,
            ryucad::view::DEFAULT_PROJECTION,
        )
    }

    fn sight(camera: &Camera) -> Sight<'_> {
        Sight {
            camera,
            aspect: 1.5,
            following: false,
            every: EVERY,
        }
    }

    const FAR: Bounds = ([1.0, 0.0, 0.0], [1.01, 0.01, 0.01]);

    #[test]
    fn work_in_the_middle_of_the_view_moves_nothing() {
        let c = on_cube();
        assert!(in_view(&c, 1.5, [0.004; 3], [0.006; 3], INNER));
        assert!(!in_view(&c, 1.5, FAR.0, FAR.1, INNER));
        let mut w = Work::default();
        assert_eq!(
            w.frame(Instant::now(), &sight(&c), Some(([0.004; 3], [0.006; 3]))),
            None
        );
        assert_eq!(w.frame(Instant::now(), &sight(&c), None), None);
    }

    #[test]
    fn work_out_of_view_is_eased_to_never_zoomed_in_on() {
        let c = on_cube();
        let mut w = Work::default();
        let to = w.frame(Instant::now(), &sight(&c), Some(FAR)).unwrap();
        // Inside the inner 90%, so the next publish leaves it be.
        assert!(in_view(&to, 1.5, FAR.0, FAR.1, INNER));
        // What was in view still is: it never zooms in.
        for p in view_corners(&c, 1.5) {
            assert!(in_view(&to, 1.5, p, p, 1.0 + 1e-9), "{p:?}");
        }
        // The same direction.
        let ((_, _, f0), (_, _, f1)) = (c.basis(), to.basis());
        assert!(near(f0, f1), "{f0:?} {f1:?}");
        // Once there, it stays.
        let later = Instant::now() + EVERY;
        assert_eq!(w.frame(later, &sight(&to), Some(FAR)), None);
    }

    #[test]
    fn a_burst_eases_once_per_publish() {
        let c = on_cube();
        let t0 = Instant::now();
        let mut w = Work::default();
        assert!(w.frame(t0, &sight(&c), Some(FAR)).is_some());
        assert_eq!(w.frame(t0 + EVERY / 2, &sight(&c), Some(FAR)), None);
        assert!(w.frame(t0 + EVERY, &sight(&c), Some(FAR)).is_some());
    }

    #[test]
    fn a_hand_pauses_it_and_following_comes_first() {
        let c = on_cube();
        let t0 = Instant::now();
        let mut w = Work::default();
        w.hand(t0);
        assert_eq!(w.frame(t0 + PAUSE / 2, &sight(&c), Some(FAR)), None);
        let following = Sight {
            following: true,
            ..sight(&c)
        };
        assert_eq!(w.frame(t0 + PAUSE, &following, Some(FAR)), None);
        assert!(w.frame(t0 + PAUSE, &sight(&c), Some(FAR)).is_some());
    }

    #[test]
    fn the_window_looks_again_when_it_may_move() {
        let c = on_cube();
        let t0 = Instant::now();
        let mut w = Work::default();
        assert_eq!(w.wake_in(t0, EVERY), None);
        w.activity(&entry("agent", &["parts/row/features/c1.json"]), t0);
        w.hand(t0);
        assert_eq!(w.wake_in(t0, EVERY), Some(PAUSE));
        assert!(w.frame(t0 + PAUSE, &sight(&c), Some(FAR)).is_some());
        assert_eq!(w.wake_in(t0 + PAUSE, EVERY), Some(EVERY));
        assert_eq!(
            w.wake_in(t0 + PAUSE + EVERY, EVERY),
            Some(Duration::from_millis(100))
        );
    }
}
