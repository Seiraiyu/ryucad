//! A layer's copper merged: its parts applied in drawing order, then the
//! holes in its span cut out.
//!
//! A point is copper when the last part covering it is positive (stage
//! 0's rule: a negative clears only what was drawn before it). So the
//! layer is the union, over each run of positive parts, of the run less
//! every negative part drawn after it: one boolean per run that has
//! negatives after it, then one more for the union less the holes. A
//! layer is two booleans deep whatever its parts, so its bound is the
//! flattening's plus two nanometres.

use rayon::prelude::*;
use ryu_board::geom::Rect;
use ryu_board::model::Polarity;

use super::{
    FillRule, FlattenOptions, Op, Polygons, Rings, boolean, flatten_hole, flatten_shape,
    ring_bounds,
};
use crate::layout::Layout;
use crate::tree::{Packed, meets};

fn rings_bounds(r: &Rings) -> Option<Rect> {
    r.rings.iter().map(|r| ring_bounds(r)).reduce(Rect::union)
}

fn joined<'a>(parts: impl IntoIterator<Item = &'a Rings>) -> Rings {
    let mut out = Rings::default();
    for r in parts {
        out.rings.extend(r.rings.iter().cloned());
        out.bound = out.bound.max(r.bound);
    }
    out
}

/// Merges a layer's flattened parts, in drawing order, and cuts out
/// `holes`.
pub fn merge_layer(parts: &[(Polarity, Rings)], holes: &[Rings]) -> Polygons {
    // Runs of positive parts, by their indices.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k < parts.len() {
        if parts[k].0 == Polarity::Negative {
            k += 1;
            continue;
        }
        let start = k;
        while k < parts.len() && parts[k].0 == Polarity::Positive {
            k += 1;
        }
        runs.push((start, k));
    }
    let negatives: Vec<(Rect, u32)> = parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.0 == Polarity::Negative)
        .filter_map(|(k, p)| rings_bounds(&p.1).map(|b| (b, k as u32)))
        .collect();
    let neg_tree = Packed::new(negatives);
    let pieces: Vec<Rings> = runs
        .par_iter()
        .map(|&(start, end)| {
            let run = joined(parts[start..end].iter().map(|p| &p.1));
            let Some(b) = rings_bounds(&run) else {
                return run;
            };
            let mut later = Vec::new();
            neg_tree.query(&b, &mut later);
            later.retain(|&k| k as usize >= end);
            later.sort_unstable();
            if later.is_empty() {
                return run;
            }
            let cut = joined(later.iter().map(|&k| &parts[k as usize].1));
            boolean(
                &run,
                FillRule::NonZero,
                &cut,
                FillRule::NonZero,
                Op::Difference,
            )
            .rings()
        })
        .collect();
    let all = joined(&pieces);
    let all_box = rings_bounds(&all);
    let holes = joined(holes.iter().filter(|h| match (rings_bounds(h), all_box) {
        (Some(h), Some(a)) => meets(&h, &a),
        _ => false,
    }));
    boolean(
        &all,
        FillRule::NonZero,
        &holes,
        FillRule::NonZero,
        Op::Difference,
    )
}

/// Every layer of a layout merged, in parallel: each layer's items in key
/// order (the caller's keys order them as drawn), copper layers less the
/// holes whose span holds them.
pub fn merge_layers<K: Ord + Sync>(layout: &Layout<K>, opts: &FlattenOptions) -> Vec<Polygons> {
    let items: Vec<(usize, Vec<(Polarity, Rings)>)> = layout
        .items
        .values()
        .collect::<Vec<_>>()
        .par_iter()
        .map(|it| (it.layer, flatten_shape(&it.shape, opts)))
        .collect();
    let holes: Vec<(&crate::layout::Hole, Rings)> = layout
        .holes
        .values()
        .collect::<Vec<_>>()
        .par_iter()
        .map(|h| (*h, flatten_hole(&h.shape, opts)))
        .collect();
    (0..layout.layers.len())
        .into_par_iter()
        .map(|layer| {
            let parts: Vec<(Polarity, Rings)> = items
                .iter()
                .filter(|(l, _)| *l == layer)
                .flat_map(|(_, p)| p.iter().cloned())
                .collect();
            let cut: Vec<Rings> = if layout.layers[layer].copper {
                holes
                    .iter()
                    .filter(|(h, _)| h.span.0 <= layer && layer <= h.span.1)
                    .map(|(_, r)| r.clone())
                    .collect()
            } else {
                Vec::new()
            };
            merge_layer(&parts, &cut)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Hole, HoleShape, Item, Layer};
    use ryu_board::geom::{Nm, Point};
    use ryu_board::model::Side;
    use ryu_board::shape::{Shape, circle, polygon};

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn square(c: Point, side: Nm, polarity: Polarity) -> Shape {
        let h = side / 2;
        let ring = polygon(&[
            c.offset(-h, -h),
            c.offset(h, -h),
            c.offset(h, h),
            c.offset(-h, h),
        ])
        .unwrap();
        Shape::regions(polarity, [ryu_board::geom::Region::solid(ring)])
    }

    fn disc_shape(c: Point, r: Nm, polarity: Polarity) -> Shape {
        Shape::regions(
            polarity,
            [ryu_board::geom::Region::solid(circle(c, r).unwrap())],
        )
    }

    #[test]
    fn negatives_clear_only_what_came_before() {
        let opts = FlattenOptions::default();
        let mut parts = Vec::new();
        for s in [
            square(p(0, 0), 10_000, Polarity::Positive),
            disc_shape(p(0, 0), 3_000, Polarity::Negative),
            square(p(0, 0), 2_000, Polarity::Positive),
        ] {
            parts.extend(flatten_shape(&s, &opts));
        }
        let m = merge_layer(&parts, &[]);
        // A square with a round hole, and a square island in the hole.
        assert_eq!(m.polygons.len(), 2);
        assert_eq!(m.polygons[0].holes.len(), 1);
        assert!(m.bound <= 50 + 2, "{}", m.bound);
        assert_eq!(m.contains(p(4_000, 4_000)), crate::measure::Verdict::Yes);
        assert_eq!(m.contains(p(2_500, 0)), crate::measure::Verdict::No);
        assert_eq!(m.contains(p(0, 0)), crate::measure::Verdict::Yes);
        // Drawn the other way round, the negative clears the island too.
        parts.swap(1, 2);
        let m = merge_layer(&parts, &[]);
        assert_eq!(m.polygons.len(), 1);
        assert_eq!(m.contains(p(0, 0)), crate::measure::Verdict::No);
    }

    #[test]
    fn holes_cut_the_copper_layers_they_span() {
        let mut l: Layout<u32> = Layout::default();
        for (name, copper) in [("top", true), ("mask", false), ("bottom", true)] {
            l.layers.push(Layer {
                name: name.into(),
                side: Side::Top,
                copper,
            });
        }
        for layer in 0..3 {
            l.items.insert(
                layer as u32,
                Item {
                    layer,
                    shape: square(p(0, 0), 10_000, Polarity::Positive),
                    net: None,
                },
            );
        }
        l.holes.insert(
            9,
            Hole {
                shape: HoleShape::Round {
                    at: p(0, 0),
                    d: 4_000,
                },
                span: (0, 2),
                plated: true,
                backdrilled: vec![],
            },
        );
        let merged = merge_layers(&l, &FlattenOptions::default());
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].polygons[0].holes.len(), 1);
        assert!(merged[1].polygons[0].holes.is_empty());
        assert_eq!(merged[0], merged[2]);
    }
}
