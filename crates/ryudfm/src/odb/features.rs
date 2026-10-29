//! Features files: a layer's graphics, a user symbol's, or a profile.
//!
//! A file is a header (`UNITS=`, `ID=`, `F`), the symbol table (`$n name`,
//! with `I` or `M` after a name whose numbers are in mils or microns
//! whatever the file's units), the attribute lookup table (`@n`, `&n`),
//! then records: `L` lines, `P` pads, `A` arcs, `T` text, `B` barcodes
//! and `S` surfaces (`OB`, `OS`, `OC`, `OE` polygons, then `SE`). Each
//! record keeps its polarity, D-code, attributes and `ID=`.
//!
//! ODB++ turns pads and text clockwise and then mirrors x; the model
//! mirrors and then turns counterclockwise, which is the same placement
//! when a mirrored angle `a` is `Mdeg::new(a)` and an unmirrored one
//! `Mdeg::from_clockwise(a)`. Surfaces' polygons are read by
//! [`super::outline`].

use std::collections::BTreeSet;

use ryu_board::geom::decimal;
use ryu_board::geom::{Arc, Dir, Mdeg, Place, Point};
use ryu_board::model::{Barcode, Feature, FeatureKind, Features, Notice, Polarity, Symbol, Text};
use ryu_board::symbol::StandardSymbol;

use super::attrs::{Decls, Table, tail};
use super::error::{Once, location, warning};
use super::outline::{ARC_SLACK, Outline, dir};
use super::text::{Bad, Token, lines, split_record, tokens};
use super::units::{Nums, Units};

/// What a features file is read with.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// The file's path in the job, for locations.
    pub path: &'a str,
    /// The units when the file has no `UNITS=`.
    pub units: Units,
    pub decls: &'a Decls,
    /// The job's user symbols, by lower-case name.
    pub user: &'a BTreeSet<String>,
}

/// A features file, read: its symbol table and features, from which
/// [`Parsed::into_features`] builds the model's table once anything else
/// (nets) is filled in.
#[derive(Clone, Debug)]
pub struct Parsed {
    pub symbols: Vec<Symbol>,
    pub features: Vec<Feature>,
    /// The file's own `ID=`.
    pub id: Option<u64>,
    pub nums: Nums,
    /// Warnings, each kind once.
    pub warnings: Vec<Notice>,
    /// Symbol names features use that are neither standard nor user
    /// symbols, sorted.
    pub unknown: Vec<String>,
}

struct Reader<'a> {
    cx: &'a Context<'a>,
    nums: Nums,
    table: Table,
    once: Once,
    line: u32,
}

