//! Numbers from text, in each file's own units, and the count of those
//! rounded to the nanometre.
//!
//! Coordinates are inches or millimetres; sizes (symbol parameters, drill
//! tools, resize factors) mils or microns; angles degrees. Every number is
//! read from its text by `ryu_board::geom::decimal`, exactly where it can
//! be, and a file's rounded numbers become one `odb_rounded` warning.

use ryu_board::geom::decimal::{self, Exact, Unit};
use ryu_board::geom::{Mdeg, Nm, Point};
use ryu_board::model::Notice;
use ryu_board::symbol::NameUnits;

use super::error::{location, notice, warning};
use super::text::{Bad, Token};

/// A file's units, from its `UNITS=` directive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Units {
    Mm,
    Inch,
}

impl Units {
    /// `MM` or `INCH`, in any case.
    pub fn parse(s: &str) -> Option<Units> {
        if s.eq_ignore_ascii_case("mm") {
            Some(Units::Mm)
        } else if s.eq_ignore_ascii_case("inch") {
            Some(Units::Inch)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Units::Mm => "MM",
            Units::Inch => "INCH",
        }
    }

    /// The unit of coordinates and distances.
    pub fn coord(self) -> Unit {
        match self {
            Units::Mm => Unit::Mm,
            Units::Inch => Unit::Inch,
        }
    }

    /// The unit of sizes: microns or mils.
    pub fn size(self) -> Unit {
        match self {
            Units::Mm => Unit::Micron,
            Units::Inch => Unit::Mil,
        }
    }

    /// The units of numbers in symbol names.
    pub fn names(self) -> NameUnits {
        match self {
            Units::Mm => NameUnits::Micron,
            Units::Inch => NameUnits::Mil,
        }
    }
}

/// Reads one file's numbers and counts what it rounds.
#[derive(Clone, Debug)]
pub struct Nums {
    pub units: Units,
    rounded: u64,
    /// The largest change, in millionths of a nanometre.
    worst: i64,
}

/// `|num / den|` in millionths, rounded down: at most 500,000 for a
/// rounding error, which is at most a half.
fn millionths(e: &Exact) -> i64 {
    if e.error_den == 0 {
        // Past the digits counted: less than anything reported.
        return 0;
    }
    let (mut n, mut d) = (e.error_num.abs(), e.error_den);
    while n > i128::MAX / 1_000_000 {
        n /= 10;
        d /= 10;
    }
    (n * 1_000_000 / d) as i64
}

impl Nums {
    pub fn new(units: Units) -> Nums {
        Nums {
            units,
            rounded: 0,
            worst: 0,
        }
    }

    fn count(&mut self, e: &Exact) {
        if e.rounded() {
            self.rounded += 1;
            self.worst = self.worst.max(millionths(e));
        }
    }

    fn read(&mut self, t: &Token, line: u32, unit: Unit) -> Result<Nm, Bad> {
        let e = decimal::nm(t.text, unit)
            .map_err(|err| Bad::new(line, t.column, format!("{}: {err}", t.text)))?;
        self.count(&e);
        Ok(e.value)
    }

    /// A coordinate or distance, in inches or millimetres.
    pub fn coord(&mut self, t: &Token, line: u32) -> Result<Nm, Bad> {
        self.read(t, line, self.units.coord())
    }

    /// A point from two coordinates.
    pub fn point(&mut self, x: &Token, y: &Token, line: u32) -> Result<Point, Bad> {
        Ok(Point::new(self.coord(x, line)?, self.coord(y, line)?))
    }

    /// A size, in mils or microns.
    pub fn size(&mut self, t: &Token, line: u32) -> Result<Nm, Bad> {
        self.read(t, line, self.units.size())
    }

    /// A length in a unit of the caller's.
    pub fn length(&mut self, t: &Token, line: u32, unit: Unit) -> Result<Nm, Bad> {
        self.read(t, line, unit)
    }

    /// An angle in degrees.
    pub fn angle(&mut self, t: &Token, line: u32) -> Result<Mdeg, Bad> {
        let (a, e) = decimal::mdeg(t.text)
            .map_err(|err| Bad::new(line, t.column, format!("{}: {err}", t.text)))?;
        self.count(&e);
        Ok(a)
    }

