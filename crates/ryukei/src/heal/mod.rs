//! Healing: what a file says about a solid, made into a body that keeps
//! ryukei's rules.
//!
//! An importer reads a solid into a [`Shape`]: plain data, not a
//! [`Body`], since what a file says may break the rules Euler operators
//! enforce. Faces may be cut open along seam edges, poles bounded by
//! vertex loops, edges repeated per face; geometry is the file's,
//! converted to ryukei's types and to meters. [`heal`] makes a body of
//! it, in steps that are each a pure function of the one before, so
//! nothing is built until every decision has been made and a refusal
//! leaves nothing half made:
//!
//! 1. **Names.** Every vertex, edge and face of the shape is named from
//!    the caller's key and a hash of its geometry as read (see
//!    [`ImportRole`]), before anything changes.
//! 2. **Sewing.** Edges the file repeats on two faces are joined, and the
//!    vertices at their ends, an edge split where it meets a neighbour's
//!    vertex first (a T-junction; see `sew`).
//! 3. **Uses.** Every edge must then be used twice: a shell that leaves
//!    an edge used once is open, one that uses it more often is
//!    non-manifold.
//! 4. **Seams and poles.** Seam edges come out of periodic faces, vertex
//!    loops at poles go, edges a seam split are joined again, and edges
//!    running through a pole are split there (see `unseam`).
//! 5. **Orientation.** Loops run the way their faces' regions need,
//!    where the loops alone fix it, and faces are turned over until every
//!    edge is used once each way (see `orient`).
//! 6. **Tightening.** An edge between two analytic or pipe faces that
//!    strays from them is put on their intersection where a branch runs
//!    along it (see `tighten`).
//! 7. **Gaps.** Every edge's and vertex's tolerance grows to cover how
//!    far it lies from its curves and surfaces, up to the cap (see
//!    `gaps`).
//! 8. **Collapsing.** Edges shorter than their tolerance go, their
//!    vertices joined, and so do slivers narrower than their edges'
//!    tolerances, their two edges joined (see `collapse`); the gaps are
//!    measured again.
//! 9. **Pcurves.** Every use of an edge gets its pcurve on the face:
//!    [`crate::pcurve::exact`] first, else the file's, if it fits the
//!    edge.
//! 10. **Build.** The shape is laid out and built through Euler operators
//!     ([`crate::layout`]), every connected piece a solid of its own, and
//!     renamed with the names of step 1. Faces on one surface that
//!     overlap in an area are refused ([`crate::check::check_overlaps`]).
//!     Each shell's volume then says
//!     which way it faces and which encloses which; shells facing the
//!     wrong way are turned over, cavities go in the solids around them,
//!     and the body is built again if anything changed (see `shells`).
//!     Tolerances cover the gaps between curves' ends and their vertices,
//!     and the bounds of approximated geometry.
//!     [`crate::check::check`] must then hold. Where faces of one of the
//!     file's solids overlap, or its shells cross, the body as built is
//!     repaired by [`crate::repair::self_union`] instead, under
//!     [`HealOptions::repair`] (design 299), and refused as before where
//!     that can't be done.
//!
//! **Sheets** (design 295). With [`Open::Sheet`] a shape whose shells
//! the file gives as surfaces ([`Shape::sheets`]) is healed into a sheet:
//! its edges may be used once (laminar), faces are oriented from the
//! least-named face of each piece as above (a Möbius band is
//! [`crate::HealError::Misoriented`]), no shell is judged by its volume,
//! and the build makes every piece a sheet in the void region, open or
//! closed ([`crate::layout`]). A solid's shell that doesn't close is
//! still [`crate::HealError::OpenShell`], and a shape holding both kinds
//! is [`crate::HealError::MixedShape`] (see `pieces`). With
//! [`Open::Refuse`], the default, surface shells are healed as solids'
//! are, as they always were.
//!
//! **References** (design 364). [`reference`] makes a body for display
//! and reading geometry, never for modelling: the shape's topology as the
//! file gives it, through healing's structural steps alone (names, each
//! edge's uses, seams and poles, orientation), sampled tolerances and
//! unfitted pcurves, built as a sheet. It passes
//! [`crate::check::check_topology`] and nothing more. A piece the layout
//! refuses is laid out face by face, and a face that fails alone is left
//! out, so a reference is refused only when no face can be laid out.
//!
//! Healing knows nothing of any file format: an importer of any kind
//! builds the same [`Shape`]. [`seamed`] goes the other way, a body as a
//! file must describe it.

