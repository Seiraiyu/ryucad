//! A board's drawings (design 924): `drawings/<id>.json`, the footprint
//! format's `Graphic` (a layer and one shape, text included) on the
//! board's non-copper layers. Text is centred on its place, and mirrored
//! about it on a bottom layer.

use crate::error::Code;
use crate::fault::Fault;
pub use crate::footprint::Graphic;
use crate::footprint::Shape as Drawn;
use ryu_board::geom::{Mdeg, Place, Point};
use ryu_board::model::Polarity;
use ryu_board::shape::Shape;

pub use super::file::{DRAWING_KIND, DRAWINGS};

/// Whether a drawing may go on `layer`: silk, fab and mask, either side,
/// and the documentation and user layers.
pub fn layer_ok(layer: &str) -> bool {
    if !crate::footprint::layer_known(layer) {
        return false;
    }
    let kind = layer.split('.').next().unwrap_or(layer);
    !matches!(
        kind,
        "copper" | "paste" | "courtyard" | "adhesive" | "edge" | "margin"
    )
}

/// A drawing's faults.
pub fn judge(g: &Graphic) -> Vec<Fault> {
    let mut out = Vec::new();
    if !layer_ok(&g.layer) {
        out.push(
            Fault::error(
                Code::InvalidDocument,
                format!("a board drawing can't go on {}: silk, fab and mask (top or bottom), drawings, comments, eco.1, eco.2 or user.1 to user.45; copper is tracks and pours", g.layer),
            )
            .with("pointer", "/layer"),
        );
    }
    if let Some(why) = crate::footprint::undrawable(&g.shape) {
        out.push(
            Fault::error(
                Code::InvalidDocument,
                format!("a drawing that can't be drawn: {why}"),
            )
            .with("pointer", "/"),
        );
    }
    out
}

/// A drawing's shape on the board, and the characters the font lacks.
pub fn shape(g: &Graphic) -> Option<(Shape, Vec<char>)> {
    let s = match &g.shape {
        Drawn::Text {
            text,
            at,
            rotation_mdeg,
            size_nm,
            thickness_nm,
            hidden,
        } => {
            if *hidden {
                return None;
            }
            let bottom = g.layer.ends_with(".bottom");
            let at0 = super::text::centred(text, [0, 0], 0, *size_nm, *thickness_nm).ok()?;
            let place = Place {
                at: Point::new(at[0], at[1]),
                rot: Mdeg::new(if bottom {
                    -rotation_mdeg
                } else {
                    *rotation_mdeg
                }),
                mirror: bottom,
                scale: None,
            };
            let missing = at0.missing.clone();
            return Some((at0.placed(&place, Polarity::Positive), missing));
        }
        other => super::graphic::drawn(other),
    };
    (!s.parts.is_empty()).then_some((s, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::Shape as Drawn;

    fn text(layer: &str) -> Graphic {
        Graphic {
            layer: layer.into(),
            shape: Drawn::Text {
                text: "HH".into(),
                at: [10_000_000, 10_000_000],
                rotation_mdeg: 0,
                size_nm: [1_000_000, 1_000_000],
                thickness_nm: 150_000,
                hidden: false,
            },
        }
    }

    #[test]
    fn drawings_go_on_non_copper_layers_only() {
        for ok in [
            "silk.top",
            "silk.bottom",
            "fab.top",
            "mask.bottom",
            "drawings",
            "comments",
            "eco.1",
            "user.7",
        ] {
            assert!(judge(&text(ok)).is_empty(), "{ok}");
        }
        for bad in [
            "copper.top",
            "copper.all",
            "paste.top",
            "courtyard.top",
            "edge",
            "margin",
            "adhesive.top",
            "silk",
        ] {
            let f = judge(&text(bad));
            assert_eq!(f[0].error.code, Code::InvalidDocument, "{bad}");
            assert_eq!(f[0].error.details["pointer"], "/layer");
        }
        let neg = Graphic {
            layer: "silk.top".into(),
            shape: Drawn::Line {
                start: [0, 0],
                end: [1, 1],
                width_nm: -1,
            },
        };
        assert_eq!(judge(&neg)[0].error.details["pointer"], "/");
    }

    #[test]
    fn text_on_the_bottom_is_its_top_twin_mirrored_about_its_place() {
        let (top, _) = shape(&text("silk.top")).unwrap();
        let (bottom, _) = shape(&text("silk.bottom")).unwrap();
        let (t, b) = (top.bounds().unwrap(), bottom.bounds().unwrap());
        assert_eq!(
            (b.min.x, b.max.x),
            (20_000_000 - t.max.x, 20_000_000 - t.min.x)
        );
        assert_eq!((b.min.y, b.max.y), (t.min.y, t.max.y));
    }
}
