//! The look a sheet is drawn in (design 348, plan 2.E), once ryueda's
//! cascade has resolved it: a colour for each role in one theme, and each
//! object's own pen. Nothing here reads a file. Geometry a style moves (a
//! part's text size and emphasis, a net's intent) is the engine's input
//! (`input::Part::text`, `input::Net::intent`), never a look, so layouts
//! don't depend on the theme.

use crate::font::{MIN_SIZE, SIZE};
use crate::geom::Nm;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// From `#rrggbb`, either case.
    pub fn parse(text: &str) -> Option<Rgb> {
        let h = text.strip_prefix('#')?;
        if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(Rgb(c(0)?, c(2)?, c(4)?))
    }

    /// As `#rrggbb`, lower case.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// Which of each colour's two values a drawing takes.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

/// What a colour is for: a palette name every look has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    Paper,
    Canvas,
    /// Symbols' lines.
    Ink,
    /// Symbols' filled bodies.
    Body,
    Wire,
    /// Net labels, off-sheet connectors and not-connected marks.
    Label,
    /// Power ports, ground and their intent.
    Power,
    Text,
    PinNumber,
    /// A block port's flag.
    PortFill,
    /// A region's frame.
    Frame,
    /// A sheet's name on the canvas, and a note's box.
    Name,
}

impl Role {
    pub const ALL: [Role; 12] = [
        Role::Paper,
        Role::Canvas,
        Role::Ink,
        Role::Body,
        Role::Wire,
        Role::Label,
        Role::Power,
        Role::Text,
        Role::PinNumber,
        Role::PortFill,
        Role::Frame,
        Role::Name,
    ];

    /// Its palette name.
    pub fn name(self) -> &'static str {
        match self {
            Role::Paper => "paper",
            Role::Canvas => "canvas",
            Role::Ink => "ink",
            Role::Body => "body",
            Role::Wire => "wire",
            Role::Label => "label",
            Role::Power => "power",
            Role::Text => "text",
            Role::PinNumber => "pin_number",
            Role::PortFill => "port_fill",
            Role::Frame => "frame",
            Role::Name => "name",
        }
    }

    pub fn parse(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.name() == name)
    }
}

/// The built-in `ryu` palette (design 348's figure 4): graphite bodies
/// with a warm fill, blue wires, red power; each name's light and dark
/// colour. The twelve roles, then the colours the built-in style's tag
/// rules use. Every colour drawn on paper is at least 3:1 against it in
/// its theme, and every colour drawn over a body or a port's flag at
/// least 3:1 against that fill (ryueda's `sch_contrast` holds it so).
pub const RYU: [(&str, Rgb, Rgb); 15] = [
    ("paper", Rgb(0xff, 0xff, 0xff), Rgb(0x1d, 0x20, 0x25)),
    ("canvas", Rgb(0xe6, 0xe8, 0xeb), Rgb(0x12, 0x14, 0x18)),
    ("ink", Rgb(0x3a, 0x3f, 0x47), Rgb(0xc3, 0xc9, 0xd2)),
    ("body", Rgb(0xfd, 0xf3, 0xe1), Rgb(0x2f, 0x2a, 0x22)),
    ("wire", Rgb(0x1f, 0x5f, 0xbf), Rgb(0x6f, 0xa6, 0xff)),
    ("label", Rgb(0x1d, 0x3f, 0x8f), Rgb(0x9c, 0xbc, 0xff)),
    ("power", Rgb(0xb8, 0x32, 0x2a), Rgb(0xff, 0x7a, 0x6e)),
    ("text", Rgb(0x20, 0x23, 0x28), Rgb(0xe8, 0xea, 0xed)),
    ("pin_number", Rgb(0x5b, 0x61, 0x6b), Rgb(0xa3, 0xaa, 0xb4)),
    ("port_fill", Rgb(0xdd, 0xe7, 0xf6), Rgb(0x26, 0x34, 0x4d)),
    ("frame", Rgb(0x8c, 0x93, 0x9d), Rgb(0x7d, 0x85, 0x8f)),
    ("name", Rgb(0x5f, 0x66, 0x70), Rgb(0xa0, 0xa7, 0xb1)),
    ("hv", Rgb(0xc0, 0x26, 0x1c), Rgb(0xff, 0x6a, 0x5c)),
    ("critical", Rgb(0xa8, 0x56, 0x0f), Rgb(0xf0, 0xa2, 0x54)),
    ("dim", Rgb(0x86, 0x8c, 0x95), Rgb(0x7b, 0x82, 0x8c)),
];

/// The built-in palette's colour `name` in `theme`.
pub fn builtin(name: &str, theme: Theme) -> Option<Rgb> {
    RYU.iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, l, d)| match theme {
            Theme::Light => *l,
            Theme::Dark => *d,
        })
}

