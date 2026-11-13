//! The look's own lint (design 348, plan 2.E): `sch_contrast` (a colour
//! under 3:1 against what it is drawn on, in either theme) and
//! `sch_override_unused` (an override naming an object that is gone).
//! Judged once a project, never in a sheet's score: a palette colour is
//! one choice however many sheets draw it.

use crate::cascade::Layers;
use crate::error::Code;
use crate::fault::Fault;
use crate::project::Project;
use crate::style::{Kind, Style};
use ryusch::lint::{Prefs, Rule, Severity};
use ryusch::style::{Rgb, Role, Theme};
use serde_json::{Value, json};

pub const RULES: [Rule; 2] = [
    Rule {
        code: "sch_contrast",
        weight: 2,
        unit: "a colour and theme",
        what: "A colour the style draws with stands under 3:1 against the paper, a symbol's body or a port's flag it is drawn on, in the light or the dark theme.",
        why: "A line or text that barely stands off its background is missed by the eye, worst on a projector, a printout or a dark screen; a theme that reads in one mode and not the other fails half its readers.",
        fix: "Choose a darker (light theme) or lighter (dark theme) value for the palette entry or the rule's colour named (style palette set NAME #light/#dark, style set).",
        evidence: "WCAG 2.x's 3:1 for graphics and large text (success criterion 1.4.11); the built-in ryu palette passes it in both themes (computed while planning plan 2.E). A convention. In round 3's blind review (plan 2.G, three judges a pair), every sheet used the built-in look and the rule found nothing; the judges' text score rose or held on every circuit.",
    },
    Rule {
        code: "sch_override_unused",
        weight: 1,
        unit: "an override",
        what: "An override in style.json names a component, net, port, region or rail that is gone.",
        why: "It does nothing, and when an object later takes the ID it styles something no one meant.",
        fix: "Remove it (style unset TARGET …), or edit style.json's overrides.",
        evidence: "Design 348: overrides follow an object by stable ID, so one whose object is deleted stays behind. A housekeeping rule; no round has judged it.",
    },
];

pub fn rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

/// WCAG 2.x relative luminance.
fn luminance(c: Rgb) -> f64 {
    let f = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ryu_math::pow((s + 0.055) / 1.055, 2.4)
        }
    };
    0.2126 * f(c.0) + 0.7152 * f(c.1) + 0.0722 * f(c.2)
}

/// WCAG 2.x contrast ratio, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x >= y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// The least ratio a colour must keep.
pub const MIN_RATIO: f64 = 3.0;

/// A finding before weights and severities.
#[derive(Clone, Debug, PartialEq)]
pub struct Raw {
    pub code: &'static str,
    pub message: String,
    pub details: Value,
}

/// The roles drawn on paper, over a symbol's body, over a port's flag.
const ON_PAPER: [Role; 11] = [
    Role::Ink,
    Role::Wire,
    Role::Label,
    Role::Power,
    Role::Text,
    Role::PinNumber,
    Role::Frame,
    Role::Name,
    Role::DiffAdded,
    Role::DiffChanged,
    Role::DiffRemoved,
];
const ON_BODY: [Role; 3] = [Role::Ink, Role::Text, Role::PinNumber];
const ON_PORT: [Role; 2] = [Role::Ink, Role::Text];

pub fn contrast_findings(layers: &Layers) -> Vec<Raw> {
    let mut out: Vec<Raw> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for theme in [Theme::Light, Theme::Dark] {
        let roles = layers.roles(theme);
        let bg = |r: Role| {
            roles
                .get(&r)
                .copied()
                .unwrap_or_else(|| ryusch::style::Look::builtin(theme).role(r))
        };
        let word = if theme == Theme::Light {
            "light"
        } else {
            "dark"
        };
        // `c` drawn on `b` (the colour of `against`), found once a place,
        // background and theme.
        let mut judge = |c: Rgb, b: Rgb, against: &str, at: String| {
            let ratio = contrast(c, b);
            if ratio < MIN_RATIO && seen.insert((at.clone(), against.to_owned(), theme)) {
                out.push(Raw {
                    code: "sch_contrast",
                    message: format!(
                        "{at} ({}) stands {ratio:.2}:1 against the {against} ({}) in the {word} theme; 3:1 at least",
                        c.hex(),
                        b.hex()
                    ),
                    details: json!({"where": at, "colour": c.hex(), "against": against,
                                    "background": b.hex(), "theme": word, "ratio": (ratio * 100.0).round() / 100.0}),
                });
            }
        };
        for (list, against) in [
            (&ON_PAPER[..], Role::Paper),
            (&ON_BODY[..], Role::Body),
            (&ON_PORT[..], Role::PortFill),
        ] {
            for r in list {
                judge(
                    bg(*r),
                    bg(against),
                    against.name(),
                    format!("palette.{}", r.name()),
                );
            }
        }
        for (file, style) in layers.files() {
            let mut each = |props: &crate::style::Props, at: String, on_body: bool| {
                if let Some(c) = props.colour.as_ref().and_then(|c| layers.colour(c, theme)) {
                    judge(
                        c,
                        bg(Role::Paper),
                        Role::Paper.name(),
                        format!("{at}/colour"),
                    );
                    if on_body {
                        judge(c, bg(Role::Body), Role::Body.name(), format!("{at}/colour"));
                    }
                }
                if let Some(f) = props.fill.as_ref().and_then(|c| layers.colour(c, theme)) {
                    // Text is drawn over a fill.
                    judge(f, bg(Role::Text), Role::Text.name(), format!("{at}/fill"));
                }
            };
            for (i, r) in style.rules.iter().enumerate() {
                let on_body = r.select.kind.is_none_or(|k| k == Kind::Component);
                each(&r.set, format!("{file}#/rules/{i}/set"), on_body);
            }
            for (k, o) in &style.overrides {
                each(
                    o,
                    format!("{file}#/overrides/{k}"),
                    k.starts_with("component/"),
                );
            }
        }
    }
    out
}