    /// A ratio, in millionths.
    pub fn ratio(&mut self, t: &Token, line: u32) -> Result<i64, Bad> {
        let e = decimal::millionths(t.text)
            .map_err(|err| Bad::new(line, t.column, format!("{}: {err}", t.text)))?;
        self.count(&e);
        Ok(e.value)
    }

    /// How many numbers were rounded.
    pub fn rounded(&self) -> u64 {
        self.rounded
    }

    /// Adds another reader's counts (a file read in parts).
    pub fn absorb(&mut self, o: &Nums) {
        self.rounded += o.rounded;
        self.worst = self.worst.max(o.worst);
    }

    /// The file's `odb_rounded` warning, if it rounded anything.
    pub fn warning(&self, path: &str) -> Option<Notice> {
        (self.rounded > 0).then(|| {
            notice(
                warning::ROUNDED,
                None,
                format!(
                    "{} numbers rounded to the nanometre or millidegree, the largest by {}.{:06} nm",
                    self.rounded,
                    self.worst / 1_000_000,
                    self.worst % 1_000_000
                ),
                Some(location(path, None, None)),
            )
        })
    }
}

/// The units a file's `UNITS=` line names, if it has one that reads.
pub fn units_of(text: &str) -> Option<Units> {
    super::text::lines(text)
        .find_map(|(_, l)| l.strip_prefix("UNITS"))
        .and_then(|rest| rest.trim_start().strip_prefix('='))
        .and_then(|u| Units::parse(u.trim()))
}

/// The text of a token, for numbers parsed by hand.
pub fn token(text: &str) -> Token<'_> {
    Token {
        text,
        column: 1,
        quoted: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_name_their_units() {
        assert_eq!(Units::parse("mm"), Some(Units::Mm));
        assert_eq!(Units::parse("INCH"), Some(Units::Inch));
        assert_eq!(Units::parse("cm"), None);
        assert_eq!(
            (Units::Inch.coord(), Units::Inch.size()),
            (Unit::Inch, Unit::Mil)
        );
        assert_eq!(
            (Units::Mm.coord(), Units::Mm.size()),
            (Unit::Mm, Unit::Micron)
        );
        assert_eq!(Units::Mm.names(), NameUnits::Micron);
    }

    #[test]
    fn numbers_are_read_exactly_and_rounding_is_counted() {
        let mut n = Nums::new(Units::Inch);
        assert_eq!(n.coord(&token("1.5"), 1).unwrap(), 38_100_000);
        assert_eq!(n.size(&token("39.3701"), 1).unwrap(), 1_000_001);
        assert_eq!(n.angle(&token("90"), 1).unwrap(), Mdeg::new(90_000));
        assert_eq!(n.rounded(), 1);
        assert_eq!(
            n.point(&token("0.00001"), &token("-1"), 1).unwrap(),
            Point::new(254, -25_400_000)
        );
        let w = n.warning("steps/pcb/layers/top/features").unwrap();
        assert_eq!(w.code, "odb_rounded");
        // 39.3701 mil is 999,999.54 nm: rounded up by 0.46 nm.
        assert_eq!(
            w.message,
            "1 numbers rounded to the nanometre or millidegree, the largest by 0.460000 nm"
        );
        assert!(Nums::new(Units::Mm).warning("x").is_none());
        let e = n
            .coord(
                &Token {
                    text: "1.2.3",
                    column: 9,
                    quoted: false,
                },
                4,
            )
            .unwrap_err();
        assert_eq!((e.line, e.column), (4, 9));
    }

    #[test]
    fn a_files_units() {
        assert_eq!(units_of("#\nUNITS=MM\nX=1\n"), Some(Units::Mm));
        assert_eq!(units_of("UNITS = inch\n"), Some(Units::Inch));
        assert_eq!(units_of("X=1\n"), None);
    }

    #[test]
    fn the_largest_change_survives_huge_denominators() {
        let mut n = Nums::new(Units::Mm);
        // 1e-7 mm is 0.1 nm, rounded down.
        n.coord(&token("0.0000001"), 1).unwrap();
        let mut m = Nums::new(Units::Mm);
        m.coord(&token(&format!("0.00000049{}", "9".repeat(29))), 1)
            .unwrap();
        n.absorb(&m);
        assert_eq!(n.rounded(), 2);
        assert!(n.warning("f").unwrap().message.ends_with("by 0.499999 nm"));
    }
}
