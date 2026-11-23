//! A board's keepouts (design 924): `keepouts/<id>.json`, an area on
//! copper layers keeping kinds of item out, judged by `drc_keepout`.

use super::file::Contour;
use crate::error::Code;
use crate::fault::Fault;
pub use crate::footprint::Keep;
use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::file::{Contour, Op};
    use crate::doc;
    use serde_json::json;

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
