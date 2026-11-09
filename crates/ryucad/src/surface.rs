//! Fills, extracts, sews and thickens (design 388, plan 18.B): each a body
//! of its own, made by one kernel call under the feature's key and the
//! request's cancel. A fill spans a loop of edges and curves; an extract
//! copies faces of one body into a sheet; a sew joins sheets into one body
//! and consumes them; a thicken gives a sheet (consumed) or a body's faces
//! (left alone) thickness as a solid, which `rebuild::build_body` then
//! founds or applies to its targets as an extrude's body.

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::surface::{Extract, Fill, Side};
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Origins, Results, Run};
use ryukei::fill::{FillBoundary, FillOptions, FillReport, FillSide};
use ryukei::name::{FillRole, Name, OpKey, Role};
use ryukei::topo::{Body, EntityId};
use serde_json::{Value, json};

/// Where a kernel side came from: the file's side, the piece of a curve
/// side of several pieces, and an edge side's name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Place {
    pub side: usize,
    pub piece: Option<usize>,
    pub edge: Option<Name>,
}

/// A fill's sides resolved in the fill's view: each edge side's body and
/// name, each curve side's pieces (a path or helix gives several).
enum Resolved<'a> {
    Edge(&'a Body, Name, bool),
    Curve(crate::geometry::Chain),
}

fn resolve_sides<'a>(
    part: &Part,
    id: &Id,
    x: &Fill,
    before: &'a dyn Results,
) -> Result<Vec<Resolved<'a>>> {
    let at = crate::geometry::At {
        part,
        order: part.order(),
        owner: id,
        results: before,
    };
    x.sides
        .iter()
        .enumerate()
        .map(|(i, s)| match s {
            Side::Edge { edge, tangent } => {
                let role = format!("sides.{i}.edge");
                let (built, e) =
                    crate::reference::resolve(part, part.order(), id, &role, edge, before)?;
                match e {
                    EntityId::Edge(_) => Ok(Resolved::Edge(&built.body, edge.name, *tangent)),
                    e => Err(crate::reference::wrong_kind(
                        part,
                        id,
                        &role,
                        edge,
                        crate::reference::kind(e),
                        "an edge",
                    )),
                }
            }
            Side::Curve(g) => {
                let role = format!("sides.{i}.curve");
                let c = crate::geometry::one(&at, &role, g, crate::geometry::Want::Curve)?;
                Ok(Resolved::Curve(c.curve()))
            }
        })
        .collect()
}

/// Refuses a fill (feature `id` of `part`, as a write would leave it)
/// whose edge sides aren't edges or whose curve sides aren't curves, in
/// `results`; whether they close up only rebuild can tell.
pub fn check_fill(part: &Part, id: &Id, x: &Fill, results: &dyn Results) -> Result<()> {
    resolve_sides(part, id, x, results).map(drop)
}

/// A fill: its sheet and its report.
pub fn fill(
    part: &Part,
    id: &Id,
    x: &Fill,
    before: &dyn Results,
    run: &Run,
) -> Result<(Built, Value)> {
    let resolved = resolve_sides(part, id, x, before)?;
    let mut sides: Vec<FillSide<'_>> = Vec::new();
    let mut place: Vec<Place> = Vec::new();
    for (i, r) in resolved.iter().enumerate() {
        match r {
            Resolved::Edge(body, name, tangent) => {
                sides.push(FillSide::Edge {
                    body,
                    edge: *name,
                    tangent: *tangent,
                });
                place.push(Place {
                    side: i,
                    piece: None,
                    edge: Some(*name),
                });
            }
            Resolved::Curve(chain) => {
                let many = chain.pieces.len() > 1;
                for (j, p) in chain.pieces.iter().enumerate() {
                    // Either end of a side may come first; the kernel takes
                    // its range increasing.
                    let [a, b] = p.range;
                    let range = if a <= b { [a, b] } else { [b, a] };
                    sides.push(FillSide::Curve {
                        curve: p.curve.clone(),
                        range,
                    });
                    place.push(Place {
                        side: i,
                        piece: many.then_some(j),
                        edge: None,
                    });
                }
            }
        }
    }
    let key = crate::rebuild::op_key(&part.id, id);
    let opts = FillOptions {
        tolerance: x.tolerance.meters(),
        angle: x.angle.radians(),
    };
    let work = crate::work::work(run.job);
    let (body, journal, report) =
        ryukei::fill::fill_with(key, &FillBoundary { sides }, &opts, &work).map_err(|e| {
            crate::work::stopped(&e, run.job).unwrap_or_else(|| fill_error(part, id, x, &place, e))
        })?;
    let origins = fill_origins(id, key, &place, &body);
    let report = fill_report(&place, &report);
    let built = crate::rebuild::finish(id, "fill", (body, journal), origins)?;
    Ok((built, report))
}

