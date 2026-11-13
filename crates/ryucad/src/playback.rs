//! Motions shown, played and drawn (design 529 phase 6, plan 26.C). A
//! window shows at most one motion (`live::View::motion`): which one, how
//! far through, and whether it plays. `motion.frames` gives where a
//! motion puts each moved body at evenly spaced fractions, solved by
//! continuation as a sweep is, so a window fetches them once and scrubs
//! and plays locally. A render draws a motion at any fraction, and with
//! `interference` paints the faces that collide there (against rest, as
//! a sweep finds them). Nothing here writes, and only a render's
//! `interference` ever intersects a pair.

use crate::doc::id::Id;

/// The motion a participant's window shows: an assembly's, how far
/// through (0 its start, 1 its end), and whether the window plays it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Shown {
    pub part: Id,
    pub motion: Id,
    pub fraction: f64,
    pub playing: bool,
}

use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde_json::{Value, json};

/// A fraction outside `[0, 1]`.
pub(crate) fn bad_fraction(f: f64) -> Error {
    Error::new(
        Code::InvalidParam,
        "a motion's fraction is from 0 (its start) to 1 (its end)",
    )
    .with("fraction", f)
}

/// What `motion.show` asks: a motion of an assembly (`part`, else the
/// participant's focus, else the project's model when that is one
/// assembly), how far through and whether it plays; or with only
/// `fraction` or `playing` the one shown; or `off`.
#[derive(Clone, Debug, Default)]
pub struct ShowAsk {
    pub part: Option<String>,
    pub motion: Option<String>,
    pub fraction: Option<f64>,
    pub playing: Option<bool>,
    pub off: bool,
}

/// What a participant shows after `ask`, given what it shows now and its
/// focus. A new motion starts at 0, paused; the one shown keeps its
/// fraction and its playing, except that a fraction given alone pauses
/// it.
pub fn show(
    session: &mut Session,
    now: Option<&Shown>,
    focus: Option<&Id>,
    ask: &ShowAsk,
) -> Result<Option<Shown>> {
    if ask.off {
        if ask.motion.is_some() || ask.fraction.is_some() || ask.playing.is_some() {
            return Err(Error::new(
                Code::Usage,
                "give a motion to show, or off, not both",
            ));
        }
        return Ok(None);
    }
    if let Some(f) = ask.fraction
        && !(0.0..=1.0).contains(&f)
    {
        return Err(bad_fraction(f));
    }
    let (part, motion) = match (&ask.motion, now) {
        (Some(m), _) => {
            let part =
                crate::views::part_for(session, ask.part.as_deref(), focus)?.ok_or_else(|| {
                    Error::new(
                        Code::InvalidParam,
                        "the project shows several parts: name the assembly whose motion it is",
                    )
                })?;
            (part, m.clone())
        }
        (None, Some(n)) if ask.part.is_none() => (n.part.clone(), n.motion.to_string()),
        _ => {
            return Err(Error::new(
                Code::Usage,
                "name a motion to show (none is shown now), or off",
            ));
        }
    };
    // It must resolve now (a part is `wrong_part_kind`, a missing one
    // `motion_not_found`).
    crate::motion::resolve_in(session, &part, &motion)?;
    let same = now.filter(|n| n.part == part && n.motion.as_str() == motion);
    let fraction = ask.fraction.or(same.map(|n| n.fraction)).unwrap_or(0.0);
    let playing = ask
        .playing
        .or_else(|| same.filter(|_| ask.fraction.is_none()).map(|n| n.playing))
        .unwrap_or(false);
    Ok(Some(Shown {
        part,
        motion: Id::parse(&motion, "motion")?,
        fraction,
        playing,
    }))
}

/// `motion.show`'s answer: who, what it shows, and how that resolves.
pub fn shown_json(session: &mut Session, participant: &str, shown: Option<&Shown>) -> Value {
    let mut v = json!({"participant": participant, "motion": shown});
    if let Some(s) = shown
        && let Ok(r) = crate::motion::resolve_in(session, &s.part, s.motion.as_str())
    {
        v["resolved"] = r.to_json();
        v["warnings"] = json!(r.warnings);
    }
    v
}

use crate::assembly::{Model, path_text};
use crate::explode::Offset;
use crate::motion::{Job, Resolved, Rest};
use std::collections::BTreeMap;

