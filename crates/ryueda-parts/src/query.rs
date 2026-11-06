//! Searching by what a part must be: a category and conditions on typed
//! parameters (`capacitance=100nF`, `voltage_rating>=16V`,
//! `package~SOT`), and supply conditions (`--stock N`, `--basic`). The
//! same judging serves every source.
//!
//! A part whose stated value fails a condition is left out. One that
//! doesn't state a parameter, or states one that doesn't read, stays, its
//! `unknown` naming it, and ranks after parts meeting more conditions.

use crate::category::Category;
use crate::dec::Dec;
use crate::error::{Code, Error, Result};
use crate::param::{self, Kind, Value};
use crate::record::Record;
use serde::Serialize;

/// How many results a search gives unless asked.
pub const LIMIT: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ge,
    Le,
    /// Text contains, without regard to case.
    Contains,
}

impl Op {
    pub fn text(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ge => ">=",
            Op::Le => "<=",
            Op::Contains => "~",
        }
    }
}

/// `name=value`, `name>=value`, `name<=value` or `name~text`, the name in
/// the vocabulary and the value read for its kind.
#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    pub name: &'static str,
    pub op: Op,
    pub text: String,
    asked: Option<Value>,
}

fn usage(s: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("a condition is name=value, name>=value, name<=value or name~text, not {s:?}"),
    )
    .with("condition", s)
}

impl Condition {
    pub fn parse(s: &str) -> Result<Condition> {
        let at = s.find(['=', '<', '>', '~']).ok_or_else(|| usage(s))?;
        let rest = &s[at..];
        let (op, len) = if rest.starts_with(">=") {
            (Op::Ge, 2)
        } else if rest.starts_with("<=") {
            (Op::Le, 2)
        } else if rest.starts_with('=') {
            (Op::Eq, 1)
        } else if rest.starts_with('~') {
            (Op::Contains, 1)
        } else {
            return Err(usage(s));
        };
        let wanted = s[..at].trim();
        let Some((name, kind, example)) = param::VOCABULARY.iter().find(|(n, _, _)| *n == wanted)
        else {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{wanted:?} isn't a parameter searches know: {}",
                    param::names().join(", ")
                ),
            )
            .with("param", wanted)
            .with("known", param::names()));
        };
        let text = s[at + len..].trim().to_owned();
        if text.is_empty() {
            return Err(usage(s));
        }
        let ordered = matches!(
            kind,
            Kind::Quantity(_) | Kind::Tolerance | Kind::TemperatureCoefficient | Kind::Temperature
        );
        if matches!(op, Op::Ge | Op::Le) && !ordered {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{name} is compared with = or ~, not {}", op.text()),
            )
            .with("param", *name));
        }
        let asked = match op {
            Op::Contains => None,
            _ => Some(param::read_kind(*kind, &text).ok_or_else(|| {
                Error::new(
                    Code::InvalidQuantity,
                    format!("{text:?} isn't a {name} value such as {example:?}"),
                )
                .with("param", *name)
                .with("value", &text)
            })?),
        };
        Ok(Condition {
            name,
            op,
            text,
            asked,
        })
    }

    /// As written by a search's answer: `voltage_rating>=16V`.
    pub fn to_text(&self) -> String {
        format!("{}{}{}", self.name, self.op.text(), self.text)
    }

    /// How a part's text for this parameter stands: met or not, or `None`
    /// where the part doesn't say (no text, text that doesn't read, or a
    /// value this can't be compared with).
    pub fn judge(&self, text: Option<&str>) -> Option<bool> {
        let text = text?;
        let Some(asked) = &self.asked else {
            return Some(text.to_lowercase().contains(&self.text.to_lowercase()));
        };
        let have = param::read_kind(param::kind_of(self.name)?, text)?;
        compare(&have, self.op, asked)
    }

    /// [`Condition::judge`] by a value already read: `text` is still
    /// needed for `~`, which compares text.
    pub fn judge_value(&self, value: Option<&Value>, text: Option<&str>) -> Option<bool> {
        let Some(asked) = &self.asked else {
            return self.judge(text);
        };
        compare(value?, self.op, asked)
    }
}

/// A record's vocabulary values, read once: for searches over a
/// catalogue, which would otherwise read each record's text per search.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Typed(Vec<(&'static str, Value)>);