impl Reader<'_> {
    fn bad(&self, column: u32, message: impl Into<String>) -> Bad {
        Bad::new(self.line, column, message)
    }

    fn at(&self) -> ryu_board::model::Location {
        location(self.cx.path, Some(self.line), None)
    }

    fn ignore(&mut self, key: &str, message: String) {
        let at = self.at();
        self.once.add(warning::IGNORED, key, None, || message, at);
    }

    /// The `n`th field, or a fault saying the record is short.
    fn field<'t>(&self, toks: &'t [Token<'t>], n: usize, what: &str) -> Result<&'t Token<'t>, Bad> {
        toks.get(n).ok_or_else(|| {
            let col = toks.last().map_or(1, |t| t.column + t.text.len() as u32);
            self.bad(col, format!("{what} missing"))
        })
    }

    fn coord(&mut self, toks: &[Token], n: usize, what: &str) -> Result<i64, Bad> {
        let t = *self.field(toks, n, what)?;
        self.nums.coord(&t, self.line)
    }

    fn point(&mut self, toks: &[Token], n: usize, what: &str) -> Result<Point, Bad> {
        Ok(Point::new(
            self.coord(toks, n, what)?,
            self.coord(toks, n + 1, what)?,
        ))
    }

    fn index(&self, toks: &[Token], n: usize, what: &str) -> Result<u32, Bad> {
        let t = self.field(toks, n, what)?;
        t.text
            .parse()
            .map_err(|_| self.bad(t.column, format!("{what} {}: not an index", t.text)))
    }

    fn polarity(&self, toks: &[Token], n: usize) -> Result<Polarity, Bad> {
        let t = self.field(toks, n, "polarity")?;
        match t.text {
            "P" => Ok(Polarity::Positive),
            "N" => Ok(Polarity::Negative),
            other => Err(self.bad(t.column, format!("polarity {other}: not P or N"))),
        }
    }

    fn dcode(&self, toks: &[Token], n: usize) -> Result<Option<i32>, Bad> {
        let t = self.field(toks, n, "dcode")?;
        let d: i32 = t
            .text
            .parse()
            .map_err(|_| self.bad(t.column, format!("dcode {}: not a number", t.text)))?;
        Ok((d != 0).then_some(d))
    }

    fn cw(&self, toks: &[Token], n: usize) -> Result<Dir, Bad> {
        dir(Some(self.field(toks, n, "direction")?), self.line)
    }

    /// An orientation from field `n` (`0` to `7`, or `8`/`9` and an angle):
    /// the turn, the mirror, and the next field's index.
    fn orient(&mut self, toks: &[Token], n: usize) -> Result<(Mdeg, bool, usize), Bad> {
        let t = *self.field(toks, n, "orientation")?;
        let (cw, mirror, next) = match t.text {
            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" => {
                let c = i64::from(t.text.as_bytes()[0] - b'0');
                ((c % 4) * 90_000, c >= 4, n + 1)
            }
            "8" | "9" => {
                let a = *self.field(toks, n + 1, "rotation")?;
                let a = self.nums.angle(&a, self.line)?;
                (i64::from(a.get()), t.text == "9", n + 2)
            }
            other => return Err(self.bad(t.column, format!("orientation {other}: not 0 to 9"))),
        };
        let rot = if mirror {
            Mdeg::new(cw)
        } else {
            Mdeg::from_clockwise(cw)
        };
        Ok((rot, mirror, next))
    }

    fn inconsistent(&mut self) {
        let at = self.at();
        self.once.add(
            warning::ARC_INCONSISTENT,
            "",
            None,
            || "an arc whose end is more than 1 µm off its circle".into(),
            at,
        );
    }

    /// The attributes and ID after a record's fields.
    fn tail(&mut self, rest: Option<&str>, f: &mut Feature) -> Result<(), Bad> {
        let (list, id) = tail(rest);
        let (attributes, ignored) = self.table.values(list);
        f.attributes = attributes;
        for i in ignored {
            let msg = format!("attribute {}: {}", i.name, i.why);
            self.ignore(&format!("attribute {} {}", i.name, i.why), msg);
        }
        if let Some(id) = id {
            f.id = Some(
                id.parse()
                    .map_err(|_| self.bad(1, format!("ID={id}: not a number")))?,
            );
        }
        Ok(())
    }
}

/// A surface being read.
struct Surface {
    feature: Feature,
    outline: Outline,
}

impl Parsed {
    pub fn into_features(self) -> Features {
        Features::new(self.symbols, self.features)
    }
}

