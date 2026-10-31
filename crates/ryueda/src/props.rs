//! Properties, as Altium has them: named text values on the project, on
//! a block (defaults for every placement of it) and on an instance (that
//! placement's own). Text on a sheet and a reference designator format
//! read them with `{name}` placeholders, walking out from the innermost
//! occurrence to the project; a few are built in and read only.
//!
//! A reference designator format is text with placeholders, each a
//! property or an integer expression in `ryu-doc`'s syntax over integer
//! properties and the designator's own `number`:
//! `"{prefix}{number}_{stack_letter}"` gives `R5_A`,
//! `"{prefix}{number + stack_index * 100}{rest}"` gives `R105`. `{{` and
//! `}}` are braces.

use crate::error::{Code, Error, Result};
use crate::expr::{Expr, Val};
use crate::units::Dim;
use std::collections::{BTreeMap, BTreeSet};

/// Properties as files hold them: name to text.
pub type Properties = BTreeMap<String, String>;

/// Names the files may not set: what every level knows of itself, and
/// what a designator format reads of the designator it is given.
pub const BUILT_IN: [&str; 13] = [
    "design",
    "block",
    "sheet",
    "sheets",
    "refdes",
    "value",
    "prefix",
    "number",
    "rest",
    "stack_index",
    "stack_letter",
    "stack_count",
    "stack_path",
];

/// Named formats for the usual schemes, so an agent rarely writes one.
pub const PRESETS: [(&str, &str); 4] = [
    ("suffix_letter", "{refdes}_{stack_letter}"),
    ("suffix_index", "{refdes}_{stack_index}"),
    (
        "offset_hundreds",
        "{prefix}{number + stack_index * 100}{rest}",
    ),
    ("prefix_block", "{block}_{refdes}"),
];

/// Whether `name` may name a property: `[a-z_][a-z0-9_]{0,62}`.
pub fn name_ok(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// What a level's properties find: names that can't be property names,
/// and built-in names, each with its JSON pointer under `at`.
pub fn faults(props: &Properties, at: &str) -> Vec<crate::fault::Fault> {
    let mut out = Vec::new();
    for name in props.keys() {
        let ptr = format!("{at}/{}", crate::symbol::pointer_escape(name));
        let why = if !name_ok(name) {
            Some(format!(
                "{name:?} can't name a property: lowercase letters, digits and _, at most 63"
            ))
        } else if BUILT_IN.contains(&name.as_str()) {
            Some(format!("{name} is built in, and read only"))
        } else {
            None
        };
        if let Some(why) = why {
            out.push(crate::fault::Fault::error(Code::InvalidDocument, why).with("pointer", ptr));
        }
    }
    out
}

/// Applies a request's changes to a level's properties: a text sets one,
/// `null` removes it. Names that can't be set are `invalid_param`.
pub fn apply(
    into: &mut Properties,
    changes: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    for (name, v) in changes {
        let param = format!("properties/{name}");
        if !name_ok(name) || BUILT_IN.contains(&name.as_str()) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{name:?} can't be set: a property is named [a-z_][a-z0-9_]{{0,62}}, and {} are built in", BUILT_IN.join(", ")),
            )
            .with("param", param));
        }
        match v {
            serde_json::Value::Null => {
                into.remove(name);
            }
            serde_json::Value::String(t) => {
                into.insert(name.clone(), t.clone());
            }
            v => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("property {name} is text, or null to remove it, not {v}"),
                )
                .with("param", param));
            }
        }
    }
    Ok(())
}

/// The format a preset names, or the text itself.
pub fn preset(text: &str) -> &str {
    PRESETS
        .iter()
        .find(|(name, _)| *name == text)
        .map_or(text, |(_, f)| f)
}

/// A piece of a format.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String),
    /// A property, as text.
    Name(String),
    /// An integer expression.
    Expr(Expr),
}

/// Text with `{…}` placeholders, parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    text: String,
    pieces: Vec<Piece>,
}

