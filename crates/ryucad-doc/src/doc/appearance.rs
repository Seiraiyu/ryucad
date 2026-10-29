//! Appearance: `parts/<part>/appearance.json`, the colours and opacities a
//! part's solids and faces are drawn in, or an assembly's overrides for
//! the occurrences it places. Nothing a rebuild makes reads it, and it
//! never counts in a release's content hash: it changes how a model looks,
//! never what it is.
//!
//! ```json
//! {
//!   "faces": {"box/0123456789abcdef0123456789abcdef": {"colour": "#d03030"}},
//!   "format": 1,
//!   "kind": "ryucad.appearance",
//!   "part": {"material": true},
//!   "solids": {"lid": {"opacity": 0.4}}
//! }
//! ```
//!
//! A part's file holds `part` (its default), `solids` by founder and
//! `faces` by `<founder>/<name>`; an assembly's holds `occurrences`, by
//! instance path (`<path>`, every occurrence under it) or by an
//! occurrence's solid (`<path>/<solid>`). Each entry sets any of
//! `colour` (`#rrggbb`, lower case), `opacity` (in `(0, 1]`) and
//! `material: true` (the part's material's colour), never `colour` and
//! `material` together, and at least one of them.

use super::files::{Obj, header, invalid, number};
use super::id::{Id, parse_step};
use super::materials::check_colour;
use crate::error::Result;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const KIND: &str = "ryucad.appearance";

/// The newest format this build reads.
pub const FORMAT: u64 = 1;

/// The file, inside its part's folder.
pub const APPEARANCE: &str = "appearance.json";

/// What one entry sets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    /// `#rrggbb`, lower case.
    pub colour: Option<String>,
    /// In `(0, 1]`.
    pub opacity: Option<f64>,
    /// The part's material's colour, in place of a colour of its own.
    pub material: bool,
}

impl Look {
    /// Whether it sets nothing (and so has no place in the file).
    pub fn is_empty(&self) -> bool {
        self.colour.is_none() && self.opacity.is_none() && !self.material
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({});
        if let Some(c) = &self.colour {
            v["colour"] = json!(c);
        }
        if self.material {
            v["material"] = json!(true);
        }
        if let Some(o) = self.opacity {
            v["opacity"] = number(o);
        }
        v
    }

    /// Reads an entry at JSON pointer `at`.
    pub fn from_json(v: &Value, at: &str) -> Result<Look> {
        let mut o = Obj::new(v, at)?;
        let colour = match o.take_opt("colour") {
            None => None,
            Some(Value::String(c)) if check_colour(&c).as_deref() == Ok(c.as_str()) => Some(c),
            Some(_) => {
                return Err(invalid(
                    &format!("{at}/colour"),
                    "expected a colour #rrggbb in lower case",
                ));
            }
        };
        let opacity = match o.take_opt("opacity") {
            None => None,
            Some(x) => Some(check_opacity(x.as_f64()).ok_or_else(|| {
                invalid(
                    &format!("{at}/opacity"),
                    "expected an opacity above 0 and at most 1",
                )
            })?),
        };
        let material = match o.take_opt("material") {
            None => false,
            Some(Value::Bool(true)) => true,
            Some(_) => {
                return Err(invalid(
                    &format!("{at}/material"),
                    "material is true or left out",
                ));
            }
        };
        o.finish()?;
        let look = Look {
            colour,
            opacity,
            material,
        };
        if look.material && look.colour.is_some() {
            return Err(invalid(
                at,
                "an entry takes a colour or its material's colour, not both",
            ));
        }
        if look.is_empty() {
            return Err(invalid(
                at,
                "an entry sets a colour, an opacity or material",
            ));
        }
        Ok(look)
    }
}

/// An opacity as files hold it: above 0, at most 1.
pub fn check_opacity(x: Option<f64>) -> Option<f64> {
    x.filter(|x| *x > 0.0 && *x <= 1.0)
}

