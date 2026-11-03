//! A fill's patches built into a sheet through the layout, and named.

use super::coons::{U0, U1, V0, V1};
use crate::error::Error;
use crate::geom::{Curve, Direction, NurbsSurface, Point3, Surface};
use crate::heal::seamed::iso2;
use crate::layout::{self, Boundary, Layout, Use};
use crate::name::{Event, FillRole, Journal, Name, OpKey, Role};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EntityId, Sense};
use crate::{TopologyError, check};
use std::sync::Arc;

/// The names' recipe kind.
pub(crate) const KIND: &str = "fill";

/// Names made from the fill's key, kept in the order they were made so
/// the body's recipes can be written for them.
pub(crate) struct Namer {
    key: OpKey,
    made: Vec<(FillRole, Vec<Name>)>,
}

impl Namer {
    pub(crate) fn new(key: OpKey) -> Self {
        Self {
            key,
            made: Vec::new(),
        }
    }

    pub(crate) fn key(&self) -> OpKey {
        self.key
    }

    /// Writes the body's recipes for every name made.
    pub(crate) fn record(&self, b: &mut Body) {
        for (role, parents) in &self.made {
            b.derive_name(KIND, self.key, Role::Fill(*role), parents);
        }
    }

    pub(crate) fn name(&mut self, role: FillRole, parents: &[Name]) -> Name {
        self.made.push((role, parents.to_vec()));
        Name::derive(self.key, Role::Fill(role), parents)
    }
}

/// A patch and its sides' edges in tensor order (`[v0, u1, v1, u0]`):
/// `None` for a side collapsed to a pole.
pub(crate) struct Piece {
    pub(crate) surface: NurbsSurface,
    pub(crate) edges: [Option<usize>; 4],
    pub(crate) name: Name,
}

/// An edge: the piece whose side it is first, and its name. Its curve is
/// that side's parameter line, running with the parameter.
pub(crate) struct EdgeSpec {
    pub(crate) piece: usize,
    pub(crate) place: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) name: Name,
}

/// The sheet of `pieces`, its edges and vertices named as given, every
/// entity `Created`.
pub(crate) fn sheet(
    namer: &Namer,
    pieces: &[Piece],
    edges: &[EdgeSpec],
    vertices: &[(Point3, Name)],
) -> Result<(Body, Journal), Error> {
    let mut out = Layout::default();
    for &(point, _) in vertices {
        out.vertices.push(layout::Vertex {
            point,
            tolerance: LINEAR_RESOLUTION,
        });
    }
    for e in edges {
        let s = &pieces[e.piece].surface;
        let curve = match e.place {
            V0 => s.iso(Direction::V, 0.0),
            V1 => s.iso(Direction::V, 1.0),
            U0 => s.iso(Direction::U, 0.0),
            _ => s.iso(Direction::U, 1.0),
        }?;
        out.curves.push(Curve::Nurbs(Arc::new(curve)));
        out.edges.push(layout::Edge {
            start: e.start,
            end: e.end,
            curve: out.curves.len() - 1,
            interval: [0.0, 1.0],
            tolerance: LINEAR_RESOLUTION,
        });
    }
    for p in pieces {
        out.surfaces.push(Surface::Nurbs(Arc::new(p.surface.clone())));
        let mut arcs = Vec::with_capacity(4);
        for (place, sense) in [
            (V0, Sense::Forward),
            (U1, Sense::Forward),
            (V1, Sense::Reversed),
            (U0, Sense::Reversed),
        ] {
            let Some(edge) = p.edges[place] else {
                continue;
            };
            let (j, at) = match place {
                V0 => (1, 0.0),
                V1 => (1, 1.0),
                U0 => (0, 0.0),
                _ => (0, 1.0),
            };
            out.curves2.push(iso2(j, at));
            arcs.push(Use {
                edge,
                sense,
                pcurve: Some((out.curves2.len() - 1, [0.0, 1.0])),
            });
        }
        out.faces.push(layout::Face {
            surface: out.surfaces.len() - 1,
            sense: Sense::Forward,
            loops: vec![Boundary::Arcs(arcs)],
        });
    }
    let key = namer.key;
    let built = layout::build_sheet(&out, key, Role::Merged)
        .map_err(|_| Error::Topology(TopologyError::Inconsistent))?;
    let mut b = built.body;
    b.set_recipes(true);
    namer.record(&mut b);
    for (k, p) in pieces.iter().enumerate() {
        b.rename(EntityId::Face(built.faces[k]), p.name);
    }
    for (k, e) in edges.iter().enumerate() {
        b.rename(EntityId::Edge(built.edges[&k]), e.name);
    }
    for (&g, &v) in &built.vertices {
        b.rename(EntityId::Vertex(v), vertices[g].1);
    }
    crate::pcurve::fill(&mut b)?;
    let mut journal = Journal::new();
    for f in b.faces() {
        journal.push(Event::Created(b.faces[f].name));
    }
    for e in b.edges() {
        journal.push(Event::Created(b.edges[e].name));
    }
    for v in b.vertices() {
        journal.push(Event::Created(b.vertices[v].name));
    }
    if cfg!(debug_assertions) {
        let faults = check::check(&b);
        assert!(faults.is_empty(), "a fill is invalid: {faults:?}");
    }
    Ok((b, journal))
}
