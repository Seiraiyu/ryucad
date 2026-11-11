//! What an offset starts from: each face's distance and offset surface,
//! and how each edge's faces move: apart (opening), together (closing),
//! along together (smooth), or not at all (kept).

use super::OffsetSpec;
use super::edges::moved_point;
use super::inverted::{Inversion, inverted};
use crate::blend::convex::{Bend, BendFail, bend};
use crate::geom::{Point3, Surface, Vector3};
use crate::intersect::face_rect;
use crate::name::Name;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, FinId, Sense};
use crate::tweak::Motion;
use crate::tweak::motion::on_both_planes;
use crate::{Error, OffsetCase, OffsetError};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// Which offset a plan is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// 8.E's local offset: closing edges are their faces' offsets'
    /// intersection, and a face whose offset passes its centre collapses.
    Local,
    /// The global offset's raw envelope (design 299): every edge between
    /// faces at one distance that isn't smooth gets a fill, inverted where
    /// the offsets close; a face whose offset passes its centre is kept
    /// inverted.
    Envelope,
    /// A face tweak (design 384): faces move by their motions, every edge
    /// keeps one curve, and nothing is filled.
    Tweak,
}

/// How far apart, at most, the offsets of an edge's faces may lie along
/// it for the edge to count as smooth: `|d|` times the angle between the
/// faces' normals, at 17 samples. Within it, both offsets meet one curve
/// within the tolerance an intersection's fit is allowed
/// (`10 × LINEAR_RESOLUTION`), so the curve is their common edge.
pub(crate) const SMOOTH_GAP: f64 = 10.0 * LINEAR_RESOLUTION;

/// Samples along an edge at which smoothness is judged, ends included.
const SAMPLES: usize = 17;

/// How far past its own box a spline face's offset reaches, at least, as
/// a share of the box's width each way: closing edges are found a little
/// way past where the face ends.
const MARGIN: f64 = 0.1;

/// How an edge's faces move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    /// Both faces stay (distance zero): the edge stays as it is.
    Kept,
    /// The faces are tangent all along it: their offsets meet along the
    /// edge moved by the distance.
    Smooth,
    /// The offsets move apart: a ball rolled about the edge fills the gap.
    Opening,
    /// The offsets cross: the edge becomes their intersection.
    Closing,
}

/// The faces, their distances and offsets, and the edges' classes.
pub(crate) struct Plan<'a> {
    pub(crate) body: &'a Body,
    /// Each face's distance, positive outwards.
    pub(crate) distance: BTreeMap<FaceId, f64>,
    /// Each face's offset surface, keeping the surface's parameters.
    pub(crate) surface: BTreeMap<FaceId, Surface>,
    /// Each edge's class, and for a smooth edge how far its faces'
    /// offsets lie apart.
    pub(crate) class: BTreeMap<EdgeId, (Class, f64)>,
    /// The live inner edges (two fins each) in name order.
    pub(crate) edges: Vec<EdgeId>,
    /// The plan's mode.
    pub(crate) mode: Mode,
    /// In an envelope, the opening edges whose fills are inverted: their
    /// offsets close.
    pub(crate) inverted: BTreeSet<EdgeId>,
    /// In an envelope, the faces whose offsets passed their centres, and
    /// how their parameters relate.
    pub(crate) inversions: BTreeMap<FaceId, Inversion>,
    /// The live laminar edges (a sheet's boundary, one fin each) in name
    /// order: a solid has none.
    pub(crate) laminar: Vec<EdgeId>,
    /// In a tweak, each changed face's motion (an offset plan has none:
    /// each face moves by its distance, [`Motion::Offset`]).
    pub(crate) motion: BTreeMap<FaceId, Motion>,
    /// In a tweak, the sense a replaced face takes on its new surface;
    /// every other face keeps its own.
    pub(crate) senses: BTreeMap<FaceId, Sense>,
}

/// A distance a face may move: finite, and zero or at least
/// `10 × LINEAR_RESOLUTION` either way.
pub(crate) fn usable(d: f64) -> bool {
    d.is_finite() && (d == 0.0 || d.abs() >= 10.0 * LINEAR_RESOLUTION)
}

