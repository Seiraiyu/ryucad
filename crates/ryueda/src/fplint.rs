//! The footprint lint (ryueda 7.F, design 924): what a footprint's own
//! checks (`Footprint::check`) can't see, from its pads' copper as a
//! board builds it: pads of different numbers touching, a number's pads
//! on separate copper, a courtyard that doesn't close, no reference
//! designator. Findings are faults with codes; they never refuse a
//! write. Exact: copper touches by `ryu_copper::measure::touches`.

use crate::board::pad;
use crate::error::Code;
use crate::fault::Fault;
use crate::footprint::{Footprint, PadKind};
use crate::id::Id;
use ryu_copper::measure::{self, Body, BodyOptions, Verdict};

/// The lint's codes, as requests name them.
pub const CODES: [(Code, &str); 4] = [
    (Code::FootprintPadsOverlap, "footprint_pads_overlap"),
    (
        Code::FootprintPadNumberRepeated,
        "footprint_pad_number_repeated",
    ),
    (Code::FootprintCourtyardOpen, "footprint_courtyard_open"),
    (Code::FootprintNoReference, "footprint_no_reference"),
];

/// A lint code by name.
pub fn code(name: &str) -> Option<Code> {
    CODES.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
}

/// A pad's copper, as the lint judges it.
struct Copper<'a> {
    id: &'a Id,
    number: Option<&'a str>,
    /// Top, bottom, every inner layer.
    sides: (bool, bool, bool),
    body: Body,
}

/// Every pad with copper (not a non-plated hole), in ID order.
fn copper(f: &Footprint) -> Vec<Copper<'_>> {
    let opts = BodyOptions::default();
    f.pads
        .iter()
        .filter(|(_, p)| p.kind != PadKind::Npth)
        .filter_map(|(id, p)| {
            let s = pad::copper(p).ok()?;
            Some(Copper {
                id,
                number: p.number.as_deref(),
                sides: pad::copper_sides(p),
                body: Body::of_shape(&s, &opts),
            })
        })
        .collect()
}

fn share(a: (bool, bool, bool), b: (bool, bool, bool)) -> bool {
    (a.0 && b.0) || (a.1 && b.1) || (a.2 && b.2)
}

/// Whether two pads' copper touches on a side both are on: `No` without
/// measuring when their boxes, grown by their bounds, are apart.
fn touch(a: &Copper, b: &Copper) -> Verdict {
    if !share(a.sides, b.sides) {
        return Verdict::No;
    }
    let (Some(x), Some(y)) = (a.body.bounds(), b.body.bounds()) else {
        return Verdict::No;
    };
    let g = a.body.bound + b.body.bound + 1;
    if x.max.x + g < y.min.x
        || y.max.x + g < x.min.x
        || x.max.y + g < y.min.y
        || y.max.y + g < x.min.y
    {
        return Verdict::No;
    }
    measure::touches(&a.body, &b.body)
}

/// Every finding of `f`, in a fixed order: overlaps by pad ID, repeated
/// numbers by number, then the courtyard and the fields.
pub fn lint(f: &Footprint) -> Vec<Fault> {
    let c = copper(f);
    let mut out = Vec::new();
    overlaps(&c, &mut out);
    repeated(&c, &mut out);
    courtyards(f, &mut out);
    reference(f, &mut out);
    out
}

