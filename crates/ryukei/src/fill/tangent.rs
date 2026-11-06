//! Tangency: each targeted side's cross-boundary derivative moved into
//! the plane its target gives, and the angle across it certified.
//!
//! A side's target is a unit normal along it: a face's (a tangent side)
//! or a plane field (an inner edge between patches, which both patches
//! take, so they meet tangentially). The bilinear patch's cross
//! derivative `B_x` there is projected into the plane: `g = -w (B_x · n)
//! n` (`w` the side's weight), less its values at the side's ends taken
//! linearly (a corner can only be tangent to a plane both its sides lie
//! in), is interpolated in the side's basis and becomes the correction's
//! term ([`super::coons::correction`]).
//!
//! The angle between the patch's normal and the target's over a stretch
//! of the side is certified in interval arithmetic: the patch's normal
//! from `S_u × S_v` over the stretch's box ([`Sheet::jet_box`]), the
//! face's from its enclosure over the edge's parameters there
//! ([`convex::Side::normal_box`]), a field's from its formula over the
//! stretch. Stretches whose bound passes the limit are halved; a side
//! that can't be certified has its direction's knots refined and the
//! patch made again.

use super::coons::{self, Net, Quad, U0, U1, V0, V1};
use super::sides::Trace;
use crate::geom::bound2::IVec3;
use crate::geom::nurbs::skin::CompatMap;
use crate::geom::sheet::Sheet;
use crate::geom::{NurbsCurve, NurbsSurface, Vector3};
use crate::interval::Interval;
use crate::work::{Halt, Meter};
use crate::{Error, math};

/// How many times a stretch may be halved before it counts as failed.
pub(crate) const MAX_DEPTH: u32 = 12;
/// How many times a patch's knots may be refined.
pub(crate) const MAX_ROUNDS: u32 = 5;
/// The share of a stretch's edge parameters an enclosure is widened by,
/// for the rounding in mapping them.
const SLACK: f64 = 1e-9;

/// What a side's normal is read from.
pub(crate) enum Target<'s, 'a> {
    /// A face, through the side's trace: the side's `[0, 1]` parameter
    /// goes through `map` to the curve given, then `flip - t` where that
    /// curve ran against the side's loop order.
    Face {
        trace: &'s Trace<'a>,
        map: CompatMap,
        flip: Option<f64>,
    },
    /// A plane field along an inner curve: the normal is `m = (1 - s) a +
    /// s b` made square to the curve's tangent `T` there (`m - (m · T) T /
    /// (T · T)`) and normalized, so the plane holds the curve; `s` the
    /// curve's own `[0, 1]` parameter, `1 - u` where the side runs against
    /// it. `a` and `b` must be square to the tangent at the curve's ends.
    Field {
        a: Vector3,
        b: Vector3,
        curve: &'s NurbsCurve,
        back: bool,
    },
}

impl Target<'_, '_> {
    /// The trace's parameter at the side's `u`.
    fn traced(map: &CompatMap, flip: Option<f64>, u: f64) -> f64 {
        let t = map.to_curve(u);
        flip.map_or(t, |f| f - t)
    }

    /// The unit normal at the side's `u`; zero where there is none.
    pub(crate) fn normal(&self, u: f64) -> Vector3 {
        match self {
            Self::Face { trace, map, flip } => trace.normal(Self::traced(map, *flip, u)),
            Self::Field { a, b, curve, back } => {
                let s = if *back { 1.0 - u } else { u };
                let m = *a * (1.0 - s) + *b * s;
                let t = curve.eval(s).d1;
                let tt = t.dot(t);
                if tt == 0.0 {
                    return Vector3::ZERO;
                }
                (m - t * (m.dot(t) / tt))
                    .normalize()
                    .map_or(Vector3::ZERO, Vector3::from)
            }
        }
    }

    /// An enclosure of the unit normal over the side's stretch `r`, whose
    /// points `p` enclose; `None` where it can't be had.
    fn normal_box(&self, r: [f64; 2], p: IVec3) -> Option<IVec3> {
        match self {
            Self::Face { trace, map, flip } => {
                let (a, b) = (
                    trace.edge_param(Self::traced(map, *flip, r[0])),
                    trace.edge_param(Self::traced(map, *flip, r[1])),
                );
                let (lo, hi) = (a.min(b), a.max(b));
                let pad = SLACK * (hi - lo) + 4.0 * f64::EPSILON * lo.abs().max(hi.abs());
                trace.face.normal_box([lo - pad, hi + pad], p)
            }
            Self::Field { a, b, curve, back } => {
                let (lo, hi) = if *back {
                    (1.0 - r[1], 1.0 - r[0])
                } else {
                    (r[0], r[1])
                };
                let s = Interval::new(lo, hi)?;
                let one = Interval::point(1.0);
                let m: IVec3 = core::array::from_fn(|k| {
                    (one - s) * Interval::point(a.to_array()[k])
                        + s * Interval::point(b.to_array()[k])
                });
                let t: IVec3 = curve.jet_box([lo, hi])[1];
                let tt = dot(t, t);
                if tt.lo() <= 0.0 {
                    return None;
                }
                let f = dot(m, t).checked_div(tt)?;
                unit(core::array::from_fn(|k| m[k] - t[k] * f))
            }
        }
    }
}

