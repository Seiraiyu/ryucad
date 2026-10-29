//! Attributes: what each is by its declaration, and values read from
//! lookup tables (`@n` names, `&n` texts, `;0,3=2` on a record) and from
//! attribute lists (`attrlist` files, `.name = value`).
//!
//! A value means what its declaration says: a boolean is set by its index
//! alone, a text names an `&n` string, an option is an index into its
//! options, an integer or float is the number written. Declarations are
//! the specification's system attributes, then the job's own in
//! `misc/sysattr*` and `misc/userattr`, which win. An attribute nobody
//! declares, or a value its declaration doesn't allow, is left out and
//! reported (`odb_ignored`): the reader never guesses what it meant.

use std::collections::BTreeMap;

use ryu_board::geom::decimal;
use ryu_board::model::{AttrValue, Attributes};

use super::sysattr::SYSTEM;
use super::text::{Bad, structured};

/// A system attribute's kind, from the specification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Boolean,
    Text,
    Integer,
    Float,
    Option(&'static [&'static str]),
}

/// An attribute's declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decl {
    Boolean,
    Text,
    Integer,
    Float,
    /// The options, in index order.
    Option(Vec<String>),
}

impl From<Kind> for Decl {
    fn from(k: Kind) -> Decl {
        match k {
            Kind::Boolean => Decl::Boolean,
            Kind::Text => Decl::Text,
            Kind::Integer => Decl::Integer,
            Kind::Float => Decl::Float,
            Kind::Option(o) => Decl::Option(o.iter().map(|s| s.to_string()).collect()),
        }
    }
}

/// The attribute declarations a job's files are read with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decls {
    own: BTreeMap<String, Decl>,
}

impl Decls {
    /// Adds the declarations of a `misc/sysattr*` or `misc/userattr` file.
    pub fn read(&mut self, text: &str) -> Result<(), Bad> {
        let s = structured(text)?;
        for a in &s.arrays {
            let Some(name) = a.get("NAME") else {
                return Err(Bad::new(a.line, 1, format!("{} without NAME", a.name)));
            };
            let decl = match a.name.to_ascii_uppercase().as_str() {
                "BOOLEAN" => Decl::Boolean,
                "TEXT" => Decl::Text,
                "INTEGER" => Decl::Integer,
                "FLOAT" => Decl::Float,
                "OPTION" => Decl::Option(
                    a.get("OPTIONS")
                        .unwrap_or("")
                        .split(';')
                        .map(|o| o.trim().to_string())
                        .collect(),
                ),
                other => {
                    return Err(Bad::new(
                        a.line,
                        1,
                        format!("{other}: not an attribute kind"),
                    ));
                }
            };
            self.own.insert(name.to_ascii_lowercase(), decl);
        }
        Ok(())
    }

    /// The declaration of `name`, the job's own before the system's.
    pub fn get(&self, name: &str) -> Option<Decl> {
        let name = name.to_ascii_lowercase();
        if let Some(d) = self.own.get(&name) {
            return Some(d.clone());
        }
        SYSTEM
            .binary_search_by(|(n, _)| (*n).cmp(name.as_str()))
            .ok()
            .map(|i| SYSTEM[i].1.into())
    }
}

/// Why an attribute was left out: its name (or `@n` where the table has
/// no such index) and the reason.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ignored {
    pub name: String,
    pub why: String,
}

fn is_number(s: &str, integer: bool) -> bool {
    if integer {
        let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
    } else {
        decimal::scaled(s, 1, i64::MAX).is_ok()
    }
}

/// A file's attribute lookup table.
#[derive(Clone, Debug, Default)]
pub struct Table {
    names: Vec<Option<(String, Option<Decl>)>>,
    texts: Vec<Option<String>>,
}

/// `@3 .name` or `&2 text` without its sign: the index and the rest.
fn entry(rest: &str, line: u32) -> Result<(usize, &str), Bad> {
    let (n, v) = rest.split_once([' ', '\t']).unwrap_or((rest, ""));
    let i: usize = n
        .parse()
        .map_err(|_| Bad::new(line, 2, format!("{n}: not an index")))?;
    if i > 1_000_000 {
        return Err(Bad::new(line, 2, format!("{i}: an index past a million")));
    }
    Ok((i, v.trim()))
}

fn set<T>(v: &mut Vec<Option<T>>, i: usize, x: T) {
    if v.len() <= i {
        v.resize_with(i + 1, || None);
    }
    v[i] = Some(x);
}

impl Table {
    /// An `@n name` line, without its `@`.
    pub fn name(&mut self, rest: &str, line: u32, decls: &Decls) -> Result<(), Bad> {
        let (i, name) = entry(rest, line)?;
        set(&mut self.names, i, (name.to_string(), decls.get(name)));
        Ok(())
    }

    /// An `&n text` line, without its `&`.
    pub fn text(&mut self, rest: &str, line: u32) -> Result<(), Bad> {
        let (i, text) = entry(rest, line)?;
        set(&mut self.texts, i, text.to_string());
        Ok(())
    }

