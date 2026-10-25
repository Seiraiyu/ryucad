//! A sketch as a system of equations: variables, the equations its
//! constraints become, and the independent components they split into.
//!
//! Coincident points (and the centers of concentric curves) are merged
//! first, as SolveSpace does: they share variables, so most coincidences
//! cost no equation at all. A coincidence that closes a loop of others is
//! redundant by structure, not by a rank test.
//!
//! Everything is scaled by a power of two near the sketch's size, which is
//! exact both ways, so the solver works on numbers near one.

use crate::core::Equations;
use crate::equation::{Form, Rad};
use crate::sketch::{Constraint, Entity, Id, Kind, Sketch};
use ryukei::math;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Every length-like residual must be within this (meters) for a sketch to
/// be solved.
pub const SOLVE_TOL: f64 = 1e-10;

/// Every angle-like residual (the sine in a parallel, perpendicular or
/// angle constraint) must be within this. It doesn't depend on the
/// sketch's scale, so a sketch solved at one scale is solved at any.
pub const ANGLE_TOL: f64 = 1e-12;

/// An equation's input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Scalar {
    Var(usize),
    Const(f64),
}

/// Which coordinate of what a variable is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Coord {
    X,
    Y,
    R,
}

/// What an equation comes from: a constraint, or an arc keeping its end
/// on its circle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Owner {
    Constraint(Id),
    Arc(Id),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Equation {
    pub form: Form,
    pub args: Vec<Scalar>,
    pub owner: Owner,
}

/// Constraints that are redundant or in conflict by structure alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fault {
    pub conflicting: bool,
    pub constraints: Vec<Id>,
}

