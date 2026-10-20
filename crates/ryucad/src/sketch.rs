//! Solving sketch features with `ryusketch`, and reporting what the solve
//! found in the sketch's own IDs.
//!
//! A sketch's stored positions are *current* when every constraint holds
//! at them, its dimensions evaluated as they are now: the solver then
//! returns them with the very same bits, and rebuild uses them as stored.
//! Otherwise the sketch is *stale*: rebuild uses positions solved in
//! memory from the stored ones and warns `sketch_stale`.

use crate::doc::id::Id;
use crate::doc::sketch::{Index, Plane, Sketch};
use crate::error::{Code, Error, Result};
use crate::rebuild::placement;
use crate::units::Unit;
use ryukei::geom::Rigid;
use ryusketch::{Coord, State};
use serde_json::{Value, json};

/// What a solve found, in the sketch's IDs.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub converged: bool,
    pub state: State,
    pub dof: usize,
    /// Entities that can still move, and the coordinates that can.
    pub free: Vec<(Id, Vec<Coord>)>,
    pub redundant: Vec<Vec<Id>>,
    pub conflicting: Vec<Vec<Id>>,
    /// Closed loops of profile curves: each curve, and whether it is
    /// walked from its end to its start.
    pub loops: Vec<Vec<(Id, bool)>>,
    /// Curves that join but don't close, and the points where they stop.
    pub open: Vec<(Vec<Id>, Vec<Id>)>,
}

impl Report {
    fn new(r: &ryusketch::Report, index: &Index) -> Report {
        let e = |i: &u32| index.entities[*i as usize].clone();
        let c = |set: &Vec<u32>| -> Vec<Id> {
            let mut ids: Vec<Id> = set
                .iter()
                .map(|i| index.constraints[*i as usize].clone())
                .collect();
            ids.sort();
            ids
        };
        Report {
            converged: r.converged,
            state: r.state,
            dof: r.dof,
            free: r
                .free
                .iter()
                .map(|f| (e(&f.entity), f.moves.clone()))
                .collect(),
            redundant: r.redundant.iter().map(c).collect(),
            conflicting: r.conflicting.iter().map(c).collect(),
            loops: r
                .loops
                .iter()
                .map(|l| {
                    l.segments
                        .iter()
                        .map(|s| (e(&s.entity), s.reversed))
                        .collect()
                })
                .collect(),
            open: r
                .open
                .iter()
                .map(|ch| {
                    (
                        ch.entities.iter().map(e).collect(),
                        ch.ends.iter().map(e).collect(),
                    )
                })
                .collect(),
        }
    }

