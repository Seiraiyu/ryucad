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
        assert!(s.interference.is_empty(), "nothing intersected");
    }
}
