//! Drawings (design 417, plan 9.A): a drawing file built against the
//! model into sheets of [`ryu_draw::Item`]s in sheet millimetres.
//!
//! A drawing is three layers, each a pure function of the one before:
//! the file (`doc::drawing`), the built drawing ([`Built`], here), and
//! the writers (`ryu-draw`'s PDF and SVG, PNG from the SVG). Every value
//! a sheet shows that the model decides is worked out again from the
//! model at the revision asked for, never stored.
//!
//! Each view's projection comes from `ryukei::project::view_with`, under
//! the request's cancel, side by side across views, kept by every placed
//! body's hash, every pose's bits and the view's bits in the session and
//! on disk (`projections`, `ryukei.projection.1`).

pub mod curves;
pub mod output;
pub mod template;

use crate::doc::assembly::Rev;
use crate::doc::drawing::{
    Angle, DrawingFile, Hidden, Key, Orient, Orientation, Scale, Side, Size, Standard, Tangent,
    ViewKind,
};
use crate::doc::drawing_template::Template;
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use ryu_draw::{Anchor, Colour, Item, LineKind, P, Path, Segment};
use ryukei::geom::Vector3;
use ryukei::project::{Class, Placed, Projection, Visibility};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Room a view's box is grown by on every side for its dimensions, in
/// sheet millimetres (design 417).
pub const DIM_ROOM: f64 = 12.0;

/// The standard scales a sheet with none takes, largest first: the
/// largest at which its views fit.
pub const SCALES: [(u32, u32); 13] = [
    (10, 1),
    (5, 1),
    (2, 1),
    (1, 1),
    (1, 2),
    (1, 5),
    (1, 10),
    (1, 20),
    (1, 50),
    (1, 100),
    (1, 200),
    (1, 500),
    (1, 1000),
];

/// The colour drawings draw what isn't right in: a reference that doesn't
/// resolve, a view that failed.
pub const WARNING: Colour = Colour(0xc0, 0x40, 0x00);

/// Projections by their key, kept by the session (design 417).
#[derive(Clone, Debug, Default)]
pub struct Cache {
    kept: BTreeMap<String, Arc<Projection>>,
}

impl Cache {
    /// At most this many are kept; past it the cache starts again.
    pub const MAX: usize = 256;

    pub fn len(&self) -> usize {
        self.kept.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }

    fn get(&self, key: &str) -> Option<Arc<Projection>> {
        self.kept.get(key).cloned()
    }

    fn put(&mut self, key: String, p: Arc<Projection>) {
        if self.kept.len() >= Self::MAX {
            self.kept.clear();
        }
        self.kept.insert(key, p);
    }
}

/// What one view drew.
#[derive(Clone, Debug)]
pub struct ViewBuilt {
    pub key: Key,
    pub sheet: Key,
    pub kind: &'static str,
    /// Its centre on the sheet, and whether ryucad placed it (no `at`, or
    /// no `offset`, written).
    pub at: P,
    pub placed: bool,
    pub scale: Scale,
    /// Its drawing's box on the sheet (dimension room not included).
    pub bounds: [P; 2],
    pub towards: [f64; 3],
    pub up: [f64; 3],
    /// The part (or assembly) it draws and the revision it reads.
    pub of: Id,
    pub rev: Rev,
    /// Pieces drawn: visible, hidden, uncertain.
    pub counts: [usize; 3],
    pub error: Option<Error>,
}

/// One sheet as drawn.
#[derive(Clone, Debug)]
pub struct SheetBuilt {
    pub key: Key,
    pub name: String,
    pub size: Size,
    pub orientation: Orientation,
    /// Its width and height in millimetres.
    pub paper: P,
    pub template: String,
    /// The scale its views take where they don't say one.
    pub scale: Scale,
    pub items: Vec<Item>,
    pub views: Vec<ViewBuilt>,
}

/// A drawing built against the model.
#[derive(Clone, Debug)]
pub struct Built {
    pub part: Id,
    pub drawing: Id,
    pub name: String,
    pub standard: Standard,
    pub projection: Angle,
    pub sheets: Vec<SheetBuilt>,
    /// `drawing_view_failed` and `drawing_view_uncertain` findings, as
    /// `{code, severity, view, sheet, message, details}`.
    pub findings: Vec<Value>,
}

impl Built {
    /// The sheet `key`, or `not_found` naming the drawing's sheets.
    pub fn sheet(&self, key: Option<&str>) -> Result<&SheetBuilt> {
        let Some(k) = key else {
            return self
                .sheets
                .first()
                .ok_or_else(|| Error::new(Code::NotFound, "the drawing has no sheet"));
        };
        self.sheets
            .iter()
            .find(|s| s.key.to_string() == k)
            .ok_or_else(|| {
                Error::new(
                    Code::NotFound,
                    format!("drawing {} has no sheet {k}", self.drawing),
                )
                .with(
                    "sheets",
                    self.sheets
                        .iter()
                        .map(|s| s.key.to_string())
                        .collect::<Vec<_>>(),
                )
            })
    }

    /// Where ryucad placed the views no `at` (or `offset`) placed, as
    /// `drawing.edit` writes them back: by view, `at` in sheet
    /// millimetres, or a projected or auxiliary view's `offset`.
    pub fn placements(&self, file: &DrawingFile) -> BTreeMap<Key, Placement> {
        let by_key: BTreeMap<Key, &ViewBuilt> = self
            .sheets
            .iter()
            .flat_map(|s| &s.views)
            .map(|v| (v.key, v))
            .collect();
        let mut out = BTreeMap::new();
        for (k, w) in &file.views {
            let Some(v) = by_key.get(k).filter(|v| v.placed) else {
                continue;
            };
            match (&w.kind, w.parent().and_then(|p| by_key.get(&p))) {
                (ViewKind::Projected { side, .. }, Some(p)) if !matches!(side, Side::Diagonal(_)) => {
                    out.insert(*k, Placement::Offset(distance(p.at, v.at)));
                }
                (ViewKind::Auxiliary { .. }, Some(p)) => {
                    out.insert(*k, Placement::Offset(distance(p.at, v.at)));
                }
                _ => {
                    out.insert(*k, Placement::At(v.at));
                }
            }
        }
        out
    }
}

