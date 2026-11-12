//! Materials and user-defined attributes read (design 448, plan 24.C),
//! as CAx-IF's *Material and Density* and *User Defined Attributes*
//! practices and OCCT's writer give them, per product definition: its
//! material (a `MATERIAL_DESIGNATION` over the definition or its shape,
//! else OCCT's `material name` property), that material's density in
//! kg/m³ (a `density` property's measure in mass per volume), and every
//! other property's representation items as typed values. What is over an
//! assembly usage or a shape aspect is `retained` (ryupdm's properties
//! are per part); an item that doesn't read is `failed`, never the import.

use crate::ImportError;
use crate::products::Products;
use crate::schema::{Entities, Entity, Id, ItemValue, MeasureUnit, scaled};
use std::collections::BTreeMap;

/// A value as the file types it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    /// `value` in `unit`, as written.
    Measure {
        value: f64,
        unit: MeasureUnit,
    },
}

/// One attribute: its item's name and value, the property definition and
/// item it was read from, and every instance read for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub value: Value,
    pub property: Id,
    pub item: Id,
    pub instances: Vec<Id>,
}

/// A product's material: its name, the designation (or `material name`
/// property) it came from, its density in kg/m³, and every instance read
/// for them, ascending.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    pub instance: Id,
    pub density: Option<f64>,
    pub instances: Vec<Id>,
}

/// What a product definition's file says of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProductProperties {
    pub material: Option<Material>,
    /// In property, then item, order.
    pub attributes: Vec<Attribute>,
}

/// Every product's, by index into [`Products::definitions`]; what isn't
/// a product's, ascending; what doesn't read, with why.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Properties {
    pub products: BTreeMap<usize, ProductProperties>,
    pub retained: Vec<Id>,
    pub failed: Vec<(Id, ImportError)>,
}

/// What a characterized definition is of.
enum Owner {
    Product(usize),
    Elsewhere,
}

/// A property definition, by what it says of itself.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    MaterialName,
    Density,
    Attributes,
}

fn kind_of(name: &str, description: &str) -> Option<Kind> {
    let (n, d) = (
        name.trim().to_ascii_lowercase(),
        description.trim().to_ascii_lowercase(),
    );
    if n == "density" || (n == "material property" && d.contains("density")) {
        return Some(Kind::Density);
    }
    if n == "material property" && d == "material name" {
        return Some(Kind::MaterialName);
    }
    match crate::census::property_category(name) {
        crate::census::Category::Properties => Some(Kind::Attributes),
        // A material property this crate doesn't read is someone's
        // material data all the same: kept, not lost.
        crate::census::Category::Materials => Some(Kind::Attributes),
        _ => None,
    }
}

fn malformed(instance: Id, attribute: usize, what: &'static str) -> ImportError {
    ImportError::Malformed {
        instance,
        attribute,
        what,
    }
}

/// A measure's number.
fn number(v: &ItemValue) -> Option<f64> {
    match v {
        ItemValue::Real { value, .. } => Some(*value),
        ItemValue::Integer(n) => Some(*n as f64),
        _ => None,
    }
}

/// The density `item` gives, in kg/m³, with the instances read.
fn density_of(e: &Entities, item: Id) -> Result<(f64, Vec<Id>), ImportError> {
    let Entity::MeasureItem {
        value,
        unit: Some(u),
        ..
    } = e.get(item, item)?
    else {
        return Err(crate::schema::wrong(item, "a density measure with a unit"));
    };
    let v = number(value).ok_or_else(|| malformed(item, 1, "expected a number"))?;
    let mut unit = e.measure_unit(item, *u)?;
    if let Some(occt) = occts_density_unit(e, *u, &unit)? {
        unit = occt;
    }
    if (unit.length, unit.angle, unit.mass, unit.other) != (-3, 0, 1, false) {
        return Err(malformed(
            item,
            2,
            "a density's unit must be a mass per volume",
        ));
    }
    let kg_m3 = match unit.decimal {
        Some(d) => scaled(v, d),
        None => v * unit.size,
    };
    let mut instances = vec![item];
    instances.extend(&unit.instances);
    Ok((kg_m3, instances))
}

