//! A reader of Gerber and Excellon exactly as this crate writes them, for
//! tests: one statement a line, the statements `gerber::write` and
//! `excellon::write` use and nothing else. It reads this crate's output
//! back into shapes and attributes; it is not a general reader (ryudfm's
//! stage 6 brings one), and gerbv in the devcontainer is the third-party
//! check.

use crate::excellon::Drill;
use crate::recognise::Aperture;
use ryu_board::geom::{Arc, Contour, Dir, Edge, Nm, Pen, Point, Region, Stroke};
use ryu_board::model::Polarity;
use ryu_board::shape::{Part, Piece, Shape};
use std::collections::BTreeMap;

/// What one object is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Flash { at: Point, aperture: Aperture },
    Draw { path: Vec<Edge>, width: Nm },
    Region(Contour),
}

/// One object read: dark or clear, its aperture's or region's
/// `.AperFunction`, and the object attributes in force (`.N`, `.P`, `.C`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    pub kind: Kind,
    pub dark: bool,
    pub function: Option<String>,
    pub attrs: BTreeMap<String, String>,
}

impl Object {
    /// The object as a shape, as `ryu-board` draws it.
    pub fn shape(&self) -> Shape {
        let polarity = if self.dark {
            Polarity::Positive
        } else {
            Polarity::Negative
        };
        let piece = match &self.kind {
            Kind::Flash { at, aperture } => {
                Piece::Region(aperture.region(*at).expect("a standard aperture"))
            }
            Kind::Draw { path, width } => Piece::Stroke(Stroke {
                path: path.clone(),
                pen: Pen::Round(*width),
            }),
            Kind::Region(c) => Piece::Region(Region::solid(c.clone())),
        };
        Shape {
            parts: vec![Part { polarity, piece }],
            ..Shape::default()
        }
    }
}

/// A Gerber file read: its file attributes (`.FileFunction` → `Copper,L1,Top`)
/// and objects in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    pub attrs: BTreeMap<String, String>,
    pub objects: Vec<Object>,
}

fn int(s: &str) -> Result<Nm, String> {
    s.parse().map_err(|_| format!("not a number: {s:?}"))
}

/// Millimetres with at most six decimals, as nanometres.
pub fn nm(s: &str) -> Result<Nm, String> {
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (i, f) = s.split_once('.').unwrap_or((s, ""));
    if i.is_empty() || f.len() > 6 {
        return Err(format!("not millimetres: {s:?}"));
    }
    let frac = if f.is_empty() {
        0
    } else {
        int(&format!("{f:0<6}"))?
    };
    let v = int(i)? * 1_000_000 + frac;
    Ok(if neg { -v } else { v })
}

/// `10C,0.250000`, `11R,1.000000X0.500000`, `12O,…`: the D code and the
/// aperture.
pub fn aperture(def: &str) -> Result<(u32, Aperture), String> {
    let digits: String = def.chars().take_while(char::is_ascii_digit).collect();
    let n: u32 = digits.parse().map_err(|_| format!("no D code: {def}"))?;
    let (t, params) = def[digits.len()..]
        .split_once(',')
        .ok_or(format!("no parameters: {def}"))?;
    let p: Vec<Nm> = params.split('X').map(nm).collect::<Result<_, _>>()?;
    let a = match (t, p.as_slice()) {
        ("C", [d]) => Aperture::Circle(*d),
        ("R", [w, h]) => Aperture::Rect(*w, *h),
        ("O", [w, h]) => Aperture::Obround(*w, *h),
        _ => return Err(format!("not a standard aperture: {def}")),
    };
    Ok((n, a))
}

/// `X…Y…I…J…`'s fields, before the `D` code.
fn fields(s: &str) -> Result<BTreeMap<char, Nm>, String> {
    let mut out = BTreeMap::new();
    let mut key: Option<char> = None;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_alphabetic() {
            if let Some(k) = key.take() {
                out.insert(k, int(&num)?);
                num.clear();
            }
            key = Some(c);
        } else {
            num.push(c);
        }
    }
    if let Some(k) = key {
        out.insert(k, int(&num)?);
    }
    Ok(out)
}

#[derive(Default)]
struct Reader {
    img: Image,
    apertures: BTreeMap<u32, (Aperture, Option<String>)>,
    function: Option<String>,
    attrs: BTreeMap<String, String>,
    clear: bool,
    mode: u8,
    current: Option<u32>,
    at: Option<Point>,
    region: Option<Vec<Edge>>,
    draw: Vec<Edge>,
}

impl Reader {
    fn object(&mut self, kind: Kind, function: Option<String>) {
        self.img.objects.push(Object {
            kind,
            dark: !self.clear,
            function,
            attrs: self.attrs.clone(),
        });
    }

    fn aperture(&self) -> Result<(Aperture, Option<String>), String> {
        let n = self.current.ok_or("no aperture selected")?;
        self.apertures
            .get(&n)
            .cloned()
            .ok_or(format!("D{n} isn't defined"))
    }