/// Where ryucad placed a view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    At(P),
    Offset(f64),
}

fn distance(a: P, b: P) -> f64 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}

fn add3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale3(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn v3(a: [f64; 3]) -> Vector3 {
    Vector3::new(a[0], a[1], a[2])
}

fn a3(v: Vector3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// A view's frame: from the model towards the eye, image right and up.
#[derive(Clone, Copy, Debug)]
struct Frame {
    towards: [f64; 3],
    x: [f64; 3],
    y: [f64; 3],
}

impl Frame {
    fn new(towards: [f64; 3], up: [f64; 3]) -> Result<Frame> {
        let v = ryukei::project::View::new(v3(towards), v3(up)).map_err(|e| {
            Error::new(
                Code::InvalidParam,
                format!("a view can't look along {towards:?} with {up:?} up: {e}"),
            )
        })?;
        Ok(Frame {
            towards: a3(v.towards().get()),
            x: a3(v.x().get()),
            y: a3(v.y().get()),
        })
    }

    fn view(&self) -> ryukei::project::View {
        ryukei::project::View::new(v3(self.towards), v3(self.y)).expect("made by `new`")
    }

    fn image(&self, p: [f64; 3]) -> P {
        let d = |a: [f64; 3]| a[0] * p[0] + a[1] * p[1] + a[2] * p[2];
        [d(self.x), d(self.y)]
    }

    /// The view `side` of this one shows, by the frame alone (the
    /// projection angle only decides where it is placed).
    fn side(&self, side: &Side) -> Result<Frame> {
        let neg = |a: [f64; 3]| scale3(a, -1.0);
        let (t, x, y) = (self.towards, self.x, self.y);
        match side {
            Side::Top => Frame::new(y, neg(t)),
            Side::Bottom => Frame::new(neg(y), t),
            Side::Right => Frame::new(x, y),
            Side::Left => Frame::new(neg(x), y),
            Side::Back => Frame::new(neg(t), y),
            Side::Diagonal(d) => {
                let c = crate::view::StandardView::parse(if d == "iso" {
                    "front_top_right"
                } else {
                    d
                })?;
                // The cube's directions are a front view's: x right, z up,
                // -y towards the eye; here they are this view's.
                let (back, _) = c.axes();
                let towards = add3(add3(scale3(x, back[0]), scale3(t, -back[1])), scale3(y, back[2]));
                Frame::new(towards, y)
            }
        }
    }
}

/// Where a view goes on its sheet.
#[derive(Clone, Copy, Debug)]
enum Place {
    /// At a centre written, or one ryucad chooses.
    At(Option<P>),
    /// Along `dir` (a unit on the sheet) from its parent, by a distance
    /// written or one ryucad chooses.
    Along { parent: Key, dir: P, offset: Option<f64> },
}

/// What a view is to draw, before its projection.
struct Plan {
    key: Key,
    sheet: Key,
    kind: &'static str,
    of: Id,
    rev: Rev,
    frame: Frame,
    /// The model point every view of its family keeps at its centre's
    /// place, so projected views line up with their parent exactly.
    anchor: [f64; 3],
    scale: Option<Scale>,
    hidden: Hidden,
    tangent: Tangent,
    label: Option<String>,
    place: Place,
}

/// A source's placed solids: each body, where it is, and how references
/// name its entities (`<part>/<solid>`, or `<asm>/<path>/<solid>`).
type Source = Vec<(Arc<crate::rebuild::Built>, ryukei::geom::Rigid, String)>;

fn source(session: &mut Session, of: &Id, rev: &Rev) -> Result<Source> {
    let model = session.model(of, rev)?;
    Ok(model
        .occurrences
        .iter()
        .flat_map(|o| {
            let path: Vec<String> = std::iter::once(of.to_string())
                .chain(o.path.iter().map(Id::to_string))
                .collect();
            o.solids.iter().map(move |s| {
                (s.built.clone(), o.pose, format!("{}/{}", path.join("/"), s.founder))
            })
        })
        .collect())
}

/// The middle of the box round a source's solids (the origin when it has
/// none).
fn middle(src: &Source) -> [f64; 3] {
    let b = src
        .iter()
        .filter_map(|(b, pose, _)| crate::interference::world_box(&b.body, pose))
        .reduce(|a, b| a.union(&b));
    match b {
        Some(b) => {
            let (lo, hi) = (b.min(), b.max());
            [(lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0, (lo.z + hi.z) / 2.0]
        }
        None => [0.0; 3],
    }
}

/// The image box (model metres, relative to the anchor's image) of a
/// source's solids' boxes: a failed view's size.
fn box_image(src: &Source, frame: &Frame, anchor: [f64; 3]) -> Option<[P; 2]> {
    let c = frame.image(anchor);
    let mut out: Option<[P; 2]> = None;
    for (b, pose, _) in src {
        let Some(bx) = crate::interference::world_box(&b.body, pose) else {
            continue;
        };
        for q in bx.corners() {
            let i = frame.image([q.x, q.y, q.z]);
            let i = [i[0] - c[0], i[1] - c[1]];
            out = Some(match out {
                None => [i, i],
                Some([lo, hi]) => [
                    [lo[0].min(i[0]), lo[1].min(i[1])],
                    [hi[0].max(i[0]), hi[1].max(i[1])],
                ],
            });
        }
    }
    out
}

/// A projection's key: every placed body's hash and pose's bits, and the
/// view's.
fn key_of(src: &Source, frame: &Frame) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"ryucad.projection.1\0");
    for (b, pose, _) in src {
        h.update(b.hash.as_bytes());
        h.update(&ryukei::codec::encode_rigid(pose));
    }
    for a in [frame.towards, frame.y] {
        for c in a {
            h.update(&c.to_bits().to_le_bytes());
        }
    }
    h.finalize().to_hex().to_string()
}

/// The direction `towards` and `up` a base view looks with.
fn orient(session: &mut Session, part: &Id, of: &Id, o: &Orient) -> Result<([f64; 3], [f64; 3])> {
    match o {
        Orient::Named(n) => {
            if let Ok(v) = crate::view::StandardView::parse(n) {
                return Ok(v.axes());
            }
            let id = Id::parse(n, "view")?;
            let p = session.project().part(of)?;
            match p.views.get(&id) {
                Some(Ok(v)) => Ok((
                    [
                        v.eye_m[0] - v.target_m[0],
                        v.eye_m[1] - v.target_m[1],
                        v.eye_m[2] - v.target_m[2],
                    ],
                    v.up,
                )),
                Some(Err(e)) => Err(e.clone()),
                None => Err(Error::new(
                    Code::ViewNotFound,
                    format!("{of} has no named view {n}"),
                )
                .with("part", of)
                .with("views", p.views.keys().collect::<Vec<_>>())),
            }
        }
        Orient::NormalTo(r) => crate::server::normal_to_face(
            session,
            &format!("{}/{}/{}", part, r.feature, r.name),
        ),
    }
}

/// The plans of `file`'s views, parents before their children.
fn plans(session: &mut Session, part: &Id, file: &DrawingFile, angle: Angle) -> Result<Vec<(Plan, Source)>> {
    let mut done: BTreeMap<Key, usize> = BTreeMap::new();
    let mut out: Vec<(Plan, Source)> = Vec::new();
    let mut sources: BTreeMap<(Id, String), Source> = BTreeMap::new();
    while done.len() < file.views.len() {
        let before = done.len();
        for (k, w) in &file.views {
            if done.contains_key(k) {
                continue;
            }
            let parent = match w.parent() {
                Some(p) => match done.get(&p) {
                    Some(i) => Some(*i),
                    None => continue,
                },
                None => None,
            };
            let at = |w: &crate::doc::drawing::View| {
                w.at.as_ref()
                    .map(|[x, y]| [x.meters() * 1000.0, y.meters() * 1000.0])
            };
            let mm = |q: &Option<crate::units::Quantity>| q.as_ref().map(|q| q.meters() * 1000.0);
            let (of, rev, frame, anchor, place, kind) = match (&w.kind, parent) {
                (ViewKind::Base { of, label, orient: o }, _) => {
                    let of = of.clone().unwrap_or_else(|| part.clone());
                    let rev = label.clone().map_or(Rev::Working, Rev::Label);
                    let key = (of.clone(), format!("{rev:?}"));
                    if !sources.contains_key(&key) {
                        let s = source(session, &of, &rev)?;
                        sources.insert(key.clone(), s);
                    }
                    let anchor = middle(&sources[&key]);
                    let (t, u) = orient(session, part, &of, o)?;
                    (of, rev, Frame::new(t, u)?, anchor, Place::At(at(w)), "base")
                }
                (ViewKind::Projected { side, offset, .. }, Some(i)) => {
                    let p = &out[i].0;
                    let frame = p.frame.side(side)?;
                    let flip = if angle == Angle::First { -1.0 } else { 1.0 };
                    let dir = match side {
                        Side::Top => Some([0.0, flip]),
                        Side::Bottom => Some([0.0, -flip]),
                        Side::Right | Side::Back => Some([flip, 0.0]),
                        Side::Left => Some([-flip, 0.0]),
                        Side::Diagonal(_) => None,
                    };
                    let place = match dir {
                        Some(dir) => Place::Along {
                            parent: p.key,
                            dir,
                            offset: mm(offset),
                        },
                        None => Place::At(at(w)),
                    };
                    (p.of.clone(), p.rev.clone(), frame, p.anchor, place, "projected")
                }
                (ViewKind::Auxiliary { normal_to, offset, .. }, Some(i)) => {
                    let p = &out[i].0;
                    let text = format!("{}/{}/{}", p.of, normal_to.feature, normal_to.name);
                    let (n, _) = crate::server::normal_to_face(session, &text)?;
                    // Along the normal's image in the parent: it must lie
                    // in the parent's plane, or the view is foreshortened.
                    let img = p.frame.image(n);
                    let len = (img[0] * img[0] + img[1] * img[1]).sqrt();
                    let along = n[0] * p.frame.towards[0]
                        + n[1] * p.frame.towards[1]
                        + n[2] * p.frame.towards[2];
                    if len < 1e-9 || along.abs() > 1e-9 {
                        return Err(Error::new(
                            Code::InvalidParam,
                            format!(
                                "view {k} looks normal to a face its parent {} doesn't see edge-on",
                                p.key
                            ),
                        )
                        .with("pointer", format!("/views/{k}/normal_to"))
                        .with("reason", "foreshortened"));
                    }
                    let frame = Frame::new(n, scale3(p.frame.towards, -1.0))?;
                    let flip = if angle == Angle::First { -1.0 } else { 1.0 };
                    let place = Place::Along {
                        parent: p.key,
                        dir: [flip * img[0] / len, flip * img[1] / len],
                        offset: mm(offset),
                    };
                    (p.of.clone(), p.rev.clone(), frame, p.anchor, place, "auxiliary")
                }
                (_, None) => unreachable!("a derived view waits for its parent"),
            };
            let key = (of.clone(), format!("{rev:?}"));
            if !sources.contains_key(&key) {
                let s = source(session, &of, &rev)?;
                sources.insert(key.clone(), s);
            }
            let scale = w
                .scale
                .or_else(|| parent.and_then(|i| out[i].0.scale.filter(|_| w.scale.is_none())));
            done.insert(*k, out.len());
            out.push((
                Plan {
                    key: *k,
                    sheet: w.sheet,
                    kind,
                    of,
                    rev,
                    frame,
                    anchor,
                    scale,
                    hidden: w.hidden,
                    tangent: w.tangent,
                    label: w.label.clone(),
                    place,
                },
                sources[&key].clone(),
            ));
        }
        if done.len() == before {
            // `DrawingFile::check` refuses cycles; nothing is left waiting.
            break;
        }
    }
    Ok(out)
}

/// What one view drew in its own frame: paths in model metres relative to
/// its anchor's image, each with the kind of line it is.
struct Drawn {
    lines: Vec<(Path, LineKind, bool)>,
    /// Its box, in the same frame.
    bounds: Option<[P; 2]>,
    counts: [usize; 3],
    uncertain: Vec<String>,
}

/// The kind of line a piece draws as, if it is drawn.
fn line_of(p: &ryukei::project::Piece, plan: &Plan) -> Option<(LineKind, bool)> {
    let tangent = p.class == Class::Smooth && !p.on_silhouette;
    let (kind, uncertain) = match p.visibility {
        Visibility::Visible => (LineKind::Visible, false),
        Visibility::Uncertain => (LineKind::Visible, true),
        Visibility::Hidden(_) => match plan.hidden {
            Hidden::Off => return None,
            Hidden::Dashed => (LineKind::Hidden, false),
        },
    };
    if tangent && kind == LineKind::Visible {
        return match plan.tangent {
            Tangent::Off => None,
            Tangent::Thin => Some((LineKind::Thin, uncertain)),
            Tangent::Phantom => Some((LineKind::Phantom, uncertain)),
        };
    }
    if tangent && plan.tangent == Tangent::Off {
        return None;
    }
    Some((kind, uncertain))
}

/// The projection's pieces as paths: each piece drawn only where it is
/// the nearest of those it runs along, and runs of one source's pieces
/// joined end to end into one path.
fn drawn(p: &Projection, plan: &Plan, src: &Source) -> Drawn {
    let c = plan.frame.image(plan.anchor);
    let mut lines: Vec<(Path, LineKind, bool)> = Vec::new();
    let mut counts = [0; 3];
    let mut uncertain = Vec::new();
    let mut last: Option<(ryukei::project::Source, LineKind)> = None;
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for (i, piece) in p.pieces.iter().enumerate() {
        if let Some(first) = piece.overlaps.first()
            && let Some(o) = p.pieces.get(*first as usize)
            && (o.depth[1] > piece.depth[1] || (o.depth[1] == piece.depth[1] && (*first as usize) < i))
        {
            continue;
        }
        let Some((kind, unsure)) = line_of(piece, plan) else {
            continue;
        };
        let Some(path) = curves::path(&piece.image) else {
            continue;
        };
        let path = path.moved([-c[0], -c[1]]);
        let [a, b] = path.bounds();
        lo = [lo[0].min(a[0]), lo[1].min(a[1])];
        hi = [hi[0].max(b[0]), hi[1].max(b[1])];
        counts[match (kind, unsure) {
            (_, true) => 2,
            (LineKind::Hidden, _) => 1,
            _ => 0,
        }] += 1;
        if unsure {
            let (body, what, name) = match piece.source {
                ryukei::project::Source::Edge { body, name, .. } => (body, "edge", name),
                ryukei::project::Source::Silhouette { body, face_name, .. } => (body, "face", face_name),
            };
            let solid = src.get(body as usize).map_or("", |(_, _, r)| r.as_str());
            uncertain.push(format!("{what} {solid}/{name}"));
        }
        // One source's next piece, drawn alike, from where the last ended:
        // the same path.
        let same = |s: ryukei::project::Source| match (last, s) {
            (Some((a, k)), b) => a == b && k == kind,
            _ => false,
        };
        if same(piece.source)
            && let Some((prev, _, u)) = lines.last_mut()
            && distance(prev.segments.last().map_or(prev.start, Segment::end), path.start) < 1e-9
        {
            join(prev, path);
            *u |= unsure;
        } else {
            lines.push((path, kind, unsure));
        }
        last = Some((piece.source, kind));
    }
    Drawn {
        lines,
        bounds: (lo[0] <= hi[0]).then_some([lo, hi]),
        counts,
        uncertain,
    }
}

/// `next` added to `path`, a line running on along a line made one.
fn join(path: &mut Path, next: Path) {
    for s in next.segments {
        if let (Segment::Line(b), Some(Segment::Line(a))) = (&s, path.segments.last()) {
            let start = if path.segments.len() >= 2 {
                path.segments[path.segments.len() - 2].end()
            } else {
                path.start
            };
            let (u, v) = ([a[0] - start[0], a[1] - start[1]], [b[0] - a[0], b[1] - a[1]]);
            let cross = u[0] * v[1] - u[1] * v[0];
            let dot = u[0] * v[0] + u[1] * v[1];
            if cross.abs() <= 1e-12 * (ryu_math::hypot(u[0], u[1]) * ryu_math::hypot(v[0], v[1])).max(1e-30) && dot > 0.0 {
                *path.segments.last_mut().expect("checked") = Segment::Line(*b);
                continue;
            }
        }
        path.segments.push(s);
    }
}

/// Projections for `plans`, from the session's cache, the disk's, or the
/// kernel side by side; each failure in its place.
fn project(session: &mut Session, plans: &[(Plan, Source)]) -> Result<Vec<std::result::Result<Arc<Projection>, Error>>> {
    let keys: Vec<String> = plans.iter().map(|(p, s)| key_of(s, &p.frame)).collect();
    let disk = session.disk().cloned();
    let mut found: Vec<Option<Arc<Projection>>> = keys
        .iter()
        .map(|k| {
            session.projections.get(k).or_else(|| {
                let d = disk.as_ref()?;
                let bytes = d.get(crate::cache::Kind::Projection, k.as_bytes())?;
                match ryukei::codec::decode_projection(&bytes) {
                    Ok(p) => Some(Arc::new(p)),
                    Err(_) => {
                        d.discard(crate::cache::Kind::Projection, k.as_bytes());
                        None
                    }
                }
            })
        })
        .collect();
    let job = session.job.clone();
    let stage = job.begin(ryu_session::job::Phase::Project, "views", plans.len());
    use rayon::prelude::*;
    let made: Vec<Option<std::result::Result<Projection, ryukei::Error>>> = plans
        .par_iter()
        .zip(found.par_iter())
        .map(|((plan, src), have)| {
            if have.is_some() {
                return None;
            }
            let placed: Vec<Placed> = src
                .iter()
                .map(|(b, pose, _)| Placed {
                    body: &b.body,
                    place: *pose,
                })
                .collect();
            let work = crate::work::work(&job);
            Some(ryukei::project::view_with(
                &placed,
                &plan.frame.view(),
                &ryukei::project::ViewOptions::default(),
                &work,
            ))
        })
        .collect();
    let mut out = Vec::new();
    for (i, m) in made.into_iter().enumerate() {
        stage.step(|| plans[i].0.key.to_string())?;
        match m {
            None => out.push(Ok(found[i].take().expect("found"))),
            Some(Ok(p)) => {
                let p = Arc::new(p);
                if let Some(d) = &disk {
                    d.put(
                        crate::cache::Kind::Projection,
                        keys[i].as_bytes(),
                        &ryukei::codec::encode_projection(&p),
                    );
                }
                session.projections.put(keys[i].clone(), p.clone());
                out.push(Ok(p));
            }
            Some(Err(e)) => {
                if let Some(c) = crate::work::stopped(&e, &job) {
                    return Err(c);
                }
                out.push(Err(view_failed(&plans[i].0, &e)));
            }
        }
    }
    Ok(out)
}

fn view_failed(plan: &Plan, e: &ryukei::Error) -> Error {
    let reason = match e {
        ryukei::Error::InvalidView(v) => format!("{v:?}"),
        other => format!("{other:?}"),
    };
    let reason: String = reason
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            let lower = c.to_ascii_lowercase();
            if c.is_ascii_uppercase() && i > 0 {
                vec!['_', lower]
            } else {
                vec![lower]
            }
        })
        .collect();
    Error::new(
        Code::DrawingViewFailed,
        format!("view {} couldn't be projected: {e}", plan.key),
    )
    .with("view", plan.key.to_string())
    .with("reason", reason)
    .with("kernel", e.to_string())
}

