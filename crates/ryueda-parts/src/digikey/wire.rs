//! DigiKey's v4 answers, as much of them as ryueda reads: the token, a
//! keyword search's products and a product's details. Every field is
//! optional and may be `null`, and fields ryueda doesn't read are
//! ignored, so DigiKey adding one changes nothing; a shape that doesn't
//! read is `parts_source_refused` naming what serde said.

use crate::error::{Code, Error, Result};
use serde::{Deserialize, Deserializer};

/// A field DigiKey may send as `null`: its default then.
fn nullable<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Token {
    #[serde(default, deserialize_with = "nullable")]
    pub access_token: String,
    /// Seconds (DigiKey gives 599).
    #[serde(default, deserialize_with = "nullable")]
    pub expires_in: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Named {
    #[serde(default, deserialize_with = "nullable")]
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Description {
    #[serde(default, deserialize_with = "nullable")]
    pub product_description: String,
    #[serde(default, deserialize_with = "nullable")]
    pub detailed_description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Status {
    #[serde(default, deserialize_with = "nullable")]
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Parameter {
    #[serde(default, deserialize_with = "nullable")]
    pub parameter_text: String,
    #[serde(default, deserialize_with = "nullable")]
    pub value_text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CategoryNode {
    #[serde(default, deserialize_with = "nullable")]
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub child_categories: Vec<CategoryNode>,
}

impl CategoryNode {
    /// Its names from the deepest first child up to itself.
    pub fn names(&self) -> Vec<&str> {
        let mut out = vec![self.name.as_str()];
        let mut at = self;
        while let Some(c) = at.child_categories.first() {
            out.push(c.name.as_str());
            at = c;
        }
        out.reverse();
        out
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Price {
    #[serde(default, deserialize_with = "nullable")]
    pub break_quantity: u64,
    #[serde(default, deserialize_with = "nullable")]
    pub unit_price: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Variation {
    #[serde(default, deserialize_with = "nullable")]
    pub digi_key_product_number: String,
    #[serde(default, deserialize_with = "nullable")]
    pub standard_pricing: Vec<Price>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Product {
    #[serde(default, deserialize_with = "nullable")]
    pub manufacturer_product_number: String,
    #[serde(default, deserialize_with = "nullable")]
    pub manufacturer: Named,
    #[serde(default, deserialize_with = "nullable")]
    pub description: Description,
    #[serde(default, deserialize_with = "nullable")]
    pub quantity_available: f64,
    #[serde(default, deserialize_with = "nullable")]
    pub datasheet_url: String,
    #[serde(default, deserialize_with = "nullable")]
    pub product_status: Status,
    #[serde(default, deserialize_with = "nullable")]
    pub parameters: Vec<Parameter>,
    #[serde(default, deserialize_with = "nullable")]
    pub category: CategoryNode,
    #[serde(default, deserialize_with = "nullable")]
    pub product_variations: Vec<Variation>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Keyword {
    #[serde(default, deserialize_with = "nullable")]
    pub products: Vec<Product>,
    #[serde(default, deserialize_with = "nullable")]
    pub exact_matches: Vec<Product>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Details {
    #[serde(default, deserialize_with = "nullable")]
    pub product: Product,
}

/// `bytes` as `T`, or `parts_source_refused` saying what didn't read
/// (`what`: the answer's name, `field`: serde's words).
pub fn read<T: for<'de> Deserialize<'de>>(bytes: &[u8], what: &str) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| {
        Error::new(
            Code::PartsSourceRefused,
            format!("DigiKey's {what} answer doesn't read: {e}"),
        )
        .with("source", super::NAME)
        .with("answer", what)
        .with("field", e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn answers_read_with_nulls_missing_and_unknown_fields() {
        let p: Details = read(
            json!({"Product": {
                "ManufacturerProductNumber": "CL05B104KO5NNNC",
                "Manufacturer": {"Id": 1, "Name": "Samsung Electro-Mechanics"},
                "Description": null,
                "QuantityAvailable": 1200.0,
                "Parameters": [{"ParameterId": 2049, "ParameterText": "Capacitance", "ValueText": "0.1 µF"}],
                "Category": {"Name": "Capacitors", "ChildCategories": [{"Name": "Ceramic Capacitors", "ChildCategories": []}]},
                "ProductVariations": [{"DigiKeyProductNumber": "1276-1001-1-ND", "StandardPricing": [{"BreakQuantity": 10, "UnitPrice": 0.1, "TotalPrice": 1.0}]}],
                "NewField": {"anything": true}
            }})
            .to_string()
            .as_bytes(),
            "details",
        )
        .unwrap();
        assert_eq!(p.product.manufacturer.name, "Samsung Electro-Mechanics");
        assert_eq!(p.product.description, Description::default());
        assert_eq!(
            p.product.category.names(),
            ["Ceramic Capacitors", "Capacitors"]
        );
        assert_eq!(
            p.product.product_variations[0].standard_pricing[0].break_quantity,
            10
        );
        let k: Keyword = read(b"{}", "keyword").unwrap();
        assert!(k.products.is_empty());
        let e = read::<Keyword>(br#"{"Products": "many"}"#, "keyword").unwrap_err();
        assert_eq!(
            (e.code, e.details["answer"].as_str()),
            (Code::PartsSourceRefused, Some("keyword"))
        );
        assert!(
            e.details["field"]
                .as_str()
                .unwrap()
                .contains("invalid type")
        );
        let t: Token = read(
            br#"{"access_token":"a","expires_in":599,"token_type":"Bearer"}"#,
            "token",
        )
        .unwrap();
        assert_eq!(t.expires_in, 599);
    }
}
