//! Picking (design 519, Pointing and highlighting): which stable
//! reference is under a sheet point, frontmost first, within `reach`
//! millimetres. On the CPU, over the scene the window already holds: the
//! daemon is asked nothing until a selection is shared.

use crate::core::scene::{Anchor, P, Scene, Shape};
use ryu_app::canvas::{bezier_at, inside, polyline_distance, segment_distance};

/// An owner kind's place in the picking order: lower is in front.
pub fn rank(kind: &str) -> u8 {
    match kind {
        "pin" => 0,
        "label" | "port" | "rail" => 1,
        "component" => 2,
        "net" | "bus" => 3,
        "instance" => 4,
        "region" => 5,
        _ => 6,
    }
}

/// The box a text covers: its width along, from a quarter of its size
/// below the baseline to three quarters above; turned a quarter
/// counterclockwise when vertical.
pub fn text_box(at: P, size: f64, width: f64, anchor: Anchor, vertical: bool) -> [P; 2] {
    let x0 = match anchor {
        Anchor::Start => 0.0,
        Anchor::Middle => -width / 2.0,
        Anchor::End => -width,
    };
    let (lo, hi) = ([x0, -0.25 * size], [x0 + width, 0.75 * size]);
    if vertical {
        [
            [at[0] - hi[1], at[1] + lo[0]],
            [at[0] - lo[1], at[1] + hi[0]],
        ]
    } else {
        [
            [at[0] + lo[0], at[1] + lo[1]],
            [at[0] + hi[0], at[1] + hi[1]],
        ]
    }
}

/// How far `p` is from box `b`: 0 inside.
pub fn box_distance(p: P, b: [P; 2]) -> f64 {
    let dx = (b[0][0] - p[0]).max(p[0] - b[1][0]).max(0.0);
    let dy = (b[0][1] - p[1]).max(p[1] - b[1][1]).max(0.0);
    ryu_math::hypot(dx, dy)
}

fn bounds(pts: &[P]) -> Option<[P; 2]> {
    let first = *pts.first()?;
    Some(pts.iter().fold([first, first], |[lo, hi], p| {
        [
            [lo[0].min(p[0]), lo[1].min(p[1])],
            [hi[0].max(p[0]), hi[1].max(p[1])],
        ]
    }))
}

fn rect_edges(r: [P; 2]) -> [P; 5] {
    [r[0], [r[1][0], r[0][1]], r[1], [r[0][0], r[1][1]], r[0]]
}

/// How far `p` is from what `shape` draws (its pen's half width taken
/// off); 0 inside a filled shape or a text's box.
pub fn distance(shape: &Shape, p: P) -> f64 {
    let pen = |d: f64, w: f64| (d - w / 2.0).max(0.0);
    match shape {
        Shape::Polyline {
            points,
            fill,
            width,
            ..
        } => {
            if fill.is_some() && bounds(points).is_some_and(|b| inside(p, b, 0.0)) {
                return 0.0;
            }
            pen(polyline_distance(p, points), *width)
        }
        Shape::Bus { points, width, .. } => pen(polyline_distance(p, points), *width),
        Shape::Circle {
            center,
            radius,
            fill,
            width,
            ..
        } => {
            let d = ryu_math::hypot(p[0] - center[0], p[1] - center[1]);
            if fill.is_some() && d <= *radius {
                0.0
            } else {
                pen((d - radius).abs(), *width)
            }
        }
        Shape::Arc {
            start,
            mid,
            end,
            width,
            ..
        } => pen(polyline_distance(p, &[*start, *mid, *end]), *width),
        Shape::Bezier { points, width, .. } => {
            let w = vec![1.0; points.len()];
            let line: Vec<P> = (0..=16)
                .map(|i| bezier_at(points, &w, f64::from(i) / 16.0))
                .collect();
            pen(polyline_distance(p, &line), *width)
        }
        Shape::Frame {
            rect,
            title_tab,
            width,
            ..
        } => {
            if title_tab.is_some_and(|t| inside(p, t, 0.0)) {
                return 0.0;
            }
            pen(polyline_distance(p, &rect_edges(*rect)), *width)
        }
        Shape::Text {
            at,
            size,
            anchor,
            vertical,
            width,
            ..
        } => box_distance(p, text_box(*at, *size, *width, *anchor, *vertical)),
    }
}

/// The ghost (what only the compared revision draws) under `p` within
/// `reach`, nearest first.
pub fn pick_ghost(scene: &Scene, p: P, reach: f64) -> Option<String> {
    let mut best: Option<(f64, usize)> = None;
    for it in &scene.items {
        let Some(g) = it.ghost else { continue };
        if let Some(b) = scene.ghosts[g].boxed
            && !inside(p, b, reach)
        {
            continue;
        }
        let d = distance(&it.shape, p);
        if d <= reach && best.is_none_or(|(x, _)| d < x) {
            best = Some((d, g));
        }
    }
    best.map(|(_, g)| scene.ghosts[g].reference.clone())
}

