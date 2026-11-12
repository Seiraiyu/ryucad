//! Materials and property items (ISO 10303-41 and -45; CAx-IF's
//! *Material and Density* and *User Defined Attributes* practices, and
//! OCCT's writer), read by [`crate::properties`] (design 448, plan 24.C).
//! Values stay as the file types them; units stay references.

use super::Entity;
use super::args::{Args, Id, real};
use crate::ImportError;
use crate::part21::{Param, Part};

/// A representation item's value as the file types it.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemValue {
    /// A real, with its measure type's name (`COUNT_MEASURE`,
    /// `LENGTH_MEASURE`, …; empty for an untyped real).
    Real {
        measure: Box<str>,
        value: f64,
    },
    Integer(i64),
    /// `.T.`, `.F.`, or `.U.` (`None`).
    Logical(Option<bool>),
    Text(Box<str>),
}

/// Which entity a value item is: `VALUE_REPRESENTATION_ITEM`, or AP242's
/// `INTEGER_`, `REAL_` or `BOOLEAN_REPRESENTATION_ITEM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueForm {
    Value,
    Integer,
    Real,
    Boolean,
}

impl ValueForm {
    pub fn name(self) -> &'static str {
        match self {
            ValueForm::Value => "VALUE_REPRESENTATION_ITEM",
            ValueForm::Integer => "INTEGER_REPRESENTATION_ITEM",
            ValueForm::Real => "REAL_REPRESENTATION_ITEM",
            ValueForm::Boolean => "BOOLEAN_REPRESENTATION_ITEM",
        }
    }
}

fn logical(e: &str) -> Option<Option<bool>> {
    match e {
        "T" => Some(Some(true)),
        "F" => Some(Some(false)),
        "U" => Some(None),
        _ => None,
    }
}

/// A select's value: a typed measure, a literal, or `None` for `$` and
/// what isn't a value.
pub(super) fn item_value(p: &Param) -> Option<ItemValue> {
    match p {
        Param::Real(x) => Some(ItemValue::Real {
            measure: "".into(),
            value: *x,
        }),
        Param::Int(n) => Some(ItemValue::Integer(*n)),
        Param::Enum(e) => logical(e).map(ItemValue::Logical),
        Param::Str(s) => Some(ItemValue::Text((&**s).into())),
        Param::Typed(name, v) => match &**v {
            Param::Real(_) | Param::Int(_) => Some(ItemValue::Real {
                measure: (&**name).into(),
                value: real(v)?,
            }),
            Param::Enum(e) => logical(e).map(ItemValue::Logical),
            Param::Str(s) => Some(ItemValue::Text((&**s).into())),
            _ => None,
        },
        _ => None,
    }
}

/// `v` as `form` holds it: an integer item's value whole, a real item's a
/// real, a boolean item's a logical.
fn typed(form: ValueForm, v: ItemValue) -> Option<ItemValue> {
    match (form, v) {
        (ValueForm::Value, v) => Some(v),
        (ValueForm::Integer, ItemValue::Integer(n)) => Some(ItemValue::Integer(n)),
        (ValueForm::Integer, ItemValue::Real { value, .. })
            if value.trunc() == value && value.abs() <= 9_007_199_254_740_992.0 =>
        {
            Some(ItemValue::Integer(value as i64))
        }
        (ValueForm::Real, ItemValue::Real { value, .. }) => Some(ItemValue::Real {
            measure: "".into(),
            value,
        }),
        (ValueForm::Real, ItemValue::Integer(n)) => Some(ItemValue::Real {
            measure: "".into(),
            value: n as f64,
        }),
        (ValueForm::Boolean, ItemValue::Logical(b)) => Some(ItemValue::Logical(b)),
        _ => None,
    }
}

fn value_item(id: Id, params: &[Param], form: ValueForm) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, 2)?;
    let name = a.string()?;
    let p = a.param();
    let value = item_value(p)
        .and_then(|v| typed(form, v))
        .ok_or_else(|| a.fault("expected a value of the item's type"))?;
    Ok(Entity::ValueItem { name, value, form })
}