/// Builds drawing `id` of `part` against the session's model.
pub fn build(session: &mut Session, part: &Id, id: &Id) -> Result<Built> {
    let p = session.project().part(part)?;
    let file = match p.drawings.get(id) {
        Some(Ok(f)) => f.clone(),
        Some(Err(e)) => return Err(e.clone()),
        None => {
            return Err(Error::new(
                Code::DrawingNotFound,
                format!("{part} has no drawing {id}"),
            )
            .with("part", part)
            .with("drawings", p.drawings.keys().collect::<Vec<_>>()));
        }
    };
    file.check()?;
    let standard = file.standard_in(session.project().manifest.standard());
    let angle = file.projection.unwrap_or_else(|| standard.projection());
    let plans = plans(session, part, &file, angle)?;
    let projected = project(session, &plans)?;
    let style = match standard {
        Standard::Asme => ryu_draw::Standard::Asme,
        Standard::Iso => ryu_draw::Standard::Iso,
    };
    let mut findings = Vec::new();
    // Each view in its own frame, or why it failed.
    let mut drawings: Vec<std::result::Result<Drawn, Error>> = Vec::new();
    for ((plan, src), p) in plans.iter().zip(&projected) {
        match p {
            Ok(p) => {
                let d = drawn(p, plan, src);
                if !d.uncertain.is_empty() {
                    findings.push(json!({
                        "code": "drawing_view_uncertain", "severity": "warning",
                        "view": plan.key.to_string(), "sheet": plan.sheet.to_string(),
                        "message": format!(
                            "view {}: {} pieces' visibility couldn't be certified; drawn visible",
                            plan.key, d.uncertain.len()
                        ),
                        "details": {"pieces": d.uncertain.len(), "sources": d.uncertain},
                    }));
                }
                drawings.push(Ok(d));
            }
            Err(e) => {
                findings.push(json!({
                    "code": "drawing_view_failed", "severity": "error",
                    "view": plan.key.to_string(), "sheet": plan.sheet.to_string(),
                    "message": e.message, "details": e.details,
                }));
                drawings.push(Err(e.clone()));
            }
        }
    }
    // Each view's box at a scale of one: its drawing's, else its solids'.
    let unit_boxes: Vec<[P; 2]> = plans
        .iter()
        .zip(&drawings)
        .map(|((plan, src), d)| {
            let b = match d {
                Ok(d) => d.bounds,
                Err(_) => None,
            };
            b.or_else(|| box_image(src, &plan.frame, plan.anchor))
                .unwrap_or([[-0.01, -0.01], [0.01, 0.01]])
        })
        .collect();
    let project_templates = session.project().templates.clone();
    let mut sheets = Vec::new();
    let mut count = 0;
    for (sk, sheet) in &file.sheets {
        count += 1;
        let tid = sheet.template.clone().unwrap_or_else(|| sheet.size.template());
        let t: Template = template::resolve(
            &tid,
            sheet.size,
            sheet.orientation,
            project_templates.get(&tid).map(|f| &f.template),
        )?;
        let on: Vec<usize> = (0..plans.len()).filter(|i| plans[*i].0.sheet == *sk).collect();
        let scales = |s: Scale| -> Vec<Scale> {
            on.iter()
                .map(|i| plans[*i].0.scale.unwrap_or(s))
                .collect()
        };
        let fixed = sheet.scale;
        let chosen = match fixed {
            Some(s) => s,
            None => SCALES
                .iter()
                .map(|(a, b)| Scale { sheet: *a, model: *b })
                .find(|s| {
                    let sc = scales(*s);
                    let placed = layout(&plans, &unit_boxes, &on, &sc, &t);
                    fits(&grown_at(&unit_boxes, &on, &sc, &placed), &t)
                })
                .unwrap_or(Scale { sheet: 1, model: 1000 }),
        };
        let views_scale = scales(chosen);
        let placed = layout(&plans, &unit_boxes, &on, &views_scale, &t);
        let mut items = template_items(session, part, &file, &t, sheet, chosen, count, style);
        let mut views = Vec::new();
        for (j, i) in on.iter().enumerate() {
            let plan = &plans[*i].0;
            let s = views_scale[j];
            let k = s.mm_per_m();
            let at = placed[j].0;
            let ub = unit_boxes[*i];
            let bounds = [
                [at[0] + k * ub[0][0], at[1] + k * ub[0][1]],
                [at[0] + k * ub[1][0], at[1] + k * ub[1][1]],
            ];
            let (counts, error) = match &drawings[*i] {
                Ok(d) => {
                    for (path, kind, unsure) in &d.lines {
                        let mut stroke = style.stroke(*kind);
                        if *unsure {
                            stroke.colour = WARNING;
                        }
                        items.push(Item::Path {
                            path: path.scaled(k, at),
                            stroke: Some(stroke),
                            fill: None,
                        });
                    }
                    (d.counts, None)
                }
                Err(e) => {
                    crossed(&mut items, bounds, style);
                    ([0; 3], Some(e.clone()))
                }
            };
            if let Some(l) = &plan.label {
                items.push(Item::Text {
                    text: l.clone(),
                    at: [(bounds[0][0] + bounds[1][0]) / 2.0, bounds[0][1] - 8.0],
                    size: 3.5,
                    angle: 0.0,
                    anchor: Anchor::Middle,
                    colour: Colour::BLACK,
                });
            }
            views.push(ViewBuilt {
                key: plan.key,
                sheet: *sk,
                kind: plan.kind,
                at,
                placed: placed[j].1,
                scale: s,
                bounds,
                towards: plan.frame.towards,
                up: plan.frame.y,
                of: plan.of.clone(),
                rev: plan.rev.clone(),
                counts,
                error,
            });
        }
        sheets.push(SheetBuilt {
            key: *sk,
            name: sheet.name.clone(),
            size: sheet.size,
            orientation: sheet.orientation,
            paper: sheet.size.paper(sheet.orientation),
            template: tid,
            scale: chosen,
            items,
            views,
        });
    }
    Ok(Built {
        part: part.clone(),
        drawing: id.clone(),
        name: file.name.clone(),
        standard,
        projection: angle,
        sheets,
        findings,
    })
}

