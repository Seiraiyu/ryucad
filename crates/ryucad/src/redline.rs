//! What ryucad works out of an annotation's redline marks (design 423):
//! the plane of the view they were drawn in, each mark's outline in it,
//! and what each touches, read off a pick pass ([`crate::render::pick`]).
//! It says where a mark is and what lies there; reading a mark as an
//! instruction is the agent's.

pub use crate::doc::redline::{Anchor, At, Kind, Mark, Marks, Touch};
use crate::render::{PICK_REACH, Picked, View};
use crate::view::{Camera, Projection};
use std::collections::BTreeMap;

/// An ellipse is drawn and judged as this many chords.
pub const ELLIPSE_SEGMENTS: usize = 64;

/// The pick buffer's height in pixels, and its least width.
pub const PICK_HEIGHT: u32 = 600;
pub const PICK_WIDTH: u32 = 800;

/// A view's plane: through its target, square to its line of sight, `x`
/// to the right and `y` up as the view sees them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub target: [f64; 3],
    pub right: [f64; 3],
    pub up: [f64; 3],
    pub forward: [f64; 3],
    pub eye: [f64; 3],
    pub perspective: bool,
}

fn add(a: [f64; 3], b: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] + s * b[0], a[1] + s * b[1], a[2] + s * b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

impl Plane {
    pub fn of(camera: &Camera) -> Plane {
        let (right, up, forward) = crate::render::view_basis(camera);
        Plane {
            target: camera.target,
            right,
            up,
            forward,
            eye: camera.eye,
            perspective: matches!(camera.projection, Projection::Perspective { .. }),
        }
    }

    /// The model point at `xy` on the plane.
    pub fn point(&self, xy: [f64; 2]) -> [f64; 3] {
        add(add(self.target, self.right, xy[0]), self.up, xy[1])
    }

    /// Where the sight line through model point `p` crosses the plane:
    /// along the view's direction (orthographic), or from the eye
    /// (perspective).
    pub fn xy(&self, p: [f64; 3]) -> [f64; 2] {
        let q = if self.perspective {
            let to = sub(p, self.eye);
            let along = dot(to, self.forward);
            if along <= 0.0 {
                p
            } else {
                add(
                    self.eye,
                    to,
                    dot(sub(self.target, self.eye), self.forward) / along,
                )
            }
        } else {
            p
        };
        let d = sub(q, self.target);
        [dot(d, self.right), dot(d, self.up)]
    }
}

/// A view mark's outline in its plane: a line's or arrow's two points,
/// a rectangle's four corners closed, an ellipse's
/// [`ELLIPSE_SEGMENTS`] chords closed (starting at its right end), a
/// stroke's points, a text's anchor. A face stroke has none.
pub fn outline(mark: &Mark) -> Vec<[f64; 2]> {
    let Anchor::View(xy) = &mark.anchor else {
        return Vec::new();
    };
    match (mark.kind, xy.as_slice()) {
        (Kind::Rectangle, [a, b]) => vec![*a, [b[0], a[1]], *b, [a[0], b[1]], *a],
        (Kind::Ellipse, [a, b]) => {
            let c = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let r = [(b[0] - a[0]).abs() / 2.0, (b[1] - a[1]).abs() / 2.0];
            let mut out: Vec<[f64; 2]> = (0..ELLIPSE_SEGMENTS)
                .map(|k| {
                    let t = 2.0 * core::f64::consts::PI * k as f64 / ELLIPSE_SEGMENTS as f64;
                    [
                        c[0] + r[0] * ryukei::math::cos(t),
                        c[1] + r[1] * ryukei::math::sin(t),
                    ]
                })
                .collect();
            out.push(out[0]);
            out
        }
        _ => xy.clone(),
    }
}

/// How big a pick buffer must be: [`PICK_HEIGHT`] high (the view's
/// height across it) and wide enough that every view mark's point lies
/// inside with the reach to spare, at least [`PICK_WIDTH`], at most the
/// renderer's limit.
pub fn pick_size(plane: &Plane, camera: &Camera, marks: &[Mark]) -> (u32, u32) {
    let view = View::new(camera, PICK_WIDTH, PICK_HEIGHT);
    let mid = PICK_WIDTH as f32 / 2.0;
    let mut half = mid;
    for m in marks {
        for p in outline(m) {
            if let Some((x, _)) = view.pixel(plane.point(p)) {
                half = half.max((x - mid).abs() + PICK_REACH + 2.0);
            }
        }
    }
    let w = (2.0 * half).ceil() as u32;
    (w.clamp(PICK_WIDTH, crate::render::MAX_SIZE), PICK_HEIGHT)
}

/// Where a face stroke's part frame sits in the scene: identity for a
/// part's own solid (`<part>/<founder>`), else the occurrence's pose
/// (`<asm>/<path>`), by instance path in `poses[asm]`.
pub fn placed(
    on: &str,
    poses: &BTreeMap<String, BTreeMap<String, ryukei::geom::Rigid>>,
) -> ryukei::geom::Rigid {
    let (part, rest) = on.split_once('/').unwrap_or((on, ""));
    poses
        .get(part)
        .and_then(|m| m.get(rest))
        .copied()
        .unwrap_or(ryukei::geom::Rigid::IDENTITY)
}

