//! A DigiKey product as a record: its DigiKey part number the key and the
//! one offer, the maker and part number, the description, the category by
//! the words in DigiKey's category names (the deepest first), the
//! vocabulary's parameters, stock, the chosen variation's price breaks in
//! USD, the lifecycle from its status, and the datasheet.

use crate::category::Category;
use crate::record::{Lifecycle, Offer, PriceBreak, Record, squash};

use super::wire::Product;

/// Words in one of DigiKey's category names, in order; short ones whole.
const WORDS: [(&str, Category); 20] = [
    ("ferrite", Category::FerriteBead),
    ("led", Category::Led),
    ("light emitting", Category::Led),
    ("fuse", Category::Fuse),
    ("voltage regulator", Category::Regulator),
    ("ldo", Category::Regulator),
    ("dc dc", Category::Regulator),
    ("crystal", Category::Crystal),
    ("oscillator", Category::Crystal),
    ("resonator", Category::Crystal),
    ("resistor", Category::Resistor),
    ("capacitor", Category::Capacitor),
    ("inductor", Category::Inductor),
    ("diode", Category::Diode),
    ("tvs", Category::Diode),
    ("transistor", Category::Transistor),
    ("fet", Category::Transistor),
    ("connector", Category::Connector),
    ("switch", Category::Switch),
    ("integrated circuit", Category::Ic),
];

fn has(name: &str, word: &str) -> bool {
    let n = name.to_lowercase().replace(['-', '/'], " ");
    if word.len() <= 3 {
        n.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == word || w.strip_suffix('s') == Some(word))
    } else {
        n.contains(word)
    }
}

/// ryueda's category for DigiKey's category names, the deepest first.
pub fn category_of(names: &[&str]) -> Category {
    for n in names {
        if let Some((_, c)) = WORDS.iter().find(|(w, _)| has(n, w)) {
            return *c;
        }
    }
    Category::Other
}

/// A product status's lifecycle; `None` for one ryueda doesn't know
/// (`Preliminary`).
pub fn lifecycle(status: &str) -> Option<Lifecycle> {
    Some(match squash(status).as_str() {
        "active" => Lifecycle::Active,
        "notfornewdesigns" => Lifecycle::NotRecommended,
        "lasttimebuy" => Lifecycle::LastTimeBuy,
        "obsolete" | "discontinuedatdigikey" | "discontinued" => Lifecycle::Obsolete,
        _ => return None,
    })
}

/// A price as decimal text: the float's shortest form.
fn price(x: f64) -> Option<String> {
    (x.is_finite() && x >= 0.0).then(|| format!("{x}"))
}

