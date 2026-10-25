//! An assembly as a system of equations for `ryusketch::core`.
//!
//! Each free body has six variables: a translation `t` (scaled) and a
//! rotation vector `w`, applied to its stored pose, so the body's
//! rotation is `exp([w]) R₀` and its origin `o₀ + t`. A solve starts at
//! zero, where every body is at its stored pose to the bit. Fixed bodies
//! are constants; copies are functions of their seed's pose and the
//! bodies their pattern's directions are on, never variables.
//!
//! Each mate becomes rows of residuals written once over dual numbers,
//! with as many slots as the bodies a row reads have variables. Each row
//! has exactly one zero near where it holds:
//!
//! - `Along(k, c)`: `(o_b - o_a) · e_k - c`, `e_k` connector `a`'s axis
//!   `k` (a length).
//! - `Axis(k)`: `2 (z_b · e_k) / (1 + z_a · z_b)` for `k` in x, y: twice
//!   the tangent of half the angle between the z axes, along `e_k`; zero
//!   only where they point the same way (an angle).
//! - `Turn(k)`: the same for the whole frame: component `k` of the
//!   rotation from `a`'s frame to `b`'s, `2 tan(θ/2)` about its axis
//!   (Rodrigues' vector), zero only where the frames agree.
//! - `Angle(α)`: `2 tan((φ - α)/2)`, `φ` the turn from `a`'s x to `b`'s
//!   about `a`'s z.
//!
//! Lengths are in solver units: everything is scaled by a power of two
//! at least the assembly's size, which is exact both ways.

use crate::assembly::{Assembly, Connector, Direction, Id, Kind, Motion, On};
use crate::pose::Pose;
use crate::rot::{Rot, V, add, dot, konst, scaled, sub};
use ryusketch::core::dual::D;
use ryusketch::core::{Component, Equations, MAX_STEP};
use ryusketch::{ANGLE_TOL, SOLVE_TOL};
use std::collections::BTreeMap;

/// The longest step a rotation variable takes: a quarter turn.
pub(crate) const MAX_TURN: f64 = core::f64::consts::FRAC_PI_2;

/// Denominators smaller than this are this, so a row stays finite where
/// it can't be read; its misfit is infinite there.
const TINY: f64 = 1e-12;
/// Below this an axis or turn row's denominator (frames more than about
/// 179.9° apart) or an angle's projection can't be judged.
const GUARD: f64 = 1e-6;

/// One residual of a mate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Row {
    Along { k: usize, c: f64 },
    Axis(usize),
    Turn(usize),
    Angle { cos: f64, sin: f64 },
}

impl Row {
    /// Whether it reads directions only, not positions.
    fn turns_only(&self) -> bool {
        !matches!(self, Row::Along { .. })
    }
}

/// An equation: a row of a mate, and the free bodies it reads, by ID,
/// ascending (each its six slots, in that order).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Eq {
    pub mate: Id,
    pub row: Row,
    pub bodies: Vec<Id>,
}

#[derive(Clone, Debug)]
pub(crate) struct System<'a> {
    pub asm: &'a Assembly,
    /// Meters per solver unit: a power of two.
    pub scale: f64,
    /// Each free body's first variable.
    pub var_of: BTreeMap<Id, usize>,
    /// Each variable's body, and which of its six it is.
    pub owner: Vec<(Id, usize)>,
    pub eqs: Vec<Eq>,
    pub components: Vec<Component>,
    /// Equations with no variables: always true or always false.
    pub constant_eqs: Vec<usize>,
}

/// The smallest power of two at least `m` (1 for 0).
fn power_of_two_above(m: f64) -> f64 {
    let mut s = 1.0;
    if m == 0.0 {
        return s;
    }
    while s < m {
        s *= 2.0;
    }
    while s / 2.0 >= m {
        s /= 2.0;
    }
    s
}