/// OCCT's density unit, as its writer (7.8 and before) gives it: a
/// `DERIVED_UNIT` of a mass unit to the power 3 and a length unit to the
/// power 2 (`g^3*cm^2` where it means `g*cm^-3`), which OCCT's own reader
/// takes as mass per volume whatever the exponents say. That form alone,
/// two elements and nothing else, reads as its mass unit over its length
/// unit cubed; any other unit is left as it is.
fn occts_density_unit(
    e: &Entities,
    u: Id,
    unit: &MeasureUnit,
) -> Result<Option<MeasureUnit>, ImportError> {
    if (unit.length, unit.angle, unit.mass, unit.other) != (2, 0, 3, false) {
        return Ok(None);
    }
    let Entity::DerivedUnit { elements } = e.get(u, u)? else {
        return Ok(None);
    };
    let [a, b] = elements[..] else {
        return Ok(None);
    };
    let (mut mass, mut length) = (None, None);
    for el in [a, b] {
        let Entity::UnitElement { unit: named, .. } = e.get(u, el)? else {
            return Ok(None);
        };
        let n = e.measure_unit(el, *named)?;
        match (n.length, n.mass) {
            (0, 1) => mass = Some(n),
            (1, 0) => length = Some(n),
            _ => return Ok(None),
        }
    }
    let (Some(m), Some(l)) = (mass, length) else {
        return Ok(None);
    };
    let decimal = m.decimal.zip(l.decimal).map(|(m, l)| m - 3 * l);
    Ok(Some(MeasureUnit {
        length: -3,
        angle: 0,
        mass: 1,
        other: false,
        size: decimal.map_or(m.size / (l.size * l.size * l.size), |d| scaled(1.0, d)),
        decimal,
        name: format!("{}*{}^-3", m.name, l.name),
        instances: unit.instances.clone(),
    }))
}

