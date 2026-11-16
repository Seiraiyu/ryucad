//! Cosmetic threads in views (design 533, Drawings), as ISO 6410-1 and
//! ASME Y14.6 draw them simplified: seen end on, a thin three-quarter
//! circle at the thread's ring diameter (an internal thread's major, an
//! external one's minor), open in its first quadrant, about its mouth;
//! seen side on, two thin lines at that diameter from its mouth to its
//! end. A section draws them side on over the caps' hatching, which for a
//! tapped hole already reaches the drilled wall. A thread is drawn as its
//! wall is: visible where any of its faces or their edges is drawn
//! visible, hidden (dashed, when the view draws hidden lines) where they
//! are only hidden, not at all where none is drawn. Not drawn seen
//! obliquely, in a detail unless wholly inside its circle, or in a flat
//! view. Its lines count as no piece and leave the view's box as it is.

use super::{Drawn, Extra, Line, Plan, Source, annotate};
use crate::doc::assembly::Rev;
use crate::doc::drawing::Hidden;
use crate::doc::id::Id;
use crate::session::Session;
use ryu_draw::{LineKind, P, Path, Segment};
use std::collections::BTreeMap;
use std::sync::Arc;

/// How near the line of sight (or across it) an axis must lie to be seen
/// end on (side on), as `Map::end_on` and `in_plane` judge.
const ALONG: f64 = 1e-9;

/// Whether a thread on faces `names` of body `i` is drawn visible, hidden
/// or not at all: by what the view drew of those faces and their edges.
fn shown(d: &Drawn, src: &Source, i: usize, names: &[&str]) -> Option<LineKind> {
    let body = &src[i].0.body;
    let mut all: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
    for f in body.faces() {
        let Some(face) = body.face(f) else { continue };
        if !names.contains(&face.name.to_string().as_str()) {
            continue;
        }
        for l in body.face_loops(f) {
            for fin in body.loop_fins(l) {
                if let Some(e) = body.fin(fin).and_then(|x| body.edge(x.edge)) {
                    all.push(e.name.to_string());
                }
            }
        }
    }
    let (mut visible, mut hidden) = (false, false);
    for n in &all {
        if let Some(x) = d.named.get(&(i, n.clone())) {
            visible |= x.visible;
            hidden |= x.hidden;
        }
    }
    match (visible, hidden) {
        (true, _) => Some(LineKind::Thin),
        (false, true) => Some(LineKind::Hidden),
        _ => None,
    }
}

/// The thin lines of every cosmetic thread the view's solids carry, in
/// the view's frame (model metres from the anchor's image), each naming
/// its first wall face.
pub(super) fn lines(session: &mut Session, plan: &Plan, src: &Source, d: &Drawn) -> Vec<Line> {
    if matches!(plan.extra, Extra::Flat { .. }) {
        return Vec::new();
    }
    let c = plan.frame.image(plan.anchor);
    let img = |p: ryukei::geom::Point3| -> P {
        let i = plan.frame.image([p.x, p.y, p.z]);
        [i[0] - c[0], i[1] - c[1]]
    };
    let mut holes: BTreeMap<(Id, Rev), Option<Arc<crate::holes::Holes>>> = BTreeMap::new();
    let mut out = Vec::new();
    for (i, (_, pose, label)) in src.iter().enumerate() {
        let founder = label.rsplit('/').next().unwrap_or_default();
        let Ok((part, rev)) = annotate::part_of(session, plan, src, i) else {
            continue;
        };
        let got = holes
            .entry((part.clone(), rev.clone()))
            .or_insert_with(|| crate::holes::at(session, &part, &rev).ok())
            .clone();
        let Some(h) = got else { continue };
        for t in crate::thread::of(&h) {
            let names: Vec<&str> = t
                .faces
                .iter()
                .filter_map(|f| {
                    let mut it = f.splitn(3, '/');
                    let (_, solid, name) = (it.next(), it.next()?, it.next()?);
                    (solid == founder).then_some(name)
                })
                .collect();
            if names.is_empty() {
                continue;
            }
            let Some(kind) = shown(d, src, i, &names) else {
                continue;
            };
            if kind == LineKind::Hidden && plan.hidden == Hidden::Off {
                continue;
            }
            let axis = pose.apply_vector(t.axis);
            let (mouth, end) = (pose.apply_point(t.mouth), pose.apply_point(t.end));
            let r = t.ring_diameter() / 2.0;
            let towards = super::v3(plan.frame.towards);
            let cos = axis.dot(towards) / axis.length().max(1e-300);
            let entity = Some(format!("{label}/{}", names[0]));
            let mut paths = Vec::new();
            if cos.abs() >= 1.0 - ALONG {
                let o = img(mouth);
                let from = std::f64::consts::FRAC_PI_2;
                let (s, co) = ryu_math::sin_cos(from);
                paths.push(Path {
                    start: [o[0] + r * co, o[1] + r * s],
                    segments: vec![Segment::Arc {
                        centre: o,
                        radius: r,
                        from,
                        sweep: 1.5 * std::f64::consts::PI,
                    }],
                    closed: false,
                });
            } else if cos.abs() <= ALONG {
                let (a, b) = (img(mouth), img(end));
                let u = [b[0] - a[0], b[1] - a[1]];
                let l = ryu_math::hypot(u[0], u[1]);
                if l <= 0.0 {
                    continue;
                }
                let n = [-u[1] / l * r, u[0] / l * r];
                for k in [1.0, -1.0] {
                    paths.push(Path::polyline(
                        &[
                            [a[0] + k * n[0], a[1] + k * n[1]],
                            [b[0] + k * n[0], b[1] + k * n[1]],
                        ],
                        false,
                    ));
                }
            }
            // A detail keeps a thread only wholly inside its circle.
            if let Extra::Detail { centre, radius, .. } = &plan.extra {
                let m = plan.frame.image(*centre);
                let m = [m[0] - c[0], m[1] - c[1]];
                let inside = paths.iter().all(|p| {
                    let [lo, hi] = p.bounds();
                    [lo, hi, [lo[0], hi[1]], [hi[0], lo[1]]]
                        .iter()
                        .all(|q| ryu_math::hypot(q[0] - m[0], q[1] - m[1]) <= *radius)
                });
                if !inside {
                    continue;
                }
            }
            for p in paths {
                out.push((p, kind, false, entity.clone()));
            }
        }
    }
    out
}