impl Typed {
    pub fn of(r: &Record) -> Typed {
        Typed(
            param::VOCABULARY
                .iter()
                .filter_map(|(name, kind, _)| {
                    let text = r.parameters.get(*name)?;
                    Some((*name, param::read_kind(*kind, text)?))
                })
                .collect(),
        )
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn by(op: Op, have: Dec, asked: Dec) -> bool {
    match op {
        Op::Eq => have == asked,
        Op::Ge => have >= asked,
        Op::Le => have <= asked,
        Op::Contains => false,
    }
}

/// `have` against `asked`: a range `>=` covers it and `<=` lies within it;
/// a temperature coefficient's bounds compare magnitudes.
fn compare(have: &Value, op: Op, asked: &Value) -> Option<bool> {
    use Value::*;
    Some(match (have, asked) {
        (Quantity { dim: a, si: x }, Quantity { dim: b, si: y }) if a == b => by(op, *x, *y),
        (Ratio(x), Ratio(y)) => by(op, *x, *y),
        (Absolute { dim: a, si: x }, Absolute { dim: b, si: y }) if a == b => by(op, *x, *y),
        (PerKelvin(x), PerKelvin(y)) => match op {
            Op::Eq => x == y,
            _ => by(op, x.abs(), y.abs()),
        },
        (Dielectric(x), Dielectric(y)) => x == y,
        (Package(x), Package(y)) => x == y,
        (Mounting(x), Mounting(y)) => x == y,
        (Range { low: a, high: b }, Range { low: c, high: d }) => match op {
            Op::Eq => a == c && b == d,
            Op::Ge => a <= c && b >= d,
            Op::Le => a >= c && b <= d,
            Op::Contains => false,
        },
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub category: Option<Category>,
    pub conditions: Vec<Condition>,
    /// At least this many in stock at the source.
    pub stock: Option<u64>,
    /// JLC Basic (or Preferred) only.
    pub basic: bool,
    pub limit: usize,
}

/// A record a search found, with how it stood.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    #[serde(flatten)]
    pub record: Record,
    /// Conditions it meets with a value it states.
    pub meets: usize,
    /// What it doesn't say, so couldn't be judged by: parameter names,
    /// `stock`, `basic`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
}

impl Query {
    /// `r` as a hit, or `None` where it is another category or fails a
    /// condition it can be judged by.
    pub fn judge(&self, r: &Record) -> Option<Hit> {
        self.judge_with(r, |c| c.judge(r.parameters.get(c.name).map(String::as_str)))
    }

    /// [`Query::judge`] with the record's values read already.
    pub fn judge_typed(&self, r: &Record, t: &Typed) -> Option<Hit> {
        self.judge_with(r, |c| {
            c.judge_value(t.get(c.name), r.parameters.get(c.name).map(String::as_str))
        })
    }

    fn judge_with(&self, r: &Record, cond: impl Fn(&Condition) -> Option<bool>) -> Option<Hit> {
        if self.category.is_some() && r.category != self.category {
            return None;
        }
        let (mut meets, mut unknown) = (0, Vec::new());
        for c in &self.conditions {
            match cond(c) {
                Some(true) => meets += 1,
                Some(false) => return None,
                None => unknown.push(c.name.to_owned()),
            }
        }
        if let Some(n) = self.stock {
            match r.stock {
                Some(s) if s >= n => meets += 1,
                Some(_) => return None,
                None => unknown.push("stock".to_owned()),
            }
        }
        if self.basic {
            match r.basic {
                Some(true) => meets += 1,
                Some(false) => return None,
                None => unknown.push("basic".to_owned()),
            }
        }
        Some(Hit {
            record: r.clone(),
            meets,
            unknown,
        })
    }
}

/// One source's hits in the order answers give them: the most conditions
/// met first, then the most in stock, then by key.
pub fn rank(hits: &mut [Hit]) {
    hits.sort_by(|a, b| {
        b.meets
            .cmp(&a.meets)
            .then_with(|| b.record.stock.cmp(&a.record.stock))
            .then_with(|| a.record.key.cmp(&b.record.key))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(params: &[(&str, &str)]) -> Record {
        let mut r = Record::new("library", "k");
        r.category = Some(Category::Capacitor);
        r.parameters = params
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        r
    }

    fn query(conditions: &[&str]) -> Query {
        Query {
            category: Some(Category::Capacitor),
            conditions: conditions
                .iter()
                .map(|s| Condition::parse(s).unwrap())
                .collect(),
            stock: None,
            basic: false,
            limit: LIMIT,
        }
    }

    #[test]
    fn conditions_parse_and_refuse() {
        let c = Condition::parse("voltage_rating>=16V").unwrap();
        assert_eq!(
            (c.name, c.op, c.to_text()),
            ("voltage_rating", Op::Ge, "voltage_rating>=16V".to_owned())
        );
        assert_eq!(Condition::parse("package~SOT").unwrap().op, Op::Contains);
        assert_eq!(
            Condition::parse(" tolerance <= 1% ").unwrap().to_text(),
            "tolerance<=1%"
        );
        assert_eq!(
            Condition::parse("colour=red").unwrap_err().code,
            Code::InvalidParam
        );
        let e = Condition::parse("capacitance=lots").unwrap_err();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::InvalidQuantity, &serde_json::json!("capacitance"))
        );
        assert_eq!(
            Condition::parse("dielectric>=X7R").unwrap_err().code,
            Code::InvalidParam
        );
        for bad in ["capacitance", "capacitance=", "=100nF", "capacitance<100nF"] {
            assert_eq!(
                Condition::parse(bad).unwrap_err().code,
                Code::InvalidParam,
                "{bad}"
            );
        }
    }

    #[test]
    fn parts_are_judged_by_value() {
        let q = query(&[
            "capacitance=100nF",
            "voltage_rating>=16V",
            "dielectric=X7R",
            "package=0402",
        ]);
        let hit = q
            .judge(&record(&[
                ("capacitance", "0.1 uF"),
                ("voltage_rating", "25 V"),
                ("dielectric", "x7r"),
                ("package", "0402"),
            ]))
            .unwrap();
        assert_eq!((hit.meets, hit.unknown.len()), (4, 0));
        assert!(
            q.judge(&record(&[
                ("capacitance", "100nF"),
                ("voltage_rating", "10 V")
            ]))
            .is_none()
        );
        let partial = q.judge(&record(&[("capacitance", "100nF")])).unwrap();
        assert_eq!(partial.unknown, ["voltage_rating", "dielectric", "package"]);
        let mut r = record(&[("capacitance", "100nF")]);
        r.category = Some(Category::Resistor);
        assert!(q.judge(&r).is_none());
        // A value that doesn't read is unknown, not a miss.
        assert_eq!(
            q.judge(&record(&[("capacitance", "lots")])).unwrap().meets,
            0
        );
        // Ranges cover; tolerances compare as fractions, never with a quantity.
        let t = Condition::parse("operating_temperature>=-40 °C to 85 °C").unwrap();
        assert_eq!(t.judge(Some("-55 °C to 125 °C")), Some(true));
        assert_eq!(t.judge(Some("0 °C to 70 °C")), Some(false));
        let tol = Condition::parse("tolerance<=1%").unwrap();
        assert_eq!(tol.judge(Some("±0.5%")), Some(true));
        assert_eq!(tol.judge(Some("5%")), Some(false));
        assert_eq!(tol.judge(Some("±0.25 pF")), None);
        let tc = Condition::parse("temperature_coefficient<=100 ppm/K").unwrap();
        assert_eq!(tc.judge(Some("-50 ppm/K")), Some(true));
        assert_eq!(
            Condition::parse("package~sot")
                .unwrap()
                .judge(Some("SOT-23-5")),
            Some(true)
        );
        assert_eq!(tol.judge(None), None);
    }

    #[test]
    fn supply_conditions_and_ranking() {
        let q = Query {
            category: None,
            conditions: vec![],
            stock: Some(100),
            basic: true,
            limit: LIMIT,
        };
        let mut a = Record::new("jlc", "b");
        a.stock = Some(500);
        a.basic = Some(true);
        let mut b = Record::new("jlc", "a");
        b.stock = Some(50);
        assert!(q.judge(&b).is_none());
        let c = Record::new("library", "c");
        assert_eq!(q.judge(&c).unwrap().unknown, ["stock", "basic"]);
        let mut hits = vec![q.judge(&c).unwrap(), q.judge(&a).unwrap()];
        rank(&mut hits);
        let keys: Vec<&str> = hits.iter().map(|h| h.record.key.as_str()).collect();
        assert_eq!(keys, ["b", "c"]);
    }

    #[test]
    fn typed_records_judge_as_their_text() {
        let mut q = query(&["capacitance=100nF", "voltage_rating>=16V", "package~04"]);
        q.stock = Some(10);
        for params in [
            vec![
                ("capacitance", "0.1 uF"),
                ("voltage_rating", "25 V"),
                ("package", "0402"),
            ],
            vec![("capacitance", "100nF"), ("voltage_rating", "10 V")],
            vec![("capacitance", "lots")],
            vec![],
        ] {
            let r = record(&params);
            let t = Typed::of(&r);
            assert_eq!(q.judge_typed(&r, &t), q.judge(&r), "{params:?}");
        }
        let r = record(&[
            ("capacitance", "100 nF"),
            ("colour", "red"),
            ("package", "0402"),
        ]);
        assert_eq!(Typed::of(&r).len(), 2, "only the vocabulary is typed");
        assert!(Typed::default().is_empty());
    }
}
