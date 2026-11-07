//! DigiKey's answers as we write them, in its v4 shape (no data of
//! DigiKey's is committed), and a fake API over them, for this crate's,
//! ryueda's and the fuzz target's tests.

use super::wire;
use crate::distributor::{Clock, Opened};
use crate::fetch::{Fake, Method, Response};
use crate::keys::{Keys, Origin};
use crate::rate::Limiter;
use serde_json::{Value, json};
use std::sync::Arc;

pub const BASE: &str = "https://dk.test";
/// Keys no one has: a `+` and a `&` so a form's encoding shows.
pub const CLIENT_ID: &str = "test-id";
pub const CLIENT_SECRET: &str = "te+st&secret";

/// Which of our two products.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixture {
    Cap,
    Ldo,
}
pub const CAP: Fixture = Fixture::Cap;
pub const LDO: Fixture = Fixture::Ldo;

/// A product's JSON as DigiKey's v4 writes it.
pub fn product_json(f: Fixture) -> Value {
    match f {
        Fixture::Cap => json!({
            "ManufacturerProductNumber": "CL05B104KO5NNNC",
            "Manufacturer": {"Id": 1, "Name": "Samsung Electro-Mechanics"},
            "Description": {"ProductDescription": "CAP CER 0.1UF 16V X7R 0402", "DetailedDescription": "0.1 µF ±10% 16V Ceramic Capacitor X7R 0402 (1005 Metric)"},
            "QuantityAvailable": 1200000,
            "DatasheetUrl": "https://example.test/cl05.pdf",
            "ProductStatus": {"Id": 0, "Status": "Active"},
            "Parameters": [
                {"ParameterId": 2049, "ParameterText": "Capacitance", "ParameterType": "String", "ValueId": "1", "ValueText": "0.1 µF"},
                {"ParameterId": 14, "ParameterText": "Voltage - Rated", "ValueText": "16V"},
                {"ParameterId": 3, "ParameterText": "Tolerance", "ValueText": "±10%"},
                {"ParameterId": 17, "ParameterText": "Temperature Coefficient", "ValueText": "X7R"},
                {"ParameterId": 16, "ParameterText": "Package / Case", "ValueText": "0402 (1005 Metric)"},
                {"ParameterId": 69, "ParameterText": "Mounting Type", "ValueText": "Surface Mount, MLCC"},
                {"ParameterId": 252, "ParameterText": "Operating Temperature", "ValueText": "-55°C ~ 125°C"},
                {"ParameterId": 7, "ParameterText": "Series", "ValueText": "CL"}
            ],
            "Category": {"CategoryId": 3, "Name": "Capacitors", "ChildCategories": [{"CategoryId": 60, "Name": "Ceramic Capacitors", "ChildCategories": []}]},
            "ProductVariations": [
                {"DigiKeyProductNumber": "1276-1001-1-ND", "PackageType": {"Name": "Cut Tape (CT)"},
                 "StandardPricing": [{"BreakQuantity": 1, "UnitPrice": 0.1, "TotalPrice": 0.1}, {"BreakQuantity": 10, "UnitPrice": 0.025, "TotalPrice": 0.25}]},
                {"DigiKeyProductNumber": "1276-1001-2-ND", "PackageType": {"Name": "Tape & Reel (TR)"},
                 "StandardPricing": [{"BreakQuantity": 10000, "UnitPrice": 0.0021, "TotalPrice": 21.0}]}
            ]
        }),
        Fixture::Ldo => json!({
            "ManufacturerProductNumber": "TPS7A2033PDBVR",
            "Manufacturer": {"Name": "Texas Instruments"},
            "Description": {"ProductDescription": "IC REG LINEAR 3.3V 300MA SOT23-5"},
            "QuantityAvailable": 0,
            "DatasheetUrl": null,
            "ProductStatus": {"Status": "Not For New Designs"},
            "Parameters": [{"ParameterText": "Package / Case", "ValueText": "SOT-23-5"}],
            "Category": {"Name": "Power Management (PMIC)", "ChildCategories": [{"Name": "Voltage Regulators - Linear, Low Drop Out (LDO) Regulators"}]},
            "ProductVariations": [{"DigiKeyProductNumber": "296-1234-1-ND", "StandardPricing": [{"BreakQuantity": 1, "UnitPrice": 0.42}]}]
        }),
    }
}

pub fn product(f: Fixture) -> wire::Product {
    serde_json::from_value(product_json(f)).expect("our fixture reads")
}

pub fn token_url() -> String {
    format!("{BASE}/v1/oauth2/token")
}

pub fn keyword_url() -> String {
    format!("{BASE}/products/v4/search/keyword")
}

pub fn details_url(key: &str) -> String {
    format!("{BASE}/products/v4/search/{key}/productdetails")
}

pub fn token_answer(n: u32) -> Response {
    Response::new(
        200,
        json!({"access_token": format!("tok-{n}"), "expires_in": 599, "token_type": "Bearer"})
            .to_string(),
    )
}

pub fn details_answer(f: Fixture) -> Response {
    Response::new(200, json!({"Product": product_json(f)}).to_string())
}

pub fn keyword_answer() -> Response {
    Response::new(
        200,
        json!({"Products": [product_json(LDO), product_json(CAP)], "ProductsCount": 2, "ExactMatches": []}).to_string(),
    )
}

/// A fake DigiKey: tokens `tok-1`, `tok-2`, … in turn, every keyword
/// search our two products, and each product's details by its first
/// DigiKey part number.
pub fn fake() -> Fake {
    let f = Fake::default();
    for n in 1..=8 {
        f.answer(Method::Post, &token_url(), token_answer(n));
    }
    f.answer(Method::Post, &keyword_url(), keyword_answer());
    f.answer(
        Method::Get,
        &details_url("1276-1001-1-ND"),
        details_answer(CAP),
    );
    f.answer(
        Method::Get,
        &details_url("296-1234-1-ND"),
        details_answer(LDO),
    );
    f
}

/// A digikey source's opening over `fetch` with our test keys.
pub fn opened(
    fetch: Arc<dyn crate::fetch::Fetch>,
    limiter: &'static Limiter,
    clock: Clock,
) -> Opened {
    Opened {
        keys: Keys::of(
            &[("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)],
            Origin::Environment,
        ),
        fetch,
        base: BASE.to_owned(),
        limiter,
        clock,
    }
}