/// The rows a mate becomes, in order; lengths in meters (scaled later).
pub(crate) fn rows(kind: &Kind) -> Vec<Row> {
    let along = |k| Row::Along { k, c: 0.0 };
    let angle = |a: &Option<f64>| {
        a.map(|a| {
            let (sin, cos) = ryukei::math::sin_cos(a);
            Row::Angle { cos, sin }
        })
    };
    let distance = |d: &Option<f64>| d.map(|c| Row::Along { k: 2, c });
    let mut out = Vec::new();
    match kind {
        Kind::Fastened => {
            out.extend([along(0), along(1), along(2)]);
            out.extend([Row::Turn(0), Row::Turn(1), Row::Turn(2)]);
        }
        Kind::Revolute { angle: a } => {
            out.extend([along(0), along(1), along(2), Row::Axis(0), Row::Axis(1)]);
            out.extend(angle(a));
        }
        Kind::Slider { distance: d } => {
            out.extend([along(0), along(1)]);
            out.extend([Row::Turn(0), Row::Turn(1), Row::Turn(2)]);
            out.extend(distance(d));
        }
        Kind::Cylindrical {
            angle: a,
            distance: d,
        } => {
            out.extend([along(0), along(1), Row::Axis(0), Row::Axis(1)]);
            out.extend(angle(a));
            out.extend(distance(d));
        }
        Kind::Planar {
            angle: a,
            distance: d,
        } => {
            out.extend([Row::Along { k: 2, c: *d }, Row::Axis(0), Row::Axis(1)]);
            out.extend(angle(a));
        }
        Kind::Ball => out.extend([along(0), along(1), along(2)]),
    }
    out
}

/// The scale: a power of two at least every coordinate, offset and
/// distance in the assembly.
fn scale(asm: &Assembly) -> f64 {
    let mut m: f64 = 0.0;
    let big = |p: &Pose| p.origin.iter().fold(0.0, |m: f64, x| m.max(x.abs()));
    for b in asm.bodies.values() {
        m = m.max(big(&b.pose));
    }
    let spacing = |p: Id, index: [u32; 2]| match &asm.patterns[&p].motion {
        Motion::Linear {
            spacing, second, ..
        } => {
            (spacing * f64::from(index[0])).abs()
                + second.map_or(0.0, |(_, s)| (s * f64::from(index[1])).abs())
        }
        Motion::Circular { .. } => 0.0,
    };
    for mate in asm.mates.values() {
        for c in [&mate.a, &mate.b] {
            m = m.max(big(&c.frame));
            if let On::Copy { pattern, index, .. } = c.on {
                m = m.max(spacing(pattern, index));
            }
        }
        for r in rows(&mate.kind) {
            if let Row::Along { c, .. } = r {
                m = m.max(c.abs());
            }
        }
    }
    for p in asm.patterns.values() {
        let dir = |d: &Direction| match d {
            Direction::Connector(c) => big(&c.frame),
            Direction::World(_) => 0.0,
        };
        match &p.motion {
            Motion::Linear {
                direction, second, ..
            } => {
                m = m.max(dir(direction));
                if let Some((d, _)) = second {
                    m = m.max(dir(d));
                }
            }
            Motion::Circular { axis, .. } => m = m.max(dir(axis)),
        }
    }
    power_of_two_above(m)
}