    /// The attributes a record assigns (`0,3=2,5=1.5`), and what was left
    /// out. A `#` before an index is allowed, as old net records write it.
    pub fn values(&self, list: &str) -> (Attributes, Vec<Ignored>) {
        let mut out = Attributes::new();
        let mut ignored = Vec::new();
        for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let item = item.strip_prefix('#').unwrap_or(item);
            let (n, v) = match item.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (item, None),
            };
            let entry = n
                .parse::<usize>()
                .ok()
                .and_then(|i| self.names.get(i)?.as_ref());
            let Some((name, decl)) = entry else {
                ignored.push(Ignored {
                    name: format!("@{n}"),
                    why: "not in the attribute table".into(),
                });
                continue;
            };
            let value = match (decl, v) {
                (None, _) => Err("undeclared"),
                (Some(Decl::Boolean), None) => Ok(AttrValue::Flag),
                (Some(Decl::Text), Some(v)) => v
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| self.texts.get(i)?.clone())
                    .map(AttrValue::Text)
                    .ok_or("a text index not in the table"),
                (Some(Decl::Option(opts)), Some(v)) => v
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| opts.get(i).cloned())
                    .map(AttrValue::Text)
                    .ok_or("an option index past its options"),
                (Some(Decl::Integer), Some(v)) if is_number(v, true) => {
                    Ok(AttrValue::Number(v.to_string()))
                }
                (Some(Decl::Float), Some(v)) if is_number(v, false) => {
                    Ok(AttrValue::Number(v.to_string()))
                }
                _ => Err("a value its declaration doesn't allow"),
            };
            match value {
                Ok(v) => {
                    out.insert(name.clone(), v);
                }
                Err(why) => ignored.push(Ignored {
                    name: name.clone(),
                    why: why.into(),
                }),
            }
        }
        (out, ignored)
    }
}

/// What follows a record's fields: its attribute list and its `ID=`.
pub fn tail(rest: Option<&str>) -> (&str, Option<&str>) {
    let mut attrs = "";
    let mut id = None;
    for part in rest.unwrap_or("").split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("ID=") {
            id = Some(v);
        } else if !part.is_empty() {
            attrs = part;
        }
    }
    (attrs, id)
}

