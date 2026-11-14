//! The window's look is `ryu_app::style`'s (ryueda 3.C). Here are the
//! colours only ryucad's viewport paints: the chosen feature, a split's
//! sides, a frame's axes and a compare's, the last the same as
//! `ryucad render --compare`'s.

pub use ryu_app::style::*;

/// The feature chosen in the tree.
pub const CHOSEN: Rgb = [0.62, 0.68, 0.79];
/// A split chosen in the tree: the side behind its plane, which its
/// target keeps, and the side in front, the solid it founds.
pub const SPLIT_BEHIND: Rgb = [0.86, 0.62, 0.30];
pub const SPLIT_FRONT: Rgb = [0.55, 0.40, 0.80];
/// A frame's x, y and z axes, as CAD draws them: red, green, blue.
pub const AXES: [Rgb; 3] = [[0.85, 0.20, 0.20], [0.20, 0.65, 0.25], [0.20, 0.40, 0.90]];

/// A compare's colours, the same as `ryucad render --compare`'s: faces
/// only the working files have, faces on both sides whose geometry
/// differs, faces only the other side has (in its ghost), and the rest of
/// the ghost.
pub use ryucad::render::{
    COMPARE_CHANGED as CHANGED, COMPARE_GHOST as GHOST, COMPARE_NEW as NEW,
    COMPARE_REMOVED as REMOVED,
};

#[cfg(test)]
mod tests {
    use super::*;

    const LOOKS: [Look; 3] = [Look::Drafting, Look::Graphite, Look::Studio];
    const CONTRASTS: [Contrast; 2] = [Contrast::Normal, Contrast::High];

    #[test]
    fn backgrounds_keep_silhouettes_visible() {
        let lum = |c: Rgb| (c[0] + c[1] + c[2]) / 3.0;
        for look in LOOKS {
            let (l, d) = (
                look.viewport_background(false, Contrast::Normal),
                look.viewport_background(true, Contrast::Normal),
            );
            assert!(lum(l) > 0.85, "{look:?}");
            assert!(lum(d) < 0.3, "{look:?}");
            assert!(lum(d) > lum(ryucad::render::EDGE) + 0.05, "{look:?}");
        }
    }

    /// Design 199's colours that must stay apart, for normal vision and
    /// simulated protanopia, deuteranopia and tritanopia: the selection,
    /// hover and chosen colours against each other and every colour
    /// below; each group's colours against each other (compare's, the
    /// sketch states', a split's two sides', the participants'); and each against every
    /// viewport background and the model's own colour, which they paint
    /// over. CIEDE2000 at least 10 for every pair.
    #[test]
    fn colours_stay_apart_for_colour_blind_eyes() {
        use ryucad::render as r;
        let groups: [(&str, Vec<(&str, Rgb)>); 5] = [
            (
                "selection",
                vec![("selected", SELECTED), ("hover", HOVER), ("chosen", CHOSEN)],
            ),
            (
                "compare",
                vec![
                    ("new", r::COMPARE_NEW),
                    ("changed", r::COMPARE_CHANGED),
                    ("removed", r::COMPARE_REMOVED),
                    ("ghost", r::COMPARE_GHOST),
                ],
            ),
            (
                "sketch",
                vec![
                    ("well", r::SKETCH_WELL),
                    ("under", r::SKETCH_UNDER),
                    ("over", r::SKETCH_OVER),
                    ("failing", r::SKETCH_FAILING),
                ],
            ),
            (
                "split",
                vec![("behind", SPLIT_BEHIND), ("front", SPLIT_FRONT)],
            ),
            (
                "participants",
                PALETTE
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (["p0", "p1", "p2", "p3", "p4", "p5"][i], *c))
                    .collect(),
            ),
        ];
        let mut grounds: Vec<(String, Rgb)> = LOOKS
            .iter()
            .flat_map(|l| {
                CONTRASTS.iter().flat_map(move |c| {
                    [false, true].map(|d| {
                        (
                            format!("{l:?} {c:?} dark={d}"),
                            l.viewport_background(d, *c),
                        )
                    })
                })
            })
            .collect();
        grounds.push(("the model".into(), r::SURFACE));
        let mut fails = Vec::new();
        for (vision, m) in vision::VISIONS {
            let mut check = |a: &str, x: Rgb, b: &str, y: Rgb| {
                let d = vision::apart(x, y, &m);
                if d < 10.0 {
                    fails.push(format!("{vision}: {a} against {b} is {d:.1}"));
                }
            };
            for (g, colours) in &groups {
                for (i, (a, x)) in colours.iter().enumerate() {
                    for (b, y) in &colours[i + 1..] {
                        check(a, *x, b, *y);
                    }
                    for (ground, y) in &grounds {
                        check(a, *x, ground, *y);
                    }
                    // The selection paints over everything.
                    if *g == "selection" {
                        for (h, others) in &groups {
                            if h != g {
                                for (b, y) in others {
                                    check(a, *x, b, *y);
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }
}
