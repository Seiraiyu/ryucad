//! A project's PDM settings, `ryupdm.json` beside the manifest: the
//! revision scheme, the release branch, the part number pattern and the
//! custom property schema. The file is absent until the first PDM write
//! that needs it, and every field has a default.
//!
//! ```json
//! {
//!   "approvals": {"rules": [{"count": 1, "from": ["alice", "bob"], "parts": ["*"]}]},
//!   "format": 2,
//!   "kind": "ryupdm.config",
//!   "part_number": { "pattern": "\\d{3}-\\d{4}" },
//!   "properties": {
//!     "finish": { "type": "enum", "values": ["anodized", "bare"], "required": true, "revising": true }
//!   },
//!   "release_branch": "main",
//!   "revisions": "letters"
//! }
//! ```
//!
//! The file is written at the lowest format holding its keys: 1, or 2
//! with `approvals` (design 261). A key a newer format added is refused in
//! a file that declares an older one.

use crate::approvals::Approvals;
use ryucad_doc::doc::files::{Obj, header, invalid};
use ryucad_doc::doc::project::PDM_CONFIG;
use ryucad_doc::doc::source::Source;
use ryucad_doc::error::{Code, Error, Result};
use ryucad_doc::units::Dim;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const KIND: &str = "ryupdm.config";
/// The newest format this build reads: 2 added `approvals`.
pub const FORMAT: u64 = 2;

/// How a part's revisions are labelled. Labels are always derived from the
/// scheme, never typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scheme {
    /// A, B, …, Y, AA, …, skipping I, O, Q, S, X and Z (ASME Y14.35).
    #[default]
    Letters,
    /// 1, 2, 3, …
    Numeric,
    /// 01, 02, …, 99, 100, …
    Numeric2,
}

impl Scheme {
    pub const ALL: [Scheme; 3] = [Scheme::Letters, Scheme::Numeric, Scheme::Numeric2];

    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Letters => "letters",
            Scheme::Numeric => "numeric",
            Scheme::Numeric2 => "numeric2",
        }
    }

    pub fn parse(s: &str) -> Option<Scheme> {
        Scheme::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// What a property holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    /// One line of text.
    String,
    /// Text that may span lines.
    Text,
    /// A whole number.
    Integer,
    /// A plain (dimensionless) number.
    Number,
    /// A quantity with units, as ryucad writes them (`2.5 mm`, `4 mm^2`),
    /// of one dimension.
    Quantity(Dim),
    Boolean,
    /// One of the listed strings.
    Enum(Vec<String>),
}

impl Type {
    pub fn name(&self) -> &'static str {
        match self {
            Type::String => "string",
            Type::Text => "text",
            Type::Integer => "integer",
            Type::Number => "number",
            Type::Quantity(_) => "quantity",
            Type::Boolean => "boolean",
            Type::Enum(_) => "enum",
        }
    }
}

/// A property's declaration: its type, whether a release needs it, and
/// whether a release pins it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decl {
    pub ty: Type,
    pub required: bool,
    pub revising: bool,
}

impl Decl {
    /// The declaration as `ryupdm.json` and `props.get` write it.
    pub fn to_json(&self) -> Value {
        let mut v = json!({"type": self.ty.name(), "required": self.required,
                           "revising": self.revising});
        match &self.ty {
            Type::Enum(values) => v["values"] = json!(values),
            Type::Quantity(dim) => v["dim"] = json!(dim.name()),
            _ => {}
        }
        v
    }