/// A part's or an assembly's appearance file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppearanceFile {
    /// The part's default (a part only).
    pub part: Option<Look>,
    /// By founder (a part only).
    pub solids: BTreeMap<String, Look>,
    /// By `<founder>/<name>` (a part only).
    pub faces: BTreeMap<String, Look>,
    /// By `<path>` or `<path>/<solid>` (an assembly only).
    pub occurrences: BTreeMap<String, Look>,
}

/// A face's key: `<founder>/<name>`.
pub fn face_key(founder: &str, name: &str) -> String {
    format!("{founder}/{name}")
}

/// Escapes a key for a JSON pointer.
fn escape(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

/// Whether `s` is a stable name as files write it: 32 lowercase hex
/// digits.
fn is_name(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn check_solid(k: &str, at: &str) -> Result<()> {
    Id::parse(k, "solid")
        .map(drop)
        .map_err(|e| invalid(at, &e.message))
}

fn check_face(k: &str, at: &str) -> Result<()> {
    match k.split_once('/') {
        Some((solid, name)) if is_name(name) => check_solid(solid, at),
        _ => Err(invalid(
            at,
            "a face is <founder>/<name>, its name 32 lowercase hex digits",
        )),
    }
}

fn check_occurrence(k: &str, at: &str) -> Result<()> {
    if k.is_empty() {
        return Err(invalid(at, "an occurrence is an instance path"));
    }
    for step in k.split('/') {
        parse_step(step, at).map_err(|e| invalid(at, &e.message))?;
    }
    Ok(())
}

impl AppearanceFile {
    /// Whether it holds no entry (and so has no file).
    pub fn is_empty(&self) -> bool {
        self.part.is_none()
            && self.solids.is_empty()
            && self.faces.is_empty()
            && self.occurrences.is_empty()
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"format": FORMAT, "kind": KIND});
        if let Some(p) = &self.part {
            v["part"] = p.to_json();
        }
        let map = |m: &BTreeMap<String, Look>| -> Value {
            Value::Object(
                m.iter()
                    .map(|(k, l)| (k.clone(), l.to_json()))
                    .collect::<Map<String, Value>>(),
            )
        };
        for (key, m) in [
            ("solids", &self.solids),
            ("faces", &self.faces),
            ("occurrences", &self.occurrences),
        ] {
            if !m.is_empty() {
                v[key] = map(m);
            }
        }
        v
    }

    /// Reads a file: entries that read, under keys of their kind.
    pub fn from_json(v: &Value) -> Result<AppearanceFile> {
        let mut o = header(v, KIND, FORMAT)?;
        let part = match o.take_opt("part") {
            None => None,
            Some(p) => Some(Look::from_json(&p, "/part")?),
        };
        let mut out = AppearanceFile {
            part,
            ..Default::default()
        };
        type Check = fn(&str, &str) -> Result<()>;
        for (key, check) in [
            ("solids", check_solid as Check),
            ("faces", check_face as Check),
            ("occurrences", check_occurrence as Check),
        ] {
            let Some(m) = o.take_opt(key) else { continue };
            let m = m
                .as_object()
                .ok_or_else(|| invalid(&format!("/{key}"), "expected an object"))?;
            let mut read = BTreeMap::new();
            for (k, l) in m {
                let at = format!("/{key}/{}", escape(k));
                check(k, &at)?;
                read.insert(k.clone(), Look::from_json(l, &at)?);
            }
            match key {
                "solids" => out.solids = read,
                "faces" => out.faces = read,
                _ => out.occurrences = read,
            }
        }
        o.finish()?;
        Ok(out)
    }

    /// Checks that it holds only what its part's kind takes: a part's
    /// own entries, or an assembly's occurrences.
    pub fn check_kind(&self, assembly: bool) -> Result<()> {
        if assembly {
            for (key, has) in [
                ("part", self.part.is_some()),
                ("solids", !self.solids.is_empty()),
                ("faces", !self.faces.is_empty()),
            ] {
                if has {
                    return Err(invalid(
                        &format!("/{key}"),
                        "an assembly's appearance colours its occurrences: colour a part in its own file",
                    ));
                }
            }
        } else if !self.occurrences.is_empty() {
            return Err(invalid(
                "/occurrences",
                "a part has no occurrences: an assembly colours what it places",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    const NAME: &str = "0123456789abcdef0123456789abcdef";

    fn file() -> Value {
        json!({"faces": {format!("box/{NAME}"): {"colour": "#d03030"}},
               "format": 1, "kind": KIND,
               "part": {"material": true},
               "solids": {"lid": {"colour": "#2050c0", "opacity": 0.4}}})
    }

    #[test]
    fn a_file_reads_and_writes_back() {
        let a = AppearanceFile::from_json(&file()).unwrap();
        assert!(a.part.as_ref().unwrap().material);
        assert_eq!(a.solids["lid"].opacity, Some(0.4));
        assert_eq!(
            a.faces[&face_key("box", NAME)].colour.as_deref(),
            Some("#d03030")
        );
        assert_eq!(a.to_json(), file());
        a.check_kind(false).unwrap();
        assert_eq!(a.check_kind(true).unwrap_err().code, Code::InvalidDocument);
        let asm = json!({"format": 1, "kind": KIND,
                         "occurrences": {"frame/pa:2/pin": {"opacity": 1},
                                         "frame/bolt/b": {"colour": "#000000"}}});
        let read = AppearanceFile::from_json(&asm).unwrap();
        read.check_kind(true).unwrap();
        assert_eq!(
            read.check_kind(false).unwrap_err().code,
            Code::InvalidDocument
        );
        assert_eq!(read.to_json(), asm);
        assert!(AppearanceFile::default().is_empty());
        assert_eq!(
            AppearanceFile::default().to_json(),
            json!({"format": 1, "kind": KIND})
        );
    }

    #[test]
    fn bad_files_say_where() {
        let at = |f: &dyn Fn(&mut Value)| {
            let mut v = file();
            f(&mut v);
            let e = AppearanceFile::from_json(&v).unwrap_err();
            (
                e.code,
                e.details["pointer"].as_str().unwrap_or("").to_owned(),
            )
        };
        let bad = |p: &str| (Code::InvalidDocument, p.to_owned());
        let face = format!("/faces/box~1{NAME}");
        assert_eq!(
            at(&|v| v["part"] = json!({"colour": "#D03030"})),
            bad("/part/colour")
        );
        assert_eq!(
            at(&|v| v["part"] = json!({"colour": "red"})),
            bad("/part/colour")
        );
        assert_eq!(
            at(&|v| v["solids"]["lid"]["opacity"] = json!(0)),
            bad("/solids/lid/opacity")
        );
        assert_eq!(
            at(&|v| v["solids"]["lid"]["opacity"] = json!(1.5)),
            bad("/solids/lid/opacity")
        );
        assert_eq!(
            at(&|v| v["part"] = json!({"material": false})),
            bad("/part/material")
        );
        assert_eq!(
            at(&|v| v["part"] = json!({"material": true, "colour": "#000000"})),
            bad("/part")
        );
        assert_eq!(at(&|v| v["part"] = json!({})), bad("/part"));
        assert_eq!(at(&|v| v["part"] = json!({"shine": 1})), bad("/part/shine"));
        assert_eq!(
            at(&|v| v["solids"] = json!({"Lid": {"opacity": 1}})),
            bad("/solids/Lid")
        );
        assert_eq!(
            at(&|v| v["faces"] = json!({"box/ABC": {"opacity": 1}})),
            bad("/faces/box~1ABC")
        );
        assert_eq!(
            at(&|v| v["faces"][format!("box/{NAME}")] = json!({"colour": 1})),
            bad(&format!("{face}/colour"))
        );
        assert_eq!(
            at(&|v| v["occurrences"] = json!({"a/p:0/b": {"opacity": 1}})),
            bad("/occurrences/a~1p:0~1b")
        );
        assert_eq!(at(&|v| v["edges"] = json!({})), bad("/edges"));
        let mut newer = file();
        newer["format"] = json!(2);
        assert_eq!(
            AppearanceFile::from_json(&newer).unwrap_err().code,
            Code::FormatTooNew
        );
    }
}
