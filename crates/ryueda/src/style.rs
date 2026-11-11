//! A schematic's style (design 348, plan 2.E): `style.json` (kind
//! `ryueda.style`, format 1). A project's beside its manifest; an
//! organisation's at the root of the library project the manifest's
//! `schematic.style_from` names, which alone may hold `locks`,
//! `standards` and `lint`. The built-in `ryu` style is the cascade's
//! first layer, written in the same form ([`builtin`]). The cascade
//! (`cascade`) resolves the layers into what ryusch draws.

use crate::error::Code;
use crate::fault::Fault;
use crate::standards::Class;
use ryusch::style::{Rgb, TextSize, Theme, Weight};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const KIND: &str = "ryueda.style";
pub const FORMAT: u64 = 1;
/// The file's name, beside a project's manifest.
pub const FILE: &str = "style.json";

/// A colour's two values, `#rrggbb` each.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub light: String,
    pub dark: String,
}

impl Pair {
    pub fn get(&self, theme: Theme) -> Option<Rgb> {
        Rgb::parse(match theme {
            Theme::Light => &self.light,
            Theme::Dark => &self.dark,
        })
    }

    fn of(light: Rgb, dark: Rgb) -> Pair {
        Pair {
            light: light.hex(),
            dark: dark.hex(),
        }
    }
}

/// A colour as a style writes it: a palette name, or both values (never
/// one colour: every colour must read in both themes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Colour {
    Name(String),
    Pair(Pair),
}

/// What an object shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Visible {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_numbers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<bool>,
}

impl Visible {
    pub fn is_empty(&self) -> bool {
        *self == Visible::default()
    }
}

/// The properties a rule or an override sets; `None` leaves one to the
/// layers under it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Props {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<Colour>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Colour>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_weight: Option<Weight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_size: Option<TextSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emphasis: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<bool>,
    #[serde(default, skip_serializing_if = "Visible::is_empty")]
    pub visible: Visible,
}

/// Every property's name, as locks, `style set` and `style explain` name
/// them.
pub const PROPERTIES: [&str; 9] = [
    "colour",
    "fill",
    "line_weight",
    "text_size",
    "emphasis",
    "dash",
    "visible.pin_numbers",
    "visible.intent",
    "visible.frame",
];

/// The properties that move geometry: a rule naming a sheet or region
/// can't set them.
pub const GEOMETRY: [&str; 3] = ["text_size", "emphasis", "visible.intent"];

fn word<T: serde::de::DeserializeOwned>(v: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(v.to_owned())).ok()
}

fn flag(v: &str) -> Result<bool, String> {
    match v {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{v:?} is true or false")),
    }
}

impl Props {
    pub fn is_empty(&self) -> bool {
        *self == Props::default()
    }

