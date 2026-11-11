//! The organisation's symbol standards: `library/standards.json` (kind
//! `ryueda.standards`, format 1) in a project's or a library project's
//! library. What an organisation writes down about how symbols are drawn:
//! the resistor's shape, the grid, pin lengths, the margin round pin
//! names, which pin names are ground, which name a function's group, and
//! per class of part which side each role of pin stands on. Every field is
//! optional; a field present replaces the built-in value whole. The
//! symbol lint (`symlint`) holds symbols to them; templates (`template`)
//! draw to them. Symbols are never redrawn automatically (design 143).

use crate::error::Code;
use crate::fault::Fault;
use crate::symbol::{PinType, Side};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const KIND: &str = "ryueda.standards";
pub const FORMAT: u64 = 1;
/// The file's name in a library's folder.
pub const FILE: &str = "standards.json";

/// Which standard resistors (the shapes that differ) are drawn to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shapes {
    /// IEEE 315: the zigzag resistor.
    #[default]
    Ansi,
    /// IEC 60617: the box resistor.
    Iec,
}

impl Shapes {
    pub fn as_str(self) -> &'static str {
        match self {
            Shapes::Ansi => "ansi",
            Shapes::Iec => "iec",
        }
    }
}

/// A class of part: what the standards' rules and templates are per.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Resistor,
    Capacitor,
    Inductor,
    Diode,
    Led,
    Transistor,
    Opamp,
    Connector,
    Regulator,
    Ic,
}

impl Class {
    pub const ALL: [Class; 10] = [
        Class::Resistor,
        Class::Capacitor,
        Class::Inductor,
        Class::Diode,
        Class::Led,
        Class::Transistor,
        Class::Opamp,
        Class::Connector,
        Class::Regulator,
        Class::Ic,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Class::Resistor => "resistor",
            Class::Capacitor => "capacitor",
            Class::Inductor => "inductor",
            Class::Diode => "diode",
            Class::Led => "led",
            Class::Transistor => "transistor",
            Class::Opamp => "opamp",
            Class::Connector => "connector",
            Class::Regulator => "regulator",
            Class::Ic => "ic",
        }
    }

    pub fn parse(s: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// Every class's name, for refusals.
    pub fn names() -> Vec<&'static str> {
        Class::ALL.iter().map(|c| c.as_str()).collect()
    }
}

/// What a pin does, as the side rules read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    PowerIn,
    PowerOut,
    Ground,
    Input,
    Output,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::PowerIn => "power_in",
            Role::PowerOut => "power_out",
            Role::Ground => "ground",
            Role::Input => "input",
            Role::Output => "output",
        }
    }
}

/// One class's rules.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassRule {
    /// The side of the body each role of pin stands on; a role left out
    /// may stand anywhere.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sides: BTreeMap<Role, Side>,
    /// Whether a function's pins (`groups`) stand together, in order.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub groups: bool,
    /// Whether pins are the standards' `pin_length_nm` long.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pin_length: bool,
    /// The most pins one unit may have before the part is drawn in units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pins_per_unit: Option<u32>,
}

/// The standards file. `Standards::default()` is the built-in standards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Standards {
    pub shapes: Shapes,
    /// Pins' connection points lie on multiples of it.
    pub grid_nm: i64,
    /// A pin's length, for classes whose rule says `pin_length`.
    pub pin_length_nm: i64,
    /// The least room between a pin's name and its body's edge, or
    /// another name, along the way the name reads.
    pub body_margin_nm: i64,
    /// Pin names are measured, and templates sized, at this size.
    pub pin_name_size_nm: i64,
    /// Held for the style (plan 2.E), which draws pin numbers.
    pub pin_number_size_nm: i64,
    /// Name patterns of ground pins.
    pub ground: Vec<String>,
    /// Function groups by name: name patterns whose pins stand together.
    pub groups: BTreeMap<String, Vec<String>>,
    pub classes: BTreeMap<Class, ClassRule>,
}