/// What `mark` touches in `picked`, seen through `view` from `plane`'s
/// camera (design 423's table): a line's or arrow's ends what the
/// window's picker would pick there; a closed mark's inside the faces
/// most of it covers; an open stroke the edges and faces it passes
/// over; a face stroke the faces under its points; a text nothing.
pub fn touches(
    picked: &Picked,
    view: &View,
    plane: &Plane,
    mark: &Mark,
    poses: &BTreeMap<String, BTreeMap<String, ryukei::geom::Rigid>>,
) -> Vec<Touch> {
    let px = |p: [f64; 3]| view.pixel(p);
    let on_plane = |xy: [f64; 2]| px(plane.point(xy));
    let touch = |at: At, r: &str| Touch {
        at,
        reference: r.to_owned(),
    };
    match (&mark.anchor, mark.kind) {
        (_, Kind::Text) => Vec::new(),
        (Anchor::View(xy), Kind::Line | Kind::Arrow) => {
            let ats = if mark.kind == Kind::Arrow {
                [At::Tail, At::Tip]
            } else {
                [At::Start, At::End]
            };
            ats.into_iter()
                .zip(xy.iter())
                .filter_map(|(at, p)| {
                    let (x, y) = on_plane(*p)?;
                    picked.top(x, y).map(|r| touch(at, r))
                })
                .collect()
        }
        (Anchor::View(_), _) if mark.closed() => {
            let poly: Vec<(f32, f32)> = outline(mark).into_iter().filter_map(on_plane).collect();
            picked
                .faces_inside(&poly)
                .into_iter()
                .map(|(r, _)| touch(At::Inside, &r))
                .collect()
        }
        (Anchor::View(xy), _) => {
            let line: Vec<(f32, f32)> = xy.iter().filter_map(|p| on_plane(*p)).collect();
            picked
                .under(&line)
                .into_iter()
                .map(|(r, _)| touch(At::Under, &r))
                .collect()
        }
        (Anchor::Face { on, points_m }, _) => {
            let pose = placed(on, poses);
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for p in points_m {
                let q = pose.apply_point(ryukei::geom::Point3::new(p[0], p[1], p[2]));
                if let Some((x, y)) = px([q.x, q.y, q.z])
                    && let Some(f) = picked.face(x, y)
                {
                    *counts.entry(f.to_owned()).or_default() += 1;
                }
            }
            let mut by: Vec<(String, usize)> = counts.into_iter().collect();
            by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            by.truncate(crate::doc::redline::MAX_TOUCHES);
            by.into_iter().map(|(r, _)| touch(At::Under, &r)).collect()
        }
    }
}