/// A failed view's frame: its box, crossed corner to corner.
fn crossed(items: &mut Vec<Item>, [lo, hi]: [P; 2], style: ryu_draw::Standard) {
    let mut stroke = style.stroke(LineKind::Thin);
    stroke.colour = WARNING;
    let rect = Path::polyline(&[lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]], true);
    for path in [rect, Path::polyline(&[lo, hi], false), Path::polyline(&[[lo[0], hi[1]], [hi[0], lo[1]]], false)] {
        items.push(Item::Path {
            path,
            stroke: Some(stroke.clone()),
            fill: None,
        });
    }
}

/// Where each of the sheet's views `on` goes at `scales`: its centre on
/// the sheet, and whether ryucad placed it. Written places stand; a base
/// view with none goes with its family (the views placed along it) at the
/// area's upper left, the next family to its right, and a diagonal view
/// with none at the area's upper right.
fn layout(
    plans: &[(Plan, Source)],
    boxes: &[[P; 2]],
    on: &[usize],
    scales: &[Scale],
    t: &Template,
) -> Vec<(P, bool)> {
    let index: BTreeMap<Key, usize> = on.iter().enumerate().map(|(j, i)| (plans[*i].0.key, j)).collect();
    let grown = |j: usize| -> [P; 2] {
        let k = scales[j].mm_per_m();
        let b = boxes[on[j]];
        [
            [k * b[0][0] - DIM_ROOM, k * b[0][1] - DIM_ROOM],
            [k * b[1][0] + DIM_ROOM, k * b[1][1] + DIM_ROOM],
        ]
    };
    let mut at: Vec<Option<(P, bool)>> = vec![None; on.len()];
    // Relative places within each family, from its root at the origin.
    let mut rel: Vec<Option<P>> = vec![None; on.len()];
    let mut root: Vec<usize> = (0..on.len()).collect();
    for (j, i) in on.iter().enumerate() {
        if let Place::Along { parent, .. } = plans[*i].0.place
            && let Some(pj) = index.get(&parent)
        {
            root[j] = root[*pj];
        }
    }
    for (j, i) in on.iter().enumerate() {
        match plans[*i].0.place {
            Place::At(_) => rel[j] = Some([0.0, 0.0]),
            Place::Along { parent, dir, offset } => {
                let Some(pj) = index.get(&parent).copied() else {
                    rel[j] = Some([0.0, 0.0]);
                    continue;
                };
                let base = rel[pj].unwrap_or([0.0, 0.0]);
                let d = offset.unwrap_or_else(|| {
                    // Just clear of the parent, both grown by their room.
                    let (a, b) = (grown(pj), grown(j));
                    let reach = |bx: [P; 2], s: f64| {
                        let along = |q: P| q[0] * dir[0] + q[1] * dir[1];
                        let c = [bx[0], [bx[1][0], bx[0][1]], bx[1], [bx[0][0], bx[1][1]]];
                        c.iter().map(|q| s * along(*q)).fold(f64::NEG_INFINITY, f64::max)
                    };
                    reach(a, 1.0) + reach(b, -1.0)
                });
                rel[j] = Some([base[0] + d * dir[0], base[1] + d * dir[1]]);
            }
        }
    }
    let [lo, hi] = t.area;
    // Families whose root has a written place: placed from it.
    let mut x = lo[0];
    for (j, i) in on.iter().enumerate() {
        if root[j] != j {
            continue;
        }
        let members: Vec<usize> = (0..on.len()).filter(|m| root[*m] == j).collect();
        let fam_box = members
            .iter()
            .map(|m| {
                let r = rel[*m].unwrap_or([0.0, 0.0]);
                let b = grown(*m);
                [[r[0] + b[0][0], r[1] + b[0][1]], [r[0] + b[1][0], r[1] + b[1][1]]]
            })
            .reduce(|a, b| [[a[0][0].min(b[0][0]), a[0][1].min(b[0][1])], [a[1][0].max(b[1][0]), a[1][1].max(b[1][1])]])
            .expect("a family has its root");
        let diagonal = plans[*i].0.kind == "projected";
        let origin = match plans[*i].0.place {
            Place::At(Some(p)) => p,
            _ if diagonal => [hi[0] - fam_box[1][0], hi[1] - fam_box[1][1]],
            _ => {
                let o = [x - fam_box[0][0], hi[1] - fam_box[1][1]];
                x = o[0] + fam_box[1][0];
                o
            }
        };
        for m in members {
            let r = rel[m].unwrap_or([0.0, 0.0]);
            let written = match plans[on[m]].0.place {
                Place::At(p) => p.is_some(),
                Place::Along { offset, .. } => offset.is_some(),
            };
            at[m] = Some(([origin[0] + r[0], origin[1] + r[1]], !written));
        }
    }
    let _ = lo[1];
    at.into_iter().map(|a| a.unwrap_or(([0.0, 0.0], true))).collect()
}

