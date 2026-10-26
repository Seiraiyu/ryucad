//! Bodies and journals as bytes, for caches outside the kernel.
//!
//! [`encode_body`] writes every array of a [`Body`] as stored, tombstones
//! included, so IDs and [`Body::compact`] behave the same after
//! [`decode_body`]; every float by its bits; names, tolerances, pcurves
//! and recipes; and each geometry an `Arc` shares (a NURBS, a swept
//! surface, a pipe) once, later uses referring back to it, so a decoded
//! body shares what the encoded one did. `decode_body(&encode_body(b))`
//! dumps exactly as `b` does.
//!
//! The format starts with `ryukei.body.1` (`ryukei.journal.1` for a
//! [`Journal`]); integers are little-endian, counts `u32`, an absent ID
//! `u32::MAX`. A format change bumps the version, and bytes of another
//! version are refused, so a cache entry written by another kernel simply
//! misses. Decoding checks lengths, tags, the geometry's own constructors
//! and [`crate::check::check_topology`], and fails with
//! [`Error::InvalidEncoding`], never a panic, whatever the bytes.

use crate::Error;
use crate::check::check_topology;
use crate::geom::{
    Circle, Circle2, Cone, Curve, Curve2, Cylinder, Ellipse, Ellipse2, Extrusion, Frame, Frame2,
    KnotVector, Line, Line2, NurbsCurve, NurbsCurve2, NurbsSurface, Pipe, Plane, Point2, Point3,
    Revolution, Sphere, Surface, Torus, UnitVector3, Vector2, Vector3,
};
use crate::name::{Event, Journal, Name, ToolKey};
use crate::tol::Tolerance;
use crate::topo::{
    Arena, Body, Edge, Face, Fin, Id, Loop, Pcurve, Region, RegionKind, Sense, Shell, Vertex,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The first bytes of an encoded body.
pub const BODY_MAGIC: &[u8] = b"ryukei.body.1";
/// The first bytes of an encoded journal.
pub const JOURNAL_MAGIC: &[u8] = b"ryukei.journal.1";

fn invalid(what: &'static str) -> Error {
    Error::InvalidEncoding { what }
}

/// The bytes written so far, and the shared geometry already written,
/// by address.
#[derive(Default)]
struct Writer {
    out: Vec<u8>,
    curves: HashMap<usize, u32>,
    curves2: HashMap<usize, u32>,
    surfaces: HashMap<usize, u32>,
    extrusions: HashMap<usize, u32>,
    revolutions: HashMap<usize, u32>,
    pipes: HashMap<usize, u32>,
}

impl Writer {
    fn u8(&mut self, x: u8) {
        self.out.push(x);
    }

    fn u32(&mut self, x: u32) {
        self.out.extend_from_slice(&x.to_le_bytes());
    }

    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("fewer than 2^32 items"));
    }

    fn f64(&mut self, x: f64) {
        self.out.extend_from_slice(&x.to_bits().to_le_bytes());
    }

    fn u128(&mut self, x: u128) {
        self.out.extend_from_slice(&x.to_le_bytes());
    }

    fn name(&mut self, n: Name) {
        self.u128(n.to_u128());
    }

    fn names(&mut self, ns: &[Name]) {
        self.len(ns.len());
        for &n in ns {
            self.name(n);
        }
    }

    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.out.extend_from_slice(s.as_bytes());
    }

    fn id<I: Id>(&mut self, id: I) {
        self.len(id.index());
    }

    fn opt<I: Id>(&mut self, id: Option<I>) {
        match id {
            Some(i) => self.id(i),
            None => self.u32(u32::MAX),
        }
    }

    fn sense(&mut self, s: Sense) {
        self.u8(match s {
            Sense::Forward => 0,
            Sense::Reversed => 1,
        });
    }

    fn point(&mut self, p: Point3) {
        self.f64(p.x);
        self.f64(p.y);
        self.f64(p.z);
    }

    fn vector(&mut self, v: Vector3) {
        self.f64(v.x);
        self.f64(v.y);
        self.f64(v.z);
    }

    fn point2(&mut self, p: Point2) {
        self.f64(p.x);
        self.f64(p.y);
    }

    fn frame(&mut self, f: &Frame) {
        self.point(f.origin());
        self.vector(f.z().get());
        self.vector(f.x().get());
    }

    fn frame2(&mut self, f: &Frame2) {
        self.point2(f.origin());
        self.f64(f.x().x);
        self.f64(f.x().y);
        self.u8(u8::from(f.is_ccw()));
    }

    fn knots(&mut self, k: &KnotVector) {
        self.len(k.degree());
        self.u8(u8::from(k.is_periodic()));
        self.len(k.knots().len());
        for (&x, &m) in k.knots().iter().zip(k.mults()) {
            self.f64(x);
            self.len(m);
        }
    }

    fn floats(&mut self, xs: &[f64]) {
        for &x in xs {
            self.f64(x);
        }
    }

    /// Writes a shared value's back reference if `table` has it (tag 1
    /// and its index), else tag 0, records it and lets `write` write it.
    fn shared<T>(
        &mut self,
        arc: &Arc<T>,
        table: fn(&mut Self) -> &mut HashMap<usize, u32>,
        write: impl FnOnce(&mut Self, &T),
    ) {
        let key = Arc::as_ptr(arc) as usize;
        if let Some(&i) = table(self).get(&key) {
            self.u8(1);
            self.u32(i);
            return;
        }
        self.u8(0);
        write(self, arc);
        let t = table(self);
        let i = t.len() as u32;
        t.insert(key, i);
    }

    fn curve(&mut self, c: &Curve) {
        match c {
            Curve::Line(l) => {
                self.u8(0);
                self.point(l.origin());
                self.vector(l.dir().get());
            }
            Curve::Circle(c) => {
                self.u8(1);
                self.frame(c.frame());
                self.f64(c.radius());
            }
            Curve::Ellipse(e) => {
                self.u8(2);
                self.frame(e.frame());
                self.f64(e.major());
                self.f64(e.minor());
            }
            Curve::Nurbs(n) => {
                self.u8(3);
                self.shared(
                    n,
                    |w| &mut w.curves,
                    |w, n| {
                        w.knots(n.knots());
                        w.len(n.control_points().len());
                        for p in n.control_points() {
                            w.point(p);
                        }
                        w.floats(n.weights());
                    },
                );
            }
        }
    }

    fn curve2(&mut self, c: &Curve2) {
        match c {
            Curve2::Line2(l) => {
                self.u8(0);
                self.point2(l.origin());
                self.f64(l.dir().x);
                self.f64(l.dir().y);
            }
            Curve2::Circle2(c) => {
                self.u8(1);
                self.frame2(c.frame());
                self.f64(c.radius());
            }
            Curve2::Ellipse2(e) => {
                self.u8(2);
                self.frame2(e.frame());
                self.f64(e.major());
                self.f64(e.minor());
            }
            Curve2::Nurbs2(n) => {
                self.u8(3);
                self.shared(
                    n,
                    |w| &mut w.curves2,
                    |w, n| {
                        w.knots(n.knots());
                        w.len(n.control_points().len());
                        for p in n.control_points() {
                            w.point2(p);
                        }
                        w.floats(n.weights());
                    },
                );
            }
        }
    }

    fn surface(&mut self, s: &Surface) {
        match s {
            Surface::Plane(p) => {
                self.u8(0);
                self.frame(p.frame());
            }
            Surface::Cylinder(c) => {
                self.u8(1);
                self.frame(s.frame().expect("analytic"));
                self.f64(c.radius());
            }
            Surface::Cone(c) => {
                self.u8(2);
                self.frame(s.frame().expect("analytic"));
                self.f64(c.radius());
                self.f64(c.half_angle());
            }
            Surface::Sphere(c) => {
                self.u8(3);
                self.frame(s.frame().expect("analytic"));
                self.f64(c.radius());
            }
            Surface::Torus(t) => {
                self.u8(4);
                self.frame(s.frame().expect("analytic"));
                self.f64(t.major());
                self.f64(t.minor());
            }
            Surface::Nurbs(n) => {
                self.u8(5);
                self.shared(
                    n,
                    |w| &mut w.surfaces,
                    |w, n| {
                        let [ku, kv] = n.knots();
                        w.knots(ku);
                        w.knots(kv);
                        w.len(n.control_points().len());
                        for p in n.control_points() {
                            w.point(p);
                        }
                        w.floats(n.weights());
                    },
                );
            }
            Surface::Extrusion(e) => {
                self.u8(6);
                self.shared(
                    e,
                    |w| &mut w.extrusions,
                    |w, e| {
                        w.vector(e.dir().get());
                        w.curve(e.curve());
                    },
                );
            }
            Surface::Revolution(r) => {
                self.u8(7);
                self.shared(
                    r,
                    |w| &mut w.revolutions,
                    |w, r| {
                        w.frame(r.frame());
                        w.curve(r.curve());
                    },
                );
            }
            Surface::Pipe(p) => {
                self.u8(8);
                self.shared(
                    p,
                    |w| &mut w.pipes,
                    |w, p| {
                        w.f64(p.radius());
                        w.curve(p.spine());
                        w.surface(p.anchor());
                    },
                );
            }
        }
    }

    /// An arena: its slot count, then each slot's tag (0 a tombstone, 1
    /// live) and record.
    fn arena<I: Id, T>(&mut self, a: &Arena<I, T>, mut write: impl FnMut(&mut Self, &T)) {
        self.len(a.slots().len());
        for slot in a.slots() {
            match slot {
                None => self.u8(0),
                Some(t) => {
                    self.u8(1);
                    write(self, t);
                }
            }
        }
    }
}