impl<'a> System<'a> {
    /// The system of an assembly that has passed
    /// [`Assembly::validate`].
    pub fn new(asm: &'a Assembly) -> System<'a> {
        let s = scale(asm);
        let mut var_of = BTreeMap::new();
        let mut owner = Vec::new();
        for (&id, b) in &asm.bodies {
            if !b.fixed {
                var_of.insert(id, owner.len());
                owner.extend((0..6).map(|k| (id, k)));
            }
        }
        let mut eqs = Vec::new();
        for (&mid, m) in &asm.mates {
            let mut bodies: Vec<Id> = asm
                .bodies_of(&m.a.on)
                .into_iter()
                .chain(asm.bodies_of(&m.b.on))
                .filter(|b| var_of.contains_key(b))
                .collect();
            bodies.sort_unstable();
            bodies.dedup();
            for row in rows(&m.kind) {
                let row = match row {
                    Row::Along { k, c } => Row::Along { k, c: c / s },
                    r => r,
                };
                eqs.push(Eq {
                    mate: mid,
                    row,
                    bodies: bodies.clone(),
                });
            }
        }
        let mut sys = System {
            asm,
            scale: s,
            var_of,
            owner,
            eqs,
            components: Vec::new(),
            constant_eqs: Vec::new(),
        };
        sys.split();
        sys
    }

    /// Splits variables and equations into components: a body's six
    /// variables always together, bodies together when an equation reads
    /// both.
    fn split(&mut self) {
        let bodies: Vec<Id> = self.var_of.keys().copied().collect();
        let at = |b: &Id| bodies.binary_search(b).expect("a free body");
        let mut parent: Vec<usize> = (0..bodies.len()).collect();
        fn find(parent: &mut [usize], i: usize) -> usize {
            let mut r = i;
            while parent[r] != r {
                r = parent[r];
            }
            let mut q = i;
            while parent[q] != r {
                let next = parent[q];
                parent[q] = r;
                q = next;
            }
            r
        }
        for eq in &self.eqs {
            for w in eq.bodies.windows(2) {
                let (a, b) = (find(&mut parent, at(&w[0])), find(&mut parent, at(&w[1])));
                if a != b {
                    parent[a.max(b)] = a.min(b);
                }
            }
        }
        let mut by_root: BTreeMap<usize, Component> = BTreeMap::new();
        for (i, b) in bodies.iter().enumerate() {
            let r = find(&mut parent, i);
            let first = self.var_of[b];
            by_root.entry(r).or_default().vars.extend(first..first + 6);
        }
        for (i, eq) in self.eqs.iter().enumerate() {
            match eq.bodies.first() {
                Some(b) => {
                    let r = find(&mut parent, at(b));
                    by_root.get_mut(&r).expect("a root").eqs.push(i);
                }
                None => self.constant_eqs.push(i),
            }
        }
        // Roots are their component's smallest body, so this is that
        // order, and each component's variables ascend.
        self.components = by_root.into_values().collect();
    }

    /// The tolerance of equation `i`, in solver units.
    fn tol(&self, i: usize) -> f64 {
        match self.eqs[i].row {
            Row::Along { .. } => SOLVE_TOL / self.scale,
            _ => ANGLE_TOL,
        }
    }

    /// Body `b`'s turn and origin (scaled) at `x`, over duals: its
    /// variables are inputs at slots `6k..6k + 6` if it is the `k`th of
    /// `bodies`, else constants.
    fn body<const N: usize>(&self, b: Id, bodies: &[Id], x: &[f64]) -> (Rot<N>, V<N>) {
        let stored = &self.asm.bodies[&b].pose;
        let o0 = konst(stored.origin.map(|v| v / self.scale));
        match (bodies.iter().position(|&q| q == b), self.var_of.get(&b)) {
            (Some(k), Some(&v)) => {
                let input = |i: usize| D::input(x[v + i], 6 * k + i);
                let t = [input(0), input(1), input(2)];
                let w = [input(3), input(4), input(5)];
                (Rot::exp(w), add(o0, t))
            }
            _ => (Rot::identity(), o0),
        }
    }

    /// Connector `c`'s frame in solver units at `x`: its origin and axes.
    fn frame<const N: usize>(&self, c: &Connector, bodies: &[Id], x: &[f64]) -> (V<N>, [V<N>; 3]) {
        match c.on {
            On::Body(b) => self.on_body(b, &c.frame, bodies, x),
            On::Copy {
                pattern,
                seed,
                index,
            } => {
                let (o, axes) = self.on_body(seed, &c.frame, bodies, x);
                self.moved(pattern, index, o, axes, bodies, x)
            }
        }
    }

    /// A frame given in body `b`'s coordinates, in the assembly's.
    fn on_body<const N: usize>(
        &self,
        b: Id,
        frame: &Pose,
        bodies: &[Id],
        x: &[f64],
    ) -> (V<N>, [V<N>; 3]) {
        let stored = &self.asm.bodies[&b].pose;
        let (turn, origin) = self.body::<N>(b, bodies, x);
        // The frame as the stored pose turns it, turned by the variables.
        let p = konst(stored.turn(frame.origin).map(|v| v / self.scale));
        let axes = [0, 1, 2].map(|k| turn.apply(konst(stored.turn(frame.axis(k)))));
        (add(turn.apply(p), origin), axes)
    }

    /// A direction's unit vector and a point on it, solver units.
    fn direction<const N: usize>(&self, d: &Direction, bodies: &[Id], x: &[f64]) -> (V<N>, V<N>) {
        match d {
            Direction::World(v) => (konst(*v), konst([0.0; 3])),
            Direction::Connector(c) => {
                let (o, axes) = self.frame::<N>(c, bodies, x);
                (axes[2], o)
            }
        }
    }

    /// A frame of a seed moved to copy `index` of `pattern`.
    fn moved<const N: usize>(
        &self,
        pattern: Id,
        index: [u32; 2],
        o: V<N>,
        axes: [V<N>; 3],
        bodies: &[Id],
        x: &[f64],
    ) -> (V<N>, [V<N>; 3]) {
        let [i, j] = index.map(f64::from);
        match &self.asm.patterns[&pattern].motion {
            Motion::Linear {
                direction,
                spacing,
                second,
            } => {
                let (d, _) = self.direction::<N>(direction, bodies, x);
                let mut o = add(o, scaled(d, i * spacing / self.scale));
                if let Some((d2, s2)) = second {
                    let (d2, _) = self.direction::<N>(d2, bodies, x);
                    o = add(o, scaled(d2, j * s2 / self.scale));
                }
                (o, axes)
            }
            Motion::Circular { axis, step } => {
                let (n, q) = self.direction::<N>(axis, bodies, x);
                let r = Rot::about(n, i * step);
                (add(q, r.apply(sub(o, q))), axes.map(|a| r.apply(a)))
            }
        }
    }

    /// Equation `i` at `x`, over `N` slots (six per body it reads), and
    /// whether it can be judged there.
    fn eval<const N: usize>(&self, i: usize, x: &[f64]) -> (D<N>, bool) {
        let eq = &self.eqs[i];
        let m = &self.asm.mates[&eq.mate];
        let (oa, a) = self.frame::<N>(&m.a, &eq.bodies, x);
        let (ob, b) = self.frame::<N>(&m.b, &eq.bodies, x);
        let safe = |d: D<N>| {
            if d.v.abs() < TINY {
                D::constant(TINY)
            } else {
                d
            }
        };
        match eq.row {
            Row::Along { k, c } => (dot(sub(ob, oa), a[k]) - D::constant(c), true),
            Row::Axis(k) => {
                let den = D::constant(1.0) + dot(a[2], b[2]);
                (dot(b[2], a[k]).scale(2.0) / safe(den), den.v > GUARD)
            }
            Row::Turn(k) => {
                let comp = match k {
                    0 => dot(a[2], b[1]) - dot(a[1], b[2]),
                    1 => dot(a[0], b[2]) - dot(a[2], b[0]),
                    _ => dot(a[1], b[0]) - dot(a[0], b[1]),
                };
                let den = D::constant(1.0) + dot(a[0], b[0]) + dot(a[1], b[1]) + dot(a[2], b[2]);
                (comp.scale(2.0) / safe(den), den.v > GUARD)
            }
            Row::Angle { cos, sin } => {
                let (u, v) = (dot(b[0], a[0]), dot(b[0], a[1]));
                let c = u.scale(cos) + v.scale(sin);
                let s = v.scale(cos) - u.scale(sin);
                let p = (c * c + s * s).sqrt();
                let den = p + c;
                (s.scale(2.0) / safe(den), p.v > GUARD && den.v > GUARD)
            }
        }
    }

    /// [`eval`](Self::eval) with as many slots as equation `i` needs, as
    /// its value and its gradient by slot.
    fn value(&self, i: usize, x: &[f64]) -> (f64, Vec<f64>, bool) {
        fn take<const N: usize>(s: &System, i: usize, x: &[f64]) -> (f64, Vec<f64>, bool) {
            let (d, ok) = s.eval::<N>(i, x);
            (d.v, d.g.to_vec(), ok)
        }
        match self.eqs[i].bodies.len() {
            0 | 1 => take::<6>(self, i, x),
            2 => take::<12>(self, i, x),
            3 => take::<18>(self, i, x),
            4 => take::<24>(self, i, x),
            5 => take::<30>(self, i, x),
            _ => take::<36>(self, i, x),
        }
    }

    /// The variable at slot `k` of equation `i`.
    fn var_at(&self, i: usize, k: usize) -> usize {
        self.var_of[&self.eqs[i].bodies[k / 6]] + k % 6
    }

    /// The slots equation `i` reads: a row of directions only reads its
    /// bodies' turns.
    fn slots(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let eq = &self.eqs[i];
        let turns = eq.row.turns_only();
        (0..6 * eq.bodies.len()).filter(move |k| !turns || k % 6 >= 3)
    }

    pub fn misfit_at(&self, i: usize, x: &[f64]) -> f64 {
        let (v, _, ok) = self.value(i, x);
        if !ok {
            return f64::INFINITY;
        }
        v.abs() / self.tol(i)
    }
}

impl Equations for System<'_> {
    fn reads(&self, eq: usize) -> Vec<usize> {
        self.slots(eq).map(|k| self.var_at(eq, k)).collect()
    }