/// An attribute list file (`attrlist`): each `.name = value` but `UNITS`.
/// Booleans are `yes` or `no` (a `no` is no attribute), options are
/// written by name, texts as written with one pair of enclosing double
/// quotes taken off.
pub fn attrlist(text: &str, decls: &Decls) -> Result<(Attributes, Vec<Ignored>), Bad> {
    let s = structured(text)?;
    if let Some(a) = s.arrays.first() {
        return Err(Bad::new(a.line, 1, "an array in an attribute list"));
    }
    let mut out = Attributes::new();
    let mut ignored = Vec::new();
    for f in &s.fields {
        if f.key.eq_ignore_ascii_case("UNITS") {
            continue;
        }
        let v = f.value.as_str();
        let value = match decls.get(&f.key) {
            None => Err("undeclared"),
            Some(Decl::Boolean) if v.eq_ignore_ascii_case("yes") => Ok(Some(AttrValue::Flag)),
            Some(Decl::Boolean) if v.eq_ignore_ascii_case("no") => Ok(None),
            Some(Decl::Text) => {
                let unquoted = v
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .filter(|s| !s.contains('"'));
                Ok(Some(AttrValue::Text(unquoted.unwrap_or(v).to_string())))
            }
            Some(Decl::Option(opts)) => opts
                .iter()
                .find(|o| o.eq_ignore_ascii_case(v))
                .map(|o| Some(AttrValue::Text(o.clone())))
                .ok_or("not one of its options"),
            Some(Decl::Integer) if is_number(v, true) => Ok(Some(AttrValue::Number(v.into()))),
            Some(Decl::Float) if is_number(v, false) => Ok(Some(AttrValue::Number(v.into()))),
            Some(_) => Err("a value its declaration doesn't allow"),
        };
        match value {
            Ok(Some(v)) => {
                out.insert(f.key.clone(), v);
            }
            Ok(None) => {}
            Err(why) => ignored.push(Ignored {
                name: f.key.clone(),
                why: why.into(),
            }),
        }
    }
    Ok((out, ignored))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(decls: &Decls) -> Table {
        let mut t = Table::default();
        for (i, l) in [
            "0 .smd",
            "1 .nomenclature",
            "2 .test_point",
            "3 .geometry",
            "4 .pad_usage",
            "5 .comp_height",
            "6 .padstack_id",
            "7 my_attr",
        ]
        .iter()
        .enumerate()
        {
            t.name(l, i as u32 + 1, decls).unwrap();
        }
        t.text("0 9796334", 9).unwrap();
        t.text("2 two words", 10).unwrap();
        t
    }

    #[test]
    fn system_attributes_are_declared() {
        let d = Decls::default();
        assert_eq!(d.get(".smd"), Some(Decl::Boolean));
        assert_eq!(d.get(".geometry"), Some(Decl::Text));
        assert_eq!(d.get(".padstack_id"), Some(Decl::Integer));
        assert_eq!(d.get(".comp_height"), Some(Decl::Float));
        assert_eq!(d.get(".part_desc7"), Some(Decl::Text));
        assert_eq!(
            d.get(".PAD_USAGE"),
            Some(Decl::Option(
                [
                    "toeprint",
                    "via",
                    "g_fiducial",
                    "l_fiducial",
                    "tooling_hole",
                    "bond_finger"
                ]
                .map(String::from)
                .to_vec()
            ))
        );
        assert_eq!(d.get("my_attr"), None);
        assert!(SYSTEM.windows(2).all(|w| w[0].0 < w[1].0), "sorted");
        assert_eq!(SYSTEM.len(), 373);
    }

    #[test]
    fn lookup_tables_read_by_declaration() {
        let d = Decls::default();
        let t = table(&d);
        // The specification's example: 3=2 is &2, 4=0 the first option.
        let (a, ignored) = t.values("0,3=2,4=0,5=0.25,6=-12");
        assert_eq!(a.get(".smd"), Some(&AttrValue::Flag));
        assert_eq!(
            a.get(".geometry"),
            Some(&AttrValue::Text("two words".into()))
        );
        assert_eq!(
            a.get(".pad_usage"),
            Some(&AttrValue::Text("toeprint".into()))
        );
        assert_eq!(
            a.get(".comp_height"),
            Some(&AttrValue::Number("0.25".into()))
        );
        assert_eq!(
            a.get(".padstack_id"),
            Some(&AttrValue::Number("-12".into()))
        );
        assert!(ignored.is_empty());
        let (a, ignored) = t.values("#0, 7=1, 9, 2=1, 3=1, 4=6, 6=1.5, 5=x");
        assert_eq!(a.len(), 1);
        let why: Vec<_> = ignored
            .iter()
            .map(|i| (i.name.as_str(), i.why.as_str()))
            .collect();
        assert_eq!(
            why,
            [
                ("my_attr", "undeclared"),
                ("@9", "not in the attribute table"),
                (".test_point", "a value its declaration doesn't allow"),
                (".geometry", "a text index not in the table"),
                (".pad_usage", "an option index past its options"),
                (".padstack_id", "a value its declaration doesn't allow"),
                (".comp_height", "a value its declaration doesn't allow"),
            ]
        );
        assert!(t.clone().name("x", 1, &d).is_err());
    }

    #[test]
    fn a_jobs_own_declarations_win() {
        let mut d = Decls::default();
        d.read("UNITS=MM\nOPTION {\n NAME=my_attr\n OPTIONS=a;b;c\n}\nTEXT\n{\nNAME=.smd\n}\n")
            .unwrap();
        let t = table(&d);
        let (a, _) = t.values("7=2,0");
        assert_eq!(a.get("my_attr"), Some(&AttrValue::Text("c".into())));
        assert!(!a.contains_key(".smd"), "now a text");
        assert!(Decls::default().read("WHAT {\nNAME=x\n}\n").is_err());
        assert!(Decls::default().read("TEXT {\nPROMPT=x\n}\n").is_err());
    }

    #[test]
    fn record_tails() {
        assert_eq!(tail(Some("3=2,4=0;ID=123456")), ("3=2,4=0", Some("123456")));
        assert_eq!(tail(Some(";ID=3948")), ("", Some("3948")));
        assert_eq!(tail(Some("1,3=0")), ("1,3=0", None));
        assert_eq!(tail(None), ("", None));
    }

    #[test]
    fn attribute_lists() {
        let text = "UNITS=INCH\n.out_mirror = no\n.lpol_done = yes\n.eda_layers = \"Top\"\n.comment =\n.layer_hdi_type = buildup\n.copper_weight = 0.50798258\nconnector = no\n.out_angle = 45\n";
        let (a, ignored) = attrlist(text, &Decls::default()).unwrap();
        assert_eq!(a.get(".lpol_done"), Some(&AttrValue::Flag));
        assert!(!a.contains_key(".out_mirror"));
        assert_eq!(a.get(".eda_layers"), Some(&AttrValue::Text("Top".into())));
        assert_eq!(a.get(".comment"), Some(&AttrValue::Text("".into())));
        assert_eq!(
            a.get(".layer_hdi_type"),
            Some(&AttrValue::Text("Buildup".into()))
        );
        assert_eq!(
            a.get(".copper_weight"),
            Some(&AttrValue::Number("0.50798258".into()))
        );
        let names: Vec<_> = ignored.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["connector", ".out_angle"]);
        let (a, _) = attrlist(".eda_layers = \"A\"&\"B\"\n", &Decls::default()).unwrap();
        assert_eq!(
            a.get(".eda_layers"),
            Some(&AttrValue::Text("\"A\"&\"B\"".into()))
        );
        assert!(attrlist("X {\n}\n", &Decls::default()).is_err());
    }
}