/// `body` as bytes (see the module docs).
pub fn encode_body(body: &Body) -> Vec<u8> {
    let mut w = Writer::default();
    w.out.extend_from_slice(BODY_MAGIC);
    w.arena(&body.regions, |w, r| {
        w.u8(match r.kind {
            RegionKind::Void => 0,
            RegionKind::Solid => 1,
        });
        w.opt(r.first_shell);
    });
    w.arena(&body.shells, |w, s| {
        w.id(s.region);
        w.id(s.next);
        w.id(s.prev);
        w.opt(s.first_face);
    });
    w.arena(&body.faces, |w, f| {
        w.id(f.shell);
        w.id(f.next);
        w.id(f.prev);
        w.opt(f.first_loop);
        w.id(f.surface);
        w.sense(f.sense);
        w.name(f.name);
    });
    w.arena(&body.loops, |w, l| {
        w.id(l.face);
        w.id(l.next);
        w.id(l.prev);
        w.opt(l.first_fin);
        w.opt(l.vertex);
    });
    w.arena(&body.fins, |w, f| {
        w.id(f.loop_);
        w.id(f.edge);
        w.sense(f.sense);
        w.id(f.next);
        w.id(f.prev);
        w.id(f.around);
        match f.pcurve {
            None => w.u8(0),
            Some(p) => {
                w.u8(1);
                w.id(p.curve);
                w.f64(p.map[0]);
                w.f64(p.map[1]);
            }
        }
    });
    w.arena(&body.edges, |w, e| {
        w.id(e.fin);
        w.id(e.start);
        w.id(e.end);
        w.id(e.curve);
        w.f64(e.interval[0]);
        w.f64(e.interval[1]);
        w.f64(e.tolerance.get());
        w.name(e.name);
    });
    w.arena(&body.vertices, |w, v| {
        w.point(v.point);
        w.f64(v.tolerance.get());
        w.opt(v.edge);
        w.opt(v.lone);
        w.name(v.name);
    });
    w.arena(&body.curves, Writer::curve);
    w.arena(&body.surfaces, Writer::surface);
    w.arena(&body.curves2, Writer::curve2);
    w.len(body.names.len());
    for (&n, &e) in &body.names {
        w.name(n);
        let (tag, index) = match e {
            crate::topo::EntityId::Face(f) => (0, f.index()),
            crate::topo::EntityId::Edge(e) => (1, e.index()),
            crate::topo::EntityId::Vertex(v) => (2, v.index()),
            _ => unreachable!("only faces, edges and vertices are named"),
        };
        w.u8(tag);
        w.len(index);
    }
    match &body.recipes {
        None => w.u8(0),
        Some(recipes) => {
            w.u8(1);
            w.len(recipes.len());
            for (&n, r) in recipes {
                w.name(n);
                w.str(r);
            }
        }
    }
    w.out
}