impl<'a> Plan<'a> {
    /// The plan for `spec`: fails with `BadDistance`, `NoSuchFace` (a name
    /// that isn't a face's, or one named twice), `FaceCollapses`, and for
    /// edges `Unsupported` (`PartlySmooth`) or `Uncertain`.
    pub(crate) fn new(body: &'a Body, spec: &OffsetSpec) -> Result<Self, Error> {
        Self::with_mode(body, spec, Mode::Local)
    }

    /// The plan in `mode`: an envelope's fails as a local plan does but
    /// where its rules differ (see [`Mode::Envelope`]).
    pub(crate) fn with_mode(body: &'a Body, spec: &OffsetSpec, mode: Mode) -> Result<Self, Error> {
        let mut faces: Vec<FaceId> = body.faces().collect();
        if faces.is_empty() {
            return Err(OffsetError::BadDistance.into());
        }
        faces.sort_by_key(|&f| body.faces[f].name);
        let by_name: BTreeMap<Name, FaceId> =
            faces.iter().map(|&f| (body.faces[f].name, f)).collect();
        let mut distance: BTreeMap<FaceId, f64> =
            faces.iter().map(|&f| (f, spec.distance)).collect();
        let mut named: Vec<Name> = Vec::new();
        for &(face, d) in &spec.faces {
            let f = *by_name.get(&face).ok_or(OffsetError::NoSuchFace { face })?;
            if named.contains(&face) {
                return Err(OffsetError::NoSuchFace { face }.into());
            }
            named.push(face);
            distance.insert(f, d);
        }
        if !distance.values().all(|&d| usable(d)) || distance.values().all(|&d| d == 0.0) {
            return Err(OffsetError::BadDistance.into());
        }
        Self::with_distances_in(body, distance, mode)
    }

    /// The plan for each face's own distance, every one usable (checked
    /// by the caller), all of them zero allowed: a thickened sheet's side
    /// at zero is the sheet's own faces, every edge kept.
    pub(crate) fn with_distances(
        body: &'a Body,
        distance: BTreeMap<FaceId, f64>,
    ) -> Result<Self, Error> {
        Self::with_distances_in(body, distance, Mode::Local)
    }

    /// A tweak's plan: each changed face's motion and every face's new
    /// surface (the unchanged ones their own, or carried past their
    /// domains), replaced faces' senses, and each inner edge's class as
    /// the offset's code reads it (kept, smooth or closing; the tweak's
    /// own classes are its own, see `crate::tweak::classify`). Distances
    /// are the offsets' and zero for every other face.
    pub(crate) fn for_tweak(
        body: &'a Body,
        motion: BTreeMap<FaceId, Motion>,
        surface: BTreeMap<FaceId, Surface>,
        senses: BTreeMap<FaceId, Sense>,
        class: BTreeMap<EdgeId, (Class, f64)>,
    ) -> Self {
        let mut all: Vec<EdgeId> = body.edges().collect();
        all.sort_by_key(|&e| body.edges[e].name);
        let (laminar, edges): (Vec<EdgeId>, Vec<EdgeId>) =
            all.into_iter().partition(|&e| body.is_laminar(e));
        let distance = body
            .faces()
            .map(|f| (f, motion.get(&f).map_or(0.0, Motion::distance)))
            .collect();
        let class = edges
            .iter()
            .map(|&e| (e, class.get(&e).copied().unwrap_or((Class::Kept, 0.0))))
            .collect();
        Self {
            body,
            distance,
            surface,
            class,
            edges,
            mode: Mode::Tweak,
            inverted: BTreeSet::new(),
            inversions: BTreeMap::new(),
            laminar,
            motion,
            senses,
        }
    }