/// Each project override naming nothing.
pub fn unused_overrides(p: &Project, own: &Style) -> Vec<Raw> {
    let mut out = Vec::new();
    for key in own.overrides.keys() {
        let parts: Vec<&str> = key.split('/').collect();
        let block = |b: &str| {
            p.conn
                .blocks
                .iter()
                .find(|(id, _)| id.as_str() == b)
                .map(|(_, f)| f)
        };
        let there = match parts.as_slice() {
            ["rail", id] => p.conn.rails.keys().any(|k| k.as_str() == *id),
            ["component", b, id] => {
                block(b).is_some_and(|f| f.components.keys().any(|k| k.as_str() == *id))
            }
            ["net", b, id] => block(b).is_some_and(|f| f.nets.keys().any(|k| k.as_str() == *id)),
            ["port", b, id] => block(b)
                .and_then(|f| f.block.item.as_ref().ok())
                .is_some_and(|x| x.ports.keys().any(|k| k.as_str() == *id)),
            ["region", b, id] => block(b)
                .is_some_and(|f| matches!(&f.layout, Some(Ok(l)) if l.regions.contains_key(*id))),
            _ => true,
        };
        if !there {
            out.push(Raw {
                code: "sch_override_unused",
                message: format!("style.json's override {key} names nothing this project has"),
                details: json!({"override": key}),
            });
        }
    }
    out
}

/// A finding as `style` and `check` answer it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub message: String,
    pub details: Value,
    pub points: i64,
    pub severity: Severity,
}

/// The project's look's findings under `prefs` (`off` drops a rule).
pub fn findings(p: &Project, prefs: &Prefs) -> Vec<Finding> {
    let layers = Layers::of(p);
    let mut raw = contrast_findings(&layers);
    if let Ok(own) = &p.style.item {
        raw.extend(unused_overrides(p, own));
    }
    raw.into_iter()
        .filter_map(|r| {
            let (severity, _) = prefs.severity(r.code);
            if severity == Severity::Off {
                return None;
            }
            let points = prefs
                .weights
                .get(r.code)
                .copied()
                .unwrap_or_else(|| rule(r.code).map_or(0, |x| x.weight));
            Some(Finding {
                code: r.code,
                message: r.message,
                details: r.details,
                points,
                severity,
            })
        })
        .collect()
}

/// The findings as faults, for `check`.
pub fn faults(p: &Project, prefs: &Prefs) -> Vec<Fault> {
    findings(p, prefs)
        .into_iter()
        .map(|f| {
            let code: Code =
                serde_json::from_value(json!(f.code)).expect("style lint codes are codes");
            let fault = match f.severity {
                Severity::Error => Fault::error(code, f.message),
                _ => Fault::warning(code, f.message),
            };
            fault
                .with("path", crate::style::FILE)
                .with("details", &f.details)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryusch::style::Theme;

    #[test]
    fn contrast_is_wcags() {
        assert!((contrast(Rgb(0, 0, 0), Rgb(255, 255, 255)) - 21.0).abs() < 1e-9);
        assert!((contrast(Rgb(255, 255, 255), Rgb(255, 255, 255)) - 1.0).abs() < 1e-12);
        let wire = ryusch::style::builtin("wire", Theme::Light).unwrap();
        assert!((contrast(wire, Rgb(255, 255, 255)) - 6.09).abs() < 0.01);
    }

    #[test]
    fn the_built_in_look_passes_its_own_contrast() {
        let own = crate::style::Style::default();
        let layers = Layers::new(None, &own);
        assert_eq!(contrast_findings(&layers), Vec::<Raw>::new());
    }

    #[test]
    fn a_pale_colour_and_a_gone_object_are_found() {
        let mut own = crate::style::Style::default();
        own.palette.insert(
            "wire".into(),
            crate::style::Pair {
                light: "#f0f0f0".into(),
                dark: "#202326".into(),
            },
        );
        let layers = Layers::new(None, &own);
        let found = contrast_findings(&layers);
        let wheres: Vec<(String, String)> = found
            .iter()
            .map(|r| {
                (
                    r.details["where"].as_str().unwrap().to_owned(),
                    r.details["theme"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert!(
            wheres.contains(&("palette.wire".into(), "light".into())),
            "{wheres:?}"
        );
        assert!(wheres.contains(&("palette.wire".into(), "dark".into())));
    }
}