    /// The properties it sets, in [`PROPERTIES`]' order.
    pub fn names(&self) -> Vec<&'static str> {
        let set = [
            self.colour.is_some(),
            self.fill.is_some(),
            self.line_weight.is_some(),
            self.text_size.is_some(),
            self.emphasis.is_some(),
            self.dash.is_some(),
            self.visible.pin_numbers.is_some(),
            self.visible.intent.is_some(),
            self.visible.frame.is_some(),
        ];
        PROPERTIES
            .into_iter()
            .zip(set)
            .filter_map(|(n, s)| s.then_some(n))
            .collect()
    }

    /// `other`'s properties over these, one by one.
    pub fn over(&mut self, other: &Props) {
        fn take<T: Clone>(a: &mut Option<T>, b: &Option<T>) {
            if b.is_some() {
                *a = b.clone();
            }
        }
        take(&mut self.colour, &other.colour);
        take(&mut self.fill, &other.fill);
        take(&mut self.line_weight, &other.line_weight);
        take(&mut self.text_size, &other.text_size);
        take(&mut self.emphasis, &other.emphasis);
        take(&mut self.dash, &other.dash);
        take(&mut self.visible.pin_numbers, &other.visible.pin_numbers);
        take(&mut self.visible.intent, &other.visible.intent);
        take(&mut self.visible.frame, &other.visible.frame);
    }

    /// Takes off property `name`; `false` if it isn't one.
    pub fn unset(&mut self, name: &str) -> bool {
        match name {
            "colour" => self.colour = None,
            "fill" => self.fill = None,
            "line_weight" => self.line_weight = None,
            "text_size" => self.text_size = None,
            "emphasis" => self.emphasis = None,
            "dash" => self.dash = None,
            "visible.pin_numbers" => self.visible.pin_numbers = None,
            "visible.intent" => self.visible.intent = None,
            "visible.frame" => self.visible.frame = None,
            _ => return false,
        }
        true
    }

    /// Sets property `name` from its text, as `style set` takes it: a
    /// colour a palette name or `#light/#dark`, a weight `thin`,
    /// `normal` or `heavy`, a size `small`, `normal` or `large`, a flag
    /// `true` or `false`.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        let colour = |v: &str| -> Result<Colour, String> {
            match v.split_once('/') {
                Some((l, d)) if Rgb::parse(l).is_some() && Rgb::parse(d).is_some() => {
                    Ok(Colour::Pair(Pair {
                        light: l.to_ascii_lowercase(),
                        dark: d.to_ascii_lowercase(),
                    }))
                }
                _ if v.starts_with('#') => Err(format!(
                    "{v:?}: a colour is a palette name or #light/#dark (every colour reads in both themes)"
                )),
                _ => Ok(Colour::Name(v.to_owned())),
            }
        };
        match name {
            "colour" => self.colour = Some(colour(value)?),
            "fill" => self.fill = Some(colour(value)?),
            "line_weight" => {
                self.line_weight =
                    Some(word(value).ok_or_else(|| format!("{value:?} is thin, normal or heavy"))?)
            }
            "text_size" => {
                self.text_size = Some(
                    word(value).ok_or_else(|| format!("{value:?} is small, normal or large"))?,
                )
            }
            "emphasis" => self.emphasis = Some(flag(value)?),
            "dash" => self.dash = Some(flag(value)?),
            "visible.pin_numbers" => self.visible.pin_numbers = Some(flag(value)?),
            "visible.intent" => self.visible.intent = Some(flag(value)?),
            "visible.frame" => self.visible.frame = Some(flag(value)?),
            _ => {
                return Err(format!(
                    "{name} isn't a property: {}",
                    PROPERTIES.join(", ")
                ));
            }
        }
        Ok(())
    }
}

/// The kinds of object a style draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Component,
    Net,
    Rail,
    Port,
    Region,
}

/// What a rule applies to: every key given must match.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Select {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    /// A block's ID or name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    /// The sheet drawn, by ID or name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    /// A region's ID or title: the region and the components in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// A net class's ID or name: nets and rails of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// A rail's name or ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_class: Option<Class>,
    /// `<id>` for the project's own library component, `<alias>/<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libcomp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl Select {
    /// How many keys it gives: its specificity.
    pub fn keys(&self) -> usize {
        [
            self.kind.is_some(),
            self.block.is_some(),
            self.sheet.is_some(),
            self.region.is_some(),
            self.class.is_some(),
            self.rail.is_some(),
            self.component_class.is_some(),
            self.libcomp.is_some(),
            self.tag.is_some(),
        ]
        .into_iter()
        .filter(|x| *x)
        .count()
    }

    /// Whether it names where an object is drawn (a sheet or a region).
    pub fn placed(&self) -> bool {
        self.sheet.is_some() || self.region.is_some()
    }

    /// From `key=value,key=value` (the CLI's selector).
    pub fn parse(text: &str) -> Result<Select, String> {
        let mut m = serde_json::Map::new();
        for pair in text.split(',').filter(|p| !p.is_empty()) {
            let (k, v) = pair
                .split_once('=')
                .ok_or_else(|| format!("{pair:?} isn't key=value"))?;
            m.insert(
                k.trim().to_owned(),
                serde_json::Value::String(v.trim().to_owned()),
            );
        }
        serde_json::from_value(serde_json::Value::Object(m)).map_err(|e| {
            format!(
                "not a selector ({e}): keys are kind, block, sheet, region, class, rail, component_class, libcomp, tag"
            )
        })
    }
}

/// A rule: what it selects and what it sets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub select: Select,
    pub set: Props,
}

/// The title block's fields a style fills (plan 2.E); the design and
/// block, and the sheet's number, are always the page's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleField {
    Title,
    Number,
    Revision,
    Date,
    Organisation,
}