/// How heavy a line is.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Weight {
    Thin,
    #[default]
    Normal,
    Heavy,
}

impl Weight {
    /// Its width: 0.1, 0.15 (every line before plan 2.E) or 0.3 mm.
    pub fn width(self) -> Nm {
        match self {
            Weight::Thin => 100_000,
            Weight::Normal => crate::paint::LINE,
            Weight::Heavy => 2 * crate::paint::LINE,
        }
    }
}

/// How large a part's designator and value are.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    /// 1 mm, the smallest drawn.
    Small,
    /// 50 mil.
    #[default]
    Normal,
    /// 70 mil.
    Large,
}

impl TextSize {
    pub fn nm(self) -> Nm {
        match self {
            TextSize::Small => MIN_SIZE,
            TextSize::Normal => SIZE,
            TextSize::Large => 1_778_000,
        }
    }
}

/// One object's own look, as the cascade resolved it: a colour or fill
/// of `None` draws in the role's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pen {
    pub colour: Option<Rgb>,
    pub fill: Option<Rgb>,
    pub weight: Weight,
    /// Its polylines dashed (2 mm on, 1 mm off).
    pub dash: bool,
    /// A component's value in SemiBold (paint reads it; the engine
    /// measures it from `input::Part::text`).
    pub emphasis: bool,
    /// A component's pin numbers drawn.
    pub pin_numbers: bool,
    /// A region's frame drawn (its title always is).
    pub frame: bool,
}

impl Default for Pen {
    fn default() -> Pen {
        Pen {
            colour: None,
            fill: None,
            weight: Weight::Normal,
            dash: false,
            emphasis: false,
            pin_numbers: true,
            frame: true,
        }
    }
}

/// What a sheet is drawn in: its theme, each role's colour, and the pens
/// of the objects a style touches, by layout key (a part's key, a net's,
/// `bus:<id>`, `region:<id>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub theme: Theme,
    pub roles: BTreeMap<Role, Rgb>,
    pub pens: BTreeMap<String, Pen>,
}

impl Look {
    /// The built-in `ryu` look in `theme`, with no pens.
    pub fn builtin(theme: Theme) -> Look {
        Look {
            theme,
            roles: Role::ALL
                .into_iter()
                .map(|r| (r, builtin(r.name(), theme).expect("every role is built in")))
                .collect(),
            pens: BTreeMap::new(),
        }
    }

    pub fn role(&self, r: Role) -> Rgb {
        self.roles
            .get(&r)
            .copied()
            .or_else(|| builtin(r.name(), self.theme))
            .expect("every role is built in")
    }

    /// The pen of the object keyed `key`, the default where none is set.
    pub fn pen(&self, key: &str) -> Pen {
        self.pens.get(key).copied().unwrap_or_default()
    }
}

impl Default for Look {
    fn default() -> Look {
        Look::builtin(Theme::Light)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_read_and_write_as_hex() {
        assert_eq!(Rgb::parse("#1F5fbf"), Some(Rgb(0x1f, 0x5f, 0xbf)));
        assert_eq!(Rgb(0x1f, 0x5f, 0xbf).hex(), "#1f5fbf");
        for bad in ["1f5fbf", "#1f5fb", "#1f5fbfa", "#gg0000", ""] {
            assert_eq!(Rgb::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn every_role_is_built_in_in_both_themes() {
        for theme in [Theme::Light, Theme::Dark] {
            let look = Look::builtin(theme);
            for r in Role::ALL {
                assert_eq!(Some(look.role(r)), builtin(r.name(), theme));
                assert_eq!(Role::parse(r.name()), Some(r));
            }
        }
        assert_eq!(Look::default().role(Role::Wire), Rgb(0x1f, 0x5f, 0xbf));
        assert_eq!(
            Look::builtin(Theme::Dark).role(Role::Paper),
            Rgb(0x1d, 0x20, 0x25)
        );
        assert_eq!(builtin("hv", Theme::Light), Some(Rgb(0xc0, 0x26, 0x1c)));
        assert_eq!(builtin("nope", Theme::Light), None);
        let names: Vec<&str> = RYU.iter().map(|(n, _, _)| *n).collect();
        let mut sorted = names.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "names are unique");
    }

    #[test]
    fn a_pen_left_alone_draws_as_the_roles_do() {
        let p = Look::default().pen("r1");
        assert_eq!(p, Pen::default());
        assert_eq!(
            (p.colour, p.weight, p.pin_numbers, p.frame),
            (None, Weight::Normal, true, true)
        );
        assert_eq!(Weight::Heavy.width(), 2 * crate::paint::LINE);
        assert_eq!(TextSize::Large.nm(), 1_778_000);
        assert_eq!(TextSize::Small.nm(), crate::font::MIN_SIZE);
    }
}