impl Default for Standards {
    fn default() -> Self {
        let sides = |list: &[(Role, Side)]| list.iter().copied().collect::<BTreeMap<_, _>>();
        let mut classes: BTreeMap<Class, ClassRule> = Class::ALL
            .iter()
            .map(|c| (*c, ClassRule::default()))
            .collect();
        classes.insert(
            Class::Ic,
            ClassRule {
                sides: sides(&[
                    (Role::PowerIn, Side::Up),
                    (Role::PowerOut, Side::Up),
                    (Role::Ground, Side::Down),
                    (Role::Input, Side::Left),
                    (Role::Output, Side::Right),
                ]),
                groups: true,
                pin_length: true,
                max_pins_per_unit: Some(64),
            },
        );
        classes.insert(
            Class::Regulator,
            ClassRule {
                sides: sides(&[
                    (Role::PowerIn, Side::Left),
                    (Role::PowerOut, Side::Right),
                    (Role::Ground, Side::Down),
                    (Role::Input, Side::Left),
                    (Role::Output, Side::Right),
                ]),
                pin_length: true,
                ..ClassRule::default()
            },
        );
        classes.insert(
            Class::Opamp,
            ClassRule {
                sides: sides(&[(Role::Input, Side::Left), (Role::Output, Side::Right)]),
                pin_length: true,
                ..ClassRule::default()
            },
        );
        let list = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        Standards {
            shapes: Shapes::Ansi,
            grid_nm: ryusch::geom::GRID,
            pin_length_nm: 2 * ryusch::geom::GRID,
            body_margin_nm: ryusch::geom::GRID / 2,
            pin_name_size_nm: ryusch::font::SIZE,
            pin_number_size_nm: ryusch::font::MIN_SIZE,
            ground: list(&["*GND*", "VSS*", "VEE"]),
            groups: [
                ("gpio", list(&["GPIO#"])),
                ("i2c", list(&["SDA", "SCL"])),
                ("spi", list(&["MOSI", "MISO", "SCK", "CS"])),
                ("usb", list(&["D+", "D-"])),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
            classes,
        }
    }
}

/// The parts of a pin's name a pattern is matched against.
fn tokens(name: &str) -> impl Iterator<Item = &str> {
    name.split(['_', '/', ',', ' ']).filter(|t| !t.is_empty())
}

/// Whether `pattern` matches the whole of `token`, ASCII case aside: `#`
/// one or more digits, `*` any run. Linear in both (no backtracking).
fn glob(pattern: &[u8], token: &[u8]) -> bool {
    // can[j]: the pattern so far matches token[..j].
    let mut can = vec![false; token.len() + 1];
    can[0] = true;
    for &c in pattern {
        let mut next = vec![false; token.len() + 1];
        match c {
            b'*' => {
                let mut any = false;
                for j in 0..=token.len() {
                    any |= can[j];
                    next[j] = any;
                }
            }
            b'#' => {
                for j in 1..=token.len() {
                    next[j] = token[j - 1].is_ascii_digit() && (can[j - 1] || next[j - 1]);
                }
            }
            c => {
                for j in 1..=token.len() {
                    next[j] = can[j - 1] && token[j - 1].eq_ignore_ascii_case(&c);
                }
            }
        }
        can = next;
    }
    can[token.len()]
}

/// Whether `pattern` matches one of `name`'s tokens.
pub fn matches(pattern: &str, name: &str) -> bool {
    tokens(name).any(|t| glob(pattern.as_bytes(), t.as_bytes()))
}

/// The first run of digits in `token`, what a `#` group orders by.
pub fn number(token: &str) -> Option<u64> {
    let digits: String = token
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits.parse().unwrap_or(u64::MAX))
    }
}

impl Standards {
    /// A pin's role, from its type and its name.
    pub fn role(&self, name: &str, ty: PinType) -> Option<Role> {
        let ground = self.ground.iter().any(|g| matches(g, name));
        match ty {
            PinType::PowerIn | PinType::PowerOut | PinType::Passive if ground => Some(Role::Ground),
            PinType::PowerIn => Some(Role::PowerIn),
            PinType::PowerOut => Some(Role::PowerOut),
            PinType::Input => Some(Role::Input),
            PinType::Output | PinType::TriState | PinType::OpenCollector | PinType::OpenEmitter => {
                Some(Role::Output)
            }
            _ => None,
        }
    }