/// A fill's origins: each face `{feature, fill: plane | patch}` (a plane
/// for a fill of one plane face), each boundary edge `{feature, side}` (and
/// `piece` of a curve side of several), anything else the feature's. The
/// kernel names side `k`'s stand-in from `k` and its edge's name, and the
/// halves a split fill cuts it into from the stand-in and 0 or 1
/// (`ryukei::name::FillRole`).
pub(crate) fn fill_origins(id: &Id, key: OpKey, place: &[Place], body: &Body) -> Origins {
    let mut sides: std::collections::BTreeMap<Name, Value> = Default::default();
    for (k, p) in place.iter().enumerate() {
        let mut parents = vec![Name::from_u128(k as u128)];
        parents.extend(p.edge);
        let stand_in = Name::derive(key, Role::Fill(FillRole::Side), &parents);
        let mut v = json!({"feature": id, "side": p.side});
        if let Some(j) = p.piece {
            v["piece"] = json!(j);
        }
        sides.insert(stand_in, v.clone());
        for half in [0u128, 1] {
            let n = Name::derive(
                key,
                Role::Fill(FillRole::Half),
                &[stand_in, Name::from_u128(half)],
            );
            sides.insert(n, v.clone());
        }
    }
    let one_plane = body.faces().count() == 1
        && body.faces().all(|f| {
            body.face(f)
                .and_then(|f| body.surface(f.surface))
                .is_some_and(|s| matches!(s, ryukei::geom::Surface::Plane(_)))
        });
    let face = if one_plane { "plane" } else { "patch" };
    crate::rebuild::names(body)
        .into_iter()
        .map(|n| {
            let o = match body.by_name(n) {
                Some(EntityId::Face(_)) => json!({"feature": id, "fill": face}),
                _ => sides
                    .get(&n)
                    .cloned()
                    .unwrap_or_else(|| json!({"feature": id})),
            };
            (n.to_string(), o)
        })
        .collect()
}

/// What a fill measured, by the file's sides: each kernel side's
/// deviation and (tangent ones) certified angle, and the inner edges'
/// angle where the fill has several patches.
pub(crate) fn fill_report(place: &[Place], r: &FillReport) -> Value {
    let sides: Vec<Value> = place
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let mut v = json!({"side": p.side, "deviation_m": r.deviations[k]});
            if let Some(j) = p.piece {
                v["piece"] = json!(j);
            }
            if let Some(Some(a)) = r.angles.get(k) {
                v["angle_deg"] = json!(a.to_degrees());
            }
            v
        })
        .collect();
    let mut v = json!({"sides": sides});
    if let Some(a) = r.inner {
        v["inner_deg"] = json!(a.to_degrees());
    }
    v
}

/// The kernel's refusal of a fill as `fill_failed`: its `reason`, the
/// kernel's text, and the side it names by the file's index (`side`,
/// `piece` of a curve of several, and the side's `source` as text).
fn fill_error(part: &Part, id: &Id, x: &Fill, place: &[Place], e: ryukei::Error) -> Error {
    use ryukei::FillError as F;
    let (reason, side, extra): (&str, Option<usize>, Option<(&str, f64)>) = match &e {
        ryukei::Error::InvalidFill(f) => match f {
            F::TooFewSides => ("too_few_sides", None, None),
            F::BadOptions => ("bad_options", None, None),
            F::NoSuchEdge { side } => ("no_such_edge", Some(*side), None),
            F::NotLaminar { side } => ("not_laminar", Some(*side), None),
            F::NotClosed { side, gap } => ("not_closed", Some(*side), Some(("gap_m", *gap))),
            F::Degenerate { side } => ("degenerate", *side, None),
            F::AngleTooLarge { side, angle } => (
                "angle_too_large",
                *side,
                Some(("angle_deg", angle.to_degrees())),
            ),
            F::Uncertain { side } => ("uncertain", *side, None),
        },
        _ => ("kernel", None, None),
    };
    let mut err = Error::new(Code::FillFailed, format!("fill {id}: {e}"))
        .with("feature", id)
        .with("reason", reason)
        .with("kernel", e.to_string());
    if let Some(k) = side
        && let Some(p) = place.get(k)
    {
        let source = match &x.sides[p.side] {
            Side::Edge { edge, .. } => edge.text(&part.id),
            Side::Curve(g) => g.text(&part.id),
        };
        err = err.with("side", p.side).with("source", source);
        if let Some(j) = p.piece {
            err = err.with("piece", j);
        }
    }
    if let Some((k, v)) = extra {
        err = err.with(k, v);
    }
    if reason == "not_laminar" {
        err = err.with(
            "hint",
            "a tangent side needs an edge with one face (a sheet's boundary); drop tangent, or fill against a sheet's edge",
        );
    }
    err
}