pub(crate) use crate::core::Component;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct System {
    /// Meters per solver unit: a power of two.
    pub scale: f64,
    /// Variables, scaled.
    pub vars: Vec<f64>,
    /// What each variable moves: the entities (every point merged into it,
    /// or a circle) and the coordinate.
    pub var_owner: Vec<(Vec<Id>, Coord)>,
    pub eqs: Vec<Equation>,
    pub components: Vec<Component>,
    /// Equations with no variables: always true or always false.
    pub constant_eqs: Vec<usize>,
    pub faults: Vec<Fault>,
    /// Whether merged points already shared a position.
    pub merged_agree: bool,
    /// Each point's merged group, by its smallest point.
    pub(crate) group_of: BTreeMap<Id, Id>,
    /// The two scalars of each group's position.
    pub(crate) group_at: BTreeMap<Id, [Scalar; 2]>,
    /// The scalar of each circle's radius.
    pub(crate) radius_of: BTreeMap<Id, Scalar>,
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

/// The scale: a power of two at least every coordinate, radius and length
/// in the sketch.
fn scale(sketch: &Sketch) -> f64 {
    let mut m: f64 = 0.0;
    for e in sketch.entities.values() {
        match *e {
            Entity::Point { at, .. } => m = m.max(at[0].abs()).max(at[1].abs()),
            Entity::Circle { r, .. } => m = m.max(r),
            _ => {}
        }
    }
    for c in sketch.constraints.values() {
        if let Constraint::Distance(_, _, v)
        | Constraint::DistanceX(_, _, v)
        | Constraint::DistanceY(_, _, v)
        | Constraint::Length(_, v)
        | Constraint::Radius(_, v)
        | Constraint::Diameter(_, v) = *c
        {
            m = m.max(v.abs());
        }
    }
    power_of_two_above(m)
}

/// Union-find over points, keeping the merges as a forest so a loop can be
/// named.
struct Groups {
    parent: BTreeMap<Id, Id>,
    /// Merges that joined two groups: `(point, point, constraint)`.
    tree: Vec<(Id, Id, Id)>,
}

impl Groups {
    fn find(&mut self, p: Id) -> Id {
        let mut r = p;
        while self.parent[&r] != r {
            r = self.parent[&r];
        }
        let mut q = p;
        while self.parent[&q] != r {
            let next = self.parent[&q];
            self.parent.insert(q, r);
            q = next;
        }
        r
    }

    /// The constraints on the tree path from `a` to `b`.
    fn path(&self, a: Id, b: Id) -> Vec<Id> {
        let mut next: BTreeMap<Id, Vec<(Id, Id)>> = BTreeMap::new();
        for &(p, q, c) in &self.tree {
            next.entry(p).or_default().push((q, c));
            next.entry(q).or_default().push((p, c));
        }
        let mut came: BTreeMap<Id, (Id, Id)> = BTreeMap::new();
        let mut todo = VecDeque::from([a]);
        let mut seen = BTreeSet::from([a]);
        while let Some(p) = todo.pop_front() {
            if p == b {
                break;
            }
            for &(q, c) in next.get(&p).into_iter().flatten() {
                if seen.insert(q) {
                    came.insert(q, (p, c));
                    todo.push_back(q);
                }
            }
        }
        let mut out = Vec::new();
        let mut p = b;
        while let Some(&(q, c)) = came.get(&p) {
            out.push(c);
            p = q;
        }
        out
    }
}

/// The point a curve's center is, for concentric merges.
fn center(sketch: &Sketch, id: Id) -> Option<Id> {
    match sketch.entities.get(&id)? {
        Entity::Arc { center, .. } | Entity::Circle { center, .. } => Some(*center),
        _ => None,
    }
}

impl System {
    /// Builds the system of a sketch that has passed
    /// [`Sketch::validate`].
    pub fn new(sketch: &Sketch) -> System {
        let s = scale(sketch);
        let tol = SOLVE_TOL / s;
        let at = |id: Id| match sketch.entities[&id] {
            Entity::Point { at, .. } => [at[0] / s, at[1] / s],
            _ => unreachable!("validated: a point"),
        };
        let fixed = |id: Id| matches!(sketch.entities[&id], Entity::Point { fixed: true, .. });
        // Merge coincident points and concentric centers, in constraint
        // order.
        let mut groups = Groups {
            parent: sketch
                .entities
                .iter()
                .filter(|(_, e)| e.kind() == Kind::Point)
                .map(|(&id, _)| (id, id))
                .collect(),
            tree: Vec::new(),
        };
        let mut faults = Vec::new();
        for (&cid, c) in &sketch.constraints {
            let (a, b) = match *c {
                Constraint::Coincident(a, b) => (a, b),
                Constraint::Concentric(a, b) => (
                    center(sketch, a).expect("validated"),
                    center(sketch, b).expect("validated"),
                ),
                _ => continue,
            };
            let (ra, rb) = (groups.find(a), groups.find(b));
            if ra == rb {
                let mut loop_ = groups.path(a, b);
                loop_.push(cid);
                loop_.sort_unstable();
                faults.push(Fault {
                    conflicting: false,
                    constraints: loop_,
                });
                continue;
            }
            // The smaller point names the group.
            let (keep, gone) = if ra < rb { (ra, rb) } else { (rb, ra) };
            groups.parent.insert(gone, keep);
            groups.tree.push((a, b, cid));
        }
        let points: Vec<Id> = groups.parent.keys().copied().collect();
        let mut members: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        let mut group_of = BTreeMap::new();
        for &p in &points {
            let g = groups.find(p);
            members.entry(g).or_default().push(p);
            group_of.insert(p, g);
        }
        // Each group's position: a fixed member's, else its smallest
        // member's. Two fixed members that disagree conflict.
        let mut vars = Vec::new();
        let mut var_owner = Vec::new();
        let mut group_at = BTreeMap::new();
        let mut merged_agree = true;
        for (&g, ps) in &members {
            let anchors: Vec<Id> = ps.iter().copied().filter(|p| fixed(*p)).collect();
            let home = anchors.first().copied().unwrap_or(g);
            let pos = at(home);
            for &p in ps {
                let q = at(p);
                let off = (q[0] - pos[0]).abs().max((q[1] - pos[1]).abs());
                if off > tol {
                    merged_agree = false;
                    if fixed(p) {
                        let mut path = groups.path(home, p);
                        path.sort_unstable();
                        faults.push(Fault {
                            conflicting: true,
                            constraints: path,
                        });
                    }
                }
            }
            let scalars = if anchors.is_empty() {
                let i = vars.len();
                vars.extend(pos);
                var_owner.push((ps.clone(), Coord::X));
                var_owner.push((ps.clone(), Coord::Y));
                [Scalar::Var(i), Scalar::Var(i + 1)]
            } else {
                [Scalar::Const(pos[0]), Scalar::Const(pos[1])]
            };
            group_at.insert(g, scalars);
        }
        let mut radius_of = BTreeMap::new();
        for (&id, e) in &sketch.entities {
            if let Entity::Circle { r, fixed, .. } = *e {
                let sc = if fixed {
                    Scalar::Const(r / s)
                } else {
                    vars.push(r / s);
                    var_owner.push((vec![id], Coord::R));
                    Scalar::Var(vars.len() - 1)
                };
                radius_of.insert(id, sc);
            }
        }
        let mut sys = System {
            scale: s,
            vars,
            var_owner,
            eqs: Vec::new(),
            components: Vec::new(),
            constant_eqs: Vec::new(),
            faults,
            merged_agree,
            group_of,
            group_at,
            radius_of,
        };
        for (&cid, c) in &sketch.constraints {
            sys.add_constraint(sketch, cid, c);
        }
        for (&id, e) in &sketch.entities {
            if let Entity::Arc {
                center, start, end, ..
            } = *e
            {
                let args = [sys.pt(center), sys.pt(end), sys.pt(center), sys.pt(start)].concat();
                sys.push(Form::DistDist, args, Owner::Arc(id));
            }
        }
        sys.split();
        sys
    }

    fn pt(&self, p: Id) -> Vec<Scalar> {
        self.group_at[&self.group_of[&p]].to_vec()
    }

    /// A point's scaled position now.
    pub(crate) fn value(&self, s: Scalar) -> f64 {
        match s {
            Scalar::Var(i) => self.vars[i],
            Scalar::Const(c) => c,
        }
    }

    /// The scalars of a curve's radius, and how to read them.
    fn rad(&self, sketch: &Sketch, id: Id) -> (Rad, Vec<Scalar>) {
        match sketch.entities[&id] {
            Entity::Circle { .. } => (Rad::Var, vec![self.radius_of[&id]]),
            Entity::Arc { center, start, .. } => {
                (Rad::Arc, [self.pt(center), self.pt(start)].concat())
            }
            _ => unreachable!("validated: an arc or circle"),
        }
    }

    fn line(&self, sketch: &Sketch, id: Id) -> (Id, Id) {
        match sketch.entities[&id] {
            Entity::Line { a, b, .. } => (a, b),
            _ => unreachable!("validated: a line"),
        }
    }

    fn push(&mut self, form: Form, args: Vec<Scalar>, owner: Owner) {
        debug_assert_eq!(form.slots(), args.len());
        self.eqs.push(Equation { form, args, owner });
    }

    /// The value of `form` at the current variables.
    fn now(&self, form: Form, args: &[Scalar]) -> f64 {
        let v: Vec<f64> = args.iter().map(|a| self.value(*a)).collect();
        form.residual(&v).v
    }

    /// +1 or -1: the side `args` (of a signed-distance form with no offset)
    /// are on now, with +1 on the line itself.
    fn side(&self, args: &[Scalar]) -> f64 {
        if self.now(Form::LineDist { sign: 1.0, c: 0.0 }, args) >= 0.0 {
            1.0
        } else {
            -1.0
        }
    }

    /// The merged group a curve's end shares with another curve's end, if
    /// any.
    fn shared_end(&self, sketch: &Sketch, a: Id, b: Id) -> Option<Id> {
        let ends = |id: Id| -> Vec<Id> {
            match sketch.entities[&id] {
                Entity::Line { a, b, .. } => vec![a, b],
                Entity::Arc { start, end, .. } => vec![start, end],
                _ => vec![],
            }
            .into_iter()
            .map(|p| self.group_of[&p])
            .collect()
        };
        let eb = ends(b);
        ends(a).into_iter().find(|g| eb.contains(g))
    }

    fn add_constraint(&mut self, sketch: &Sketch, cid: Id, c: &Constraint) {
        use Constraint as C;
        let s = self.scale;
        let own = Owner::Constraint(cid);
        let kind = |id: Id| sketch.entities[&id].kind();
        match *c {
            C::Coincident(..) | C::Concentric(..) => {}
            C::Horizontal(l) | C::Vertical(l) => {
                let (a, b) = self.line(sketch, l);
                let k = usize::from(matches!(c, C::Horizontal(_)));
                self.push(
                    Form::Diff { c: 0.0 },
                    vec![self.pt(a)[k], self.pt(b)[k]],
                    own,
                );
            }
            C::Parallel(l1, l2) | C::Perpendicular(l1, l2) | C::Angle(l1, l2, _) => {
                let (a, b) = self.line(sketch, l1);
                let (p, q) = self.line(sketch, l2);
                let args = [self.pt(a), self.pt(b), self.pt(p), self.pt(q)].concat();
                let form = match *c {
                    C::Parallel(..) => Form::Cross,
                    C::Perpendicular(..) => Form::Dot,
                    C::Angle(_, _, v) => Form::Angle {
                        cos: math::cos(v),
                        sin: math::sin(v),
                    },
                    _ => unreachable!(),
                };
                self.push(form, args, own);
            }
            C::Tangent(x, y) => self.tangent(sketch, own, x, y),
            C::Equal(x, y) if kind(x) == Kind::Line => {
                let (a, b) = self.line(sketch, x);
                let (p, q) = self.line(sketch, y);
                let args = [self.pt(a), self.pt(b), self.pt(p), self.pt(q)].concat();
                self.push(Form::DistDist, args, own);
            }
            C::Equal(x, y) => {
                let (r1, a1) = self.rad(sketch, x);
                let (r2, a2) = self.rad(sketch, y);
                self.push(Form::Radii { r1, r2 }, [a1, a2].concat(), own);
            }
            C::Symmetric(p, q, l) => {
                let (a, b) = self.line(sketch, l);
                let args = [self.pt(a), self.pt(b), self.pt(p), self.pt(q)].concat();
                self.push(Form::MidLine, args.clone(), own);
                self.push(Form::Along, args, own);
            }
            C::Midpoint(p, l) => {
                let (a, b) = self.line(sketch, l);
                for k in 0..2 {
                    let args = vec![self.pt(a)[k], self.pt(b)[k], self.pt(p)[k]];
                    self.push(Form::Mid, args, own);
                }
            }
            C::On(p, k) => match sketch.entities[&k] {
                Entity::Line { a, b, .. } => {
                    let args = [self.pt(a), self.pt(b), self.pt(p)].concat();
                    self.push(Form::LineDist { sign: 1.0, c: 0.0 }, args, own);
                }
                Entity::Arc { center, .. } | Entity::Circle { center, .. } => {
                    let (r, ra) = self.rad(sketch, k);
                    let args = [self.pt(center), self.pt(p), ra].concat();
                    self.push(Form::DistRad { r }, args, own);
                }
                Entity::Point { .. } => unreachable!("validated: a curve"),
            },
            C::Fix(e) => self.fix(sketch, own, e),
            C::Distance(x, y, v) => {
                let c = v / s;
                match (kind(x), kind(y)) {
                    (Kind::Point, Kind::Point) => {
                        self.push(Form::Dist { c }, [self.pt(x), self.pt(y)].concat(), own);
                    }
                    (Kind::Point, Kind::Line) | (Kind::Line, Kind::Point) => {
                        let (p, l) = if kind(x) == Kind::Point {
                            (x, y)
                        } else {
                            (y, x)
                        };
                        let (a, b) = self.line(sketch, l);
                        let args = [self.pt(a), self.pt(b), self.pt(p)].concat();
                        let sign = self.side(&args);
                        self.push(Form::LineDist { sign, c }, args, own);
                    }
                    _ => {
                        // Both ends of the first line at the distance from
                        // the second.
                        let (a, b) = self.line(sketch, y);
                        let (p, q) = self.line(sketch, x);
                        for end in [p, q] {
                            let args = [self.pt(a), self.pt(b), self.pt(end)].concat();
                            let sign = self.side(&args);
                            self.push(Form::LineDist { sign, c }, args, own);
                        }
                    }
                }
            }
            C::DistanceX(p, q, v) | C::DistanceY(p, q, v) => {
                let k = usize::from(matches!(c, C::DistanceY(..)));
                self.push(
                    Form::Diff { c: v / s },
                    vec![self.pt(p)[k], self.pt(q)[k]],
                    own,
                );
            }
            C::Length(l, v) => {
                let (a, b) = self.line(sketch, l);
                self.push(
                    Form::Dist { c: v / s },
                    [self.pt(a), self.pt(b)].concat(),
                    own,
                );
            }
            C::Radius(k, v) | C::Diameter(k, v) => {
                let (r, args) = self.rad(sketch, k);
                let times = if matches!(c, C::Diameter(..)) {
                    2.0
                } else {
                    1.0
                };
                self.push(
                    Form::Radius {
                        r,
                        k: times,
                        c: v / s,
                    },
                    args,
                    own,
                );
            }
        }
    }

    fn tangent(&mut self, sketch: &Sketch, own: Owner, x: Id, y: Id) {
        let kind = |id: Id| sketch.entities[&id].kind();
        let (x, y) = if kind(y) == Kind::Line {
            (y, x)
        } else {
            (x, y)
        };
        if kind(x) == Kind::Line {
            let (a, b) = self.line(sketch, x);
            let c = center(sketch, y).expect("validated");
            if let Some(g) = self.shared_end(sketch, x, y) {
                // Touching at a shared end: the radius there is square to
                // the line.
                let args = [
                    self.pt(a),
                    self.pt(b),
                    self.pt(c),
                    self.group_at[&g].to_vec(),
                ]
                .concat();
                self.push(Form::Along, args, own);
            } else {
                let line = [self.pt(a), self.pt(b), self.pt(c)].concat();
                let sign = self.side(&line);
                let (r, ra) = self.rad(sketch, y);
                self.push(Form::LineDistRad { sign, r }, [line, ra].concat(), own);
            }
            return;
        }
        let (c1, c2) = (
            center(sketch, x).expect("validated"),
            center(sketch, y).expect("validated"),
        );
        if let Some(g) = self.shared_end(sketch, x, y) {
            let args = [self.group_at[&g].to_vec(), self.pt(c1), self.pt(c2)].concat();
            self.push(Form::Collinear, args, own);
            return;
        }
        let ((r1, a1), (r2, a2)) = (self.rad(sketch, x), self.rad(sketch, y));
        let args = [self.pt(c1), self.pt(c2), a1, a2].concat();
        // Outside or inside, whichever the circles are nearer now; inside,
        // the larger radius counts positive.
        let value =
            |sys: &System, r: Rad, a: &[Scalar]| sys.now(Form::Radius { r, k: 1.0, c: 0.0 }, a);
        let (v1, v2) = (
            value(self, r1, &args[4..4 + r1.slots()]),
            value(self, r2, &args[4 + r1.slots()..]),
        );
        let d = self.now(Form::Dist { c: 0.0 }, &args[..4]);
        let (outer, sign) = if (d - (v1 + v2)).abs() <= (d - (v1 - v2).abs()).abs() {
            (1.0, 1.0)
        } else if v1 >= v2 {
            (1.0, -1.0)
        } else {
            (-1.0, 1.0)
        };
        self.push(
            Form::Circles {
                r1,
                r2,
                sign,
                outer,
            },
            args,
            own,
        );
    }

    fn fix(&mut self, sketch: &Sketch, own: Owner, e: Id) {
        let pin = |sys: &mut System, p: Id| {
            for sc in sys.pt(p) {
                let c = sys.value(sc);
                sys.push(Form::Offset { c }, vec![sc], own);
            }
        };
        match sketch.entities[&e] {
            Entity::Point { .. } => pin(self, e),
            Entity::Line { a, b, .. } => {
                pin(self, a);
                pin(self, b);
            }
            Entity::Circle { center, .. } => {
                pin(self, center);
                let r = self.radius_of[&e];
                let c = self.value(r);
                self.push(Form::Offset { c }, vec![r], own);
            }
            Entity::Arc {
                center, start, end, ..
            } => {
                pin(self, center);
                pin(self, start);
                // The arc keeps its end on its circle; this stops it
                // sliding round.
                let (c, p) = (self.pt(center), self.pt(end));
                let (cx, cy) = (self.value(c[0]), self.value(c[1]));
                let p0 = [self.value(p[0]), self.value(p[1])];
                let (dx, dy) = (p0[0] - cx, p0[1] - cy);
                let n = math::sqrt(dx * dx + dy * dy);
                if n > 0.0 {
                    self.push(
                        Form::Toward {
                            p0,
                            t: [-dy / n, dx / n],
                        },
                        p,
                        own,
                    );
                }
            }
        }
    }

    /// Splits the equations into components, in the order of their
    /// smallest variable.
    fn split(&mut self) {
        let n = self.vars.len();
        let mut parent: Vec<usize> = (0..n).collect();
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
            let vs: Vec<usize> = eq
                .args
                .iter()
                .filter_map(|a| match a {
                    Scalar::Var(i) => Some(*i),
                    Scalar::Const(_) => None,
                })
                .collect();
            for w in vs.windows(2) {
                let (a, b) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
                if a != b {
                    parent[a.max(b)] = a.min(b);
                }
            }
        }
        let mut by_root: BTreeMap<usize, Component> = BTreeMap::new();
        for v in 0..n {
            let r = find(&mut parent, v);
            by_root
                .entry(r)
                .or_insert_with(|| Component {
                    vars: Vec::new(),
                    eqs: Vec::new(),
                })
                .vars
                .push(v);
        }
        for (i, eq) in self.eqs.iter().enumerate() {
            let first = eq.args.iter().find_map(|a| match a {
                Scalar::Var(v) => Some(*v),
                Scalar::Const(_) => None,
            });
            match first {
                Some(v) => {
                    let r = find(&mut parent, v);
                    by_root
                        .get_mut(&r)
                        .expect("every var has a root")
                        .eqs
                        .push(i);
                }
                None => self.constant_eqs.push(i),
            }
        }
        // A root is its component's smallest variable, so this is that
        // order.
        self.components = by_root.into_values().collect();
    }

    /// How close to zero equation `i`'s residual must be.
    pub fn tol(&self, i: usize) -> f64 {
        match self.eqs[i].form {
            Form::Cross | Form::Dot | Form::Angle { .. } => ANGLE_TOL,
            _ => SOLVE_TOL / self.scale,
        }
    }

    /// Equation `i`'s inputs at variables `x`.
    fn inputs(&self, i: usize, x: &[f64]) -> Vec<f64> {
        self.eqs[i]
            .args
            .iter()
            .map(|a| match *a {
                Scalar::Var(k) => x[k],
                Scalar::Const(c) => c,
            })
            .collect()
    }

    /// Equation `i`'s residual at variables `x`.
    pub fn residual(&self, i: usize, x: &[f64]) -> f64 {
        self.eqs[i].form.residual(&self.inputs(i, x)).v
    }

    /// Equation `i`'s residual at `x` as a multiple of its tolerance, so it
    /// holds within 1. Infinite while a line whose direction it reads is
    /// shorter than the length tolerance: such a line has no direction to
    /// hold, and how its residual reads would depend on the scale.
    pub fn misfit(&self, i: usize, x: &[f64]) -> f64 {
        let v = self.inputs(i, x);
        let tol = SOLVE_TOL / self.scale;
        let collapsed = self.eqs[i].form.lines().iter().any(|&k| {
            let (dx, dy) = (v[k + 2] - v[k], v[k + 3] - v[k + 1]);
            (dx * dx + dy * dy).sqrt() < tol
        });
        if collapsed {
            return f64::INFINITY;
        }
        self.eqs[i].form.residual(&v).v.abs() / self.tol(i)
    }

    /// Equation `i`'s partial derivatives at `x`: each variable it reads,
    /// once, in the order it first reads them.
    pub(crate) fn partials(&self, i: usize, x: &[f64]) -> Vec<(usize, f64)> {
        let d = self.eqs[i].form.residual(&self.inputs(i, x));
        let mut out: Vec<(usize, f64)> = Vec::new();
        for (slot, a) in self.eqs[i].args.iter().enumerate() {
            if let Scalar::Var(k) = *a {
                match out.iter_mut().find(|(v, _)| *v == k) {
                    Some(e) => e.1 += d.g[slot],
                    None => out.push((k, d.g[slot])),
                }
            }
        }
        out
    }

    /// Equation `i`'s residual at `x`, and its gradient added into `row`
    /// (indexed by variable through `col`, `None` for variables outside).
    pub fn gradient(
        &self,
        i: usize,
        x: &[f64],
        col: &dyn Fn(usize) -> Option<usize>,
        row: &mut [f64],
    ) -> f64 {
        let eq = &self.eqs[i];
        let v: Vec<f64> = eq
            .args
            .iter()
            .map(|a| match *a {
                Scalar::Var(k) => x[k],
                Scalar::Const(c) => c,
            })
            .collect();
        let d = eq.form.residual(&v);
        for (slot, a) in eq.args.iter().enumerate() {
            if let Scalar::Var(k) = *a
                && let Some(j) = col(k)
            {
                row[j] += d.g[slot];
            }
        }
        d.v
    }
}