fn invalid(text: &str, at: usize, why: &str) -> Error {
    Error::new(
        Code::RefdesFormatInvalid,
        format!("{text:?}: {why} at byte {at}"),
    )
    .with("format", text)
    .with("position", at)
}

impl Template {
    /// Parses `text`; `refdes_format_invalid` names the byte that doesn't.
    pub fn parse(text: &str) -> Result<Template> {
        let mut pieces = Vec::new();
        let mut lit = String::new();
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'{' if b.get(i + 1) == Some(&b'{') => {
                    lit.push('{');
                    i += 2;
                }
                b'}' if b.get(i + 1) == Some(&b'}') => {
                    lit.push('}');
                    i += 2;
                }
                b'}' => return Err(invalid(text, i, "a } with no {")),
                b'{' => {
                    let Some(len) = text[i + 1..].find('}') else {
                        return Err(invalid(text, i, "a { with no }"));
                    };
                    let inner = text[i + 1..i + 1 + len].trim();
                    if inner.is_empty() {
                        return Err(invalid(text, i, "an empty placeholder"));
                    }
                    if inner.contains('{') {
                        return Err(invalid(text, i, "a placeholder inside a placeholder"));
                    }
                    if !lit.is_empty() {
                        pieces.push(Piece::Text(std::mem::take(&mut lit)));
                    }
                    if name_ok(inner) {
                        pieces.push(Piece::Name(inner.to_owned()));
                    } else {
                        let e = Expr::parse(inner).map_err(|e| {
                            invalid(
                                text,
                                i,
                                &format!("{{{inner}}} isn't an expression: {}", e.message),
                            )
                        })?;
                        pieces.push(Piece::Expr(e));
                    }
                    i += len + 2;
                }
                _ => {
                    let c = text[i..].chars().next().expect("in the text");
                    lit.push(c);
                    i += c.len_utf8();
                }
            }
        }
        if !lit.is_empty() {
            pieces.push(Piece::Text(lit));
        }
        Ok(Template {
            text: text.to_owned(),
            pieces,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Every property name it reads.
    pub fn names(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for p in &self.pieces {
            match p {
                Piece::Name(n) => {
                    out.insert(n.clone());
                }
                Piece::Expr(e) => out.extend(e.names()),
                Piece::Text(_) => {}
            }
        }
        out
    }

    /// The text with each placeholder filled from `lookup`: a name it
    /// doesn't know is `property_unknown`, arithmetic on text that isn't
    /// an integer `refdes_format_invalid`.
    pub fn render(&self, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String> {
        let mut out = String::new();
        for p in &self.pieces {
            match p {
                Piece::Text(t) => out.push_str(t),
                Piece::Name(n) => out.push_str(&lookup(n).ok_or_else(|| unknown(n))?),
                Piece::Expr(e) => {
                    let v = e.eval(&|name: &str| {
                        let text = lookup(name).ok_or_else(|| unknown(name))?;
                        let n: i64 = text.trim().parse().map_err(|_| {
                            Error::new(
                                Code::RefdesFormatInvalid,
                                format!("{name} is {text:?}, not an integer, so it can't be counted with"),
                            )
                            .with("property", name)
                        })?;
                        Ok(Val::new(n as f64, Dim::NUMBER))
                    })?;
                    let whole = v.x.round();
                    if v.dim != Dim::NUMBER || whole != v.x || whole.abs() >= 9.0e15 {
                        return Err(Error::new(
                            Code::RefdesFormatInvalid,
                            format!("{} doesn't give a whole number", e.text()),
                        )
                        .with("format", &self.text));
                    }
                    out.push_str(&format!("{}", whole as i64));
                }
            }
        }
        Ok(out)
    }
}

fn unknown(name: &str) -> Error {
    Error::new(
        Code::PropertyUnknown,
        format!("no level sets a property {name}: the instance, its block, the blocks above or the project"),
    )
    .with("property", name)
}

/// `A`, `B`, … `Z`, `AA`: a layer's letter from its index (from 1).
pub fn letter(index: u64) -> String {
    let mut n = index;
    let mut out = Vec::new();
    while n > 0 {
        let d = (n - 1) % 26;
        out.push(b'A' + d as u8);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII")
}

/// What a level knows: its properties, in the order they are read.
pub struct Levels<'a> {
    /// Innermost first.
    pub levels: Vec<&'a Properties>,
    /// Read before any level: the built-ins at this place.
    pub built_in: BTreeMap<&'static str, String>,
}

impl Levels<'_> {
    pub fn get(&self, name: &str) -> Option<String> {
        if let Some(v) = self.built_in.get(name) {
            return Some(v.clone());
        }
        self.levels.iter().find_map(|l| l.get(name).cloned())
    }
}

/// The built-ins a designator format reads of the designator it is
/// given: `refdes`, `prefix`, `number` and `rest`.
pub fn designator(refdes: &str, into: &mut BTreeMap<&'static str, String>) {
    let (prefix, number, rest) = crate::conn::refdes_parts(refdes);
    into.insert("refdes", refdes.to_owned());
    into.insert("prefix", prefix.to_owned());
    into.insert("number", number.to_string());
    into.insert("rest", rest.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(list: &[(&str, &str)]) -> BTreeMap<String, String> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn render(format: &str, list: &[(&str, &str)]) -> Result<String> {
        let p = props(list);
        Template::parse(format)?.render(&|n| p.get(n).cloned())
    }

    #[test]
    fn formats_fill_names_and_integer_expressions() {
        let r5 = [
            ("refdes", "R5"),
            ("prefix", "R"),
            ("number", "5"),
            ("rest", ""),
            ("stack_index", "2"),
            ("stack_letter", "B"),
        ];
        assert_eq!(
            render("{prefix}{number}_{stack_letter}", &r5).unwrap(),
            "R5_B"
        );
        assert_eq!(
            render("{prefix}{number + stack_index * 100}{rest}", &r5).unwrap(),
            "R205"
        );
        assert_eq!(render(preset("suffix_index"), &r5).unwrap(), "R5_2");
        assert_eq!(render("{{x}} {refdes}}}", &r5).unwrap(), "{x} R5}");
        assert_eq!(
            Template::parse("{prefix}{number + offset}")
                .unwrap()
                .names(),
            ["number", "offset", "prefix"].map(String::from).into()
        );
    }

    #[test]
    fn bad_formats_name_the_byte_and_the_property() {
        for (f, at) in [
            ("R{", 1),
            ("R}", 1),
            ("{}", 0),
            ("{number +}", 0),
            ("a{b{c}}", 1),
        ] {
            let e = Template::parse(f).unwrap_err();
            assert_eq!(
                (e.code, &e.details["position"]),
                (Code::RefdesFormatInvalid, &serde_json::json!(at)),
                "{f}"
            );
        }
        let e = render("{nope}", &[]).unwrap_err();
        assert_eq!(e.code, Code::PropertyUnknown);
        let e = render("{number + 1}", &[("number", "five")]).unwrap_err();
        assert_eq!(e.code, Code::RefdesFormatInvalid);
        let e = render("{number / 2}", &[("number", "5")]).unwrap_err();
        assert_eq!(e.code, Code::RefdesFormatInvalid);
    }

    #[test]
    fn names_and_letters() {
        assert!(name_ok("channel") && name_ok("_x1") && !name_ok("1x") && !name_ok("Ch"));
        assert_eq!(
            faults(
                &props(&[("Bad", "x"), ("refdes", "y"), ("ok", "z")]),
                "/properties"
            )
            .len(),
            2
        );
        assert_eq!(
            [1, 2, 26, 27, 52, 703].map(letter),
            ["A", "B", "Z", "AA", "AZ", "AAA"]
        );
        assert_eq!(preset("R{number}"), "R{number}");
    }
}