    /// [`Self::with_distances`] in `mode`.
    fn with_distances_in(
        body: &'a Body,
        distance: BTreeMap<FaceId, f64>,
        mode: Mode,
    ) -> Result<Self, Error> {
        let mut faces: Vec<FaceId> = body.faces().collect();
        faces.sort_by_key(|&f| body.faces[f].name);
        let found: Vec<(FaceId, Surface, Option<Inversion>)> = faces
            .par_iter()
            .map(|&f| envelope_surface(body, f, distance[&f], mode).map(|(s, i)| (f, s, i)))
            .collect::<Result<_, Error>>()?;
        let mut surface: BTreeMap<FaceId, Surface> = BTreeMap::new();
        let mut inversions: BTreeMap<FaceId, Inversion> = BTreeMap::new();
        for (f, s, i) in found {
            surface.insert(f, s);
            if let Some(i) = i {
                inversions.insert(f, i);
            }
        }
        let mut all: Vec<EdgeId> = body.edges().collect();
        all.sort_by_key(|&e| body.edges[e].name);
        let (laminar, edges): (Vec<EdgeId>, Vec<EdgeId>) =
            all.into_iter().partition(|&e| body.is_laminar(e));
        let mut class: BTreeMap<EdgeId, (Class, f64)> = edges
            .par_iter()
            .map(|&e| classify(body, &distance, e).map(|c| (e, c)))
            .collect::<Result<_, Error>>()?;
        let mut inverted = BTreeSet::new();
        if mode == Mode::Envelope {
            for &e in &edges {
                let [fa, fb] = fins(body, e);
                let face = |fin: FinId| body.loops[body.fins[fin].loop_].face;
                if class[&e].0 == Class::Closing && distance[&face(fa)] == distance[&face(fb)] {
                    class.insert(e, (Class::Opening, 0.0));
                    inverted.insert(e);
                }
            }
        }
        Ok(Self {
            body,
            distance,
            surface,
            class,
            edges,
            mode,
            inverted,
            inversions,
            laminar,
            motion: BTreeMap::new(),
            senses: BTreeMap::new(),
        })
    }

    /// The laminar edge's one fin.
    pub(crate) fn laminar_fin(&self, e: EdgeId) -> FinId {
        self.body.edges[e].fin
    }

    /// The edge's two fins: `[0]` runs along it, `[1]` against it.
    pub(crate) fn fins(&self, e: EdgeId) -> [FinId; 2] {
        fins(self.body, e)
    }

    /// The face a fin lies on.
    pub(crate) fn face_of(&self, fin: FinId) -> FaceId {
        self.body.loops[self.body.fins[fin].loop_].face
    }

    /// The face's outward normal at the edge's parameter `t`, through the
    /// fin's pcurve; zero where its surface has none.
    pub(crate) fn normal(&self, fin: FinId, t: f64) -> Vector3 {
        normal(self.body, fin, t)
    }

    /// The face's distance along its outward normal.
    pub(crate) fn d(&self, f: FaceId) -> f64 {
        self.distance[&f]
    }

    /// The face's motion: its tweak motion, else its offset.
    pub(crate) fn motion_of(&self, f: FaceId) -> Motion {
        self.motion
            .get(&f)
            .copied()
            .unwrap_or(Motion::Offset(self.distance[&f]))
    }

    /// The face's sense on its new surface.
    pub(crate) fn sense_of(&self, f: FaceId) -> Sense {
        self.senses
            .get(&f)
            .copied()
            .unwrap_or(self.body.faces[f].sense)
    }

    /// Where the point `p` of the edge the fins share (at its parameter
    /// `t`) goes: on both faces' new tangent planes, nearest where each
    /// face's motion takes it. Two offsets give [`moved_point`] exactly,
    /// as the offset always computed it.
    pub(crate) fn goes(&self, fins: [FinId; 2], p: Point3, t: f64) -> Point3 {
        let faces = fins.map(|f| self.face_of(f));
        let n = [self.normal(fins[0], t), self.normal(fins[1], t)];
        match faces.map(|f| self.motion_of(f)) {
            [Motion::Offset(a), Motion::Offset(b)] => moved_point(p, n, [a, b]),
            ms => {
                let [(qa, na), (qb, nb)] = [0, 1].map(|k| {
                    crate::tweak::motion::apply(
                        &ms[k],
                        &self.surface[&faces[k]],
                        self.sense_of(faces[k]),
                        p,
                        n[k],
                    )
                });
                on_both_planes([qa, qb], [na, nb])
            }
        }
    }