/// Each of `a`'s marks in words (design 423's Reading marks), in the
/// project's default unit.
pub fn words(
    session: &mut crate::session::Session,
    a: &crate::annotation::Annotation,
) -> crate::error::Result<Vec<serde_json::Value>> {
    use crate::measure::{Kind as M, point, value};
    use ryukei::geom::Point3;
    use ryukei::measure::Value as Measured;
    use serde_json::json;
    let unit = session.project().manifest.default_unit;
    let (Some(marks), Some(view)) = (&a.marks, &a.view) else {
        return Ok(Vec::new());
    };
    let plane = Plane::of(&crate::view::Camera::from(view));
    let f = unit.factor();
    let two = |p: [f64; 2]| {
        let c = |x: f64| crate::measure::number(x / f);
        json!({"m": p, "value": format!("({} {u}, {} {u})", c(p[0]), c(p[1]), u = unit.as_str())})
    };
    let three = |p: [f64; 3]| point(Point3::new(p[0], p[1], p[2]), unit);
    let mut out = Vec::new();
    for (i, m) in marks.list.iter().enumerate() {
        let mut v = json!({"index": i, "kind": m.kind.as_str()});
        match &m.anchor {
            Anchor::View(xy) => {
                v["in"] = json!("view");
                v["points"] = json!(
                    xy.iter()
                        .map(|p| json!({"plane": two(*p), "model": three(plane.point(*p))}))
                        .collect::<Vec<_>>()
                );
                if let ([a0, b0], Kind::Line | Kind::Arrow) = (xy.as_slice(), m.kind) {
                    let d = [b0[0] - a0[0], b0[1] - a0[1]];
                    let len = ryukei::math::sqrt(d[0] * d[0] + d[1] * d[1]);
                    v["length"] = value(Measured::Exact(len), M::Length, unit);
                    if len > 0.0 {
                        let w =
                            [0, 1, 2].map(|k| (plane.right[k] * d[0] + plane.up[k] * d[1]) / len);
                        v["direction"] = json!(w);
                    }
                }
                if m.kind != Kind::Text && (m.closed() || m.kind == Kind::Stroke) {
                    let pts = outline(m);
                    let lo = [0, 1].map(|k| pts.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min));
                    let hi =
                        [0, 1].map(|k| pts.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max));
                    v["box"] = json!({"plane": {"min": two(lo), "max": two(hi),
                                                "size": two([hi[0] - lo[0], hi[1] - lo[1]])}});
                }
            }
            Anchor::Face { on, points_m } => {
                v["in"] = json!("face");
                v["on"] = json!(on);
                v["count"] = json!(points_m.len());
                let lo =
                    [0, 1, 2].map(|k| points_m.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min));
                let hi = [0, 1, 2].map(|k| {
                    points_m
                        .iter()
                        .map(|p| p[k])
                        .fold(f64::NEG_INFINITY, f64::max)
                });
                v["box"] = json!({"part": {"min": three(lo), "max": three(hi)}});
            }
        }
        if let Some(t) = &m.text {
            v["text"] = json!(t);
        }
        let mut touches = Vec::new();
        for t in m.touches.iter().flatten() {
            let mut tv = json!({"at": t.at.as_str(), "ref": t.reference});
            match crate::entity::EntityRef::parse(&t.reference).and_then(|r| r.resolve(session)) {
                Ok(target) => {
                    tv["kind"] = json!(target.as_str());
                    tv["outdated"] = json!(false);
                    let part = t.reference.split('/').next().unwrap_or_default().to_owned();
                    let req = crate::proto::Request::Inspect {
                        part,
                        feature: None,
                        entity: Some(t.reference.clone()),
                        rev: None,
                    };
                    if let Ok(now) = crate::server::dispatch(session, None, None, req) {
                        tv["now"] = now;
                    }
                }
                Err(e) => {
                    tv["outdated"] = json!(true);
                    tv["error"] = json!(e);
                }
            }
            touches.push(tv);
        }
        v["touches"] = json!(touches);
        out.push(v);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::view::{Camera, Projection};

    pub(crate) fn corner(projection: Projection) -> Camera {
        Camera {
            eye: [0.12, -0.10, 0.09],
            target: [0.03, 0.02, 0.0025],
            up: [0.0, 0.0, 1.0],
            projection,
        }
    }

    #[test]
    fn a_point_goes_onto_the_plane_and_back() {
        for p in [
            Projection::Orthographic { height: 0.08 },
            Projection::Perspective { fov_deg: 40.0 },
        ] {
            let plane = Plane::of(&corner(p));
            let o = plane.xy(plane.target);
            assert!(o[0].abs() < 1e-15 && o[1].abs() < 1e-15, "{o:?}");
            let q = plane.point([0.01, -0.004]);
            let back = plane.xy(q);
            assert!((back[0] - 0.01).abs() < 1e-15 && (back[1] + 0.004).abs() < 1e-15);
            // A model point and where it lands on the plane are on one
            // sight line, so they fall on one pixel.
            let v = crate::render::View::new(&corner(p), 800, 600);
            let m = [0.01, 0.01, 0.005];
            let (a, b) = (
                v.pixel(m).unwrap(),
                v.pixel(plane.point(plane.xy(m))).unwrap(),
            );
            assert!(
                (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3,
                "{a:?} {b:?}"
            );
        }
    }

    #[test]
    fn outlines_follow_their_kinds() {
        let mark = |kind, xy: Vec<[f64; 2]>| Mark {
            kind,
            anchor: Anchor::View(xy),
            text: None,
            touches: None,
        };
        let r = outline(&mark(Kind::Rectangle, vec![[0.0, 0.0], [2.0, 1.0]]));
        assert_eq!(
            r,
            vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0], [0.0, 0.0]]
        );
        let e = outline(&mark(Kind::Ellipse, vec![[0.0, 0.0], [2.0, 1.0]]));
        assert_eq!(e.len(), ELLIPSE_SEGMENTS + 1);
        assert_eq!(e[0], [2.0, 0.5]);
        assert_eq!(e[0], e[ELLIPSE_SEGMENTS]);
        assert_eq!(
            outline(&mark(Kind::Arrow, vec![[0.0, 0.0], [1.0, 1.0]])).len(),
            2
        );
    }

    #[test]
    fn the_pick_buffer_is_600_high_and_as_wide_as_the_marks() {
        let cam = corner(Projection::Orthographic { height: 0.06 });
        let plane = Plane::of(&cam);
        let near = vec![Mark {
            kind: Kind::Line,
            anchor: Anchor::View(vec![[0.0, 0.0], [0.01, 0.0]]),
            text: None,
            touches: None,
        }];
        assert_eq!(pick_size(&plane, &cam, &near), (800, 600));
        // 0.06 m tall at 600 pixels: 0.1 mm a pixel; a mark 0.07 m right
        // of the target needs 700 pixels each side, and the reach.
        let far = vec![Mark {
            kind: Kind::Line,
            anchor: Anchor::View(vec![[0.0, 0.0], [0.07, 0.0]]),
            text: None,
            touches: None,
        }];
        let (w, h) = pick_size(&plane, &cam, &far);
        assert_eq!(h, 600);
        assert!((1400..=1420).contains(&w), "{w}");
    }
}