/// A side whose angle couldn't be brought within the limit: its place,
/// the largest angle sampled, and whether the samples were within the
/// limit (no bound certified it) or not.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Failed {
    pub(crate) place: usize,
    pub(crate) angle: f64,
    pub(crate) sampled_within: bool,
}

/// What [`patch`] couldn't do.
pub(crate) enum PatchError {
    Error(Error),
    /// The work ran out: [`crate::work::run`] says which way.
    Halt,
    Failed(Failed),
}

impl From<Error> for PatchError {
    fn from(e: Error) -> Self {
        Self::Error(e)
    }
}

impl From<Halt> for PatchError {
    fn from(_: Halt) -> Self {
        Self::Halt
    }
}

/// The patch through `quad`, tangent to each side's target where it has
/// one, within `limit` radians of a face and half that of a field (two
/// patches share each field, and their angles add), and each targeted
/// side's certified angle.
pub(crate) fn patch(
    mut quad: Quad,
    targets: &[Option<Target<'_, '_>>; 4],
    limit: f64,
    meter: &Meter,
) -> Result<(NurbsSurface, Quad, [Option<f64>; 4]), PatchError> {
    if targets.iter().all(Option::is_none) {
        let net = quad.bilinear();
        let s = quad.surface(&net, None)?;
        return Ok((s, quad, [None; 4]));
    }
    let mut round = 0;
    loop {
        let net = quad.bilinear();
        let b = quad.surface(&net, None)?;
        let mut g: [Option<Vec<Vector3>>; 4] = [None, None, None, None];
        for k in 0..4 {
            if let Some(t) = &targets[k] {
                g[k] = Some(fit(&quad, &net, &b, k, t, meter)?);
            }
        }
        let e = coons::correction(&quad, &g);
        let s = quad.surface(&net, Some(&e))?;
        let mut angles = [None; 4];
        let mut failed: Vec<Failed> = Vec::new();
        for k in 0..4 {
            if let Some(t) = &targets[k] {
                let within = match t {
                    Target::Face { .. } => limit,
                    Target::Field { .. } => 0.5 * limit,
                };
                match certify(&s, k, t, within, meter)? {
                    Ok(a) => angles[k] = Some(a),
                    Err(f) => failed.push(f),
                }
            }
        }
        if failed.is_empty() {
            return Ok((s, quad, angles));
        }
        if round == MAX_ROUNDS {
            return Err(PatchError::Failed(failed[0]));
        }
        round += 1;
        let mut dirs = [false; 2];
        for f in &failed {
            dirs[usize::from(f.place == U0 || f.place == U1)] = true;
        }
        for (d, refine) in dirs.into_iter().enumerate() {
            if refine {
                quad.refine(d)?;
            }
        }
    }
}

/// The parameter point on side `k` at its own parameter `t`.
fn at(k: usize, t: f64) -> (f64, f64) {
    match k {
        V0 => (t, 0.0),
        V1 => (t, 1.0),
        U0 => (0.0, t),
        _ => (1.0, t),
    }
}

/// Side `k`'s fitted correction coefficients: `g = -w (B_x · n) n` less
/// its ends taken linearly, interpolated at the side's Greville abscissae.
fn fit(
    quad: &Quad,
    _net: &Net,
    b: &NurbsSurface,
    k: usize,
    target: &Target<'_, '_>,
    meter: &Meter,
) -> Result<Vec<Vector3>, PatchError> {
    let side = &quad.sides[k];
    let knots = side.knots();
    let weight = |t: f64| {
        let basis = knots.basis(t);
        let p = knots.degree();
        basis
            .values(0)
            .iter()
            .enumerate()
            .map(|(r, &n)| n * side.weights()[basis.span() - p + r])
            .sum::<f64>()
    };
    let raw = |t: f64| -> Vector3 {
        let (u, v) = at(k, t);
        let e = b.eval(u, v);
        let cross = if k == V0 || k == V1 { e.dv } else { e.du };
        let n = target.normal(t);
        n * (-weight(t) * cross.dot(n))
    };
    let (g0, g1) = (raw(0.0), raw(1.0));
    let xi = coons::greville(knots);
    let mut values = Vec::with_capacity(xi.len());
    for &t in &xi {
        meter.charge(1)?;
        values.push(raw(t) - g0 * (1.0 - t) - g1 * t);
    }
    // The ends are exactly zero, so the correction keeps the corners.
    let last = values.len() - 1;
    values[0] = Vector3::ZERO;
    values[last] = Vector3::ZERO;
    let mut c = coons::interpolate(knots, &values);
    c[0] = Vector3::ZERO;
    c[last] = Vector3::ZERO;
    Ok(c)
}

