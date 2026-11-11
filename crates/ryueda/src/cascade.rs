//! The style cascade (design 348, plan 2.E): the built-in `ryu` style,
//! the organisation's, the project's, rules by selector and overrides by
//! stable ID, the most specific winning; resolved per object into
//! properties with where each came from (`style explain`), and per sheet
//! into the look ryusch draws ([`look`]).

use crate::project::Project;
use crate::style::{Colour, Kind, Layer, Props, Select, Style};
use ryusch::style::{Look, Pen, Rgb, Role, Theme, Weight};
use serde::Serialize;
use std::collections::BTreeMap;

/// An object a style can name, with the attributes selectors read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    pub kind: Kind,
    /// Its layout key: a part's (a component's every unit shares its
    /// component's ID), a net's, `region:<id>`.
    pub key: String,
    /// Its override key (`component/<block>/<id>`, …).
    pub id: String,
    /// Its block's ID and name.
    pub block: (String, String),
    /// The regions it is in, by ID and title (a region names itself).
    pub regions: Vec<(String, String)>,
    /// A net's or rail's class, by ID and name.
    pub class: Option<(String, String)>,
    /// A rail's ID and name.
    pub rail: Option<(String, String)>,
    pub component_class: Option<crate::standards::Class>,
    /// `<id>` or `<alias>/<id>`.
    pub libcomp: Option<String>,
    pub tags: std::collections::BTreeSet<String>,
}

/// Whether `s` selects `o`, drawn on `sheet` (its ID and name; `None`
/// when no sheet is meant: a rule naming one then selects nothing).
pub fn matches(s: &Select, o: &Object, sheet: Option<(&str, &str)>) -> bool {
    let pair = |want: &Option<String>, have: Option<(&str, &str)>| match want {
        None => true,
        Some(w) => have.is_some_and(|(id, name)| w == id || w == name),
    };
    s.kind.is_none_or(|k| k == o.kind)
        && pair(&s.block, Some((&o.block.0, &o.block.1)))
        && pair(&s.sheet, sheet)
        && s.region
            .as_ref()
            .is_none_or(|w| o.regions.iter().any(|(id, t)| w == id || w == t))
        && pair(
            &s.class,
            o.class.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
        )
        && pair(
            &s.rail,
            o.rail.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
        )
        && s.component_class
            .is_none_or(|c| o.component_class == Some(c))
        && s.libcomp
            .as_ref()
            .is_none_or(|l| o.libcomp.as_deref() == Some(l))
        && s.tag.as_ref().is_none_or(|t| o.tags.contains(t))
}

/// Where a property's value came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Origin {
    pub layer: Layer,
    /// The file (`style.json`, the organisation's path; none built in).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The rule's index in its file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select: Option<Select>,
    /// The override's key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub over: Option<String>,
}

/// An object's resolved properties and where each came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolved {
    pub props: Props,
    pub from: BTreeMap<&'static str, Origin>,
}

/// The style layers of a project, ready to resolve.
pub struct Layers<'a> {
    builtin: Style,
    org: Option<(String, &'a Style)>,
    own: &'a Style,
}

impl<'a> Layers<'a> {
    /// The built-in style under `org` (its file's path and style) and the
    /// project's `own`.
    pub fn new(org: Option<(String, &'a Style)>, own: &'a Style) -> Layers<'a> {
        Layers {
            builtin: crate::style::builtin(),
            org,
            own,
        }
    }