/// The key a frame's offset is under, for occurrence `path`: its
/// top-level instance (`pin` for `pin/below`), or the copy and seed it
/// is under (`ring:2/pin`). Every occurrence under one moves with it.
pub fn top_key(path: &str) -> &str {
    let first = path.split('/').next().unwrap_or(path);
    if !first.contains(':') {
        return first;
    }
    match path[first.len()..].strip_prefix('/') {
        Some(rest) => {
            let seed = rest.split('/').next().unwrap_or(rest);
            &path[..first.len() + 1 + seed.len()]
        }
        None => path,
    }
}

/// Each moved occurrence's offset from the rest model `model` to the
/// step's `step` (`motion::step_model`'s, with its `movers`), by path:
/// where it is drawn, applied after its rest pose.
pub fn occurrence_offsets(
    model: &Model,
    step: &Model,
    movers: &[Option<u32>],
) -> BTreeMap<String, Offset> {
    model
        .occurrences
        .iter()
        .zip(&step.occurrences)
        .zip(movers)
        .filter(|(_, m)| m.is_some())
        .map(|((o, n), _)| {
            (
                path_text(&o.path),
                Offset(n.pose.then_after(&o.pose.inverse())),
            )
        })
        .collect()
}

/// [`occurrence_offsets`] by [`top_key`]: one per moved body, the first
/// occurrence under it in path order.
fn body_offsets(by_path: &BTreeMap<String, Offset>) -> BTreeMap<String, Offset> {
    let mut out = BTreeMap::new();
    for (p, o) in by_path {
        out.entry(top_key(p).to_owned()).or_insert(*o);
    }
    out
}

/// One frame of a motion: its fraction, whether it holds, each drive's
/// value there (as a sweep gives them), and each moved body's offset by
/// [`top_key`]. A blocked frame keeps the last held frame's offsets.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub t: f64,
    pub holds: bool,
    pub values: Value,
    pub offsets: BTreeMap<String, Offset>,
}

impl Frame {
    /// As `motion.frames` gives it: frame `index`, with `poses` (the
    /// moved instances' poses, as a sweep's `poses` gives them).
    pub fn to_json(&self, index: usize, poses: Value) -> Value {
        let offsets: serde_json::Map<String, Value> = self
            .offsets
            .iter()
            .map(|(k, o)| (k.clone(), crate::assembly::rigid_pose(&o.0).to_json()))
            .collect();
        json!({"index": index, "t": self.t,
               "state": if self.holds { "holds" } else { "blocked" },
               "values": self.values, "poses": poses, "offsets": offsets})
    }

    /// From `motion.frames`' answer; `None` if it isn't one.
    pub fn from_json(v: &Value) -> Option<Frame> {
        let mut offsets = BTreeMap::new();
        for (k, p) in v["offsets"].as_object()? {
            let pose = crate::doc::assembly::Pose::from_json(p, "/offsets").ok()?;
            offsets.insert(k.clone(), Offset(crate::assembly::pose_rigid(&pose).ok()?));
        }
        Some(Frame {
            t: v["t"].as_f64()?,
            holds: v["state"] == "holds",
            values: v["values"].clone(),
            offsets,
        })
    }

    /// Where this frame draws occurrence `path`, relative to rest: its
    /// body's offset; `None` for what doesn't move.
    pub fn offset_of(&self, path: &str) -> Option<&Offset> {
        self.offsets.get(top_key(path))
    }
}

/// `resolved`'s frames at fractions `ts` over the assembly at `rest` and
/// `model`: each solved from the last that held, as a sweep's steps are
/// (`motion::steps`), with the moved instances' poses. A blocked frame,
/// or one whose model can't be placed, keeps the last held frame's
/// offsets and poses.
pub fn frames_of(
    rest: &Rest,
    model: &Model,
    resolved: &Resolved,
    ts: &[f64],
    unit: crate::units::Unit,
    job: &Job,
) -> Result<Vec<(Frame, Value)>> {
    let steps = crate::motion::steps(&rest.asm, &resolved.drives, ts, job)?;
    let mut out = Vec::with_capacity(steps.len());
    let mut held: (BTreeMap<String, Offset>, Value) = (BTreeMap::new(), json!({}));
    for s in &steps {
        job.check()?;
        let mut holds = s.holds;
        if holds {
            match crate::motion::step_model(model, rest, &s.moved) {
                Ok((m, movers)) => {
                    held = (
                        body_offsets(&occurrence_offsets(model, &m, &movers)),
                        crate::motion::poses_json(rest, &s.moved),
                    );
                }
                Err(_) => holds = false,
            }
        }
        let frame = Frame {
            t: s.t,
            holds,
            values: crate::motion::values_at(&resolved.drives, s.t, unit),
            offsets: held.0.clone(),
        };
        out.push((frame, held.1.clone()));
    }
    Ok(out)
}

