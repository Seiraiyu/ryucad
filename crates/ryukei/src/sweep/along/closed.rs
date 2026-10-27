//! Sweeps round closed paths: no caps, and each loop of the profile a
//! tube of its own, a lump's outer loop its solid's outer shell and each
//! hole a cavity in it.
//!
//! A tube is a ring of faces where one direction has one piece: round a
//! path of one segment, each profile segment's side is a band between
//! the closed laterals its vertices sweep, as a full revolve's are; a
//! closed profile curve's sides along a path of several segments are
//! bands between the closed sections at the joints. Both are built as a
//! full revolve's ring (`build::turned`); a closed curve round a path of
//! one segment is one face with no edges. Otherwise the tube is a grid,
//! built by `build::closed_ring`.

use super::super::build::{self, LoopBuilt, LoopPlan, Ring, SegmentPlan, Span, TurnPlan};
use super::super::build::{TurnSegment, TurnVertex, VertexPlan};
use super::super::{Lump, ValidLoop};
use super::{Carried, Joint, Keys, Leg, Namer, Ordered, capped, place};
use crate::Error;
use crate::name::{Name, SweepRole};
use crate::tol::Tolerance;
use crate::topo::{Body, EntityId, RegionId};

impl Carried<'_> {
    /// Builds, tolerances and names one lump round a closed path.
    pub(super) fn sweep_closed(
        &self,
        b: &mut Body,
        keys: &mut Keys,
        namer: &Namer,
        lump: &Lump,
    ) -> Result<(), Error> {
        let paths: Vec<Name> = self
            .legs
            .iter()
            .map(|l| namer.path_segment(b, l.segment.key.0))
            .collect();
        let mut region = None;
        for lp in capped::loops(lump) {
            let made = self.tube(b, keys, namer, lp, region, &paths)?;
            region.get_or_insert(made);
        }
        Ok(())
    }

    /// The span a leg starts from for a segment over `range`, as a span
    /// of the body.
    fn span_at(&self, b: &mut Body, joint: &Joint, s: &Ordered) -> Result<Span, Error> {
        let (curve, interval) = joint.span(self.o, self.frame, &s.segment.curve, s.range)?;
        Ok(Span {
            curve: b.add_curve(curve),
            interval,
        })
    }

    /// One loop's tube: into `region`, or a new solid region for a lump's
    /// outer loop. Returns its region.
    fn tube(
        &self,
        b: &mut Body,
        keys: &mut Keys,
        namer: &Namer,
        lp: &ValidLoop,
        region: Option<RegionId>,
        paths: &[Name],
    ) -> Result<RegionId, Error> {
        let n = lp.segments.len();
        let k = self.legs.len();
        let segs: Vec<Name> = lp
            .segments
            .iter()
            .map(|s| namer.segment(b, s.key.0))
            .collect();
        let pair = |i: usize| [segs[(i + n - 1) % n], segs[i]];
        if k == 1 || capped::closed(lp) {
            return self.ringed(b, keys, namer, lp, region, paths, &segs);
        }
        // A grid, built as a capped sweep closed at its last stretch: its
        // paths run counterclockwise about the motion for an outer loop's
        // outer shell and clockwise for a cavity, which is the loop's own
        // way when the motion is along the profile's normal.
        let fwd = self.sign > 0.0;
        let mut order = Vec::with_capacity(n);
        let mut segments_in = Vec::with_capacity(n);
        for j in 0..n {
            let i = if fwd { j } else { n - 1 - j };
            let s = &lp.segments[i];
            let range = if fwd { [s.t0, s.t1] } else { [s.t1, s.t0] };
            segments_in.push((i, Ordered { segment: s, range }));
            order.push(if fwd { j } else { (n - j) % n });
        }
        let at = |j: &Joint, v: usize| j.point(self.o, place::point(self.frame, lp.vertices[v]));
        let first = &self.legs[0].start;
        let mut near = Vec::with_capacity(n);
        let mut points = Vec::with_capacity(n);
        for (j, (_, s)) in segments_in.iter().enumerate() {
            near.push(self.span_at(b, first, s)?);
            points.push(at(first, order[j]));
        }
        let mut rings = Vec::with_capacity(k);
        let mut fits = Vec::with_capacity(k);
        for leg in self.legs {
            let mut segments = Vec::with_capacity(n);
            let mut sides = Vec::with_capacity(n);
            for (_, s) in &segments_in {
                let (surface, sense) = self.side(leg, s)?;
                sides.push(surface.clone());
                segments.push(SegmentPlan {
                    far: self.span_at(b, &leg.end, s)?,
                    side: Some((b.add_surface(surface), sense)),
                });
            }
            let mut vertices = Vec::with_capacity(n);
            let mut fit = Vec::with_capacity(n);
            for (j, (_, s)) in segments_in.iter().enumerate() {
                let start = leg
                    .start
                    .span(self.o, self.frame, &s.segment.curve, s.range)?;
                let (made, tol) = self.lateral(leg, start, &sides[j], lp.vertices[order[j]])?;
                fit.push(tol);
                vertices.push(VertexPlan {
                    far: at(&leg.end, order[j]),
                    lateral: Some(Span {
                        curve: b.add_curve(made.0),
                        interval: made.1,
                    }),
                });
            }
            rings.push(Ring { segments, vertices });
            fits.push(fit);
        }
        let plan = LoopPlan {
            near,
            points,
            rings,
            closed: false,
            ccw: true,
        };
        let (made, m) = build::closed_ring(b, keys, &plan, region)?;
        // Tolerances, then names.
        for (j, &v) in order.iter().enumerate() {
            let tol = Tolerance::new(lp.tolerances[v])?;
            grow(b, m.near_vertices[j], tol);
            for (r, rb) in m.rings.iter().enumerate() {
                if let Some(e) = rb.laterals[j] {
                    b.edges[e].tolerance = Tolerance::new(lp.tolerances[v] + fits[r][j])?;
                }
            }
        }
        self.name_grid(b, namer, &m, &segments_in, &order, &segs, paths, &pair);
        Ok(made)
    }

    #[allow(clippy::too_many_arguments)]
    fn name_grid(
        &self,
        b: &mut Body,
        namer: &Namer,
        m: &LoopBuilt,
        segments_in: &[(usize, Ordered)],
        order: &[usize],
        segs: &[Name],
        paths: &[Name],
        pair: &dyn Fn(usize) -> [Name; 2],
    ) {
        let k = m.rings.len();
        for (j, (i, _)) in segments_in.iter().enumerate() {
            let sides: Vec<Name> = m
                .rings
                .iter()
                .enumerate()
                .map(|(r, ring)| {
                    let face = ring.sides[j].expect("every leg sweeps a side");
                    namer.name(
                        b,
                        EntityId::Face(face),
                        SweepRole::Side,
                        &[segs[*i], paths[r]],
                    )
                })
                .collect();
            // The section at joint r starts leg r: the near edges at the
            // path's closing joint, else the far edges of leg r - 1.
            for r in 0..k {
                let edge = if r == 0 {
                    m.near[j]
                } else {
                    m.rings[r - 1].far[j]
                };
                let before = sides[(r + k - 1) % k];
                namer.name(
                    b,
                    EntityId::Edge(edge),
                    SweepRole::Section,
                    &[before, sides[r]],
                );
            }
        }
        for (j, &v) in order.iter().enumerate() {
            let p = pair(v);
            for r in 0..k {
                let vertex = if r == 0 {
                    m.near_vertices[j]
                } else {
                    m.rings[r - 1].far_vertices[j]
                };
                let parents = [p[0], p[1], paths[(r + k - 1) % k], paths[r]];
                namer.name(
                    b,
                    EntityId::Vertex(vertex),
                    SweepRole::JointVertex,
                    &parents,
                );
                if let Some(e) = m.rings[r].laterals[j] {
                    namer.name(
                        b,
                        EntityId::Edge(e),
                        SweepRole::Lateral,
                        &[p[0], p[1], paths[r]],
                    );
                }
            }
        }
    }

    /// A tube with one piece one way: a ring of bands between closed
    /// laterals (round a path of one segment) or closed sections (a
    /// closed profile curve round several), or one face (a closed curve
    /// round one).
    #[allow(clippy::too_many_arguments)]
    fn ringed(
        &self,
        b: &mut Body,
        keys: &mut Keys,
        namer: &Namer,
        lp: &ValidLoop,
        region: Option<RegionId>,
        paths: &[Name],
        segs: &[Name],
    ) -> Result<RegionId, Error> {
        let n = lp.segments.len();
        let k = self.legs.len();
        let pair = |i: usize| [segs[(i + n - 1) % n], segs[i]];
        let mut plan = TurnPlan {
            segments: Vec::new(),
            vertices: Vec::new(),
            ends: None,
            region,
        };
        if capped::closed(lp) {
            // Bands along the path between the sections at its joints, the
            // curve running counterclockwise about the motion at an outer
            // loop and clockwise at a hole: the loop's own way when the
            // motion is along the profile's normal.
            let s = &lp.segments[0];
            let range = if self.sign > 0.0 {
                [s.t0, s.t1]
            } else {
                [s.t1, s.t0]
            };
            let o = Ordered { segment: s, range };
            for leg in self.legs {
                let (surface, sense) = self.side(leg, &o)?;
                plan.segments.push(TurnSegment {
                    side: (b.add_surface(surface), sense),
                });
            }
            if k > 1 {
                for leg in self.legs {
                    let circle = self.span_at(b, &leg.start, &o)?;
                    let at = place::point(self.frame, s.curve.point(range[0]));
                    plan.vertices.push(TurnVertex {
                        point: leg.start.point(self.o, at),
                        circle,
                    });
                }
            }
            let made = build::turned(b, keys, &plan)?;
            let sides: Vec<Name> = made
                .sides
                .iter()
                .zip(paths)
                .map(|(&f, &p)| namer.name(b, EntityId::Face(f), SweepRole::Side, &[segs[0], p]))
                .collect();
            let tol = Tolerance::new(lp.tolerances[0])?;
            for r in 0..made.circles.len() {
                let before = sides[(r + k - 1) % k];
                namer.name(
                    b,
                    EntityId::Edge(made.circles[r]),
                    SweepRole::Section,
                    &[before, sides[r]],
                );
                let parents = [segs[0], segs[0], paths[(r + k - 1) % k], paths[r]];
                namer.name(
                    b,
                    EntityId::Vertex(made.vertices[r]),
                    SweepRole::JointVertex,
                    &parents,
                );
                grow(b, made.vertices[r], tol);
            }
            return Ok(made.region);
        }
        // Round a path of one segment: bands between the closed laterals,
        // in the order a full revolve takes them, which is the loop's own
        // when the motion is against the profile's normal.
        let leg: &Leg = &self.legs[0];
        let forward = self.sign < 0.0;
        let sides: Vec<usize> = (0..n)
            .map(|j| if forward { j } else { n - 1 - j })
            .collect();
        let mut surfaces = Vec::with_capacity(n);
        let mut ordered = Vec::with_capacity(n);
        for &i in &sides {
            let s = &lp.segments[i];
            let range = if forward { [s.t0, s.t1] } else { [s.t1, s.t0] };
            let o = Ordered { segment: s, range };
            let (surface, sense) = self.side(leg, &o)?;
            surfaces.push(surface.clone());
            plan.segments.push(TurnSegment {
                side: (b.add_surface(surface), sense),
            });
            ordered.push(o);
        }
        // Side j starts at the vertex its range starts at.
        let start_of = |j: usize| {
            if forward {
                sides[j]
            } else {
                (sides[j] + 1) % n
            }
        };
        let mut fits = Vec::with_capacity(n);
        for j in 0..n {
            let v = start_of(j);
            let start = leg.start.span(
                self.o,
                self.frame,
                &ordered[j].segment.curve,
                ordered[j].range,
            )?;
            let ((curve, interval), tol) =
                self.lateral(leg, start, &surfaces[j], lp.vertices[v])?;
            fits.push(tol);
            plan.vertices.push(TurnVertex {
                point: leg
                    .start
                    .point(self.o, place::point(self.frame, lp.vertices[v])),
                circle: Span {
                    curve: b.add_curve(curve),
                    interval,
                },
            });
        }
        let made = build::turned(b, keys, &plan)?;
        for (j, &i) in sides.iter().enumerate() {
            namer.name(
                b,
                EntityId::Face(made.sides[j]),
                SweepRole::Side,
                &[segs[i], paths[0]],
            );
        }
        for (j, fit) in fits.iter().enumerate() {
            let v = start_of(j);
            let p = pair(v);
            let tol = Tolerance::new(lp.tolerances[v])?;
            grow(b, made.vertices[j], tol);
            b.edges[made.circles[j]].tolerance = Tolerance::new(lp.tolerances[v] + fit)?;
            namer.name(
                b,
                EntityId::Edge(made.circles[j]),
                SweepRole::Lateral,
                &[p[0], p[1], paths[0]],
            );
            let parents = [p[0], p[1], paths[0], paths[0]];
            namer.name(
                b,
                EntityId::Vertex(made.vertices[j]),
                SweepRole::JointVertex,
                &parents,
            );
        }
        Ok(made.region)
    }
}

/// Raises a vertex's tolerance to `tol`.
fn grow(b: &mut Body, v: crate::topo::VertexId, tol: Tolerance) {
    if b.vertices[v].tolerance.get() < tol.get() {
        b.vertices[v].tolerance = tol;
    }
}