/// Every material and attribute of `e`, whose product tree is `products`.
pub fn read(e: &Entities, products: &Products) -> Properties {
    let mut out = Properties::default();
    let found = e.select(|_, x| {
        matches!(
            x,
            Ok(Entity::PropertyDefinition { .. }
                | Entity::PropertyRepresentation { .. }
                | Entity::MaterialDesignation { .. }
                | Entity::DefinitionShape { .. })
        )
    });
    let by_definition: BTreeMap<Id, usize> = products
        .definitions
        .iter()
        .enumerate()
        .map(|(k, d)| (d.instance, k))
        .collect();
    let mut shape_of: BTreeMap<Id, Id> = BTreeMap::new();
    let mut defs: Vec<(Id, Box<str>, Box<str>, Id)> = Vec::new();
    let mut reps: BTreeMap<Id, Vec<(Id, Id)>> = BTreeMap::new();
    let mut designations: Vec<(Id, Box<str>, Vec<Id>)> = Vec::new();
    for &id in &found {
        match e.get(id, id) {
            Ok(Entity::DefinitionShape { definition }) => {
                shape_of.insert(id, *definition);
            }
            Ok(Entity::PropertyDefinition {
                name,
                description,
                definition,
            }) => defs.push((id, name.clone(), description.clone(), *definition)),
            Ok(Entity::PropertyRepresentation {
                definition,
                representation,
            }) => reps
                .entry(*definition)
                .or_default()
                .push((id, *representation)),
            Ok(Entity::MaterialDesignation { name, definitions }) => {
                designations.push((id, name.clone(), definitions.clone()))
            }
            _ => {}
        }
    }
    let owner = |x: Id| -> Owner {
        if let Some(&k) = by_definition.get(&x) {
            return Owner::Product(k);
        }
        match shape_of.get(&x).and_then(|d| by_definition.get(d)) {
            Some(&k) => Owner::Product(k),
            None => Owner::Elsewhere,
        }
    };
    // Names first (designations, then OCCT's material name properties),
    // so a density finds its material whatever the order.
    let mut names: BTreeMap<usize, Material> = BTreeMap::new();
    for (id, name, definitions) in &designations {
        let mine: Vec<usize> = definitions
            .iter()
            .filter_map(|&d| match owner(d) {
                Owner::Product(k) => Some(k),
                Owner::Elsewhere => None,
            })
            .collect();
        let mut kept = false;
        for k in mine {
            if let std::collections::btree_map::Entry::Vacant(v) = names.entry(k) {
                v.insert(Material {
                    name: name.to_string(),
                    instance: *id,
                    density: None,
                    instances: vec![*id],
                });
                kept = true;
            }
        }
        if !kept {
            out.retained.push(*id);
        }
    }
    let closure = |pd: Id| -> Vec<Id> {
        let mut v = vec![pd];
        for (pdr, rep) in reps.get(&pd).into_iter().flatten() {
            v.push(*pdr);
            v.push(*rep);
            if let Ok(Entity::Representation { items, .. }) = e.get(*rep, *rep) {
                v.extend(items);
            }
        }
        v
    };
    let items_of = |pd: Id, out: &mut Properties| -> Vec<(Id, Id, Id)> {
        let mut v = Vec::new();
        for &(pdr, rep) in reps.get(&pd).into_iter().flatten() {
            match e.get(pdr, rep) {
                Ok(Entity::Representation { items, .. }) => {
                    v.extend(items.iter().map(|&it| (pdr, rep, it)))
                }
                Ok(_) => out
                    .failed
                    .push((rep, crate::schema::wrong(rep, "a representation"))),
                Err(err) => out.failed.push((rep, err)),
            }
        }
        v
    };
    let mut densities: Vec<(usize, Id)> = Vec::new();
    for (pd, name, description, of) in &defs {
        let Some(kind) = kind_of(name, description) else {
            continue;
        };
        let k = match owner(*of) {
            Owner::Product(k) => k,
            Owner::Elsewhere => {
                out.retained.extend(closure(*pd));
                continue;
            }
        };
        match kind {
            Kind::Density => densities.push((k, *pd)),
            Kind::MaterialName => {
                let found = items_of(*pd, &mut out)
                    .into_iter()
                    .find_map(|(pdr, rep, it)| match e.get(rep, it) {
                        Ok(Entity::Descriptive { name, .. }) => {
                            Some((name.to_string(), pdr, rep, it))
                        }
                        _ => None,
                    });
                match (found, names.get_mut(&k)) {
                    (Some((n, pdr, rep, it)), None) => {
                        names.insert(
                            k,
                            Material {
                                name: n,
                                instance: *pd,
                                density: None,
                                instances: vec![*pd, pdr, rep, it],
                            },
                        );
                    }
                    (Some((n, pdr, rep, it)), Some(m)) if m.name == n => {
                        m.instances.extend([*pd, pdr, rep, it]);
                    }
                    _ => out.retained.extend(closure(*pd)),
                }
            }
            Kind::Attributes => attributes(e, k, *pd, items_of(*pd, &mut out), &mut out),
        }
    }
    for (k, pd) in densities {
        let Some(m) = names.get_mut(&k).filter(|m| m.density.is_none()) else {
            out.retained.extend(closure(pd));
            continue;
        };
        let items = items_of(pd, &mut out);
        let Some(&(pdr, rep, it)) = items.first() else {
            out.retained.extend(closure(pd));
            continue;
        };
        match density_of(e, it) {
            Ok((d, instances)) => {
                m.density = Some(d);
                m.instances.extend([pd, pdr, rep]);
                m.instances.extend(instances);
            }
            Err(err) => out.failed.push((it, err)),
        }
    }
    for (k, mut m) in names {
        m.instances.sort_unstable();
        m.instances.dedup();
        out.products.entry(k).or_default().material = Some(m);
    }
    out.retained.sort_unstable();
    out.retained.dedup();
    out.failed.sort_by_key(|f| f.0);
    out
}

/// One item as a value, with the instances read; `Ok(None)` for an item
/// that isn't a value (a point, a placement): kept, not read.
fn attribute(e: &Entities, item: Id) -> Result<Option<(String, Value, Vec<Id>)>, ImportError> {
    Ok(Some(match e.get(item, item)? {
        Entity::Descriptive { name, description } => (
            name.to_string(),
            Value::Text(description.to_string()),
            vec![item],
        ),
        Entity::ValueItem { name, value, .. } => {
            (name.to_string(), plain(item, value)?, vec![item])
        }
        Entity::MeasureItem { name, value, unit } => {
            let x = number(value).ok_or_else(|| malformed(item, 1, "expected a number"))?;
            match unit {
                None => (name.to_string(), Value::Number(x), vec![item]),
                Some(u) => {
                    let unit = e.measure_unit(item, *u)?;
                    let mut instances = vec![item];
                    instances.extend(&unit.instances);
                    (
                        name.to_string(),
                        Value::Measure { value: x, unit },
                        instances,
                    )
                }
            }
        }
        _ => return Ok(None),
    }))
}