/// Each view's box on the sheet grown by its room, where `placed` put it.
fn grown_at(boxes: &[[P; 2]], on: &[usize], scales: &[Scale], placed: &[(P, bool)]) -> Vec<[P; 2]> {
    on.iter()
        .enumerate()
        .map(|(j, i)| {
            let k = scales[j].mm_per_m();
            let (b, at) = (boxes[*i], placed[j].0);
            [
                [at[0] + k * b[0][0] - DIM_ROOM, at[1] + k * b[0][1] - DIM_ROOM],
                [at[0] + k * b[1][0] + DIM_ROOM, at[1] + k * b[1][1] + DIM_ROOM],
            ]
        })
        .collect()
}

fn overlap(a: [P; 2], b: [P; 2]) -> bool {
    a[0][0] < b[1][0] && b[0][0] < a[1][0] && a[0][1] < b[1][1] && b[0][1] < a[1][1]
}

/// Whether every box lies in the template's area, clear of its keep-out
/// boxes and of each other.
fn fits(grown: &[[P; 2]], t: &Template) -> bool {
    let [lo, hi] = t.area;
    grown.iter().enumerate().all(|(i, b)| {
        b[0][0] >= lo[0]
            && b[0][1] >= lo[1]
            && b[1][0] <= hi[0]
            && b[1][1] <= hi[1]
            && t.keep_out.iter().all(|k| !overlap(*b, *k))
            && grown[i + 1..].iter().all(|c| !overlap(*b, *c))
    })
}

