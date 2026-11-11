//! Styles written (design 448, plan 24.B), as CAx-IF's *Model Styling
//! and Organization* gives them: a `STYLED_ITEM` per B-rep item or face
//! with one `PRESENTATION_STYLE_ASSIGNMENT` (a surface style on both
//! sides with its fill colour, and a
//! `SURFACE_STYLE_RENDERING_WITH_PROPERTIES` with a
//! `SURFACE_STYLE_TRANSPARENT` only below opacity 1), collected in one
//! `MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION` per product;
//! an occurrence that differs from its part as a
//! `CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM` on its body in the context
//! of its usages (outermost first), over the body's own style or, where
//! it has none, a `STYLED_ITEM` of `NULL_STYLE`. Identical styles share
//! their entities, so a thousand red faces write one colour, and every
//! style is written after the geometry, the products and the usages: no
//! geometry instance is renumbered by a colour.

use super::{Contexts, Out, WriteModel, WrittenBody, e, f, r, refs, s};
use crate::part21::Param;
use ryukei::name::Name;
use std::collections::BTreeMap;

/// A surface style to write: an sRGB colour and an opacity in `(0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    pub colour: [u8; 3],
    pub opacity: f64,
}

impl Paint {
    fn key(&self) -> ([u8; 3], u64) {
        (self.colour, self.opacity.to_bits())
    }
}

/// Body `body` of product `product` (indices into the model; a product's
/// bodies as [`super::Product::bodies`] lists them): its own style, and
/// its faces' by name where they differ.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BodyPaint {
    pub product: usize,
    pub body: usize,
    pub base: Option<Paint>,
    pub faces: Vec<(Name, Paint)>,
}

/// Body `body` of the product `path` ends at, painted in that occurrence:
/// `path` is each step (product, index into its `children`) from the
/// model's first product down.
#[derive(Clone, Debug, PartialEq)]
pub struct OccurrencePaint {
    pub path: Vec<(usize, usize)>,
    pub body: usize,
    pub paint: Paint,
}

/// Everything painted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paints {
    pub bodies: Vec<BodyPaint>,
    pub occurrences: Vec<OccurrencePaint>,
}

impl Paints {
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty() && self.occurrences.is_empty()
    }
}

/// Entities written once and shared.
#[derive(Default)]
struct Shared {
    colours: BTreeMap<[u8; 3], u64>,
    assignments: BTreeMap<([u8; 3], u64), u64>,
    null: Option<u64>,
}

impl Shared {
    fn colour(&mut self, out: &mut Out, c: [u8; 3]) -> u64 {
        if let Some(&id) = self.colours.get(&c) {
            return id;
        }
        let k = |x: u8| f(f64::from(x) / 255.0);
        let id = out.add("COLOUR_RGB", vec![s(""), k(c[0]), k(c[1]), k(c[2])]);
        self.colours.insert(c, id);
        id
    }

    fn assignment(&mut self, out: &mut Out, p: Paint) -> u64 {
        if let Some(&id) = self.assignments.get(&p.key()) {
            return id;
        }
        let colour = self.colour(out, p.colour);
        let fasc = out.add("FILL_AREA_STYLE_COLOUR", vec![s(""), r(colour)]);
        let fas = out.add("FILL_AREA_STYLE", vec![s(""), refs([fasc])]);
        let fill = out.add("SURFACE_STYLE_FILL_AREA", vec![r(fas)]);
        let mut elements = vec![fill];
        if p.opacity < 1.0 {
            let t = out.add(
                "SURFACE_STYLE_TRANSPARENT",
                vec![f(crate::styles::transparency_of(p.opacity))],
            );
            elements.push(out.add(
                "SURFACE_STYLE_RENDERING_WITH_PROPERTIES",
                vec![e("NORMAL_SHADING"), r(colour), refs([t])],
            ));
        }
        let side = out.add("SURFACE_SIDE_STYLE", vec![s(""), refs(elements)]);
        let usage = out.add("SURFACE_STYLE_USAGE", vec![e("BOTH"), r(side)]);
        let psa = out.add("PRESENTATION_STYLE_ASSIGNMENT", vec![refs([usage])]);
        self.assignments.insert(p.key(), psa);
        psa
    }