/// The reference under `p`, frontmost first and nearest within a rank,
/// within `reach`; with `alt`, a pin's net instead of the pin.
pub fn pick(scene: &Scene, p: P, reach: f64, alt: bool) -> Option<String> {
    let mut best: Option<((u8, f64), String)> = None;
    let mut offer = |key: (u8, f64), r: &str| {
        if key.1 <= reach
            && best
                .as_ref()
                .is_none_or(|(b, _)| key.0 < b.0 || (key.0 == b.0 && key.1 < b.1))
        {
            best = Some((key, r.to_owned()));
        }
    };
    for pin in &scene.pins {
        let d = segment_distance(p, pin.base, pin.at);
        let r = match (&pin.net, alt) {
            (Some(net), true) => net.as_str(),
            _ => pin.reference.as_str(),
        };
        // A pin is in front only on its line (a third of the reach, 2
        // screen points); farther off it is a part's, nearest winning, so
        // a small part's body can be clicked between its pins.
        let rank = if d <= reach / 3.0 {
            rank("pin")
        } else {
            rank("component")
        };
        offer((rank, d), r);
    }
    for it in &scene.items {
        let Some(o) = it.owner.and_then(|i| scene.owners.get(i)) else {
            continue;
        };
        if let Some(b) = o.boxed
            && !inside(p, b, reach)
        {
            continue;
        }
        offer((rank(&o.kind), distance(&it.shape, p)), &o.reference);
    }
    best.map(|(_, r)| r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scene::{Scene, tests::small};

    #[test]
    fn frontmost_first_then_nearest() {
        let s = Scene::from_json(&small()).unwrap();
        let reach = 0.5;
        // On pin 1's end, which the wire and its junction also touch.
        assert_eq!(
            pick(&s, [0.0, 0.0], reach, false).as_deref(),
            Some("top/r1/1")
        );
        // Alt: the pin's net instead.
        assert_eq!(
            pick(&s, [0.0, 0.0], reach, true).as_deref(),
            Some("top/sda")
        );
        // Inside the resistor's filled body.
        assert_eq!(
            pick(&s, [5.0, 0.0], reach, false).as_deref(),
            Some("top/r1")
        );
        // On the body near pin 2's root: within reach of the pin, but on
        // the part, which wins (a two-pin part's body can be clicked).
        assert_eq!(
            pick(&s, [7.3, 0.0], reach, false).as_deref(),
            Some("top/r1")
        );
        // On pin 2's line: the pin.
        assert_eq!(
            pick(&s, [8.9, 0.0], reach, false).as_deref(),
            Some("top/r1/2")
        );
        // On the wire, away from the pin.
        assert_eq!(
            pick(&s, [-3.0, 0.1], reach, false).as_deref(),
            Some("top/sda")
        );
        // On the label's text.
        assert_eq!(
            pick(&s, [-6.0, 0.9], reach, false).as_deref(),
            Some("top/label:sda@1")
        );
        // On the region's frame edge, and on its title tab.
        assert_eq!(
            pick(&s, [20.0, 5.0], reach, false).as_deref(),
            Some("top/region:power")
        );
        assert_eq!(
            pick(&s, [-10.0, 11.5], reach, false).as_deref(),
            Some("top/region:power")
        );
        // Empty paper inside the region: nothing.
        assert_eq!(pick(&s, [15.0, -7.0], reach, false), None);
        // The paper's border is owned by no one.
        assert_eq!(pick(&s, [100.0, -20.0], reach, false), None);
    }

    #[test]
    fn hit_shapes() {
        assert_eq!(rank("pin"), 0);
        assert!(rank("label") < rank("component") && rank("component") < rank("net"));
        assert!(rank("instance") < rank("region") && rank("region") < rank("note"));
        let b = text_box([0.0, 0.0], 1.0, 4.0, Anchor::End, false);
        assert_eq!(b, [[-4.0, -0.25], [0.0, 0.75]]);
        let v = text_box([0.0, 0.0], 1.0, 4.0, Anchor::Start, true);
        assert_eq!(v, [[-0.75, 0.0], [0.25, 4.0]]);
        assert_eq!(box_distance([5.0, 0.0], [[0.0, 0.0], [2.0, 2.0]]), 3.0);
        assert_eq!(box_distance([1.0, 1.0], [[0.0, 0.0], [2.0, 2.0]]), 0.0);
    }

    #[test]
    fn a_ghost_is_picked_only_where_nothing_working_is_nearer() {
        use crate::core::scene::tests::compared;
        let s = Scene::from_json(&compared()).unwrap();
        // On the ghost's line, far from anything working.
        assert_eq!(
            pick(&s, [31.0, 1.0], 0.5, false),
            None,
            "nothing working there"
        );
        assert_eq!(
            pick_ghost(&s, [31.27, 1.27], 0.5).as_deref(),
            Some("top/c3")
        );
        assert_eq!(pick_ghost(&s, [100.0, 100.0], 0.5), None);
    }
}
