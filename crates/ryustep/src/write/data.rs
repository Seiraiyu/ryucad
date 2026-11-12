//! Materials and user-defined attributes written (design 448, plan 24.C),
//! in the forms CAx-IF's practices and OCCT's XDE give them, after every
//! geometry, product, usage and style instance, so no geometry instance
//! is renumbered by them. Per product, in product order: its
//! `MATERIAL_DESIGNATION` over its `PRODUCT_DEFINITION_SHAPE` (a set of
//! definitions in AP242, one in AP214), OCCT's `material name` property,
//! and with a density the `density` property, a `POSITIVE_RATIO_MEASURE`
//! in kg/m³ over one shared `DERIVED_UNIT`; then one `user defined
//! attributes` property over its `PRODUCT_DEFINITION`, its items in the
//! order given. Every property shares one `REPRESENTATION_CONTEXT`.

use super::{Contexts, Out, e, f, r, refs, s};
use crate::part21::Param;
use crate::schema::Schema;

/// A value to write.
#[derive(Clone, Debug, PartialEq)]
pub enum DataValue {
    Text(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    /// A length in metres, written in the file's length unit.
    Length(f64),
    /// A plane angle in radians.
    Angle(f64),
}

/// One user-defined attribute.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeData {
    pub name: String,
    pub value: DataValue,
}

/// A material: its name, and its density in kg/m³ where known.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialData {
    pub name: String,
    pub density: Option<f64>,
}

/// Product `product`'s (an index into the model's products) material
/// and attributes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProductData {
    pub product: usize,
    pub material: Option<MaterialData>,
    pub attributes: Vec<AttributeData>,
}

/// Everything written of the products besides their shapes and styles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartData {
    pub products: Vec<ProductData>,
}

impl PartData {
    pub fn is_empty(&self) -> bool {
        self.products
            .iter()
            .all(|p| p.material.is_none() && p.attributes.is_empty())
    }
}

/// Entities written once and shared.
#[derive(Default)]
struct Shared {
    context: Option<u64>,
    density_unit: Option<u64>,
}

impl Shared {
    fn context(&mut self, out: &mut Out) -> u64 {
        if let Some(c) = self.context {
            return c;
        }
        let c = out.add("REPRESENTATION_CONTEXT", vec![s(""), s("")]);
        self.context = Some(c);
        c
    }

    /// Kilograms per cubic metre.
    fn density_unit(&mut self, out: &mut Out) -> u64 {
        if let Some(u) = self.density_unit {
            return u;
        }
        let kg = out.complex(vec![
            ("MASS_UNIT", vec![]),
            ("NAMED_UNIT", vec![Param::Derived]),
            ("SI_UNIT", vec![e("KILO"), e("GRAM")]),
        ]);
        let m = out.complex(vec![
            ("LENGTH_UNIT", vec![]),
            ("NAMED_UNIT", vec![Param::Derived]),
            ("SI_UNIT", vec![Param::Unset, e("METRE")]),
        ]);
        let per_kg = out.add("DERIVED_UNIT_ELEMENT", vec![r(kg), f(1.0)]);
        let per_m = out.add("DERIVED_UNIT_ELEMENT", vec![r(m), f(-3.0)]);
        let u = out.add("DERIVED_UNIT", vec![refs([per_kg, per_m])]);
        self.density_unit = Some(u);
        u
    }
}

/// A property of `of` named `name` with `description`: its definition,
/// the representation relationship, the representation named `rep_name`,
/// then the items `items` writes.
fn property(
    out: &mut Out,
    ctx: u64,
    (name, description): (&str, &str),
    of: u64,
    rep_name: &str,
    items: impl FnOnce(&mut Out) -> Vec<u64>,
) {
    let pd = out.add("PROPERTY_DEFINITION", vec![s(name), s(description), r(of)]);
    let rep = out.reserve();
    out.add("PROPERTY_DEFINITION_REPRESENTATION", vec![r(pd), r(rep)]);
    let items = items(out);
    out.fill(
        rep,
        "REPRESENTATION",
        vec![s(rep_name), refs(items), r(ctx)],
    );
}

fn typed(name: &str, x: Param) -> Param {
    Param::Typed(name.into(), Box::new(x))
}

/// One attribute's item (Task 8 fills every type in).
fn item(out: &mut Out, _cx: &Contexts, _schema: Schema, a: &AttributeData) -> u64 {
    match &a.value {
        DataValue::Text(t) => out.add("DESCRIPTIVE_REPRESENTATION_ITEM", vec![s(&a.name), s(t)]),
        _ => out.add("DESCRIPTIVE_REPRESENTATION_ITEM", vec![s(&a.name), s("")]),
    }
}

/// Writes `data` over what was written: `products[k]` is product `k`'s
/// `(PRODUCT_DEFINITION, PRODUCT_DEFINITION_SHAPE)`.
pub(crate) fn write(
    out: &mut Out,
    cx: &Contexts,
    schema: Schema,
    products: &[(u64, u64)],
    data: &PartData,
) {
    if data.is_empty() {
        return;
    }
    let mut shared = Shared::default();
    let mut list: Vec<&ProductData> = data.products.iter().collect();
    list.sort_by_key(|p| p.product);
    for p in list {
        let Some(&(definition, shape)) = products.get(p.product) else {
            continue;
        };
        if let Some(m) = &p.material {
            let of = match schema {
                Schema::Ap214 => r(shape),
                _ => refs([shape]),
            };
            out.add("MATERIAL_DESIGNATION", vec![s(&m.name), of]);
            let ctx = shared.context(out);
            property(
                out,
                ctx,
                ("material property", "material name"),
                shape,
                "material name",
                |out| {
                    vec![out.add(
                        "DESCRIPTIVE_REPRESENTATION_ITEM",
                        vec![s(&m.name), s(&m.name)],
                    )]
                },
            );
            if let Some(d) = m.density {
                let unit = shared.density_unit(out);
                property(
                    out,
                    ctx,
                    ("material property", "density"),
                    shape,
                    "density",
                    |out| {
                        vec![out.add(
                            "MEASURE_REPRESENTATION_ITEM",
                            vec![s("density"), typed("POSITIVE_RATIO_MEASURE", f(d)), r(unit)],
                        )]
                    },
                );
            }
        }
        if !p.attributes.is_empty() {
            let ctx = shared.context(out);
            property(
                out,
                ctx,
                ("user defined attributes", ""),
                definition,
                "",
                |out| {
                    p.attributes
                        .iter()
                        .map(|a| item(out, cx, schema, a))
                        .collect()
                },
            );
        }
    }
}