/// A value item's value: a whole count or integer measure an integer,
/// any other real a number, a logical a boolean, a string text.
fn plain(item: Id, v: &ItemValue) -> Result<Value, ImportError> {
    Ok(match v {
        ItemValue::Integer(n) => Value::Integer(*n),
        ItemValue::Real { measure, value }
            if matches!(&**measure, "COUNT_MEASURE" | "INTEGER")
                && value.trunc() == *value
                && value.abs() <= 9_007_199_254_740_992.0 =>
        {
            Value::Integer(*value as i64)
        }
        ItemValue::Real { value, .. } => Value::Number(*value),
        ItemValue::Logical(Some(b)) => Value::Boolean(*b),
        ItemValue::Logical(None) => {
            return Err(malformed(item, 1, "an unknown logical is no value"));
        }
        ItemValue::Text(s) => Value::Text(s.to_string()),
    })
}

/// Reads property `pd`'s items as product `k`'s attributes.
fn attributes(e: &Entities, k: usize, pd: Id, items: Vec<(Id, Id, Id)>, out: &mut Properties) {
    for (pdr, rep, it) in items {
        match attribute(e, it) {
            Ok(Some((name, value, mut instances))) => {
                instances.extend([pd, pdr, rep]);
                instances.sort_unstable();
                instances.dedup();
                out.products
                    .entry(k)
                    .or_default()
                    .attributes
                    .push(Attribute {
                        name,
                        value,
                        property: pd,
                        item: it,
                        instances,
                    });
            }
            Ok(None) => out.retained.push(it),
            Err(err) => out.failed.push((it, err)),
        }
    }
}