/// The template's lines and texts, its fields filled.
#[allow(clippy::too_many_arguments)]
fn template_items(
    session: &mut Session,
    part: &Id,
    file: &DrawingFile,
    t: &Template,
    sheet: &crate::doc::drawing::Sheet,
    scale: Scale,
    number: usize,
    style: ryu_draw::Standard,
) -> Vec<Item> {
    let values = fields(session, part, file, sheet, scale, number);
    let mut items = Vec::new();
    for (lines, kind) in [(&t.lines, LineKind::Border), (&t.thin, LineKind::Thin)] {
        for l in lines {
            items.push(Item::Path {
                path: Path::polyline(l, false),
                stroke: Some(style.stroke(kind)),
                fill: None,
            });
        }
    }
    for x in &t.texts {
        let text = crate::doc::drawing_template::fill(&x.pieces, &|n| values.get(n).cloned());
        if text.is_empty() {
            continue;
        }
        items.push(Item::Text {
            text,
            at: x.at,
            size: x.size,
            angle: 0.0,
            anchor: match x.anchor {
                crate::doc::drawing_template::Anchor::Start => Anchor::Start,
                crate::doc::drawing_template::Anchor::Middle => Anchor::Middle,
                crate::doc::drawing_template::Anchor::End => Anchor::End,
            },
            colour: Colour::BLACK,
        });
    }
    items
}