/// `p` as a record fetched at `fetched_at`: keyed by the variation whose
/// DigiKey part number is `key` (letters and digits), else its first.
/// `None` for a product with no DigiKey part number.
pub fn record(p: &Product, key: Option<&str>, fetched_at: u64) -> Option<Record> {
    let want = key.map(squash);
    let v = p
        .product_variations
        .iter()
        .filter(|v| !v.digi_key_product_number.trim().is_empty())
        .find(|v| {
            want.as_ref()
                .is_none_or(|w| squash(&v.digi_key_product_number) == *w)
        })
        .or_else(|| {
            p.product_variations
                .iter()
                .find(|v| !v.digi_key_product_number.trim().is_empty())
        })?;
    let sku = v.digi_key_product_number.trim();
    let mut r = Record::new(super::NAME, sku);
    r.manufacturer = p.manufacturer.name.trim().to_owned();
    r.mpn = p.manufacturer_product_number.trim().to_owned();
    r.description = p.description.product_description.trim().to_owned();
    r.category = Some(category_of(&p.category.names()));
    for prm in &p.parameters {
        if let Some((name, text)) = super::attr::vocabulary(&prm.parameter_text, &prm.value_text) {
            r.parameters.entry(name.to_owned()).or_insert(text);
        }
    }
    r.offers = vec![Offer {
        supplier: super::SUPPLIER.to_owned(),
        sku: sku.to_owned(),
    }];
    let q = p.quantity_available;
    r.stock = (q.is_finite() && q >= 0.0).then_some(q as u64);
    r.price_breaks = v
        .standard_pricing
        .iter()
        .filter_map(|b| {
            Some(PriceBreak {
                quantity: b.break_quantity,
                price: price(b.unit_price)?,
                currency: "USD".to_owned(),
            })
        })
        .collect();
    r.lifecycle = lifecycle(&p.product_status.status);
    r.datasheet = p.datasheet_url.trim().to_owned();
    r.fetched_at = Some(fetched_at);
    r.attribution = super::ATTRIBUTION.to_owned();
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digikey::testing;

    #[test]
    fn a_product_becomes_a_record() {
        let p = testing::product(testing::CAP);
        let r = record(&p, None, 1_000).unwrap();
        assert_eq!(
            (r.source.as_str(), r.key.as_str()),
            ("digikey", "1276-1001-1-ND")
        );
        assert_eq!(
            (r.manufacturer.as_str(), r.mpn.as_str()),
            ("Samsung Electro-Mechanics", "CL05B104KO5NNNC")
        );
        assert_eq!(r.category, Some(Category::Capacitor));
        assert_eq!(r.parameters["capacitance"], "0.1 uF");
        assert_eq!(r.parameters["dielectric"], "X7R");
        assert_eq!(r.parameters["package"], "0402");
        assert_eq!(
            r.offers,
            [Offer {
                supplier: "DigiKey".into(),
                sku: "1276-1001-1-ND".into()
            }]
        );
        assert_eq!(r.stock, Some(1_200_000));
        assert_eq!(
            r.price_breaks[0],
            PriceBreak {
                quantity: 1,
                price: "0.1".into(),
                currency: "USD".into()
            }
        );
        assert_eq!(r.lifecycle, Some(Lifecycle::Active));
        assert_eq!(
            (r.fetched_at, r.attribution.as_str()),
            (Some(1_000), "Data from DigiKey")
        );
        // Another variation by its part number.
        assert_eq!(
            record(&p, Some("1276-1001-2-nd"), 1).unwrap().key,
            "1276-1001-2-ND"
        );
        assert_eq!(
            record(&p, Some("nothing"), 1).unwrap().key,
            "1276-1001-1-ND"
        );
        assert!(record(&Product::default(), None, 1).is_none());
    }

    #[test]
    fn categories_and_lifecycles_read_digikeys_words() {
        assert_eq!(
            category_of(&["Ceramic Capacitors", "Capacitors"]),
            Category::Capacitor
        );
        assert_eq!(
            category_of(&["Ferrite Beads and Chips", "Filters"]),
            Category::FerriteBead
        );
        assert_eq!(
            category_of(&["Single FETs, MOSFETs", "Discrete Semiconductor Products"]),
            Category::Transistor
        );
        assert_eq!(
            category_of(&[
                "Voltage Regulators - Linear, Low Drop Out (LDO) Regulators",
                "Power Management (PMIC)"
            ]),
            Category::Regulator
        );
        assert_eq!(
            category_of(&["LED Indication - Discrete", "Optoelectronics"]),
            Category::Led
        );
        assert_eq!(
            category_of(&["Molded", "Boxes"]),
            Category::Other,
            "led in Molded isn't an LED"
        );
        assert_eq!(
            category_of(&["Microcontrollers", "Embedded", "Integrated Circuits (ICs)"]),
            Category::Ic
        );
        assert_eq!(
            lifecycle("Not For New Designs"),
            Some(Lifecycle::NotRecommended)
        );
        assert_eq!(lifecycle("Last Time Buy"), Some(Lifecycle::LastTimeBuy));
        assert_eq!(
            lifecycle("Discontinued at Digi-Key"),
            Some(Lifecycle::Obsolete)
        );
        assert_eq!(lifecycle("Obsolete"), Some(Lifecycle::Obsolete));
        assert_eq!(lifecycle("Preliminary"), None);
    }
}
