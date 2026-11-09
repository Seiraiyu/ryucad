//! Draft (design 384): faces tilted to an angle from a pull direction
//! about a neutral plane or a parting line, so a part releases from a
//! mould. A drafted face's surface is ruled from its hinge: a plane
//! through a line, a cone through a circle about an axis along the pull,
//! else a sweep of a straight profile along the hinge in the binormal
//! frame of the pull.

use super::engine::{self, Changes};
use super::{Motion, Topology, chain, resolve};
use crate::geom::{Frame, Plane, Point3, Surface, UnitVector3, Vector3};
use crate::name::{Journal, Name, OpKey};
use crate::tol::ANGULAR_RESOLUTION;
use crate::topo::{Body, EdgeId, EntityId, FaceId};
use crate::work::{Meter, Work};
use crate::{Error, TweakError};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) mod hinge;
pub(crate) mod report;
pub(crate) mod surface;

/// The largest draft angle's size: a right angle less a hundredth of a
/// radian.
pub const MAX_ANGLE: f64 = core::f64::consts::FRAC_PI_2 - 0.01;

/// A draft: faces tilted to `angle` from `pull` about `hinge`. A positive
/// angle tapers a boss inward going along the pull and opens a hole
/// along it. With `tangent`, faces tangent to a drafted face across a
/// smooth edge join the draft.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftSpec {
    pub pull: UnitVector3,
    pub angle: f64,
    pub hinge: Hinge,
    pub faces: Vec<DraftFace>,
    pub tangent: bool,
    /// Whether the tweak may change the body's topology.
    pub topology: Topology,
}

/// What drafted faces turn about.
#[derive(Clone, Debug, PartialEq)]
pub enum Hinge {
    /// The plane through `origin` square to the pull.
    Neutral { origin: Point3 },
    /// Edges on the drafted faces' boundaries, which stay: a face on the
    /// pull's side of its hinge drafts along the pull, one on the other
    /// side against it.
    PartingLine(Vec<Name>),
}

/// A face to draft, at its own angle if given, its pull reversed by
/// `flip`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DraftFace {
    pub face: Name,
    pub angle: Option<f64>,
    pub flip: bool,
}

/// A drafted face, as the engine reads it: its hinge, what the hinge
/// lies on, its pull and signed angle.
#[derive(Clone, Debug)]
pub(crate) struct Drafted {
    pub(crate) hinge: hinge::HingeCurve,
    pub(crate) on: HingeOn,
    pub(crate) pull: crate::geom::UnitVector3,
    pub(crate) angle: f64,
}

/// What a hinge lies on.
#[derive(Clone, Debug)]
pub(crate) enum HingeOn {
    /// The neutral plane.
    Plane(crate::geom::Surface),
    /// The face's parting-line edges.
    Edges(Vec<crate::topo::EdgeId>),
}

/// What a draft made of each face: its least and greatest angle from
/// its pull (radians, positive along the pull's side the draft tilts
/// to), certified. Exactly the angle asked for wherever the hinge is
/// square to the pull (every neutral plane); a parting line climbing
/// along the pull holds the angle in the section square to the hinge
/// instead, and this says what the face's angle is.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftReport {
    pub faces: Vec<FaceAngle>,
}

/// One drafted face's angles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceAngle {
    pub face: crate::name::Name,
    pub least: f64,
    pub greatest: f64,
}

/// An angle a draft takes: finite, its size in
/// `[ANGULAR_RESOLUTION, MAX_ANGLE]`.
fn usable(a: f64) -> bool {
    a.is_finite() && (ANGULAR_RESOLUTION..=MAX_ANGLE).contains(&a.abs())
}

/// `body` with `spec`'s faces drafted: a new body, the journal and each
/// drafted face's angles. Refusals are `Error::InvalidTweak`, made before
/// anything is built.
pub fn draft(
    key: OpKey,
    body: &Body,
    spec: &DraftSpec,
) -> Result<(Body, Journal, DraftReport), Error> {
    draft_with(key, body, spec, &Work::unlimited())
}

/// [`draft`] under `work`: refused with `Cancelled` or `BudgetExceeded`
/// when it stops, building nothing.
pub fn draft_with(
    key: OpKey,
    body: &Body,
    spec: &DraftSpec,
    work: &Work,
) -> Result<(Body, Journal, DraftReport), Error> {
    crate::work::run(work, |meter| draft_in(key, body, spec, meter))
}