    /// How far the edge's points may go: the larger distance for two
    /// offsets, as the offset always took it; otherwise the furthest its
    /// ends and middle go under [`Self::goes`].
    pub(crate) fn reach_of(&self, e: EdgeId) -> f64 {
        let fins = self.fins(e);
        let faces = fins.map(|f| self.face_of(f));
        if let [Motion::Offset(a), Motion::Offset(b)] = faces.map(|f| self.motion_of(f)) {
            return a.abs().max(b.abs());
        }
        let edge = &self.body.edges[e];
        let curve = &self.body.curves[edge.curve];
        let [t0, t1] = edge.interval;
        [t0, (t0 + t1) / 2.0, t1]
            .into_iter()
            .map(|t| {
                let p = curve.point(t);
                self.goes(fins, p, t).distance(p)
            })
            .fold(0.0, f64::max)
    }
}

/// The edge's two fins: `[0]` runs along it, `[1]` against it.
pub(crate) fn fins(body: &Body, e: EdgeId) -> [FinId; 2] {
    let fins: Vec<FinId> = body.edge_fins(e).collect();
    if body.fins[fins[0]].sense == Sense::Forward {
        [fins[0], fins[1]]
    } else {
        [fins[1], fins[0]]
    }
}

/// A fin's face's outward normal at its edge's parameter `t`.
pub(crate) fn normal(body: &Body, fin: FinId, t: f64) -> Vector3 {
    let face = &body.faces[body.loops[body.fins[fin].loop_].face];
    let s = &body.surfaces[face.surface];
    let sign = if face.sense == Sense::Forward {
        1.0
    } else {
        -1.0
    };
    body.fin_uv(fin, t)
        .and_then(|uv| s.normal(uv.x, uv.y))
        .map_or(Vector3::ZERO, |n| n.get() * sign)
}

/// The face's surface moved by its distance along its outward normal:
/// the surface itself at distance zero, an analytic surface or a pipe of
/// its own kind, any other an [`crate::geom::Offset`] over the face's box
/// grown by [`MARGIN`] each way (or the box alone, where that folds).
pub(crate) fn offset_surface(body: &Body, f: FaceId, d: f64) -> Result<Surface, Error> {
    offset_surface_in(body, f, d, false)
}

/// [`offset_surface`]; with `through`, a spline, swept or bent face's
/// offset may pass its focal surface over the whole box
/// ([`crate::geom::Offset::inverted_new`], design 484).
fn offset_surface_in(body: &Body, f: FaceId, d: f64, through: bool) -> Result<Surface, Error> {
    let face = &body.faces[f];
    let s = &body.surfaces[face.surface];
    if d == 0.0 {
        return Ok(s.clone());
    }
    let d = if face.sense == Sense::Forward { d } else { -d };
    let collapses = || -> Error { OffsetError::FaceCollapses { face: face.name }.into() };
    if s.frame().is_some() || matches!(s, Surface::Pipe(_)) {
        return s.offset(d).ok_or_else(collapses)?.map_err(|_| collapses());
    }
    let whole = s.domain();
    let (pu, pv) = s.periods();
    let periods = [pu, pv];
    let rect = face_rect(body, f).unwrap_or(whole);
    // Past the box by a share of it, and at least three times the
    // distance in model space: closing edges are searched for within two
    // distances of their edges, so no two faces' offsets end together
    // there, where no crossing could be certified.
    let mid = s.eval(
        (rect[0][0] + rect[0][1]) / 2.0,
        (rect[1][0] + rect[1][1]) / 2.0,
    );
    let speed = [mid.du.length(), mid.dv.length()];
    let grown: [[f64; 2]; 2] = core::array::from_fn(|k| {
        let [a, b] = rect[k];
        let reach = if speed[k] > 0.0 {
            3.0 * d.abs() / speed[k]
        } else {
            0.0
        };
        let m = ((b - a) * MARGIN).max(reach);
        match periods[k] {
            Some(p) if b - a + 2.0 * m >= p => whole[k],
            Some(_) => [a - m, b + m],
            None => [(a - m).max(whole[k][0]), (b + m).min(whole[k][1])],
        }
    });
    let within = |r: [[f64; 2]; 2]| -> Result<Surface, Error> {
        if through {
            crate::geom::Offset::inverted_new(s.clone(), d, r)
                .map(|o| Surface::Offset(std::sync::Arc::new(o)))
        } else {
            s.offset_within(d, r).ok_or(Error::Degenerate)?
        }
    };
    within(grown).or_else(|_| within(rect).map_err(|_| collapses()))
}