/// Two numbered pads of different numbers whose copper touches: a
/// warning, uncertain verdicts too. Not an error (a departure from plan
/// 1015, by the census of KiCad 9.0.9's footprints): KiCad stacks lands of
/// different numbers on purpose (USB-C's A4/B9 VBUS and A1/B12 GND,
/// QFNs' several exposed pads, RF land patterns), and a real short shows
/// on a board, where DRC judges pads of different nets.
fn overlaps(c: &[Copper], out: &mut Vec<Fault>) {
    for i in 0..c.len() {
        for j in i + 1..c.len() {
            let (a, b) = (&c[i], &c[j]);
            let (Some(na), Some(nb)) = (a.number, b.number) else {
                continue;
            };
            if na == nb {
                continue;
            }
            let v = touch(a, b);
            if v == Verdict::No {
                continue;
            }
            out.push(
                Fault::warning(
                    Code::FootprintPadsOverlap,
                    format!("pads {} ({na}) and {} ({nb}) touch", a.id, b.id),
                )
                .with("pointer", format!("/pads/{}", b.id))
                .with("pads", [a.id.as_str(), b.id.as_str()])
                .with("numbers", [na, nb])
                .with("uncertain", v == Verdict::Uncertain),
            );
        }
    }
}

/// A number whose pads aren't one piece of copper (touching, through
/// one another, on shared sides): a warning, each piece's pads listed.
fn repeated(c: &[Copper], out: &mut Vec<Fault>) {
    let mut by: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
    for (i, x) in c.iter().enumerate() {
        if let Some(n) = x.number {
            by.entry(n).or_default().push(i);
        }
    }
    for (n, pads) in by {
        if pads.len() < 2 {
            continue;
        }
        // Union-find over the number's pads.
        let mut root: Vec<usize> = (0..pads.len()).collect();
        fn find(r: &mut [usize], mut i: usize) -> usize {
            while r[i] != i {
                r[i] = r[r[i]];
                i = r[i];
            }
            i
        }
        for a in 0..pads.len() {
            for b in a + 1..pads.len() {
                if touch(&c[pads[a]], &c[pads[b]]) != Verdict::No {
                    let (x, y) = (find(&mut root, a), find(&mut root, b));
                    root[x.max(y)] = x.min(y);
                }
            }
        }
        let mut pieces: std::collections::BTreeMap<usize, Vec<&str>> = Default::default();
        for a in 0..pads.len() {
            let r = find(&mut root, a);
            pieces.entry(r).or_default().push(c[pads[a]].id.as_str());
        }
        if pieces.len() < 2 {
            continue;
        }
        let lists: Vec<Vec<&str>> = pieces.into_values().collect();
        out.push(
            Fault::warning(
                Code::FootprintPadNumberRepeated,
                format!(
                    "pad {n}'s {} pads are {} separate pieces of copper",
                    pads.len(),
                    lists.len()
                ),
            )
            .with("pointer", "/pads")
            .with("number", n)
            .with("pads", lists),
        );
    }
}

/// A courtyard, per side, whose graphics don't close (a warning).
fn courtyards(f: &Footprint, out: &mut Vec<Fault>) {
    for side in ["top", "bottom"] {
        let layer = format!("courtyard.{side}");
        let on: Vec<&crate::footprint::Graphic> =
            f.graphics.values().filter(|g| g.layer == layer).collect();
        if let Some((_, true)) = crate::board::courtyard::of_graphics(&on) {
            out.push(
                Fault::warning(
                    Code::FootprintCourtyardOpen,
                    format!("the {side} courtyard doesn't close"),
                )
                .with("pointer", "/graphics")
                .with("side", side),
            );
        }
    }
}