/// A simple record of a material or a property item, or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Entity, ImportError>> {
    Some(match name {
        // MATERIAL_DESIGNATION(name, of_definition): one characterized
        // definition (AP214, OCCT), or a set of them (ISO 10303-45).
        "MATERIAL_DESIGNATION" => (|| {
            let mut a = Args::new(id, params, 2)?;
            let name = a.string()?;
            let definitions = match params.get(1) {
                Some(Param::List(_)) => a.ids()?,
                _ => vec![a.id()?],
            };
            Ok(Entity::MaterialDesignation { name, definitions })
        })(),
        // MEASURE_REPRESENTATION_ITEM(name, value_component, unit_component)
        "MEASURE_REPRESENTATION_ITEM" => (|| {
            let mut a = Args::new(id, params, 3)?;
            let name = a.string()?;
            let p = a.param();
            let value = item_value(p).ok_or_else(|| a.fault("expected a measure"))?;
            Ok(Entity::MeasureItem {
                name,
                value,
                unit: a.opt_id()?,
            })
        })(),
        "VALUE_REPRESENTATION_ITEM" => value_item(id, params, ValueForm::Value),
        "INTEGER_REPRESENTATION_ITEM" => value_item(id, params, ValueForm::Integer),
        "REAL_REPRESENTATION_ITEM" => value_item(id, params, ValueForm::Real),
        "BOOLEAN_REPRESENTATION_ITEM" => value_item(id, params, ValueForm::Boolean),
        "DERIVED_UNIT" => (|| {
            let mut a = Args::new(id, params, 1)?;
            Ok(Entity::DerivedUnit { elements: a.ids()? })
        })(),
        "DERIVED_UNIT_ELEMENT" => (|| {
            let mut a = Args::new(id, params, 2)?;
            Ok(Entity::UnitElement {
                unit: a.id()?,
                exponent: a.real()?,
            })
        })(),
        _ => return None,
    })
}

/// A complex instance of a measure or value item (CAx-IF writes
/// `(MEASURE_REPRESENTATION_ITEM()MEASURE_WITH_UNIT(…)REPRESENTATION_ITEM('…'))`,
/// AP242 writers `(INTEGER_REPRESENTATION_ITEM()INT_LITERAL()LITERAL_NUMBER(7)REPRESENTATION_ITEM('…'))`),
/// or `None`.
pub(super) fn complex(id: Id, parts: &[Part]) -> Option<Result<Entity, ImportError>> {
    let part = |n: &str| parts.iter().find(|p| &*p.name == n);
    let form = if part("MEASURE_REPRESENTATION_ITEM").is_some() {
        None
    } else if part("INTEGER_REPRESENTATION_ITEM").is_some() {
        Some(ValueForm::Integer)
    } else if part("REAL_REPRESENTATION_ITEM").is_some() {
        Some(ValueForm::Real)
    } else if part("BOOLEAN_REPRESENTATION_ITEM").is_some() {
        Some(ValueForm::Boolean)
    } else if part("VALUE_REPRESENTATION_ITEM").is_some() {
        Some(ValueForm::Value)
    } else {
        return None;
    };
    Some((|| {
        let name = match part("REPRESENTATION_ITEM") {
            Some(r) => Args::new(id, &r.params, 1)?.string()?,
            None => "".into(),
        };
        let malformed = |what| ImportError::Malformed {
            instance: id,
            attribute: 0,
            what,
        };
        match form {
            None => {
                let m = part("MEASURE_WITH_UNIT")
                    .ok_or(malformed("a measure item needs its measure"))?;
                let mut a = Args::new(id, &m.params, 2)?;
                let p = a.param();
                let value = item_value(p).ok_or_else(|| a.fault("expected a measure"))?;
                Ok(Entity::MeasureItem {
                    name,
                    value,
                    unit: a.opt_id()?,
                })
            }
            Some(form) => {
                // The value is the one attribute of the part that holds it
                // (VALUE_REPRESENTATION_ITEM, LITERAL_NUMBER,
                // BOOLEAN_LITERAL, …).
                let value = parts
                    .iter()
                    .filter(|p| &*p.name != "REPRESENTATION_ITEM" && p.params.len() == 1)
                    .find_map(|p| item_value(&p.params[0]))
                    .and_then(|v| typed(form, v))
                    .ok_or(malformed("expected a value of the item's type"))?;
                Ok(Entity::ValueItem { name, value, form })
            }
        }
    })())
}