use crate::doc::assembly::Rev;
use crate::doc::motion::MotionFile;

/// Frames `motion.frames` gives by default, and at most.
pub const FRAMES: u32 = 120;
pub const MAX_FRAMES: u32 = 1000;

/// What `motion.frames` asks: a saved `motion`, or inline `drives` and
/// `steps`, of assembly `part`, at `frames` even fractions.
#[derive(Clone, Debug, Default)]
pub struct FramesAsk {
    pub part: String,
    pub motion: Option<String>,
    pub drives: Option<Value>,
    pub steps: Option<u32>,
    pub frames: Option<u32>,
}

/// `motion.frames`: where the motion puts each moved body at `frames`
/// even fractions of the way, from the working files' rebuild; asked
/// again, from the session's cache. It only solves.
pub fn frames(session: &mut Session, ask: FramesAsk) -> Result<Value> {
    let pid = crate::motion::assembly(session, &ask.part, None)?;
    let part = session.project().part(&pid)?.clone();
    let unit = session.project().manifest.default_unit;
    let n = ask.frames.unwrap_or(FRAMES);
    if !(2..=MAX_FRAMES).contains(&n) {
        return Err(
            Error::new(Code::InvalidParam, format!("frames is 2 to {MAX_FRAMES}"))
                .with("frames", n),
        );
    }
    let file = match (&ask.motion, &ask.drives) {
        (Some(m), None) if ask.steps.is_none() => crate::motion::file_of(session, &pid, m)?,
        (None, Some(d)) => {
            let doc = json!({"format": crate::doc::motion::FORMAT,
                             "kind": crate::doc::motion::KIND,
                             "steps": ask.steps.unwrap_or(crate::doc::motion::STEPS),
                             "drives": d});
            MotionFile::from_json(&doc, unit, &part.env).map_err(crate::motion::as_param)?
        }
        _ => {
            return Err(Error::new(
                Code::InvalidParam,
                "frames of a saved motion or of inline drives: exactly one of motion and drives (steps goes with drives)",
            ));
        }
    };
    let result = session.rebuilt(&pid)?.clone();
    let rest = Rest::of(&result);
    if !rest.holds {
        return Err(crate::motion::unsolved(&pid));
    }
    let motion_id = ask
        .motion
        .as_deref()
        .map(|m| Id::parse(m, "motion"))
        .transpose()?;
    let resolved = crate::motion::resolve(
        &pid,
        &part,
        &rest,
        ask.motion.as_deref().unwrap_or("(inline)"),
        &file,
    )?;
    let model = session.model(&pid, &Rev::Working)?;
    let key = crate::motion::answer_key(
        "frames",
        &part.revision,
        &model.hash,
        &resolved,
        &json!({"frames": n}),
        motion_id.as_ref(),
    );
    if let Some(v) = session.caches_mut().sweeps_mut().get(&key) {
        return Ok((*v).clone());
    }
    let job = session.job.clone();
    let ts: Vec<f64> = (0..n as usize)
        .map(|k| crate::motion::t_of(k, n as usize))
        .collect();
    let frames = frames_of(&rest, &model, &resolved, &ts, unit, &job)?;
    let v = json!({
        "part": pid,
        "motion": motion_id,
        "revision": part.revision,
        "frames": frames
            .iter()
            .enumerate()
            .map(|(k, (f, poses))| f.to_json(k, poses.clone()))
            .collect::<Vec<_>>(),
        "warnings": resolved.warnings,
    });
    session
        .caches_mut()
        .sweeps_mut()
        .put(key, std::sync::Arc::new(v.clone()));
    Ok(v)
}

use std::collections::BTreeSet;

/// The fractions a render's motion is solved through to reach
/// `fraction`: the motion's own steps (of `n`) before it, then
/// `fraction`: continuation from rest, as its sweep runs.
pub fn ts_to(n: usize, fraction: f64) -> Vec<f64> {
    let mut ts: Vec<f64> = (0..n)
        .map(|k| crate::motion::t_of(k, n))
        .filter(|t| *t < fraction)
        .collect();
    ts.push(fraction);
    ts
}

/// What a render draws of a motion at a fraction: each moved
/// occurrence's offset by path, and with `interference` the faces of
/// the pairs that collide there and don't at rest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn {
    pub offsets: BTreeMap<String, Offset>,
    pub faces: Vec<String>,
}