/// The bytes left, and the shared geometry read so far, in order.
struct Reader<'a> {
    bytes: &'a [u8],
    curves: Vec<Arc<NurbsCurve>>,
    curves2: Vec<Arc<NurbsCurve2>>,
    surfaces: Vec<Arc<NurbsSurface>>,
    extrusions: Vec<Arc<Extrusion>>,
    revolutions: Vec<Arc<Revolution>>,
    pipes: Vec<Arc<Pipe>>,
}

type R<T> = Result<T, Error>;

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            curves: Vec::new(),
            curves2: Vec::new(),
            surfaces: Vec::new(),
            extrusions: Vec::new(),
            revolutions: Vec::new(),
            pipes: Vec::new(),
        }
    }

    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        if self.bytes.len() < n {
            return Err(invalid("cut short"));
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(head)
    }

    fn magic(&mut self, magic: &[u8]) -> R<()> {
        if self.bytes.starts_with(magic) {
            self.bytes = &self.bytes[magic.len()..];
            Ok(())
        } else {
            Err(invalid("not this format or version"))
        }
    }

    fn end(&self) -> R<()> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(invalid("bytes after the end"))
        }
    }

    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> R<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A count of items each at least `min` bytes long, refused when the
    /// bytes left can't hold them (so nothing huge is allocated).
    fn count(&mut self, min: usize) -> R<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min) > self.bytes.len() {
            return Err(invalid("a count longer than the bytes left"));
        }
        Ok(n)
    }

    fn f64(&mut self) -> R<f64> {
        let b = self.take(8)?;
        Ok(f64::from_bits(u64::from_le_bytes(
            b.try_into().expect("8 bytes"),
        )))
    }

    fn finite(&mut self) -> R<f64> {
        let x = self.f64()?;
        if x.is_finite() {
            Ok(x)
        } else {
            Err(invalid("a coordinate that isn't finite"))
        }
    }

    fn u128(&mut self) -> R<u128> {
        let b = self.take(16)?;
        Ok(u128::from_le_bytes(b.try_into().expect("16 bytes")))
    }

    fn name(&mut self) -> R<Name> {
        Ok(Name::from_u128(self.u128()?))
    }

    fn names(&mut self) -> R<Vec<Name>> {
        let n = self.count(16)?;
        (0..n).map(|_| self.name()).collect()
    }

    fn str(&mut self) -> R<String> {
        let n = self.count(1)?;
        let b = self.take(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| invalid("text that isn't UTF-8"))
    }

    fn id<I: Id>(&mut self) -> R<I> {
        let i = self.u32()?;
        if i == u32::MAX {
            return Err(invalid("an ID that must be present is absent"));
        }
        Ok(I::from_index(i as usize))
    }

    fn opt<I: Id>(&mut self) -> R<Option<I>> {
        let i = self.u32()?;
        Ok((i != u32::MAX).then(|| I::from_index(i as usize)))
    }

    fn sense(&mut self) -> R<Sense> {
        match self.u8()? {
            0 => Ok(Sense::Forward),
            1 => Ok(Sense::Reversed),
            _ => Err(invalid("an unknown sense")),
        }
    }

    fn flag(&mut self) -> R<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid("a flag that is neither 0 nor 1")),
        }
    }

    fn point(&mut self) -> R<Point3> {
        Ok(Point3::new(self.finite()?, self.finite()?, self.finite()?))
    }

    fn vector(&mut self) -> R<Vector3> {
        Ok(Vector3::new(self.finite()?, self.finite()?, self.finite()?))
    }

    fn unit(&mut self) -> R<UnitVector3> {
        UnitVector3::exact(self.vector()?).ok_or(invalid("a direction that isn't unit"))
    }

    fn point2(&mut self) -> R<Point2> {
        Ok(Point2::new(self.finite()?, self.finite()?))
    }

    fn vector2(&mut self) -> R<Vector2> {
        Ok(Vector2::new(self.finite()?, self.finite()?))
    }

    fn tolerance(&mut self) -> R<Tolerance> {
        Tolerance::new(self.f64()?).map_err(|_| invalid("a tolerance below resolution"))
    }

    fn frame(&mut self) -> R<Frame> {
        let origin = self.point()?;
        let z = self.unit()?;
        let x = self.unit()?;
        Frame::from_axes(origin, z, x).map_err(|_| invalid("a frame that isn't orthonormal"))
    }

    fn frame2(&mut self) -> R<Frame2> {
        let origin = self.point2()?;
        let x = self.vector2()?;
        let ccw = self.flag()?;
        Frame2::exact(origin, x, ccw).ok_or(invalid("a plane frame that isn't unit"))
    }

    fn knots(&mut self) -> R<KnotVector> {
        let degree = self.u32()? as usize;
        let periodic = self.flag()?;
        let n = self.count(12)?;
        let mut knots = Vec::with_capacity(n);
        let mut mults = Vec::with_capacity(n);
        for _ in 0..n {
            knots.push(self.f64()?);
            mults.push(self.u32()? as usize);
        }
        if mults.iter().any(|&m| m > crate::geom::MAX_DEGREE + 1) {
            return Err(invalid("a knot multiplicity past the highest degree"));
        }
        let k = if periodic {
            KnotVector::periodic(degree, &knots, &mults)
        } else {
            KnotVector::clamped(degree, &knots, &mults)
        };
        k.map_err(|_| invalid("a bad knot vector"))
    }

    fn floats(&mut self, n: usize) -> R<Vec<f64>> {
        (0..n).map(|_| self.f64()).collect()
    }

    /// Reads tag 0 and a new value by `read`, recording it in `table`, or
    /// tag 1 and the index of one read before.
    fn shared<T>(
        &mut self,
        table: fn(&mut Self) -> &mut Vec<Arc<T>>,
        read: impl FnOnce(&mut Self) -> R<T>,
    ) -> R<Arc<T>> {
        match self.u8()? {
            0 => {
                let t = Arc::new(read(self)?);
                table(self).push(t.clone());
                Ok(t)
            }
            1 => {
                let i = self.u32()? as usize;
                table(self)
                    .get(i)
                    .cloned()
                    .ok_or(invalid("a reference to geometry not yet read"))
            }
            _ => Err(invalid("an unknown shared tag")),
        }
    }

    fn curve(&mut self) -> R<Curve> {
        let bad = |_| invalid("a curve its constructor refuses");
        Ok(match self.u8()? {
            0 => {
                let origin = self.point()?;
                Curve::Line(Line::from_unit(origin, self.unit()?).map_err(bad)?)
            }
            1 => {
                let f = self.frame()?;
                Curve::Circle(Circle::new(f, self.f64()?).map_err(bad)?)
            }
            2 => {
                let f = self.frame()?;
                let (a, b) = (self.f64()?, self.f64()?);
                Curve::Ellipse(Ellipse::new(f, a, b).map_err(bad)?)
            }
            3 => Curve::Nurbs(self.shared(
                |r| &mut r.curves,
                |r| {
                    let knots = r.knots()?;
                    let n = r.count(24)?;
                    let points = (0..n).map(|_| r.point()).collect::<R<Vec<_>>>()?;
                    let weights = r.floats(n)?;
                    NurbsCurve::new(knots, points, Some(weights)).map_err(bad)
                },
            )?),
            _ => return Err(invalid("an unknown curve kind")),
        })
    }

    fn curve2(&mut self) -> R<Curve2> {
        let bad = |_| invalid("a plane curve its constructor refuses");
        Ok(match self.u8()? {
            0 => {
                let origin = self.point2()?;
                let dir = self.vector2()?;
                Curve2::Line2(Line2::exact(origin, dir).ok_or(invalid("a plane line"))?)
            }
            1 => {
                let f = self.frame2()?;
                Curve2::Circle2(Circle2::new(f, self.f64()?).map_err(bad)?)
            }
            2 => {
                let f = self.frame2()?;
                let (a, b) = (self.f64()?, self.f64()?);
                Curve2::Ellipse2(Ellipse2::new(f, a, b).map_err(bad)?)
            }
            3 => Curve2::Nurbs2(self.shared(
                |r| &mut r.curves2,
                |r| {
                    let knots = r.knots()?;
                    let n = r.count(16)?;
                    let points = (0..n).map(|_| r.point2()).collect::<R<Vec<_>>>()?;
                    let weights = r.floats(n)?;
                    NurbsCurve2::new(knots, points, Some(weights)).map_err(bad)
                },
            )?),
            _ => return Err(invalid("an unknown plane curve kind")),
        })
    }

    fn surface(&mut self) -> R<Surface> {
        let bad = |_| invalid("a surface its constructor refuses");
        Ok(match self.u8()? {
            0 => Surface::Plane(Plane::new(self.frame()?)),
            1 => {
                let f = self.frame()?;
                Surface::Cylinder(Cylinder::new(f, self.f64()?).map_err(bad)?)
            }
            2 => {
                let f = self.frame()?;
                let (r, a) = (self.f64()?, self.f64()?);
                Surface::Cone(Cone::new(f, r, a).map_err(bad)?)
            }
            3 => {
                let f = self.frame()?;
                Surface::Sphere(Sphere::new(f, self.f64()?).map_err(bad)?)
            }
            4 => {
                let f = self.frame()?;
                let (a, b) = (self.f64()?, self.f64()?);
                Surface::Torus(Torus::new(f, a, b).map_err(bad)?)
            }
            5 => Surface::Nurbs(self.shared(
                |r| &mut r.surfaces,
                |r| {
                    let knots = [r.knots()?, r.knots()?];
                    let n = r.count(24)?;
                    let points = (0..n).map(|_| r.point()).collect::<R<Vec<_>>>()?;
                    let weights = r.floats(n)?;
                    NurbsSurface::new(knots, points, Some(weights)).map_err(bad)
                },
            )?),
            6 => Surface::Extrusion(self.shared(
                |r| &mut r.extrusions,
                |r| {
                    let d = r.unit()?;
                    let curve = r.curve()?;
                    Extrusion::from_unit(curve, d).map_err(bad)
                },
            )?),
            7 => Surface::Revolution(self.shared(
                |r| &mut r.revolutions,
                |r| {
                    let f = r.frame()?;
                    let curve = r.curve()?;
                    Revolution::new(f, curve).map_err(bad)
                },
            )?),
            8 => Surface::Pipe(self.shared(
                |r| &mut r.pipes,
                |r| {
                    let radius = r.f64()?;
                    let spine = r.curve()?;
                    let anchor = r.surface()?;
                    if !matches!(
                        anchor,
                        Surface::Plane(_)
                            | Surface::Cylinder(_)
                            | Surface::Cone(_)
                            | Surface::Sphere(_)
                            | Surface::Torus(_)
                    ) {
                        return Err(invalid("a pipe anchored on a surface that isn't analytic"));
                    }
                    Pipe::new(spine, radius, anchor).map_err(bad)
                },
            )?),
            _ => return Err(invalid("an unknown surface kind")),
        })
    }

    fn arena<I: Id, T>(&mut self, mut read: impl FnMut(&mut Self) -> R<T>) -> R<Arena<I, T>> {
        let n = self.count(1)?;
        let mut slots = Vec::with_capacity(n);
        for _ in 0..n {
            slots.push(match self.u8()? {
                0 => None,
                1 => Some(read(self)?),
                _ => return Err(invalid("an unknown slot tag")),
            });
        }
        Ok(Arena::from_slots(slots))
    }
}