    /// A project's layers: a style that doesn't read is left out (its
    /// fault is the project's).
    pub fn of(p: &'a Project) -> Layers<'a> {
        let own = p.style.item.as_ref().unwrap_or(&EMPTY_STYLE);
        let org = p
            .org_style
            .as_ref()
            .and_then(|o| o.style.item.as_ref().ok().map(|s| (o.path.clone(), s)));
        Layers::new(org, own)
    }

    fn locks(&self) -> &[String] {
        self.org.as_ref().map_or(&[], |(_, s)| s.locks.as_slice())
    }

    pub fn locked(&self, lock: &str) -> bool {
        self.locks().iter().any(|l| l == lock)
    }

    /// The organisation's file, for `style_locked`'s details.
    pub fn org_path(&self) -> Option<&str> {
        self.org.as_ref().map(|(p, _)| p.as_str())
    }

    /// Each lock the project's style breaks, with the pointer where.
    pub fn broken_locks(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for lock in self.locks() {
            if let Some(name) = lock.strip_prefix("palette.")
                && self.own.palette.contains_key(name)
            {
                out.push((lock.clone(), format!("/palette/{name}")));
            }
            if crate::style::PROPERTIES.contains(&lock.as_str()) {
                for (i, r) in self.own.rules.iter().enumerate() {
                    if r.set.names().contains(&lock.as_str()) {
                        out.push((lock.clone(), format!("/rules/{i}/set")));
                    }
                }
                for (k, o) in &self.own.overrides {
                    if o.names().contains(&lock.as_str()) {
                        out.push((
                            lock.clone(),
                            format!("/overrides/{}", crate::symbol::pointer_escape(k)),
                        ));
                    }
                }
            }
            if lock == "title_block" && !self.own.title_block.is_empty() {
                out.push((lock.clone(), "/title_block".into()));
            }
            if let Some(f) = lock.strip_prefix("title_block.")
                && self.own.title_block.keys().any(|t| t.as_str() == f)
            {
                out.push((lock.clone(), format!("/title_block/{f}")));
            }
        }
        out
    }

    /// Props `p` less what the organisation locks (the project's layer).
    fn unlocked(&self, p: &Props) -> Props {
        let mut out = p.clone();
        for name in p.names() {
            if self.locked(name) {
                out.unset(name);
            }
        }
        out
    }

    /// `o`'s properties drawn on `sheet` (`None`: wherever it is drawn,
    /// which rules naming a sheet or region don't reach).
    pub fn resolve(&self, o: &Object, sheet: Option<(&str, &str)>) -> Resolved {
        // Every rule that selects it: (specificity, layer, index, …).
        let mut hits: Vec<(usize, Layer, usize, Option<&str>, &crate::style::Rule)> = Vec::new();
        let files: [(Layer, Option<&str>, &Style); 3] = [
            (Layer::BuiltIn, None, &self.builtin),
            (
                Layer::Organisation,
                self.org.as_ref().map(|(p, _)| p.as_str()),
                self.org.as_ref().map_or(&EMPTY_STYLE, |(_, s)| *s),
            ),
            (Layer::Project, Some(crate::style::FILE), self.own),
        ];
        for (layer, file, style) in files {
            for (i, r) in style.rules.iter().enumerate() {
                let reach = if sheet.is_none() && r.select.placed() {
                    false
                } else {
                    matches(&r.select, o, sheet)
                };
                if reach {
                    hits.push((r.select.keys(), layer, i, file, r));
                }
            }
        }
        hits.sort_by_key(|(k, layer, i, _, _)| (*k, *layer, *i));
        let mut out = Resolved::default();
        let mut take = |p: &Props, from: Origin| {
            for name in p.names() {
                out.from.insert(name, from.clone());
            }
            out.props.over(p);
        };
        for (_, layer, i, file, r) in hits {
            let set = if layer == Layer::Project {
                self.unlocked(&r.set)
            } else {
                r.set.clone()
            };
            take(
                &set,
                Origin {
                    layer,
                    file: file.map(str::to_owned),
                    rule: Some(i),
                    select: Some(r.select.clone()),
                    over: None,
                },
            );
        }
        for (layer, file, style) in [
            (
                Layer::Organisation,
                self.org.as_ref().map(|(p, _)| p.as_str()),
                self.org.as_ref().map(|(_, s)| *s),
            ),
            (Layer::Project, Some(crate::style::FILE), Some(self.own)),
        ] {
            let Some(p) = style.and_then(|s| s.overrides.get(&o.id)) else {
                continue;
            };
            let set = if layer == Layer::Project {
                self.unlocked(p)
            } else {
                p.clone()
            };
            take(
                &set,
                Origin {
                    layer,
                    file: file.map(str::to_owned),
                    rule: None,
                    select: None,
                    over: Some(o.id.clone()),
                },
            );
        }
        out
    }

    /// Palette name `name`'s entry: the project's (unless locked), else
    /// the organisation's, else the built-in one.
    pub fn palette(&self, name: &str) -> Option<&crate::style::Pair> {
        let own = (!self.locked(&format!("palette.{name}")))
            .then(|| self.own.palette.get(name))
            .flatten();
        own.or_else(|| self.org.as_ref().and_then(|(_, s)| s.palette.get(name)))
            .or_else(|| self.builtin.palette.get(name))
    }

    /// A style's colour in `theme`.
    pub fn colour(&self, c: &Colour, theme: Theme) -> Option<Rgb> {
        match c {
            Colour::Name(n) => self.palette(n)?.get(theme),
            Colour::Pair(p) => p.get(theme),
        }
    }

    /// Each role's colour in `theme`.
    pub fn roles(&self, theme: Theme) -> BTreeMap<Role, Rgb> {
        Role::ALL
            .into_iter()
            .filter_map(|r| Some((r, self.colour(&Colour::Name(r.name().into()), theme)?)))
            .collect()
    }

    /// The pen resolved properties draw with in `theme`.
    pub fn pen(&self, r: &Props, theme: Theme) -> Pen {
        let d = Pen::default();
        Pen {
            colour: r.colour.as_ref().and_then(|c| self.colour(c, theme)),
            fill: r.fill.as_ref().and_then(|c| self.colour(c, theme)),
            weight: r.line_weight.unwrap_or(Weight::Normal),
            dash: r.dash.unwrap_or(d.dash),
            emphasis: r.emphasis.unwrap_or(d.emphasis),
            pin_numbers: r.visible.pin_numbers.unwrap_or(d.pin_numbers),
            frame: r.visible.frame.unwrap_or(d.frame),
        }
    }

    /// The title block's field `f`'s template: the project's (unless
    /// locked), else the organisation's, else the default.
    pub fn title_field(&self, f: crate::style::TitleField) -> String {
        let locked =
            self.locked("title_block") || self.locked(&format!("title_block.{}", f.as_str()));
        let own = (!locked).then(|| self.own.title_block.get(&f)).flatten();
        own.or_else(|| self.org.as_ref().and_then(|(_, s)| s.title_block.get(&f)))
            .cloned()
            .unwrap_or_else(|| f.default_template().to_owned())
    }
}

static EMPTY_STYLE: Style = Style {
    palette: BTreeMap::new(),
    rules: Vec::new(),
    overrides: BTreeMap::new(),
    locks: Vec::new(),
    standards: false,
    title_block: BTreeMap::new(),
    lint: crate::style::LintLayer {
        weights: BTreeMap::new(),
        severity: BTreeMap::new(),
    },
};

/// A block's objects as a style names them: each component (with every
/// unit's part key), net, rail drawn in it, port and region.
pub fn objects(
    p: &Project,
    b: &crate::design::BlockFiles,
    built: &crate::schem::Built,
) -> Vec<Object> {
    use crate::schem::Of;
    let block = (
        b.id.to_string(),
        b.block
            .item
            .as_ref()
            .map_or_else(|_| b.id.to_string(), |x| x.name.clone()),
    );
    let regions = |key: &str| -> Vec<(String, String)> {
        match &b.layout {
            Some(Ok(l)) => l
                .regions
                .iter()
                .filter(|(_, r)| {
                    r.parts
                        .iter()
                        .any(|k| k == key || k.split(':').next() == Some(key))
                })
                .map(|(id, r)| (id.clone(), r.title.clone()))
                .collect(),
            _ => Vec::new(),
        }
    };
    let class_of = |id: &Option<crate::id::Id>| {
        id.as_ref().map(|c| {
            let name = p
                .conn
                .classes
                .get(c)
                .and_then(|x| x.item.as_ref().ok())
                .map_or_else(|| c.to_string(), |x| x.name.clone());
            (c.to_string(), name)
        })
    };
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for of in &built.of {
        match of {
            Of::Unit { component, .. } if seen.insert(component.clone()) => {
                let Some(Ok(c)) = b.components.get(component).map(|c| c.item.as_ref()) else {
                    continue;
                };
                let (cc, libcomp) = match crate::partcmd::placed(p, &c.of) {
                    Ok((lc, sym)) => (
                        sym.class
                            .or_else(|| crate::standards::Class::infer(&lc.prefix)),
                        Some(match &c.of.library {
                            Some(a) => format!("{a}/{}", c.of.id),
                            None => c.of.id.to_string(),
                        }),
                    ),
                    Err(_) => (None, None),
                };
                out.push(Object {
                    kind: Kind::Component,
                    key: component.to_string(),
                    id: format!("component/{}/{component}", b.id),
                    block: block.clone(),
                    regions: regions(component.as_str()),
                    class: None,
                    rail: None,
                    component_class: cc,
                    libcomp,
                    tags: c.tags.clone(),
                });
            }
            Of::Port(pid) => out.push(Object {
                kind: Kind::Port,
                key: format!("port:{pid}"),
                id: format!("port/{}/{pid}", b.id),
                block: block.clone(),
                regions: Vec::new(),
                class: None,
                rail: None,
                component_class: None,
                libcomp: None,
                tags: Default::default(),
            }),
            _ => {}
        }
    }
    for net in &built.input.nets {
        if let Some(rid) = net.key.strip_prefix("rail:") {
            let Some((id, Ok(r))) = p
                .conn
                .rails
                .iter()
                .find(|(k, _)| k.as_str() == rid)
                .map(|(k, r)| (k, r.item.as_ref()))
            else {
                continue;
            };
            out.push(Object {
                kind: Kind::Rail,
                key: net.key.clone(),
                id: format!("rail/{id}"),
                block: block.clone(),
                regions: Vec::new(),
                class: class_of(&r.class),
                rail: Some((id.to_string(), r.name.clone())),
                component_class: None,
                libcomp: None,
                tags: r.tags.clone(),
            });
        } else if !net.key.starts_with("port:")
            && let Some((id, Ok(n))) = b
                .nets
                .iter()
                .find(|(k, _)| k.as_str() == net.key)
                .map(|(k, n)| (k, n.item.as_ref()))
        {
            out.push(Object {
                kind: Kind::Net,
                key: net.key.clone(),
                id: format!("net/{}/{id}", b.id),
                block: block.clone(),
                regions: Vec::new(),
                class: class_of(&n.class),
                rail: None,
                component_class: None,
                libcomp: None,
                tags: n.tags.clone(),
            });
        }
    }
    if let Some(Ok(l)) = &b.layout {
        for (rid, r) in &l.regions {
            out.push(Object {
                kind: Kind::Region,
                key: format!("region:{rid}"),
                id: format!("region/{}/{rid}", b.id),
                block: block.clone(),
                regions: vec![(rid.clone(), r.title.clone())],
                class: None,
                rail: None,
                component_class: None,
                libcomp: None,
                tags: Default::default(),
            });
        }
    }
    out
}

/// The look sheet `sheet` (its ID and name) of a block is drawn in, in
/// `theme`: every role, and a pen for each object a rule or override
/// touches (keyed by every part key a component draws).
pub fn look(
    layers: &Layers,
    objects: &[Object],
    built: &crate::schem::Built,
    sheet: (&str, &str),
    theme: Theme,
) -> Look {
    let mut look = Look::builtin(theme);
    look.roles = layers.roles(theme);
    for o in objects {
        let r = layers.resolve(o, Some(sheet));
        if r.props.is_empty() {
            continue;
        }
        let pen = layers.pen(&r.props, theme);
        if o.kind == Kind::Component {
            for part in &built.input.parts {
                if part.key == o.key || part.key.split(':').next() == Some(o.key.as_str()) {
                    look.pens.insert(part.key.clone(), pen);
                }
            }
        } else {
            look.pens.insert(o.key.clone(), pen);
        }
    }
    look
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{Colour, Kind, Props, Rule, Select, Style};
    use ryusch::style::{Rgb, Theme, Weight};

    fn object(kind: Kind, tags: &[&str]) -> Object {
        Object {
            kind,
            key: "r1".into(),
            id: "component/b1/r1".into(),
            block: ("b1".into(), "main".into()),
            regions: vec![],
            class: None,
            rail: None,
            component_class: None,
            libcomp: None,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        }
    }

    fn rule(select: Select, set: Props) -> Rule {
        Rule { select, set }
    }

    #[test]
    fn the_most_specific_rule_wins_then_the_nearer_layer_then_the_later() {
        let mut org = Style::default();
        org.rules.push(rule(
            Select {
                tag: Some("hv".into()),
                kind: Some(Kind::Component),
                ..Select::default()
            },
            Props {
                line_weight: Some(Weight::Thin),
                ..Props::default()
            },
        ));
        let mut own = Style::default();
        own.rules.push(rule(
            Select {
                tag: Some("hv".into()),
                ..Select::default()
            },
            Props {
                line_weight: Some(Weight::Normal),
                dash: Some(true),
                ..Props::default()
            },
        ));
        let layers = Layers::new(Some(("org/style.json".into(), &org)), &own);
        let o = object(Kind::Component, &["hv"]);
        let r = layers.resolve(&o, None);
        // Two keys (the organisation's) beat one (the project's and the
        // built-in's).
        assert_eq!(r.props.line_weight, Some(Weight::Thin));
        assert_eq!(
            r.from["line_weight"].layer,
            crate::style::Layer::Organisation
        );
        // The built-in hv rule's colour, the project's dash.
        assert_eq!(r.props.colour, Some(Colour::Name("hv".into())));
        assert_eq!(r.from["colour"].layer, crate::style::Layer::BuiltIn);
        assert_eq!(r.props.dash, Some(true));
        // An override beats every rule.
        own.overrides.insert(
            "component/b1/r1".into(),
            Props {
                line_weight: Some(Weight::Heavy),
                ..Props::default()
            },
        );
        let layers = Layers::new(Some(("org/style.json".into(), &org)), &own);
        let r = layers.resolve(&o, None);
        assert_eq!(r.props.line_weight, Some(Weight::Heavy));
        assert_eq!(r.from["line_weight"].rule, None);
        assert_eq!(
            r.from["line_weight"].over.as_deref(),
            Some("component/b1/r1")
        );
    }

    #[test]
    fn locks_keep_the_project_from_what_the_organisation_holds() {
        let org = Style {
            locks: vec!["colour".into(), "palette.wire".into()],
            ..Style::default()
        };
        let mut own = Style::default();
        own.rules.push(rule(
            Select::default(),
            Props {
                colour: Some(Colour::Name("hv".into())),
                ..Props::default()
            },
        ));
        own.palette.insert(
            "wire".into(),
            crate::style::Pair {
                light: "#000000".into(),
                dark: "#ffffff".into(),
            },
        );
        let layers = Layers::new(Some(("org/style.json".into(), &org)), &own);
        let r = layers.resolve(&object(Kind::Net, &[]), None);
        assert_eq!(r.props.colour, None, "the project's colour is locked out");
        assert_eq!(
            layers.colour(&Colour::Name("wire".into()), Theme::Light),
            Some(Rgb(0x1f, 0x5f, 0xbf))
        );
        let broken: Vec<String> = layers.broken_locks().into_iter().map(|(l, _)| l).collect();
        assert_eq!(broken, ["colour", "palette.wire"]);
    }

    #[test]
    fn selectors_match_by_every_key_and_sheets_only_when_drawn_there() {
        let mut o = object(Kind::Component, &["dnp"]);
        o.component_class = Some(crate::standards::Class::Resistor);
        o.regions = vec![("r2".into(), "Power".into())];
        let sel = |s: Select| matches(&s, &o, Some(("s1", "Sheet 1")));
        assert!(sel(Select {
            component_class: Some(crate::standards::Class::Resistor),
            tag: Some("dnp".into()),
            ..Select::default()
        }));
        assert!(!sel(Select {
            kind: Some(Kind::Net),
            ..Select::default()
        }));
        assert!(sel(Select {
            region: Some("Power".into()),
            ..Select::default()
        }));
        assert!(sel(Select {
            block: Some("main".into()),
            sheet: Some("s1".into()),
            ..Select::default()
        }));
        assert!(!matches(
            &Select {
                sheet: Some("s1".into()),
                ..Select::default()
            },
            &o,
            None
        ));
        assert!(!sel(Select {
            class: Some("fast".into()),
            ..Select::default()
        }));
    }
}
