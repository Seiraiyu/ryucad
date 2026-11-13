//! What a render or a mesh answer is made of: each solid's display
//! mesh (cached by content), an assembly's placed occurrences, the
//! points a camera frames, and a render's scene (geometry, camera,
//! highlights, ghosts, sections, explodes, annotations). Moved out of
//! `server` (ryucad 27.B): annotations and release packages render
//! through it too.

use crate::budget::{mesh_budget, render_budget};
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::entity::{self, EntityRef};
use crate::error::{Code, Error, Result};
use crate::live::{self, Live};
use crate::mesh;
use crate::part::{feature, part_id, part_kind};
use crate::proto::Answer;
use crate::rebuild;
use crate::session::Session;
use crate::solid::Solids;
use crate::view::{DEFAULT_PROJECTION, StandardView};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The display meshes of a part's solids as the model shows them,
/// `(solid, body, mesh)` by founder; a solid that never built has its
/// error instead. `only` keeps one solid; a sketch has none.
pub fn meshes(
    session: &mut Session,
    part: &Id,
    only: Option<&Id>,
    quality: mesh::Quality,
) -> Result<Vec<(Id, Result<mesh::Meshed>)>> {
    let p = session.project().part(part)?;
    let solids = Solids::of(p);
    if let Some(f) = only {
        // An inserted solid's key names no feature of its own: its
        // insert's family holds it (design 521).
        let sketch = !f.is_path() && feature(p, f)?.sketch().is_some();
        if !sketch && !solids.is_solid(f) {
            return Err(rebuild::not_a_solid(p, &solids, f, "feature", f));
        }
    }
    let features: BTreeMap<Id, Feature> = p
        .features
        .iter()
        .filter_map(|(id, f)| Some((id.clone(), f.as_ref().ok()?.clone())))
        .collect();
    let budget = mesh_budget();
    let result = session.rebuilt(part)?;
    // The feature each shown state is from sizes its mesh.
    let built: Vec<(Id, Result<(Id, crate::rebuild::Built)>)> = result
        .shown(&solids)
        .into_iter()
        .filter(|s| only.is_none_or(|o| o == s.solid))
        .map(|s| {
            let r = match (s.state, s.error) {
                (Some((f, b)), _) => Ok((f.clone(), b.as_ref().clone())),
                (None, Some(e)) => Err(e.clone()),
                (None, None) => unreachable!("a solid that never built failed"),
            };
            (s.solid.clone(), r)
        })
        .collect();
    let ok: Vec<(&Feature, &crate::rebuild::Built)> = built
        .iter()
        .filter_map(|(_, b)| {
            let (f, b) = b.as_ref().ok()?;
            Some((&features[f], b))
        })
        .collect();
    let job = session.job.clone();
    let mut made = session
        .meshes
        .get_all_for(&ok, quality, budget, &job)?
        .into_iter();
    Ok(built
        .into_iter()
        .map(|(id, b)| {
            let r = b.map(|(_, b)| {
                let mesh = made.next().expect("one mesh per built body");
                let warning = mesh::unmeshed_warning(part, &id, &b, &mesh);
                mesh::Meshed {
                    body: b.body,
                    hash: b.hash,
                    mesh,
                    warning,
                }
            });
            (id, r)
        })
        .collect())
}

/// Solid `solid` of `part` as feature `reader` read it, meshed: the last
/// state its chain built before `reader`, shown or not (a deleted solid's
/// ghost, ryucad 25.B).
pub fn mesh_before(
    session: &mut Session,
    part: &Id,
    solid: &Id,
    reader: &Id,
    quality: mesh::Quality,
) -> Result<mesh::Meshed> {
    let p = session.project().part(part)?;
    feature(p, reader)?;
    let solids = Solids::of(p);
    if !solids.is_solid(solid) {
        return Err(rebuild::not_a_solid(p, &solids, solid, "feature", solid));
    }
    let features: BTreeMap<Id, Feature> = p
        .features
        .iter()
        .filter_map(|(id, f)| Some((id.clone(), f.as_ref().ok()?.clone())))
        .collect();
    let result = session.rebuilt(part)?;
    let Some((f, b)) = result.read_by(&solids, solid, reader) else {
        return Err(Error::new(
            Code::NotFound,
            format!("{reader} reads no {solid} of {part}: nothing of it built before {reader}"),
        )
        .with("part", part)
        .with("feature", solid)
        .with("before", reader));
    };
    let b = b.as_ref().clone();
    let job = session.job.clone();
    let mesh = session
        .meshes
        .get_all_for(&[(&features[&f], &b)], quality, mesh_budget(), &job)?
        .pop()
        .expect("one mesh for one body");
    let warning = mesh::unmeshed_warning(part, solid, &b, &mesh);
    Ok(mesh::Meshed {
        body: b.body,
        hash: b.hash,
        mesh,
        warning,
    })
}

/// The points of the given entities, or of the whole model when there
/// are none, from coarse display meshes: what a camera frames.
pub fn model_points(session: &mut Session, refs: &[EntityRef]) -> Result<Vec<[f64; 3]>> {
    let mut points = Vec::new();
    let mut grow = |p: &ryukei::geom::Point3| points.push([p.x, p.y, p.z]);
    let coarse = mesh::Quality::Coarse;
    // The whole model: each part shown, an assembly as all its
    // occurrences (an occurrence reference with no path).
    let mut asked = refs.to_vec();
    if refs.is_empty() {
        for part in crate::assembly::shown_parts(session.project()) {
            if part_kind(session.project().part(&part)?) == PartKind::Assembly {
                asked.push(EntityRef {
                    feature: part.clone(),
                    part,
                    path: Vec::new(),
                    name: String::new(),
                });
                continue;
            }
            for (_, m) in meshes(session, &part, None, coarse)? {
                if let Ok(Ok(mesh)) = m.as_ref().map(|m| m.mesh.mesh.as_ref()) {
                    mesh.positions.iter().for_each(&mut grow);
                }
            }
        }
    }
    for r in &asked {
        let p = session.project().part(&r.part)?;
        if part_kind(p) == PartKind::Assembly {
            let r = r.in_assembly();
            let model = session.model(&r.part, &crate::doc::assembly::Rev::Working)?;
            let chosen: Vec<&crate::assembly::Occurrence> = model
                .occurrences
                .iter()
                .filter(|o| match r.is_occurrence() {
                    true => o.path.starts_with(&r.path),
                    false => o.path == r.path,
                })
                .collect();
            for o in chosen {
                let solids: Vec<&crate::assembly::Solid> = o
                    .solids
                    .iter()
                    .filter(|s| r.is_occurrence() || s.founder == r.feature)
                    .collect();
                for (s, m) in solids
                    .iter()
                    .zip(occurrence_meshes(session, &solids, coarse)?)
                {
                    let Ok(mesh) = &m.mesh else { continue };
                    let mut placed = |q: &ryukei::geom::Point3| grow(&o.pose.apply_point(*q));
                    match r.is_occurrence() {
                        true => mesh.positions.iter().for_each(&mut placed),
                        false => named_points(&s.built.body, mesh, &r.name, &mut placed),
                    }
                }
            }
            continue;
        }
        // A sketch entity shows by its points.
        if let Some(sk) = session
            .rebuilt(&r.part)?
            .output(&r.feature)
            .and_then(|o| o.sketch())
        {
            let id = Id::parse(&r.name, "entity")?;
            for [x, y, z] in crate::sketch::entity_points(sk, &id) {
                grow(&ryukei::geom::Point3::new(x, y, z));
            }
            continue;
        }
        for (_, m) in meshes(session, &r.part, Some(&r.feature), coarse)? {
            let Ok(m) = m else { continue };
            let Ok(mesh) = &m.mesh.mesh else { continue };
            named_points(&m.body, mesh, &r.name, &mut grow);
        }
    }
    Ok(points)
}