mod build;
mod collapse;
mod gaps;
mod names;
mod orient;
mod pcurves;
mod pieces;
mod reference;
pub(crate) mod seamed;
mod sew;
mod sewing;
pub(crate) mod shells;
mod tighten;
mod unseam;

pub use names::{Names as ImportNames, names as import_names};
pub use reference::{ReferenceOptions, ReferenceReport, reference, reference_with};
pub use seamed::{Origins, seamed, seamed_with_origins};
pub use sewing::{SewOptions, sew};

use crate::geom::{Curve, Curve2, Point3, Surface};
use crate::name::{Event, Journal, Name, OpKey};
use crate::topo::{Body, Sense};
use crate::work::{Meter, Work};
use crate::{Error, HealError};
use std::collections::{BTreeMap, BTreeSet};

pub use crate::name::ImportRole;

/// A solid as a file describes it. Entities refer to each other by index
/// into these arrays; geometry is shared the same way.
#[derive(Clone, Default, PartialEq)]
pub struct Shape {
    pub surfaces: Vec<Surface>,
    pub curves: Vec<Curve>,
    /// Curves in surfaces' parameter planes: the file's pcurves.
    pub curves2: Vec<Curve2>,
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub faces: Vec<Face>,
    pub shells: Vec<Shell>,
    pub solids: Vec<Solid>,
    /// Shells the file gives as surfaces, in no solid: a
    /// `SHELL_BASED_SURFACE_MODEL`'s, say. Healed as sheets with
    /// [`Open::Sheet`], as solids' shells otherwise.
    pub sheets: Vec<usize>,
}

/// Written as derived, but `sheets` only where there are any: a solid's
/// shape reads as it did before sheets, so text that records one (the
/// golden hashes) doesn't move.
impl core::fmt::Debug for Shape {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut d = f.debug_struct("Shape");
        d.field("surfaces", &self.surfaces)
            .field("curves", &self.curves)
            .field("curves2", &self.curves2)
            .field("vertices", &self.vertices)
            .field("edges", &self.edges)
            .field("faces", &self.faces)
            .field("shells", &self.shells)
            .field("solids", &self.solids);
        if !self.sheets.is_empty() {
            d.field("sheets", &self.sheets);
        }
        d.finish()
    }
}

/// A vertex at `point`, with the tolerance its source gives it (0 where
/// it gives none). `seam` marks a vertex a writer added only to end a
/// seam where it crosses a loop inside an edge (see [`seamed`]): once the
/// seam is gone, healing removes it and joins the edge's two pieces. A
/// file's own vertices are never removed that way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub point: Point3,
    pub tolerance: f64,
    pub seam: bool,
}

/// An edge along curve `curve` over `interval` (increasing), from vertex
/// `start` to vertex `end` (the same vertex for a closed edge). Its
/// tolerance covers how far the curve may be from the true one: the
/// bound of an approximation, or 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub curve: usize,
    pub interval: [f64; 2],
    pub start: usize,
    pub end: usize,
    pub tolerance: f64,
}

/// A face on surface `surface`, its normal the surface's natural one
/// when `sense` is `Forward`; its loops run counterclockwise about that
/// normal. Its tolerance covers how far the surface may be from the true
/// one: the bound of an approximation, or 0.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub surface: usize,
    pub sense: Sense,
    pub loops: Vec<Loop>,
    pub tolerance: f64,
}

