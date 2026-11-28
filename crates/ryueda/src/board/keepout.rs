//! A board's keepouts (design 924): `keepouts/<id>.json`, an area on
//! copper layers keeping kinds of item out, judged by `drc_keepout`.

use super::drc::{Finding, finding};
use super::file::Contour;
use super::shapes::BoardShapes;
use crate::error::Code;
use crate::fault::Fault;
use crate::fault::Severity;
pub use crate::footprint::Keep;
use ryu_board::geom::{Place, Point, Region};
use ryu_board::model::Polarity;
use ryu_board::shape::Shape;
use ryu_copper::measure::{self, Body, BodyOptions, Verdict};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub use super::file::{KEEPOUT_KIND, KEEPOUTS};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keepout {
    pub contour: Contour,
    /// `top`, `in1` …, `bottom`, or `all`.
    pub layers: Vec<String>,
    pub keeps: Vec<Keep>,
}

impl Keepout {
    /// The copper layers it covers on a board of `n`, by index, sorted.
    pub fn indices(&self, n: usize) -> Vec<usize> {
        let mut out: Vec<usize> = if self.layers.iter().any(|l| l == "all") {
            (0..n).collect()
        } else {
            self.layers
                .iter()
                .filter_map(|l| super::file::CopperLayer::parse(l)?.index(n))
                .collect()
        };
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// A keepout's faults on a board of `n` copper layers.
pub fn judge(k: &Keepout, n: usize) -> Vec<Fault> {
    let mut out = Vec::new();
    let bad = |why: &str| {
        Fault::error(
            Code::BoardContourInvalid,
            format!("a keepout's contour {why}"),
        )
        .with("pointer", "/contour")
    };
    match super::outline::contour(&k.contour) {
        Err(super::outline::Bad::OutOfRange) => out.push(bad("reaches past ±2^40 nm")),
        Err(super::outline::Bad::Invalid(why)) => out.push(bad(why)),
        Ok(c) => {
            let r = super::outline::ring(&c);
            if r.len() < 3 || !super::outline::simple(&r) {
                out.push(bad("crosses or touches itself"));
            } else if c.polygon_area2().unwrap_or(1) == 0 {
                out.push(bad("has no area"));
            }
        }
    }
    if k.keeps.is_empty() {
        out.push(
            Fault::error(Code::InvalidDocument, "a keepout keeps something out")
                .with("pointer", "/keeps"),
        );
    }
    if k.layers.is_empty() {
        out.push(
            Fault::error(Code::InvalidDocument, "a keepout is on a layer")
                .with("pointer", "/layers"),
        );
    }
    for l in &k.layers {
        if l == "all" {
            continue;
        }
        if super::file::CopperLayer::parse(l)
            .and_then(|c| c.index(n))
            .is_none()
            || l.starts_with("copper.")
        {
            out.push(
                Fault::error(
                    Code::BoardLayerNotFound,
                    format!(
                        "the board has {n} copper layers and no {l}; they are {}, or all",
                        super::load::layer_list(n)
                    ),
                )
                .with("pointer", "/layers")
                .with("layer", l),
            );
        }
    }
    out
}

/// A footprint's keepout where its part puts it: its polygon placed, and
/// its footprint layers (`copper.top`, `copper.bottom`, `copper.inner`,
/// `copper.all`) as the board's layer indices, top and bottom swapped for
/// a part on the bottom.
pub fn placed_footprint_keepout(
    k: &crate::footprint::Keepout,
    place: Place,
    n: usize,
) -> (Vec<usize>, Shape) {
    let flipped = place.mirror;
    let mut layers = Vec::new();
    for l in &k.layers {
        match l.as_str() {
            "copper.top" => layers.push(if flipped { n - 1 } else { 0 }),
            "copper.bottom" => layers.push(if flipped { 0 } else { n - 1 }),
            "copper.inner" => layers.extend(1..n - 1),
            "copper.all" => layers.extend(0..n),
            _ => {}
        }
    }
    layers.sort_unstable();
    layers.dedup();
    let pts: Vec<Point> = k.polygon.iter().map(|p| Point::new(p[0], p[1])).collect();
    let shape = ryu_board::shape::polygon(&pts)
        .ok()
        .map(|c| {
            if c.polygon_area2().unwrap_or(0) < 0 {
                c.reversed()
            } else {
                c
            }
        })
        .map(|c| {
            Shape::regions(Polarity::Positive, [Region::solid(c)])
                .placed(&place, Polarity::Positive)
        })
        .unwrap_or_default();
    (layers, shape)
}

fn keeps(k: &super::shapes::KeepoutInfo, what: Keep) -> bool {
    k.keeps.contains(&what)
        || (k.keeps.contains(&Keep::Copper)
            && matches!(what, Keep::Tracks | Keep::Vias | Keep::Pads | Keep::Pours))
}

/// `drc_keepout`: each item of a kept kind meeting a keepout on one of its layers.
pub fn check(s: &BoardShapes, board: &str, scope: super::drc::Scope) -> Vec<Finding> {
    let n = s.n();
    let opts = BodyOptions::default();
    let mut out = Vec::new();
    for k in &s.keepouts {
        let area = Body::of_shape(&k.shape, &opts);
        let on = |l: usize| k.layers.contains(&l);
        let own = |occ: &str| k.part.as_deref() == Some(occ);
        let dirty_keepout = scope.reference(&k.reference);
        let mut meet = |reference: String, shape: &Shape| {
            if !dirty_keepout && !scope.reference(&reference) {
                return;
            }
            let v = measure::touches(&area, &Body::of_shape(shape, &opts));
            if v != Verdict::No {
                out.push(finding(
                    Code::DrcKeepout,
                    Severity::Error,
                    board,
                    vec![k.reference.clone(), reference.clone()],
                    format!("{reference} is inside keepout {}", k.reference),
                    json!({"uncertain": v == Verdict::Uncertain, "keeps": k.keeps}),
                ));
            }
        };
        if keeps(k, Keep::Tracks) {
            for t in s.tracks.iter().filter(|t| t.layer.index(n).is_some_and(on)) {
                meet(format!("{board}:track/{}", t.id), &t.shape);
            }
        }
        if keeps(k, Keep::Vias) {
            for v in s.vias.iter().filter(|v| (v.from..=v.to).any(on)) {
                meet(format!("{board}:via/{}", v.id), &v.shape);
            }
        }
        if keeps(k, Keep::Pads) {
            for p in s
                .pads
                .iter()
                .filter(|p| !own(&p.occurrence) && (0..n).any(|l| on(l) && p.on(l, n)))
            {
                meet(p.reference.clone(), &p.copper);
            }
        }
        if keeps(k, Keep::Pours) {
            for f in s.pours.iter().filter(|f| on(f.layer)) {
                for r in &f.regions {
                    meet(format!("{board}:pour/{}", f.id), r);
                }
            }
        }
        if keeps(k, Keep::Holes) {
            for h in s.holes.iter().filter(|h| (h.span.0..=h.span.1).any(on)) {
                if let super::shapes::HoleOf::Pad { occ, .. } = &h.of
                    && own(occ)
                {
                    continue;
                }
                let shape = crate::board::render::hole_shape(&h.shape);
                meet(format!("{}#hole", h.owner), &shape);
            }
        }
        if keeps(k, Keep::Placements) {
            for c in s.courtyards.iter().filter(|c| !own(&c.occurrence)) {
                let outer = if c.bottom { n - 1 } else { 0 };
                if on(outer) {
                    meet(c.owner.clone(), &c.shape);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::file::{Contour, Op};
    use crate::board::shapes::KeepoutInfo;
    use crate::board::testing::{MM, UM, board, rect};
    use crate::doc;
    use serde_json::json;

    fn ko(keeps: Vec<Keep>, layers: Vec<usize>, part: Option<&str>) -> KeepoutInfo {
        KeepoutInfo {
            reference: "b:keepout/k1".into(),
            part: part.map(Into::into),
            layers,
            keeps,
            shape: rect(ryu_board::geom::Point::new(10 * MM, 5 * MM), 2 * MM, 2 * MM),
        }
    }

    #[test]
    fn kept_kinds_meeting_a_keepout_are_findings_and_others_arent() {
        let mut s = board(20 * MM, 10 * MM);
        s.via("v1", "a", (10 * MM, 5 * MM), 300 * UM, 600 * UM)
            .smd("r1", "1", (12 * MM, 5 * MM), (MM, MM), Some("b"))
            .courtyard("r1", (12 * MM, 5 * MM), (2 * MM, 2 * MM), false);
        s.keepouts.push(ko(vec![Keep::Vias], vec![0], None));
        let f = check(&s, "b", crate::board::drc::Scope::All);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].code, Code::DrcKeepout);
        assert_eq!(f[0].refs, ["b:keepout/k1", "b:via/v1"]);
        // The pad's edge (11.5 mm) is clear of the keepout (to 11 mm):
        // pads aren't kept, and the courtyard (from 11 mm) touches it.
        s.keepouts[0] = ko(vec![Keep::Pads], vec![0], None);
        assert!(check(&s, "b", crate::board::drc::Scope::All).is_empty());
        s.keepouts[0] = ko(vec![Keep::Placements], vec![0], None);
        assert_eq!(
            check(&s, "b", crate::board::drc::Scope::All)[0].refs,
            ["b:keepout/k1", "b:r1"]
        );
        // On the bottom only: a top part and a top-only via are clear.
        s.keepouts[0] = ko(vec![Keep::Placements, Keep::Tracks], vec![1], None);
        assert!(check(&s, "b", crate::board::drc::Scope::All).is_empty());
        // A part's own keepout leaves its own pads alone.
        s.keepouts[0] = ko(vec![Keep::Copper], vec![0], Some("r1"));
        let f = check(&s, "b", crate::board::drc::Scope::All);
        assert_eq!(
            f.iter().map(|x| x.refs[1].as_str()).collect::<Vec<_>>(),
            ["b:via/v1"]
        );
    }
    #[test]
    fn a_footprints_keepout_is_placed_and_its_layers_swap_on_the_bottom() {
        let k = crate::footprint::Keepout {
            polygon: vec![[0, 0], [10, 0], [10, 10], [0, 10]],
            keeps: vec![Keep::Tracks],
            layers: vec!["copper.top".into()],
        };
        let top = ryu_board::geom::Place {
            at: ryu_board::geom::Point::new(100, 0),
            ..Default::default()
        };
        let (l, s) = placed_footprint_keepout(&k, top, 4);
        assert_eq!(l, vec![0]);
        assert_eq!(s.bounds().unwrap().min.x, 100);
        let bottom = ryu_board::geom::Place {
            mirror: true,
            ..top
        };
        let (l, s) = placed_footprint_keepout(&k, bottom, 4);
        assert_eq!(
            l,
            vec![3],
            "copper.top of a bottom part is the board's bottom"
        );
        assert_eq!(
            s.bounds().unwrap().max.x,
            100,
            "mirrored about the part's y axis"
        );
        let all = crate::footprint::Keepout {
            layers: vec!["copper.inner".into()],
            ..k
        };
        assert_eq!(placed_footprint_keepout(&all, top, 4).0, vec![1, 2]);
    }

    fn square() -> Contour {
        Contour {
            start: [0, 0],
            edges: vec![
                Op::Line { to: [10, 0] },
                Op::Line { to: [10, 10] },
                Op::Line { to: [0, 10] },
                Op::Line { to: [0, 0] },
            ],
        }
    }

    #[test]
    fn a_fill_inside_a_keepout_keeping_pours_is_a_finding() {
        use crate::board::pour::PourInfo;
        let mut s = board(20 * MM, 10 * MM);
        let pr = crate::board::rules::Rules::from_parts(
            std::sync::Arc::new(crate::board::preset::JLC_2L),
            None,
            Default::default(),
        )
        .pour(&crate::board::pour::Pour::whole(
            "gnd",
            crate::board::file::CopperLayer::Top,
        ));
        s.pours.push(PourInfo {
            id: crate::id::Id::parse("p1", "pour").unwrap(),
            net: "gnd".into(),
            layer: 0,
            rules: pr,
            polygons: Default::default(),
            regions: vec![rect(ryu_board::geom::Point::new(10 * MM, 5 * MM), MM, MM)],
            islands_removed: 0,
            uncertain: vec![],
            thermals: vec![],
            tiled: Default::default(),
            places: vec![(0, 0, 0)],
            grown: Default::default(),
        });
        s.keepouts.push(ko(vec![Keep::Pours], vec![0], None));
        let f = check(&s, "b", crate::board::drc::Scope::All);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].refs.contains(&"b:pour/p1".to_owned()));
        s.keepouts[0].keeps = vec![Keep::Vias];
        assert!(check(&s, "b", crate::board::drc::Scope::All).is_empty());
    }

    #[test]
    fn a_keepout_reads_writes_and_is_judged() {
        let v = json!({"kind": KEEPOUT_KIND, "format": 1,
            "contour": {"start": [0, 0], "edges": [{"type": "line", "to": [10, 0]}, {"type": "line", "to": [10, 10]},
                        {"type": "line", "to": [0, 10]}, {"type": "line", "to": [0, 0]}]},
            "layers": ["all"], "keeps": ["placements", "vias"]});
        let k: Keepout = doc::from_value(v.clone(), "k.json", KEEPOUT_KIND, 1).unwrap();
        assert_eq!(doc::to_value(&k, KEEPOUT_KIND, 1), v);
        assert!(judge(&k, 2).is_empty());
        let open = Keepout {
            contour: Contour {
                edges: vec![Op::Line { to: [10, 0] }, Op::Line { to: [10, 10] }],
                ..square()
            },
            ..k.clone()
        };
        assert_eq!(judge(&open, 2)[0].error.code, Code::BoardContourInvalid);
        let bow = Keepout {
            contour: Contour {
                start: [0, 0],
                edges: vec![
                    Op::Line { to: [10, 10] },
                    Op::Line { to: [10, 0] },
                    Op::Line { to: [0, 10] },
                    Op::Line { to: [0, 0] },
                ],
            },
            ..k.clone()
        };
        assert_eq!(judge(&bow, 2)[0].error.code, Code::BoardContourInvalid);
        let inner = Keepout {
            layers: vec!["in1".into()],
            ..k.clone()
        };
        assert_eq!(judge(&inner, 2)[0].error.code, Code::BoardLayerNotFound);
        assert!(judge(&inner, 4).is_empty());
        let none = Keepout { keeps: vec![], ..k };
        assert_eq!(judge(&none, 2)[0].error.code, Code::InvalidDocument);
    }

    #[test]
    fn layer_names_cover_the_stackup() {
        let k = |l: &[&str]| Keepout {
            contour: square(),
            layers: l.iter().map(|x| x.to_string()).collect(),
            keeps: vec![Keep::Tracks],
        };
        assert_eq!(k(&["all"]).indices(4), vec![0, 1, 2, 3]);
        assert_eq!(k(&["top", "bottom"]).indices(4), vec![0, 3]);
        assert_eq!(k(&["in2"]).indices(4), vec![2]);
    }
}