    /// The group a pin's name belongs to (the first by name), and its
    /// number when the pattern it matched counts (`#`).
    pub fn group_of(&self, name: &str) -> Option<(&str, Option<u64>)> {
        for (group, patterns) in &self.groups {
            for p in patterns {
                if let Some(t) = tokens(name).find(|t| glob(p.as_bytes(), t.as_bytes())) {
                    let n = if p.contains('#') { number(t) } else { None };
                    return Some((group.as_str(), n));
                }
            }
        }
        None
    }

    /// A class's rule; no rules for a class the file leaves out.
    pub fn rule(&self, class: Class) -> ClassRule {
        self.classes.get(&class).cloned().unwrap_or_default()
    }

    /// What `standards_invalid` finds, each with its JSON pointer.
    pub fn check(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        let bad = |ptr: String, msg: String| {
            Fault::error(Code::StandardsInvalid, msg).with("pointer", ptr)
        };
        let ten_mm = 10_000_000;
        if !(1..=ten_mm).contains(&self.grid_nm) {
            out.push(bad(
                "/grid_nm".into(),
                format!("the grid is {} nm; it is 1 nm to 10 mm", self.grid_nm),
            ));
        }
        if !(0..=5 * ten_mm).contains(&self.pin_length_nm) {
            out.push(bad(
                "/pin_length_nm".into(),
                format!(
                    "a pin length of {} nm; it is 0 to 50 mm",
                    self.pin_length_nm
                ),
            ));
        }
        if !(0..=ten_mm).contains(&self.body_margin_nm) {
            out.push(bad(
                "/body_margin_nm".into(),
                format!("a margin of {} nm; it is 0 to 10 mm", self.body_margin_nm),
            ));
        }
        for (field, v) in [
            ("pin_name_size_nm", self.pin_name_size_nm),
            ("pin_number_size_nm", self.pin_number_size_nm),
        ] {
            if !(1..=ten_mm).contains(&v) {
                out.push(bad(
                    format!("/{field}"),
                    format!("a text size of {v} nm; it is 1 nm to 10 mm"),
                ));
            }
        }
        let pattern_ok = |p: &str| {
            !p.is_empty()
                && p.chars().count() <= 32
                && p.chars()
                    .all(|c| !(c.is_whitespace() || c.is_control() || "_/,".contains(c)))
        };
        let why = "a pattern is 1 to 32 characters, with no space, control character, _, / or ,";
        for (i, p) in self.ground.iter().enumerate() {
            if !pattern_ok(p) {
                out.push(bad(format!("/ground/{i}"), format!("{p:?}: {why}")));
            }
        }
        for (name, patterns) in &self.groups {
            let at = format!("/groups/{}", crate::symbol::pointer_escape(name));
            if patterns.is_empty() {
                out.push(bad(at.clone(), format!("group {name} has no pattern")));
            }
            for (i, p) in patterns.iter().enumerate() {
                if !pattern_ok(p) {
                    out.push(bad(format!("{at}/{i}"), format!("{p:?}: {why}")));
                }
            }
        }
        for (class, rule) in &self.classes {
            if rule.max_pins_per_unit == Some(0) {
                out.push(bad(
                    format!("/classes/{}/max_pins_per_unit", class.as_str()),
                    format!(
                        "{} allows no pin in a unit; it is 1 or more",
                        class.as_str()
                    ),
                ));
            }
        }
        out
    }
}

/// `s` as its file holds it: only the top-level fields that differ from
/// the built-in standards (`{}` for the built-in standards themselves).
pub fn reduced(s: &Standards) -> Value {
    let built_in = serde_json::to_value(Standards::default()).expect("standards serialize");
    let mut v = serde_json::to_value(s).expect("standards serialize");
    if let (Value::Object(o), Value::Object(d)) = (&mut v, &built_in) {
        o.retain(|k, x| d.get(k) != Some(x));
    }
    v
}