/// One loop of a face.
#[derive(Clone, Debug, PartialEq)]
pub enum Loop {
    /// Edges in loop order.
    Edges(Vec<Use>),
    /// A single vertex: a cone's apex, or a pole a file bounds.
    Vertex(usize),
}

/// One use of an edge by a loop: the sense the loop runs it, and the
/// file's pcurve for it on the face, if it has one: a `curves2` entry
/// whose parameter is an affine function of the edge's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Use {
    pub edge: usize,
    pub sense: Sense,
    pub pcurve: Option<usize>,
}

/// Faces joined by edges. `closed` is what the file says; healing finds
/// out for itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub faces: Vec<usize>,
    pub closed: bool,
}

/// A solid: its outer shell and its cavities' shells, which face into
/// the cavity.
#[derive(Clone, Debug, PartialEq)]
pub struct Solid {
    pub outer: usize,
    pub voids: Vec<usize>,
}

/// What healing makes of a shape's open shells (design 295).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Open {
    /// Every shell must close: an open one is
    /// [`crate::HealError::OpenShell`]. The default, so callers that read
    /// only solids keep refusing surfaces.
    #[default]
    Refuse,
    /// A shape whose shells are surfaces ([`Shape::sheets`]) heals into a
    /// sheet; one of solids' shells as before.
    Sheet,
}

/// How far healing may go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HealOptions {
    /// The largest tolerance healing may give an entity, in meters.
    pub tolerance_cap: f64,
    /// Whether edges that stray from their faces are put on the faces'
    /// intersection (step 6). Off, every gap stays a tolerance: the most
    /// tolerant body a file gives, for tests and comparisons.
    pub tighten: bool,
    /// Whether a solid of the file whose shells cross, or two of whose
    /// faces overlap, is repaired by [`crate::repair::self_union`]
    /// (design 299) rather than refused. Off, it is refused as
    /// [`HealError::ShellsCross`] or [`HealError::FacesOverlap`].
    pub repair: bool,
    /// What open shells become.
    pub open: Open,
}

impl Default for HealOptions {
    fn default() -> Self {
        Self {
            tolerance_cap: crate::tol::TOLERANCE_CAP,
            tighten: true,
            repair: true,
            open: Open::Refuse,
        }
    }
}

/// What healing did, by the shape's import names. Lists are sorted by
/// name.
#[derive(Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct HealReport {
    /// Edges made by joining two of the file's that were one: sewn, or
    /// either side of a collapsed sliver.
    pub sewn: Vec<Name>,
    /// Vertices made by joining the file's: at the ends of sewn edges, or
    /// across collapsed ones.
    pub merged: Vec<Name>,
    /// Edges shorter than their tolerance and slivers narrower than it,
    /// removed, and free edges whose ends sewing joined.
    pub collapsed: Vec<Name>,
    /// The file's edges split: where a neighbour's vertex lay on them (a
    /// T-junction), so their pieces could be sewn, or where they ran
    /// through a pole or apex of a face's surface.
    pub split: Vec<Name>,
    /// Seam edges removed.
    pub seams: Vec<Name>,
    /// Faces some of whose loops ran against the region they bound, and
    /// were reversed.
    pub reversed: Vec<Name>,
    /// Faces turned over: to agree with their neighbours, or with their
    /// shell when it faced the wrong way.
    pub turned: Vec<Name>,
    /// Edges put on the intersection of their faces' surfaces.
    pub tightened: Vec<Name>,
    /// Pole vertices dropped with their loops or seams.
    pub poles: Vec<Name>,
    /// Vertices removed between two edges on one curve, which were joined.
    pub joined: Vec<Name>,
    /// Entities whose tolerance is above [`crate::tol::LINEAR_RESOLUTION`],
    /// with it.
    pub tolerances: Vec<(Name, f64)>,
    /// Faces whose surface is an approximation, with its bound.
    pub approximated: Vec<(Name, f64)>,
    /// Faces of a solid whose shells crossed or whose faces overlapped
    /// that its repair cut, split or removed (see [`HealOptions::repair`]).
    pub repaired: Vec<Name>,
    /// A sheet's laminar edges: its boundaries.
    pub laminar: Vec<Name>,
}