    fn residual(&self, eq: usize, x: &[f64]) -> f64 {
        self.value(eq, x).0
    }

    fn misfit(&self, eq: usize, x: &[f64]) -> f64 {
        self.misfit_at(eq, x)
    }

    fn gradient(
        &self,
        eq: usize,
        x: &[f64],
        col: &dyn Fn(usize) -> Option<usize>,
        row: &mut [f64],
    ) -> f64 {
        let (v, g, _) = self.value(eq, x);
        for k in self.slots(eq) {
            if let Some(j) = col(self.var_at(eq, k)) {
                row[j] += g[k];
            }
        }
        v
    }

    fn partials(&self, eq: usize, x: &[f64]) -> Vec<(usize, f64)> {
        let (_, g, _) = self.value(eq, x);
        self.slots(eq).map(|k| (self.var_at(eq, k), g[k])).collect()
    }

    fn group(&self, eq: usize) -> Option<u32> {
        Some(self.eqs[eq].mate)
    }

    fn conflict_tol(&self) -> f64 {
        SOLVE_TOL / self.scale
    }

    fn max_step(&self, var: usize) -> f64 {
        if self.owner[var].1 < 3 {
            MAX_STEP
        } else {
            MAX_TURN
        }
    }
}

/// Body `b`'s pose after variables `x`: its stored pose turned by
/// `exp([w])` and moved by `t`, meters. A body whose variables are all
/// zero keeps its stored pose to the bit.
pub(crate) fn pose_at(sys: &System, b: Id, x: &[f64]) -> Pose {
    let stored = sys.asm.bodies[&b].pose;
    let Some(&v) = sys.var_of.get(&b) else {
        return stored;
    };
    if x[v..v + 6].iter().all(|&q| q == 0.0) {
        return stored;
    }
    let w: V<1> = konst([x[v + 3], x[v + 4], x[v + 5]]);
    let turn = Rot::exp(w);
    let cols = [0, 1, 2].map(|k| turn.apply(konst(stored.axis(k))).map(|d| d.v));
    let s = sys.scale;
    let origin = [0, 1, 2].map(|i| (stored.origin[i] / s + x[v + i]) * s);
    Pose::from_axes(origin, cols[0], cols[1], cols[2])
}

/// Where copies of seeds are, given every body's pose in `asm`: each
/// `(pattern, seed, index)` is the seed's pose moved by the pattern's
/// motion.
pub fn copy_poses(asm: &Assembly, copies: &[(Id, Id, [u32; 2])]) -> Vec<Pose> {
    let sys = System::new(asm);
    let x = vec![0.0; sys.owner.len()];
    let s = sys.scale;
    let v = |a: V<1>| a.map(|d| d.v);
    copies
        .iter()
        .map(|&(pattern, seed, index)| {
            let c = Connector {
                on: On::Copy {
                    pattern,
                    seed,
                    index,
                },
                frame: Pose::IDENTITY,
            };
            let (o, axes) = sys.frame::<1>(&c, &[], &x);
            Pose::from_axes(v(o).map(|c| c * s), v(axes[0]), v(axes[1]), v(axes[2]))
        })
        .collect()
}
