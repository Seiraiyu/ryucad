//! JLC's categories as ryueda's, by the words in them: jlcparts
//! normalizes JLC's own taxonomy (`jlcparts/taxonomy.py`), whose names
//! change over time, so this reads words, the subcategory first, and
//! anything it doesn't know is `other`.

use crate::category::Category;

/// Words in a subcategory that decide alone, in order.
const BY_SUBCATEGORY: [(&str, Category); 9] = [
    ("ferrite", Category::FerriteBead),
    ("light emitting", Category::Led),
    ("led", Category::Led),
    ("fuse", Category::Fuse),
    ("voltage regulator", Category::Regulator),
    ("ldo", Category::Regulator),
    ("dc-dc", Category::Regulator),
    ("tvs", Category::Diode),
    ("crystal", Category::Crystal),
];

/// Words in a category, in order.
const BY_CATEGORY: [(&str, Category); 16] = [
    ("resistor", Category::Resistor),
    ("capacitor", Category::Capacitor),
    ("inductor", Category::Inductor),
    ("diode", Category::Diode),
    ("optoelectronic", Category::Led),
    ("transistor", Category::Transistor),
    ("crystal", Category::Crystal),
    ("oscillator", Category::Crystal),
    ("connector", Category::Connector),
    ("switch", Category::Switch),
    ("power management", Category::Ic),
    ("embedded", Category::Ic),
    ("amplifier", Category::Ic),
    ("interface", Category::Ic),
    ("data converter", Category::Ic),
    ("clock", Category::Ic),
];

pub fn category_of(category: &str, subcategory: &str) -> Category {
    let sub = subcategory.to_lowercase();
    for (w, c) in BY_SUBCATEGORY {
        // Whole words for short ones ("led" in "Molded" isn't an LED).
        let hit = if w.len() <= 3 {
            sub.split(|ch: char| !ch.is_ascii_alphanumeric())
                .any(|x| x == w)
        } else {
            sub.contains(w)
        };
        if hit {
            return c;
        }
    }
    let cat = category.to_lowercase();
    BY_CATEGORY
        .iter()
        .find(|(w, _)| cat.contains(w))
        .map_or(Category::Other, |(_, c)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::Category::*;

    #[test]
    fn jlc_categories_map_onto_ryuedas() {
        for (cat, sub, want) in [
            ("Resistors", "Chip Resistor - Surface Mount", Resistor),
            ("Capacitors", "Multilayer Ceramic Capacitors MLCC - SMD/SMT", Capacitor),
            ("Inductors, Coils and Transformers", "Power Inductors", Inductor),
            ("Filters and EMI Optimization", "Ferrite Beads", FerriteBead),
            ("Diodes", "Schottky Diodes", Diode),
            ("Optoelectronics", "Light Emitting Diodes (LED)", Led),
            ("Transistors and Thyristors", "MOSFETs", Transistor),
            ("Power Management", "Linear Voltage Regulators (LDO)", Regulator),
            ("Power Management", "DC-DC Converters", Regulator),
            ("Power Management", "Battery Management", Ic),
            ("Embedded Processors and Controllers", "Microcontrollers", Ic),
            ("Crystals, Oscillators and Resonators", "Crystals", Crystal),
            ("Connectors", "Pin Headers", Connector),
            ("Switches", "Tactile Switches", Switch),
            ("Circuit Protection", "Fuses", Fuse),
            ("Circuit Protection", "TVS Diodes", Diode),
            ("Audio Products and Motors", "Buzzers", Other),
            ("Plastic", "Molded Parts", Other),
            ("", "", Other),
        ] {
            assert_eq!(category_of(cat, sub), want, "{cat} / {sub}");
        }
    }
}