    pub fn state_str(&self) -> &'static str {
        match self.state {
            State::Under => "under",
            State::Well => "well",
            State::Over => "over",
        }
    }

    pub fn to_json(&self) -> Value {
        let coord = |c: &Coord| match c {
            Coord::X => "x",
            Coord::Y => "y",
            Coord::R => "r",
        };
        json!({
            "converged": self.converged,
            "state": self.state_str(),
            "dof": self.dof,
            "free": self.free.iter().map(|(id, m)| {
                json!({"entity": id, "moves": m.iter().map(coord).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
            "redundant": self.redundant,
            "conflicting": self.conflicting,
            "loops": self.loops.iter().map(|l| {
                json!({"entities": l.iter().map(|(id, _)| id).collect::<Vec<_>>(),
                       "reversed": l.iter().map(|(_, r)| r).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
            "open": self.open.iter().map(|(entities, ends)| {
                json!({"entities": entities, "ends": ends})
            }).collect::<Vec<_>>(),
        })
    }
}

/// A sketch with its positions solved.
#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    /// The sketch at its solved positions.
    pub sketch: Sketch,
    pub report: Report,
    /// Whether the stored positions didn't hold, so these were solved in
    /// memory.
    pub stale: bool,
    /// The sketch's frame: its plane's placement.
    pub frame: Rigid,
    /// BLAKE3 of the solved positions, frame and report.
    pub hash: String,
}

/// Solves a sketch from its stored positions. A sketch whose constraints
/// can't all be made to hold is `sketch_unsolved`, with the report.
pub fn solve(id: &Id, s: &Sketch) -> Result<Solved> {
    let (solver, index) = s.solver()?;
    let solved = ryusketch::solve(&solver)
        .map_err(|e| Error::new(Code::Internal, format!("sketch {id}: {e}")).with("feature", id))?;
    let report = Report::new(&solved.report, &index);
    if !report.converged {
        return Err(Error::new(
            Code::SketchUnsolved,
            format!("sketch {id}: its constraints can't all be made to hold"),
        )
        .with("feature", id)
        .with("report", report.to_json()));
    }
    let mut sketch = s.clone();
    sketch.take_positions(&solved.sketch, &index);
    let frame = frame(&sketch.plane)?;
    let hash = hash(&sketch, &frame, &report);
    Ok(Solved {
        stale: sketch != *s,
        sketch,
        report,
        frame,
        hash,
    })
}

/// The rigid frame of a sketch's plane.
pub fn frame(plane: &Plane) -> Result<Rigid> {
    // The unit only writes a zero origin; it doesn't change the frame.
    placement(&plane.placement(Unit::M))
}

fn hash(s: &Sketch, frame: &Rigid, report: &Report) -> String {
    let mut h = blake3::Hasher::new();
    h.update(crate::doc::canonical::to_string(&s.fields()).as_bytes());
    for x in frame.rotation_matrix().iter().flatten() {
        h.update(&x.to_bits().to_le_bytes());
    }
    let o = frame.apply_point(ryukei::geom::Point3::new(0.0, 0.0, 0.0));
    for x in [o.x, o.y, o.z] {
        h.update(&x.to_bits().to_le_bytes());
    }
    h.update(report.to_json().to_string().as_bytes());
    h.finalize().to_hex()[..32].to_owned()
}

/// The warning for a sketch rebuilt from positions solved in memory.
pub fn stale_warning(id: &Id) -> Value {
    json!({
        "code": "sketch_stale",
        "feature": id,
        "message": format!(
            "sketch {id}: its stored positions don't satisfy its constraints; \
             rebuilt from positions solved in memory (run `ryucad doctor` to write them)"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::Feature;
    use crate::expr::Env;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    pub(crate) fn rectangle() -> Sketch {
        let v = crate::doc::sketch::tests::rectangle();
        let p = [(
            "width".to_owned(),
            crate::expr::Expr::parse("20 mm").unwrap(),
        )]
        .into();
        match Feature::from_json(&v, Unit::Mm, &Env::new(&p)).unwrap() {
            Feature::Sketch(s) => s,
            _ => unreachable!(),
        }
    }

    #[test]
    fn current_positions_are_used_as_stored() {
        let s = rectangle();
        let solved = solve(&id("s"), &s).unwrap();
        assert!(!solved.stale);
        assert_eq!(solved.sketch, s);
        assert_eq!(solved.report.state, State::Under);
        assert_eq!(solved.report.loops.len(), 1);
        assert_eq!(solved.report.loops[0][0].0.as_str(), "l1");
        let v = solved.report.to_json();
        assert_eq!(v["state"], "under");
        assert_eq!(v["loops"][0]["entities"], json!(["l1", "l2", "l3", "l4"]));
        assert_eq!(solve(&id("s"), &s).unwrap().hash, solved.hash);
    }

    #[test]
    fn stale_positions_are_solved_in_memory() {
        let mut s = rectangle();
        let d1 = Id::parse("d1", "constraint").unwrap();
        s.constraints.get_mut(&d1).unwrap().value =
            Some(crate::units::Quantity::parse("15 mm", Unit::Mm).unwrap());
        let solved = solve(&id("s"), &s).unwrap();
        assert!(solved.stale);
        let at = |name: &str| {
            let e = &solved.sketch.entities[&Id::parse(name, "entity").unwrap()];
            let crate::doc::sketch::Geometry::Point { m } = e.geometry else {
                panic!()
            };
            m
        };
        let (p1, p2) = (at("p1"), at("p2"));
        let width = ryukei::math::hypot(p2[0] - p1[0], p2[1] - p1[1]);
        assert!((width - 0.015).abs() < 1e-10, "{width}");
        // Solved positions are current: solving again changes nothing.
        let again = solve(&id("s"), &solved.sketch).unwrap();
        assert!(!again.stale);
        assert_eq!(again.hash, solved.hash);
    }

    #[test]
    fn conflicts_are_unsolved() {
        let mut s = rectangle();
        let d2 = Id::parse("d2", "constraint").unwrap();
        // The diagonal can't be shorter than the width.
        s.constraints.get_mut(&d2).unwrap().value =
            Some(crate::units::Quantity::parse("5 mm", Unit::Mm).unwrap());
        let e = solve(&id("s"), &s).unwrap_err();
        assert_eq!(e.code, Code::SketchUnsolved);
        assert_eq!(e.details["report"]["converged"], false);
    }
}