/// The mesh points of the face, edge or vertex of `body` named `name`.
fn named_points(
    body: &ryukei::topo::Body,
    mesh: &ryukei::mesh::BodyMesh,
    name: &str,
    grow: &mut impl FnMut(&ryukei::geom::Point3),
) {
    let named = |n: String| n == name;
    for f in &mesh.faces {
        if named(body.face(f.face).expect("live").name.to_string()) {
            for c in &mesh.corners[f.corners.start as usize..f.corners.end as usize] {
                grow(&mesh.positions[c.position as usize]);
            }
        }
    }
    for e in &mesh.edges {
        if named(body.edge(e.edge).expect("live").name.to_string()) {
            e.positions
                .iter()
                .for_each(|p| grow(&mesh.positions[*p as usize]));
        }
    }
    for (i, v) in body.vertices().enumerate() {
        if named(body.vertex(v).expect("live").name.to_string()) {
            grow(&mesh.positions[i]);
        }
    }
}

/// The display meshes of occurrences' solids, in order, each body meshed
/// once however many occurrences place it.
pub fn occurrence_meshes(
    session: &mut Session,
    solids: &[&crate::assembly::Solid],
    quality: mesh::Quality,
) -> Result<Vec<std::sync::Arc<mesh::DisplayMesh>>> {
    let budget = mesh_budget();
    let items: Vec<(&Feature, &crate::rebuild::Built)> = solids
        .iter()
        .map(|s| (s.feature.as_ref(), s.built.as_ref()))
        .collect();
    let job = session.job.clone();
    session.meshes.get_all_for(&items, quality, budget, &job)
}

/// A render request, unpacked.
pub(crate) struct RenderAsk {
    pub(crate) part: Option<String>,
    pub(crate) camera: Option<crate::view::CameraArg>,
    pub(crate) view: Option<String>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) highlight: Vec<String>,
    pub(crate) edges: bool,
    pub(crate) labels: bool,
    pub(crate) sketches: bool,
    /// Draw the shown parts' datums.
    pub(crate) datums: bool,
    pub(crate) rev: Option<String>,
    pub(crate) frame: Vec<String>,
    pub(crate) compare: Option<String>,
    /// What to leave out: occurrences and solids, at the leaves.
    pub(crate) hidden: std::collections::BTreeSet<String>,
    /// Draw the origin's axes and planes.
    pub(crate) origin: bool,
    /// Draw each face in its appearance, else the fixed theme.
    pub(crate) appearance: bool,
    /// Cut away what lies beyond these planes.
    pub(crate) sections: Vec<crate::section::Plane>,
    /// An explode to draw.
    pub(crate) explode: Option<DrawExplode>,
    /// Draw open annotations' pins, numbered.
    pub(crate) annotations: bool,
    /// A sheet metal solid whose flat pattern to draw instead (`""`: the
    /// part's one).
    pub(crate) flat: Option<String>,
    /// A camera in metres that wins over `camera`, a participant's and
    /// `view` (a pick pass, `render --annotation`).
    pub(crate) camera_m: Option<crate::view::Camera>,
    /// Tag every entity drawn, for a pick pass.
    pub(crate) pick: bool,
    /// An annotation to draw, its pins and marks.
    pub(crate) annotation: Option<crate::annotation::Annotation>,
}

impl RenderAsk {
    /// What a pick pass draws of `part` seen through `camera`: the
    /// working model (or the session's draft), every edge and sketch,
    /// no datums, labels, appearance, compare or explode, cut by
    /// `sections` and leaving out `hidden`.
    pub(crate) fn pick(
        part: &str,
        camera: crate::view::Camera,
        sections: Vec<crate::section::Plane>,
        hidden: std::collections::BTreeSet<String>,
    ) -> RenderAsk {
        RenderAsk {
            part: Some(part.to_owned()),
            camera: None,
            view: None,
            width: 800,
            height: 600,
            highlight: Vec::new(),
            edges: true,
            labels: false,
            sketches: true,
            datums: false,
            rev: None,
            frame: Vec::new(),
            compare: None,
            hidden,
            origin: false,
            appearance: false,
            sections,
            explode: None,
            annotations: false,
            flat: None,
            camera_m: Some(camera),
            pick: true,
            annotation: None,
        }
    }
}

/// What a render draws and from where: its geometry, camera, each
/// assembly's occurrences' poses by instance path, the
/// commit read, the compare's revision and commit, and its warnings.
pub(crate) struct Scene {
    pub(crate) g: crate::render::Geometry,
    pub(crate) camera: crate::view::Camera,
    pub(crate) poses: BTreeMap<String, BTreeMap<String, ryukei::geom::Rigid>>,
    pub(crate) commit: Option<String>,
    pub(crate) compare: Option<Value>,
    pub(crate) warnings: Vec<Value>,
}