/// Written as derived, but `repaired` and `laminar` only where there are
/// any: a report with neither reads as it did before self-union and
/// sheets, so text that records one (the golden hashes) doesn't move.
impl core::fmt::Debug for HealReport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut d = f.debug_struct("HealReport");
        d.field("sewn", &self.sewn)
            .field("merged", &self.merged)
            .field("collapsed", &self.collapsed)
            .field("split", &self.split)
            .field("seams", &self.seams)
            .field("reversed", &self.reversed)
            .field("turned", &self.turned)
            .field("tightened", &self.tightened)
            .field("poles", &self.poles)
            .field("joined", &self.joined)
            .field("tolerances", &self.tolerances)
            .field("approximated", &self.approximated);
        if !self.repaired.is_empty() {
            d.field("repaired", &self.repaired);
        }
        if !self.laminar.is_empty() {
            d.field("laminar", &self.laminar);
        }
        d.finish()
    }
}

/// Heals `shape` into a body named from `key`: the body, the journal of
/// what happened to the shape's import names, and the report. Fails with
/// [`Error::InvalidHeal`] for a shape that can't be made into a valid
/// body, naming its entities by import name; geometry outside the size
/// box fails with [`Error::OutsideSizeBox`].
pub fn heal(
    key: OpKey,
    shape: &Shape,
    options: &HealOptions,
) -> Result<(Body, Journal, HealReport), Error> {
    heal_with(key, shape, options, &Work::unlimited())
}

/// [`heal`] under `work`: refused with `Cancelled` or `BudgetExceeded`
/// when it stops, building nothing.
pub fn heal_with(
    key: OpKey,
    shape: &Shape,
    options: &HealOptions,
    work: &Work,
) -> Result<(Body, Journal, HealReport), Error> {
    crate::work::run(work, |m| heal_in(key, shape, options, m))
}

fn heal_in(
    key: OpKey,
    shape: &Shape,
    options: &HealOptions,
    m: &Meter,
) -> Result<(Body, Journal, HealReport), Error> {
    validate(shape)?;
    let mut names = names::names(key, shape);
    let mut shape = shape.clone();
    let sewn = sew::sew(key, &mut shape, &mut names, options.tolerance_cap, m)?;
    let sheet = pieces::kind(&shape, &names, options.open)? == pieces::Kind::Sheet;
    let (body, journal, report) = after_sewing(key, shape, names, &sewn, options, sheet, m)?;
    let mut all = sewn.journal;
    all.append(journal);
    Ok((body, all, report))
}