/// The sketch's equations to the solver's core: a constraint's equations
/// are its group, an arc's own equation is in every set.
impl Equations for System {
    fn reads(&self, eq: usize) -> Vec<usize> {
        self.eqs[eq]
            .args
            .iter()
            .filter_map(|a| match *a {
                Scalar::Var(v) => Some(v),
                Scalar::Const(_) => None,
            })
            .collect()
    }

    fn residual(&self, eq: usize, x: &[f64]) -> f64 {
        System::residual(self, eq, x)
    }

    fn misfit(&self, eq: usize, x: &[f64]) -> f64 {
        System::misfit(self, eq, x)
    }

    fn gradient(
        &self,
        eq: usize,
        x: &[f64],
        col: &dyn Fn(usize) -> Option<usize>,
        row: &mut [f64],
    ) -> f64 {
        System::gradient(self, eq, x, col, row)
    }

    fn partials(&self, eq: usize, x: &[f64]) -> Vec<(usize, f64)> {
        System::partials(self, eq, x)
    }

    fn group(&self, eq: usize) -> Option<u32> {
        match self.eqs[eq].owner {
            Owner::Constraint(c) => Some(c),
            Owner::Arc(_) => None,
        }
    }

    fn conflict_tol(&self) -> f64 {
        SOLVE_TOL / self.scale
    }