/// An explode a render draws.
pub(crate) struct DrawExplode {
    /// The assembly of the explode a participant's window shows, which the
    /// render draws only if it draws that assembly; `None` for one the
    /// request (or a named view) names, of the one part rendered.
    pub(crate) shown: Option<Id>,
    pub(crate) id: String,
    /// How far through it.
    pub(crate) fraction: f64,
}

/// The explode a render draws: its assembly, where it moves each
/// occurrence, its resolved steps and how far through them.
type Exploded = (
    Id,
    BTreeMap<String, crate::explode::Offset>,
    Vec<crate::explode::Step>,
    f64,
);

/// How a compare paints one rendered solid: wholly new when the other
/// side doesn't have it, else its faces new or changed by name.
fn compare_paint(
    theirs: Option<&mesh::Meshed>,
    ours: &mesh::Meshed,
) -> (
    Option<crate::render::Rgb>,
    BTreeMap<String, crate::render::Rgb>,
) {
    use crate::render as r;
    let Some(theirs) = theirs else {
        return (Some(r::COMPARE_NEW), BTreeMap::new());
    };
    let mut faces = BTreeMap::new();
    if theirs.hash != ours.hash {
        let d = crate::diff::bodies(&theirs.body, &ours.body);
        for (kind, color) in [("new", r::COMPARE_NEW), ("changed", r::COMPARE_CHANGED)] {
            for n in d["faces"][kind].as_array().into_iter().flatten() {
                if let Some(n) = n.as_str() {
                    faces.insert(n.to_owned(), color);
                }
            }
        }
    }
    (None, faces)
}

/// Adds `m`'s faces to `g` as ghosts, where `pose` puts them: those
/// `gone` says in the removed colour, the rest in the ghost colour.
pub(crate) fn ghost(
    g: &mut crate::render::Geometry,
    m: &mesh::Meshed,
    pose: &ryukei::geom::Rigid,
    gone: impl Fn(&str) -> bool,
) {
    use crate::render as r;
    let Ok(mesh) = &m.mesh.mesh else { return };
    let moved = *pose != ryukei::geom::Rigid::IDENTITY;
    let base = g.positions.len() as u32;
    g.positions.extend(mesh.positions.iter().map(|p| {
        let p = if moved { pose.apply_point(*p) } else { *p };
        [p.x, p.y, p.z]
    }));
    for f in &mesh.faces {
        let name = m.body.face(f.face).expect("live").name.to_string();
        let color = if gone(&name) {
            r::COMPARE_REMOVED
        } else {
            r::COMPARE_GHOST
        };
        for t in &mesh.triangles[f.triangles.start as usize..f.triangles.end as usize] {
            let c = t.map(|i| mesh.corners[i as usize]);
            g.ghosts.push(r::Triangle {
                p: c.map(|c| base + c.position),
                n: c.map(|c| {
                    let n = c.normal.get();
                    let n = if moved { pose.apply_vector(n) } else { n };
                    [n.x as f32, n.y as f32, n.z as f32]
                }),
                color,
            });
        }
    }
}

/// The names of the faces `theirs` has and `ours` doesn't.
fn gone_faces(theirs: &mesh::Meshed, ours: &mesh::Meshed) -> std::collections::BTreeSet<String> {
    let d = crate::diff::bodies(&theirs.body, &ours.body);
    d["faces"]["gone"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n.as_str().map(str::to_owned))
        .collect()
}

/// Solids' meshes by part and founder.
type SolidMeshes = BTreeMap<(Id, Id), mesh::Meshed>;

/// Each solid's mesh at a revision, by part and founder, for the parts
/// given (those the revision has), and the commit it names.
fn solids_at(
    session: &mut Session,
    rev: &str,
    parts: &[Id],
) -> Result<(Option<String>, SolidMeshes)> {
    let (commit, s) = session.at(Some(rev))?;
    let mut out = BTreeMap::new();
    for part in parts {
        if s.project().part(part).is_err() {
            continue;
        }
        let Ok(list) = meshes(s, part, None, mesh::Quality::Normal) else {
            continue;
        };
        for (fid, m) in list {
            if let Ok(m) = m
                && m.mesh.mesh.is_ok()
            {
                out.insert((part.clone(), fid), m);
            }
        }
    }
    Ok((commit, out))
}

/// An assembly's occurrences' solids at a revision, by occurrence path
/// and founder: where each is and its mesh.
type OccurrenceMeshes = BTreeMap<(String, Id), (ryukei::geom::Rigid, mesh::Meshed)>;

/// [`OccurrenceMeshes`] of `part` at `rev`: empty where the revision has
/// no such assembly or it doesn't build.
fn occurrences_at(session: &mut Session, rev: &str, part: &Id) -> Result<OccurrenceMeshes> {
    let (_, s) = session.at(Some(rev))?;
    let mut out = BTreeMap::new();
    let assembly = s
        .project()
        .part(part)
        .is_ok_and(|p| part_kind(p) == PartKind::Assembly);
    if !assembly {
        return Ok(out);
    }
    let Ok(model) = s.model(part, &crate::doc::assembly::Rev::Working) else {
        return Ok(out);
    };
    for o in &model.occurrences {
        let path = crate::assembly::path_text(&o.path);
        let solids: Vec<&crate::assembly::Solid> = o.solids.iter().collect();
        let made = occurrence_meshes(s, &solids, mesh::Quality::Normal)?;
        for (sol, m) in solids.iter().zip(made) {
            if m.mesh.is_err() {
                continue;
            }
            let m = mesh::Meshed {
                body: sol.built.body.clone(),
                hash: sol.built.hash.clone(),
                mesh: m,
                warning: None,
            };
            out.insert((path.clone(), sol.founder.clone()), (o.pose, m));
        }
    }
    Ok(out)
}

/// The parts a render draws: the one asked for, or the project's model
/// ([`crate::assembly::shown_parts`]).
fn render_parts(session: &Session, part: Option<&str>) -> Result<Vec<Id>> {
    Ok(match part {
        Some(p) => {
            let id = part_id(p)?;
            session.project().part(&id)?;
            vec![id]
        }
        None => crate::assembly::shown_parts(session.project()),
    })
}