/// Healing from its third step on (see the module's notes), of a shape
/// sewn already (`sewn` says what sewing did, for the report), as a solid
/// or as a sheet: the body, the journal of every step after sewing, and
/// the report.
fn after_sewing(
    key: OpKey,
    mut shape: Shape,
    mut names: names::Names,
    sewn: &sew::Sewn,
    options: &HealOptions,
    sheet: bool,
    m: &Meter,
) -> Result<(Body, Journal, HealReport), Error> {
    if sheet {
        build::uses_sheet(&shape, &names)?;
    } else {
        build::uses(&shape, &names)?;
    }
    let unseamed = unseam::unseam(key, &mut shape, &mut names, options.tolerance_cap)?;
    let at_poles = unseam::split_at_poles(key, &mut shape, &mut names);
    let mut reversed = orient::loops(&mut shape, &names)?;
    let (turned, against) = orient::faces(&mut shape, &names)?;
    reversed.extend(against);
    reversed.sort();
    let (tightened, measured) = if options.tighten {
        tighten::tighten(&mut shape, &names, options.tolerance_cap, m)
    } else {
        (Vec::new(), Vec::new())
    };
    gaps::tolerate(&mut shape, &names, options.tolerance_cap, &measured)?;
    let collapsed = collapse::collapse(key, &mut shape, &mut names, options.tolerance_cap)?;
    if !collapsed.removed.is_empty() {
        gaps::tolerate(&mut shape, &names, options.tolerance_cap, &[])?;
    }
    let mut pcurves = pcurves::pcurves(&shape, &names, m)?;
    // Shells facing the wrong way, turned over (solids only: a sheet's
    // orientation is its faces' agreement alone).
    let mut shell_turn: Vec<usize> = Vec::new();
    let body = if sheet {
        let body = build::assemble_sheet(key, &shape, &names, &pcurves)?;
        overlaps(&body, m)?;
        body
    } else {
        let first = build::assemble(key, &shape, &names, &pcurves, None)?;
        let index: BTreeMap<Name, usize> = names
            .faces
            .iter()
            .enumerate()
            .map(|(k, &n)| (n, k))
            .collect();
        let judged = overlaps(&first, m).and_then(|()| shells::judge(&first, &index, m));
        let judged = match judged {
            Ok(j) => j,
            Err(refused) => {
                // Shells crossing, or faces overlapping, within one of the
                // file's solids: the body as the file describes it, repaired.
                if !repairable(&refused, &shape, &index, options) {
                    return Err(refused);
                }
                let (body, repair_journal) =
                    match crate::repair::self_union_in(repair_key(key), &first, m) {
                        Ok(done) if !done.1.is_empty() => done,
                        Err(e @ (Error::Cancelled | Error::BudgetExceeded { .. })) => {
                            return Err(e);
                        }
                        // The pieces touch along a line or at a point: say
                        // so, naming where, rather than only that the
                        // faces overlap (#434).
                        Err(Error::InvalidRepair(crate::RepairError::NonManifold {
                            at,
                            faces,
                        })) => {
                            return Err(match refused {
                                Error::InvalidHeal(
                                    HealError::FacesOverlap { a, b }
                                    | HealError::ShellsCross { a, b },
                                ) => Error::InvalidHeal(HealError::PiecesTouch { a, b, at, faces }),
                                other => other,
                            });
                        }
                        _ => return Err(refused),
                    };
                return repaired(
                    (body, repair_journal),
                    &first,
                    &shape,
                    &names,
                    options,
                    Steps {
                        sewn,
                        unseamed,
                        at_poles,
                        collapsed,
                        reversed,
                        turned,
                        tightened,
                    },
                );
            }
        };
        if judged.stands() {
            first
        } else {
            for &k in &judged.turn {
                orient::turn(&mut shape.faces[k]);
                for list in &mut pcurves[k] {
                    list.reverse();
                }
            }
            shell_turn = judged.turn;
            build::assemble(key, &shape, &names, &pcurves, Some(&judged.solids))?
        }
    };
    let (body, created, report) = build::finish(body, &shape, &names, options)?;
    // A face turned twice, with its neighbours and with its shell, is as
    // the file had it.
    let mut turned: BTreeSet<Name> = turned.into_iter().collect();
    for &k in &shell_turn {
        if !turned.remove(&names.faces[k]) {
            turned.insert(names.faces[k]);
        }
    }
    let turned: Vec<Name> = turned.into_iter().collect();
    Ok(report_steps(
        body,
        created,
        report,
        Steps {
            sewn,
            unseamed,
            at_poles,
            collapsed,
            reversed,
            turned,
            tightened,
        },
    ))
}

/// What healing's steps before the build did, for the journal and the
/// report.
struct Steps<'a> {
    sewn: &'a sew::Sewn,
    unseamed: unseam::Unseamed,
    at_poles: unseam::AtPoles,
    collapsed: collapse::Collapsed,
    reversed: Vec<Name>,
    turned: Vec<Name>,
    tightened: Vec<Name>,
}

