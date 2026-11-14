//! Where a hole's axis is inside its targets (design 533, Geometry): the
//! stretches of a line through a place, found by cutting the line with
//! each face's surface it may meet (`ryukei::intersect::curve_surface`),
//! then classifying each gap's midpoint with ryukei's certified point
//! classification. Classification decides topology, so no float sign
//! decides it; a classification that stays uncertain is `Uncertain`, and
//! the caller refuses or reports it, never guesses.

use ryukei::geom::{Box3, Curve, Line, Point3, Vector3};
use ryukei::intersect::{CurveHit, Inclusion, curve_surface, face_box, point_in_body};
use ryukei::tol::LINEAR_RESOLUTION;
use ryukei::topo::Body;

/// A line the kernel couldn't place against the body for certain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Uncertain;

/// The stretches `[t_in, t_out]` of the line `from + t * dir` (`dir` a
/// unit vector) inside `body`, in increasing `t`, adjacent ones merged.
/// `t` runs both ways, out past the body's box. Stops for `work`'s cancel.
pub fn exits(
    body: &Body,
    from: Point3,
    dir: Vector3,
    work: &ryukei::work::Work,
) -> Result<Vec<[f64; 2]>, Uncertain> {
    let faces: Vec<_> = body.faces().collect();
    let boxes: Vec<Box3> = faces.iter().map(|f| face_box(body, *f)).collect();
    let Some(all) = boxes.iter().copied().reduce(|a, b| a.union(&b)) else {
        return Ok(Vec::new());
    };
    // Out past every corner of the body's box, both ways.
    let (lo, hi) = (all.min(), all.max());
    let mut reach = 0.0f64;
    for p in [lo, hi] {
        reach = reach.max((p - from).length());
    }
    reach += (hi - lo).length() + 1e-6;
    let line = Line::new(from, dir).map_err(|_| Uncertain)?;
    let curve = Curve::Line(line);
    let (a, b) = (from + dir * -reach, from + dir * reach);
    let pad = Vector3::new(1e-6, 1e-6, 1e-6);
    let lb = Box3::new(
        Point3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)) - pad,
        Point3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)) + pad,
    );
    let mut ts = vec![-reach, reach];
    for (f, bx) in faces.iter().zip(&boxes) {
        if work.cancelled() {
            return Err(Uncertain);
        }
        if !bx.intersects(&lb) {
            continue;
        }
        let Some(surface) = body.face(*f).and_then(|x| body.surface(x.surface)) else {
            continue;
        };
        let hits = curve_surface(&curve, [-reach, reach], surface, LINEAR_RESOLUTION)
            .map_err(|_| Uncertain)?;
        for h in hits {
            match h {
                CurveHit::Point { t, .. } => ts.push(t),
                CurveHit::On { t } => ts.extend(t),
            }
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|b, a| (*b - *a).abs() <= LINEAR_RESOLUTION);
    let mut out: Vec<[f64; 2]> = Vec::new();
    for w in ts.windows(2) {
        let (t0, t1) = (w[0], w[1]);
        if t1 - t0 <= 2.0 * LINEAR_RESOLUTION {
            continue;
        }
        let mid = from + dir * ((t0 + t1) / 2.0);
        match point_in_body(body, mid).map_err(|_| Uncertain)? {
            Inclusion::Inside => match out.last_mut() {
                Some(last) if (last[1] - t0).abs() <= LINEAR_RESOLUTION => last[1] = t1,
                _ => out.push([t0, t1]),
            },
            // On a face along the line, or outside: no material.
            Inclusion::Boundary | Inclusion::Outside => {}
        }
    }
    Ok(out)
}

/// Where the first stretch that reaches past the start ends: a
/// through_next bore's exit, from its mouth.
pub fn first_exit(stretches: &[[f64; 2]]) -> Option<f64> {
    stretches
        .iter()
        .find(|s| s[1] > LINEAR_RESOLUTION)
        .map(|s| s[1])
}

/// Where the last stretch ends: a through-all bore's exit.
pub fn last_exit(stretches: &[[f64; 2]]) -> Option<f64> {
    stretches
        .iter()
        .rev()
        .find(|s| s[1] > LINEAR_RESOLUTION)
        .map(|s| s[1])
}

/// The deepest exit over several bodies (a hole's targets): the first's
/// least, the last's greatest.
pub fn over<'a>(
    bodies: impl IntoIterator<Item = &'a Body>,
    from: Point3,
    dir: Vector3,
    next: bool,
    work: &ryukei::work::Work,
) -> Result<Option<f64>, Uncertain> {
    let mut best: Option<f64> = None;
    for b in bodies {
        let s = exits(b, from, dir, work)?;
        let e = if next { first_exit(&s) } else { last_exit(&s) };
        best = match (best, e) {
            (Some(x), Some(y)) if next => Some(x.min(y)),
            (Some(x), Some(y)) => Some(x.max(y)),
            (x, y) => x.or(y),
        };
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::id::Id;
    use crate::testing::call;
    use serde_json::json;

    #[test]
    fn where_lines_are_inside_a_stepped_body() {
        // Box b (20 x 10 x 5 mm at the origin), a 10 x 10 x 5 mm box
        // joined on its left half, and a gap: a 2 mm slot cut across at
        // x 14 to 16 through the lower box.
        let (_dir, mut s, _) = crate::sketch::tests::with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "up",
            "operation": "join", "targets": ["b"], "params": {"x": 10, "y": 10, "z": 5},
            "placement": {"origin": [0, 0, 5]}}),
        )
        .unwrap();
        let r = s.rebuilt(&Id::parse("p", "part").unwrap()).unwrap();
        let b = r
            .output(&Id::parse("up", "feature").unwrap())
            .unwrap()
            .solid(&Id::parse("b", "feature").unwrap())
            .unwrap()
            .clone();
        let down = Vector3::new(0.0, 0.0, -1.0);
        let at = |x: f64, y: f64, z: f64| Point3::new(x / 1e3, y / 1e3, z / 1e3);
        let close = |got: Vec<[f64; 2]>, want: &[[f64; 2]]| {
            assert_eq!(got.len(), want.len(), "{got:?}");
            for (g, w) in got.iter().zip(want) {
                assert!(
                    (g[0] - w[0] / 1e3).abs() < 1e-12 && (g[1] - w[1] / 1e3).abs() < 1e-12,
                    "{got:?}"
                );
            }
        };
        let work = ryukei::work::Work::unlimited();
        // Through the step: 10 mm of material from the top.
        close(
            exits(&b.body, at(5.0, 5.0, 10.0), down, &work).unwrap(),
            &[[0.0, 10.0]],
        );
        // Beside it: air for 5 mm, then 5 mm of material.
        close(
            exits(&b.body, at(15.0, 5.0, 10.0), down, &work).unwrap(),
            &[[5.0, 10.0]],
        );
        // Starting inside: the stretch runs from behind the start.
        close(
            exits(&b.body, at(5.0, 5.0, 8.0), down, &work).unwrap(),
            &[[-2.0, 8.0]],
        );
        // Missing the body altogether.
        assert!(
            exits(&b.body, at(30.0, 5.0, 10.0), down, &work)
                .unwrap()
                .is_empty()
        );
        // The first exit below a mouth, and the last.
        let e = exits(&b.body, at(5.0, 5.0, 10.0), down, &work).unwrap();
        assert!((first_exit(&e).unwrap() - 0.010).abs() < 1e-12);
        assert!((last_exit(&e).unwrap() - 0.010).abs() < 1e-12);
    }
}