/// Applies a JSON merge patch (RFC 7396) to `base`: objects merge, `null`
/// removes a key, anything else replaces.
pub fn merge(base: &mut Value, patch: &Value) {
    let Value::Object(p) = patch else {
        *base = patch.clone();
        return;
    };
    if !base.is_object() {
        *base = Value::Object(Map::new());
    }
    let b = base.as_object_mut().expect("an object");
    for (k, v) in p {
        if v.is_null() {
            b.remove(k);
        } else {
            merge(b.entry(k.clone()).or_insert(Value::Null), v);
        }
    }
}

/// `standards.get`: the standards that judge `library`'s symbols (this
/// project's own by default), whole, with where they come from.
pub fn get(p: &crate::project::Project, library: Option<&str>) -> crate::error::Result<Value> {
    let alias = library.unwrap_or(crate::project::SELF);
    let lib = p.library(alias)?;
    let (s, from) = p.standards_for(alias)?;
    let path = (from != "built_in").then(|| {
        crate::source::join(
            &crate::source::join(
                &p.library(&from).map(|l| l.base.clone()).unwrap_or_default(),
                crate::library::DIR,
            ),
            FILE,
        )
    });
    Ok(serde_json::json!({
        "library": alias,
        "from": from,
        "path": path,
        "standards": s,
        "revision": lib.revision,
    }))
}