/// The journal (sewing's apart) and the report of a healed body, from its
/// steps.
fn report_steps(
    body: Body,
    created: Journal,
    mut report: HealReport,
    st: Steps<'_>,
) -> (Body, Journal, HealReport) {
    let Steps {
        sewn,
        unseamed,
        at_poles,
        collapsed,
        reversed,
        turned,
        tightened,
    } = st;
    let mut journal = unseamed.journal;
    journal.append(at_poles.journal);
    journal.append(collapsed.journal);
    journal.append(created);
    for &f in &reversed {
        journal.push(Event::Modified(f));
    }
    for &f in &turned {
        journal.push(Event::Modified(f));
    }
    for &e in &tightened {
        journal.push(Event::Modified(e));
    }
    report.reversed = reversed;
    report.turned = turned;
    report.tightened = tightened;
    report.sewn = [sewn.edges.clone(), collapsed.edges].concat();
    report.sewn.sort();
    report.merged = [sewn.vertices.clone(), collapsed.vertices].concat();
    report.merged.sort();
    report.collapsed = [sewn.removed.clone(), collapsed.removed].concat();
    report.collapsed.sort();
    report.split = [sewn.split.clone(), at_poles.split].concat();
    report.split.sort();
    report.seams = unseamed.seams;
    report.poles = unseamed.poles;
    report.joined = unseamed.joined;
    report.laminar = body
        .edges()
        .filter(|&e| body.is_laminar(e))
        .map(|e| body.edges[e].name)
        .collect();
    report.laminar.sort();
    (body, journal, report)
}

/// The key a repair inside healing is named under.
fn repair_key(key: OpKey) -> OpKey {
    OpKey(
        crate::name::Name::derive(key, crate::name::Role::Import(ImportRole::Repair), &[])
            .to_u128(),
    )
}

/// Whether healing may repair what it refused: shells that cross, or
/// faces that overlap, within one of the file's solids, under
/// [`HealOptions::repair`]. Solids of one file that cross or touch each
/// other are the importer's to unite (ryustep heals each alone and unites
/// them), as they were before (issues #315, #208, #319).
fn repairable(
    refused: &Error,
    shape: &Shape,
    index: &BTreeMap<Name, usize>,
    options: &HealOptions,
) -> bool {
    let (a, b) = match refused {
        Error::InvalidHeal(HealError::ShellsCross { a, b } | HealError::FacesOverlap { a, b }) => {
            (*a, *b)
        }
        _ => return false,
    };
    // Each face's solid in the file.
    let mut solid_of = vec![usize::MAX; shape.faces.len()];
    for (k, so) in shape.solids.iter().enumerate() {
        for &sh in core::iter::once(&so.outer).chain(&so.voids) {
            for &f in &shape.shells[sh].faces {
                solid_of[f] = k;
            }
        }
    }
    match (index.get(&a), index.get(&b)) {
        (Some(&fa), Some(&fb)) => options.repair && solid_of[fa] == solid_of[fb],
        _ => false,
    }
}

/// A repaired body finished as healing finishes one: its journal and
/// report, `repaired` naming the faces the repair split or removed.
fn repaired(
    (body, repair_journal): (Body, Journal),
    first: &Body,
    shape: &Shape,
    names: &names::Names,
    options: &HealOptions,
    mut steps: Steps<'_>,
) -> Result<(Body, Journal, HealReport), Error> {
    let (body, created, report) = build::finish(body, shape, names, options)?;
    // In name order, as the unrepaired path reports them.
    let turned: BTreeSet<Name> = core::mem::take(&mut steps.turned).into_iter().collect();
    steps.turned = turned.into_iter().collect();
    let (body, mut journal, mut report) = report_steps(body, created, report, steps);
    let faces: BTreeSet<Name> = first.faces().map(|f| first.faces[f].name).collect();
    let mut touched: BTreeSet<Name> = BTreeSet::new();
    for e in repair_journal.events() {
        match e {
            Event::Split { parent, .. } | Event::Deleted(parent) | Event::Modified(parent)
                if faces.contains(parent) =>
            {
                touched.insert(*parent);
            }
            _ => {}
        }
    }
    report.repaired = touched.into_iter().collect();
    journal.append(repair_journal);
    Ok((body, journal, report))
}