fn draft_in(
    key: OpKey,
    body: &Body,
    spec: &DraftSpec,
    meter: &Meter,
) -> Result<(Body, Journal, DraftReport), Error> {
    let pull = spec.pull.get();
    if ![pull.x, pull.y, pull.z].iter().all(|x| x.is_finite()) {
        return Err(TweakError::NotFinite.into());
    }
    if !usable(spec.angle)
        || spec
            .faces
            .iter()
            .any(|f| f.angle.is_some_and(|a| !usable(a)))
    {
        return Err(TweakError::BadAngle.into());
    }
    let faces = resolve(body, spec.faces.iter().map(|f| f.face))?;
    let (plane, hinges): (Option<Surface>, BTreeSet<EdgeId>) = match &spec.hinge {
        Hinge::Neutral { origin } => {
            if ![origin.x, origin.y, origin.z].iter().all(|c| c.is_finite()) {
                return Err(TweakError::NotFinite.into());
            }
            let frame = Frame::new(*origin, pull, any_square(pull))?;
            (Some(Surface::Plane(Plane::new(frame))), BTreeSet::new())
        }
        Hinge::PartingLine(names) => {
            let edges = names
                .iter()
                .map(|&n| match body.by_name(n) {
                    Some(EntityId::Edge(e)) => Ok(e),
                    _ => Err(Error::from(TweakError::NoSuchFace { face: n })),
                })
                .collect::<Result<_, _>>()?;
            (None, edges)
        }
    };
    // Each named face's hinge first (a parting line's side picks its
    // pull), then its motion.
    let mut drafts: BTreeMap<FaceId, Drafted> = BTreeMap::new();
    let mut motion: BTreeMap<FaceId, Motion> = BTreeMap::new();
    for (&f, df) in faces.iter().zip(&spec.faces) {
        laminar_free(body, f)?;
        let angle = df.angle.unwrap_or(spec.angle);
        let (h, on) = hinge_of(body, f, plane.as_ref(), &hinges, meter)?;
        let mut sign = if df.flip { -1.0 } else { 1.0 };
        if matches!(on, HingeOn::Edges(_)) && !on_pull_side(body, f, &h, pull) {
            sign = -sign;
        }
        let p = UnitVector3::exact(pull * sign).unwrap_or(spec.pull);
        motion.insert(f, Motion::Draft { pull: p, angle });
        drafts.insert(
            f,
            Drafted {
                hinge: h,
                on,
                pull: p,
                angle,
            },
        );
    }
    if spec.tangent {
        motion = chain::grow(body, motion)?;
        for (&f, m) in &motion {
            if drafts.contains_key(&f) {
                continue;
            }
            laminar_free(body, f)?;
            let Motion::Draft { pull: p, angle } = *m else {
                unreachable!("drafts grow drafts")
            };
            let (h, on) = hinge_of(body, f, plane.as_ref(), &hinges, meter)?;
            drafts.insert(
                f,
                Drafted {
                    hinge: h,
                    on,
                    pull: p,
                    angle,
                },
            );
        }
    }
    let mut changes = Changes {
        motion,
        hinges,
        ..Changes::default()
    };
    for (&f, d) in &drafts {
        let (s, sense) = surface::drafted(body, f, &d.hinge, d.pull, d.angle, meter)?;
        changes.surface.insert(f, s);
        changes.senses.insert(f, sense);
    }
    changes.drafts = drafts;
    let (out, journal) = engine::run(key, body, &changes, meter)?;
    let named: Vec<(Name, UnitVector3)> = changes
        .drafts
        .iter()
        .map(|(&f, d)| (body.faces[f].name, d.pull))
        .collect();
    let report = report::report(&out, &named)?;
    Ok((out, journal, report))
}

/// A direction square to `d`.
fn any_square(d: Vector3) -> Vector3 {
    let a = if d.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    a - d * a.dot(d)
}

/// `LaminarFace` for a face with a laminar edge: where a sheet's
/// boundary goes on its draft is undefined.
fn laminar_free(body: &Body, f: FaceId) -> Result<(), Error> {
    let laminar = body.face_loops(f).any(|l| {
        body.loop_fins(l)
            .any(|fin| body.is_laminar(body.fins[fin].edge))
    });
    if laminar {
        return Err(TweakError::LaminarFace {
            face: body.faces[f].name,
        }
        .into());
    }
    Ok(())
}

/// Face `f`'s hinge: on the neutral plane, else on the parting line.
fn hinge_of(
    body: &Body,
    f: FaceId,
    plane: Option<&Surface>,
    hinges: &BTreeSet<EdgeId>,
    meter: &Meter,
) -> Result<(hinge::HingeCurve, HingeOn), Error> {
    match plane {
        Some(p) => Ok((
            hinge::neutral(body, f, p, meter)?,
            HingeOn::Plane(p.clone()),
        )),
        None => {
            let (h, on) = hinge::parting(body, f, hinges)?;
            Ok((h, HingeOn::Edges(on)))
        }
    }
}

/// Whether face `f`'s middle lies on the pull's side of its hinge (or on
/// it).
fn on_pull_side(body: &Body, f: FaceId, h: &hinge::HingeCurve, pull: Vector3) -> bool {
    let s = &body.surfaces[body.faces[f].surface];
    let rect = crate::intersect::face_rect(body, f).unwrap_or(s.domain());
    let m = s
        .eval(
            (rect[0][0] + rect[0][1]) / 2.0,
            (rect[1][0] + rect[1][1]) / 2.0,
        )
        .point;
    let foot = h.curve.project(m).point;
    (m - foot).dot(pull) >= 0.0
}