    /// A drawn path ends at anything but one more `D01` of it.
    fn flush(&mut self) -> Result<(), String> {
        if self.draw.is_empty() {
            return Ok(());
        }
        let path = std::mem::take(&mut self.draw);
        let (a, f) = self.aperture()?;
        let Aperture::Circle(width) = a else {
            return Err("a draw with a non-circular aperture".into());
        };
        self.object(Kind::Draw { path, width }, f);
        Ok(())
    }

    /// One line; `true` at `M02`.
    fn line(&mut self, l: &str) -> Result<bool, String> {
        let l = l.trim();
        if l.is_empty() {
            return Ok(false);
        }
        if let Some(x) = l.strip_prefix('%') {
            let body = x.strip_suffix("*%").ok_or(format!("unterminated: {l}"))?;
            self.flush()?;
            if let Some(a) = body.strip_prefix("TF.") {
                let (k, v) = a.split_once(',').ok_or(format!("bad attribute: {l}"))?;
                self.img.attrs.insert(format!(".{k}"), v.to_owned());
            } else if let Some(v) = body.strip_prefix("TA.AperFunction,") {
                self.function = Some(v.to_owned());
            } else if body == "TD.AperFunction" {
                self.function = None;
            } else if let Some(a) = body.strip_prefix("TO.") {
                let (k, v) = a.split_once(',').ok_or(format!("bad attribute: {l}"))?;
                self.attrs.insert(format!(".{k}"), v.to_owned());
            } else if body == "TD" {
                self.attrs.clear();
                self.function = None;
            } else if body == "LPD" {
                self.clear = false;
            } else if body == "LPC" {
                self.clear = true;
            } else if let Some(d) = body.strip_prefix("ADD") {
                let (n, a) = aperture(d)?;
                self.apertures.insert(n, (a, self.function.clone()));
            } else if body != "FSLAX46Y46" && body != "MOMM" {
                return Err(format!("not read here: {l}"));
            }
            return Ok(false);
        }
        let s = l.strip_suffix('*').ok_or(format!("unterminated: {l}"))?;
        match s {
            "G75" => return Ok(false),
            "G01" | "G02" | "G03" => {
                self.mode = s[2..].parse().expect("1 to 3");
                return Ok(false);
            }
            "G36" => {
                self.flush()?;
                self.region = Some(Vec::new());
                self.at = None;
                return Ok(false);
            }
            "G37" => {
                let edges = self.region.take().ok_or("G37 outside a region")?;
                let c = Contour::new(edges).map_err(|e| format!("an open region: {e:?}"))?;
                let f = self.function.clone();
                self.object(Kind::Region(c), f);
                self.at = None;
                return Ok(false);
            }
            "M02" => {
                self.flush()?;
                return Ok(true);
            }
            _ => {}
        }
        if let Some(n) = s.strip_prefix('D')
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            self.flush()?;
            self.current = Some(int(n)? as u32);
            return Ok(false);
        }
        let (coords, d) = s.rsplit_once('D').ok_or(format!("not read here: {l}"))?;
        let f = fields(coords)?;
        let to = Point::new(
            *f.get(&'X').ok_or(format!("no X: {l}"))?,
            *f.get(&'Y').ok_or(format!("no Y: {l}"))?,
        );
        match d {
            "02" => {
                self.flush()?;
                if self.region.as_ref().is_some_and(|r| !r.is_empty()) {
                    return Err("two contours in one region".into());
                }
                self.at = Some(to);
            }
            "01" => {
                let from = self.at.ok_or(format!("D01 with no current point: {l}"))?;
                let e = match self.mode {
                    1 => Edge::line(from, to),
                    m => Edge::Arc(Arc {
                        start: from,
                        end: to,
                        centre: from.offset(
                            *f.get(&'I').ok_or(format!("no I: {l}"))?,
                            *f.get(&'J').ok_or(format!("no J: {l}"))?,
                        ),
                        dir: if m == 2 { Dir::Cw } else { Dir::Ccw },
                    }),
                };
                match &mut self.region {
                    Some(r) => r.push(e),
                    None => self.draw.push(e),
                }
                self.at = Some(to);
            }
            "03" => {
                self.flush()?;
                let (a, fun) = self.aperture()?;
                self.object(
                    Kind::Flash {
                        at: to,
                        aperture: a,
                    },
                    fun,
                );
                self.at = Some(to);
            }
            _ => return Err(format!("not read here: {l}")),
        }
        Ok(false)
    }
}

/// A Gerber file as `gerber::write` writes it.
pub fn gerber(text: &str) -> Result<Image, String> {
    let mut r = Reader {
        mode: 1,
        ..Reader::default()
    };
    for l in text.lines() {
        if r.line(l)? {
            return Ok(r.img);
        }
    }
    Err("no M02".into())
}

/// A drill file read: its file attributes, its tools (number, diameter,
/// function) in order, and its holes with their tools' functions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrillImage {
    pub attrs: BTreeMap<String, String>,
    pub tools: Vec<(u32, Nm, Option<String>)>,
    pub holes: Vec<(Drill, Option<String>)>,
}

