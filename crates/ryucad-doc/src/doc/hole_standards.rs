//! The project's own hole standards, `hole_standards.json` beside the
//! manifest: what ryucad doesn't ship (design 533, Project tables).
//!
//! - **New standards** (`threads` and `fasteners` under a name no
//!   built-in uses): a thread size's `pitch` or `tpi`, `major` and
//!   `minor` (basic, or `[min, max]` per class), `tap_drill`,
//!   `roll_form_drill`, `gauge_diameter`, `gauge_length`; a fastener
//!   size's `clearance` by fit (`close`, `normal`, `loose`),
//!   `counterbore {diameter, depth}`, `countersink {diameter, angle}`,
//!   `spotface {diameter}`.
//! - **Gaps in a built-in standard** (under `iso_metric`, `unc`, `npt`,
//!   ...): a size the built-in lacks, or a value a built-in size lacks.
//!   Never a value the built-in derives (`shadows_builtin`), so `M6x1.0-6H`
//!   means the same in every project.
//!
//! Every length carries its unit (`"5.6 mm"`, `"0.339 in"`); `tpi` is a
//! number. A part's revision hashes the entries its holes read (tag
//! `ryucad.hole_standards`).
//!
//! ```json
//! {
//!   "fasteners": {"house_cap": {"M5": {"clearance": {"normal": "5.6 mm"}}}},
//!   "format": 1,
//!   "kind": "ryucad.hole_standards",
//!   "threads": {"npt": {"1/8": {"tap_drill": "8.7 mm"}}}
//! }
//! ```

use super::canonical;
use super::files::{Obj, header, invalid};
use super::project::conflicted;
use super::source::Source;
use crate::error::{Code, Error, Result};
use crate::units::{Field, Quantity, Unit};
use ryu_standards::thread::{Standard, lookup};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const HOLE_STANDARDS: &str = "hole_standards.json";
pub const KIND: &str = "ryucad.hole_standards";
pub const FORMAT: u64 = 1;
/// The tag of the piece of a part's revision holding the entries its
/// holes read.
pub const TAG: &str = "ryucad.hole_standards";

/// The fits a fastener's clearance is given by.
pub const FITS: [&str; 3] = ["close", "normal", "loose"];