/// Refuses a body two of whose faces on one surface overlap in an area,
/// naming the least pair.
fn overlaps(body: &Body, m: &Meter) -> Result<(), Error> {
    let pair = crate::check::check_overlaps_in(body, m)
        .into_iter()
        .filter_map(|f| match f {
            crate::check::Fault::FacesOverlap { a, b } => {
                let (x, y) = (body.faces[a].name, body.faces[b].name);
                Some((x.min(y), x.max(y)))
            }
            _ => None,
        })
        .min();
    match pair {
        Some((a, b)) => Err(crate::HealError::FacesOverlap { a, b }.into()),
        None => Ok(()),
    }
}

fn malformed(what: &'static str) -> Error {
    crate::HealError::Malformed { what }.into()
}

/// Indices in range, intervals increasing, every face in exactly one
/// shell and every shell in exactly one solid or among the sheets.
fn validate(s: &Shape) -> Result<(), Error> {
    let finite = |x: f64| x.is_finite() && x >= 0.0;
    for v in &s.vertices {
        if !finite(v.tolerance) {
            return Err(malformed("a tolerance that is negative or not finite"));
        }
    }
    for e in &s.edges {
        if e.curve >= s.curves.len() || e.start >= s.vertices.len() || e.end >= s.vertices.len() {
            return Err(malformed(
                "an edge refers to a curve or vertex the shape lacks",
            ));
        }
        let [t0, t1] = e.interval;
        if !(t0.is_finite() && t1.is_finite() && t0 < t1) {
            return Err(malformed("an edge's interval is not finite and increasing"));
        }
        if !finite(e.tolerance) {
            return Err(malformed("a tolerance that is negative or not finite"));
        }
    }
    for f in &s.faces {
        if f.surface >= s.surfaces.len() {
            return Err(malformed("a face refers to a surface the shape lacks"));
        }
        if !finite(f.tolerance) {
            return Err(malformed("a tolerance that is negative or not finite"));
        }
        for l in &f.loops {
            match l {
                Loop::Vertex(v) if *v >= s.vertices.len() => {
                    return Err(malformed("a loop refers to a vertex the shape lacks"));
                }
                Loop::Vertex(_) => {}
                Loop::Edges(uses) => {
                    if uses.is_empty() {
                        return Err(malformed("a loop has no edges"));
                    }
                    for u in uses {
                        if u.edge >= s.edges.len() || u.pcurve.is_some_and(|p| p >= s.curves2.len())
                        {
                            return Err(malformed("a loop refers to an edge the shape lacks"));
                        }
                    }
                }
            }
        }
    }
    let mut face_shell = vec![None; s.faces.len()];
    for (k, sh) in s.shells.iter().enumerate() {
        if sh.faces.is_empty() {
            return Err(malformed("a shell has no faces"));
        }
        for &f in &sh.faces {
            match face_shell.get_mut(f) {
                Some(slot @ None) => *slot = Some(k),
                Some(Some(_)) => return Err(malformed("a face is in two shells")),
                None => return Err(malformed("a shell refers to a face the shape lacks")),
            }
        }
    }
    if face_shell.iter().any(Option::is_none) {
        return Err(malformed("a face is in no shell"));
    }
    let mut shell_solid = vec![false; s.shells.len()];
    let held = s
        .solids
        .iter()
        .flat_map(|so| core::iter::once(&so.outer).chain(&so.voids))
        .chain(&s.sheets);
    for &sh in held {
        match shell_solid.get_mut(sh) {
            Some(slot @ false) => *slot = true,
            Some(true) => return Err(malformed("a shell is in two solids or sheets")),
            None => return Err(malformed("a solid refers to a shell the shape lacks")),
        }
    }
    if shell_solid.iter().any(|&x| !x) {
        return Err(malformed("a shell is in no solid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