/// Motion `motion` of assembly `asm` at `fraction`, as `session`'s files
/// have it: solved through [`ts_to`], drawn where the last step that held
/// leaves it; with `interference` its collisions against rest found
/// with the session's cache.
pub fn drawn(
    session: &mut Session,
    asm: &Id,
    motion: &str,
    fraction: f64,
    interference: bool,
) -> Result<Drawn> {
    if !(0.0..=1.0).contains(&fraction) {
        return Err(bad_fraction(fraction));
    }
    let file = crate::motion::file_of(session, asm, motion)?;
    let part = session.project().part(asm)?.clone();
    let result = session.rebuilt(asm)?.clone();
    let rest = Rest::of(&result);
    if !rest.holds {
        return Err(crate::motion::unsolved(asm));
    }
    let resolved = crate::motion::resolve(asm, &part, &rest, motion, &file)?;
    let model = session.model(asm, &Rev::Working)?;
    let job = session.job.clone();
    let steps = crate::motion::steps(
        &rest.asm,
        &resolved.drives,
        &ts_to(resolved.steps, fraction),
        &job,
    )?;
    let moved = steps
        .iter()
        .rev()
        .find(|s| s.holds)
        .map(|s| s.moved.clone())
        .unwrap_or_default();
    let (step, movers) = crate::motion::step_model(&model, &rest, &moved)?;
    let offsets = occurrence_offsets(&model, &step, &movers);
    let mut faces = Vec::new();
    if interference {
        let pairs = |v: &Value| -> Vec<Value> {
            v["interferences"].as_array().cloned().unwrap_or_default()
        };
        let at_rest: BTreeSet<(String, String)> = pairs(&crate::interference::find_for(
            asm,
            &model,
            &[],
            session.caches_mut().interference_mut(),
            &job,
        )?)
        .iter()
        .map(crate::motion::pair_key)
        .collect();
        let found = crate::interference::find_for(
            asm,
            &step,
            &[],
            session.caches_mut().interference_mut(),
            &job,
        )?;
        for i in pairs(&found)
            .iter()
            .filter(|i| !at_rest.contains(&crate::motion::pair_key(i)))
        {
            for side in ["a", "b"] {
                for f in i["faces"][side].as_array().into_iter().flatten() {
                    if let Some(f) = f.as_str() {
                        faces.push(f.to_owned());
                    }
                }
            }
        }
    }
    Ok(Drawn { offsets, faces })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use crate::live::{CadLive, Kind, Live};
    use crate::motion::tests::{stopped, swing, turning};
    use crate::testing::{call, call_live};
    use serde_json::{Value, json};

    /// [`stopped`] with motion `swing`: `m1` from 0° to 10° in 11 steps,
    /// past its 6° stop from step 7.
    pub(crate) fn swinging() -> (tempfile::TempDir, crate::session::Session) {
        let (dir, mut s) = stopped();
        call(
            &mut s,
            "motion.save",
            json!({"part": "frame", "motion": "swing", "steps": 11,
                   "drives": [{"mate": "m1", "angle": {"from": "0 deg", "to": "10 deg"}}]}),
        )
        .unwrap();
        (dir, s)
    }

    #[test]
    fn a_participants_window_shows_a_motion() {
        let (_dir, mut s) = swinging();
        let mut live = Live::new();
        let human = live.hello(Kind::Human, Some("ann"), None).unwrap();
        let show = |s: &mut crate::session::Session, live: &mut Live, v: Value| {
            call_live(s, live, None, "motion.show", v)
        };
        let v = show(
            &mut s,
            &mut live,
            json!({"part": "frame", "motion": "swing"}),
        )
        .unwrap();
        assert_eq!(v["participant"], json!(human));
        assert_eq!(
            v["motion"],
            json!({"part": "frame", "motion": "swing", "fraction": 0.0, "playing": false})
        );
        assert_eq!(v["resolved"][0]["mate"], "m1");
        assert_eq!(v["warnings"][0]["code"], "motion_past_limit");
        assert_eq!(live.get(&human).unwrap().to_json()["motion"], v["motion"]);
        // Played, then a fraction alone moves it and pauses it.
        let v = show(&mut s, &mut live, json!({"playing": true})).unwrap();
        assert_eq!(v["motion"]["playing"], true);
        let v = show(&mut s, &mut live, json!({"fraction": 0.5})).unwrap();
        assert_eq!(
            (
                v["motion"]["fraction"].clone(),
                v["motion"]["playing"].clone()
            ),
            (json!(0.5), json!(false))
        );
        // Playing again keeps the fraction.
        let v = show(&mut s, &mut live, json!({"playing": true})).unwrap();
        assert_eq!(v["motion"]["fraction"], 0.5);
        // Refusals.
        let e = show(&mut s, &mut live, json!({"fraction": 1.5})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let e = show(
            &mut s,
            &mut live,
            json!({"part": "frame", "motion": "nothing"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::MotionNotFound);
        let e = show(
            &mut s,
            &mut live,
            json!({"part": "plate", "motion": "swing"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::WrongPartKind);
        let e = show(&mut s, &mut live, json!({"off": true, "fraction": 0.2})).unwrap_err();
        assert_eq!(e.code, Code::Usage);
        // An explode shown takes it away, and the motion shown takes the
        // explode away.
        live.set_explode(
            &human,
            Some(crate::explode::Shown {
                part: Id::parse("frame", "part").unwrap(),
                explode: Id::parse("apart", "explode").unwrap(),
                fraction: 1.0,
            }),
        )
        .unwrap();
        assert_eq!(live.get(&human).unwrap().view.motion, None);
        show(
            &mut s,
            &mut live,
            json!({"part": "frame", "motion": "swing"}),
        )
        .unwrap();
        assert_eq!(live.get(&human).unwrap().view.explode, None);
        // Off; then nothing is shown to move.
        let v = show(&mut s, &mut live, json!({"off": true})).unwrap();
        assert_eq!(v["motion"], Value::Null);
        let e = show(&mut s, &mut live, json!({"fraction": 0.5})).unwrap_err();
        assert_eq!(e.code, Code::Usage);
        // Only in a session.
        let e = call(&mut s, "motion.show", json!({"motion": "swing"})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
        // Deleted, it goes from every window showing it.
        show(
            &mut s,
            &mut live,
            json!({"part": "frame", "motion": "swing"}),
        )
        .unwrap();
        call_live(
            &mut s,
            &mut live,
            None,
            "motion.delete",
            json!({"part": "frame", "motion": "swing"}),
        )
        .unwrap();
        assert_eq!(live.get(&human).unwrap().view.motion, None);
    }

    #[test]
    fn showing_a_motion_writes_nothing_and_moves_nothing() {
        let (dir, mut s) = swing(false);
        call(
            &mut s,
            "motion.save",
            json!({"part": "swing", "motion": "turn", "steps": 46, "drives": turning("45 deg")}),
        )
        .unwrap();
        let before = call(&mut s, "rebuild", json!({"part": "swing"})).unwrap();
        let files = crate::annotation::propose::tests::on_disk(dir.path());
        let mut live = Live::new();
        live.hello(Kind::Human, Some("ann"), None).unwrap();
        call_live(
            &mut s,
            &mut live,
            None,
            "motion.show",
            json!({"part": "swing", "motion": "turn", "fraction": 0.5, "playing": true}),
        )
        .unwrap();
        assert_eq!(
            call(&mut s, "rebuild", json!({"part": "swing"})).unwrap(),
            before
        );
        assert_eq!(
            crate::annotation::propose::tests::on_disk(dir.path()),
            files
        );
        assert!(s.caches().interference().is_empty(), "nothing intersected");
    }

    /// `resolved` frames of a saved motion at `n` even fractions.
    fn framed(
        s: &mut crate::session::Session,
        asm: &str,
        motion: &str,
        n: usize,
    ) -> Vec<(Frame, Value)> {
        let pid = Id::parse(asm, "part").unwrap();
        let r = crate::motion::resolve_in(s, &pid, motion).unwrap();
        let result = s.rebuilt(&pid).unwrap().clone();
        let rest = crate::motion::Rest::of(&result);
        let model = s.model(&pid, &crate::doc::assembly::Rev::Working).unwrap();
        let ts: Vec<f64> = (0..n).map(|k| crate::motion::t_of(k, n)).collect();
        let unit = s.project().manifest.default_unit;
        frames_of(&rest, &model, &r, &ts, unit, &crate::motion::Job::default()).unwrap()
    }

    #[test]
    fn top_keys_are_instances_and_copies() {
        assert_eq!(top_key("pin"), "pin");
        assert_eq!(top_key("sub/inner/deep"), "sub");
        assert_eq!(top_key("ring:2/pin"), "ring:2/pin");
        assert_eq!(top_key("ring:2/pin/below"), "ring:2/pin");
        assert_eq!(top_key("grid:1:3/pin"), "grid:1:3/pin");
    }

    #[test]
    fn frames_move_what_the_motion_moves_by_continuation() {
        let (_dir, mut s) = swing(false);
        call(
            &mut s,
            "motion.save",
            json!({"part": "swing", "motion": "turn", "steps": 46, "drives": turning("45 deg")}),
        )
        .unwrap();
        let frames = framed(&mut s, "swing", "turn", 10);
        assert_eq!(frames.len(), 10);
        // At rest nothing has moved; the block, fixed, never moves.
        assert!(frames[0].0.offsets.is_empty(), "{:?}", frames[0].0);
        assert!(
            frames
                .iter()
                .all(|(f, _)| f.holds && !f.offsets.contains_key("block"))
        );
        // At the end the arm's far end has turned 45° about z.
        let last = &frames[9].0;
        assert_eq!(last.t, 1.0);
        let tip = last.offset_of("arm").unwrap().apply([0.05, 0.0, 0.0]);
        let c = 0.05 * ryukei::math::cos(core::f64::consts::FRAC_PI_4);
        assert!(
            (tip[0] - c).abs() < 1e-12 && (tip[1] - c).abs() < 1e-12,
            "{tip:?}"
        );
        assert_eq!(last.values["drives/0"]["angle_deg"], 45.0);
        // Each frame's poses are the moved instances', as a sweep's.
        assert!(frames[9].1["arm"]["origin_m"].is_array(), "{}", frames[9].1);
        // A frame reads back as it was written.
        let v = last.to_json(9, frames[9].1.clone());
        assert_eq!(v["index"], 9);
        assert_eq!(v["state"], "holds");
        let back = Frame::from_json(&v).unwrap();
        let p = back.offset_of("arm/below").unwrap().apply([0.05, 0.0, 0.0]);
        assert!((p[0] - tip[0]).abs() < 1e-12 && (p[1] - tip[1]).abs() < 1e-12);
    }

    #[test]
    fn a_blocked_frame_stays_where_the_motion_last_held() {
        let (_dir, mut s) = swinging();
        let frames = framed(&mut s, "frame", "swing", 11);
        // 0° to 10° by 1°: past the 6° stop from 7°.
        let states: Vec<bool> = frames.iter().map(|(f, _)| f.holds).collect();
        assert_eq!(
            states,
            [
                true, true, true, true, true, true, true, false, false, false, false
            ]
        );
        assert_eq!(frames[10].0.offsets, frames[6].0.offsets);
        assert_eq!(frames[10].1, frames[6].1);
        assert_ne!(frames[6].0.offsets, frames[1].0.offsets);
        // Its values still say where the drive is.
        let deg = frames[10].0.values["m1"]["angle_deg"].as_f64().unwrap();
        assert!((deg - 10.0).abs() < 1e-12, "{deg}");
    }

    #[test]
    fn motion_frames_answers_saved_and_inline_alike_and_caches() {
        let (_dir, mut s) = swing(false);
        call(
            &mut s,
            "motion.save",
            json!({"part": "swing", "motion": "turn", "steps": 46, "drives": turning("45 deg")}),
        )
        .unwrap();
        let saved = call(
            &mut s,
            "motion.frames",
            json!({"part": "swing", "motion": "turn", "frames": 5}),
        )
        .unwrap();
        assert_eq!(saved["part"], "swing");
        assert_eq!(saved["motion"], "turn");
        assert!(saved["revision"].is_string());
        let frames = saved["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[2]["t"], 0.5);
        assert_eq!(frames[0]["offsets"], json!({}));
        assert!(
            frames[4]["offsets"]["arm"]["origin_m"].is_array(),
            "{}",
            frames[4]
        );
        // Inline drives give the same frames; 120 by default.
        let inline = call(
            &mut s,
            "motion.frames",
            json!({"part": "swing", "drives": turning("45 deg"), "steps": 46, "frames": 5}),
        )
        .unwrap();
        assert_eq!(inline["motion"], Value::Null);
        assert_eq!(inline["frames"], saved["frames"]);
        let plain = call(
            &mut s,
            "motion.frames",
            json!({"part": "swing", "motion": "turn"}),
        )
        .unwrap();
        assert_eq!(plain["frames"].as_array().unwrap().len(), FRAMES as usize);
        // Asked again: from the session's cache; nothing intersected.
        let kept = s.caches().sweeps().len();
        let again = call(
            &mut s,
            "motion.frames",
            json!({"part": "swing", "motion": "turn", "frames": 5}),
        )
        .unwrap();
        assert_eq!(again, saved);
        assert_eq!(s.caches().sweeps().len(), kept);
        assert!(s.caches().interference().is_empty());
        // Refusals.
        for (p, code) in [
            (json!({"part": "swing"}), Code::InvalidParam),
            (
                json!({"part": "swing", "motion": "turn", "frames": 1}),
                Code::InvalidParam,
            ),
            (
                json!({"part": "swing", "motion": "turn", "frames": 1001}),
                Code::InvalidParam,
            ),
            (
                json!({"part": "swing", "motion": "turn", "drives": turning("1 deg")}),
                Code::InvalidParam,
            ),
            (
                json!({"part": "swing", "motion": "nope"}),
                Code::MotionNotFound,
            ),
            (
                json!({"part": "arm", "drives": turning("1 deg")}),
                Code::WrongPartKind,
            ),
        ] {
            assert_eq!(
                call(&mut s, "motion.frames", p.clone()).unwrap_err().code,
                code,
                "{p}"
            );
        }
    }

    #[test]
    fn a_frame_of_an_assembly_that_does_not_hold_is_refused() {
        // As 26.B's sweep test: a second mate fastening the pin 20 mm from
        // the hinge's axis, so the two can't both hold.
        let (dir, mut s) = swinging();
        let top = crate::mate::tests::face(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
        let foot = crate::mate::tests::face(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
        crate::mate::tests::add_in(
            dir.path(),
            "frame",
            "m2",
            "mate",
            json!({"form": "fastened",
                   "a": {"instance": ["base"], "at": {"feature": "slab", "name": top},
                         "offset": {"origin": ["40 mm", "10 mm", "0 mm"]}},
                   "b": {"instance": ["pin"], "at": {"feature": "shank", "name": foot},
                         "offset": {"flip": true}}}),
        );
        s.refresh().unwrap();
        let e = call(
            &mut s,
            "motion.frames",
            json!({"part": "frame", "motion": "swing"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::AssemblyUnsolved, "{e:?}");
    }

    /// A render of `params` (64 × 48) over the project at `r`, as a
    /// participant `me` of `live` asks it.
    fn ask(
        slot: &mut Option<crate::session::Session>,
        live: &mut Live,
        r: &std::path::Path,
        me: Option<&str>,
        method: &str,
        v: Value,
    ) -> crate::error::Result<crate::proto::Answer> {
        let req = crate::proto::Request::from_rpc(method, Some(v)).unwrap();
        crate::server::handle_safely(slot, Some(live), me, r, req)
    }

    #[test]
    fn ts_to_runs_the_motions_steps_then_the_fraction() {
        assert_eq!(ts_to(5, 0.0), [0.0]);
        assert_eq!(ts_to(5, 0.6), [0.0, 0.25, 0.5, 0.6]);
        assert_eq!(ts_to(5, 0.5), [0.0, 0.25, 0.5]);
        assert_eq!(ts_to(5, 1.0), [0.0, 0.25, 0.5, 0.75, 1.0]);
    }

    #[test]
    fn renders_draw_motions_and_nothing_else_moves() {
        let (dir, mut s) = swing(false);
        call(
            &mut s,
            "motion.save",
            json!({"part": "swing", "motion": "turn", "steps": 46, "drives": turning("45 deg")}),
        )
        .unwrap();
        drop(s);
        let r = dir.path();
        let small = json!({"part": "swing", "width": 64, "height": 48, "view": "top"});
        let mut live = Live::new();
        let human = live.hello(Kind::Human, Some("ann"), None).unwrap();
        let mut slot = Some(crate::session::Session::open(r).unwrap());
        let before = ask(
            &mut slot,
            &mut live,
            r,
            None,
            "rebuild",
            json!({"part": "swing"}),
        )
        .unwrap()
        .result;
        let whole = ask(&mut slot, &mut live, r, None, "render", small.clone())
            .unwrap()
            .binary;
        let mut named = small.clone();
        named["motion"] = json!("turn");
        // At its start (by default) it is drawn at rest.
        assert_eq!(
            ask(&mut slot, &mut live, r, None, "render", named.clone())
                .unwrap()
                .binary,
            whole
        );
        named["fraction"] = json!(1.0);
        let end = ask(&mut slot, &mut live, r, None, "render", named.clone())
            .unwrap()
            .binary;
        assert_ne!(end, whole);
        named["fraction"] = json!(0.5);
        let half = ask(&mut slot, &mut live, r, None, "render", named.clone())
            .unwrap()
            .binary;
        assert!(half != whole && half != end);
        named["fraction"] = json!(2.0);
        assert_eq!(
            ask(&mut slot, &mut live, r, None, "render", named)
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
        // A window's motion, by default, where it shows it; "" draws none.
        ask(
            &mut slot,
            &mut live,
            r,
            None,
            "motion.show",
            json!({"part": "swing", "motion": "turn", "fraction": 0.5}),
        )
        .unwrap();
        assert_eq!(
            ask(
                &mut slot,
                &mut live,
                r,
                Some(&human),
                "render",
                small.clone()
            )
            .unwrap()
            .binary,
            half
        );
        let mut none = small.clone();
        none["motion"] = json!("");
        assert_eq!(
            ask(&mut slot, &mut live, r, None, "render", none)
                .unwrap()
                .binary,
            whole
        );
        // A motion and an explode aren't drawn together.
        let mut both = small.clone();
        both["motion"] = json!("turn");
        both["explode"] = json!("apart");
        assert_eq!(
            ask(&mut slot, &mut live, r, None, "render", both)
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
        // Nothing a motion draws moves the assembly, or intersects a pair.
        assert_eq!(
            ask(
                &mut slot,
                &mut live,
                r,
                None,
                "rebuild",
                json!({"part": "swing"})
            )
            .unwrap()
            .result,
            before
        );
        assert!(slot.as_ref().unwrap().caches().interference().is_empty());
        // Another part's render never reads the window's motion; one
        // named of a part is wrong_part_kind.
        let mut arm = small.clone();
        arm["part"] = json!("arm");
        let plain_arm = ask(&mut slot, &mut live, r, None, "render", arm.clone()).unwrap();
        assert!(plain_arm.result.get("warnings").is_none());
        arm["motion"] = json!("turn");
        assert_eq!(
            ask(&mut slot, &mut live, r, None, "render", arm)
                .unwrap_err()
                .code,
            Code::WrongPartKind
        );
        // Gone behind the session's back: the window's draws nothing, with
        // a warning; one the request names is still an error.
        std::fs::remove_file(r.join("parts/swing/motions/turn.json")).unwrap();
        let a = ask(
            &mut slot,
            &mut live,
            r,
            Some(&human),
            "render",
            small.clone(),
        )
        .unwrap();
        assert_eq!(a.binary, whole);
        assert_eq!(a.result["warnings"][0]["code"], "motion_not_found");
        assert_eq!(a.result["warnings"][0]["motion"], "turn");
        let mut gone = small;
        gone["motion"] = json!("turn");
        assert_eq!(
            ask(&mut slot, &mut live, r, None, "render", gone)
                .unwrap_err()
                .code,
            Code::MotionNotFound
        );
    }

    #[test]
    fn a_render_paints_what_collides_at_its_fraction() {
        let (dir, mut s) = swing(false);
        call(
            &mut s,
            "motion.save",
            json!({"part": "swing", "motion": "turn", "steps": 46, "drives": turning("45 deg")}),
        )
        .unwrap();
        let swing_id = Id::parse("swing", "part").unwrap();
        // About 21.43° is contact: at 0.25 (11.25°) clear, at 0.5 (22.5°) in.
        let clear = drawn(&mut s, &swing_id, "turn", 0.25, true).unwrap();
        assert!(clear.faces.is_empty(), "{:?}", clear.faces);
        assert!(!clear.offsets.is_empty());
        let hit = drawn(&mut s, &swing_id, "turn", 0.5, true).unwrap();
        assert!(
            hit.faces.iter().any(|f| f.starts_with("swing/arm/body/")),
            "{:?}",
            hit.faces
        );
        assert!(
            hit.faces.iter().any(|f| f.starts_with("swing/block/body/")),
            "{:?}",
            hit.faces
        );
        // What collides at rest is never painted.
        let (_d2, mut nubbed) = swing(true);
        call(
            &mut nubbed,
            "motion.save",
            json!({"part": "swing", "motion": "turn", "steps": 46, "drives": turning("45 deg")}),
        )
        .unwrap();
        let rest = drawn(&mut nubbed, &swing_id, "turn", 0.0, true).unwrap();
        assert!(
            rest.faces.is_empty(),
            "the nub is at rest: {:?}",
            rest.faces
        );
        // The PNG paints them: a render with interference differs from one
        // without, at 0.5 and not at 0.25.
        drop(s);
        let r = dir.path();
        let px = |v: Value| {
            crate::testing::render_pixels(r, crate::session::Session::open(r).unwrap(), v)
        };
        let at = |f: f64, i: bool| {
            json!({"part": "swing", "width": 96, "height": 72,
                   "view": "top", "motion": "turn", "fraction": f,
                   "interference": i})
        };
        assert_ne!(px(at(0.5, true)), px(at(0.5, false)));
        assert_eq!(px(at(0.25, true)), px(at(0.25, false)));
        // With no motion to paint, interference is refused.
        let req = crate::proto::Request::from_rpc(
            "render",
            Some(json!({"part": "swing", "width": 64, "height": 48, "interference": true})),
        )
        .unwrap();
        let e = crate::server::handle_safely(
            &mut Some(crate::session::Session::open(r).unwrap()),
            None,
            None,
            r,
            req,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }
}
