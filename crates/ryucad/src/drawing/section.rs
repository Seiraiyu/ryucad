//! Section views (design 417, plan 9.B): a cut resolved against what the
//! view draws, each body cut by it (a full section through
//! `ryukei::project::section`, an offset one through a stepped tool and
//! the boolean), the faces the cut makes (its caps) hatched, and the
//! cutting-plane line drawn on the parent.
//!
//! A cut body is kept by its body's hash, its pose's bits and the cut's
//! bits, in the session and on disk (`sections`), with its caps by name:
//! the next build of the view projects it again from the cache.

use super::{Frame, Source};
use crate::doc::assembly::Rev;
use crate::doc::drawing::{Cut, ModelPoint};
use crate::doc::geometry::GeomRef;
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use ryu_draw::{Anchor, Colour, Item, LineKind, P, Path};
use ryukei::geom::{Point3, Rigid, Vector3};
use ryukei::name::{Event, Name, OpKey, ToolKey};
use ryukei::topo::{Body, EntityId, FaceId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// How a section cuts.
#[derive(Clone, Debug)]
pub enum How {
    /// A full section: what lies on the side the plane's normal points to
    /// goes.
    Plane(crate::section::Plane),
    /// An offset section through `points` (model metres) in order, each
    /// pair's line swept along `square` (its parent's line of sight).
    Steps {
        points: Vec<[f64; 3]>,
        square: [f64; 3],
    },
}

/// A section view's cut.
#[derive(Clone, Debug)]
pub struct Cutting {
    pub parent: super::Key,
    pub letter: String,
    /// Towards the side cut away, unit: the view looks along its reverse.
    pub normal: [f64; 3],
    pub how: How,
    /// Instance paths left whole.
    pub unsectioned: Vec<String>,
}

fn bad(pointer: String, message: String) -> Error {
    Error::new(Code::InvalidParam, message).with("pointer", pointer)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn text_of(of: &Id, g: &GeomRef) -> String {
    match g {
        GeomRef::Plane(p) => p.as_str().to_owned(),
        GeomRef::Entity(r) => format!("{of}/{}/{}", r.feature, r.name),
        GeomRef::Feature(f) => format!("{of}/{f}"),
        other => other.text(of),
    }
}

/// A model point a view names, in the model `of` at `rev` gives.
pub(super) fn point_at(
    session: &mut Session,
    of: &Id,
    rev: &Rev,
    p: &ModelPoint,
    at: &str,
) -> Result<[f64; 3]> {
    let g = match p {
        ModelPoint::At(q) => return Ok(q.clone().map(|x| x.meters())),
        ModelPoint::Ref(g) => g,
    };
    let text = text_of(of, g);
    let not_a_point = || {
        bad(
            at.to_owned(),
            format!("{text} names no point: give a vertex, a datum point, origin or three lengths"),
        )
    };
    match g {
        GeomRef::Origin => Ok([0.0; 3]),
        GeomRef::Feature(_) => match crate::geometry::locate_at(session, &text, rev)? {
            Some(crate::geometry::Located {
                geom: crate::geometry::Geom::Point(q),
                ..
            }) => Ok(q.to_array()),
            _ => Err(not_a_point()),
        },
        GeomRef::Entity(_) => {
            let l = crate::measure::locate_at(session, &text, rev)?;
            let [l] = l.as_slice() else {
                return Err(not_a_point());
            };
            let ryukei::measure::Entity::Vertex(v) = l.entity else {
                return Err(not_a_point());
            };
            let q = l.built.body.vertex(v).ok_or_else(not_a_point)?.point;
            Ok(l.pose.apply_point(q).to_array())
        }
        _ => Err(not_a_point()),
    }
}

/// The cut of view `k`, from `parent` (its frame), in the model `of` at
/// `rev` gives.
#[allow(clippy::too_many_arguments)]
pub(super) fn cutting(
    session: &mut Session,
    of: &Id,
    rev: &Rev,
    k: super::Key,
    parent: (super::Key, &Frame),
    cut: &Cut,
    flip: bool,
    unsectioned: &[String],
    letter: &str,
) -> Result<Cutting> {
    let at = format!("/views/{k}/cut");
    let (normal, how) = match cut {
        Cut::Plane { plane, offset } => {
            let p = match plane {
                GeomRef::Plane(_) | GeomRef::Entity(_) | GeomRef::Feature(_) => {
                    crate::section::named_at(session, &text_of(of, plane), rev, &at)?
                }
                _ => {
                    return Err(bad(
                        format!("{at}/plane"),
                        format!("view {k} cuts on {}, which is no plane", text_of(of, plane)),
                    ));
                }
            };
            let p = match offset {
                Some(d) => p.moved(d.meters()),
                None => p,
            };
            let p = if flip { p.flipped() } else { p };
            (p.normal, How::Plane(p))
        }
        Cut::At { origin, normal } => {
            let o = origin.clone().map(|q| q.meters());
            let p = crate::section::Plane::new(o, *normal)
                .ok_or_else(|| bad(format!("{at}/normal"), "a cut's normal has a length".into()))?;
            let p = if flip { p.flipped() } else { p };
            (p.normal, How::Plane(p))
        }
        Cut::Through(points) => {
            let pts: Vec<[f64; 3]> = points
                .iter()
                .enumerate()
                .map(|(i, p)| point_at(session, of, rev, p, &format!("{at}/through/{i}")))
                .collect::<Result<_>>()?;
            let f = parent.1;
            let img: Vec<P> = pts.iter().map(|p| f.image(*p)).collect();
            let d0 = [img[1][0] - img[0][0], img[1][1] - img[0][1]];
            let len = ryu_math::hypot(d0[0], d0[1]);
            if len < 1e-9 {
                return Err(bad(
                    format!("{at}/through/1"),
                    format!(
                        "view {k}'s first two points lie on one line of sight of view {}",
                        parent.0
                    ),
                ));
            }
            // Each step runs on from the last, never back along the first
            // segment's way: else the cut would fold.
            for (i, w) in img.windows(2).enumerate() {
                let d = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
                if d[0] * d0[0] + d[1] * d0[1] < -1e-12 * len * ryu_math::hypot(d[0], d[1]) {
                    return Err(bad(
                        format!("{at}/through/{}", i + 1),
                        format!("view {k}'s cut turns back on itself: each point goes on along its first segment's way"),
                    )
                    .with("reason", "folds"));
                }
            }
            // To the left of the first segment, seen in the parent.
            let s = if flip { -1.0 } else { 1.0 };
            let m = [-d0[1] / len * s, d0[0] / len * s];
            let n = [0, 1, 2].map(|i| m[0] * f.x[i] + m[1] * f.y[i]);
            (
                n,
                How::Steps {
                    points: pts,
                    square: f.towards,
                },
            )
        }
    };
    Ok(Cutting {
        parent: parent.0,
        letter: letter.to_owned(),
        normal,
        how,
        unsectioned: unsectioned.to_vec(),
    })
}

/// A section view's frame: looking along `-normal`, up its parent's up
/// unless that runs along the normal, else away from its parent's eye (a
/// top view's way).
pub(super) fn frame_of(normal: [f64; 3], parent: &Frame) -> Result<Frame> {
    let up = if dot(normal, parent.y).abs() < 0.9 {
        parent.y
    } else {
        parent.towards.map(|x| -x)
    };
    Frame::new(normal, up)
}
