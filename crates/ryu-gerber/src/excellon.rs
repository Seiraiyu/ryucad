//! Excellon drill files in the dialect fabricators read (KiCad's):
//! metric, absolute, explicit decimals; X2 attributes in comments; tools
//! sorted by diameter, then function; holes per tool sorted by drill
//! (rounds by position, then slots); slots drilled (`G85`) or routed
//! (`M15`/`M16`), as the caller chooses.

use crate::fmt::{LIMIT, mm};
use crate::{Error, Software, fmt::field};
use ryu_board::geom::{Nm, Point};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One drill: a round hole, or a slot from `a` to `b`, `d` wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Drill {
    Round { at: Point, d: Nm },
    Slot { a: Point, b: Point, d: Nm },
}

impl Drill {
    pub fn diameter(&self) -> Nm {
        match *self {
            Drill::Round { d, .. } | Drill::Slot { d, .. } => d,
        }
    }
}

/// A drill and its `.AperFunction` (`Plated,PTH,ViaDrill`, …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hole {
    pub drill: Drill,
    pub function: Option<String>,
}

/// One drill file: its `.FileFunction` (`Plated,1,2,PTH`) and holes.
#[derive(Clone, Debug, Default)]
pub struct Drills {
    pub function: String,
    pub holes: Vec<Hole>,
}

/// How a slot is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotForm {
    /// `X…Y…G85X…Y…`: a drilled slot.
    G85,
    /// `G00X…Y…`, `M15`, `G01X…Y…`, `M16`, `G05`: routed.
    Routed,
}

fn xy(p: Point) -> Result<String, Error> {
    if p.x.abs() > LIMIT || p.y.abs() > LIMIT {
        return Err(Error::OutOfRange(p));
    }
    Ok(format!("X{}Y{}", mm(p.x), mm(p.y)))
}

/// The drills as an Excellon file.
pub fn write(d: &Drills, sw: &Software, slots: SlotForm) -> Result<String, Error> {
    let mut tools: BTreeMap<(Nm, Option<&str>), Vec<Drill>> = BTreeMap::new();
    for h in &d.holes {
        let dia = h.drill.diameter();
        if dia <= 0 {
            return Err(Error::NoDiameter);
        }
        tools
            .entry((dia, h.function.as_deref()))
            .or_default()
            .push(h.drill);
    }
    for v in tools.values_mut() {
        v.sort();
    }
    let mut out = String::new();
    let w = &mut out;
    writeln!(w, "M48").unwrap();
    writeln!(w, "; FORMAT={{-:-/ absolute / metric / decimal}}").unwrap();
    writeln!(
        w,
        "; #@! TF.GenerationSoftware,{},{},{}",
        field(&sw.vendor),
        field(&sw.application),
        field(&sw.version)
    )
    .unwrap();
    writeln!(w, "; #@! TF.SameCoordinates,Original").unwrap();
    writeln!(w, "; #@! TF.FileFunction,{}", d.function).unwrap();
    writeln!(w, "FMAT,2").unwrap();
    writeln!(w, "METRIC").unwrap();
    for (n, (dia, f)) in tools.keys().enumerate() {
        if let Some(f) = f {
            writeln!(w, "; #@! TA.AperFunction,{f}").unwrap();
        }
        writeln!(w, "T{}C{}", n + 1, mm(*dia)).unwrap();
    }
    writeln!(w, "%").unwrap();
    writeln!(w, "G90").unwrap();
    writeln!(w, "G05").unwrap();
    for (n, drills) in tools.values().enumerate() {
        writeln!(w, "T{}", n + 1).unwrap();
        for dr in drills {
            match *dr {
                Drill::Round { at, .. } => writeln!(w, "{}", xy(at)?).unwrap(),
                Drill::Slot { a, b, .. } => match slots {
                    SlotForm::G85 => writeln!(w, "{}G85{}", xy(a)?, xy(b)?).unwrap(),
                    SlotForm::Routed => {
                        writeln!(w, "G00{}", xy(a)?).unwrap();
                        writeln!(w, "M15").unwrap();
                        writeln!(w, "G01{}", xy(b)?).unwrap();
                        writeln!(w, "M16").unwrap();
                        writeln!(w, "G05").unwrap();
                    }
                },
            }
        }
    }
    writeln!(w, "T0").unwrap();
    writeln!(w, "M30").unwrap();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sw() -> Software {
        Software {
            vendor: "Seiraiyu".into(),
            application: "test".into(),
            version: "1.0.0".into(),
        }
    }

    fn drills() -> Drills {
        let p = |x, y| Point::new(x, y);
        Drills {
            function: "Plated,1,2,PTH".into(),
            holes: vec![
                Hole {
                    drill: Drill::Round {
                        at: p(3_000_000, 1_000_000),
                        d: 800_000,
                    },
                    function: Some("Plated,PTH,ComponentDrill".into()),
                },
                Hole {
                    drill: Drill::Round {
                        at: p(1_000_000, 1_000_000),
                        d: 300_000,
                    },
                    function: Some("Plated,PTH,ViaDrill".into()),
                },
                Hole {
                    drill: Drill::Slot {
                        a: p(5_000_000, 0),
                        b: p(6_500_000, 0),
                        d: 800_000,
                    },
                    function: Some("Plated,PTH,ComponentDrill".into()),
                },
                Hole {
                    drill: Drill::Round {
                        at: p(2_000_000, -500_000),
                        d: 800_000,
                    },
                    function: Some("Plated,PTH,ComponentDrill".into()),
                },
            ],
        }
    }

    #[test]
    fn tools_by_diameter_holes_by_position_and_g85_slots() {
        let text = write(&drills(), &sw(), SlotForm::G85).unwrap();
        let want = "M48\n\
                    ; FORMAT={-:-/ absolute / metric / decimal}\n\
                    ; #@! TF.GenerationSoftware,Seiraiyu,test,1.0.0\n\
                    ; #@! TF.SameCoordinates,Original\n\
                    ; #@! TF.FileFunction,Plated,1,2,PTH\n\
                    FMAT,2\n\
                    METRIC\n\
                    ; #@! TA.AperFunction,Plated,PTH,ViaDrill\n\
                    T1C0.3\n\
                    ; #@! TA.AperFunction,Plated,PTH,ComponentDrill\n\
                    T2C0.8\n\
                    %\n\
                    G90\n\
                    G05\n\
                    T1\n\
                    X1.0Y1.0\n\
                    T2\n\
                    X2.0Y-0.5\n\
                    X3.0Y1.0\n\
                    X5.0Y0.0G85X6.5Y0.0\n\
                    T0\n\
                    M30\n";
        assert_eq!(text, want);
    }

    #[test]
    fn routed_slots_plunge_route_and_lift() {
        let text = write(&drills(), &sw(), SlotForm::Routed).unwrap();
        assert!(
            text.contains("X3.0Y1.0\nG00X5.0Y0.0\nM15\nG01X6.5Y0.0\nM16\nG05\nT0\nM30\n"),
            "{text}"
        );
    }

    #[test]
    fn a_drill_of_no_diameter_or_out_of_range_is_refused() {
        let mut d = drills();
        d.holes[0].drill = Drill::Round {
            at: Point::new(0, 0),
            d: 0,
        };
        assert_eq!(write(&d, &sw(), SlotForm::G85), Err(Error::NoDiameter));
        let mut d = drills();
        d.holes[0].drill = Drill::Round {
            at: Point::new(LIMIT + 1, 0),
            d: 300_000,
        };
        assert!(matches!(
            write(&d, &sw(), SlotForm::G85),
            Err(Error::OutOfRange(_))
        ));
    }
}