/// Reads a features file.
pub fn features(text: &str, cx: &Context) -> Result<Parsed, Bad> {
    let mut r = Reader {
        cx,
        nums: Nums::new(cx.units),
        table: Table::default(),
        once: Once::default(),
        line: 0,
    };
    let mut id = None;
    let mut symbols: Vec<Option<Symbol>> = Vec::new();
    let mut out: Vec<Feature> = Vec::new();
    let mut surface: Option<Surface> = None;
    for (n, line) in lines(text) {
        r.line = n;
        if let Some(s) = &mut surface {
            let toks = tokens(line).map_err(|b| r.bad(b.column, b.message))?;
            if toks[0].text != "SE" {
                s.outline.line(&toks, &mut r.nums, n)?;
                continue;
            }
            let s = surface.take().unwrap();
            if s.outline.inconsistent > 0 {
                r.inconsistent();
            }
            let regions = s.outline.finish(n)?;
            if regions.is_empty() {
                return Err(r.bad(1, "a surface with no polygon"));
            }
            let mut f = s.feature;
            f.kind = FeatureKind::Surface { regions };
            out.push(f);
            continue;
        }
        if let Some(u) = line.strip_prefix("UNITS=") {
            r.nums.units = Units::parse(u.trim())
                .ok_or_else(|| r.bad(7, format!("UNITS={u}: not MM or INCH")))?;
            continue;
        }
        if let Some(v) = line.strip_prefix("ID=") {
            id = Some(
                v.trim()
                    .parse()
                    .map_err(|_| r.bad(4, format!("ID={v}: not a number")))?,
            );
            continue;
        }
        if let Some(rest) = line.strip_prefix('$') {
            let (i, s) = symbol(&mut r, rest)?;
            if symbols.len() <= i {
                symbols.resize(i + 1, None);
            }
            symbols[i] = Some(s);
            continue;
        }
        if let Some(rest) = line.strip_prefix('@') {
            r.table.name(rest, n, cx.decls)?;
            continue;
        }
        if let Some(rest) = line.strip_prefix('&') {
            r.table.text(rest, n)?;
            continue;
        }
        let (fields, rest) = split_record(line);
        let toks = tokens(fields).map_err(|b| r.bad(b.column, b.message))?;
        let kind = toks.first().map_or("", |t| t.text);
        match kind {
            "F" => {}
            "L" | "P" | "A" | "T" | "B" => {
                let mut f = record(&mut r, kind, &toks)?;
                r.tail(rest, &mut f)?;
                out.push(f);
            }
            "S" => {
                let mut f = Feature::new(
                    FeatureKind::Surface {
                        regions: Vec::new(),
                    },
                    r.polarity(&toks, 1)?,
                );
                f.dcode = r.dcode(&toks, 2)?;
                r.tail(rest, &mut f)?;
                surface = Some(Surface {
                    feature: f,
                    outline: Outline::default(),
                });
            }
            other => r.ignore(&format!("record {other}"), format!("record kind {other}")),
        }
    }
    if surface.is_some() {
        return Err(Bad::new(r.line, 1, "a surface without SE"));
    }
    let symbols: Vec<Symbol> = symbols
        .into_iter()
        .enumerate()
        .map(|(i, s)| s.unwrap_or_else(|| Symbol::Unknown(format!("${i}"))))
        .collect();
    let mut unknown = BTreeSet::new();
    for f in &out {
        let used = match &f.kind {
            FeatureKind::Line { symbol, .. }
            | FeatureKind::Arc { symbol, .. }
            | FeatureKind::Pad { symbol, .. } => *symbol as usize,
            _ => continue,
        };
        match symbols.get(used) {
            Some(Symbol::Unknown(name)) => {
                unknown.insert(name.clone());
            }
            None => {
                unknown.insert(format!("${used}"));
            }
            _ => {}
        }
    }
    Ok(Parsed {
        symbols,
        features: out,
        id,
        nums: r.nums,
        warnings: r.once.into_vec(),
        unknown: unknown.into_iter().collect(),
    })
}

/// A `$n name [I|M]` line without its `$`.
fn symbol(r: &mut Reader, rest: &str) -> Result<(usize, Symbol), Bad> {
    let toks = tokens(rest).map_err(|b| r.bad(b.column + 1, b.message))?;
    let i: usize = toks
        .first()
        .and_then(|t| t.text.parse().ok())
        .filter(|&i| i <= 1_000_000)
        .ok_or_else(|| r.bad(2, "a symbol without an index"))?;
    let name = toks
        .get(1)
        .ok_or_else(|| r.bad(2, "a symbol without a name"))?
        .text
        .to_ascii_lowercase();
    let units = match toks.get(2).map(|t| t.text) {
        None => r.nums.units,
        Some("I") | Some("i") => Units::Inch,
        Some("M") | Some("m") => Units::Mm,
        Some(other) => {
            return Err(r.bad(toks[2].column + 1, format!("{other}: not I or M")));
        }
    };
    let s = match StandardSymbol::parse(&name, units.names()) {
        Some(s) => Symbol::Standard(s),
        None if r.cx.user.contains(&name) => Symbol::User(name),
        None => Symbol::Unknown(name),
    };
    Ok((i, s))
}