/// `render_too_large` for a render of `part` whose geometry passed
/// `budget` after `drawn` of its `total` occurrences (#400).
fn render_past_budget(part: &Id, drawn: usize, total: usize, bytes: usize, budget: usize) -> Error {
    Error::new(
        Code::RenderTooLarge,
        format!(
            "{part}'s render would hold more than {} MiB (a quarter of session.max_memory) \
             after {drawn} of its {total} occurrences: hide some (`--hidden`), render a \
             subassembly, or cut it with a section",
            budget >> 20,
        ),
    )
    .with("part", part)
    .with("occurrences", total)
    .with("drawn", drawn)
    .with("bytes", bytes)
    .with("budget", budget)
}

/// A release package's render (design 436): `render <part> --rev <commit>
/// --view iso --size 1600x1200` with the CLI's other defaults, no
/// participant's view, highlights, hidden set, sections or explode.
pub(crate) fn render_for_package(session: &mut Session, part: &Id, commit: &str) -> Result<Answer> {
    let ask = RenderAsk {
        part: Some(part.to_string()),
        camera: None,
        view: Some("iso".into()),
        width: 1600,
        height: 1200,
        highlight: Vec::new(),
        edges: true,
        labels: false,
        sketches: true,
        datums: true,
        rev: Some(commit.to_owned()),
        frame: Vec::new(),
        compare: None,
        hidden: Default::default(),
        origin: false,
        appearance: true,
        sections: Vec::new(),
        explode: None,
        annotations: false,
        flat: None,
        camera_m: None,
        pick: false,
        annotation: None,
    };
    render_noting(session, None, Vec::new(), ask)
}

/// [`render`], counting the meshes it makes against its budget, and
/// taking them back out of the mesh cache when its budget refuses it, so
/// a refused render leaves nothing behind (#407).
pub(crate) fn render_noting(
    session: &mut Session,
    target: Option<(&str, Option<&Live>)>,
    shown: Vec<live::Highlight>,
    ask: RenderAsk,
) -> Result<Answer> {
    session.meshes.note();
    let r = render(session, target, shown, ask);
    let refused = matches!(&r, Err(e) if e.code == Code::RenderTooLarge);
    session.meshes.end_note(refused);
    r
}

/// Renders the model. `target` names a participant whose camera to use
/// (with the live session to find it in, if any); `shown` are the
/// participants' highlights, drawn in their styles.
pub(crate) fn render(
    session: &mut Session,
    target: Option<(&str, Option<&Live>)>,
    shown: Vec<live::Highlight>,
    ask: RenderAsk,
) -> Result<Answer> {
    crate::render::check_size(ask.width, ask.height)?;
    if let Some(solid) = ask.flat.clone() {
        return render_flat(session, ask, solid);
    }
    let (width, height, edges, part) = (ask.width, ask.height, ask.edges, ask.part.clone());
    let scene = render_scene(session, target, shown, ask)?;
    let options = crate::render::Options {
        silhouettes: edges,
        triad: true,
    };
    let img = crate::render::render(&scene.g, &scene.camera, width, height, options)?;
    let mut result =
        json!({"part": part, "width": width, "height": height, "camera": scene.camera});
    if let Some(c) = scene.commit {
        result["commit"] = json!(c);
    }
    if let Some(c) = scene.compare {
        result["compare"] = c;
    }
    if !scene.warnings.is_empty() {
        result["warnings"] = json!(scene.warnings);
    }
    Ok(Answer::bytes(result, "image/png", img.png()))
}