/// The face's offset, as [`offset_surface`] finds it; in an envelope, a
/// cylinder, sphere, torus or pipe whose offset passes its centre is kept
/// inverted ([`inverted`]), and any other face whose offset collapses is
/// tried again past its focal surface (design 484): kept where it passes
/// it whole, on its base's parameters and with its sense, its natural
/// normal along `O_u × O_v` as the turned cylinder's and torus's are.
fn envelope_surface(
    body: &Body,
    f: FaceId,
    d: f64,
    mode: Mode,
) -> Result<(Surface, Option<Inversion>), Error> {
    let face = &body.faces[f];
    let s = &body.surfaces[face.surface];
    if mode == Mode::Envelope && d != 0.0 {
        let dn = if face.sense == Sense::Forward { d } else { -d };
        match inverted(s, dn) {
            Some(Some((o, i))) => return Ok((o, Some(i))),
            Some(None) => return Err(OffsetError::FaceCollapses { face: face.name }.into()),
            None => {}
        }
        return match offset_surface(body, f, d) {
            Err(Error::InvalidOffset(OffsetError::FaceCollapses { .. })) if inverts(s) => {
                let o = offset_surface_in(body, f, d, true)?;
                Ok((
                    o,
                    Some(Inversion {
                        map: None,
                        flip: false,
                    }),
                ))
            }
            r => r.map(|s| (s, None)),
        };
    }
    offset_surface(body, f, d).map(|s| (s, None))
}

/// Whether a surface's offset is an [`crate::geom::Offset`], which may
/// pass its focal surface.
fn inverts(s: &Surface) -> bool {
    matches!(
        s,
        Surface::Nurbs(_)
            | Surface::Extrusion(_)
            | Surface::Revolution(_)
            | Surface::Sweep(_)
            | Surface::Offset(_)
            | Surface::Bent(_)
    )
}

/// How the edge's faces move, and for a smooth edge how far apart their
/// offsets lie along it.
fn classify(
    body: &Body,
    distance: &BTreeMap<FaceId, f64>,
    e: EdgeId,
) -> Result<(Class, f64), Error> {
    let at = body.edges[e].name;
    let [fa, fb] = fins(body, e);
    let face = |fin: FinId| body.loops[body.fins[fin].loop_].face;
    let (da, db) = (distance[&face(fa)], distance[&face(fb)]);
    if da != db {
        return Ok((Class::Closing, 0.0));
    }
    if da == 0.0 {
        return Ok((Class::Kept, 0.0));
    }
    let [t0, t1] = body.edges[e].interval;
    let mut gap = 0.0f64;
    let mut smooth = true;
    for i in 0..SAMPLES {
        let t = t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64;
        let (a, b) = (normal(body, fa, t), normal(body, fb, t));
        let angle = crate::math::atan2(a.cross(b).length(), a.dot(b));
        gap = gap.max(angle * da.abs());
        if (angle * da.abs())
            .partial_cmp(&SMOOTH_GAP)
            .is_none_or(|o| o.is_gt())
        {
            smooth = false;
        }
    }
    if smooth {
        return Ok((Class::Smooth, gap));
    }
    let convex = match bend(body, e) {
        Ok(Bend::Convex) => true,
        Ok(Bend::Concave) => false,
        Ok(Bend::Smooth) | Err(BendFail::Changes) => {
            return Err(OffsetError::Unsupported {
                at,
                case: OffsetCase::PartlySmooth,
            }
            .into());
        }
        Err(BendFail::Uncertain) => return Err(crate::OffsetError::Uncertain { at }.into()),
    };
    Ok((
        if convex == (da > 0.0) {
            Class::Opening
        } else {
            Class::Closing
        },
        0.0,
    ))
}