    /// Reads a declaration of a custom property at `at`.
    pub fn from_json(v: &Value, at: &str) -> Result<Decl> {
        let mut o = Obj::new(v, at)?;
        let ty = o.take("type")?;
        let ty = match ty.as_str() {
            Some("string") => Type::String,
            Some("text") => Type::Text,
            Some("integer") => Type::Integer,
            Some("number") => Type::Number,
            Some("boolean") => Type::Boolean,
            Some("quantity") => {
                let dim = o.take("dim")?;
                Type::Quantity(dim.as_str().and_then(Dim::parse).ok_or_else(|| {
                    invalid(
                        &format!("{at}/dim"),
                        "dim must be a dimension as ryucad names it: length, angle, length^2, length^3, number, …",
                    )
                })?)
            }
            Some("enum") => {
                let values = o.take("values")?;
                let list: Option<Vec<String>> = values
                    .as_array()
                    .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_owned)).collect());
                let list = list
                    .filter(|l| !l.is_empty() && l.iter().all(|s| !s.is_empty()))
                    .ok_or_else(|| {
                        invalid(
                            &format!("{at}/values"),
                            "values must be a non-empty array of non-empty strings",
                        )
                    })?;
                let mut sorted = list.clone();
                sorted.sort();
                sorted.dedup();
                if sorted.len() != list.len() {
                    return Err(invalid(&format!("{at}/values"), "values must be unique"));
                }
                Type::Enum(list)
            }
            _ => {
                return Err(invalid(
                    &format!("{at}/type"),
                    "type must be string, text, integer, number, quantity, boolean or enum",
                ));
            }
        };
        let flag = |o: &mut Obj, key: &str, default: bool| -> Result<bool> {
            match o.take_opt(key) {
                None => Ok(default),
                Some(Value::Bool(b)) => Ok(b),
                Some(_) => Err(invalid(
                    &format!("{at}/{key}"),
                    &format!("{key} must be true or false"),
                )),
            }
        };
        let required = flag(&mut o, "required", false)?;
        let revising = flag(&mut o, "revising", true)?;
        o.finish()?;
        Ok(Decl {
            ty,
            required,
            revising,
        })
    }
}

/// The properties ryupdm understands, which a project can't redeclare.
pub const CORE: [&str; 7] = [
    "number",
    "name",
    "description",
    "material",
    "uom",
    "make_buy",
    "amount",
];

/// Units of measure a part may be counted in.
pub const UOMS: [&str; 6] = ["each", "m", "m2", "m3", "kg", "l"];

/// Names no property may take: the file's own keys.
const RESERVED: [&str; 3] = ["format", "kind", "conflicts"];

/// The declaration of a core property.
pub fn core(name: &str) -> Option<Decl> {
    let decl = |ty, required, revising| Decl {
        ty,
        required,
        revising,
    };
    let choices = |l: &[&str]| Type::Enum(l.iter().map(|s| (*s).to_owned()).collect());
    Some(match name {
        "number" => decl(Type::String, true, true),
        "name" => decl(Type::Text, false, false),
        "description" => decl(Type::Text, false, false),
        "material" => decl(Type::String, false, true),
        "uom" => decl(choices(&UOMS), false, true),
        "make_buy" => decl(choices(&["make", "buy"]), false, false),
        // How much of the part's `uom` one occurrence uses: checked against
        // the `uom` by `props::check`, needed to release one not counted
        // `each`.
        "amount" => decl(Type::String, false, true),
        _ => return None,
    })
}