/// The body `bytes` encode (see the module docs). Fails with
/// [`Error::InvalidEncoding`] for bytes [`encode_body`] didn't write, or
/// that describe a body [`check_topology`] refuses.
pub fn decode_body(bytes: &[u8]) -> Result<Body, Error> {
    let mut r = Reader::new(bytes);
    r.magic(BODY_MAGIC)?;
    let regions = r.arena(|r| {
        let kind = match r.u8()? {
            0 => RegionKind::Void,
            1 => RegionKind::Solid,
            _ => return Err(invalid("an unknown region kind")),
        };
        Ok(Region {
            kind,
            first_shell: r.opt()?,
        })
    })?;
    let shells = r.arena(|r| {
        Ok(Shell {
            region: r.id()?,
            next: r.id()?,
            prev: r.id()?,
            first_face: r.opt()?,
        })
    })?;
    let faces = r.arena(|r| {
        Ok(Face {
            shell: r.id()?,
            next: r.id()?,
            prev: r.id()?,
            first_loop: r.opt()?,
            surface: r.id()?,
            sense: r.sense()?,
            name: r.name()?,
        })
    })?;
    let loops = r.arena(|r| {
        Ok(Loop {
            face: r.id()?,
            next: r.id()?,
            prev: r.id()?,
            first_fin: r.opt()?,
            vertex: r.opt()?,
        })
    })?;
    let fins = r.arena(|r| {
        Ok(Fin {
            loop_: r.id()?,
            edge: r.id()?,
            sense: r.sense()?,
            next: r.id()?,
            prev: r.id()?,
            around: r.id()?,
            pcurve: match r.u8()? {
                0 => None,
                1 => Some(Pcurve {
                    curve: r.id()?,
                    map: [r.finite()?, r.finite()?],
                }),
                _ => return Err(invalid("an unknown pcurve tag")),
            },
        })
    })?;
    let edges = r.arena(|r| {
        Ok(Edge {
            fin: r.id()?,
            start: r.id()?,
            end: r.id()?,
            curve: r.id()?,
            interval: [r.finite()?, r.finite()?],
            tolerance: r.tolerance()?,
            name: r.name()?,
        })
    })?;
    let vertices = r.arena(|r| {
        Ok(Vertex {
            point: r.point()?,
            tolerance: r.tolerance()?,
            edge: r.opt()?,
            lone: r.opt()?,
            name: r.name()?,
        })
    })?;
    let curves = r.arena(Reader::curve)?;
    let surfaces = r.arena(Reader::surface)?;
    let curves2 = r.arena(Reader::curve2)?;
    let mut names = BTreeMap::new();
    for _ in 0..r.count(21)? {
        let n = r.name()?;
        let tag = r.u8()?;
        let i = r.u32()? as usize;
        let e = match tag {
            0 => crate::topo::EntityId::Face(Id::from_index(i)),
            1 => crate::topo::EntityId::Edge(Id::from_index(i)),
            2 => crate::topo::EntityId::Vertex(Id::from_index(i)),
            _ => return Err(invalid("an unknown named entity kind")),
        };
        if names.insert(n, e).is_some() {
            return Err(invalid("a name listed twice"));
        }
    }
    let recipes = if r.flag()? {
        let mut recipes = BTreeMap::new();
        for _ in 0..r.count(20)? {
            let n = r.name()?;
            recipes.insert(n, r.str()?);
        }
        Some(recipes)
    } else {
        None
    };
    r.end()?;
    let body = Body {
        regions,
        shells,
        faces,
        loops,
        fins,
        edges,
        vertices,
        curves,
        surfaces,
        curves2,
        names,
        recipes,
    };
    if !check_topology(&body).is_empty() {
        return Err(invalid("a body the structural checks refuse"));
    }
    Ok(body)
}