/// The geometry and camera a render draws, without its image: what
/// `render` rasterizes, and what a pick pass ([`crate::render::pick`])
/// reads with `ask.pick`.
pub(crate) fn render_scene(
    session: &mut Session,
    target: Option<(&str, Option<&Live>)>,
    shown: Vec<live::Highlight>,
    ask: RenderAsk,
) -> Result<Scene> {
    use crate::render::{self as r, Geometry};
    let parts = render_parts(session.at(ask.rev.as_deref())?.1, ask.part.as_deref())?;
    // Other revisions' models only widen the framing.
    let mut extra: Vec<[f64; 3]> = Vec::new();
    for rev in &ask.frame {
        for m in solids_at(session, rev, &parts)?.1.values() {
            if let Ok(mesh) = &m.mesh.mesh {
                extra.extend(mesh.positions.iter().map(|p| [p.x, p.y, p.z]));
            }
        }
    }
    let other = match &ask.compare {
        Some(rev) => Some((rev.clone(), solids_at(session, rev, &parts)?)),
        None => None,
    };
    // A compare's assemblies: the other side's occurrences, and this
    // side's as they are drawn, to paint and ghost them (issue #135).
    let mut theirs_occurrences: BTreeMap<Id, OccurrenceMeshes> = BTreeMap::new();
    if let Some(rev) = &ask.compare {
        for part in &parts {
            theirs_occurrences.insert(part.clone(), occurrences_at(session, rev, part)?);
        }
    }
    let mut seen_occurrences: BTreeMap<(Id, String, Id), (ryukei::geom::Rigid, mesh::Meshed)> =
        BTreeMap::new();
    // The rendered side's solids, for the ghosts.
    let mut seen = SolidMeshes::new();
    let (commit, session) = session.at(ask.rev.as_deref())?;
    // What to colour, and how: every participant's highlights in their
    // styles, then the request's own as notes.
    let mut marks: BTreeMap<String, (r::Rgb, Option<String>)> = BTreeMap::new();
    for h in &shown {
        let color = match h.style {
            live::Style::Pending => r::PENDING,
            live::Style::Changed => r::CHANGED,
            live::Style::Note => r::NOTE,
        };
        for e in &h.entities {
            marks.insert(e.to_string(), (color, h.label.clone()));
        }
    }
    for e in entity::parse_all(session, &ask.highlight)? {
        marks.insert(e.to_string(), (r::NOTE, None));
    }
    let mut g = Geometry::default();
    if ask.pick {
        g.tags = Some(Default::default());
    }
    // Each assembly's occurrences' poses, by instance path, for the pins
    // `annotations` draws on them.
    let mut poses: BTreeMap<String, BTreeMap<String, ryukei::geom::Rigid>> = BTreeMap::new();
    // What the geometry holds, judged against the render's budget (#400).
    let budget = render_budget();
    let mut tally = r::Tally::default();
    // How each part's solids look, by key, unless asked for the theme.
    let mut looks: BTreeMap<Id, BTreeMap<String, crate::appearance::SolidLook>> = BTreeMap::new();
    if ask.appearance {
        for part in &parts {
            looks.insert(part.clone(), crate::appearance::looks(session, part)?);
        }
    }
    // The explode drawn, by its assembly: where it moves each occurrence,
    // and the lines it leaves.
    let mut warnings: Vec<Value> = Vec::new();
    let exploded: Option<Exploded> = match &ask.explode {
        None => None,
        // A window's explode of an assembly not drawn draws nothing.
        Some(DrawExplode { shown: Some(p), .. }) if !parts.contains(p) => None,
        Some(DrawExplode {
            shown: Some(part),
            id,
            fraction,
        }) => {
            // A window's explode that is gone (deleted, undone, checked out
            // away) draws nothing, with a warning (issue #262).
            match crate::explode::resolve(session, part, id) {
                Ok(r) => {
                    let offsets = crate::explode::offsets(&r.steps, *fraction);
                    Some((part.clone(), offsets, r.steps, *fraction))
                }
                Err(e) => {
                    warnings.push(json!({
                        "code": e.code,
                        "message": format!("the window's exploded view isn't drawn: {}", e.message),
                        "part": part,
                        "explode": id,
                    }));
                    None
                }
            }
        }
        Some(DrawExplode {
            shown: None,
            id,
            fraction,
        }) => {
            let [part] = parts.as_slice() else {
                return Err(Error::new(
                    Code::InvalidParam,
                    "the render shows several parts: render the assembly whose explode it is",
                )
                .with("explode", id.as_str()));
            };
            let r = crate::explode::resolve(session, part, id)?;
            let offsets = crate::explode::offsets(&r.steps, *fraction);
            Some((part.clone(), offsets, r.steps, *fraction))
        }
    };
    // Renders don't follow anyone's display quality, so they compare
    // across machines.
    for part in &parts {
        let look = |key: &str| looks.get(part).and_then(|l| l.get(key));
        let moved = exploded.as_ref().filter(|(p, ..)| p == part);
        // An assembly draws its occurrences where its instances put them.
        if part_kind(session.project().part(part)?) == PartKind::Assembly {
            let model = session.model(part, &crate::doc::assembly::Rev::Working)?;
            if let Some((_, _, steps, fraction)) = moved {
                for (path, start) in crate::explode::middles(&model) {
                    let trail = crate::explode::trail(steps, &path, start, *fraction);
                    if trail.len() < 2 || ask.hidden.contains(&format!("{part}/{path}")) {
                        continue;
                    }
                    let base = g.positions.len() as u32;
                    g.positions.extend(trail.iter().copied());
                    g.lines.push(crate::render::Line {
                        points: (base..base + trail.len() as u32).collect(),
                        color: crate::render::TRAIL,
                        width: 1.0,
                        dash: true,
                    });
                }
            }
            let total = model.occurrences.len();
            for (drawn, o) in model.occurrences.iter().enumerate() {
                let bytes = g.bytes_with(&mut tally) + session.meshes.noted_bytes();
                if bytes > budget {
                    return Err(render_past_budget(part, drawn, total, bytes, budget));
                }
                let path = crate::assembly::path_text(&o.path);
                if ask.hidden.contains(&format!("{part}/{path}")) {
                    continue;
                }
                let pose = match moved.and_then(|(_, offsets, ..)| offsets.get(&path)) {
                    Some(off) => off.0.then_after(&o.pose),
                    None => o.pose,
                };
                poses
                    .entry(part.to_string())
                    .or_default()
                    .insert(path.clone(), pose);
                let solids: Vec<&crate::assembly::Solid> = o.solids.iter().collect();
                let made = occurrence_meshes(session, &solids, mesh::Quality::Normal)?;
                for (s, m) in solids.iter().zip(made) {
                    let m = mesh::Meshed {
                        body: s.built.body.clone(),
                        hash: s.built.hash.clone(),
                        mesh: m,
                        warning: None,
                    };
                    let key = (path.clone(), s.founder.clone());
                    let (whole, faces) = match theirs_occurrences.get(part) {
                        Some(theirs) => compare_paint(theirs.get(&key).map(|(_, t)| t), &m),
                        None => (None, BTreeMap::new()),
                    };
                    draw_body(
                        &mut g,
                        &m,
                        &pose,
                        &format!("{part}/{path}/{}", s.founder),
                        &marks,
                        (whole, &faces, look(&format!("{path}/{}", s.founder))),
                        (ask.labels, ask.edges),
                    );
                    seen_occurrences.insert((part.clone(), key.0, key.1), (o.pose, m));
                }
            }
            let bytes = g.bytes_with(&mut tally) + session.meshes.noted_bytes();
            if bytes > budget {
                return Err(render_past_budget(part, total, total, bytes, budget));
            }
            continue;
        }
        let first = g.positions.len();
        for (fid, m) in meshes(session, part, None, mesh::Quality::Normal)? {
            let Ok(m) = m else { continue };
            if m.mesh.mesh.is_err() || ask.hidden.contains(&format!("{part}/{fid}")) {
                continue;
            }
            seen.insert((part.clone(), fid.clone()), m.clone());
            let (whole, faces) = match &other {
                Some((_, (_, theirs))) => {
                    compare_paint(theirs.get(&(part.clone(), fid.clone())), &m)
                }
                None => (None, BTreeMap::new()),
            };
            draw_body(
                &mut g,
                &m,
                &ryukei::geom::Rigid::IDENTITY,
                &format!("{part}/{fid}"),
                &marks,
                (whole, &faces, look(fid.as_str())),
                (ask.labels, ask.edges),
            );
            let bytes = g.bytes_with(&mut tally) + session.meshes.noted_bytes();
            if bytes > budget {
                return Err(render_past_budget(part, 1, 1, bytes, budget));
            }
        }
        if ask.sketches {
            sketches(session, part, &marks, ask.labels, &mut g)?;
        }
        if ask.datums {
            let drawn = (
                bounds(&g.positions[first..]),
                session.rebuilt(part)?.features.clone(),
            );
            for (fid, r) in &drawn.1 {
                let Ok(crate::rebuild::Output::Datum(d)) = r else {
                    continue;
                };
                let key = format!("{part}/{fid}");
                if ask.hidden.contains(&key) {
                    continue;
                }
                g.add_datum(&key, &d.geom, drawn.0, marks.get(&key).map(|m| m.0));
            }
            // Paths and helices too, as polylines.
            let (lo, hi) = drawn.0;
            let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            let span = ryukei::math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
            let span = if span.is_finite() { span } else { 0.0 };
            for (fid, r) in &drawn.1 {
                let Ok(crate::rebuild::Output::Path(m)) = r else {
                    continue;
                };
                let key = format!("{part}/{fid}");
                if ask.hidden.contains(&key) {
                    continue;
                }
                g.add_path(&key, &m.chain(), span, marks.get(&key).map(|m| m.0));
            }
        }
    }
    if let Some((_, (_, theirs))) = &other {
        for (key, t) in theirs {
            match seen.get(key) {
                Some(ours) if ours.hash == t.hash => {}
                Some(ours) => {
                    let gone = gone_faces(t, ours);
                    ghost(&mut g, t, &ryukei::geom::Rigid::IDENTITY, |n| {
                        gone.contains(n)
                    });
                }
                None => ghost(&mut g, t, &ryukei::geom::Rigid::IDENTITY, |_| true),
            }
        }
    }
    for (part, theirs) in &theirs_occurrences {
        for ((path, founder), (pose, t)) in theirs {
            // What this render leaves out leaves no ghost either.
            if ask.hidden.contains(&format!("{part}/{path}")) {
                continue;
            }
            match seen_occurrences.get(&(part.clone(), path.clone(), founder.clone())) {
                Some((p, ours)) if ours.hash == t.hash && p == pose => {}
                // Only moved: all of it where it was.
                Some((_, ours)) if ours.hash == t.hash => ghost(&mut g, t, pose, |_| false),
                Some((_, ours)) => {
                    let gone = gone_faces(t, ours);
                    ghost(&mut g, t, pose, |n| gone.contains(n));
                }
                None => ghost(&mut g, t, pose, |_| true),
            }
        }
    }
    if ask.origin {
        g.add_origin();
    }
    let files = session.snapshot();
    let none = BTreeMap::new();
    // The annotation `render --annotation` names is drawn once, by its
    // own block below; the others `--annotations` asks for keep their
    // numbers.
    let asked = ask.annotation.as_ref().map(|a| &a.id);
    let numbers = crate::annotation::numbers(&files);
    if ask.annotations {
        for part in &parts {
            let on = poses.get(part.as_str()).unwrap_or(&none);
            for (at, n) in crate::annotation::markers(&files, part.as_str(), on, asked) {
                g.add_marker(at, &n);
            }
        }
        let all = crate::annotation::readable(&crate::annotation::read_all(&files));
        for a in all
            .values()
            .filter(|a| a.status.is_open() && Some(&a.id) != asked)
        {
            if let (Some(m), Some(v)) = (&a.marks, &a.view)
                && parts.contains(&m.part)
            {
                let plane = crate::redline::Plane::of(&crate::view::Camera::from(v));
                crate::redline::draw(
                    &mut g,
                    &plane,
                    m,
                    &poses,
                    r::participant(&a.by.participant),
                    1.0,
                );
            }
        }
    }
    if let Some(a) = &ask.annotation {
        // Beside the other open annotations, an open one's pins carry its
        // number, as theirs do; alone, or closed, their place in it.
        let number = numbers.get(&a.id).filter(|_| ask.annotations);
        for (k, pin) in a.pins.iter().enumerate() {
            let label = number.map_or_else(|| (k + 1).to_string(), |n| n.to_string());
            for part in &parts {
                let on = poses.get(part.as_str()).unwrap_or(&none);
                if let Some(at) = crate::annotation::pin_on(pin, part.as_str(), on) {
                    g.add_marker(at, &label);
                }
            }
        }
        if let (Some(m), Some(v)) = (&a.marks, &a.view) {
            let plane = crate::redline::Plane::of(&crate::view::Camera::from(v));
            let alpha = if a.status.is_open() {
                1.0
            } else {
                crate::redline::CLOSED_ALPHA
            };
            crate::redline::draw(
                &mut g,
                &plane,
                m,
                &poses,
                r::participant(&a.by.participant),
                alpha,
            );
        }
    }

    g.sections = ask.sections.clone();
    let fixed = ask.camera_m.clone();
    let camera = match (ask.camera, target, ask.view.as_deref()) {
        _ if fixed.is_some() => fixed.expect("checked"),
        (Some(c), _, _) => c.resolve(session.project().manifest.default_unit)?,
        (None, Some((p, live)), _) => {
            let live =
                live.ok_or_else(|| live::no_live_session("render with a participant's camera"))?;
            live.get(p)?.view.camera.clone().ok_or_else(|| {
                Error::new(Code::InvalidParam, format!("participant {p} has no camera"))
                    .with("participant", p)
            })?
        }
        // A named view of the rendered part, as saved.
        (None, None, Some(v)) if StandardView::named(v).is_none() => {
            let part = match parts.as_slice() {
                [one] => Some(one),
                _ => None,
            };
            crate::views::named(session, part, v)?
        }
        (None, None, view) => {
            let view = view
                .map(StandardView::parse)
                .transpose()?
                .unwrap_or(StandardView::Iso);
            let points: Vec<[f64; 3]> = g.positions.iter().chain(&extra).copied().collect();
            if points.is_empty() {
                crate::view::frame([-0.5; 3], [0.5; 3], view, DEFAULT_PROJECTION)
            } else {
                let aspect = f64::from(ask.width) / f64::from(ask.height);
                crate::view::frame_points(&points, view, DEFAULT_PROJECTION, aspect)
            }
        }
    };
    Ok(Scene {
        g,
        camera,
        poses,
        commit,
        compare: other.map(|(rev, (commit, _))| json!({"rev": rev, "commit": commit})),
        warnings,
    })
}

