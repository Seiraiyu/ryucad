//! Steps: each step's header (datum and step-and-repeat), and the rule
//! that chooses which step is the board.

use ryu_board::geom::{Mdeg, Point};
use ryu_board::model::Repeat;

use super::error::{Code, Error};
use super::text::{Bad, structured};
use super::units::{Nums, Units, token};

/// A step's `stephdr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub datum: Point,
    pub repeats: Vec<Repeat>,
    /// Repeats written `FLIP=YES`, which the model doesn't hold: the step
    /// is placed unflipped.
    pub flipped: usize,
}

/// Reads a step header.
pub fn header(text: &str, units: Units) -> Result<(Header, Nums), Bad> {
    let s = structured(text)?;
    let units = match s.field("UNITS") {
        None => units,
        Some(f) => Units::parse(&f.value)
            .ok_or_else(|| Bad::new(f.line, 1, format!("UNITS={}: not MM or INCH", f.value)))?,
    };
    let mut nums = Nums::new(units);
    let coord = |key: &str, fields: &[super::text::Field], nums: &mut Nums| -> Result<i64, Bad> {
        match fields.iter().find(|f| f.key.eq_ignore_ascii_case(key)) {
            None => Ok(0),
            Some(f) if f.value.is_empty() => Ok(0),
            Some(f) => nums.coord(&token(&f.value), f.line),
        }
    };
    let datum = Point::new(
        coord("X_DATUM", &s.fields, &mut nums)?,
        coord("Y_DATUM", &s.fields, &mut nums)?,
    );
    let mut repeats = Vec::new();
    let mut flipped = 0;
    for a in s.arrays("STEP-REPEAT") {
        let name = a
            .get("NAME")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| Bad::new(a.line, 1, "STEP-REPEAT without NAME"))?;
        let count = |k: &str| -> Result<u32, Bad> {
            let f = a
                .field(k)
                .ok_or_else(|| Bad::new(a.line, 1, format!("STEP-REPEAT without {k}")))?;
            f.value
                .parse()
                .map_err(|_| Bad::new(f.line, 1, format!("{k}={}: not a count", f.value)))
        };
        let yes = |k: &str| a.get(k).is_some_and(|v| v.eq_ignore_ascii_case("YES"));
        let mirror = yes("MIRROR");
        let angle = match a.field("ANGLE").filter(|f| !f.value.is_empty()) {
            None => Mdeg::ZERO,
            Some(f) => nums.angle(&token(&f.value), f.line)?,
        };
        // Angles turn clockwise and mirroring follows, as for pads.
        let cw = i64::from(angle.get());
        let rot = if mirror {
            Mdeg::new(cw)
        } else {
            Mdeg::from_clockwise(cw)
        };
        if yes("FLIP") {
            flipped += 1;
        }
        repeats.push(Repeat {
            step: name.to_ascii_lowercase(),
            at: Point::new(
                coord("X", &a.fields, &mut nums)?,
                coord("Y", &a.fields, &mut nums)?,
            ),
            dx: coord("DX", &a.fields, &mut nums)?,
            dy: coord("DY", &a.fields, &mut nums)?,
            nx: count("NX")?,
            ny: count("NY")?,
            rot,
            mirror,
        });
    }
    Ok((
        Header {
            datum,
            repeats,
            flipped,
        },
        nums,
    ))
}

