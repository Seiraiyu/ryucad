//! A jlcparts shard (`components-*.jsonl.gz`: a schema line, then rows)
//! as records of the `jlc` source. Rows that don't read are left out and
//! counted; a shard without its schema is refused.

use super::wire::{Category, Lut, gunzip, refused};
use crate::error::Result;
use crate::record::{Offer, PriceBreak, Record};
use serde_json::Value;
use std::collections::BTreeMap;

/// Where a row keeps each field, from the schema line.
struct Schema(BTreeMap<String, usize>);

impl Schema {
    fn at<'a>(&self, row: &'a [Value], field: &str) -> Option<&'a Value> {
        row.get(*self.0.get(field)?)
    }

    fn text(&self, row: &[Value], field: &str) -> String {
        self.at(row, field)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned()
    }
}

/// A shard's records, and how many of its rows were left out.
#[derive(Debug)]
pub struct Shard {
    pub records: Vec<Record>,
    pub skipped: usize,
}

pub fn read_shard(gzipped: &[u8], lut: &Lut, cat: &Category) -> Result<Shard> {
    let raw = gunzip(gzipped)?;
    let text = std::str::from_utf8(&raw).map_err(|_| refused("shard", "a shard isn't UTF-8"))?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let schema: BTreeMap<String, usize> = lines
        .next()
        .and_then(|l| serde_json::from_str(l).ok())
        .ok_or_else(|| refused("schema", "a shard's first line isn't its schema"))?;
    let schema = Schema(schema);
    for field in ["lcsc", "mfr", "attributes", "stock"] {
        if !schema.0.contains_key(field) {
            return Err(refused("schema", format!("a shard's schema has no {field}")));
        }
    }
    let ours = super::category::category_of(&cat.category, &cat.subcategory);
    let (mut records, mut skipped) = (Vec::new(), 0);
    for l in lines {
        match serde_json::from_str::<Vec<Value>>(l)
            .ok()
            .and_then(|row| record(&schema, &row, lut, ours))
        {
            Some(r) => records.push(r),
            None => skipped += 1,
        }
    }
    Ok(Shard { records, skipped })
}

fn record(s: &Schema, row: &[Value], lut: &Lut, cat: crate::category::Category) -> Option<Record> {
    let lcsc = s.at(row, "lcsc")?.as_str()?.trim();
    if lcsc.is_empty() {
        return None;
    }
    let mut r = Record::new(super::NAME, lcsc);
    r.mpn = s.text(row, "mfr");
    r.description = s.text(row, "description");
    r.datasheet = s.text(row, "datasheet");
    r.category = Some(cat);
    r.offers = vec![Offer {
        supplier: "LCSC".to_owned(),
        sku: lcsc.to_owned(),
    }];
    r.stock = s.at(row, "stock").and_then(Value::as_u64);
    r.price_breaks = s
        .at(row, "price")
        .and_then(Value::as_array)
        .map(|ps| {
            ps.iter()
                .filter_map(|p| {
                    let q = p.get("qFrom")?.as_u64()?;
                    let price = p.get("price")?.as_f64().filter(|x| x.is_finite())?;
                    Some(PriceBreak {
                        quantity: q,
                        price: format!("{price}"),
                        currency: "USD".to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    for id in s.at(row, "attributes")?.as_array()? {
        let Some((name, value)) = id
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| lut.get(i))
        else {
            continue;
        };
        match name.as_str() {
            "Manufacturer" => r.manufacturer = super::attr::text(value).unwrap_or_default(),
            "Basic/Extended" => {
                r.basic = super::attr::text(value)
                    .map(|t| matches!(t.as_str(), "Basic" | "Preferred"))
            }
            _ => {
                if let Some((vocab, text)) = super::attr::vocabulary(name, value, cat) {
                    r.parameters.entry(vocab.to_owned()).or_insert(text);
                }
            }
        }
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jlc::{testing, wire::{Lut, Manifest}};

    #[test]
    fn a_shard_reads_into_records() {
        let site = testing::site("2026-11-06T11:33:30+00:00");
        let m = Manifest::read(&site[0].1).unwrap();
        let lut = Lut::read(&site[1].1).unwrap();
        let caps = read_shard(&site[2].1, &lut, &m.categories[0]).unwrap().records;
        assert_eq!(caps.len(), 2);
        let r = &caps[0];
        assert_eq!(
            (r.source.as_str(), r.key.as_str(), r.mpn.as_str()),
            ("jlc", "C1525", "CL05B104KO5NNNC")
        );
        assert_eq!(r.manufacturer, "Samsung Electro-Mechanics");
        assert_eq!(r.category, Some(crate::category::Category::Capacitor));
        assert_eq!(r.parameters["capacitance"], "100 nF");
        assert_eq!(r.parameters["voltage_rating"], "16 V");
        assert_eq!(r.parameters["dielectric"], "X7R");
        assert_eq!(r.parameters["package"], "0402");
        assert_eq!(r.parameters["tolerance"], "10%");
        assert_eq!(r.parameters.len(), 5, "only the vocabulary");
        assert_eq!((r.stock, r.basic), (Some(1_200_000), Some(true)));
        assert_eq!(
            r.offers,
            [crate::record::Offer { supplier: "LCSC".into(), sku: "C1525".into() }]
        );
        assert_eq!(r.price_breaks[1].quantity, 100);
        assert_eq!(
            (r.price_breaks[1].price.as_str(), r.price_breaks[1].currency.as_str()),
            ("0.0009", "USD")
        );
        assert_eq!(r.datasheet, "https://datasheets.test/C1525.pdf");
        assert_eq!(caps[1].basic, Some(false), "Extended");
        let res = read_shard(&site[3].1, &lut, &m.categories[1]).unwrap().records;
        assert_eq!(res[1].basic, Some(true), "Preferred counts as Basic");
        assert_eq!(res[0].parameters["operating_temperature"], "-55 °C to 125 °C");
        assert_eq!(res[0].parameters["power_rating"], "62.5 mW");
        // Its schema first, or it's refused naming the shard.
        let bad = testing::gz(b"[\"C1\"]\n");
        let e = read_shard(&bad, &lut, &m.categories[0]).unwrap_err();
        assert_eq!(e.code, crate::error::Code::PartsSourceRefused);
        // A row that doesn't read is left out, counted.
        let one_bad = testing::gz(format!("{}\n[1,2]\n", testing::SCHEMA).as_bytes());
        let s = read_shard(&one_bad, &lut, &m.categories[0]).unwrap();
        assert_eq!((s.records.len(), s.skipped), (0, 1));
    }
}