/// `render --flat`: `solid`'s flat pattern (`""`: the part's one sheet
/// metal solid) as `flat.get` gives it, its flat body drawn as any body
/// and seen along the pattern's `z` with its `y` up, its bend lines dashed
/// over it in [`crate::render::BEND_UP`] or `BEND_DOWN`, labelled with
/// their notes when `labels`.
pub(crate) fn render_flat(session: &mut Session, ask: RenderAsk, solid: String) -> Result<Answer> {
    use crate::render::{self as r, Geometry, Label, Line, Options};
    for (param, given) in [
        ("compare", ask.compare.is_some()),
        ("frame", !ask.frame.is_empty()),
        ("view", ask.view.is_some()),
    ] {
        if given {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{param} doesn't apply to a flat pattern's render"),
            )
            .with("param", param));
        }
    }
    let part = match render_parts(session.at(ask.rev.as_deref())?.1, ask.part.as_deref())?
        .as_slice()
    {
        [one] => one.to_string(),
        _ => {
            return Err(
                Error::new(Code::InvalidParam, "a flat pattern is one part's: name it")
                    .with("param", "part"),
            );
        }
    };
    let named = (!solid.is_empty()).then_some(solid.as_str());
    let (commit, f) = crate::sheet_metal::flat::of(
        session,
        &part,
        named,
        ask.rev.as_deref(),
        &Default::default(),
    )?;
    let p = &f.pattern;
    let meshed = flat_meshed(&f);
    let mut g = Geometry::default();
    draw_body(
        &mut g,
        &meshed,
        &ryukei::geom::Rigid::IDENTITY,
        &format!("{}/{}", f.part, f.solid),
        &BTreeMap::new(),
        (None, &BTreeMap::new(), None),
        (false, ask.edges),
    );
    let fr = &p.frame;
    let lift = |q: ryukei::geom::Point2| {
        let w = fr.origin() + fr.x().get() * q.x + fr.y().get() * q.y;
        [w.x, w.y, w.z]
    };
    for l in &p.bend_lines {
        let at = g.positions.len() as u32;
        g.positions.push(lift(l.start));
        g.positions.push(lift(l.end));
        let color = match l.direction {
            ryukei::sheet_metal::BendDirection::Up => r::BEND_UP,
            ryukei::sheet_metal::BendDirection::Down => r::BEND_DOWN,
        };
        g.lines.push(Line {
            points: vec![at, at + 1],
            color,
            width: 2.0,
            dash: true,
        });
        if ask.labels {
            let mid =
                ryukei::geom::Point2::new(0.5 * (l.start.x + l.end.x), 0.5 * (l.start.y + l.end.y));
            g.labels.push(Label {
                at: lift(mid),
                text: ryuformats::dxf::note(l, ryuformats::dxf::DxfUnit::Mm, "°"),
            });
        }
    }
    let camera = match ask.camera {
        Some(c) => c.resolve(session.project().manifest.default_unit)?,
        None => {
            let z = fr.z().get();
            let y = fr.y().get();
            let aspect = f64::from(ask.width) / f64::from(ask.height);
            crate::view::frame_points_along(
                &g.positions,
                [z.x, z.y, z.z],
                [y.x, y.y, y.z],
                DEFAULT_PROJECTION,
                aspect,
            )
        }
    };
    let img = r::render(
        &g,
        &camera,
        ask.width,
        ask.height,
        Options {
            silhouettes: ask.edges,
            triad: true,
        },
    )?;
    let mut result = json!({
        "part": f.part, "flat": f.solid, "width": ask.width, "height": ask.height, "camera": camera,
        "from": match &f.from { crate::sheet_metal::flat::From::Whole => "whole", crate::sheet_metal::flat::From::Unfold(_) => "unfold" },
    });
    if let Some(c) = commit {
        result["commit"] = json!(c);
    }
    Ok(Answer::bytes(result, "image/png", img.png()))
}