    /// An assignment of `NULL_STYLE`: a style that styles nothing.
    fn null(&mut self, out: &mut Out) -> u64 {
        if let Some(id) = self.null {
            return id;
        }
        let id = out.add(
            "PRESENTATION_STYLE_ASSIGNMENT",
            vec![Param::List(vec![Param::Typed(
                "NULL_STYLE".into(),
                Box::new(e("NULL")),
            )])],
        );
        self.null = Some(id);
        id
    }
}

/// Writes `paints` over what was written: `bodies[k][j]` is product
/// `k`'s body `j`, `usages[k][n]` product `k`'s `n`th usage's instance.
pub(crate) fn write(
    out: &mut Out,
    cx: &Contexts,
    model: &WriteModel,
    bodies: &[Vec<WrittenBody>],
    usages: &[Vec<u64>],
    paints: &Paints,
) {
    if paints.is_empty() {
        return;
    }
    let mut shared = Shared::default();
    // Each B-rep item's own styled item, for an occurrence to override.
    let mut own: BTreeMap<u64, u64> = BTreeMap::new();
    let mut by_product: BTreeMap<usize, Vec<&BodyPaint>> = BTreeMap::new();
    for b in &paints.bodies {
        by_product.entry(b.product).or_default().push(b);
    }
    for list in by_product.into_values() {
        let mut list = list;
        list.sort_by_key(|b| b.body);
        let mut styled = Vec::new();
        for b in list {
            let Some(w) = bodies.get(b.product).and_then(|p| p.get(b.body)) else {
                continue;
            };
            if let Some(p) = b.base {
                let psa = shared.assignment(out, p);
                for &item in &w.items {
                    let id = out.add("STYLED_ITEM", vec![s(""), refs([psa]), r(item)]);
                    own.insert(item, id);
                    styled.push(id);
                }
            }
            let mut faces: Vec<(u64, Paint)> = b
                .faces
                .iter()
                .filter_map(|(n, p)| w.faces.get(n).map(|&id| (id, *p)))
                .collect();
            faces.sort_by_key(|x| x.0);
            for (face, p) in faces {
                let psa = shared.assignment(out, p);
                styled.push(out.add("STYLED_ITEM", vec![s(""), refs([psa]), r(face)]));
            }
        }
        if !styled.is_empty() {
            out.add(
                "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION",
                vec![s(""), refs(styled), r(cx.geometric)],
            );
        }
    }
    occurrences(out, cx, model, bodies, usages, paints, &mut shared, &mut own);
}

/// Each occurrence paint as a `CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM`
/// on each B-rep item of the body, in the context of its usages
/// (outermost first), over the item's own styled item or a `NULL_STYLE`
/// one written for it; all in one presentation representation.
#[allow(clippy::too_many_arguments)]
fn occurrences(
    out: &mut Out,
    cx: &Contexts,
    model: &WriteModel,
    bodies: &[Vec<WrittenBody>],
    usages: &[Vec<u64>],
    paints: &Paints,
    shared: &mut Shared,
    own: &mut BTreeMap<u64, u64>,
) {
    let mut styled = Vec::new();
    for o in &paints.occurrences {
        let Some(&(parent, n)) = o.path.last() else {
            continue;
        };
        let Some(leaf) = model
            .products
            .get(parent)
            .and_then(|p| p.children.get(n))
            .map(|c| c.product)
        else {
            continue;
        };
        let context: Vec<u64> = o
            .path
            .iter()
            .filter_map(|&(p, c)| usages.get(p)?.get(c).copied())
            .collect();
        if context.len() != o.path.len() {
            continue;
        }
        let Some(w) = bodies.get(leaf).and_then(|p| p.get(o.body)) else {
            continue;
        };
        let psa = shared.assignment(out, o.paint);
        for &item in &w.items {
            let over = match own.get(&item) {
                Some(&id) => id,
                None => {
                    let null = shared.null(out);
                    let id = out.add("STYLED_ITEM", vec![s(""), refs([null]), r(item)]);
                    own.insert(item, id);
                    styled.push(id);
                    id
                }
            };
            styled.push(out.add(
                "CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM",
                vec![
                    s(""),
                    refs([psa]),
                    r(item),
                    r(over),
                    refs(context.iter().copied()),
                ],
            ));
        }
    }
    if !styled.is_empty() {
        out.add(
            "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION",
            vec![s(""), refs(styled), r(cx.geometric)],
        );
    }
}