/// The board step: `asked` if given (it must be a step), else the one
/// step that nothing repeats and that repeats nothing. `repeats` gives
/// each step's repeated steps, by index into `steps`.
pub fn board_step(
    steps: &[String],
    repeats: &[Vec<String>],
    asked: Option<&str>,
) -> Result<usize, Error> {
    if let Some(name) = asked {
        let name = name.to_ascii_lowercase();
        return steps.iter().position(|s| *s == name).ok_or_else(|| {
            let mut e = Error::new(
                Code::StepNotFound,
                format!("no step {name}: the steps are {}", steps.join(", ")),
            );
            e.steps = steps.to_vec();
            e
        });
    }
    let repeated: Vec<&str> = repeats.iter().flatten().map(String::as_str).collect();
    let candidates: Vec<usize> = (0..steps.len())
        .filter(|&i| repeats[i].is_empty() && !repeated.contains(&steps[i].as_str()))
        .collect();
    match candidates.as_slice() {
        [one] => Ok(*one),
        _ => {
            let named: Vec<String> = if candidates.is_empty() {
                steps.to_vec()
            } else {
                candidates.iter().map(|&i| steps[i].clone()).collect()
            };
            let mut e = Error::new(
                Code::StepAmbiguous,
                format!("which step is the board? Name one of {}", named.join(", ")),
            );
            e.steps = named;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_and_repeats() {
        let t = "UNITS=INCH\nX_DATUM=0.5\nY_DATUM=-1\nID=123456\nSTEP-REPEAT {\nNAME=1UP\nX=1.5\nY=1.6\nDX=1.2\nDY=1.2\nNX=6\nNY=6\nANGLE=90\nFLIP=NO\nMIRROR=NO\n}\nSTEP-REPEAT {\nNAME=coupon\nX=0\nY=0\nDX=0\nDY=0\nNX=1\nNY=1\nANGLE=90\nFLIP=YES\nMIRROR=YES\n}\nTOP_ACTIVE=1\n";
        let (h, _) = header(t, Units::Mm).unwrap();
        assert_eq!(h.datum, Point::new(12_700_000, -25_400_000));
        assert_eq!(h.flipped, 1);
        assert_eq!(
            h.repeats[0],
            Repeat {
                step: "1up".into(),
                at: Point::new(38_100_000, 40_640_000),
                dx: 30_480_000,
                dy: 30_480_000,
                nx: 6,
                ny: 6,
                rot: Mdeg::new(270_000),
                mirror: false,
            }
        );
        assert_eq!(
            (h.repeats[1].rot, h.repeats[1].mirror),
            (Mdeg::new(90_000), true)
        );
        // KiCad's: every field, nothing repeated.
        let (h, _) = header("X_ORIGIN=0\nY_DATUM=0\nX_DATUM=0\nUNITS=MM\n", Units::Inch).unwrap();
        assert_eq!((h.datum, h.repeats.len()), (Point::ORIGIN, 0));
        assert!(header("STEP-REPEAT {\nNAME=a\nNX=1\n}\n", Units::Mm).is_err());
        assert!(header("STEP-REPEAT {\nNX=1\n}\n", Units::Mm).is_err());
    }

    #[test]
    fn the_board_step() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let steps = s(&["pcb", "panel", "coupon"]);
        let reps = vec![vec![], s(&["pcb", "coupon"]), vec![]];
        // pcb and coupon are both repeated: nothing qualifies.
        let e = board_step(&steps, &reps, None).unwrap_err();
        assert_eq!((e.code, e.steps.len()), (Code::StepAmbiguous, 3));
        assert_eq!(board_step(&steps, &reps, Some("PCB")).unwrap(), 0);
        let e = board_step(&steps, &reps, Some("array")).unwrap_err();
        assert_eq!(
            (e.code, e.steps.clone()),
            (Code::StepNotFound, steps.clone())
        );
        assert_eq!(board_step(&s(&["pcb"]), &[vec![]], None).unwrap(), 0);
        // A panel of boards beside a coupon nothing repeats: two candidates.
        let reps = vec![vec![], s(&["pcb"]), vec![]];
        assert_eq!(board_step(&steps, &reps, None).unwrap(), 2);
        let reps = vec![vec![], vec![], vec![]];
        let e = board_step(&steps, &reps, None).unwrap_err();
        assert_eq!(e.steps, steps);
        assert!(e.message.contains("pcb, panel, coupon"));
        // A panel of the board: the board is repeated, so the rule finds
        // none, and the job must name it.
        let reps = vec![vec![], s(&["pcb"])];
        let e = board_step(&s(&["pcb", "panel"]), &reps, None).unwrap_err();
        assert_eq!(e.steps, s(&["pcb", "panel"]));
    }
}