/// A flat's display mesh, at normal quality for the pattern's own size,
/// made the first time anything asks and kept with the flat: every name
/// the shown body's, so a pick on it is a pick on the solid.
pub(crate) fn flat_meshed(f: &crate::sheet_metal::flat::FlatOf) -> mesh::Meshed {
    let p = &f.pattern;
    let size = (p.bounds.max() - p.bounds.min()).length().max(1e-3);
    let mesh = f
        .mesh
        .get_or_init(|| {
            let params = crate::mesh::Quality::Normal.params(size);
            std::sync::Arc::new(crate::mesh::DisplayMesh {
                mesh: ryukei::mesh::tessellate(&f.body.body, &params)
                    .map(std::sync::Arc::new)
                    .map_err(|e| Error::new(Code::Internal, format!("meshing the flat: {e:?}"))),
                params,
            })
        })
        .clone();
    mesh::Meshed {
        body: f.body.body.clone(),
        hash: f.body.hash.clone(),
        mesh,
        warning: None,
    }
}

/// Draws one body into a render's geometry where `pose` puts it: its
/// faces in their marks' or paint's colours, its edges, and its marked
/// vertices, labelled when asked. Marks are keyed by `prefix/<name>`.
pub(crate) fn draw_body(
    g: &mut crate::render::Geometry,
    m: &mesh::Meshed,
    pose: &ryukei::geom::Rigid,
    prefix: &str,
    marks: &BTreeMap<String, (crate::render::Rgb, Option<String>)>,
    (whole, faces, look): (
        Option<crate::render::Rgb>,
        &BTreeMap<String, crate::render::Rgb>,
        Option<&crate::appearance::SolidLook>,
    ),
    (labels, edges): (bool, bool),
) {
    use crate::render::{self as r, Label, Line, Triangle};
    let Ok(mesh) = &m.mesh.mesh else { return };
    // A part's own bodies stay exactly as meshed.
    let moved = *pose != ryukei::geom::Rigid::IDENTITY;
    let positions: Vec<ryukei::geom::Point3> = mesh
        .positions
        .iter()
        .map(|p| if moved { pose.apply_point(*p) } else { *p })
        .collect();
    let base = g.positions.len() as u32;
    g.positions
        .extend(positions.iter().map(|p| [p.x, p.y, p.z]));
    let key = |name: String| format!("{prefix}/{name}");
    let label =
        |g: &mut crate::render::Geometry, name: &str, text: &Option<String>, pts: &[u32]| {
            if !labels || pts.is_empty() {
                return;
            }
            let n = pts.len() as f64;
            let mut at = [0.0; 3];
            for p in pts {
                let q = positions[*p as usize];
                at = [at[0] + q.x / n, at[1] + q.y / n, at[2] + q.z / n];
            }
            let text = text.clone().unwrap_or_else(|| name[..8].to_owned());
            g.labels.push(Label { at, text });
        };
    // Translucent faces by ID, with their opacities: their edges fade
    // with them.
    let mut glass: BTreeMap<ryukei::topo::FaceId, f32> = BTreeMap::new();
    // A sheet's faces are drawn from both sides (ryucad 18).
    let sheet = m.body.kind() == ryukei::topo::BodyKind::Sheet;
    for f in &mesh.faces {
        let name = m.body.face(f.face).expect("live").name.to_string();
        let mark = marks.get(&key(name.clone()));
        // Highlights over the compare's paint, as in the app, and both
        // over the appearance, opaque.
        let painted = mark
            .map(|(c, _)| *c)
            .or(whole)
            .or_else(|| faces.get(&name).copied());
        let seen = look.map(|l| l.face(&name));
        let color = painted.unwrap_or_else(|| seen.map_or(r::SURFACE, |e| e.rgb(r::SURFACE)));
        let alpha = match (painted, seen) {
            (None, Some(e)) if e.opacity < 1.0 => Some(e.opacity as f32),
            _ => None,
        };
        if let Some(a) = alpha {
            glass.insert(f.face, a);
        }
        let (start, sheet_start) = (g.triangles.len(), g.sheets.len());
        for t in &mesh.triangles[f.triangles.start as usize..f.triangles.end as usize] {
            let c = t.map(|i| mesh.corners[i as usize]);
            let t = Triangle {
                p: c.map(|c| base + c.position),
                n: c.map(|c| {
                    let n = c.normal.get();
                    let n = if moved { pose.apply_vector(n) } else { n };
                    [n.x as f32, n.y as f32, n.z as f32]
                }),
                color,
            };
            match alpha {
                Some(a) => g.translucent.push((t, a)),
                None if sheet => g.sheets.push(t),
                None => g.triangles.push(t),
            }
        }
        if let Some(t) = g.tags.as_mut() {
            let id = t.name(&key(name.clone()));
            if g.triangles.len() > start {
                t.triangles.push((start..g.triangles.len(), id));
            }
            if g.sheets.len() > sheet_start {
                t.sheets.push((sheet_start..g.sheets.len(), id));
            }
        }
        if let Some((_, text)) = mark {
            let pts: Vec<u32> = mesh.corners[f.corners.start as usize..f.corners.end as usize]
                .iter()
                .map(|c| c.position)
                .collect();
            label(g, &name, text, &pts);
        }
    }
    for e in &mesh.edges {
        let name = m.body.edge(e.edge).expect("live").name.to_string();
        let points: Vec<u32> = e.positions.iter().map(|p| base + p).collect();
        match marks.get(&key(name.clone())) {
            Some((color, text)) => {
                label(g, &name, text, &e.positions);
                g.lines.push(Line {
                    points,
                    color: *color,
                    width: 3.0,
                    dash: false,
                });
                if let Some(t) = g.tags.as_mut() {
                    let id = t.name(&key(name.clone()));
                    t.lines.push((g.lines.len() - 1, id));
                }
            }
            None if edges => {
                let line = Line {
                    points,
                    color: r::EDGE,
                    width: 1.0,
                    dash: false,
                };
                // As opaque as the most opaque face it bounds.
                let alpha = m
                    .body
                    .edge_fins(e.edge)
                    .filter_map(|fin| m.body.loop_(m.body.fin(fin)?.loop_))
                    .map(|l| glass.get(&l.face).copied().unwrap_or(1.0))
                    .fold(0.0f32, f32::max);
                if alpha > 0.0 && alpha < 1.0 {
                    g.translucent_lines.push((line, alpha));
                } else {
                    g.lines.push(line);
                    if let Some(t) = g.tags.as_mut() {
                        let id = t.name(&key(name.clone()));
                        t.lines.push((g.lines.len() - 1, id));
                    }
                }
            }
            None => {}
        }
    }
    for (i, v) in m.body.vertices().enumerate() {
        let name = m.body.vertex(v).expect("live").name.to_string();
        if let Some(t) = g.tags.as_mut() {
            let id = t.name(&key(name.clone()));
            t.points.push((base + i as u32, id));
        }
        if let Some((color, text)) = marks.get(&key(name.clone())) {
            label(g, &name, text, &[i as u32]);
            g.lines.push(Line {
                points: vec![base + i as u32],
                color: *color,
                width: 7.0,
                dash: false,
            });
        }
    }
}