fn point(s: &str) -> Result<Point, String> {
    let s = s.strip_prefix('X').ok_or(format!("no X: {s}"))?;
    let (x, y) = s.split_once('Y').ok_or(format!("no Y: {s}"))?;
    Ok(Point::new(nm(x)?, nm(y)?))
}

/// An Excellon file as `excellon::write` writes it, in either slot form.
pub fn excellon(text: &str) -> Result<DrillImage, String> {
    let mut img = DrillImage::default();
    let mut header = true;
    let mut pending: Option<String> = None;
    let mut tools: BTreeMap<u32, (Nm, Option<String>)> = BTreeMap::new();
    let mut tool: Option<u32> = None;
    let (mut start, mut end): (Option<Point>, Option<Point>) = (None, None);
    let current = |tool: Option<u32>, tools: &BTreeMap<u32, (Nm, Option<String>)>| {
        tool.and_then(|t| tools.get(&t).cloned())
            .ok_or_else(|| "a hole with no tool".to_owned())
    };
    for l in text.lines().map(str::trim) {
        if let Some(c) = l.strip_prefix(';') {
            let c = c.trim();
            if let Some(a) = c.strip_prefix("#@! TF.") {
                let (k, v) = a.split_once(',').ok_or(format!("bad attribute: {l}"))?;
                img.attrs.insert(format!(".{k}"), v.to_owned());
            } else if let Some(v) = c.strip_prefix("#@! TA.AperFunction,") {
                pending = Some(v.to_owned());
            }
            continue;
        }
        if header {
            match l {
                "M48" | "FMAT,2" | "METRIC" => {}
                "%" => header = false,
                t if t.starts_with('T') => {
                    let (n, d) = t[1..].split_once('C').ok_or(format!("bad tool: {l}"))?;
                    let n = int(n)? as u32;
                    let d = nm(d)?;
                    img.tools.push((n, d, pending.clone()));
                    tools.insert(n, (d, pending.take()));
                }
                _ => return Err(format!("not read here: {l}")),
            }
            continue;
        }
        match l {
            "G90" | "G05" | "M15" => {}
            "M30" => return Ok(img),
            "T0" => tool = None,
            "M16" => {
                let (a, b) = (
                    start.take().ok_or("M16 with no G00")?,
                    end.take().ok_or("M16 with no G01")?,
                );
                let (d, f) = current(tool, &tools)?;
                img.holes.push((Drill::Slot { a, b, d }, f));
            }
            t if t.starts_with('T') => tool = Some(int(&t[1..])? as u32),
            g if g.starts_with("G00") => start = Some(point(&g[3..])?),
            g if g.starts_with("G01") => end = Some(point(&g[3..])?),
            c if c.starts_with('X') => {
                let (d, f) = current(tool, &tools)?;
                let drill = match c.split_once("G85") {
                    Some((a, b)) => Drill::Slot {
                        a: point(a)?,
                        b: point(b)?,
                        d,
                    },
                    None => Drill::Round { at: point(c)?, d },
                };
                img.holes.push((drill, f));
            }
            _ => return Err(format!("not read here: {l}")),
        }
    }
    Err("no M30".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_apertures_read() {
        assert_eq!(nm("1.5"), Ok(1_500_000));
        assert_eq!(nm("-0.000001"), Ok(-1));
        assert_eq!(nm("12"), Ok(12_000_000));
        assert!(nm("1.0000001").is_err());
        assert_eq!(
            aperture("10C,0.250000"),
            Ok((10, Aperture::Circle(250_000)))
        );
        assert_eq!(
            aperture("11R,1.000000X0.500000"),
            Ok((11, Aperture::Rect(1_000_000, 500_000)))
        );
        assert_eq!(
            aperture("12O,0.6X1.2"),
            Ok((12, Aperture::Obround(600_000, 1_200_000)))
        );
    }

    #[test]
    fn a_small_file_reads() {
        let text = "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG75*\n\
                    %TA.AperFunction,ViaPad*%\n%ADD10C,0.600000*%\n%TD.AperFunction*%\n\
                    %TO.N,gnd*%\nD10*\nX1000Y2000D03*\n%TD*%\n\
                    G36*\nX0Y0D02*\nG01*\nX10Y0D01*\nX10Y10D01*\nX0Y0D01*\nG37*\nM02*\n";
        let img = gerber(text).unwrap();
        assert_eq!(img.attrs[".FileFunction"], "Copper,L1,Top");
        assert_eq!(img.objects.len(), 2);
        assert_eq!(
            img.objects[0].kind,
            Kind::Flash {
                at: Point::new(1000, 2000),
                aperture: Aperture::Circle(600_000)
            }
        );
        assert_eq!(img.objects[0].function.as_deref(), Some("ViaPad"));
        assert_eq!(img.objects[0].attrs[".N"], "gnd");
        assert!(img.objects[1].attrs.is_empty());
        assert!(matches!(img.objects[1].kind, Kind::Region(_)));
        assert!(gerber("%FSLAX46Y46*%\n").is_err(), "no M02");
    }
}