    /// A circle's radius can't go below zero: a step that would turn one
    /// inside out stops it at zero, and the rest of the step stands.
    fn clamp(&self, var: usize, value: f64) -> f64 {
        if self.var_owner[var].1 == Coord::R && value < 0.0 {
            0.0
        } else {
            value
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::tests::{line, point, rectangle};

    fn circle(center: Id, r: f64) -> Entity {
        Entity::Circle {
            center,
            r,
            fixed: false,
            construction: false,
        }
    }

    #[test]
    fn scales_are_powers_of_two() {
        assert_eq!(power_of_two_above(0.0), 1.0);
        assert_eq!(power_of_two_above(1.0), 1.0);
        assert_eq!(power_of_two_above(0.3), 0.5);
        assert_eq!(power_of_two_above(5.0), 8.0);
        assert_eq!(power_of_two_above(1e-3), 1.0 / 512.0);
        let s = System::new(&rectangle());
        assert_eq!(s.scale, 2.0);
        assert_eq!(s.vars, [0.0, 0.0, 1.0, 0.0, 1.0, 0.5, 0.0, 0.5]);
    }

    #[test]
    fn coincident_points_share_variables() {
        let mut sk = rectangle();
        sk.entities.insert(5, point(2.0, 0.0));
        sk.entities.insert(15, line(2, 5));
        sk.constraints.insert(1, Constraint::Coincident(5, 2));
        let s = System::new(&sk);
        // Five points, two of them one: four groups.
        assert_eq!(s.vars.len(), 8);
        assert_eq!(s.group_of[&5], 2);
        assert!(s.merged_agree);
        assert_eq!(s.var_owner[2], (vec![2, 5], Coord::X));
        // Apart, they don't agree yet.
        sk.entities.insert(5, point(3.0, 0.0));
        assert!(!System::new(&sk).merged_agree);
    }

    #[test]
    fn a_loop_of_coincidences_is_redundant_by_structure() {
        let mut sk = rectangle();
        sk.entities.insert(5, point(0.0, 0.0));
        sk.constraints.insert(1, Constraint::Coincident(1, 5));
        sk.constraints.insert(2, Constraint::Coincident(5, 2));
        sk.constraints.insert(3, Constraint::Coincident(2, 1));
        sk.constraints.insert(4, Constraint::Horizontal(11));
        let s = System::new(&sk);
        assert_eq!(
            s.faults,
            [Fault {
                conflicting: false,
                constraints: vec![1, 2, 3]
            }]
        );
    }

    #[test]
    fn fixed_points_that_disagree_conflict() {
        let mut sk = rectangle();
        sk.entities.insert(
            5,
            Entity::Point {
                at: [0.0, 0.0],
                fixed: true,
            },
        );
        sk.entities.insert(
            6,
            Entity::Point {
                at: [1.0, 0.0],
                fixed: true,
            },
        );
        sk.constraints.insert(1, Constraint::Coincident(5, 1));
        sk.constraints.insert(2, Constraint::Coincident(1, 6));
        let s = System::new(&sk);
        assert_eq!(
            s.faults,
            [Fault {
                conflicting: true,
                constraints: vec![1, 2]
            }]
        );
        // The group is constant: no variables for it.
        assert_eq!(s.vars.len(), 6);
    }

    #[test]
    fn components_split_what_doesnt_touch() {
        let mut sk = rectangle();
        sk.entities.insert(5, point(5.0, 5.0));
        sk.entities.insert(20, circle(5, 1.0));
        sk.constraints.insert(1, Constraint::Horizontal(11));
        sk.constraints.insert(2, Constraint::Radius(20, 2.0));
        sk.constraints.insert(3, Constraint::Vertical(12));
        let s = System::new(&sk);
        // Point 1 and 2's y, point 2 and 3's x, the circle's radius; every
        // other variable alone.
        let comps: Vec<(Vec<usize>, usize)> = s
            .components
            .iter()
            .map(|c| (c.vars.clone(), c.eqs.len()))
            .collect();
        assert!(comps.contains(&(vec![1, 3], 1)));
        assert!(comps.contains(&(vec![2, 4], 1)));
        assert!(comps.contains(&(vec![10], 1)));
        assert_eq!(comps.len(), 9);
    }

    #[test]
    fn every_constraint_makes_its_equations() {
        use Constraint as C;
        let mut sk = rectangle();
        sk.entities.insert(5, point(1.0, 3.0));
        sk.entities.insert(6, point(2.0, 3.0));
        sk.entities.insert(7, point(1.0, 2.0));
        sk.entities.insert(20, circle(5, 1.0));
        sk.entities.insert(
            21,
            Entity::Arc {
                center: 5,
                start: 6,
                end: 7,
                construction: false,
            },
        );
        let cases: Vec<(C, usize)> = vec![
            (C::Horizontal(11), 1),
            (C::Parallel(11, 13), 1),
            (C::Angle(11, 12, 0.5), 1),
            (C::Tangent(11, 20), 1),
            (C::Tangent(20, 21), 1),
            (C::Equal(11, 13), 1),
            (C::Equal(20, 21), 1),
            (C::Symmetric(1, 2, 12), 2),
            (C::Midpoint(5, 11), 2),
            (C::On(5, 11), 1),
            (C::On(6, 21), 1),
            (C::Fix(1), 2),
            (C::Fix(11), 4),
            (C::Fix(20), 3),
            (C::Fix(21), 5),
            (C::Distance(1, 2, 1.0), 1),
            (C::Distance(5, 11, 1.0), 1),
            (C::Distance(11, 13, 1.0), 2),
            (C::DistanceY(1, 4, 1.0), 1),
            (C::Length(12, 1.0), 1),
            (C::Diameter(21, 1.0), 1),
            (C::Coincident(1, 5), 0),
            (C::Concentric(20, 21), 0),
        ];
        let arcs = 1;
        for (c, n) in cases {
            let mut one = sk.clone();
            one.constraints.insert(1, c.clone());
            one.validate().unwrap();
            assert_eq!(System::new(&one).eqs.len(), n + arcs, "{c:?}");
        }
    }

    #[test]
    fn tangency_at_a_shared_end_is_square() {
        // A line from (0, 0) to (1, 0), an arc about (1, 1) from (1, 0).
        let mut sk = Sketch::default();
        for (id, x, y) in [(1, 0.0, 0.0), (2, 1.0, 0.0), (3, 1.0, 1.0), (4, 2.0, 1.0)] {
            sk.entities.insert(id, point(x, y));
        }
        sk.entities.insert(10, line(1, 2));
        sk.entities.insert(
            11,
            Entity::Arc {
                center: 3,
                start: 2,
                end: 4,
                construction: false,
            },
        );
        sk.constraints.insert(1, Constraint::Tangent(10, 11));
        let s = System::new(&sk);
        assert_eq!(s.eqs[0].form, Form::Along);
        assert_eq!(s.residual(0, &s.vars), 0.0);
    }
}