/// No reference designator field (a warning): the board can't write it.
fn reference(f: &Footprint, out: &mut Vec<Fault>) {
    if f.fields.reference.is_none() {
        out.push(
            Fault::warning(
                Code::FootprintNoReference,
                "the footprint has no reference designator field",
            )
            .with("pointer", "/fields"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fpedit::tests::{empty, ok};
    use serde_json::json;

    fn codes(f: &Footprint) -> Vec<Code> {
        lint(f).iter().map(|x| x.error.code).collect()
    }

    /// Two rectangular pads `w` wide (a quantity: `"1"` is a millimetre,
    /// `999999` nanometres), centres 1 mm apart, numbered `a` and `b`, on
    /// `layers`.
    pub(crate) fn two(
        w: serde_json::Value,
        a: &str,
        b: &str,
        layers: serde_json::Value,
    ) -> Footprint {
        let mut f = empty();
        ok(
            &mut f,
            json!([
                {"op": "add", "pad": null, "number": a, "kind": "smd", "shape": "rect", "at": ["-0.5", 0], "size": [w, "0.5"], "layers": layers},
                {"op": "add", "pad": null, "number": b, "kind": "smd", "shape": "rect", "at": ["0.5", 0], "size": [w, "0.5"], "layers": layers},
                {"op": "add", "graphic": null, "type": "rect", "layer": "courtyard.top", "start": ["-2", "1"], "end": ["2", "-1"], "width": "0.05"},
                {"op": "add", "field": "reference", "layer": "silk.top", "at": [0, "1.5"], "size": ["1", "1"]}
            ]),
        );
        f
    }

    #[test]
    fn pads_of_different_numbers_that_touch_overlap() {
        let top = json!(["copper.top"]);
        // 1 mm wide, centres 1 mm apart: they meet edge to edge, exactly.
        assert_eq!(
            codes(&two(json!("1"), "1", "2", top.clone())),
            [Code::FootprintPadsOverlap]
        );
        // A nanometre narrower each: apart.
        assert_eq!(
            codes(&two(json!(999_999), "1", "2", top.clone())),
            Vec::<Code>::new()
        );
        // Same number: not an overlap.
        assert!(
            !codes(&two(json!("1.2"), "1", "1", top.clone())).contains(&Code::FootprintPadsOverlap)
        );
        // Different sides: not an overlap.
        let mut f = two(json!("1.2"), "1", "2", top);
        ok(
            &mut f,
            json!([{"op": "set", "pad": "p2", "layers": ["copper.bottom"]}]),
        );
        assert_eq!(codes(&f), Vec::<Code>::new());
        let found = lint(&two(json!("1.2"), "1", "2", json!(["copper.top"])));
        // A warning (KiCad stacks lands on purpose); a board's DRC judges shorts.
        assert!(!found[0].is_error());
        assert_eq!(found[0].error.details["pads"], json!(["p1", "p2"]));
    }

    #[test]
    fn a_number_on_separate_copper_is_a_warning() {
        let top = json!(["copper.top"]);
        // Same number, apart: two pieces.
        let found = lint(&two(json!("0.5"), "1", "1", top.clone()));
        assert_eq!(
            found.iter().map(|x| x.error.code).collect::<Vec<_>>(),
            [Code::FootprintPadNumberRepeated]
        );
        assert!(!found[0].is_error());
        assert_eq!(found[0].error.details["number"], "1");
        assert_eq!(found[0].error.details["pads"], json!([["p1"], ["p2"]]));
        // Same number, touching: one piece, nothing to say.
        assert_eq!(codes(&two(json!("1.2"), "1", "1", top)), Vec::<Code>::new());
    }

    #[test]
    fn an_open_courtyard_and_no_reference_are_warnings() {
        let mut f = two(json!("0.5"), "1", "2", json!(["copper.top"]));
        ok(
            &mut f,
            json!([
                {"op": "remove", "graphic": "g1"},
                {"op": "add", "graphic": null, "type": "line", "layer": "courtyard.top", "start": ["-2", "1"], "end": ["2", "1"], "width": "0.05"},
                {"op": "remove", "field": "reference"}
            ]),
        );
        let found = lint(&f);
        assert_eq!(
            found.iter().map(|x| x.error.code).collect::<Vec<_>>(),
            [Code::FootprintCourtyardOpen, Code::FootprintNoReference]
        );
        assert!(found.iter().all(|x| !x.is_error()));
        assert_eq!(found[0].error.details["side"], "top");
        // No courtyard at all is the format's own warning, not the lint's.
        // (The line took the freed ID g1.)
        ok(&mut f, json!([{"op": "remove", "graphic": "g1"}]));
        assert_eq!(codes(&f), [Code::FootprintNoReference]);
    }
}