/// Checks a custom property's name: like a parameter's,
/// `[a-z_][a-z0-9_]{0,62}`, and neither a core property nor one of the
/// file's own keys.
pub fn property_name(name: &str, at: &str) -> Result<()> {
    let b = name.as_bytes();
    let ok = b
        .first()
        .is_some_and(|c| c.is_ascii_lowercase() || *c == b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
        && b.len() <= 63;
    if !ok || RESERVED.contains(&name) {
        return Err(Error::new(
            Code::PropertyInvalid,
            format!(
                "bad property name {name:?}: use 1 to 63 of a-z, 0-9 and _, starting with a letter or _"
            ),
        )
        .with("property", name)
        .with("pointer", at));
    }
    if CORE.contains(&name) {
        return Err(Error::new(
            Code::PropertyInvalid,
            format!("{name} is one of ryupdm's core properties and can't be declared"),
        )
        .with("property", name)
        .with("pointer", at));
    }
    Ok(())
}

/// A part number pattern: matched against the whole number.
#[derive(Clone, Debug)]
pub struct Pattern {
    text: String,
    re: regex_lite::Regex,
}

impl PartialEq for Pattern {
    fn eq(&self, o: &Pattern) -> bool {
        self.text == o.text
    }
}

impl Eq for Pattern {}

impl Pattern {
    /// Compiles `text`, anchored at both ends.
    pub fn new(text: &str) -> Result<Pattern> {
        let re = regex_lite::Regex::new(&format!("^(?:{text})$")).map_err(|e| {
            Error::new(
                Code::InvalidParam,
                format!("bad part number pattern {text:?}: {e}"),
            )
            .with("pattern", text)
        })?;
        Ok(Pattern {
            text: text.to_owned(),
            re,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn matches(&self, number: &str) -> bool {
        self.re.is_match(number)
    }
}

/// `ryupdm.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub revisions: Scheme,
    /// The branch whose history makes release records official.
    pub release_branch: String,
    pub pattern: Option<Pattern>,
    /// The custom properties, by name.
    pub properties: BTreeMap<String, Decl>,
    /// Who must approve releases, and the keys trusted to say they did
    /// (format 2). `None`: nothing is required.
    pub approvals: Option<Approvals>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            revisions: Scheme::Letters,
            release_branch: "main".to_owned(),
            pattern: None,
            properties: BTreeMap::new(),
            approvals: None,
        }
    }
}

impl Config {
    /// The declaration of `name`: core or custom.
    pub fn decl(&self, name: &str) -> Option<Decl> {
        core(name).or_else(|| self.properties.get(name).cloned())
    }

    /// Every property, core first in their fixed order, then the custom
    /// ones by name.
    pub fn decls(&self) -> Vec<(String, Decl)> {
        CORE.iter()
            .map(|n| ((*n).to_owned(), core(n).expect("a core property")))
            .chain(self.properties.iter().map(|(n, d)| (n.clone(), d.clone())))
            .collect()
    }

    /// The lowest format that holds these settings.
    pub fn format(&self) -> u64 {
        if self.approvals.is_some() { 2 } else { 1 }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "format": self.format(),
            "kind": KIND,
            "release_branch": self.release_branch,
            "revisions": self.revisions.as_str(),
        });
        if let Some(p) = &self.pattern {
            v["part_number"] = json!({"pattern": p.text()});
        }
        if !self.properties.is_empty() {
            let props: Map<String, Value> = self
                .properties
                .iter()
                .map(|(n, d)| (n.clone(), d.to_json()))
                .collect();
            v["properties"] = Value::Object(props);
        }
        if let Some(a) = &self.approvals {
            v["approvals"] = a.to_json();
        }
        v
    }

    pub fn from_json(v: &Value) -> Result<Config> {
        let mut o = header(v, KIND, FORMAT)?;
        let mut c = Config::default();
        // `header` has checked the format is a whole number up to FORMAT.
        let declared = v["format"].as_u64().unwrap_or(1);
        if let Some(r) = o.take_opt("revisions") {
            c.revisions = r.as_str().and_then(Scheme::parse).ok_or_else(|| {
                invalid(
                    "/revisions",
                    "revisions must be letters, numeric or numeric2",
                )
            })?;
        }
        if let Some(b) = o.take_opt("release_branch") {
            c.release_branch = b
                .as_str()
                .filter(|b| branch_name(b))
                .ok_or_else(|| invalid("/release_branch", "release_branch must be a branch name"))?
                .to_owned();
        }
        if let Some(p) = o.take_opt("part_number") {
            let mut po = Obj::new(&p, "/part_number")?;
            let text = po.take("pattern")?;
            let text = text
                .as_str()
                .ok_or_else(|| invalid("/part_number/pattern", "pattern must be a string"))?;
            c.pattern =
                Some(Pattern::new(text).map_err(|e| invalid("/part_number/pattern", &e.message))?);
            po.finish()?;
        }
        if let Some(props) = o.take_opt("properties") {
            let map = props
                .as_object()
                .ok_or_else(|| invalid("/properties", "properties must be an object"))?;
            for (name, d) in map {
                let at = format!("/properties/{name}");
                property_name(name, &at)?;
                c.properties.insert(name.clone(), Decl::from_json(d, &at)?);
            }
        }
        if let Some(a) = o.take_opt("approvals") {
            if declared < 2 {
                return Err(invalid(
                    "/approvals",
                    "approvals needs format 2: write the file at format 2 (pdm configure does)",
                ));
            }
            c.approvals = Some(Approvals::from_json(&a, "/approvals")?);
        }
        o.finish()?;
        Ok(c)
    }

    /// The project's settings: the defaults when it has no `ryupdm.json`.
    pub fn read(src: &dyn Source) -> Result<Config> {
        match crate::read_json(src, PDM_CONFIG)? {
            None => Ok(Config::default()),
            Some(v) => Config::from_json(&v).map_err(|e| e.with("path", PDM_CONFIG)),
        }
    }
}

