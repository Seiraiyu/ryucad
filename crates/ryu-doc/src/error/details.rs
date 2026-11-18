//! What each error code's `details` may hold (design 698, #771): the keys
//! a code declares, the keys every code may carry, and the check every
//! error passes as it leaves a product in a debug build. A code with an
//! entry in the table is declared; one without is not yet, and every code
//! added from now on must be (`tests/golden/undeclared.txt` only shrinks).

use super::Code;
use serde_json::Value;
use std::fmt;

/// What a key's values are. `null` is every kind: a key with no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Str,
    Num,
    Bool,
    Arr,
    Obj,
    Any,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Str => "str",
            Kind::Num => "num",
            Kind::Bool => "bool",
            Kind::Arr => "arr",
            Kind::Obj => "obj",
            Kind::Any => "any",
        }
    }

    /// Whether `v` is of this kind.
    pub fn admits(self, v: &Value) -> bool {
        matches!(
            (self, v),
            (_, Value::Null)
                | (Kind::Any, _)
                | (Kind::Str, Value::String(_))
                | (Kind::Num, Value::Number(_))
                | (Kind::Bool, Value::Bool(_))
                | (Kind::Arr, Value::Array(_))
                | (Kind::Obj, Value::Object(_))
        )
    }

    /// The kind of `v` (`any` for `null`).
    pub fn of(v: &Value) -> Kind {
        match v {
            Value::Null => Kind::Any,
            Value::String(_) => Kind::Str,
            Value::Number(_) => Kind::Num,
            Value::Bool(_) => Kind::Bool,
            Value::Array(_) => Kind::Arr,
            Value::Object(_) => Kind::Obj,
        }
    }
}

/// One key a code's details may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub name: &'static str,
    /// An alias's is its declared key's ([`kind`]).
    pub kind: Kind,
    /// What it holds, for agents (empty for an alias: [`about`]).
    pub about: &'static str,
    /// The declared key this older spelling is carried beside: an error
    /// that holds it holds the declared key too (`Error::with`).
    pub alias_of: Option<&'static str>,
}

/// A declared key.
pub const fn key(name: &'static str, kind: Kind, about: &'static str) -> Key {
    Key {
        name,
        kind,
        about,
        alias_of: None,
    }
}

/// An older spelling of the declared key `of`, still declared, so nothing
/// an agent reads today disappears.
pub const fn alias(name: &'static str, of: &'static str) -> Key {
    Key {
        name,
        kind: Kind::Any,
        about: "",
        alias_of: Some(of),
    }
}

/// The keys every code may carry, declared or not.
pub const COMMON: &[Key] = &[
    key(
        "hint",
        Kind::Str,
        "What to do about it: a command to run or a sentence.",
    ),
    key("commit", Kind::Str, "The commit the request read or wrote."),
    key(
        "pointer",
        Kind::Str,
        "A JSON pointer to the value at fault, in the request or the file.",
    ),
    key("path", Kind::Str, "The file or folder at fault."),
    key(
        "op",
        Kind::Num,
        "In a batch, the index of the op that failed.",
    ),
    key(
        "method",
        Kind::Str,
        "In a batch, the method of the op that failed.",
    ),
];

use Kind::Str;

/// The table: each declared code and its own keys, beside [`COMMON`].
macro_rules! details {
    ($($code:ident => [$($key:expr),* $(,)?],)*) => {
        /// The keys `code` declares, beside [`COMMON`]; `None` while it is
        /// undeclared.
        pub fn keys(code: Code) -> Option<&'static [Key]> {
            match code {
                $(Code::$code => {
                    const KEYS: &[Key] = &[$($key),*];
                    Some(KEYS)
                })*
                _ => None,
            }
        }

        /// Every declared code, in the table's order.
        pub const DECLARED: &[Code] = &[$(Code::$code),*];
    };
}

details! {
    LabelNotFound => [
        key("part", Str, "The part whose label is missing."),
        key("label", Str, "The label asked for."),
        key("project", Str, "The project ID the label was looked for in."),
        key("alias", Str, "The dependency alias the reference went through."),
        key("component", Str, "The library component whose label is missing (ryueda)."),
        key("param", Str, "The request parameter that named the label."),
    ],
}

/// `code` as agents see it: `not_found`.
pub fn name(code: Code) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The key `name` that `code` declares, common keys included; `None` when
/// `code` is undeclared or doesn't declare it.
pub fn find(code: Code, name: &str) -> Option<&'static Key> {
    keys(code)?.iter().chain(COMMON).find(|k| k.name == name)
}

