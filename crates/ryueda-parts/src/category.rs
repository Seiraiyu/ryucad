//! What kind of part a library component is: a fixed list, written as
//! `category` or inferred from its prefix. Later stages add categories;
//! these never change.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Resistor,
    Capacitor,
    Inductor,
    FerriteBead,
    Diode,
    Led,
    Transistor,
    Regulator,
    Ic,
    Crystal,
    Connector,
    Switch,
    Fuse,
    Other,
}

pub const ALL: [Category; 14] = [
    Category::Resistor,
    Category::Capacitor,
    Category::Inductor,
    Category::FerriteBead,
    Category::Diode,
    Category::Led,
    Category::Transistor,
    Category::Regulator,
    Category::Ic,
    Category::Crystal,
    Category::Connector,
    Category::Switch,
    Category::Fuse,
    Category::Other,
];

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Resistor => "resistor",
            Category::Capacitor => "capacitor",
            Category::Inductor => "inductor",
            Category::FerriteBead => "ferrite_bead",
            Category::Diode => "diode",
            Category::Led => "led",
            Category::Transistor => "transistor",
            Category::Regulator => "regulator",
            Category::Ic => "ic",
            Category::Crystal => "crystal",
            Category::Connector => "connector",
            Category::Switch => "switch",
            Category::Fuse => "fuse",
            Category::Other => "other",
        }
    }

    pub fn parse(name: &str) -> Option<Category> {
        ALL.into_iter().find(|c| c.as_str() == name)
    }

    /// Every category's name, for messages.
    pub fn names() -> Vec<&'static str> {
        ALL.iter().map(|c| c.as_str()).collect()
    }

    /// The category a reference designator's letters imply (design 347):
    /// `R`, `C`, `L`, `FB`, `D`, `Q`, `Y`, `J`, `F`, `SW`, `U`; anything
    /// else is `other`.
    pub fn from_prefix(prefix: &str) -> Category {
        match prefix.to_ascii_uppercase().as_str() {
            "R" => Category::Resistor,
            "C" => Category::Capacitor,
            "L" => Category::Inductor,
            "FB" => Category::FerriteBead,
            "D" => Category::Diode,
            "Q" => Category::Transistor,
            "Y" => Category::Crystal,
            "J" => Category::Connector,
            "F" => Category::Fuse,
            "SW" => Category::Switch,
            "U" => Category::Ic,
            _ => Category::Other,
        }
    }

    /// Categories often generic (a header, a test point): ERC asks them for
    /// a maker part number only when `erc.json` sets `part_unidentified`.
    pub fn often_generic(self) -> bool {
        matches!(self, Category::Connector | Category::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_by_name_and_from_prefixes() {
        for c in ALL {
            assert_eq!(Category::parse(c.as_str()), Some(c));
            assert_eq!(serde_json::to_value(c).unwrap(), c.as_str());
        }
        assert_eq!(Category::parse("valve"), None);
        for (prefix, c) in [
            ("R", Category::Resistor),
            ("C", Category::Capacitor),
            ("L", Category::Inductor),
            ("FB", Category::FerriteBead),
            ("D", Category::Diode),
            ("Q", Category::Transistor),
            ("Y", Category::Crystal),
            ("J", Category::Connector),
            ("F", Category::Fuse),
            ("SW", Category::Switch),
            ("U", Category::Ic),
            ("u", Category::Ic),
            ("TP", Category::Other),
        ] {
            assert_eq!(Category::from_prefix(prefix), c, "{prefix}");
        }
        assert!(Category::Connector.often_generic() && Category::Other.often_generic());
        assert!(!Category::Resistor.often_generic());
    }
}