/// Whether Git accepts `b` as a branch name (`git check-ref-format
/// --branch`), for the names a project would use.
pub fn branch_name(b: &str) -> bool {
    !b.is_empty()
        && !b.starts_with(['-', '/', '.'])
        && !b.ends_with(['/', '.'])
        && !b.ends_with(".lock")
        && !b.contains("..")
        && !b.contains("//")
        && !b.contains("@{")
        && !b.contains("/.")
        && b != "@"
        && b.bytes().all(|c| {
            c > b' ' && c != 0x7f && !matches!(c, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Value {
        json!({
            "format": 1,
            "kind": "ryupdm.config",
            "part_number": {"pattern": "\\d{3}-\\d{4}"},
            "properties": {
                "finish": {"type": "enum", "values": ["anodized", "bare", "painted"],
                           "required": true, "revising": true},
                "mass": {"type": "quantity", "dim": "length^3", "required": false,
                         "revising": true},
                "supplier_ref": {"type": "string", "required": false, "revising": false}
            },
            "release_branch": "release/v1",
            "revisions": "numeric2"
        })
    }

    #[test]
    fn round_trips() {
        let c = Config::from_json(&full()).unwrap();
        assert_eq!(c.revisions, Scheme::Numeric2);
        assert_eq!(c.release_branch, "release/v1");
        assert!(c.pattern.as_ref().unwrap().matches("100-0042"));
        assert_eq!(
            c.properties["mass"].ty,
            Type::Quantity(Dim::parse("length^3").unwrap())
        );
        assert_eq!(c.to_json(), full());
        assert_eq!(Config::from_json(&c.to_json()).unwrap(), c);
    }

    #[test]
    fn everything_has_a_default() {
        let c = Config::from_json(&json!({"format": 1, "kind": "ryupdm.config"})).unwrap();
        assert_eq!(c, Config::default());
        assert_eq!(
            c.to_json(),
            json!({"format": 1, "kind": "ryupdm.config", "release_branch": "main",
                   "revisions": "letters"})
        );
        // Flags default to optional and revising.
        let d = Decl::from_json(&json!({"type": "text"}), "/p").unwrap();
        assert_eq!((d.required, d.revising), (false, true));
        assert_eq!(
            Config::read(&BTreeMap::<String, Vec<u8>>::new()).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn patterns_match_the_whole_number() {
        let p = Pattern::new("\\d{3}-\\d{4}").unwrap();
        assert!(p.matches("100-0042"));
        assert!(!p.matches("100-0042x"));
        assert!(!p.matches("x100-0042"));
        // Digits are ASCII digits.
        assert!(!p.matches("١٠٠-٠٠٤٢"));
        let alt = Pattern::new("A|B").unwrap();
        assert!(alt.matches("A") && !alt.matches("AB"));
        assert_eq!(Pattern::new("(").unwrap_err().code, Code::InvalidParam);
    }

    #[test]
    fn faults_point_at_their_field() {
        let cases = [
            (
                "/revisions",
                json!({"revisions": "roman"}),
                Code::InvalidDocument,
            ),
            (
                "/release_branch",
                json!({"release_branch": "a..b"}),
                Code::InvalidDocument,
            ),
            (
                "/part_number/pattern",
                json!({"part_number": {"pattern": "("}}),
                Code::InvalidDocument,
            ),
            (
                "/part_number/x",
                json!({"part_number": {"pattern": "x", "x": 1}}),
                Code::InvalidDocument,
            ),
            (
                "/properties/number",
                json!({"properties": {"number": {"type": "string"}}}),
                Code::PropertyInvalid,
            ),
            (
                "/properties/Finish",
                json!({"properties": {"Finish": {"type": "string"}}}),
                Code::PropertyInvalid,
            ),
            (
                "/properties/kind",
                json!({"properties": {"kind": {"type": "string"}}}),
                Code::PropertyInvalid,
            ),
            (
                "/properties/f/type",
                json!({"properties": {"f": {"type": "colour"}}}),
                Code::InvalidDocument,
            ),
            (
                "/properties/f/values",
                json!({"properties": {"f": {"type": "enum", "values": []}}}),
                Code::InvalidDocument,
            ),
            (
                "/properties/f/values",
                json!({"properties": {"f": {"type": "enum", "values": ["a", "a"]}}}),
                Code::InvalidDocument,
            ),
            (
                "/properties/f/dim",
                json!({"properties": {"f": {"type": "quantity", "dim": "mass"}}}),
                Code::InvalidDocument,
            ),
            (
                "/properties/f/required",
                json!({"properties": {"f": {"type": "text", "required": 1}}}),
                Code::InvalidDocument,
            ),
            (
                "/properties/f/x",
                json!({"properties": {"f": {"type": "text", "x": 1}}}),
                Code::InvalidDocument,
            ),
            ("/extra", json!({"extra": 1}), Code::InvalidDocument),
        ];
        for (pointer, fields, code) in cases {
            let mut v = json!({"format": 1, "kind": "ryupdm.config"});
            for (k, x) in fields.as_object().unwrap() {
                v[k] = x.clone();
            }
            let e = Config::from_json(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{v}"
            );
        }
        let newer = json!({"format": 3, "kind": "ryupdm.config"});
        assert_eq!(
            Config::from_json(&newer).unwrap_err().code,
            Code::FormatTooNew
        );
    }

    #[test]
    fn approvals_need_format_2_and_write_it() {
        let rules = json!({"rules": [{"count": 1, "from": ["alice", "bob"], "parts": ["*"]}]});
        let v = json!({"approvals": rules, "format": 2, "kind": "ryupdm.config",
                       "release_branch": "main", "revisions": "letters"});
        let c = Config::from_json(&v).unwrap();
        assert_eq!(c.approvals.as_ref().unwrap().rules[0].count, 1);
        assert_eq!(c.format(), 2);
        assert_eq!(c.to_json(), v);
        // Without approvals it stays at format 1, whatever was read.
        let mut plain = c.clone();
        plain.approvals = None;
        assert_eq!(plain.to_json()["format"], 1);
        let read2 = Config::from_json(&json!({"format": 2, "kind": "ryupdm.config"})).unwrap();
        assert_eq!(read2.to_json()["format"], 1);
        // A format 1 file can't hold them.
        let mut old = v.clone();
        old["format"] = json!(1);
        let e = Config::from_json(&old).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidDocument, Some("/approvals"))
        );
        // Their faults are approval_rule_invalid.
        let mut bad = v;
        bad["approvals"]["rules"][0]["count"] = json!(3);
        let e = Config::from_json(&bad).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::ApprovalRuleInvalid, Some("/approvals/rules/0/count"))
        );
    }

    #[test]
    fn branch_names_follow_git() {
        for ok in ["main", "release/v1", "dev-2", "a.b"] {
            assert!(branch_name(ok), "{ok}");
        }
        for bad in [
            "", "-x", "a b", "a..b", "a//b", "x.lock", "a~1", "x/", ".x", "a/.b", "@", "a@{1}",
        ] {
            assert!(!branch_name(bad), "{bad}");
        }
    }

    #[test]
    fn core_properties_come_first() {
        let c = Config::from_json(&full()).unwrap();
        let names: Vec<String> = c.decls().into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            [
                "number",
                "name",
                "description",
                "material",
                "uom",
                "make_buy",
                "amount",
                "finish",
                "mass",
                "supplier_ref"
            ]
        );
        assert!(c.decl("number").unwrap().required);
        assert!(!c.decl("description").unwrap().revising);
        assert_eq!(c.decl("nothing"), None);
    }
}