/// An `L`, `P`, `A`, `T` or `B` record's fields.
fn record(r: &mut Reader, kind: &str, t: &[Token]) -> Result<Feature, Bad> {
    Ok(match kind {
        "L" => {
            let a = r.point(t, 1, "start")?;
            let b = r.point(t, 3, "end")?;
            let symbol = r.index(t, 5, "symbol")?;
            let mut f = Feature::new(FeatureKind::Line { a, b, symbol }, r.polarity(t, 6)?);
            f.dcode = r.dcode(t, 7)?;
            f
        }
        "P" => {
            let at = r.point(t, 1, "centre")?;
            // `-1 <symbol> <resize>` resizes; the factor isn't taken.
            let (symbol, next) = if r.field(t, 3, "symbol")?.text == "-1" {
                r.ignore("resize", "pad resize factors".into());
                (r.index(t, 4, "symbol")?, 6)
            } else {
                (r.index(t, 3, "symbol")?, 4)
            };
            let polarity = r.polarity(t, next)?;
            let dcode = r.dcode(t, next + 1)?;
            let (rot, mirror, _) = r.orient(t, next + 2)?;
            let place = Place {
                at,
                rot,
                mirror,
                scale: None,
            };
            let mut f = Feature::new(FeatureKind::Pad { symbol, place }, polarity);
            f.dcode = dcode;
            f
        }
        "A" => {
            let start = r.point(t, 1, "start")?;
            let end = r.point(t, 3, "end")?;
            let centre = r.point(t, 5, "centre")?;
            let symbol = r.index(t, 7, "symbol")?;
            let polarity = r.polarity(t, 8)?;
            let dcode = r.dcode(t, 9)?;
            let dir = r.cw(t, 10)?;
            let arc = Arc {
                start,
                end,
                centre,
                dir,
            };
            if arc.end_error() > ARC_SLACK {
                r.inconsistent();
            }
            let mut f = Feature::new(FeatureKind::Arc { arc, symbol }, polarity);
            f.dcode = dcode;
            f
        }
        "T" => {
            let at = r.point(t, 1, "location")?;
            let font = r.field(t, 3, "font")?.text.to_ascii_lowercase();
            let polarity = r.polarity(t, 4)?;
            let (rot, mirror, n) = r.orient(t, 5)?;
            let width = r.coord(t, n, "character width")?;
            let height = r.coord(t, n + 1, "character height")?;
            let wf = *r.field(t, n + 2, "width factor")?;
            // The width factor is in units of 12 mils, whatever the units.
            let line_width = decimal::scaled(wf.text, 304_800, 1 << 40)
                .map_err(|e| r.bad(wf.column, format!("{}: {e}", wf.text)))?
                .value;
            let s = r.field(t, n + 3, "text")?;
            if !s.quoted {
                return Err(r.bad(s.column, "text not in quotes"));
            }
            let string = s.text.to_string();
            if string.contains("$$") {
                let at = r.at();
                r.once.add(
                    warning::TEXT_DYNAMIC,
                    "",
                    None,
                    || "dynamic text, kept as written".into(),
                    at,
                );
            }
            if r.field(t, n + 4, "version")?.text == "1" {
                r.ignore(
                    "version 1",
                    "text placed by its widest character (version 1)".into(),
                );
            }
            Feature::new(
                FeatureKind::Text(Text {
                    font,
                    place: Place {
                        at,
                        rot,
                        mirror,
                        scale: None,
                    },
                    width,
                    height,
                    line_width,
                    string,
                }),
                polarity,
            )
        }
        _ => {
            let at = r.point(t, 1, "location")?;
            let kind = r.field(t, 3, "barcode")?.text.to_string();
            let polarity = r.polarity(t, 5)?;
            let (rot, mirror, n) = r.orient(t, 6)?;
            let s = r.field(t, n + 8, "text")?;
            let string = s.text.to_string();
            let at_ = r.at();
            r.once.add(
                warning::BARCODE_SKIPPED,
                "",
                None,
                || "a barcode, kept and left out of shapes".into(),
                at_,
            );
            Feature::new(
                FeatureKind::Barcode(Barcode {
                    place: Place {
                        at,
                        rot,
                        mirror,
                        scale: None,
                    },
                    kind,
                    string,
                }),
                polarity,
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_board::geom::Edge;
    use ryu_board::model::AttrValue;
    use ryu_board::symbol::StandardSymbol as S;

    fn read(text: &str) -> Result<Parsed, Bad> {
        let decls = Decls::default();
        let user = BTreeSet::from(["punch_target".to_string()]);
        let cx = Context {
            path: "steps/pcb/layers/top/features",
            units: Units::Inch,
            decls: &decls,
            user: &user,
        };
        features(text, &cx)
    }

    const SPEC: &str = "UNITS=MM\nID=216\n#\nF 5\n$0 r120\n$1 rect20x60 M\n$2 rect3x5 I\n$3 punch_target\n$5 donut_r3x9\n\
@0 .smd\n@1 .nomenclature\n@2 .test_point\n@3 .geometry\n@4 .pad_usage\n&0 9796334\n&2 moire\n\
P -0.198 1.62 2 P 0 3;3=2,4=0;ID=123456\n\
L 3.834 -1.16 3.86728 -1.16 0 P 0 ;1,3=0\n\
A 1 0 -1 0 0 0 0 N 12 Y\n\
P 1.0 2.0 3 P 4 8 30.0;;ID=7205\n\
P 1.0 2.0 1 N 0 9 30.0\n";

    #[test]
    fn the_specifications_example() {
        let p = read(SPEC).unwrap();
        assert_eq!(p.id, Some(216));
        let s = p.symbols;
        assert_eq!(s.len(), 6);
        assert_eq!(s[0], Symbol::Standard(S::Round { d: 120_000 }));
        assert_eq!(
            s[1],
            Symbol::Standard(S::parse("rect20x60", Units::Mm.names()).unwrap())
        );
        assert_eq!(
            s[2],
            Symbol::Standard(S::parse("rect3x5", Units::Inch.names()).unwrap())
        );
        assert_eq!(s[3], Symbol::User("punch_target".into()));
        assert_eq!(s[4], Symbol::Unknown("$4".into()));
        let f = p.features;
        assert_eq!(f.len(), 5);
        // Rotated 270° clockwise, not mirrored: 90° counterclockwise.
        let FeatureKind::Pad { symbol, place } = &f[0].kind else {
            panic!()
        };
        assert_eq!(
            (*symbol, place.at, place.rot, place.mirror),
            (2, Point::new(-198_000, 1_620_000), Mdeg::new(90_000), false)
        );
        assert_eq!(f[0].id, Some(123456));
        assert_eq!(
            f[0].attributes.get(".geometry"),
            Some(&AttrValue::Text("moire".into()))
        );
        assert_eq!(
            f[0].attributes.get(".pad_usage"),
            Some(&AttrValue::Text("toeprint".into()))
        );
        let FeatureKind::Line { a, b, symbol } = &f[1].kind else {
            panic!()
        };
        assert_eq!(
            (*a, *b, *symbol),
            (
                Point::new(3_834_000, -1_160_000),
                Point::new(3_867_280, -1_160_000),
                0
            )
        );
        assert_eq!(f[1].attributes.get(".nomenclature"), Some(&AttrValue::Flag));
        let FeatureKind::Arc { arc, .. } = &f[2].kind else {
            panic!()
        };
        assert_eq!(
            (arc.dir, f[2].dcode, f[2].polarity),
            (Dir::Cw, Some(12), Polarity::Negative)
        );
        let FeatureKind::Pad { place, .. } = &f[3].kind else {
            panic!()
        };
        assert_eq!(
            (place.rot, f[3].dcode, f[3].id),
            (Mdeg::new(330_000), Some(4), Some(7205))
        );
        let FeatureKind::Pad { place, .. } = &f[4].kind else {
            panic!()
        };
        assert_eq!(
            (place.rot, place.mirror, f[4].polarity),
            (Mdeg::new(30_000), true, Polarity::Negative)
        );
        assert_eq!(
            f[1].attributes.get(".geometry"),
            Some(&AttrValue::Text("9796334".into()))
        );
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert!(p.unknown.is_empty());
        assert_eq!(p.nums.rounded(), 0);
    }

    #[test]
    fn surfaces_and_their_orientation() {
        let t = "UNITS=INCH\nS P 0;ID=9\nOB 0 0 I\nOS 0 1\nOS 1 1\nOS 1 0\nOS 0 0\nOE\n\
OB 0.25 0.25 H\nOS 0.75 0.25\nOS 0.75 0.75\nOS 0.25 0.75\nOS 0.25 0.25\nOE\n\
OB 2 0 I\nOC 2 0 2.5 0 Y\nOE\nSE\n";
        let p = read(t).unwrap();
        let f = &p.features[0];
        assert_eq!(f.id, Some(9));
        let FeatureKind::Surface { regions } = &f.kind else {
            panic!()
        };
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].outer.orientation(), Dir::Ccw);
        assert_eq!(regions[0].holes[0].orientation(), Dir::Cw);
        assert_eq!(regions[0].holes.len(), 1);
        // One inch squared, less a half inch squared, twice over.
        let inch = 25_400_000f64;
        assert!((regions[0].area2() - 2.0 * 0.75 * inch * inch).abs() < 1.0);
        let Edge::Arc(a) = regions[1].outer.edges()[0] else {
            panic!()
        };
        assert!(a.is_full());
        assert_eq!(regions[1].outer.orientation(), Dir::Ccw);
    }

    #[test]
    fn text_barcodes_and_warnings() {
        let t = "UNITS=INCH\n$0 r10\nT 1 2 standard P 8 90 0.2 0.1 0.5 'R1 $$DATE' 0\n\
T 0 0 Standard N 5 0.1 0.1 1 'x;y' 1\n\
B -1.7 0.25 UPC39 standard P 8 0 E 0.008 0.2 Y N Y Y T 'Barcode 1234567890';0\n\
P 0 0 -1 0 0.02 P 0 0\nX 1 2 3\nX 4\nA 1 0 -1 0.01 0 0 0 P 0 N\nL 0 0 1 1 7 P 0\n";
        let p = read(t).unwrap();
        let f = p.features;
        let FeatureKind::Text(x) = &f[0].kind else {
            panic!()
        };
        assert_eq!(x.font, "standard");
        assert_eq!((x.place.rot, x.place.mirror), (Mdeg::new(270_000), false));
        assert_eq!(
            (x.width, x.height, x.line_width),
            (5_080_000, 2_540_000, 152_400)
        );
        assert_eq!(x.string, "R1 $$DATE");
        let FeatureKind::Text(y) = &f[1].kind else {
            panic!()
        };
        assert_eq!(
            (
                y.string.as_str(),
                y.place.rot,
                y.place.mirror,
                y.font.as_str()
            ),
            ("x;y", Mdeg::new(90_000), true, "standard")
        );
        let FeatureKind::Barcode(b) = &f[2].kind else {
            panic!()
        };
        assert_eq!(
            (b.kind.as_str(), b.string.as_str()),
            ("UPC39", "Barcode 1234567890")
        );
        let codes: Vec<_> = p
            .warnings
            .iter()
            .map(|w| (w.code.as_str(), w.message.as_str()))
            .collect();
        assert_eq!(
            codes,
            [
                ("odb_text_dynamic", "dynamic text, kept as written"),
                (
                    "odb_ignored",
                    "text placed by its widest character (version 1)"
                ),
                (
                    "odb_barcode_skipped",
                    "a barcode, kept and left out of shapes"
                ),
                ("odb_ignored", "attribute @0: not in the attribute table"),
                ("odb_ignored", "pad resize factors"),
                ("odb_ignored", "record kind X (2 times)"),
                (
                    "odb_arc_inconsistent",
                    "an arc whose end is more than 1 µm off its circle"
                ),
            ]
        );
        assert_eq!(p.unknown, ["$7"]);
    }

    #[test]
    fn faults_say_where() {
        let at = |t: &str| {
            let b = read(t).unwrap_err();
            (b.line, b.column, b.message)
        };
        assert_eq!(at("L 0 0 1\n"), (1, 8, "end missing".into()));
        assert_eq!(at("L 0 0 1 x 0 P 0\n").1, 9);
        assert_eq!(at("L 0 0 1 1 0 Q 0\n").2, "polarity Q: not P or N");
        assert_eq!(at("P 0 0 0 P 0 12\n").2, "orientation 12: not 0 to 9");
        assert_eq!(at("UNITS=CM\n").2, "UNITS=CM: not MM or INCH");
        assert_eq!(
            at("S P 0\nOB 0 0 I\nOS 1 0\nOS 1 1\nOE\nSE\n").2,
            "a polygon that doesn't close"
        );
        assert_eq!(at("S P 0\nOB 0 0 H\n").2, "a hole before any island");
        assert_eq!(at("S P 0\nOB 0 0 I\nOS 1 0\n").2, "a surface without SE");
        assert_eq!(at("S P 0\nSE\n").2, "a surface with no polygon");
        assert_eq!(at("S P 0\nL 0 0 1 1 0 P 0\n").2, "L: not OB, OS, OC or OE");
        assert_eq!(
            at("T 0 0 standard P 0 1 1 1 bare 0\n").2,
            "text not in quotes"
        );
        assert_eq!(at("$x r10\n").2, "a symbol without an index");
        assert_eq!(at("L 0 0 1 1 0 P 0;;ID=x\n").2, "ID=x: not a number");
    }
}