/// The box of `positions`; an empty one at the origin when there are none.
pub(crate) fn bounds(positions: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if positions.is_empty() {
        ([0.0; 3], [0.0; 3])
    } else {
        (lo, hi)
    }
}

/// Draws `part`'s sketches into `g`: curves in their solve states'
/// colours (construction dashed), points as dots, highlighted ones in
/// their marks' colours, and with `labels` each dimension's label.
pub(crate) fn sketches(
    session: &mut Session,
    part: &Id,
    marks: &BTreeMap<String, (crate::render::Rgb, Option<String>)>,
    labels: bool,
    g: &mut crate::render::Geometry,
) -> Result<()> {
    use crate::render::{self as r, Label, Line};
    let unit = session.project().manifest.default_unit;
    let built = session.rebuilt(part)?;
    for (fid, out) in &built.features {
        let Some(sk) = out.as_ref().ok().and_then(|o| o.sketch()) else {
            continue;
        };
        let d = crate::sketch_view::drawing(sk, mesh::Quality::Normal.angle(), unit);
        let mark = |e: &Id| marks.get(&format!("{part}/{fid}/{e}")).map(|m| m.0);
        for c in &d.curves {
            let base = g.positions.len() as u32;
            g.positions.extend(&c.points);
            let (color, width) = match mark(&c.entity) {
                Some(m) => (m, 3.0),
                None => (r::sketch_color(c.state), 1.5),
            };
            g.lines.push(Line {
                points: (base..g.positions.len() as u32).collect(),
                color,
                width,
                dash: c.construction,
            });
            if let Some(t) = g.tags.as_mut() {
                let id = t.name(&format!("{part}/{fid}/{}", c.entity));
                t.lines.push((g.lines.len() - 1, id));
            }
        }
        for p in &d.dots {
            g.positions.push(p.at);
            let (color, width) = match mark(&p.entity) {
                Some(m) => (m, 7.0),
                None => (r::sketch_color(p.state), 4.0),
            };
            g.lines.push(Line {
                points: vec![g.positions.len() as u32 - 1],
                color,
                width,
                dash: false,
            });
            if let Some(t) = g.tags.as_mut() {
                let id = t.name(&format!("{part}/{fid}/{}", p.entity));
                t.points.push((g.positions.len() as u32 - 1, id));
            }
        }
        if labels {
            g.labels.extend(d.labels.into_iter().map(|l| Label {
                at: l.at,
                text: l.text,
            }));
        }
    }
    Ok(())
}