/// What a title block's fields read: PDM properties, the release, the
/// sheet, and the drawing's own `fields` last (they win).
fn fields(
    session: &mut Session,
    part: &Id,
    file: &DrawingFile,
    sheet: &crate::doc::drawing::Sheet,
    scale: Scale,
    number: usize,
) -> BTreeMap<String, String> {
    let mut v: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(props) = ryupdm::props::read(&*session, part) {
        for (k, x) in &props.values {
            let text = match x {
                Value::String(s) => s.clone(),
                Value::Null => continue,
                other => other.to_string(),
            };
            v.insert(k.clone(), text);
        }
    }
    v.entry("name".into()).or_insert_with(|| file.name.clone());
    v.entry("number".into()).or_insert_with(|| part.to_string());
    let (revision, date) = release_of(session, part);
    v.insert("revision".into(), revision);
    v.insert("date".into(), date);
    v.insert("scale".into(), scale.to_string());
    v.insert("sheet".into(), number.to_string());
    v.insert("sheets".into(), file.sheets.len().to_string());
    v.insert("size".into(), sheet.size.as_str().trim_start_matches("ansi_").trim_start_matches("iso_").to_uppercase());
    let standard = file.standard_in(session.project().manifest.standard());
    v.insert(
        "standard".into(),
        match standard {
            Standard::Asme => "ASME Y14.5".into(),
            Standard::Iso => "ISO 128".into(),
        },
    );
    for (k, x) in &file.fields {
        v.insert(k.clone(), x.clone());
    }
    v
}