/// The largest angle between `s`'s normal along side `k` and the target's,
/// certified: `Ok(Ok(bound))` within `limit`, `Ok(Err(failed))` where a
/// stretch couldn't be certified within it after [`MAX_DEPTH`] halvings.
fn certify(
    s: &NurbsSurface,
    k: usize,
    target: &Target<'_, '_>,
    limit: f64,
    meter: &Meter,
) -> Result<Result<f64, Failed>, Halt> {
    let knots = if k == V0 || k == V1 {
        s.knots()[0].clone()
    } else {
        s.knots()[1].clone()
    };
    let breaks = knots.knots().to_vec();
    let mut stack: Vec<([f64; 2], u32)> =
        breaks.windows(2).rev().map(|w| ([w[0], w[1]], 0)).collect();
    let mut worst: f64 = 0.0;
    let sheet = Sheet::Nurbs(s);
    while let Some((r, depth)) = stack.pop() {
        meter.charge(1)?;
        if let Some(bound) = stretch_bound(&sheet, k, r, target)
            && bound <= limit
        {
            worst = worst.max(bound);
            continue;
        }
        if depth < MAX_DEPTH {
            let m = 0.5 * (r[0] + r[1]);
            stack.push(([m, r[1]], depth + 1));
            stack.push(([r[0], m], depth + 1));
            continue;
        }
        // Not certified: how far off the samples are.
        let sampled = [r[0], 0.5 * (r[0] + r[1]), r[1]]
            .into_iter()
            .map(|t| sampled_angle(s, k, t, target))
            .fold(0.0, f64::max);
        return Ok(Err(Failed {
            place: k,
            angle: sampled,
            sampled_within: sampled <= limit,
        }));
    }
    Ok(Ok(worst))
}

/// A bound on the angle between the patch's normal and the target's over
/// the stretch `r` of side `k`; `None` where either normal can't be
/// enclosed, or their lines can't be told apart from crossing.
fn stretch_bound(sheet: &Sheet<'_>, k: usize, r: [f64; 2], target: &Target<'_, '_>) -> Option<f64> {
    let (c, other) = match k {
        V0 => (0, [0.0, 0.0]),
        V1 => (0, [1.0, 1.0]),
        U0 => (1, [0.0, 0.0]),
        _ => (1, [1.0, 1.0]),
    };
    let bx = if c == 0 { [r, other] } else { [other, r] };
    let jets = sheet.jet_box(bx)?;
    let p: IVec3 = core::array::from_fn(|i| jets[i].v);
    let su: IVec3 = core::array::from_fn(|i| jets[i].d[0]);
    let sv: IVec3 = core::array::from_fn(|i| jets[i].d[1]);
    let np = unit(cross(su, sv))?;
    let nt = target.normal_box(r, p)?;
    let d = dot(np, nt);
    if d.lo() <= 0.0 && d.hi() >= 0.0 {
        return None;
    }
    let x = cross(np, nt);
    let sin = dot(x, x).sqrt()?.hi();
    Some(math::asin(sin.min(1.0)))
}

/// The angle between the lines of the patch's normal and the target's at
/// side `k`'s parameter `t`; a right angle where either has none.
fn sampled_angle(s: &NurbsSurface, k: usize, t: f64, target: &Target<'_, '_>) -> f64 {
    let (u, v) = at(k, t);
    let e = s.eval(u, v);
    let n = e.du.cross(e.dv);
    let m = target.normal(t);
    let (ln, lm) = (n.length(), m.length());
    if ln == 0.0 || lm == 0.0 {
        return core::f64::consts::FRAC_PI_2;
    }
    math::atan2(n.cross(m).length(), n.dot(m).abs())
}

fn cross(a: IVec3, b: IVec3) -> IVec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: IVec3, b: IVec3) -> Interval {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `w / |w|`; `None` where `|w|` may vanish.
fn unit(w: IVec3) -> Option<IVec3> {
    let len = dot(w, w).sqrt()?;
    if len.lo() <= 0.0 {
        return None;
    }
    let mut n = w;
    for x in &mut n {
        *x = x.checked_div(len)?;
    }
    Some(n)
}