/// A diameter: basic, or `[min, max]` per class.
#[derive(Clone, Debug, PartialEq)]
pub enum Diameters {
    Basic(Quantity),
    Classes(BTreeMap<String, [Quantity; 2]>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadEntry {
    pub pitch: Option<Quantity>,
    pub tpi: Option<Quantity>,
    pub major: Option<Diameters>,
    pub minor: Option<Diameters>,
    pub tap_drill: Option<Quantity>,
    pub roll_form_drill: Option<Quantity>,
    pub gauge_diameter: Option<Quantity>,
    pub gauge_length: Option<Quantity>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FastenerEntry {
    /// By fit: `close`, `normal`, `loose`.
    pub clearance: BTreeMap<String, Quantity>,
    /// Diameter and depth.
    pub counterbore: Option<(Quantity, Quantity)>,
    /// Diameter and included angle.
    pub countersink: Option<(Quantity, Quantity)>,
    pub spotface: Option<Quantity>,
}

/// Which table an entry is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Thread,
    Fastener,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Thread => "threads",
            Kind::Fastener => "fasteners",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoleStandards {
    pub threads: BTreeMap<String, BTreeMap<String, ThreadEntry>>,
    pub fasteners: BTreeMap<String, BTreeMap<String, FastenerEntry>>,
}

/// A JSON pointer's escaping of one key (`1/8` is `1~18`).
fn esc(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

/// A standard's name: lower case letters, digits and `_`, a letter first.
fn check_standard(name: &str, at: &str) -> Result<()> {
    let ok = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(invalid(
            at,
            &format!("a standard's name is lower case letters, digits and _: not {name:?}"),
        ))
    }
}

/// A length with its unit, a literal (no parameters here).
fn length(v: &Value, at: &str) -> Result<Quantity> {
    let s = v
        .as_str()
        .filter(|s| s.contains(|c: char| c.is_ascii_alphabetic()))
        .ok_or_else(|| invalid(at, "expected a length with its unit, such as \"5.6 mm\""))?;
    let q = Quantity::parse(s, Unit::Mm).map_err(|e| e.with("pointer", at))?;
    if !q.is_literal() || q.meters() <= 0.0 {
        return Err(invalid(at, "expected a positive length, not an expression"));
    }
    Ok(q)
}

fn angle(v: &Value, at: &str) -> Result<Quantity> {
    let q = Quantity::from_json_as(v, Field::Angle, &|_| {
        Err(Error::new(Code::InvalidDocument, "no parameters here"))
    })
    .map_err(|e| e.with("pointer", at))?;
    Ok(q)
}

fn number(v: &Value, at: &str) -> Result<Quantity> {
    Quantity::from_json_as(v, Field::Number, &|_| {
        Err(Error::new(Code::InvalidDocument, "no parameters here"))
    })
    .map_err(|e| e.with("pointer", at))
}

impl Diameters {
    fn from_json(v: &Value, at: &str) -> Result<Diameters> {
        if v.is_string() {
            return Ok(Diameters::Basic(length(v, at)?));
        }
        let o = v
            .as_object()
            .ok_or_else(|| invalid(at, "a diameter is a length, or [min, max] by class"))?;
        let mut out = BTreeMap::new();
        for (class, pair) in o {
            let p = format!("{at}/{}", esc(class));
            let [lo, hi] = pair.as_array().map(Vec::as_slice).unwrap_or(&[]) else {
                return Err(invalid(&p, "a class's limits are [min, max]"));
            };
            let (lo, hi) = (length(lo, &p)?, length(hi, &p)?);
            if lo.meters() > hi.meters() {
                return Err(invalid(&p, "a class's min is past its max"));
            }
            out.insert(class.clone(), [lo, hi]);
        }
        Ok(Diameters::Classes(out))
    }

    fn to_json(&self) -> Value {
        match self {
            Diameters::Basic(q) => json!(q.text()),
            Diameters::Classes(m) => Value::Object(
                m.iter()
                    .map(|(k, [a, b])| (k.clone(), json!([a.text(), b.text()])))
                    .collect(),
            ),
        }
    }
}

impl ThreadEntry {
    fn from_json(v: &Value, at: &str) -> Result<ThreadEntry> {
        let mut o = Obj::new(v, at)?;
        let mut len = |k: &str| -> Result<Option<Quantity>> {
            o.take_opt(k)
                .map(|x| length(&x, &format!("{at}/{k}")))
                .transpose()
        };
        let pitch = len("pitch")?;
        let tap_drill = len("tap_drill")?;
        let roll_form_drill = len("roll_form_drill")?;
        let gauge_diameter = len("gauge_diameter")?;
        let gauge_length = len("gauge_length")?;
        let tpi = o
            .take_opt("tpi")
            .map(|x| number(&x, &format!("{at}/tpi")))
            .transpose()?;
        let major = o
            .take_opt("major")
            .map(|x| Diameters::from_json(&x, &format!("{at}/major")))
            .transpose()?;
        let minor = o
            .take_opt("minor")
            .map(|x| Diameters::from_json(&x, &format!("{at}/minor")))
            .transpose()?;
        o.finish()?;
        if pitch.is_some() && tpi.is_some() {
            return Err(invalid(at, "a size has a pitch or a tpi, not both"));
        }
        Ok(ThreadEntry {
            pitch,
            tpi,
            major,
            minor,
            tap_drill,
            roll_form_drill,
            gauge_diameter,
            gauge_length,
        })
    }

    fn to_json(&self) -> Value {
        let mut v = Map::new();
        for (k, q) in [
            ("pitch", &self.pitch),
            ("tpi", &self.tpi),
            ("tap_drill", &self.tap_drill),
            ("roll_form_drill", &self.roll_form_drill),
            ("gauge_diameter", &self.gauge_diameter),
            ("gauge_length", &self.gauge_length),
        ] {
            if let Some(q) = q {
                // A tpi is a number, written whole when it is (`27`, `11.5`).
                let value = match k {
                    "tpi" if q.number().fract() == 0.0 => json!(q.number() as i64),
                    "tpi" => json!(q.number()),
                    _ => json!(q.text()),
                };
                v.insert(k.into(), value);
            }
        }
        for (k, d) in [("major", &self.major), ("minor", &self.minor)] {
            if let Some(d) = d {
                v.insert(k.into(), d.to_json());
            }
        }
        Value::Object(v)
    }
}

impl FastenerEntry {
    fn from_json(v: &Value, at: &str) -> Result<FastenerEntry> {
        let mut o = Obj::new(v, at)?;
        let mut clearance = BTreeMap::new();
        if let Some(c) = o.take_opt("clearance") {
            let c = c
                .as_object()
                .ok_or_else(|| invalid(&format!("{at}/clearance"), "clearance is by fit"))?;
            for (fit, q) in c {
                let p = format!("{at}/clearance/{}", esc(fit));
                if !FITS.contains(&fit.as_str()) {
                    return Err(invalid(&p, "a fit is close, normal or loose"));
                }
                clearance.insert(fit.clone(), length(q, &p)?);
            }
        }
        let pair = |o: &mut Obj,
                    k: &str,
                    second: &str,
                    angled: bool|
         -> Result<Option<(Quantity, Quantity)>> {
            let Some(x) = o.take_opt(k) else {
                return Ok(None);
            };
            let p = format!("{at}/{k}");
            let mut m = Obj::new(&x, &p)?;
            let d = length(&m.take("diameter")?, &format!("{p}/diameter"))?;
            let s = m.take(second)?;
            let s = if angled {
                angle(&s, &format!("{p}/{second}"))?
            } else {
                length(&s, &format!("{p}/{second}"))?
            };
            m.finish()?;
            Ok(Some((d, s)))
        };
        let counterbore = pair(&mut o, "counterbore", "depth", false)?;
        let countersink = pair(&mut o, "countersink", "angle", true)?;
        let spotface = match o.take_opt("spotface") {
            None => None,
            Some(x) => {
                let p = format!("{at}/spotface");
                let mut m = Obj::new(&x, &p)?;
                let d = length(&m.take("diameter")?, &format!("{p}/diameter"))?;
                m.finish()?;
                Some(d)
            }
        };
        o.finish()?;
        Ok(FastenerEntry {
            clearance,
            counterbore,
            countersink,
            spotface,
        })
    }

    fn to_json(&self) -> Value {
        let mut v = Map::new();
        if !self.clearance.is_empty() {
            let c: Map<String, Value> = self
                .clearance
                .iter()
                .map(|(k, q)| (k.clone(), json!(q.text())))
                .collect();
            v.insert("clearance".into(), Value::Object(c));
        }
        if let Some((d, h)) = &self.counterbore {
            v.insert(
                "counterbore".into(),
                json!({"diameter": d.text(), "depth": h.text()}),
            );
        }
        if let Some((d, a)) = &self.countersink {
            v.insert(
                "countersink".into(),
                json!({"diameter": d.text(), "angle": a.text()}),
            );
        }
        if let Some(d) = &self.spotface {
            v.insert("spotface".into(), json!({"diameter": d.text()}));
        }
        Value::Object(v)
    }
}

/// `shadows_builtin` at `at`, for a built-in standard's size.
fn shadows(standard: &str, size: &str, what: &str, at: &str) -> Error {
    Error::new(
        Code::InvalidDocument,
        format!(
            "{standard} {size}'s {what} is built in: ryucad derives it, so a project table may \
             not restate it; take it out"
        ),
    )
    .with("reason", "shadows_builtin")
    .with("pointer", at)
}

/// Refuses an entry under a built-in standard that restates what it
/// derives: a built-in size's pitch or TPI, its basic diameters, or the
/// limits of a class it has a formula for.
fn check_gap(standard: Standard, size: &str, e: &ThreadEntry, at: &str) -> Result<()> {
    let key = standard.key();
    let Ok(t) = lookup(standard, size, None, None) else {
        // A size the built-in lacks: the project defines it.
        if e.pitch.is_none() && e.tpi.is_none() {
            return Err(invalid(
                at,
                "a size ryucad doesn't build in needs its pitch or tpi",
            ));
        }
        return Ok(());
    };
    for (k, given) in [("pitch", e.pitch.is_some()), ("tpi", e.tpi.is_some())] {
        if given {
            return Err(shadows(key, size, k, &format!("{at}/{k}")));
        }
    }
    for (k, d) in [("major", &e.major), ("minor", &e.minor)] {
        let p = format!("{at}/{k}");
        match d {
            Some(Diameters::Basic(_)) if t.major.is_some() => {
                return Err(shadows(key, size, k, &p));
            }
            Some(Diameters::Classes(m)) => {
                for class in m.keys() {
                    if lookup(standard, size, None, Some(class)).is_ok() {
                        return Err(shadows(key, size, &format!("{k} in {class}"), &p));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

impl HoleStandards {
    pub fn from_json(v: &Value) -> Result<HoleStandards> {
        let mut o = header(v, KIND, FORMAT)?;
        let mut out = HoleStandards::default();
        if let Some(t) = o.take_opt("threads") {
            let t = t
                .as_object()
                .ok_or_else(|| invalid("/threads", "threads is by standard"))?;
            for (std, sizes) in t {
                let at = format!("/threads/{}", esc(std));
                check_standard(std, &at)?;
                let sizes = sizes
                    .as_object()
                    .ok_or_else(|| invalid(&at, "a standard is by size"))?;
                let mut m = BTreeMap::new();
                for (size, e) in sizes {
                    let p = format!("{at}/{}", esc(size));
                    let entry = ThreadEntry::from_json(e, &p)?;
                    match Standard::parse(std) {
                        Some(b) => check_gap(b, size, &entry, &p)?,
                        None if entry.pitch.is_none() && entry.tpi.is_none() => {
                            return Err(invalid(&p, "a thread size needs its pitch or tpi"));
                        }
                        None => {}
                    }
                    m.insert(size.clone(), entry);
                }
                out.threads.insert(std.clone(), m);
            }
        }
        if let Some(f) = o.take_opt("fasteners") {
            let f = f
                .as_object()
                .ok_or_else(|| invalid("/fasteners", "fasteners is by standard"))?;
            for (std, sizes) in f {
                let at = format!("/fasteners/{}", esc(std));
                check_standard(std, &at)?;
                let sizes = sizes
                    .as_object()
                    .ok_or_else(|| invalid(&at, "a standard is by size"))?;
                let mut m = BTreeMap::new();
                for (size, e) in sizes {
                    let p = format!("{at}/{}", esc(size));
                    m.insert(size.clone(), FastenerEntry::from_json(e, &p)?);
                }
                out.fasteners.insert(std.clone(), m);
            }
        }
        o.finish()?;
        Ok(out)
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"format": FORMAT, "kind": KIND});
        if !self.threads.is_empty() {
            v["threads"] = json!(
                self.threads
                    .iter()
                    .map(|(s, m)| {
                        (
                            s.clone(),
                            Value::Object(
                                m.iter().map(|(k, e)| (k.clone(), e.to_json())).collect(),
                            ),
                        )
                    })
                    .collect::<Map<String, Value>>()
            );
        }
        if !self.fasteners.is_empty() {
            v["fasteners"] = json!(
                self.fasteners
                    .iter()
                    .map(|(s, m)| {
                        (
                            s.clone(),
                            Value::Object(
                                m.iter().map(|(k, e)| (k.clone(), e.to_json())).collect(),
                            ),
                        )
                    })
                    .collect::<Map<String, Value>>()
            );
        }
        v
    }

    pub fn thread(&self, standard: &str, size: &str) -> Option<&ThreadEntry> {
        self.threads.get(standard)?.get(size)
    }

    pub fn fastener(&self, standard: &str, size: &str) -> Option<&FastenerEntry> {
        self.fasteners.get(standard)?.get(size)
    }

    /// One entry as its file holds it, for hashing and `standards`.
    pub fn entry_json(&self, kind: Kind, standard: &str, size: &str) -> Option<Value> {
        match kind {
            Kind::Thread => self.thread(standard, size).map(ThreadEntry::to_json),
            Kind::Fastener => self.fastener(standard, size).map(FastenerEntry::to_json),
        }
    }
}

/// The entries a part's holes read, in a fixed order: each thread's and
/// fastener's (table, standard, size).
pub fn reads<'a>(
    features: impl IntoIterator<Item = &'a super::files::Feature>,
) -> std::collections::BTreeSet<(Kind, String, String)> {
    let mut out = std::collections::BTreeSet::new();
    for f in features {
        let super::files::Feature::Hole(h) = f else {
            continue;
        };
        if let Some(t) = &h.thread {
            out.insert((Kind::Thread, t.standard.clone(), t.size.clone()));
        }
        if let Some(x) = &h.fastener {
            out.insert((Kind::Fastener, x.standard.clone(), x.size.clone()));
        }
    }
    out
}

/// The project's table as read: its bytes and what they hold, or `None`
/// when it has none. Bytes that don't read are kept with the error, so a
/// part reading the table while it's broken hashes them.
pub fn read(src: &dyn Source) -> Result<Option<(Vec<u8>, Result<HoleStandards>)>> {
    let Some(bytes) = src
        .read(HOLE_STANDARDS)
        .map_err(|e| e.with("path", HOLE_STANDARDS))?
    else {
        return Ok(None);
    };
    let bytes = bytes.to_vec();
    let parsed = (|| {
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Error::new(Code::InvalidJson, format!("{HOLE_STANDARDS} is not UTF-8")))?;
        let v = canonical::parse(text)?;
        if let Some(e) = conflicted(&v, HOLE_STANDARDS) {
            return Err(e);
        }
        HoleStandards::from_json(&v)
    })()
    .map_err(|e| e.with("path", HOLE_STANDARDS));
    Ok(Some((bytes, parsed)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn house() -> Value {
        crate::doc::fixtures::house_standards()
    }

    #[test]
    fn tables_read_and_write_back_canonically() {
        let t = HoleStandards::from_json(&house()).unwrap();
        assert_eq!(t.to_json(), house());
        let cap = t.fastener("house_cap", "M5").unwrap();
        assert_eq!(cap.clearance["normal"].meters(), 0.0056);
        assert_eq!(cap.counterbore.as_ref().unwrap().1.text(), "5.3 mm");
        assert!(t.thread("iso_metric", "M6").unwrap().tap_drill.is_some());
        assert_eq!(
            t.entry_json(Kind::Thread, "npt", "1/8").unwrap(),
            json!({"tap_drill": "8.7 mm"})
        );
        assert!(t.entry_json(Kind::Fastener, "house_cap", "M6").is_none());
        assert_eq!(
            HoleStandards::default().to_json(),
            json!({"format": 1, "kind": KIND})
        );
    }

    #[test]
    fn a_built_in_value_is_never_restated() {
        for (standard, size, key, value) in [
            ("iso_metric", "M6", "pitch", json!("1 mm")),
            ("iso_metric", "M6", "major", json!("6 mm")),
            (
                "iso_metric",
                "M6",
                "minor",
                json!({"6H": ["4.917 mm", "5.153 mm"]}),
            ),
            (
                "unc",
                "1/4",
                "minor",
                json!({"2B": ["0.196 in", "0.207 in"]}),
            ),
            ("npt", "1/8", "tpi", json!(27)),
        ] {
            let mut v = house();
            v["threads"][standard][size][key] = value;
            let e = HoleStandards::from_json(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["reason"].as_str()),
                (Code::InvalidDocument, Some("shadows_builtin")),
                "{standard} {size} {key}: {e}"
            );
        }
        // What a built-in size lacks is the project's to add: a house tap
        // drill, a roll-form drill, a pipe thread's sizes.
        assert!(HoleStandards::from_json(&house()).is_ok());
    }

    #[test]
    fn bad_tables_say_where() {
        type Edit<'a> = &'a dyn Fn(&mut Value);
        let cases: [(Edit, &str); 4] = [
            (
                &|v| v["threads"]["house_acme"]["T10"]["pitch"] = json!("2"),
                "/threads/house_acme/T10/pitch",
            ),
            (
                &|v| v["threads"]["house_acme"]["T10"]["tpi"] = json!(12),
                "/threads/house_acme/T10",
            ),
            (
                &|v| v["fasteners"]["house_cap"]["M5"]["clearance"] = json!({"snug": "5 mm"}),
                "/fasteners/house_cap/M5/clearance/snug",
            ),
            (
                &|v| v["fasteners"]["House"] = json!({"M5": {}}),
                "/fasteners/House",
            ),
        ];
        for (edit, pointer) in cases {
            let mut v = house();
            edit(&mut v);
            let e = HoleStandards::from_json(&v).unwrap_err();
            assert_eq!(e.details["pointer"], pointer, "{pointer}: {e}");
        }
        let mut v = house();
        v["format"] = json!(2);
        assert_eq!(
            HoleStandards::from_json(&v).unwrap_err().code,
            Code::FormatTooNew
        );
    }
}