impl TitleField {
    pub const ALL: [TitleField; 5] = [
        TitleField::Title,
        TitleField::Number,
        TitleField::Revision,
        TitleField::Date,
        TitleField::Organisation,
    ];

    /// What it holds where no style says: a property of the same idea.
    pub fn default_template(self) -> &'static str {
        match self {
            TitleField::Title => "{design}",
            TitleField::Number => "{number}",
            TitleField::Revision => "{rev}",
            TitleField::Date => "{date}",
            TitleField::Organisation => "{organisation}",
        }
    }

    /// Its cell's label.
    pub fn label(self) -> &'static str {
        match self {
            TitleField::Title => "Title",
            TitleField::Number => "Number",
            TitleField::Revision => "Rev",
            TitleField::Date => "Date",
            TitleField::Organisation => "Organisation",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TitleField::Title => "title",
            TitleField::Number => "number",
            TitleField::Revision => "revision",
            TitleField::Date => "date",
            TitleField::Organisation => "organisation",
        }
    }
}

/// An organisation's lint preferences, under every project's `lint.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintLayer {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub weights: BTreeMap<String, i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub severity: BTreeMap<String, ryusch::lint::Severity>,
}

impl LintLayer {
    pub fn is_empty(&self) -> bool {
        self.weights.is_empty() && self.severity.is_empty()
    }
}

/// The style file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Style {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub palette: BTreeMap<String, Pair>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    /// By stable ID: `component/<block>/<id>`, `net/<block>/<id>`,
    /// `port/<block>/<id>`, `region/<block>/<region>`, `rail/<id>`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, Props>,
    /// The organisation's: what a project may not set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub locks: Vec<String>,
    /// The organisation's: its library project's `library/standards.json`
    /// judges every library of a project that has none of its own.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub standards: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub title_block: BTreeMap<TitleField, String>,
    /// The organisation's lint weights and severities.
    #[serde(skip_serializing_if = "LintLayer::is_empty")]
    pub lint: LintLayer,
}

/// Which layer a style file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    BuiltIn,
    Organisation,
    Project,
}

/// `~` and `/` escaped for a JSON pointer.
fn esc(s: &str) -> String {
    crate::symbol::pointer_escape(s)
}

