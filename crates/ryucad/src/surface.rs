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
pub fn fill(part: &Part, id: &Id, x: &Fill, before: &dyn Results, run: &Run) -> Result<(Built, Value)> {
    let resolved = resolve_sides(part, id, x, before)?;
    let mut sides: Vec<FillSide<'_>> = Vec::new();
    let mut place: Vec<Place> = Vec::new();
    for (i, r) in resolved.iter().enumerate() {
        match r {
            Resolved::Edge(body, name, tangent) => {
                sides.push(FillSide::Edge { body, edge: *name, tangent: *tangent });
                place.push(Place { side: i, piece: None, edge: Some(*name) });
            }
            Resolved::Curve(chain) => {
                let many = chain.pieces.len() > 1;
                for (j, p) in chain.pieces.iter().enumerate() {
                    // Either end of a side may come first; the kernel takes
                    // its range increasing.
                    let [a, b] = p.range;
                    let range = if a <= b { [a, b] } else { [b, a] };
                    sides.push(FillSide::Curve { curve: p.curve.clone(), range });
                    place.push(Place { side: i, piece: many.then_some(j), edge: None });
                }
            }
        }
    }
    let key = crate::rebuild::op_key(&part.id, id);
    let opts = FillOptions {
        tolerance: x.tolerance.meters(),
        angle: x.angle.meters(),
    };
    let work = crate::work::work(run.job);
    let (body, journal, report) = ryukei::fill::fill_with(key, &FillBoundary { sides }, &opts, &work)
        .map_err(|e| {
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
            let n = Name::derive(key, Role::Fill(FillRole::Half), &[stand_in, Name::from_u128(half)]);
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
                _ => sides.get(&n).cloned().unwrap_or_else(|| json!({"feature": id})),
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
            F::AngleTooLarge { side, angle } => {
                ("angle_too_large", *side, Some(("angle_deg", angle.to_degrees())))
            }
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
        .map(|n| (n.to_string(), json!({"feature": id, "extract_of": n.to_string()})))
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
            err = err.with("reason", "no_such_face").with("face", name.to_string());
        }
        _ => err = err.with("reason", "kernel"),
    }
    err
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
        let pts = [[0.0, 0.0, 0.0], [0.01, 0.0, 0.0], [0.01, 0.01, 0.0], [0.0, 0.01, 0.0]];
        let sides: Vec<FillSide<'_>> = (0..4)
            .map(|k| {
                let (curve, range) = line(pts[k], pts[(k + 1) % 4]);
                FillSide::Curve { curve, range }
            })
            .collect();
        let opts = FillOptions { tolerance: 1e-8, angle: 1f64.to_radians() };
        let (body, journal, report) =
            ryukei::fill::fill(key, &FillBoundary { sides }, &opts).unwrap();
        // Each kernel side is a file side of its own (none is a path).
        let place: Vec<Place> = (0..4).map(|i| Place { side: i, piece: None, edge: None }).collect();
        let id = Id::parse("f", "feature").unwrap();
        let o = fill_origins(&id, key, &place, &body);
        let faces: Vec<&Value> = body
            .faces()
            .map(|f| o.get(&body.name_of(EntityId::Face(f)).unwrap().to_string()).unwrap())
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
        let b = ryukei::primitive::block(OpKey::from_bytes(b"k"), 0.01, 0.01, 0.01, &ryukei::geom::Rigid::IDENTITY)
            .unwrap()
            .0;
        let top = b.faces().next().unwrap();
        let name = b.name_of(EntityId::Face(top)).unwrap();
        let (sheet, _) = ryukei::sheet::extract(OpKey::from_bytes(b"x"), &b, &[name]).unwrap();
        let id = Id::parse("x", "feature").unwrap();
        let o = extract_origins(&id, &sheet);
        assert_eq!(o.get(&name.to_string()), Some(&json!({"feature": "x", "extract_of": name.to_string()})));
        assert!(sheet.by_name(name).is_some());
    }
}