impl crate::Opened {
    /// Every material and attribute of the file, read once.
    pub fn properties(&self) -> &Properties {
        self.properties
            .get_or_init(|| read(&self.entities, &self.products))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::products::Products;
    use crate::schema::tests::entities;

    /// Product `P1` (definition #6, its shape #7), with `extra` after.
    pub(crate) fn product(extra: &str) -> (Entities, Products) {
        let e = entities(&format!(
            "#1=APPLICATION_CONTEXT('');\
             #2=PRODUCT_CONTEXT('',#1,'');\
             #3=PRODUCT('P1','bracket','',(#2));\
             #4=PRODUCT_DEFINITION_FORMATION('','',#3);\
             #5=PRODUCT_DEFINITION_CONTEXT('',#1,'');\
             #6=PRODUCT_DEFINITION('design','',#4,#5);\
             #7=PRODUCT_DEFINITION_SHAPE('','',#6);\
             #8=REPRESENTATION_CONTEXT('','');\
             #9=(MASS_UNIT()NAMED_UNIT(*)SI_UNIT($,.GRAM.));\
             #10=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.CENTI.,.METRE.));\
             #11=DERIVED_UNIT_ELEMENT(#9,1.);\
             #12=DERIVED_UNIT_ELEMENT(#10,-3.);\
             #13=DERIVED_UNIT((#11,#12));{extra}"
        ));
        let p = Products::build(&e);
        (e, p)
    }

    #[test]
    fn a_designation_with_its_density() {
        let (e, p) = product(
            "#20=MATERIAL_DESIGNATION('6061-T6',#7);\
             #21=PROPERTY_DEFINITION('material property','density',#7);\
             #22=PROPERTY_DEFINITION_REPRESENTATION(#21,#23);\
             #23=REPRESENTATION('density',(#24),#8);\
             #24=MEASURE_REPRESENTATION_ITEM('density',POSITIVE_RATIO_MEASURE(2.7),#13);",
        );
        let r = read(&e, &p);
        let m = r.products[&0].material.as_ref().unwrap();
        assert_eq!(
            (m.name.as_str(), m.instance, m.density),
            ("6061-T6", 20, Some(2700.0))
        );
        assert_eq!(m.instances, vec![9, 10, 11, 12, 13, 20, 21, 22, 23, 24]);
        assert!(r.retained.is_empty() && r.failed.is_empty(), "{r:?}");
    }

    #[test]
    fn occts_material_name_and_density_properties() {
        let (e, p) = product(
            "#20=PROPERTY_DEFINITION('material property','material name',#7);\
             #21=PROPERTY_DEFINITION_REPRESENTATION(#20,#22);\
             #22=REPRESENTATION('material name',(#23),#8);\
             #23=DESCRIPTIVE_REPRESENTATION_ITEM('Steel','Steel');\
             #24=PROPERTY_DEFINITION('density','',#6);\
             #25=PROPERTY_DEFINITION_REPRESENTATION(#24,#26);\
             #26=REPRESENTATION('density',(#27),#8);\
             #27=MEASURE_REPRESENTATION_ITEM('density',POSITIVE_RATIO_MEASURE(7.85),#13);",
        );
        let r = read(&e, &p);
        let m = r.products[&0].material.as_ref().unwrap();
        assert_eq!((m.name.as_str(), m.density), ("Steel", Some(7850.0)));
    }

    /// OCCT 7.8's writer gives its density unit the exponents 3 (mass)
    /// and 2 (length); OCCT's reader takes it as mass per volume, and so
    /// does this (`tests/data/occt/material.stp`).
    #[test]
    fn occts_misnumbered_density_unit_is_mass_per_volume() {
        let (e, p) = product(
            "#20=PROPERTY_DEFINITION('material property','material name',#6);\
             #21=PROPERTY_DEFINITION_REPRESENTATION(#20,#22);\
             #22=REPRESENTATION('material name',(#23),#8);\
             #23=DESCRIPTIVE_REPRESENTATION_ITEM('Aluminium','6061 alloy');\
             #24=PROPERTY_DEFINITION('material property','density',#6);\
             #25=PROPERTY_DEFINITION_REPRESENTATION(#24,#26);\
             #26=REPRESENTATION('density',(#27),#8);\
             #27=MEASURE_REPRESENTATION_ITEM('density',POSITIVE_RATIO_MEASURE(2.7),#30);\
             #30=DERIVED_UNIT((#31,#33));\
             #31=DERIVED_UNIT_ELEMENT(#32,3.);\
             #32=(MASS_UNIT()NAMED_UNIT(*)SI_UNIT($,.GRAM.));\
             #33=DERIVED_UNIT_ELEMENT(#34,2.);\
             #34=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.CENTI.,.METRE.));",
        );
        let pr = read(&e, &p);
        let m = pr.products[&0].material.as_ref().unwrap();
        assert_eq!((m.name.as_str(), m.density), ("Aluminium", Some(2700.0)));
        assert!(pr.failed.is_empty(), "{:?}", pr.failed);
        // Any other exponents are still not a density.
        let (e, p) = product(
            "#20=PROPERTY_DEFINITION('material property','material name',#6);\
             #21=PROPERTY_DEFINITION_REPRESENTATION(#20,#22);\
             #22=REPRESENTATION('material name',(#23),#8);\
             #23=DESCRIPTIVE_REPRESENTATION_ITEM('Aluminium','6061 alloy');\
             #24=PROPERTY_DEFINITION('material property','density',#6);\
             #25=PROPERTY_DEFINITION_REPRESENTATION(#24,#26);\
             #26=REPRESENTATION('density',(#27),#8);\
             #27=MEASURE_REPRESENTATION_ITEM('density',POSITIVE_RATIO_MEASURE(2.7),#30);\
             #30=DERIVED_UNIT((#31,#33));\
             #31=DERIVED_UNIT_ELEMENT(#32,2.);\
             #32=(MASS_UNIT()NAMED_UNIT(*)SI_UNIT($,.GRAM.));\
             #33=DERIVED_UNIT_ELEMENT(#34,2.);\
             #34=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.CENTI.,.METRE.));",
        );
        let pr = read(&e, &p);
        assert_eq!(pr.products[&0].material.as_ref().unwrap().density, None);
    }

    #[test]
    fn a_second_designation_and_a_density_alone_are_retained() {
        let (e, p) = product(
            "#20=MATERIAL_DESIGNATION('A',#6);\
             #21=MATERIAL_DESIGNATION('B',#7);",
        );
        let r = read(&e, &p);
        assert_eq!(r.products[&0].material.as_ref().unwrap().name, "A");
        assert_eq!(r.retained, vec![21]);
        let (e, p) = product(
            "#21=PROPERTY_DEFINITION('material property','density',#7);\
             #22=PROPERTY_DEFINITION_REPRESENTATION(#21,#23);\
             #23=REPRESENTATION('density',(#24),#8);\
             #24=MEASURE_REPRESENTATION_ITEM('density',POSITIVE_RATIO_MEASURE(2.7),#13);",
        );
        let r = read(&e, &p);
        assert!(r.products.get(&0).is_none_or(|x| x.material.is_none()));
        assert_eq!(r.retained, vec![21, 22, 23, 24]);
    }

    #[test]
    fn a_density_in_the_wrong_unit_fails() {
        let (e, p) = product(
            "#20=MATERIAL_DESIGNATION('A',#7);\
             #21=PROPERTY_DEFINITION('material property','density',#7);\
             #22=PROPERTY_DEFINITION_REPRESENTATION(#21,#23);\
             #23=REPRESENTATION('density',(#24),#8);\
             #24=MEASURE_REPRESENTATION_ITEM('density',POSITIVE_RATIO_MEASURE(2.7),#10);",
        );
        let r = read(&e, &p);
        let m = r.products[&0].material.as_ref().unwrap();
        assert_eq!((m.name.as_str(), m.density), ("A", None));
        assert_eq!(r.failed.iter().map(|f| f.0).collect::<Vec<_>>(), vec![24]);
    }

    #[test]
    fn every_item_type_reads_as_its_value() {
        let (e, p) = product(
            "#20=PROPERTY_DEFINITION('user defined attributes','',#6);\
             #21=PROPERTY_DEFINITION_REPRESENTATION(#20,#22);\
             #22=REPRESENTATION('',(#23,#24,#25,#26,#27,#28,#29,#30,#31),#8);\
             #23=DESCRIPTIVE_REPRESENTATION_ITEM('Supplier','ACME Metals');\
             #24=VALUE_REPRESENTATION_ITEM('Batch Size',COUNT_MEASURE(250.));\
             #25=VALUE_REPRESENTATION_ITEM('Yield',RATIO_MEASURE(0.95));\
             #26=VALUE_REPRESENTATION_ITEM('RoHS',BOOLEAN(.T.));\
             #27=MEASURE_REPRESENTATION_ITEM('Wall-Thickness',LENGTH_MEASURE(2.5),#32);\
             #28=(MEASURE_REPRESENTATION_ITEM()MEASURE_WITH_UNIT(MASS_MEASURE(0.12),#33)REPRESENTATION_ITEM('Net Mass'));\
             #29=INTEGER_REPRESENTATION_ITEM('Revision Count',3);\
             #30=MEASURE_REPRESENTATION_ITEM('Plain',LENGTH_MEASURE(4.),$);\
             #31=CARTESIAN_POINT('',(0.,0.,0.));\
             #32=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\
             #33=(MASS_UNIT()NAMED_UNIT(*)SI_UNIT(.KILO.,.GRAM.));",
        );
        let r = read(&e, &p);
        let a = &r.products[&0].attributes;
        let got: Vec<(&str, &Value)> = a.iter().map(|x| (x.name.as_str(), &x.value)).collect();
        assert_eq!(got.len(), 8, "{got:?}");
        assert_eq!(got[0], ("Supplier", &Value::Text("ACME Metals".into())));
        assert_eq!(got[1], ("Batch Size", &Value::Integer(250)));
        assert_eq!(got[2], ("Yield", &Value::Number(0.95)));
        assert_eq!(got[3], ("RoHS", &Value::Boolean(true)));
        let Value::Measure { value, unit } = got[4].1 else {
            panic!("{got:?}")
        };
        assert_eq!((*value, unit.name.as_str(), unit.length), (2.5, "mm", 1));
        let Value::Measure { value, unit } = got[5].1 else {
            panic!("{got:?}")
        };
        assert_eq!((*value, unit.name.as_str(), unit.mass), (0.12, "kg", 1));
        assert_eq!(got[6], ("Revision Count", &Value::Integer(3)));
        assert_eq!(got[7], ("Plain", &Value::Number(4.0)));
        assert_eq!(
            a[4].instances,
            vec![20, 21, 22, 27, 32],
            "the property's, the item's, its unit's"
        );
        assert_eq!((a[0].property, a[0].item), (20, 23));
        // The point is no value: kept, not read.
        assert_eq!(r.retained, vec![31]);
    }

    #[test]
    fn what_isnt_a_parts_is_retained_and_what_doesnt_read_fails() {
        let (e, p) = product(
            "#20=PROPERTY_DEFINITION('user defined attributes','',#40);\
             #21=PROPERTY_DEFINITION_REPRESENTATION(#20,#22);\
             #22=REPRESENTATION('',(#23),#8);\
             #23=DESCRIPTIVE_REPRESENTATION_ITEM('Torque','4 N m');\
             #24=PROPERTY_DEFINITION('user defined attributes','',#6);\
             #25=PROPERTY_DEFINITION_REPRESENTATION(#24,#26);\
             #26=REPRESENTATION('',(#27,#28),#8);\
             #27=VALUE_REPRESENTATION_ITEM('Broken',$);\
             #28=VALUE_REPRESENTATION_ITEM('Maybe',LOGICAL(.U.));\
             #29=PROPERTY_DEFINITION('geometric validation property','volume',#7);\
             #40=SHAPE_ASPECT('hole','',#7,.F.);",
        );
        let r = read(&e, &p);
        assert_eq!(r.retained, vec![20, 21, 22, 23], "a shape aspect's");
        assert_eq!(
            r.failed.iter().map(|f| f.0).collect::<Vec<_>>(),
            vec![27, 28]
        );
        assert!(r.products.get(&0).is_none_or(|x| x.attributes.is_empty()));
        // A validation property is the census's, not read here.
        assert!(!r.retained.contains(&29));
    }

    #[test]
    fn an_occurrences_attributes_are_retained() {
        let (e, p) = product(
            "#30=PRODUCT('P2','pin','',(#2));\
             #31=PRODUCT_DEFINITION_FORMATION('','',#30);\
             #32=PRODUCT_DEFINITION('design','',#31,#5);\
             #33=NEXT_ASSEMBLY_USAGE_OCCURRENCE('1','','',#6,#32,$);\
             #34=PRODUCT_DEFINITION_SHAPE('','',#33);\
             #35=PROPERTY_DEFINITION('user defined attributes','',#34);\
             #36=PROPERTY_DEFINITION_REPRESENTATION(#35,#37);\
             #37=REPRESENTATION('',(#38),#8);\
             #38=DESCRIPTIVE_REPRESENTATION_ITEM('Torque','4 N m');",
        );
        let r = read(&e, &p);
        assert_eq!(r.retained, vec![35, 36, 37, 38]);
        assert!(r.products.values().all(|x| x.attributes.is_empty()));
    }

    #[test]
    fn an_opened_file_reads_its_properties_once() {
        let bytes = b"ISO-10303-21;HEADER;FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;DATA;\
            #1=APPLICATION_CONTEXT('');#2=PRODUCT_CONTEXT('',#1,'');\
            #3=PRODUCT('P1','bracket','',(#2));#4=PRODUCT_DEFINITION_FORMATION('','',#3);\
            #5=PRODUCT_DEFINITION_CONTEXT('',#1,'');#6=PRODUCT_DEFINITION('design','',#4,#5);\
            #7=MATERIAL_DESIGNATION('Steel',#6);ENDSEC;END-ISO-10303-21;";
        let o = crate::open(bytes).unwrap();
        let first: *const Properties = o.properties();
        assert!(std::ptr::eq(first, o.properties()));
        assert_eq!(
            o.properties().products[&0].material.as_ref().unwrap().name,
            "Steel"
        );
    }

    #[test]
    fn a_measure_whose_derived_unit_overflows_fails_the_item_alone() {
        let refs = vec!["#41"; 16].join(",");
        let (e, p) = product(&format!(
            "#20=PROPERTY_DEFINITION('user defined attributes','',#6);\
             #21=PROPERTY_DEFINITION_REPRESENTATION(#20,#22);\
             #22=REPRESENTATION('',(#23,#24),#8);\
             #23=MEASURE_REPRESENTATION_ITEM('w',LENGTH_MEASURE(1.),#42);\
             #24=DESCRIPTIVE_REPRESENTATION_ITEM('Supplier','ACME');\
             #41=DERIVED_UNIT_ELEMENT(#10,8.);\
             #42=DERIVED_UNIT(({refs}));"
        ));
        let r = read(&e, &p);
        assert_eq!(r.failed.iter().map(|f| f.0).collect::<Vec<_>>(), vec![23]);
        assert_eq!(r.products[&0].attributes.len(), 1, "the rest still reads");
    }
}