/// Whether `n` can name a palette colour.
pub fn palette_name_ok(n: &str) -> bool {
    (1..=32).contains(&n.len())
        && n.as_bytes()[0].is_ascii_lowercase()
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Whether `key` has an override key's shape.
pub fn override_key_ok(key: &str) -> bool {
    let parts: Vec<&str> = key.split('/').collect();
    let ok = |s: &str| !s.is_empty() && s.len() <= 64;
    match parts.as_slice() {
        ["rail", id] => ok(id),
        ["component" | "net" | "port" | "region", b, id] => ok(b) && ok(id),
        _ => false,
    }
}

/// Whether `lock` is one a style can hold.
pub fn lock_ok(lock: &str) -> bool {
    PROPERTIES.contains(&lock)
        || lock == "title_block"
        || lock
            .strip_prefix("title_block.")
            .is_some_and(|f| TitleField::ALL.iter().any(|t| t.as_str() == f))
        || lock.strip_prefix("palette.").is_some_and(palette_name_ok)
        || ["lint.weights.", "lint.severity."]
            .iter()
            .any(|p| lock.strip_prefix(p).is_some_and(|c| !c.is_empty()))
}

impl Style {
    /// What `style_invalid` finds, each with its pointer. `known` says
    /// whether a palette name is any layer's.
    pub fn check(&self, layer: Layer, known: &dyn Fn(&str) -> bool) -> Vec<Fault> {
        let mut out = Vec::new();
        let bad =
            |ptr: String, msg: String| Fault::error(Code::StyleInvalid, msg).with("pointer", ptr);
        for (name, pair) in &self.palette {
            let at = format!("/palette/{}", esc(name));
            if !palette_name_ok(name) {
                out.push(bad(
                    at.clone(),
                    format!(
                        "{name:?} isn't a palette name: a-z first, then a-z, 0-9 and _, 32 at most"
                    ),
                ));
            }
            for (theme, v) in [("light", &pair.light), ("dark", &pair.dark)] {
                if Rgb::parse(v).is_none() {
                    out.push(bad(format!("{at}/{theme}"), format!("{v:?} isn't #rrggbb")));
                }
            }
        }
        let props = |p: &Props, at: &str, out: &mut Vec<Fault>| {
            for (field, c) in [("colour", &p.colour), ("fill", &p.fill)] {
                match c {
                    Some(Colour::Name(n)) if !known(n) && !self.palette.contains_key(n) => out
                        .push(bad(
                            format!("{at}/{field}"),
                            format!("no palette names {n:?}"),
                        )),
                    Some(Colour::Pair(pair)) => {
                        for (theme, v) in [("light", &pair.light), ("dark", &pair.dark)] {
                            if Rgb::parse(v).is_none() {
                                out.push(bad(
                                    format!("{at}/{field}/{theme}"),
                                    format!("{v:?} isn't #rrggbb"),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }
        };
        for (i, r) in self.rules.iter().enumerate() {
            let at = format!("/rules/{i}/set");
            props(&r.set, &at, &mut out);
            if r.select.placed() {
                for g in GEOMETRY {
                    if r.set.names().contains(&g) {
                        out.push(bad(
                            format!("{at}/{}", g.replace('.', "/")),
                            format!(
                                "a rule selecting by sheet or region can't set {g}: where a part is drawn depends on it"
                            ),
                        ));
                    }
                }
            }
        }
        for (key, p) in &self.overrides {
            let at = format!("/overrides/{}", esc(key));
            if !override_key_ok(key) {
                out.push(bad(
                    at.clone(),
                    format!(
                        "{key:?} isn't an override's key: component/<block>/<id>, net/<block>/<id>, port/<block>/<id>, region/<block>/<region>, rail/<id>"
                    ),
                ));
            }
            props(p, &at, &mut out);
        }
        for (field, text) in &self.title_block {
            if let Err(e) = crate::props::Template::parse(text) {
                out.push(bad(
                    format!("/title_block/{}", field.as_str()),
                    format!("the {} field doesn't read: {}", field.as_str(), e.message),
                ));
            }
        }
        if layer == Layer::Project {
            let only = "only an organisation's style (schematic.style_from) holds";
            if !self.locks.is_empty() {
                out.push(bad("/locks".into(), format!("{only} locks")));
            }
            if self.standards {
                out.push(bad("/standards".into(), format!("{only} standards")));
            }
            if !self.lint.is_empty() {
                out.push(bad(
                    "/lint".into(),
                    format!("{only} lint preferences; a project's are lint.json"),
                ));
            }
        } else {
            for (i, l) in self.locks.iter().enumerate() {
                if !lock_ok(l) {
                    out.push(bad(
                        format!("/locks/{i}"),
                        format!("{l:?} isn't a lock: a property, palette.<name>, title_block[.<field>], lint.weights.<code> or lint.severity.<code>"),
                    ));
                }
            }
            let prefs = ryusch::lint::Prefs {
                weights: self.lint.weights.clone(),
                severity: self.lint.severity.clone(),
                ..ryusch::lint::Prefs::default()
            };
            for f in crate::lint::check(&prefs) {
                let ptr = f.error.details["pointer"].as_str().unwrap_or("").to_owned();
                out.push(bad(format!("/lint{ptr}"), f.error.message.clone()));
            }
        }
        out
    }
}

/// The built-in `ryu` style: ryusch's palette, and the rules that give
/// the built-in tags their meaning and show rails' voltages.
pub fn builtin() -> Style {
    let palette = ryusch::style::RYU
        .iter()
        .map(|(n, l, d)| ((*n).to_owned(), Pair::of(*l, *d)))
        .collect();
    let rule = |select: Select, set: Props| Rule { select, set };
    let tag = |t: &str| Select {
        tag: Some(t.to_owned()),
        ..Select::default()
    };
    Style {
        palette,
        rules: vec![
            rule(
                tag("hv"),
                Props {
                    colour: Some(Colour::Name("hv".into())),
                    line_weight: Some(Weight::Heavy),
                    ..Props::default()
                },
            ),
            rule(
                tag("critical"),
                Props {
                    colour: Some(Colour::Name("critical".into())),
                    emphasis: Some(true),
                    ..Props::default()
                },
            ),
            rule(
                tag("dnp"),
                Props {
                    colour: Some(Colour::Name("dim".into())),
                    dash: Some(true),
                    ..Props::default()
                },
            ),
            rule(
                Select {
                    kind: Some(Kind::Rail),
                    ..Select::default()
                },
                Props {
                    visible: Visible {
                        intent: Some(true),
                        ..Visible::default()
                    },
                    ..Props::default()
                },
            ),
        ],
        ..Style::default()
    }
}

/// What an object of `kind` draws `property` in when no layer sets it:
/// its role (a colour or fill), or the default value.
fn default_of(kind: Kind, property: &str) -> Option<(&'static str, serde_json::Value)> {
    use serde_json::json;
    let role = |r: ryusch::style::Role| Some((r.name(), json!(r.name())));
    match (property, kind) {
        ("colour", Kind::Component | Kind::Port) => role(ryusch::style::Role::Ink),
        ("colour", Kind::Net) => role(ryusch::style::Role::Wire),
        ("colour", Kind::Rail) => role(ryusch::style::Role::Power),
        ("colour", Kind::Region) => role(ryusch::style::Role::Frame),
        ("fill", Kind::Component) => role(ryusch::style::Role::Body),
        ("fill", Kind::Port) => role(ryusch::style::Role::PortFill),
        ("line_weight", _) => Some(("", json!("normal"))),
        ("text_size", Kind::Component) => Some(("", json!("normal"))),
        ("emphasis", Kind::Component) | ("dash", _) => Some(("", json!(false))),
        ("visible.pin_numbers", Kind::Component) | ("visible.frame", Kind::Region) => {
            Some(("", json!(true)))
        }
        ("visible.intent", Kind::Net) => Some(("", json!(false))),
        ("visible.intent", Kind::Rail) => Some(("", json!(true))),
        _ => None,
    }
}

/// `style.get`: the project's style, the organisation's (its alias and
/// path), the built-in one, the organisation's locks, and the project's
/// revision.
pub fn get(p: &crate::project::Project) -> crate::error::Result<serde_json::Value> {
    use serde_json::json;
    let own = p.style.item.clone().map_err(|e| e.clone())?;
    let org = match &p.org_style {
        None => serde_json::Value::Null,
        Some(o) => json!({
            "alias": o.alias,
            "path": o.path,
            "style": o.style.item.as_ref().map_err(|e| e.clone())?,
        }),
    };
    let locks = p
        .org_style
        .as_ref()
        .and_then(|o| o.style.item.as_ref().ok())
        .map(|s| s.locks.clone())
        .unwrap_or_default();
    Ok(json!({
        "project": own,
        "organisation": org,
        "builtin": builtin(),
        "locks": locks,
        "revision": p.revision,
    }))
}

/// The object `target` names in `block` (the agent's vocabulary: a
/// designator, a net's name, `rail:NAME`, `port:NAME`, a region's title
/// or ID), as the cascade knows it.
pub fn object_named(
    p: &crate::project::Project,
    block: &str,
    target: &str,
) -> crate::error::Result<(crate::cascade::Object, crate::schem::Built)> {
    let b = crate::write::block(p, block)?;
    let built = crate::schem::build(p, b)?;
    let names = crate::vocab::Names {
        p,
        b,
        built: &built,
    };
    let key = names
        .part(target)
        .map(|k| match k.split_once(':') {
            // A unit names its component.
            Some((c, _))
                if !k.starts_with("port:")
                    && !k.starts_with("region:")
                    && !k.starts_with("instance:") =>
            {
                c.to_owned()
            }
            _ => k,
        })
        .or_else(|_| names.net(target))?;
    let objects = crate::cascade::objects(p, b, &built);
    let o = objects.into_iter().find(|o| o.key == key).ok_or_else(|| {
        crate::error::Error::new(
            Code::NotFound,
            format!("{target} is drawn but takes no style (an instance's symbol)"),
        )
        .with("reference", target)
    })?;
    Ok((o, built))
}

/// `style.explain`: each property `target` is drawn with, its value (a
/// colour's both values too) and the layer, rule or override that set
/// it, or the role or default it falls to; on `sheet` when given.
pub fn explain(
    p: &crate::project::Project,
    block: &str,
    target: &str,
    sheet: Option<&str>,
) -> crate::error::Result<serde_json::Value> {
    use serde_json::json;
    let (o, _) = object_named(p, block, target)?;
    let sheet_of = sheet.map(|s| {
        crate::write::block(p, block)
            .ok()
            .and_then(|b| match &b.layout {
                Some(Ok(l)) => l
                    .sheets
                    .iter()
                    .find(|(k, v)| *k == s || v.name == s)
                    .map(|(k, v)| (k.clone(), v.name.clone())),
                _ => None,
            })
            .unwrap_or((s.to_owned(), s.to_owned()))
    });
    let layers = crate::cascade::Layers::of(p);
    let r = layers.resolve(&o, sheet_of.as_ref().map(|(a, b)| (a.as_str(), b.as_str())));
    let full = serde_json::to_value(&r.props).expect("props serialize");
    let mut props = serde_json::Map::new();
    for name in PROPERTIES {
        let value = match name.split_once('.') {
            Some((a, b)) => full.get(a).and_then(|v| v.get(b)).cloned(),
            None => full.get(name).cloned(),
        };
        let mut entry = match (value, r.from.get(name)) {
            (Some(v), Some(from)) => json!({"value": v, "from": from}),
            _ => match default_of(o.kind, name) {
                Some((role, v)) if !role.is_empty() => {
                    json!({"value": v, "from": {"layer": "built_in", "role": role}})
                }
                Some((_, v)) => json!({"value": v, "from": {"layer": "built_in", "default": true}}),
                None => continue,
            },
        };
        if name == "colour" || name == "fill" {
            let c: Option<Colour> = serde_json::from_value(entry["value"].clone()).ok();
            if let Some(c) = c {
                for theme in [Theme::Light, Theme::Dark] {
                    if let Some(rgb) = layers.colour(&c, theme) {
                        entry[if theme == Theme::Light {
                            "light"
                        } else {
                            "dark"
                        }] = json!(rgb.hex());
                    }
                }
            }
        }
        props.insert(name.to_owned(), entry);
    }
    Ok(json!({
        "object": {"kind": o.kind, "id": o.id, "key": o.key, "tags": o.tags},
        "sheet": sheet_of.map(|(a, _)| a),
        "properties": props,
    }))
}

/// What a style write changes.
pub enum Edit<'a> {
    /// Over the override of `target` (in `block`), or over the rule with
    /// exactly `target`'s selector when it is one (`key=value,…`).
    Set {
        target: &'a str,
        block: Option<&'a str>,
        props: &'a [(String, String)],
    },
    /// Those properties off, likewise; an entry left empty goes.
    Unset {
        target: &'a str,
        block: Option<&'a str>,
        names: &'a [String],
    },
    /// A new rule at the end.
    RuleAdd {
        select: &'a str,
        props: &'a [(String, String)],
    },
    /// The rule at `index` (from 0).
    RuleRemove { index: usize },
    /// A palette entry, `light/dark`; `default` removes it.
    Palette { name: &'a str, value: &'a str },
}

fn usage(param: &str, msg: String) -> crate::error::Error {
    crate::error::Error::new(Code::InvalidParam, msg).with("param", param)
}

/// The override key of what `target` names in `block`.
fn override_key(
    p: &crate::project::Project,
    block: Option<&str>,
    target: &str,
) -> crate::error::Result<String> {
    if let Some(r) = target.strip_prefix("rail:") {
        let (id, _) = crate::topcmd::rail_named(p, r)?;
        return Ok(format!("rail/{id}"));
    }
    let block = block.ok_or_else(|| {
        usage(
            "block",
            "name the block an object is in with --block".into(),
        )
    })?;
    Ok(object_named(p, block, target)?.0.id)
}

fn props_of(pairs: &[(String, String)]) -> crate::error::Result<Props> {
    let mut out = Props::default();
    for (k, v) in pairs {
        out.set(k, v).map_err(|m| usage("set", m))?;
    }
    Ok(out)
}

/// A style write: `edit` applied to this project's style, refused where
/// the organisation locks what it sets (`style_locked`) or where the file
/// would fail its checks (`style_invalid`); written as one logged change
/// guarded by the project's revision, the file deleted when it holds
/// nothing. Blocks whose drawing it moves are laid out again in the same
/// change (`write::commit`'s `keep_current`).
pub fn write(
    session: &mut crate::session::Session,
    edit: Edit,
    expected: Option<&str>,
) -> crate::error::Result<serde_json::Value> {
    let p = session.project();
    crate::server::check_revision("the project", &p.revision, expected)?;
    let mut s = p.style.item.clone().map_err(|e| e.clone())?;
    let layers = crate::cascade::Layers::of(p);
    let locked = |lock: &str| -> crate::error::Result<()> {
        if layers.locked(lock) {
            return Err(crate::error::Error::new(
                Code::StyleLocked,
                format!("the organisation's style locks {lock}"),
            )
            .with("lock", lock)
            .with("path", layers.org_path().unwrap_or_default()));
        }
        Ok(())
    };
    let what;
    match edit {
        Edit::Set {
            target,
            block,
            props,
        } => {
            let set = props_of(props)?;
            for n in set.names() {
                locked(n)?;
            }
            if target.contains('=') {
                let select = Select::parse(target).map_err(|m| usage("target", m))?;
                match s.rules.iter_mut().find(|r| r.select == select) {
                    Some(r) => r.set.over(&set),
                    None => s.rules.push(Rule { select, set }),
                }
                what = format!("style rule {target}");
            } else {
                let key = override_key(p, block, target)?;
                s.overrides.entry(key.clone()).or_default().over(&set);
                what = format!("style {key}");
            }
        }
        Edit::Unset {
            target,
            block,
            names,
        } => {
            let take = |pr: &mut Props| -> crate::error::Result<()> {
                for n in names {
                    if !pr.unset(n) {
                        return Err(usage(
                            "names",
                            format!("{n} isn't a property: {}", PROPERTIES.join(", ")),
                        ));
                    }
                }
                Ok(())
            };
            if target.contains('=') {
                let select = Select::parse(target).map_err(|m| usage("target", m))?;
                let i = s
                    .rules
                    .iter()
                    .position(|r| r.select == select)
                    .ok_or_else(|| {
                        crate::error::Error::new(
                            Code::NotFound,
                            format!("no rule selects {target}"),
                        )
                        .with("reference", target)
                    })?;
                take(&mut s.rules[i].set)?;
                if s.rules[i].set.is_empty() {
                    s.rules.remove(i);
                }
                what = format!("style rule {target}");
            } else {
                let key = override_key(p, block, target)?;
                let pr = s.overrides.get_mut(&key).ok_or_else(|| {
                    crate::error::Error::new(Code::NotFound, format!("{target} has no override"))
                        .with("reference", target)
                })?;
                take(pr)?;
                if pr.is_empty() {
                    s.overrides.remove(&key);
                }
                what = format!("style {key}");
            }
        }
        Edit::RuleAdd { select, props } => {
            let select = Select::parse(select).map_err(|m| usage("select", m))?;
            let set = props_of(props)?;
            for n in set.names() {
                locked(n)?;
            }
            s.rules.push(Rule { select, set });
            what = "style rules".into();
        }
        Edit::RuleRemove { index } => {
            if index >= s.rules.len() {
                return Err(usage(
                    "index",
                    format!(
                        "the style has {} rule(s); rules count from 0",
                        s.rules.len()
                    ),
                ));
            }
            s.rules.remove(index);
            what = "style rules".into();
        }
        Edit::Palette { name, value } => {
            locked(&format!("palette.{name}"))?;
            if value == "default" {
                s.palette.remove(name);
            } else {
                let (l, d) = value
                    .split_once('/')
                    .filter(|(l, d)| Rgb::parse(l).is_some() && Rgb::parse(d).is_some())
                    .ok_or_else(|| {
                        usage("value", format!("{value:?} is #light/#dark, or default"))
                    })?;
                s.palette.insert(
                    name.to_owned(),
                    Pair {
                        light: l.to_ascii_lowercase(),
                        dark: d.to_ascii_lowercase(),
                    },
                );
            }
            what = format!("style palette {name}");
        }
    }
    let builtin = builtin();
    let org = p
        .org_style
        .as_ref()
        .and_then(|o| o.style.item.as_ref().ok());
    let known = |n: &str| {
        builtin.palette.contains_key(n)
            || org.is_some_and(|o| o.palette.contains_key(n))
            || s.palette.contains_key(n)
    };
    let faults: Vec<_> = s
        .check(Layer::Project, &known)
        .into_iter()
        .map(|f| f.with("path", FILE))
        .collect();
    let errors: Vec<_> = faults.into_iter().filter(|f| f.is_error()).collect();
    if !errors.is_empty() {
        return Err(crate::write::refused(FILE, &errors));
    }
    let mut w = crate::write::Writes::default();
    if s == Style::default() {
        w.delete(FILE.to_owned());
    } else {
        w.0.push((
            FILE.to_owned(),
            Some(crate::doc::to_string(&s, KIND, FORMAT)),
        ));
    }
    crate::write::commit(session, &what, w)?;
    get(session.project())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(v: serde_json::Value) -> Style {
        serde_json::from_value(v).unwrap()
    }

    fn known(n: &str) -> bool {
        builtin().palette.contains_key(n) || n == "brand"
    }

    #[test]
    fn a_style_reads_and_writes_canonically() {
        let s = read(json!({
            "palette": {"brand": {"light": "#0055AA", "dark": "#66aaff"}},
            "rules": [{"select": {"tag": "hv", "kind": "net"}, "set": {"colour": "brand", "line_weight": "heavy"}}],
            "overrides": {"component/b1/r1": {"colour": {"light": "#ff0000", "dark": "#ff8080"}, "visible": {"pin_numbers": false}}},
            "title_block": {"title": "{design}", "number": "{number}"}
        }));
        assert_eq!(s.rules[0].select.keys(), 2);
        let text = crate::doc::to_string(&s, KIND, FORMAT);
        let back: Style = crate::doc::read(text.as_bytes(), FILE, KIND, FORMAT).unwrap();
        assert_eq!(back, s);
        assert!(
            s.check(Layer::Project, &known).is_empty(),
            "{:?}",
            s.check(Layer::Project, &known)
        );
        assert!(Style::default().check(Layer::Project, &known).is_empty());
        assert!(builtin().check(Layer::BuiltIn, &known).is_empty());
    }

    #[test]
    fn checks_name_their_pointer() {
        let bad = read(json!({
            "palette": {"Bad Name": {"light": "#000000", "dark": "#fff"}},
            "rules": [
                {"select": {"sheet": "s1"}, "set": {"text_size": "large"}},
                {"select": {}, "set": {"colour": "nowhere"}}
            ],
            "overrides": {"thing/x": {"dash": true}},
            "locks": ["colour"],
            "title_block": {"title": "{unclosed"}
        }));
        let pointers: Vec<String> = bad
            .check(Layer::Project, &known)
            .iter()
            .map(|f| f.error.details["pointer"].as_str().unwrap().to_owned())
            .collect();
        for p in [
            "/palette/Bad Name",
            "/palette/Bad Name/dark",
            "/rules/0/set/text_size",
            "/rules/1/set/colour",
            "/overrides/thing~1x",
            "/locks",
            "/title_block/title",
        ] {
            assert!(pointers.contains(&p.to_owned()), "{p} in {pointers:?}");
        }
        let org = read(
            json!({"locks": ["colour", "palette.wire", "lint.severity.sch_jog", "nope"],
                              "lint": {"severity": {"sch_jog": "error", "sch_nope": "error"}}}),
        );
        let f = org.check(Layer::Organisation, &known);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f.iter().all(|f| f.error.code == Code::StyleInvalid));
    }

    #[test]
    fn properties_set_unset_and_layer_over_each_other() {
        let mut p = Props::default();
        p.set("colour", "hv").unwrap();
        p.set("fill", "#ffffff/#000000").unwrap();
        p.set("line_weight", "heavy").unwrap();
        p.set("visible.pin_numbers", "false").unwrap();
        assert!(
            p.set("colour", "#ff0000").is_err(),
            "a single colour reads in one theme only"
        );
        assert!(p.set("line_weight", "bold").is_err());
        assert!(p.set("nope", "1").is_err());
        assert_eq!(
            p.names(),
            ["colour", "fill", "line_weight", "visible.pin_numbers"]
        );
        let mut base = Props::default();
        base.set("dash", "true").unwrap();
        base.set("colour", "dim").unwrap();
        base.over(&p);
        assert_eq!(base.colour, Some(Colour::Name("hv".into())));
        assert_eq!(base.dash, Some(true));
        assert!(base.unset("dash"));
        assert_eq!(base.dash, None);
        assert!(!base.unset("nope"));
        let s = Select::parse("tag=hv,kind=net").unwrap();
        assert_eq!((s.tag.as_deref(), s.kind), (Some("hv"), Some(Kind::Net)));
        assert!(Select::parse("colour=red").is_err());
    }
}