/// The part's latest official release label (with `*` while it is
/// modified) and the date its commit became official, both empty in
/// work: no clock, so builds stay deterministic.
fn release_of(session: &mut Session, part: &Id) -> (String, String) {
    let Ok((repo, releases)) = crate::release::history(session) else {
        return (String::new(), String::new());
    };
    let state = ryupdm::state::derive(&session.files(), part, &releases);
    let label = match &state.status {
        ryupdm::state::Status::Released(l) | ryupdm::state::Status::Obsolete(l) => l.clone(),
        ryupdm::state::Status::Modified(l) => format!("{l}*"),
        _ => return (String::new(), String::new()),
    };
    let bare = label.trim_end_matches('*');
    let date = releases
        .official
        .iter()
        .find(|(path, _)| {
            path.strip_prefix(&format!("releases/{part}/"))
                .and_then(ryupdm::record::parse_name)
                .is_some_and(|(l, k)| l == bare && k == ryupdm::record::Kind::Release)
        })
        .and_then(|(_, o)| repo.commit_time(&o.commit).ok())
        .map(|t| crate::export::utc(t)[..10].to_owned())
        .unwrap_or_default();
    (label, date)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};

    /// A project with part `p`, a box `b` of `x` × `y` × `z` mm at the
    /// origin, and drawing `d` of `views` on one ANSI B sheet.
    pub(crate) fn drawn(x: f64, y: f64, z: f64, views: Value) -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": x, "y": y, "z": z}}),
        )
        .unwrap();
        let file = json!({"format": 1, "kind": "ryucad.drawing", "name": "Block",
            "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b"}}, "views": views});
        let at = dir.path().join("parts/p/drawings");
        std::fs::create_dir_all(&at).unwrap();
        crate::doc::project::write_json(&at.join("d.json"), &file).unwrap();
        s.reload().unwrap();
        (dir, s)
    }

    fn id(s: &str) -> Id {
        Id::parse(s, "x").unwrap()
    }

    /// The length of a path made of lines, in its unit.
    fn length(p: &Path) -> f64 {
        let mut at = p.start;
        let mut n = 0.0;
        for s in &p.segments {
            match s {
                Segment::Line(q) => n += distance(at, *q),
                other => panic!("not a line: {other:?}"),
            }
            at = s.end();
        }
        n
    }

    /// The thick visible paths in the view's box.
    fn visible_in(sheet: &SheetBuilt, v: &ViewBuilt) -> Vec<Path> {
        let [lo, hi] = v.bounds;
        sheet
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Path { path, stroke: Some(s), .. } if s.width == 0.6 && s.dash.is_empty() => {
                    let [a, b] = path.bounds();
                    (a[0] >= lo[0] - 1e-6 && a[1] >= lo[1] - 1e-6 && b[0] <= hi[0] + 1e-6 && b[1] <= hi[1] + 1e-6)
                        .then(|| path.clone())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_blocks_three_views_and_isometric_draw_its_edges_at_their_lengths() {
        let views = json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "front"},
            "v2": {"type": "projected", "sheet": "s1", "from": "v1", "side": "top"},
            "v3": {"type": "projected", "sheet": "s1", "from": "v1", "side": "right"},
            "v4": {"type": "projected", "sheet": "s1", "from": "v1", "side": "iso"}
        });
        let (_d, mut s) = drawn(40.0, 20.0, 10.0, views);
        let b = build(&mut s, &id("p"), &id("d")).unwrap();
        assert!(b.findings.is_empty(), "{:?}", b.findings);
        let sheet = &b.sheets[0];
        // The largest standard scale the views fit at: 1:1 on ANSI B.
        assert_eq!(sheet.scale.to_string(), "2:1");
        let k = 2.0;
        let views: BTreeMap<String, &ViewBuilt> =
            sheet.views.iter().map(|v| (v.key.to_string(), v)).collect();
        // Front: 40 × 10, its outline a rectangle; top 40 × 20; right 20 × 10.
        for (v, w, h) in [("v1", 40.0, 10.0), ("v2", 40.0, 20.0), ("v3", 20.0, 10.0)] {
            let lines = visible_in(sheet, views[v]);
            let total: f64 = lines.iter().map(length).sum();
            assert!((total - k * 2.0 * (w + h)).abs() < 1e-9, "{v}: {total}");
            let [lo, hi] = views[v].bounds;
            assert!((hi[0] - lo[0] - k * w).abs() < 1e-9 && (hi[1] - lo[1] - k * h).abs() < 1e-9);
        }
        // Third angle: the top above the front, the right to its right,
        // each on the front's column or row exactly.
        let (f, t, r) = (views["v1"].at, views["v2"].at, views["v3"].at);
        assert_eq!(f[0], t[0]);
        assert!(t[1] > f[1]);
        assert_eq!(f[1], r[1]);
        assert!(r[0] > f[0]);
        // The isometric's nine visible edges: 4 × 40, 4 × 20 and 4 × 10
        // less the three hidden ones, each foreshortened by sqrt(2/3).
        let iso: f64 = visible_in(sheet, views["v4"]).iter().map(length).sum();
        let want = k * (3.0 * 40.0 + 3.0 * 20.0 + 3.0 * 10.0) * (2.0f64 / 3.0).sqrt();
        assert!((iso - want).abs() < 1e-6, "{iso} against {want}");
        // ryucad placed them all, and says so for writing back.
        let places = b.placements(match &s.project().part(&id("p")).unwrap().drawings[&id("d")] {
            Ok(f) => f,
            Err(e) => panic!("{e:?}"),
        });
        assert_eq!(places.len(), 4);
        assert!(matches!(places[&Key::parse("v2", 'v').unwrap()], Placement::Offset(_)));
        // The sheet's title block reads the drawing.
        assert!(sheet.items.iter().any(|it| matches!(it, Item::Text { text, .. } if text == "Block")));
    }

    #[test]
    fn hidden_lines_are_dashed_only_when_a_view_asks() {
        // A block with a through hole along y: the front sees the bore's
        // circle, the top its two hidden generators.
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 40, "y": 20, "z": 20},
                   "placement": {"origin": [-20, -10, -10]}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "c", "operation": "cut", "targets": ["b"],
                   "params": {"radius": 5, "height": 40},
                   "placement": {"origin": [0, -20, 0], "z": [0, 1, 0], "x": [1, 0, 0]}}),
        )
        .unwrap();
        let at = dir.path().join("parts/p/drawings");
        std::fs::create_dir_all(&at).unwrap();
        for (name, hidden) in [("off", json!(null)), ("on", json!("dashed"))] {
            let mut top = json!({"type": "projected", "sheet": "s1", "from": "v1", "side": "top"});
            if !hidden.is_null() {
                top["hidden"] = hidden;
            }
            let file = json!({"format": 1, "kind": "ryucad.drawing", "name": "Bored",
                "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b", "scale": "1:1"}},
                "views": {"v1": {"type": "base", "sheet": "s1", "orient": "front"}, "v2": top}});
            crate::doc::project::write_json(&at.join(format!("{name}.json")), &file).unwrap();
        }
        s.reload().unwrap();
        let dashed = |b: &Built| -> usize {
            b.sheets[0]
                .items
                .iter()
                .filter(|it| matches!(it, Item::Path { stroke: Some(s), .. } if !s.dash.is_empty()))
                .count()
        };
        let off = build(&mut s, &id("p"), &id("off")).unwrap();
        assert_eq!(dashed(&off), 0);
        let on = build(&mut s, &id("p"), &id("on")).unwrap();
        // The bore's two generators, as long as the block is deep (20 mm),
        // in the top view.
        let lines: Vec<f64> = on.sheets[0]
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Path { path, stroke: Some(st), .. } if !st.dash.is_empty() => Some(length(path)),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| (l - 20.0).abs() < 1e-9), "{lines:?}");
        // The second build is the session's: no kernel call, the same items.
        let again = build(&mut s, &id("p"), &id("on")).unwrap();
        assert_eq!(again.sheets[0].items, on.sheets[0].items);
    }

    #[test]
    fn a_drawing_the_part_hasnt_is_not_found() {
        let (_d, mut s) = drawn(10.0, 10.0, 10.0, json!({}));
        let e = build(&mut s, &id("p"), &id("other")).unwrap_err();
        assert_eq!(e.code, Code::DrawingNotFound);
        assert_eq!(e.details["drawings"], json!(["d"]));
    }
}