/// The declared key that `name` is an older spelling of, in `code`.
pub fn alias_of(code: Code, name: &str) -> Option<&'static str> {
    find(code, name)?.alias_of
}

/// The kind of `key`'s values in `code`: an alias's is its declared key's.
pub fn kind(code: Code, key: &Key) -> Kind {
    match key.alias_of {
        Some(of) => find(code, of).map_or(Kind::Any, |k| k.kind),
        None => key.kind,
    }
}

/// What a key's words are: an alias says what it is beside.
pub fn about(key: &Key) -> String {
    match key.alias_of {
        Some(of) => format!("An older spelling of `{of}`, carried beside it."),
        None => key.about.to_owned(),
    }
}

/// One way an error's details break its code's declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The key, empty when the details aren't an object.
    pub key: String,
    /// The kind of value it holds.
    pub got: Kind,
    /// The kind declared, `None` when the key isn't declared.
    pub declared: Option<Kind>,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let got = self.got.name();
        match self.declared {
            _ if self.key.is_empty() => write!(f, "details are {got}, not an object"),
            None => write!(f, "`{}` ({got}) isn't declared", self.key),
            Some(k) => write!(f, "`{}` is {got}, declared {}", self.key, k.name()),
        }
    }
}

/// How `details` break `code`'s declaration: a key it doesn't declare, or
/// a value of another kind. Nothing for an undeclared code, and missing
/// keys are never a problem (one code is raised from many places).
pub fn problems(code: Code, details: &Value) -> Vec<Problem> {
    if keys(code).is_none() {
        return Vec::new();
    }
    let map = match details {
        Value::Null => return Vec::new(),
        Value::Object(m) => m,
        other => {
            return vec![Problem {
                key: String::new(),
                got: Kind::of(other),
                declared: Some(Kind::Obj),
            }];
        }
    };
    let mut found = Vec::new();
    for (k, v) in map {
        match find(code, k) {
            None => found.push(Problem {
                key: k.clone(),
                got: Kind::of(v),
                declared: None,
            }),
            Some(key) => {
                let want = kind(code, key);
                if !want.admits(v) {
                    found.push(Problem {
                        key: k.clone(),
                        got: Kind::of(v),
                        declared: Some(want),
                    });
                }
            }
        }
    }
    found
}

/// Set to a file, the check appends what it finds there (one
/// `code<TAB>key<TAB>kind<TAB>declared` line a problem) instead of
/// panicking: the record run of plan 33.E, and of whoever declares the
/// next codes.
pub const RECORD: &str = "RYU_DETAILS_RECORD";

/// Checks `e` against its code's declaration as it leaves a product. A
/// debug build panics on a key the code doesn't declare or a value of
/// another kind (see [`RECORD`]); a release build does nothing.
pub fn check(e: &super::Error) {
    check_value(e.code, &e.details);
}

/// [`check`] for a refusal whose details are already JSON text
/// (`ryu_session::rpc::Raw::Error`: a failed rebuild's report).
pub fn check_raw(code: Code, details: &str) {
    if !cfg!(debug_assertions) || keys(code).is_none() || details == "null" {
        return;
    }
    let v: Value = serde_json::from_str(details)
        .unwrap_or_else(|e| panic!("error `{}`: its details aren't JSON: {e}", name(code)));
    check_value(code, &v);
}

fn check_value(code: Code, details: &Value) {
    if !cfg!(debug_assertions) {
        return;
    }
    let found = problems(code, details);
    if found.is_empty() {
        return;
    }
    if let Some(file) = std::env::var_os(RECORD) {
        record(std::path::Path::new(&file), code, &found);
        return;
    }
    let list: Vec<String> = found.iter().map(ToString::to_string).collect();
    panic!(
        "error `{}`: {}; declare it in crates/ryu-doc/src/error/details.rs or fix the call (design 698, #771)",
        name(code),
        list.join("; ")
    );
}