/// `standards.set`: a merge patch on this project's own standards (the
/// built-in ones where it has none), written as only what differs from
/// the built-in standards, the file deleted when nothing does. Guarded by
/// this project's library's revision.
pub fn set(
    session: &mut crate::session::Session,
    patch: &Value,
    expected: Option<&str>,
) -> crate::error::Result<Value> {
    use crate::error::Error;
    use crate::project::SELF;
    let p = session.project();
    let lib = p.library(SELF)?;
    crate::server::check_revision("library self", &lib.revision, expected)?;
    if !patch.is_object() {
        return Err(Error::new(
            Code::InvalidParam,
            "a patch is a JSON object of the fields to change (null puts one back)",
        )
        .with("param", "patch"));
    }
    let base = match &lib.standards {
        Some(st) => st.item.clone()?,
        None => p.standards_for(SELF).map(|(s, _)| s).unwrap_or_default(),
    };
    let mut v = serde_json::to_value(&base).expect("standards serialize");
    merge(&mut v, patch);
    let s: Standards = serde_json::from_value(v).map_err(|e| {
        Error::new(Code::InvalidParam, format!("not standards: {e}")).with("param", "patch")
    })?;
    let faults = s.check();
    if !faults.is_empty() {
        return Err(crate::write::refused("library/standards.json", &faults));
    }
    let path = crate::source::join(crate::library::DIR, FILE);
    let mut w = crate::write::Writes::default();
    match reduced(&s) {
        Value::Object(o) if o.is_empty() => w.delete(path),
        v => {
            w.0.push((path, Some(crate::doc::to_string(&v, KIND, FORMAT))))
        }
    }
    crate::write::commit(session, "library/standards.json", w)?;
    get(session.project(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc;
    use serde_json::json;

    #[test]
    fn patterns_match_tokens_digits_and_runs() {
        assert!(matches("SDA", "I2C1_SDA"));
        assert!(matches("sda", "SDA"));
        assert!(matches("GPIO#", "GPIO12/ADC2"));
        assert!(!matches("GPIO#", "GPIO"));
        assert!(!matches("GPIO#", "GPIOA"));
        assert!(matches("*GND*", "AGND"));
        assert!(matches("*GND*", "GND"));
        assert!(matches("VSS*", "VSSA"));
        assert!(!matches("VSS*", "AVSS"));
        assert!(matches("D+", "USB_D+"));
        assert!(!matches("CS", "~{CS}"));
        assert_eq!(number("GPIO12"), Some(12));
        assert_eq!(number("SDA"), None);
        // Linear in the name: a long name and a pattern of wildcards
        // answer at once.
        let long = "1".repeat(100_000);
        assert!(matches("#*#*#*#", &long));
    }

    #[test]
    fn roles_come_from_type_and_name() {
        let s = Standards::default();
        assert_eq!(s.role("AGND", PinType::PowerIn), Some(Role::Ground));
        assert_eq!(s.role("GND", PinType::Passive), Some(Role::Ground));
        assert_eq!(s.role("VDD", PinType::PowerIn), Some(Role::PowerIn));
        assert_eq!(s.role("VOUT", PinType::PowerOut), Some(Role::PowerOut));
        assert_eq!(s.role("EN", PinType::Input), Some(Role::Input));
        assert_eq!(s.role("SDO", PinType::TriState), Some(Role::Output));
        assert_eq!(s.role("SDA", PinType::Bidirectional), None);
        assert_eq!(s.role("1", PinType::Passive), None);
    }

    #[test]
    fn groups_are_found_by_name_with_their_numbers() {
        let s = Standards::default();
        assert_eq!(s.group_of("GPIO7"), Some(("gpio", Some(7))));
        assert_eq!(s.group_of("I2C0_SCL"), Some(("i2c", None)));
        assert_eq!(s.group_of("QSPI_SD0"), None);
    }

    #[test]
    fn a_file_holds_only_what_differs_and_reads_back_whole() {
        let s = Standards::default();
        assert!(s.check().is_empty(), "{:?}", s.check());
        let empty: Standards = doc::from_value(
            json!({"kind": KIND, "format": 1}),
            "library/standards.json",
            KIND,
            FORMAT,
        )
        .unwrap();
        assert_eq!(empty, s);
        let iec: Standards = doc::from_value(
            json!({"kind": KIND, "format": 1, "shapes": "iec", "classes": {"ic": {"sides": {"power_in": "up"}}}}),
            "library/standards.json",
            KIND,
            FORMAT,
        )
        .unwrap();
        assert_eq!(iec.shapes, Shapes::Iec);
        assert_eq!(iec.grid_nm, s.grid_nm);
        // `classes` given replaces the built-in classes whole.
        assert_eq!(iec.classes.len(), 1);
        assert_eq!(iec.rule(Class::Regulator), ClassRule::default());
        assert_eq!(
            reduced(&iec),
            json!({"shapes": "iec", "classes": {"ic": {"sides": {"power_in": "up"}}}})
        );
        assert_eq!(reduced(&s), json!({}));
        let e = doc::from_value::<Standards>(
            json!({"kind": KIND, "format": 1, "colour": "red"}),
            "library/standards.json",
            KIND,
            FORMAT,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
    }

    #[test]
    fn checks_name_each_bad_value_by_pointer() {
        let mut s = Standards::default();
        s.grid_nm = 0;
        s.pin_length_nm = -1;
        s.pin_name_size_nm = 0;
        s.ground = vec![String::new()];
        s.groups.insert("bad".into(), vec![]);
        s.groups.insert("spaced".into(), vec!["A B".into()]);
        s.classes.insert(
            Class::Ic,
            ClassRule {
                max_pins_per_unit: Some(0),
                ..ClassRule::default()
            },
        );
        let pointers: Vec<String> = s
            .check()
            .iter()
            .map(|f| {
                assert_eq!(f.error.code, Code::StandardsInvalid);
                f.error.details["pointer"].as_str().unwrap().to_owned()
            })
            .collect();
        assert_eq!(
            pointers,
            [
                "/grid_nm",
                "/pin_length_nm",
                "/pin_name_size_nm",
                "/ground/0",
                "/groups/bad",
                "/groups/spaced/0",
                "/classes/ic/max_pins_per_unit",
            ]
        );
    }

    #[test]
    fn merge_patches_replace_merge_and_remove() {
        let mut v = json!({"a": 1, "b": {"c": 2, "d": 3}});
        merge(
            &mut v,
            &json!({"a": null, "b": {"c": null, "e": {"f": null, "g": 4}}, "h": [1]}),
        );
        assert_eq!(v, json!({"b": {"d": 3, "e": {"g": 4}}, "h": [1]}));
    }
}
