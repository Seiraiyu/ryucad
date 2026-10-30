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