/// Faces resolved in the feature's view: the one body they are faces of
/// (`targets_mixed` for faces of several), its founder, and each name.
fn resolve_faces<'a, 'f>(
    part: &Part,
    id: &Id,
    ty: &str,
    faces: &[(String, &'f crate::doc::reference::Reference)],
    before: &'a dyn Results,
) -> Result<(&'f Id, &'a Built, Vec<Name>)> {
    let first = &faces[0].1.feature;
    for (role, r) in faces {
        if r.feature != *first {
            return Err(Error::new(
                Code::TargetsMixed,
                format!(
                    "{ty} {id}: its {role} {} is a face of {}, not of {first}; a {ty} takes faces of one body",
                    r.text(&part.id),
                    r.feature
                ),
            )
            .with("feature", id)
            .with("role", role.as_str())
            .with("solids", [first.as_str(), r.feature.as_str()]));
        }
    }
    let mut built = None;
    let mut names = Vec::with_capacity(faces.len());
    for (role, r) in faces {
        let (b, e) = crate::reference::resolve(part, part.order(), id, role, r, before)?;
        if !matches!(e, EntityId::Face(_)) {
            return Err(crate::reference::wrong_kind(
                part,
                id,
                role,
                r,
                crate::reference::kind(e),
                "a face",
            ));
        }
        built = Some(b);
        names.push(r.name);
    }
    Ok((first, built.expect("at least one face"), names))
}

/// Refuses an extract whose faces aren't faces of one body, in `results`.
pub fn check_extract(part: &Part, id: &Id, x: &Extract, results: &dyn Results) -> Result<()> {
    resolve_faces(part, id, "extract", &x.references(), results).map(drop)
}

/// An extract: the named faces of their body, as it sees it, as a sheet.
pub fn extract(part: &Part, id: &Id, x: &Extract, before: &dyn Results) -> Result<Built> {
    let (from, built, names) = resolve_faces(part, id, "extract", &x.references(), before)?;
    let key = crate::rebuild::op_key(&part.id, id);
    let made = ryukei::sheet::extract(key, &built.body, &names)
        .map_err(|e| extract_error(part, id, from, &e))?;
    let origins = extract_origins(id, &made.0);
    crate::rebuild::finish(id, "extract", made, origins)
}

/// An extract's origins: every entity keeps its source's name, and says
/// so (`{feature, extract_of: <name>}`).
pub(crate) fn extract_origins(id: &Id, body: &Body) -> Origins {
    crate::rebuild::names(body)
        .into_iter()
        .map(|n| {
            (
                n.to_string(),
                json!({"feature": id, "extract_of": n.to_string()}),
            )
        })
        .collect()
}

/// The kernel's refusal of an extract: `no_faces`, or
/// `non_manifold_vertex` naming the vertex in its body.
fn extract_error(part: &Part, id: &Id, from: &Id, e: &ryukei::Error) -> Error {
    use ryukei::SheetError as S;
    let mut err = Error::new(Code::ExtractFailed, format!("extract {id}: {e}"))
        .with("feature", id)
        .with("kernel", e.to_string());
    match e {
        ryukei::Error::InvalidSheet(S::NonManifoldVertex { vertex }) => {
            err = err
                .with("reason", "non_manifold_vertex")
                .with("vertex", format!("{}/{from}/{vertex}", part.id))
                .with("hint", "the faces touch at this vertex alone: extract them as two sheets, or add the faces between them");
        }
        ryukei::Error::InvalidSheet(S::NoFaces) => err = err.with("reason", "no_faces"),
        ryukei::Error::InvalidSheet(S::NoSuchFace { name }) => {
            err = err
                .with("reason", "no_such_face")
                .with("face", name.to_string());
        }
        _ => err = err.with("reason", "kernel"),
    }
    err
}

/// A sew: its sheets, as it sees them, sewn into one body. Several pieces,
/// or a body that doesn't close where a solid was asked for, are
/// `sew_failed`.
pub fn sew(
    part: &Part,
    solids: &crate::solid::Solids,
    id: &Id,
    x: &crate::doc::surface::Sew,
    before: &dyn Results,
    run: &Run,
) -> Result<Built> {
    let need = crate::rebuild::Need {
        role: "sheets",
        want: Some(crate::kind::Want::Sheet),
        changes: true,
    };
    let inputs: Vec<(&Id, &Built)> = x
        .sheets
        .iter()
        .map(|t| {
            Ok((
                t,
                crate::rebuild::body_as(part, solids, id, t, need, before)?,
            ))
        })
        .collect::<Result<_>>()?;
    let sheets: Vec<Body> = inputs.iter().map(|(_, b)| b.body.clone()).collect();
    let key = crate::rebuild::op_key(&part.id, id);
    let opts = ryukei::heal::SewOptions {
        tolerance: x.tolerance.meters(),
        solid: x.solid,
    };
    let work = crate::work::work(run.job);
    let (mut pieces, journal) = ryukei::heal::sew_with(key, &sheets, &opts, &work)
        .map_err(|e| crate::work::stopped(&e, run.job).unwrap_or_else(|| sew_error(id, e)))?;
    if pieces.len() > 1 {
        let listed: Vec<Value> = pieces
            .iter()
            .map(|p| {
                json!({
                    "sheets": inputs
                        .iter()
                        .filter(|(_, b)| p.faces().any(|f| p.name_of(EntityId::Face(f)).is_some_and(|n| b.body.by_name(n).is_some())))
                        .map(|(t, _)| t.as_str())
                        .collect::<Vec<_>>(),
                    "laminar": laminar(part, &inputs, p),
                })
            })
            .collect();
        return Err(Error::new(
            Code::SewFailed,
            format!(
                "sew {id}: its sheets make {} pieces: their edges are farther apart than {} where the pieces meet",
                pieces.len(),
                x.tolerance
            ),
        )
        .with("feature", id)
        .with("reason", "pieces")
        .with("pieces", listed)
        .with("hint", "raise the tolerance (at most 0.1 mm), move the sheets' edges together, or sew each piece alone"));
    }
    let body = pieces.pop().expect("a sew of sheets gives a piece");
    if x.solid && body.kind() != ryukei::topo::BodyKind::Solid {
        return Err(Error::new(
            Code::SewFailed,
            format!("sew {id}: its sheets don't close up, so they make no solid"),
        )
        .with("feature", id)
        .with("reason", "open")
        .with("laminar", laminar(part, &inputs, &body))
        .with("hint", "these edges have nothing to sew to: add sheets to close them, or set solid false for a sheet"));
    }
    let origins = sew_origins(id, &inputs, &body, &journal);
    crate::rebuild::finish(id, "sew", (body, journal), origins)
}

/// A piece's laminar edges, each as a reference into the sheet whose
/// name it kept (`<part>/<sheet>/<name>`), else its name alone.
fn laminar(part: &Part, inputs: &[(&Id, &Built)], piece: &Body) -> Vec<String> {
    piece
        .edges()
        .filter(|e| piece.is_laminar(*e))
        .filter_map(|e| piece.name_of(EntityId::Edge(e)))
        .map(
            |n| match inputs.iter().find(|(_, b)| b.body.by_name(n).is_some()) {
                Some((t, _)) => format!("{}/{t}/{n}", part.id),
                None => n.to_string(),
            },
        )
        .collect()
}

/// A sew's origins: a sewn edge or a joined vertex (the kernel's `Merged`
/// child) is `{feature, sewn: [<names>]}`; a split copy takes its
/// parent's; everything else keeps its sheet's origin.
fn sew_origins(
    id: &Id,
    inputs: &[(&Id, &Built)],
    body: &Body,
    journal: &ryukei::name::Journal,
) -> Origins {
    use ryukei::name::Event;
    let mut sewn: std::collections::BTreeMap<Name, Vec<String>> = Default::default();
    let mut parent: std::collections::BTreeMap<Name, Name> = Default::default();
    for e in journal.events() {
        match e {
            Event::Merged { parents, child } => {
                sewn.insert(*child, parents.iter().map(Name::to_string).collect());
            }
            Event::Split {
                parent: p,
                children,
            } => {
                for c in children {
                    parent.insert(*c, *p);
                }
            }
            _ => {}
        }
    }
    let of = |n: Name| -> Option<Value> {
        inputs
            .iter()
            .find_map(|(_, b)| b.origins.get(&n.to_string()).cloned())
    };
    crate::rebuild::names(body)
        .into_iter()
        .map(|n| {
            let o = match sewn.get(&n) {
                Some(p) => json!({"feature": id, "sewn": p}),
                None => of(n)
                    .or_else(|| parent.get(&n).and_then(|p| of(*p)))
                    .unwrap_or_else(|| json!({"feature": id})),
            };
            (n.to_string(), o)
        })
        .collect()
}

/// The kernel's refusal of a sew: `sew_failed` with the healing refusal
/// as `reason` (its variant in snake case), its text, and the sheets'
/// entities it names where it names one.
fn sew_error(id: &Id, e: ryukei::Error) -> Error {
    let debug = format!("{e:?}");
    let reason = match &e {
        ryukei::Error::InvalidHeal(h) => variant_snake(&format!("{h:?}")),
        ryukei::Error::NotASheet { .. } => "not_a_sheet".to_owned(),
        _ => "kernel".to_owned(),
    };
    Error::new(Code::SewFailed, format!("sew {id}: {e}"))
        .with("feature", id)
        .with("reason", reason)
        .with("kernel", e.to_string())
        .with("refusal", debug)
}

/// An enum variant's name, from its debug form, in snake case:
/// `GapTooLarge { .. }` is `gap_too_large`.
fn variant_snake(debug: &str) -> String {
    let end = debug.find([' ', '(', '{']).unwrap_or(debug.len());
    let mut out = String::new();
    for (k, c) in debug[..end].chars().enumerate() {
        if c.is_ascii_uppercase() {
            if k > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The key a thicken from faces extracts them under.
fn extract_key(part: &Id, id: &Id) -> OpKey {
    let mut bytes = b"ryucad.thicken.extract\0".to_vec();
    bytes.extend_from_slice(part.as_str().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(id.as_str().as_bytes());
    OpKey::from_bytes(&bytes)
}

/// What a thicken thickens, as it sees it: the sheet it consumes (by
/// founder) or the body whose faces it takes, and the sheet itself
/// (those faces extracted).
fn thicken_input<'a>(
    part: &Part,
    solids: &crate::solid::Solids,
    id: &Id,
    t: &'a crate::doc::surface::Thicken,
    before: &'a dyn Results,
) -> Result<(&'a Id, &'a Built, std::borrow::Cow<'a, Body>)> {
    use crate::doc::surface::ThickenFrom;
    match &t.from {
        ThickenFrom::Sheet(s) => {
            let need = crate::rebuild::Need {
                role: "from",
                want: Some(crate::kind::Want::Sheet),
                changes: true,
            };
            let built = crate::rebuild::body_as(part, solids, id, s, need, before)?;
            Ok((s, built, std::borrow::Cow::Borrowed(&built.body)))
        }
        ThickenFrom::Faces(_) => {
            let refs: Vec<(String, &crate::doc::reference::Reference)> = t
                .references()
                .into_iter()
                .filter(|(role, _)| role.starts_with("from."))
                .collect();
            let (from, built, names) = resolve_faces(part, id, "thicken", &refs, before)?;
            let (sheet, _) = ryukei::sheet::extract(extract_key(&part.id, id), &built.body, &names)
                .map_err(|e| extract_error(part, id, from, &e))?;
            Ok((from, built, std::borrow::Cow::Owned(sheet)))
        }
    }
}

/// Refuses a thicken whose sheet or faces don't resolve, in `results`
/// (what the kernel makes of them only rebuild can tell).
pub fn check_thicken(
    part: &Part,
    id: &Id,
    t: &crate::doc::surface::Thicken,
    results: &dyn Results,
) -> Result<()> {
    let solids = crate::solid::Solids::of(part);
    thicken_input(part, &solids, id, t, results).map(drop)
}

/// A thicken's own body: its sheet (or its faces extracted) given its
/// front and back, each group's faces their own, by one kernel call.
/// `build_body` founds it or applies it to the targets.
pub fn thicken(
    part: &Part,
    solids: &crate::solid::Solids,
    id: &Id,
    t: &crate::doc::surface::Thicken,
    before: &dyn Results,
    run: &Run,
) -> Result<Built> {
    let (from, built, sheet) = thicken_input(part, solids, id, t, before)?;
    let spec = ryukei::offset::ThickenSpec {
        front: t.front.meters(),
        back: t.back.meters(),
        faces: t
            .faces
            .iter()
            .flat_map(|g| {
                g.faces
                    .iter()
                    .map(move |r| (r.name, g.front.meters(), g.back.meters()))
            })
            .collect(),
    };
    let key = crate::rebuild::op_key(&part.id, id);
    let work = crate::work::work(run.job);
    let made = ryukei::offset::thicken_with(key, &sheet, &spec, &work).map_err(|e| {
        crate::work::stopped(&e, run.job)
            .unwrap_or_else(|| crate::body::kernel_error(part, id, "thicken", (from, built), e))
    })?;
    let origins = thicken_origins(id, key, built, &sheet, &made);
    crate::rebuild::finish(id, "thicken", made, origins)
}

/// A thicken's origins: a face, edge or vertex moved to the front (or the
/// back) is `{feature, front_of | back_of: <name>}` of the sheet's entity
/// (the kernel names it `ThickenRole::Front` or `Back` from that entity
/// and journals it `Replaced`); a wall `{feature, wall_of: [<edges>]}` of
/// the laminar edges it replaces; an entity a side at zero left with its
/// name keeps its origin; anything else (fills, new points) the
/// feature's.
fn thicken_origins(
    id: &Id,
    key: OpKey,
    from: &Built,
    sheet: &Body,
    made: &(Body, ryukei::name::Journal),
) -> Origins {
    use ryukei::name::{Event, ThickenRole};
    let mut of: std::collections::BTreeMap<Name, Value> = Default::default();
    let mut walls: std::collections::BTreeMap<Name, Vec<String>> = Default::default();
    // Every sheet entity a side moves takes `Front` or `Back` from it,
    // whether or not the journal says it was replaced (a side at zero
    // keeps the sheet's names, and the kernel journals nothing then).
    for n in crate::rebuild::names(sheet) {
        for (role, word) in [
            (ThickenRole::Front, "front_of"),
            (ThickenRole::Back, "back_of"),
        ] {
            let m = Name::derive(key, Role::Thicken(role), &[n]);
            if made.0.by_name(m).is_some() {
                of.insert(m, json!({"feature": id, word: n.to_string()}));
            }
        }
    }
    for e in made.1.events() {
        if let Event::Replaced { old, new } = e {
            for n in new {
                if !of.contains_key(n) {
                    walls.entry(*n).or_default().push(old.to_string());
                }
            }
        }
    }
    crate::rebuild::names(&made.0)
        .into_iter()
        .map(|n| {
            let o = match (of.get(&n), walls.get(&n)) {
                (Some(o), _) => o.clone(),
                (None, Some(w)) => {
                    let mut w = w.clone();
                    w.sort();
                    json!({"feature": id, "wall_of": w})
                }
                (None, None) => from
                    .origins
                    .get(&n.to_string())
                    .cloned()
                    .unwrap_or_else(|| json!({"feature": id})),
            };
            (n.to_string(), o)
        })
        .collect()
}

/// A fill, extract, sew or thicken as output reports it: its file's
/// fields, references as text, quantities with their meters.
pub fn report(part: &Id, f: &Feature) -> Value {
    match f {
        Feature::Fill(x) => {
            let mut v = x.fields();
            for (i, s) in x.sides.iter().enumerate() {
                v["sides"][i] = match s {
                    Side::Edge { edge, tangent } => {
                        json!({"edge": edge.text(part), "tangent": tangent})
                    }
                    Side::Curve(g) => json!({"curve": g.text(part)}),
                };
            }
            v["tolerance"] = x.tolerance.report();
            v["angle"] = x.angle.report();
            v
        }
        Feature::Extract(x) => {
            json!({"faces": x.faces.iter().map(|r| r.text(part)).collect::<Vec<_>>()})
        }
        Feature::Sew(x) => {
            let mut v = x.fields();
            v["tolerance"] = x.tolerance.report();
            v
        }
        Feature::Thicken(t) => {
            let mut v = t.fields();
            if let crate::doc::surface::ThickenFrom::Faces(f) = &t.from {
                v["from"]["faces"] = json!(f.iter().map(|r| r.text(part)).collect::<Vec<_>>());
            }
            v["front"] = t.front.report();
            v["back"] = t.back.report();
            for (i, g) in t.faces.iter().enumerate() {
                v["faces"][i]["front"] = g.front.report();
                v["faces"][i]["back"] = g.back.report();
                v["faces"][i]["faces"] =
                    json!(g.faces.iter().map(|r| r.text(part)).collect::<Vec<_>>());
            }
            v
        }
        _ => unreachable!("a fill, an extract, a sew or a thicken"),
    }
}

/// A fill, extract, sew or thicken (`ty`) from `current`'s fields (none
/// for a new one) with what was given laid over them. A fill: `sides`
/// whole (else its `edges` as edge sides, or its one `curve`),
/// `tolerance`, `angle`. An extract: `faces` (else `face_list`, else
/// `face`) whole. A sew: `sheets`, `tolerance`, `make_solid`. A thicken:
/// its `sheet` (or one `sheets`) or its faces (`from_faces`, `face_list`
/// or `face`), `front`, `back`, `face_sides` whole, `operation`,
/// `targets`.
pub fn from_given(
    ty: &str,
    current: Option<&Feature>,
    f: &crate::proto::FeatureFields,
    part: &Id,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<Feature> {
    use crate::doc::files::Obj;
    use crate::doc::geometry::GeomRef;
    use crate::doc::reference::Reference;
    let mut v = match current {
        Some(Feature::Fill(x)) => x.fields(),
        Some(Feature::Extract(x)) => x.fields(),
        Some(Feature::Sew(x)) => x.fields(),
        Some(Feature::Thicken(x)) => x.fields(),
        _ => json!({}),
    };
    let o = v.as_object_mut().expect("fields are an object");
    let reference = |t: &str, at: &str| -> Result<Value> {
        Ok(Reference::parse(t, part)
            .map_err(|e| e.with("pointer", at))?
            .to_json())
    };
    let curve = |t: &str, at: &str| -> Result<Value> {
        Ok(GeomRef::parse(t, part)
            .map_err(|e| e.with("pointer", at))?
            .to_json())
    };
    match ty {
        "fill" => {
            let sides: Option<Vec<Value>> = if let Some(Value::Array(given)) = &f.sides {
                Some(
                    given
                        .iter()
                        .enumerate()
                        .map(|(i, s)| {
                            let at = format!("/sides/{i}");
                            match s {
                                Value::String(t) => Ok(json!({"curve": curve(t, &at)?})),
                                Value::Object(m) => match (m.get("edge"), m.get("curve")) {
                                    (Some(Value::String(t)), None) => {
                                        let mut x = json!({"edge": reference(t, &format!("{at}/edge"))?});
                                        if m.get("tangent") == Some(&Value::Bool(true)) {
                                            x["tangent"] = json!(true);
                                        }
                                        Ok(x)
                                    }
                                    (None, Some(Value::String(t))) => {
                                        Ok(json!({"curve": curve(t, &format!("{at}/curve"))?}))
                                    }
                                    _ => Err(Error::new(
                                        Code::InvalidParam,
                                        "a fill's side is {\"edge\": text, \"tangent\": bool} or {\"curve\": text}",
                                    )
                                    .with("param", "sides")
                                    .with("pointer", at)),
                                },
                                _ => Err(Error::new(Code::InvalidParam, "a fill's sides are a list")
                                    .with("param", "sides")),
                            }
                        })
                        .collect::<Result<Vec<_>>>()?,
                )
            } else if let Some(edges) = &f.edges {
                Some(
                    edges
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            Ok(json!({"edge": reference(t, &format!("/sides/{i}/edge"))?}))
                        })
                        .collect::<Result<Vec<_>>>()?,
                )
            } else if let Some(c) = &f.curve {
                Some(vec![json!({"curve": curve(c, "/sides/0/curve")?})])
            } else {
                None
            };
            if let Some(s) = sides {
                o.insert("sides".into(), Value::Array(s));
            }
            if let Some(q) = &f.tolerance {
                o.insert("tolerance".into(), q.clone());
            }
            if let Some(q) = &f.angle {
                o.insert("angle".into(), q.clone());
            }
        }
        "sew" => {
            if let Some(s) = &f.sheets {
                o.insert("sheets".into(), json!(s));
            }
            if let Some(q) = &f.tolerance {
                o.insert("tolerance".into(), q.clone());
            }
            if let Some(b) = f.make_solid {
                o.insert("solid".into(), json!(b));
            }
        }
        "thicken" => {
            let one_sheet = match (&f.sheet, &f.sheets) {
                (Some(s), _) => Some(s.clone()),
                (None, Some(l)) if l.len() == 1 => Some(l[0].clone()),
                (None, Some(l)) => {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!("a thicken thickens one sheet, not {}", l.len()),
                    )
                    .with("param", "sheet"));
                }
                (None, None) => None,
            };
            let faces: Option<Vec<String>> = f
                .from_faces
                .clone()
                .or_else(|| f.face_list.clone())
                .or_else(|| f.face.clone().map(|x| vec![x]));
            match (one_sheet, faces) {
                (Some(_), Some(_)) => {
                    return Err(Error::new(
                        Code::Usage,
                        "a thicken thickens a sheet or faces, not both",
                    ));
                }
                (Some(s), None) => {
                    o.insert("from".into(), json!({"sheet": s}));
                }
                (None, Some(fs)) => {
                    let refs = fs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| reference(t, &format!("/from/faces/{i}")))
                        .collect::<Result<Vec<_>>>()?;
                    o.insert("from".into(), json!({"faces": refs}));
                }
                (None, None) => {}
            }
            for (k, q) in [("front", &f.front), ("back", &f.back)] {
                if let Some(q) = q {
                    o.insert(k.into(), q.clone());
                }
            }
            if let Some(groups) = &f.face_sides {
                let Value::Array(gs) = groups else {
                    return Err(Error::new(
                        Code::InvalidParam,
                        "a thicken's face_sides are a list of groups {front, back, faces}",
                    )
                    .with("param", "face_sides"));
                };
                let mut out = Vec::with_capacity(gs.len());
                for (i, g) in gs.iter().enumerate() {
                    let mut g = g.clone();
                    if let Some(Value::Array(fs)) = g.get_mut("faces") {
                        for (j, x) in fs.iter_mut().enumerate() {
                            if let Some(t) = x.as_str().map(str::to_owned) {
                                *x = reference(&t, &format!("/faces/{i}/faces/{j}"))?;
                            }
                        }
                    }
                    out.push(g);
                }
                if out.is_empty() {
                    o.remove("faces");
                } else {
                    o.insert("faces".into(), Value::Array(out));
                }
            }
            if let Some(op) = &f.operation {
                o.insert("operation".into(), json!(op));
                if op == "new" {
                    o.remove("targets");
                }
            }
            if let Some(t) = &f.targets {
                o.insert("targets".into(), json!(t));
            }
            // A new thicken founds a solid unless told otherwise.
            o.entry("operation").or_insert(json!("new"));
        }
        _ => {
            let given: Option<Vec<String>> = match (&f.faces, &f.face_list, &f.face) {
                (Some(Value::Array(a)), _, _) => Some(
                    a.iter()
                        .map(|x| {
                            x.as_str().map(str::to_owned).ok_or_else(|| {
                                Error::new(
                                    Code::InvalidParam,
                                    "an extract's faces are text, <part>/<solid>/<name>",
                                )
                                .with("param", "faces")
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                ),
                (_, Some(l), _) => Some(l.clone()),
                (_, None, Some(one)) => Some(vec![one.clone()]),
                _ => None,
            };
            if let Some(faces) = given {
                let refs = faces
                    .iter()
                    .enumerate()
                    .map(|(i, t)| reference(t, &format!("/faces/{i}")))
                    .collect::<Result<Vec<_>>>()?;
                o.insert("faces".into(), Value::Array(refs));
            }
        }
    }
    let mut r = Obj::new(&v, "")?;
    let out = match ty {
        "fill" => Feature::Fill(Box::new(Fill::from_fields(&mut r, unit, env)?)),
        "sew" => Feature::Sew(Box::new(crate::doc::surface::Sew::from_fields(
            &mut r, unit, env,
        )?)),
        "thicken" => Feature::Thicken(Box::new(crate::doc::surface::Thicken::from_fields(
            &mut r, unit, env,
        )?)),
        _ => Feature::Extract(Box::new(Extract::from_fields(&mut r)?)),
    };
    r.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::geom::{Curve, Line, Point3, Vector3};

    fn line(a: [f64; 3], b: [f64; 3]) -> (Curve, [f64; 2]) {
        let (a, b) = (Point3::new(a[0], a[1], a[2]), Point3::new(b[0], b[1], b[2]));
        let d = b - a;
        let len = d.length();
        (Curve::Line(Line::new(a, d).unwrap()), [0.0, len])
    }

    #[test]
    fn a_square_of_lines_fills_one_plane_face_and_its_edges_name_their_sides() {
        let key = ryukei::name::OpKey::from_bytes(b"ryucad.feature\0p\0f");
        let pts = [
            [0.0, 0.0, 0.0],
            [0.01, 0.0, 0.0],
            [0.01, 0.01, 0.0],
            [0.0, 0.01, 0.0],
        ];
        let sides: Vec<FillSide<'_>> = (0..4)
            .map(|k| {
                let (curve, range) = line(pts[k], pts[(k + 1) % 4]);
                FillSide::Curve { curve, range }
            })
            .collect();
        let opts = FillOptions {
            tolerance: 1e-8,
            angle: 1f64.to_radians(),
        };
        let (body, journal, report) =
            ryukei::fill::fill(key, &FillBoundary { sides }, &opts).unwrap();
        // Each kernel side is a file side of its own (none is a path).
        let place: Vec<Place> = (0..4)
            .map(|i| Place {
                side: i,
                piece: None,
                edge: None,
            })
            .collect();
        let id = Id::parse("f", "feature").unwrap();
        let o = fill_origins(&id, key, &place, &body);
        let faces: Vec<&Value> = body
            .faces()
            .map(|f| {
                o.get(&body.name_of(EntityId::Face(f)).unwrap().to_string())
                    .unwrap()
            })
            .collect();
        assert_eq!(faces, [&json!({"feature": "f", "fill": "plane"})]);
        let mut sides: Vec<i64> = body
            .edges()
            .filter_map(|e| o.get(&body.name_of(EntityId::Edge(e)).unwrap().to_string()))
            .filter_map(|v| v["side"].as_i64())
            .collect();
        sides.sort();
        assert_eq!(sides, [0, 1, 2, 3]);
        let r = fill_report(&place, &report);
        assert_eq!(r["sides"].as_array().unwrap().len(), 4);
        let _ = (journal, Vector3::new(0.0, 0.0, 1.0));
    }
    #[test]
    fn an_extracts_entities_keep_their_names_and_say_what_they_were() {
        let b = ryukei::primitive::block(
            OpKey::from_bytes(b"k"),
            0.01,
            0.01,
            0.01,
            &ryukei::geom::Rigid::IDENTITY,
        )
        .unwrap()
        .0;
        let top = b.faces().next().unwrap();
        let name = b.name_of(EntityId::Face(top)).unwrap();
        let (sheet, _) = ryukei::sheet::extract(OpKey::from_bytes(b"x"), &b, &[name]).unwrap();
        let id = Id::parse("x", "feature").unwrap();
        let o = extract_origins(&id, &sheet);
        assert_eq!(
            o.get(&name.to_string()),
            Some(&json!({"feature": "x", "extract_of": name.to_string()}))
        );
        assert!(sheet.by_name(name).is_some());
    }

    #[test]
    fn variants_are_named_in_snake_case() {
        assert_eq!(variant_snake("GapTooLarge { at: .. }"), "gap_too_large");
        assert_eq!(variant_snake("OpenShell"), "open_shell");
    }
}