/// Appends `found` to `file`, in one write so parallel tests' lines don't
/// interleave.
fn record(file: &std::path::Path, code: Code, found: &[Problem]) {
    use std::io::Write;
    let code = name(code);
    let mut text = String::new();
    for p in found {
        let declared = p.declared.map_or("-", Kind::name);
        text.push_str(&format!(
            "{code}\t{}\t{}\t{declared}\n",
            p.key,
            p.got.name()
        ));
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .unwrap_or_else(|e| panic!("{RECORD}={}: {e}", file.display()));
    let _ = f.write_all(text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_kind_admits_its_values() {
        let values = [
            json!("a"),
            json!(1.5),
            json!(true),
            json!([1]),
            json!({"a": 1}),
        ];
        let kinds = [Kind::Str, Kind::Num, Kind::Bool, Kind::Arr, Kind::Obj];
        for (i, k) in kinds.iter().enumerate() {
            for (j, v) in values.iter().enumerate() {
                assert_eq!(k.admits(v), i == j, "{k:?} {v}");
            }
            assert!(k.admits(&Value::Null), "null is every kind");
            assert_eq!(Kind::of(&values[i]), *k);
        }
        assert!(values.iter().all(|v| Kind::Any.admits(v)));
    }

    #[test]
    fn problems_name_undeclared_keys_and_wrong_kinds() {
        let d = json!({"part": "p", "label": 3, "colour": "red", "hint": "x", "op": 1});
        let mut found: Vec<String> = problems(Code::LabelNotFound, &d)
            .iter()
            .map(ToString::to_string)
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                "`colour` (str) isn't declared",
                "`label` is num, declared str"
            ]
        );
        assert!(problems(Code::LabelNotFound, &json!({"label": null})).is_empty());
    }

    #[test]
    fn undeclared_codes_and_absent_details_have_no_problems() {
        assert!(problems(Code::Conflict, &json!({"anything": 1})).is_empty());
        assert!(problems(Code::LabelNotFound, &Value::Null).is_empty());
        assert_eq!(
            problems(Code::LabelNotFound, &json!("text"))[0].to_string(),
            "details are str, not an object"
        );
    }

    /// The table's own rules: no code declared twice, no key twice in a
    /// code or beside a common key, every alias resolving to a declared
    /// key that isn't itself an alias, and every declared key explained.
    #[test]
    fn the_table_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for &code in DECLARED {
            assert!(seen.insert(code), "{code:?} declared twice");
            let own = keys(code).expect("declared");
            let mut names = std::collections::HashSet::new();
            for k in own.iter().chain(COMMON) {
                assert!(names.insert(k.name), "{code:?}: `{}` twice", k.name);
                match k.alias_of {
                    Some(of) => {
                        let target = find(code, of)
                            .unwrap_or_else(|| panic!("{code:?}: `{}` aliases nothing", k.name));
                        assert!(
                            target.alias_of.is_none(),
                            "{code:?}: `{}` aliases an alias",
                            k.name
                        );
                    }
                    None => assert!(!k.about.is_empty(), "{code:?}: `{}` unexplained", k.name),
                }
            }
        }
    }

    use crate::error::Error;

    #[test]
    fn a_declared_error_passes_and_undeclared_codes_are_not_read() {
        check(
            &Error::new(Code::LabelNotFound, "x")
                .with("part", "p")
                .with("hint", "h"),
        );
        check_raw(Code::LabelNotFound, r#"{"part":"p","commit":"abc"}"#);
        check_raw(Code::LabelNotFound, "null");
        check_raw(
            Code::Conflict,
            "not even JSON: an undeclared code isn't read",
        );
    }

    /// In a debug build an undeclared key panics, naming the code, the key
    /// and where to declare it. (Not under the record switch, which writes
    /// instead.)
    #[test]
    fn check_panics_in_debug_builds() {
        if !cfg!(debug_assertions) || std::env::var_os(RECORD).is_some() {
            return;
        }
        let e = Error::new(Code::LabelNotFound, "x").with("colour", "red");
        let panic = std::panic::catch_unwind(|| check(&e)).unwrap_err();
        let text = panic.downcast_ref::<String>().expect("a formatted panic");
        assert!(
            text.contains("error `label_not_found`: `colour` (str) isn't declared")
                && text.contains("crates/ryu-doc/src/error/details.rs"),
            "{text}"
        );
        let raw = std::panic::catch_unwind(|| check_raw(Code::LabelNotFound, r#"{"label":1}"#));
        assert!(raw.is_err(), "a raw refusal is checked too");
    }

    #[test]
    fn the_record_switch_appends_one_line_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("found.txt");
        let found = problems(Code::LabelNotFound, &json!({"colour": "red", "label": 1}));
        record(&file, Code::LabelNotFound, &found);
        record(&file, Code::LabelNotFound, &found);
        let once = "label_not_found\tcolour\tstr\t-\nlabel_not_found\tlabel\tnum\tstr\n";
        assert_eq!(std::fs::read_to_string(&file).unwrap(), once.repeat(2));
    }
}