/// `journal` as bytes: its events in order, each a tag and its names.
pub fn encode_journal(journal: &Journal) -> Vec<u8> {
    let mut w = Writer::default();
    w.out.extend_from_slice(JOURNAL_MAGIC);
    w.len(journal.events().len());
    for e in journal.events() {
        match e {
            Event::Created(n) => {
                w.u8(0);
                w.name(*n);
            }
            Event::Deleted(n) => {
                w.u8(1);
                w.name(*n);
            }
            Event::Modified(n) => {
                w.u8(2);
                w.name(*n);
            }
            Event::Split { parent, children } => {
                w.u8(3);
                w.name(*parent);
                w.names(children);
            }
            Event::Merged { parents, child } => {
                w.u8(4);
                w.names(parents);
                w.name(*child);
            }
            Event::Placed { tool, from, to } => {
                w.u8(5);
                w.u128(tool.0);
                w.name(*from);
                w.name(*to);
            }
            Event::Replaced { old, new } => {
                w.u8(6);
                w.name(*old);
                w.names(new);
            }
        }
    }
    w.out
}

/// The journal `bytes` encode. Fails with [`Error::InvalidEncoding`] for
/// bytes [`encode_journal`] didn't write.
pub fn decode_journal(bytes: &[u8]) -> Result<Journal, Error> {
    let mut r = Reader::new(bytes);
    r.magic(JOURNAL_MAGIC)?;
    let mut journal = Journal::new();
    for _ in 0..r.count(17)? {
        journal.push(match r.u8()? {
            0 => Event::Created(r.name()?),
            1 => Event::Deleted(r.name()?),
            2 => Event::Modified(r.name()?),
            3 => Event::Split {
                parent: r.name()?,
                children: r.names()?,
            },
            4 => Event::Merged {
                parents: r.names()?,
                child: r.name()?,
            },
            5 => Event::Placed {
                tool: ToolKey(r.u128()?),
                from: r.name()?,
                to: r.name()?,
            },
            6 => Event::Replaced {
                old: r.name()?,
                new: r.names()?,
            },
            _ => return Err(invalid("an unknown event")),
        });
    }
    r.end()?;
    Ok(journal)
}

#[cfg(test)]
mod tests;
