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

pub mod annotate;
pub mod curves;
/// ISO 286 limits and fits, in `ryu-standards` since ryucad 28 (holes
/// read them too); the path drawings always used.
pub use ryu_standards::fit;
pub mod lint;
pub mod output;
pub mod request;
pub mod section;
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
use ryukei::topo::FaceId;
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
    /// Whether it lies along its parent's row or column (placed by an
    /// offset, not an at).
    pub along: bool,
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
    /// Its items, as indices into its sheet's.
    pub items: std::ops::Range<usize>,
}

impl ViewBuilt {
    /// A hash of what it draws and where: equal, the same view drawn.
    pub fn fingerprint(&self, sheet: &SheetBuilt) -> String {
        let mut h = blake3::Hasher::new();
        h.update(self.sheet.to_string().as_bytes());
        h.update(self.scale.to_string().as_bytes());
        for x in [self.at, self.bounds[0], self.bounds[1]].concat() {
            h.update(&x.to_bits().to_le_bytes());
        }
        h.update(format!("{:?}", &sheet.items[self.items.clone()]).as_bytes());
        h.finalize().to_hex().to_string()
    }
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
    /// Whether ryucad chose it (the sheet says none).
    pub scale_chosen: bool,
    pub items: Vec<Item>,
    pub views: Vec<ViewBuilt>,
    /// Where its views go, and the boxes in it none may cover (its
    /// template's title block), in sheet millimetres.
    pub area: [P; 2],
    pub keep_out: Vec<[P; 2]>,
    /// Its annotations as drawn.
    pub annotations: Vec<annotate::Built>,
    /// The model entity each view's curve draws, by the item's index:
    /// what picking a curve in the app chooses.
    pub sources: BTreeMap<usize, String>,
    /// The hole features its views show, each once: its part, its hole,
    /// a view showing it and a wall's reference there (for the lint).
    pub holes: Vec<(Id, Id, Key, String)>,
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
    /// `{code, severity, view, sheet, message, details}`, then the
    /// annotations' and the lint's.
    pub findings: Vec<Value>,
    /// Each sheet's lint score, by sheet key: 100 less its findings'
    /// points.
    pub scores: BTreeMap<String, i64>,
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
            match w.parent().and_then(|p| by_key.get(&p)) {
                // On its parent's row or column: its distance along it.
                Some(p) if v.along => {
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
                let towards = add3(
                    add3(scale3(x, back[0]), scale3(t, -back[1])),
                    scale3(y, back[2]),
                );
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
    /// written or one ryucad chooses; `reach` orders a family's views as
    /// they reach from their parent (0 an orthographic neighbour, 1 the
    /// back view, 2 an auxiliary view), so a nearer one is placed first.
    Along {
        parent: Key,
        dir: P,
        offset: Option<f64>,
        reach: u8,
    },
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
    /// What it draws beyond its projection's pieces.
    extra: Extra,
    /// Each source solid's caps once cut (a section, or a detail of one),
    /// by solid; empty for a view that cuts nothing.
    caps: Vec<Vec<FaceId>>,
    /// Why it can't be drawn, found before its projection (a cut the
    /// kernel refused).
    failed: Option<Error>,
}

/// What a view draws beyond its projection's pieces.
#[derive(Clone, Debug)]
enum Extra {
    None,
    Section(section::Cutting),
    /// Its parent's pieces kept inside the circle about `centre` (model
    /// metres) of `radius`.
    Detail {
        parent: Key,
        centre: [f64; 3],
        radius: f64,
    },
    /// A flat pattern, drawn from the pattern itself: no projection.
    Flat {
        flat: Arc<crate::sheet_metal::flat::FlatOf>,
        notes: bool,
    },
    /// An explode's trails, each a polyline in model space.
    Exploded {
        trails: Vec<Vec<[f64; 3]>>,
    },
}

/// How far past every solid of `src` a cut's tool reaches: their box's
/// diagonal and a millimetre.
fn reach(src: &Source) -> f64 {
    src.iter()
        .filter_map(|(b, pose, _)| crate::interference::world_box(&b.body, pose))
        .reduce(|a, b| a.union(&b))
        .map_or(0.0, |b| (b.max() - b.min()).length())
        + 1e-3
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
                (
                    s.built.clone(),
                    o.pose,
                    format!("{}/{}", path.join("/"), s.founder),
                )
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
            [
                (lo.x + hi.x) / 2.0,
                (lo.y + hi.y) / 2.0,
                (lo.z + hi.z) / 2.0,
            ]
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
fn orient(session: &mut Session, of: &Id, rev: &Rev, o: &Orient) -> Result<([f64; 3], [f64; 3])> {
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
                None => Err(
                    Error::new(Code::ViewNotFound, format!("{of} has no named view {n}"))
                        .with("part", of)
                        .with("views", p.views.keys().collect::<Vec<_>>()),
                ),
            }
        }
        // A face of the view's own part, at the revision it is drawn at.
        Orient::NormalTo(r) => crate::geometry::normal_to_face_at(
            session,
            &format!("{}/{}/{}", of, r.feature, r.name),
            rev,
        ),
    }
}

/// The plans of `file`'s views, parents before their children.
fn plans(
    session: &mut Session,
    part: &Id,
    file: &DrawingFile,
    angle: Angle,
) -> Result<Vec<(Plan, Source)>> {
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
            let (of, rev, frame, anchor, place, kind, extra, own) = match (&w.kind, parent) {
                (
                    ViewKind::Base {
                        of,
                        label,
                        orient: o,
                    },
                    _,
                ) => {
                    let of = of.clone().unwrap_or_else(|| part.clone());
                    let rev = label.clone().map_or(Rev::Working, Rev::Label);
                    let key = (of.clone(), format!("{rev:?}"));
                    if !sources.contains_key(&key) {
                        let s = source(session, &of, &rev)?;
                        sources.insert(key.clone(), s);
                    }
                    let anchor = middle(&sources[&key]);
                    let (t, u) = orient(session, &of, &rev, o)?;
                    (
                        of,
                        rev,
                        Frame::new(t, u)?,
                        anchor,
                        Place::At(at(w)),
                        "base",
                        Extra::None,
                        None,
                    )
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
                            reach: u8::from(*side == Side::Back),
                        },
                        None => Place::At(at(w)),
                    };
                    (
                        p.of.clone(),
                        p.rev.clone(),
                        frame,
                        p.anchor,
                        place,
                        "projected",
                        Extra::None,
                        None,
                    )
                }
                (
                    ViewKind::Auxiliary {
                        normal_to, offset, ..
                    },
                    Some(i),
                ) => {
                    let p = &out[i].0;
                    let text = format!("{}/{}/{}", p.of, normal_to.feature, normal_to.name);
                    let (n, _) = crate::geometry::normal_to_face_at(session, &text, &p.rev)?;
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
                        reach: 2,
                    };
                    (
                        p.of.clone(),
                        p.rev.clone(),
                        frame,
                        p.anchor,
                        place,
                        "auxiliary",
                        Extra::None,
                        None,
                    )
                }
                (
                    ViewKind::Section {
                        cut,
                        flip,
                        unsectioned,
                        offset,
                        ..
                    },
                    Some(i),
                ) => {
                    let p = &out[i].0;
                    let letter = w.label.clone().unwrap_or_default();
                    let c = section::cutting(
                        session,
                        &p.of,
                        &p.rev,
                        *k,
                        (p.key, &p.frame),
                        cut,
                        *flip,
                        unsectioned,
                        &letter,
                    )?;
                    let frame = section::frame_of(c.normal, &p.frame)?;
                    // Along its parent's row or column where it looks along
                    // the parent's plane, as a projected view; else by its at.
                    let img = p.frame.image(c.normal);
                    let len = ryu_math::hypot(img[0], img[1]);
                    let along = c.normal[0] * p.frame.towards[0]
                        + c.normal[1] * p.frame.towards[1]
                        + c.normal[2] * p.frame.towards[2];
                    let flip = if angle == Angle::First { -1.0 } else { 1.0 };
                    let place = if along.abs() < 1e-9 && len > 1e-9 {
                        Place::Along {
                            parent: p.key,
                            dir: [flip * img[0] / len, flip * img[1] / len],
                            offset: mm(offset),
                            reach: 1,
                        }
                    } else if offset.is_some() {
                        return Err(Error::new(
                            Code::InvalidParam,
                            format!(
                                "view {k} doesn't look along its parent {}'s plane: place it by at",
                                p.key
                            ),
                        )
                        .with("pointer", format!("/views/{k}/offset")));
                    } else {
                        Place::At(at(w))
                    };
                    (
                        p.of.clone(),
                        p.rev.clone(),
                        frame,
                        p.anchor,
                        place,
                        "section",
                        Extra::Section(c),
                        None,
                    )
                }
                (ViewKind::Detail { centre, radius, .. }, Some(i)) => {
                    let p = &out[i].0;
                    let (of, rev, frame, pk) = (p.of.clone(), p.rev.clone(), p.frame, p.key);
                    let c = section::point_at(
                        session,
                        &of,
                        &rev,
                        centre,
                        &format!("/views/{k}/centre"),
                    )?;
                    (
                        of,
                        rev,
                        frame,
                        c,
                        Place::At(at(w)),
                        "detail",
                        Extra::Detail {
                            parent: pk,
                            centre: c,
                            radius: radius.meters(),
                        },
                        None,
                    )
                }
                (ViewKind::Flat { of, solid, notes }, _) => {
                    let of = of.clone().unwrap_or_else(|| part.clone());
                    let (_, flat) = crate::sheet_metal::flat::of(
                        session,
                        of.as_str(),
                        solid.as_ref().map(Id::as_str),
                        None,
                        &crate::sheet_metal::flat::Options::default(),
                    )?;
                    let f = flat.pattern.frame;
                    let b = flat.pattern.bounds;
                    let (lo, hi) = (b.min(), b.max());
                    let mid = [(lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0];
                    let o = f.origin().to_array();
                    let (x, y, z) = (
                        f.x().get().to_array(),
                        f.y().get().to_array(),
                        f.z().get().to_array(),
                    );
                    let anchor = [0, 1, 2].map(|i| o[i] + mid[0] * x[i] + mid[1] * y[i]);
                    (
                        of,
                        Rev::Working,
                        Frame::new(z, y)?,
                        anchor,
                        Place::At(at(w)),
                        "flat",
                        Extra::Flat {
                            flat,
                            notes: *notes,
                        },
                        Some(Vec::new()),
                    )
                }
                (
                    ViewKind::Exploded {
                        of,
                        explode,
                        fraction,
                        orient: o,
                        trails,
                    },
                    _,
                ) => {
                    let of = of.clone().unwrap_or_else(|| part.clone());
                    let resolved = crate::explode::resolve(session, &of, explode.as_str())?;
                    let offsets = crate::explode::offsets(&resolved.steps, *fraction);
                    let src: Source = source(session, &of, &Rev::Working)?
                        .into_iter()
                        .map(|(b, pose, name)| {
                            let path = name
                                .split_once('/')
                                .and_then(|(_, r)| r.rsplit_once('/'))
                                .map_or("", |(p, _)| p);
                            let pose = offsets.get(path).map_or(pose, |m| m.0.then_after(&pose));
                            (b, pose, name)
                        })
                        .collect();
                    let mut lines = Vec::new();
                    if *trails {
                        let model = session.model(&of, &Rev::Working)?;
                        for (path, start) in crate::explode::middles(&model) {
                            if offsets.contains_key(&path) {
                                lines.push(crate::explode::trail(
                                    &resolved.steps,
                                    &path,
                                    start,
                                    *fraction,
                                ));
                            }
                        }
                    }
                    let anchor = middle(&src);
                    let (t, u) = orient(session, &of, &Rev::Working, o)?;
                    (
                        of,
                        Rev::Working,
                        Frame::new(t, u)?,
                        anchor,
                        Place::At(at(w)),
                        "exploded",
                        Extra::Exploded { trails: lines },
                        Some(src),
                    )
                }
                (_, None) => unreachable!("a derived view waits for its parent"),
            };
            let key = (of.clone(), format!("{rev:?}"));
            if !sources.contains_key(&key) {
                let s = source(session, &of, &rev)?;
                sources.insert(key.clone(), s);
            }
            // A detail takes its own scale, else twice its sheet's.
            let scale = match extra {
                Extra::Detail { .. } => w.scale,
                _ => w
                    .scale
                    .or_else(|| parent.and_then(|i| out[i].0.scale.filter(|_| w.scale.is_none()))),
            };
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
                    extra,
                    caps: Vec::new(),
                    failed: None,
                },
                own.unwrap_or_else(|| sources[&key].clone()),
            ));
        }
        if done.len() == before {
            // `DrawingFile::check` refuses cycles; nothing is left waiting.
            break;
        }
    }
    Ok(out)
}

/// A path a view draws, the kind of line it is, whether its visibility
/// is uncertain, and the model entity it draws (`<part>/<solid>/<name>`,
/// an occurrence's with its instance path) where it draws one.
type Line = (Path, LineKind, bool, Option<String>);

/// What one view drew in its own frame: paths in model metres relative to
/// its anchor's image, each with the kind of line it is.
struct Drawn {
    lines: Vec<Line>,
    /// Text it writes (a flat's bend notes): what, where on its baseline's
    /// middle, and its angle, in the same frame.
    texts: Vec<(String, P, f64)>,
    /// Each source solid's caps' boundaries, by solid, to hatch.
    caps: Vec<Vec<Path>>,
    /// Each entity it drew (an edge, or a face by its silhouette), by its
    /// source solid and name: what annotations stand on.
    named: BTreeMap<(usize, String), Named>,
    /// Its box, in the same frame.
    bounds: Option<[P; 2]>,
    counts: [usize; 3],
    uncertain: Vec<String>,
}

/// What a view drew of one entity, in the view's frame (model metres
/// relative to its anchor's image).
#[derive(Clone, Debug, Default)]
struct Named {
    visible: bool,
    hidden: bool,
    paths: Vec<Path>,
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
    if let Extra::Flat { flat, notes } = &plan.extra {
        return drawn_flat(flat, *notes);
    }
    let c = plan.frame.image(plan.anchor);
    let mut lines: Vec<Line> = Vec::new();
    let mut counts = [0; 3];
    let mut uncertain = Vec::new();
    let mut last: Option<(ryukei::project::Source, LineKind)> = None;
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut named: BTreeMap<(usize, String), Named> = BTreeMap::new();
    // An edge between two caps (an offset section's step) isn't drawn.
    let between_caps = |s: &ryukei::project::Source| -> bool {
        let ryukei::project::Source::Edge { body, edge, .. } = s else {
            return false;
        };
        let Some(caps) = plan.caps.get(*body as usize).filter(|c| !c.is_empty()) else {
            return false;
        };
        let b = &src[*body as usize].0.body;
        let faces: Vec<FaceId> = b
            .edge_fins(*edge)
            .filter_map(|f| b.fin(f).and_then(|f| b.loop_(f.loop_)).map(|l| l.face))
            .collect();
        faces.len() >= 2 && faces.iter().all(|f| caps.contains(f))
    };
    for (i, piece) in p.pieces.iter().enumerate() {
        if let Some(first) = piece.overlaps.first()
            && let Some(o) = p.pieces.get(*first as usize)
            && (o.depth[1] > piece.depth[1]
                || (o.depth[1] == piece.depth[1] && (*first as usize) < i))
        {
            continue;
        }
        let Some((kind, unsure)) = line_of(piece, plan) else {
            continue;
        };
        if between_caps(&piece.source) {
            continue;
        }
        let source = || {
            let (body, what, name) = match piece.source {
                ryukei::project::Source::Edge { body, name, .. } => (body, "edge", name),
                ryukei::project::Source::Silhouette {
                    body, face_name, ..
                } => (body, "face", face_name),
            };
            let solid = src.get(body as usize).map_or("", |(_, _, r)| r.as_str());
            format!("{what} {solid}/{name}")
        };
        // The stretches it draws: all of it, or a detail's inside its circle.
        let stretches: Vec<ryukei::project::ImageCurve> = match &plan.extra {
            Extra::Detail { centre, radius, .. } => {
                let m = plan.frame.image(*centre);
                match ryukei::project::inside_circle(
                    &piece.image,
                    ryukei::geom::Point2::new(m[0], m[1]),
                    *radius,
                ) {
                    Ok(r) => r
                        .into_iter()
                        .map(|range| ryukei::project::ImageCurve {
                            image: piece.image.image.clone(),
                            range,
                        })
                        .collect(),
                    Err(_) => {
                        counts[2] += 1;
                        uncertain.push(format!(
                            "{} (not drawn: its crossing of the detail's circle is uncertain)",
                            source()
                        ));
                        continue;
                    }
                }
            }
            _ => vec![piece.image.clone()],
        };
        for stretch in &stretches {
            let path = match curves::path(stretch) {
                Ok(Some(path)) => path,
                Ok(None) => continue,
                // Not drawn, but never silently: counted uncertain and named.
                Err(curves::Uncut) => {
                    counts[2] += 1;
                    uncertain.push(format!("{} (not drawn: its image can't be cut)", source()));
                    continue;
                }
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
                uncertain.push(source());
            }
            let (body, name) = match piece.source {
                ryukei::project::Source::Edge { body, name, .. } => (body, name),
                ryukei::project::Source::Silhouette {
                    body, face_name, ..
                } => (body, face_name),
            };
            let entity = src
                .get(body as usize)
                .map(|(_, _, r)| format!("{r}/{name}"));
            let n = named.entry((body as usize, name.to_string())).or_default();
            if kind == LineKind::Hidden {
                n.hidden = true;
            } else {
                n.visible = true;
            }
            n.paths.push(path.clone());
            // One source's next piece, drawn alike, from where the last
            // ended: the same path.
            let same = |s: ryukei::project::Source| match (last, s) {
                (Some((a, k)), b) => a == b && k == kind,
                _ => false,
            };
            if same(piece.source)
                && let Some((prev, _, u, _)) = lines.last_mut()
                && distance(
                    prev.segments.last().map_or(prev.start, Segment::end),
                    path.start,
                ) < 1e-9
            {
                join(prev, path);
                *u |= unsure;
            } else {
                lines.push((path, kind, unsure, entity));
            }
            last = Some((piece.source, kind));
        }
    }
    // An explode's trails, thin phantoms through where each part goes.
    if let Extra::Exploded { trails } = &plan.extra {
        for t in trails.iter().filter(|t| t.len() >= 2) {
            let pts: Vec<P> = t
                .iter()
                .map(|q| {
                    let i = plan.frame.image(*q);
                    [i[0] - c[0], i[1] - c[1]]
                })
                .collect();
            lines.push((Path::polyline(&pts, false), LineKind::Phantom, false, None));
        }
    }
    let mut bounds = (lo[0] <= hi[0]).then_some([lo, hi]);
    if let Extra::Detail { radius, .. } = &plan.extra {
        bounds = Some([[-radius, -radius], [*radius, *radius]]);
    }
    Drawn {
        lines,
        texts: Vec::new(),
        caps: section::cap_paths(src, &plan.caps, &plan.frame, plan.anchor),
        named,
        bounds,
        counts,
        uncertain,
    }
}

/// A flat view: its pattern's loops thick, its bend lines thin phantoms
/// with their notes, about the middle of the pattern's box, in metres.
fn drawn_flat(flat: &crate::sheet_metal::flat::FlatOf, notes: bool) -> Drawn {
    let p = &flat.pattern;
    let (lo, hi) = (p.bounds.min(), p.bounds.max());
    let m = [(lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0];
    let mut lines = Vec::new();
    let mut counts = [0; 3];
    let mut uncertain = Vec::new();
    for l in &p.loops {
        for s in &l.segments {
            match curves::plane(&s.curve, s.t0, s.t1) {
                Ok(path) => {
                    let entity = format!("{}/{}/{}", flat.part, flat.solid, s.edge);
                    lines.push((
                        path.moved([-m[0], -m[1]]),
                        LineKind::Visible,
                        false,
                        Some(entity),
                    ));
                    counts[0] += 1;
                }
                Err(curves::Uncut) => {
                    counts[2] += 1;
                    uncertain.push(format!(
                        "edge {}/{}/{} (not drawn: its curve can't be cut)",
                        flat.part, flat.solid, s.edge
                    ));
                }
            }
        }
    }
    let mut texts = Vec::new();
    for b in &p.bend_lines {
        let (a, e) = (
            [b.start.x - m[0], b.start.y - m[1]],
            [b.end.x - m[0], b.end.y - m[1]],
        );
        lines.push((
            Path::polyline(&[a, e], false),
            LineKind::Phantom,
            false,
            None,
        ));
        if notes {
            // Reading left to right, at the line's middle.
            let (mut dx, mut dy) = (e[0] - a[0], e[1] - a[1]);
            if dx < 0.0 || (dx == 0.0 && dy < 0.0) {
                (dx, dy) = (-dx, -dy);
            }
            let text = ryuformats::dxf::note(b, ryuformats::dxf::DxfUnit::Mm, "°");
            texts.push((
                text,
                [(a[0] + e[0]) / 2.0, (a[1] + e[1]) / 2.0],
                ryu_math::atan2(dy, dx),
            ));
        }
    }
    Drawn {
        lines,
        texts,
        caps: Vec::new(),
        named: BTreeMap::new(),
        bounds: Some([[lo.x - m[0], lo.y - m[1]], [hi.x - m[0], hi.y - m[1]]]),
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
            let (u, v) = (
                [a[0] - start[0], a[1] - start[1]],
                [b[0] - a[0], b[1] - a[1]],
            );
            let cross = u[0] * v[1] - u[1] * v[0];
            let dot = u[0] * v[0] + u[1] * v[1];
            if cross.abs()
                <= 1e-12 * (ryu_math::hypot(u[0], u[1]) * ryu_math::hypot(v[0], v[1])).max(1e-30)
                && dot > 0.0
            {
                *path.segments.last_mut().expect("checked") = Segment::Line(*b);
                continue;
            }
        }
        path.segments.push(s);
    }
}

/// Projections for `plans`, from the session's cache, the disk's, or the
/// kernel side by side, each distinct view projected once (a detail shares
/// its parent's); each failure in its place. A flat view projects nothing.
fn project(
    session: &mut Session,
    plans: &[(Plan, Source)],
) -> Result<Vec<std::result::Result<Arc<Projection>, Error>>> {
    let keys: Vec<String> = plans.iter().map(|(p, s)| key_of(s, &p.frame)).collect();
    // The first plan of each key projects for every plan of it.
    let mut first: BTreeMap<&str, usize> = BTreeMap::new();
    let mut lead: Vec<usize> = Vec::with_capacity(plans.len());
    for (i, k) in keys.iter().enumerate() {
        let skip = plans[i].0.failed.is_some() || matches!(plans[i].0.extra, Extra::Flat { .. });
        lead.push(if skip {
            i
        } else {
            *first.entry(k).or_insert(i)
        });
    }
    let disk = session.disk().cloned();
    let mut found: Vec<Option<Arc<Projection>>> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            if lead[i] != i || plans[i].0.failed.is_some() {
                return None;
            }
            if matches!(plans[i].0.extra, Extra::Flat { .. }) {
                return Some(Arc::new(Projection::default()));
            }
            session.caches_mut().projections_mut().get(k).or_else(|| {
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
    let job = session.job().clone();
    let stage = job.begin(ryu_session::job::Phase::Project, "views", plans.len());
    use rayon::prelude::*;
    let made: Vec<Option<std::result::Result<Projection, ryukei::Error>>> = plans
        .par_iter()
        .zip(found.par_iter())
        .enumerate()
        .map(|(i, ((plan, src), have))| {
            if have.is_some() || lead[i] != i || plan.failed.is_some() {
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
    let mut out: Vec<std::result::Result<Arc<Projection>, Error>> = Vec::new();
    for (i, m) in made.into_iter().enumerate() {
        stage.step(|| plans[i].0.key.to_string())?;
        if let Some(e) = &plans[i].0.failed {
            out.push(Err(e.clone()));
            continue;
        }
        if lead[i] != i {
            let p = out[lead[i]].clone();
            out.push(p);
            continue;
        }
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
                session
                    .caches_mut()
                    .projections_mut()
                    .put(keys[i].clone(), p.clone());
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
            return Err(
                Error::new(Code::DrawingNotFound, format!("{part} has no drawing {id}"))
                    .with("part", part)
                    .with("drawings", p.drawings.keys().collect::<Vec<_>>()),
            );
        }
    };
    file.check()?;
    let standard = file.standard_in(session.project().manifest.standard());
    let angle = file.projection.unwrap_or_else(|| standard.projection());
    let mut plans = plans(session, part, &file, angle)?;
    section::cut_all(session, &mut plans)?;
    // A detail draws its parent as cut: its source and caps, and its
    // failure.
    for i in 0..plans.len() {
        if let Extra::Detail { parent, .. } = plans[i].0.extra
            && let Some(j) = plans.iter().position(|(p, _)| p.key == parent)
        {
            let (src, caps, failed) = (
                plans[j].1.clone(),
                plans[j].0.caps.clone(),
                plans[j].0.failed.clone(),
            );
            plans[i].1 = src;
            plans[i].0.caps = caps;
            if plans[i].0.failed.is_none() {
                plans[i].0.failed = failed;
            }
        }
    }
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
        let tid = sheet
            .template
            .clone()
            .unwrap_or_else(|| sheet.size.template());
        let t: Template = template::resolve(
            &tid,
            sheet.size,
            sheet.orientation,
            project_templates.get(&tid).map(|f| &f.template),
        )?;
        let on: Vec<usize> = (0..plans.len())
            .filter(|i| plans[*i].0.sheet == *sk)
            .collect();
        let scales = |s: Scale| -> Vec<Scale> {
            on.iter()
                .map(|i| {
                    let p = &plans[*i].0;
                    p.scale.unwrap_or(match p.extra {
                        // Twice its parent's: the parent's own, else this
                        // sheet's.
                        Extra::Detail { parent, .. } => doubled(
                            plans
                                .iter()
                                .find(|(q, _)| q.key == parent)
                                .and_then(|(q, _)| q.scale)
                                .unwrap_or(s),
                        ),
                        _ => s,
                    })
                })
                .collect()
        };
        let fixed = sheet.scale;
        let chosen = match fixed {
            Some(s) => s,
            None => SCALES
                .iter()
                .map(|(a, b)| Scale {
                    sheet: *a,
                    model: *b,
                })
                .find(|s| {
                    let sc = scales(*s);
                    let placed = layout(&plans, &unit_boxes, &on, &sc, &t);
                    fits(&grown_at(&unit_boxes, &on, &sc, &placed), &t)
                })
                .unwrap_or(Scale {
                    sheet: 1,
                    model: 1000,
                }),
        };
        let views_scale = scales(chosen);
        let placed = layout(&plans, &unit_boxes, &on, &views_scale, &t);
        let mut items = template_items(session, part, &file, &t, sheet, chosen, count, style);
        let mut views = Vec::new();
        let mut sources = BTreeMap::new();
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
            let first = items.len();
            let (counts, error) = match &drawings[*i] {
                Ok(d) => {
                    for (path, kind, unsure, entity) in &d.lines {
                        if let Some(e) = entity {
                            sources.insert(items.len(), e.clone());
                        }
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
                    // Caps hatched, each solid its own angle and spacing,
                    // a detail's kept inside its circle.
                    let base = section::spacing(sheet.size.paper(sheet.orientation));
                    for (n, paths) in d.caps.iter().filter(|p| !p.is_empty()).enumerate() {
                        let (angle, gap) = section::hatch_of(n, base);
                        let paths: Vec<Path> = paths.iter().map(|p| p.scaled(k, at)).collect();
                        let mut hatch = ryu_draw::hatch::lines(&paths, angle, gap);
                        if let Extra::Detail { radius, .. } = &plan.extra {
                            hatch = ryu_draw::hatch::clip_to_circle(&hatch, at, k * radius);
                        }
                        for [a, b] in hatch {
                            items.push(Item::Path {
                                path: Path::polyline(&[a, b], false),
                                stroke: Some(style.stroke(LineKind::Hatch)),
                                fill: None,
                            });
                        }
                    }
                    for (text, q, turn) in &d.texts {
                        // 1.5 mm off the line, on its left as it reads.
                        let (sin, cos) = ryu_math::sin_cos(*turn);
                        items.push(Item::Text {
                            text: text.clone(),
                            at: [at[0] + k * q[0] - 1.5 * sin, at[1] + k * q[1] + 1.5 * cos],
                            size: 2.5,
                            angle: *turn,
                            anchor: Anchor::Middle,
                            colour: Colour::BLACK,
                        });
                    }
                    (d.counts, None)
                }
                Err(e) => {
                    crossed(&mut items, bounds, style);
                    ([0; 3], Some(e.clone()))
                }
            };
            let caption = match &plan.extra {
                Extra::Section(c) => Some(section::caption(&c.letter, standard)),
                Extra::Detail { .. } => plan.label.as_ref().map(|l| {
                    let s = format!("({s})");
                    match standard {
                        Standard::Asme => format!("DETAIL {l} {s}"),
                        Standard::Iso => format!("{l} {s}"),
                    }
                }),
                _ => plan.label.clone(),
            };
            if let Some(l) = &caption {
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
                along: matches!(plan.place, Place::Along { .. }),
                scale: s,
                bounds,
                towards: plan.frame.towards,
                up: plan.frame.y,
                of: plan.of.clone(),
                rev: plan.rev.clone(),
                counts,
                error,
                items: first..items.len(),
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
            scale_chosen: fixed.is_none(),
            items,
            views,
            area: t.area,
            keep_out: t.keep_out.clone(),
            annotations: Vec::new(),
            sources,
            holes: Vec::new(),
        });
    }
    marks(&plans, &mut sheets, style);
    let drawn: Vec<Option<&Drawn>> = drawings.iter().map(|d| d.as_ref().ok()).collect();
    annotate::all(
        session,
        &annotate::Context {
            part,
            file: &file,
            plans: &plans,
            drawn: &drawn,
            standard,
            style,
        },
        &mut sheets,
        &mut findings,
    )?;
    let scores = lint::run(session, part, &file, &sheets, &mut findings);
    Ok(Built {
        part: part.clone(),
        drawing: id.clone(),
        name: file.name.clone(),
        standard,
        projection: angle,
        sheets,
        findings,
        scores,
    })
}

/// Twice `s`, in lowest terms: a detail's scale where none is written.
fn doubled(s: Scale) -> Scale {
    let (a, b) = (2 * s.sheet, s.model);
    let g = gcd(a, b);
    Scale {
        sheet: (a / g).min(crate::doc::drawing::MAX_SCALE),
        model: b / g,
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Each section's cutting-plane line and each detail's circle, with
/// their letters, on the sheet their parent is drawn on.
fn marks(plans: &[(Plan, Source)], sheets: &mut [SheetBuilt], style: ryu_draw::Standard) {
    let index: BTreeMap<Key, usize> = plans
        .iter()
        .enumerate()
        .map(|(i, (p, _))| (p.key, i))
        .collect();
    for (plan, _) in plans {
        let parent = match &plan.extra {
            Extra::Section(c) => c.parent,
            Extra::Detail { parent, .. } => *parent,
            _ => continue,
        };
        let Some((si, pv)) = sheets.iter().enumerate().find_map(|(si, s)| {
            s.views
                .iter()
                .find(|v| v.key == parent)
                .map(|v| (si, v.clone()))
        }) else {
            continue;
        };
        let pp = &plans[index[&parent]].0;
        let k = pv.scale.mm_per_m();
        let c = pp.frame.image(pp.anchor);
        let on_sheet = |q: [f64; 3]| {
            let i = pp.frame.image(q);
            [pv.at[0] + k * (i[0] - c[0]), pv.at[1] + k * (i[1] - c[1])]
        };
        let items = match &plan.extra {
            Extra::Section(cut) => {
                let n = pp.frame.image(cut.normal);
                let len = ryu_math::hypot(n[0], n[1]);
                // A cut square to the parent's plane shows as a line on it.
                if len < 1e-9 {
                    continue;
                }
                let sight = [-n[0] / len, -n[1] / len];
                let along = [-sight[1], sight[0]];
                let mut line: Vec<P> = match &cut.how {
                    section::How::Plane(p) => {
                        // The plane's line across the parent's drawing and
                        // 5 mm past it each way.
                        let o = on_sheet(p.origin_m);
                        let [lo, hi] = pv.bounds;
                        let ts = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
                            .map(|q| (q[0] - o[0]) * along[0] + (q[1] - o[1]) * along[1]);
                        let t0 = ts.iter().copied().fold(f64::INFINITY, f64::min) - 5.0;
                        let t1 = ts.iter().copied().fold(f64::NEG_INFINITY, f64::max) + 5.0;
                        vec![
                            [o[0] + t0 * along[0], o[1] + t0 * along[1]],
                            [o[0] + t1 * along[0], o[1] + t1 * along[1]],
                        ]
                    }
                    section::How::Steps { points, .. } => {
                        points.iter().map(|q| on_sheet(*q)).collect()
                    }
                };
                if let section::How::Steps { .. } = cut.how {
                    // Run on past the first and last points by 5 mm.
                    let ext = |a: P, b: P| {
                        let d = ryu_math::hypot(a[0] - b[0], a[1] - b[1]).max(1e-12);
                        [
                            a[0] + 5.0 * (a[0] - b[0]) / d,
                            a[1] + 5.0 * (a[1] - b[1]) / d,
                        ]
                    };
                    let n = line.len();
                    let (first, last) = (ext(line[0], line[1]), ext(line[n - 1], line[n - 2]));
                    line[0] = first;
                    line[n - 1] = last;
                }
                section::cutting_marks(&line, sight, &cut.letter, style)
            }
            Extra::Detail { centre, radius, .. } => {
                let m = on_sheet(*centre);
                let r = k * radius;
                let d = std::f64::consts::FRAC_1_SQRT_2 * (r + 3.0);
                vec![
                    Item::Path {
                        path: Path::circle(m, r),
                        stroke: Some(style.stroke(LineKind::Thin)),
                        fill: None,
                    },
                    Item::Text {
                        text: plan.label.clone().unwrap_or_default(),
                        at: [m[0] + d, m[1] + d],
                        size: 5.0,
                        angle: 0.0,
                        anchor: Anchor::Start,
                        colour: Colour::BLACK,
                    },
                ]
            }
            _ => continue,
        };
        sheets[si].items.extend(items);
    }
}

/// A failed view's frame: its box, crossed corner to corner.
fn crossed(items: &mut Vec<Item>, [lo, hi]: [P; 2], style: ryu_draw::Standard) {
    let mut stroke = style.stroke(LineKind::Thin);
    stroke.colour = WARNING;
    let rect = Path::polyline(&[lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]], true);
    for path in [
        rect,
        Path::polyline(&[lo, hi], false),
        Path::polyline(&[[lo[0], hi[1]], [hi[0], lo[1]]], false),
    ] {
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
    let index: BTreeMap<Key, usize> = on
        .iter()
        .enumerate()
        .map(|(j, i)| (plans[*i].0.key, j))
        .collect();
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
    // Parents before their children, and among a generation the views
    // nearest their parents first (neighbours, then back, then auxiliary
    // views), keys only within each: a back view never takes the place
    // beside the front because it was added first.
    let mut depth = vec![0usize; on.len()];
    for (j, i) in on.iter().enumerate() {
        if let Place::Along { parent, .. } = plans[*i].0.place
            && let Some(pj) = index.get(&parent)
        {
            depth[j] = depth[*pj] + 1;
        }
    }
    let rank = |j: usize| match plans[on[j]].0.place {
        Place::At(_) => 0,
        Place::Along { reach, .. } => reach,
    };
    let mut order: Vec<usize> = (0..on.len()).collect();
    order.sort_by_key(|j| (depth[*j], rank(*j), plans[on[*j]].0.key));
    for j in order {
        let i = &on[j];
        match plans[*i].0.place {
            Place::At(_) => rel[j] = Some([0.0, 0.0]),
            Place::Along {
                parent,
                dir,
                offset,
                ..
            } => {
                let Some(pj) = index.get(&parent).copied() else {
                    rel[j] = Some([0.0, 0.0]);
                    continue;
                };
                let base = rel[pj].unwrap_or([0.0, 0.0]);
                let d = offset.unwrap_or_else(|| {
                    // Just clear of the parent, both grown by their room,
                    // then past every view of the family already placed
                    // that it would meet there (a back view goes past the
                    // right view on its row).
                    let reach = |bx: [P; 2], s: f64| {
                        let along = |q: P| q[0] * dir[0] + q[1] * dir[1];
                        let c = [bx[0], [bx[1][0], bx[0][1]], bx[1], [bx[0][0], bx[1][1]]];
                        c.iter()
                            .map(|q| s * along(*q))
                            .fold(f64::NEG_INFINITY, f64::max)
                    };
                    let shift = |bx: [P; 2], r: P| {
                        [
                            [bx[0][0] + r[0], bx[0][1] + r[1]],
                            [bx[1][0] + r[0], bx[1][1] + r[1]],
                        ]
                    };
                    let b = grown(j);
                    let mut d = reach(grown(pj), 1.0) + reach(b, -1.0);
                    loop {
                        let here = shift(b, [base[0] + d * dir[0], base[1] + d * dir[1]]);
                        let past = (0..on.len())
                            .filter(|m| *m != j && root[*m] == root[j])
                            .filter_map(|m| rel[m].map(|r| shift(grown(m), r)))
                            .filter(|o| overlap(here, *o))
                            .map(|o| {
                                let o = shift(o, [-base[0], -base[1]]);
                                reach(o, 1.0) + reach(b, -1.0)
                            })
                            .fold(f64::NEG_INFINITY, f64::max);
                        if past <= d {
                            break d;
                        }
                        d = past;
                    }
                });
                rel[j] = Some([base[0] + d * dir[0], base[1] + d * dir[1]]);
            }
        }
    }
    let [lo, hi] = t.area;
    let fam_box = |j: usize| -> [P; 2] {
        (0..on.len())
            .filter(|m| root[*m] == j)
            .map(|m| {
                let r = rel[m].unwrap_or([0.0, 0.0]);
                let b = grown(m);
                [
                    [r[0] + b[0][0], r[1] + b[0][1]],
                    [r[0] + b[1][0], r[1] + b[1][1]],
                ]
            })
            .reduce(|a, b| {
                [
                    [a[0][0].min(b[0][0]), a[0][1].min(b[0][1])],
                    [a[1][0].max(b[1][0]), a[1][1].max(b[1][1])],
                ]
            })
            .expect("a family has its root")
    };
    let written_root = |j: usize| matches!(plans[on[j]].0.place, Place::At(Some(_)));
    let moved = |b: [P; 2], o: P| {
        [
            [b[0][0] + o[0], b[0][1] + o[1]],
            [b[1][0] + o[0], b[1][1] + o[1]],
        ]
    };
    // Families whose root has a written place first, from it: they hold
    // their room; then the others, each where the rule puts it (the
    // area's upper left, the next family to its right; a diagonal view's
    // at the upper right), or, where that meets a family already placed,
    // the first free room down the area from its top left.
    let mut held: Vec<[P; 2]> = Vec::new();
    let mut x = lo[0];
    let roots: Vec<usize> = (0..on.len()).filter(|j| root[*j] == *j).collect();
    for j in roots
        .iter()
        .copied()
        .filter(|j| written_root(*j))
        .chain(roots.iter().copied().filter(|j| !written_root(*j)))
    {
        let fb = fam_box(j);
        let diagonal = plans[on[j]].0.kind == "projected";
        let origin = match plans[on[j]].0.place {
            Place::At(Some(p)) => p,
            _ => {
                let rule = if diagonal {
                    [hi[0] - fb[1][0], hi[1] - fb[1][1]]
                } else {
                    [x - fb[0][0], hi[1] - fb[1][1]]
                };
                let free = |o: P| held.iter().all(|h| !overlap(moved(fb, o), *h));
                let o = if free(rule) {
                    rule
                } else {
                    let step = 5.0;
                    let (w, h) = (fb[1][0] - fb[0][0], fb[1][1] - fb[0][1]);
                    let mut found = None;
                    let mut y = hi[1] - h;
                    'rows: while y >= lo[1] {
                        let mut cx = lo[0];
                        while cx + w <= hi[0] {
                            let o = [cx - fb[0][0], y - fb[0][1]];
                            if free(o) {
                                found = Some(o);
                                break 'rows;
                            }
                            cx += step;
                        }
                        y -= step;
                    }
                    found.unwrap_or(rule)
                };
                if !diagonal {
                    x = x.max(o[0] + fb[1][0]);
                }
                o
            }
        };
        held.push(moved(fb, origin));
        for m in (0..on.len()).filter(|m| root[*m] == j) {
            let r = rel[m].unwrap_or([0.0, 0.0]);
            let written = match plans[on[m]].0.place {
                Place::At(p) => p.is_some(),
                Place::Along { offset, .. } => offset.is_some(),
            };
            at[m] = Some(([origin[0] + r[0], origin[1] + r[1]], !written));
        }
    }
    at.into_iter()
        .map(|a| a.unwrap_or(([0.0, 0.0], true)))
        .collect()
}

/// Each view's box on the sheet grown by its room, where `placed` put it.
fn grown_at(boxes: &[[P; 2]], on: &[usize], scales: &[Scale], placed: &[(P, bool)]) -> Vec<[P; 2]> {
    on.iter()
        .enumerate()
        .map(|(j, i)| {
            let k = scales[j].mm_per_m();
            let (b, at) = (boxes[*i], placed[j].0);
            [
                [
                    at[0] + k * b[0][0] - DIM_ROOM,
                    at[1] + k * b[0][1] - DIM_ROOM,
                ],
                [
                    at[0] + k * b[1][0] + DIM_ROOM,
                    at[1] + k * b[1][1] + DIM_ROOM,
                ],
            ]
        })
        .collect()
}

fn overlap(a: [P; 2], b: [P; 2]) -> bool {
    a[0][0] + FIT_SLACK < b[1][0]
        && b[0][0] + FIT_SLACK < a[1][0]
        && a[0][1] + FIT_SLACK < b[1][1]
        && b[0][1] + FIT_SLACK < a[1][1]
}

/// How far past the area a view may reach and still fit, in sheet
/// millimetres: places are written back to a thousandth.
const FIT_SLACK: f64 = 1e-3;

/// Whether every box lies in the template's area, clear of its keep-out
/// boxes and of each other.
fn fits(grown: &[[P; 2]], t: &Template) -> bool {
    let [lo, hi] = t.area;
    grown.iter().enumerate().all(|(i, b)| {
        b[0][0] >= lo[0] - FIT_SLACK
            && b[0][1] >= lo[1] - FIT_SLACK
            && b[1][0] <= hi[0] + FIT_SLACK
            && b[1][1] <= hi[1] + FIT_SLACK
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
    v.insert(
        "size".into(),
        sheet
            .size
            .as_str()
            .trim_start_matches("ansi_")
            .trim_start_matches("iso_")
            .to_uppercase(),
    );
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
    use crate::testing::{call, fresh};

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
                Item::Path {
                    path,
                    stroke: Some(s),
                    ..
                } if s.width == 0.6 && s.dash.is_empty() => {
                    let [a, b] = path.bounds();
                    (a[0] >= lo[0] - 1e-6
                        && a[1] >= lo[1] - 1e-6
                        && b[0] <= hi[0] + 1e-6
                        && b[1] <= hi[1] + 1e-6)
                        .then(|| path.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// Every hatch line of the sheet: their total length on the sheet.
    fn hatched(sheet: &SheetBuilt) -> f64 {
        sheet
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Path {
                    path,
                    stroke: Some(s),
                    ..
                } if s.kind == Some(LineKind::Hatch) => Some(length(path)),
                _ => None,
            })
            .sum()
    }

    #[test]
    fn every_curve_of_a_view_names_the_entity_it_draws() {
        let views =
            json!({"v1": {"type": "base", "sheet": "s1", "orient": "front", "scale": "1:1"}});
        let (_d, mut s) = drawn(40.0, 20.0, 10.0, views);
        let b = build(&mut s, &id("p"), &id("d")).unwrap();
        let sheet = &b.sheets[0];
        let v = &sheet.views[0];
        // The front of a box: its four visible edges, each named, inside
        // the view's own items.
        let named: Vec<&String> = v
            .items
            .clone()
            .filter_map(|i| sheet.sources.get(&i))
            .collect();
        assert_eq!(named.len(), visible_in(sheet, v).len(), "{named:?}");
        assert_eq!(named.len(), 4);
        for e in &named {
            assert!(e.starts_with("p/b/") && e.len() == "p/b/".len() + 32, "{e}");
        }
        // Nothing outside a view (the template, its captions) names one.
        assert!(sheet.sources.keys().all(|i| v.items.contains(i)));
        let r = crate::drawing::request::get(&mut s, "p", "d", None, true).unwrap();
        let j = &r["sheets"][0];
        assert_eq!(j["views"][0]["items"], json!([v.items.start, v.items.end]));
        let with: Vec<&Value> = j["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i.get("entity").is_some())
            .collect();
        assert_eq!(with.len(), 4);
        // And the file as written, for the app's forms and drags.
        assert_eq!(r["file"]["views"]["v1"]["orient"], "front");
    }

    #[test]
    fn a_full_section_hatches_its_cap_at_its_area() {
        let views = json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "front", "scale": "1:1"},
            "v2": {"type": "section", "sheet": "s1", "from": "v1", "label": "A",
                   "cut": {"origin": ["20 mm", 0, 0], "normal": [1, 0, 0]}}
        });
        let (_d, mut s) = drawn(40.0, 20.0, 10.0, views);
        let b = build(&mut s, &id("p"), &id("d")).unwrap();
        assert!(b.findings.is_empty(), "{:?}", b.findings);
        let sheet = &b.sheets[0];
        let v2 = sheet
            .views
            .iter()
            .find(|v| v.key.to_string() == "v2")
            .unwrap();
        // Looking along -x at the half that stays: its cap is 20 × 10 mm.
        assert_eq!(v2.towards, [1.0, 0.0, 0.0]);
        let [lo, hi] = v2.bounds;
        assert!((hi[0] - lo[0] - 20.0).abs() < 1e-9 && (hi[1] - lo[1] - 10.0).abs() < 1e-9);
        let area = hatched(sheet) * section::spacing(sheet.paper);
        assert!((area - 200.0).abs() < 200.0 * 0.05, "{area}");
        // Third angle: it stands right of its parent, on its row.
        let v1 = sheet
            .views
            .iter()
            .find(|v| v.key.to_string() == "v1")
            .unwrap();
        assert_eq!(v1.at[1], v2.at[1]);
        assert!(v2.at[0] > v1.at[0]);
        // The caption, and the cutting line's two letters on the parent.
        let texts: Vec<&str> = sheet
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"SECTION A-A"), "{texts:?}");
        // The letters at the cutting line's ends (5 mm high; the border's
        // zone letters are smaller).
        let letters = sheet
            .items
            .iter()
            .filter(|it| matches!(it, Item::Text { text, size, .. } if text == "A" && *size == 5.0))
            .count();
        assert_eq!(letters, 2);
        // Built again, from the session's cut and projection.
        let again = build(&mut s, &id("p"), &id("d")).unwrap();
        assert_eq!(
            format!("{:?}", again.sheets[0].items),
            format!("{:?}", sheet.items)
        );
    }

    #[test]
    fn an_offset_section_steps_and_hatches_its_facing_caps_only() {
        // Seen from the top, the cut runs along y = 5 from x = 0 to 20, steps
        // to y = 15 and runs on to x = 40: what is left of it (towards -y)
        // goes, and the section looks along +y.
        let views = json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "top", "scale": "1:1"},
            "v2": {"type": "section", "sheet": "s1", "from": "v1", "label": "B", "flip": true,
                   "cut": {"through": [["-1 mm", "5 mm", 0], ["20 mm", "5 mm", 0], ["20 mm", "15 mm", 0], ["41 mm", "15 mm", 0]]}}
        });
        let (_d, mut s) = drawn(40.0, 20.0, 10.0, views);
        let b = build(&mut s, &id("p"), &id("d")).unwrap();
        assert!(b.findings.is_empty(), "{:?}", b.findings);
        let sheet = &b.sheets[0];
        // Two caps facing the view, 20 × 10 each, at the scale of 1.
        let area = hatched(sheet) * section::spacing(sheet.paper);
        assert!((area - 400.0).abs() < 400.0 * 0.05, "{area}");
    }

    #[test]
    fn a_detail_draws_its_parent_inside_its_circle_at_twice_its_scale() {
        let views = json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "front", "scale": "1:1"},
            "v2": {"type": "detail", "sheet": "s1", "from": "v1", "label": "C",
                   "centre": [0, 0, 0], "radius": "5 mm"}
        });
        let (_d, mut s) = drawn(40.0, 20.0, 10.0, views);
        let b = build(&mut s, &id("p"), &id("d")).unwrap();
        assert!(b.findings.is_empty(), "{:?}", b.findings);
        let sheet = &b.sheets[0];
        let v2 = sheet
            .views
            .iter()
            .find(|v| v.key.to_string() == "v2")
            .unwrap();
        assert_eq!(v2.scale.to_string(), "2:1");
        // The block's corner at the circle's centre: two edges of 5 mm at 2:1.
        let total: f64 = visible_in(sheet, v2).iter().map(length).sum();
        assert!((total - 20.0).abs() < 1e-6, "{total}");
        let texts: Vec<&str> = sheet
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"DETAIL C (2:1)"), "{texts:?}");
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
        let places = b.placements(
            match &s.project().part(&id("p")).unwrap().drawings[&id("d")] {
                Ok(f) => f,
                Err(e) => panic!("{e:?}"),
            },
        );
        assert_eq!(places.len(), 4);
        assert!(matches!(
            places[&Key::parse("v2", 'v').unwrap()],
            Placement::Offset(_)
        ));
        // The sheet's title block reads the drawing.
        assert!(
            sheet
                .items
                .iter()
                .any(|it| matches!(it, Item::Text { text, .. } if text == "Block"))
        );
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
                Item::Path {
                    path,
                    stroke: Some(st),
                    ..
                } if !st.dash.is_empty() => Some(length(path)),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| (l - 20.0).abs() < 1e-9), "{lines:?}");
        // The second build is the session's: no kernel call, the same items.
        let again = build(&mut s, &id("p"), &id("on")).unwrap();
        assert_eq!(again.sheets[0].items, on.sheets[0].items);
    }

    /// The name of `part`'s solid `b`'s planar face whose outward normal
    /// is `n`.
    fn face_along(s: &mut Session, n: [f64; 3]) -> String {
        face_along_in(s, "p", n)
    }

    /// [`face_along`] of `part`'s one solid.
    fn face_along_in(s: &mut Session, part: &str, n: [f64; 3]) -> String {
        let v = call(s, "inspect", json!({"part": part})).unwrap();
        for f in v["bodies"][0]["faces"].as_array().unwrap() {
            let z = &f["surface"]["frame"]["z"];
            let k = if f["reversed"] == true { -1.0 } else { 1.0 };
            let d: f64 = (0..3)
                .map(|i| k * z[i].as_f64().unwrap_or(0.0) * n[i])
                .sum();
            if f["surface"]["type"] == "plane" && d > 1.0 - 1e-9 {
                return f["name"].as_str().unwrap().to_owned();
            }
        }
        panic!("no face along {n:?}");
    }

    #[test]
    fn a_back_view_goes_past_the_right_view_and_no_two_views_overlap() {
        // Whichever has the lower key: placement follows reach, not keys.
        for (right, back) in [("v3", "v5"), ("v4", "v3")] {
            back_past_right(right, back);
        }
    }

    fn back_past_right(right: &str, back: &str) {
        let mut views = json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "front"},
            "v2": {"type": "projected", "sheet": "s1", "from": "v1", "side": "top"},
        });
        views[right] = json!({"type": "projected", "sheet": "s1", "from": "v1", "side": "right"});
        views[back] = json!({"type": "projected", "sheet": "s1", "from": "v1", "side": "back"});
        let (_d, mut s) = drawn(60.0, 40.0, 20.0, views);
        let apart = |views: &[ViewBuilt]| {
            for (i, a) in views.iter().enumerate() {
                for b in &views[i + 1..] {
                    assert!(
                        !overlap(a.bounds, b.bounds),
                        "{} {:?} meets {} {:?}",
                        a.key,
                        a.bounds,
                        b.key,
                        b.bounds
                    );
                }
            }
        };
        let b = build(&mut s, &id("p"), &id("d")).unwrap();
        let sheet = &b.sheets[0];
        apart(&sheet.views);
        let at = |k: &str| {
            sheet
                .views
                .iter()
                .find(|v| v.key.to_string() == k)
                .unwrap()
                .at
        };
        // Third angle: the back view on the front's row, past the right.
        assert_eq!(at(back)[1], at("v1")[1]);
        assert!(
            at(back)[0] > at(right)[0],
            "{back} beside the front, {right} past it"
        );
        // `drawing layout` lays them out alike, at a scale that fits.
        let v = call(
            &mut s,
            "drawing.layout",
            json!({"part": "p", "drawing": "d", "dry_run": true}),
        )
        .unwrap();
        let sh = &v["sheets"][0];
        assert_ne!(sh["scale"], "1:1000", "{sh}");
        let boxes: Vec<[P; 2]> = sh["views"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| serde_json::from_value(w["box_mm"].clone()).unwrap())
            .collect();
        for (i, a) in boxes.iter().enumerate() {
            for c in &boxes[i + 1..] {
                assert!(!overlap(*a, *c), "{a:?} meets {c:?}");
            }
        }
    }

    #[test]
    fn a_base_view_of_another_part_looks_normal_to_that_parts_face() {
        let (_d, mut s) = drawn(
            40.0,
            20.0,
            10.0,
            json!({"v1": {"type": "base", "sheet": "s1", "orient": "front"}}),
        );
        call(&mut s, "part.create", json!({"part": "q"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "q", "type": "box", "id": "qb", "params": {"x": 30, "y": 20, "z": 10}}),
        )
        .unwrap();
        let right = face_along_in(&mut s, "q", [1.0, 0.0, 0.0]);
        let file = json!({"format": 1, "kind": "ryucad.drawing", "name": "Other",
            "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b"}},
            "views": {"v1": {"type": "base", "sheet": "s1", "of": "q",
                "orient": {"normal_to": {"feature": "qb", "name": right}}}}});
        crate::doc::project::write_json(&s.root().join("parts/p/drawings/d.json"), &file).unwrap();
        s.reload().unwrap();
        let b = build(&mut s, &id("p"), &id("d")).unwrap();
        let v = &b.sheets[0].views[0];
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!((v.towards[0] - 1.0).abs() < 1e-12, "{:?}", v.towards);
    }

    #[test]
    fn an_auxiliary_view_shows_a_tilted_face_at_its_true_size() {
        // A 40 × 20 × 10 box turned 30° about y: its top face's normal
        // lies in the front view's plane, seen there edge-on.
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let (sn, cs) = ryu_math::sin_cos(core::f64::consts::FRAC_PI_6);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 40, "y": 20, "z": 10},
                   "placement": {"z": [sn, 0, cs], "x": [cs, 0, -sn]}}),
        )
        .unwrap();
        let top = face_along(&mut s, [sn, 0.0, cs]);
        let file = json!({"format": 1, "kind": "ryucad.drawing", "name": "Tilted",
        "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b", "scale": "1:1"}},
        "views": {
            "v1": {"type": "base", "sheet": "s1", "orient": "front"},
            "v2": {"type": "auxiliary", "sheet": "s1", "from": "v1",
                   "normal_to": {"feature": "b", "name": top}}
        }});
        let at = dir.path().join("parts/p/drawings");
        std::fs::create_dir_all(&at).unwrap();
        crate::doc::project::write_json(&at.join("d.json"), &file).unwrap();
        s.reload().unwrap();
        let b = build(&mut s, &id("p"), &id("d")).unwrap();
        let sheet = &b.sheets[0];
        let aux = sheet
            .views
            .iter()
            .find(|v| v.key.to_string() == "v2")
            .unwrap();
        // Seen along its normal, the top face is its own 40 × 20, every
        // edge behind it drawn once, where it lies under the outline.
        let total: f64 = visible_in(sheet, aux).iter().map(length).sum();
        assert!((total - 120.0).abs() < 1e-6, "{total}");
        let [lo, hi] = aux.bounds;
        let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
        assert!(
            (w.max(h) - 40.0).abs() < 1e-6 && (w.min(h) - 20.0).abs() < 1e-6,
            "{w} {h}"
        );
        // It stands off its parent along the face's normal's image: up and
        // to the right, at 30° from the vertical, in third angle.
        let f = sheet
            .views
            .iter()
            .find(|v| v.key.to_string() == "v1")
            .unwrap();
        let (dx, dy) = (aux.at[0] - f.at[0], aux.at[1] - f.at[1]);
        assert!(dx > 0.0 && dy > 0.0, "{dx} {dy}");
        assert!((dx / dy - (sn / cs)).abs() < 1e-9, "{dx} {dy}");
        // A face its parent doesn't see edge-on is refused.
        let side = face_along(&mut s, [0.0, -1.0, 0.0]);
        let mut bad = file.clone();
        bad["views"]["v2"]["normal_to"]["name"] = json!(side);
        crate::doc::project::write_json(&at.join("d.json"), &bad).unwrap();
        s.reload().unwrap();
        let e = build(&mut s, &id("p"), &id("d")).unwrap_err();
        assert_eq!(e.details["reason"], "foreshortened", "{e:?}");
    }

    #[test]
    fn first_angle_puts_the_top_view_below_and_the_right_view_left() {
        let views = json!({
            "v1": {"type": "base", "sheet": "s1", "orient": "front"},
            "v2": {"type": "projected", "sheet": "s1", "from": "v1", "side": "top"},
            "v3": {"type": "projected", "sheet": "s1", "from": "v1", "side": "right"}
        });
        let (dir, mut s) = drawn(40.0, 20.0, 10.0, views.clone());
        let file = json!({"format": 1, "kind": "ryucad.drawing", "name": "Block",
            "standard": "iso",
            "sheets": {"s1": {"name": "Sheet 1", "size": "iso_a3"}}, "views": views});
        crate::doc::project::write_json(&dir.path().join("parts/p/drawings/d.json"), &file)
            .unwrap();
        s.reload().unwrap();
        let b = build(&mut s, &id("p"), &id("d")).unwrap();
        assert_eq!(b.projection, Angle::First);
        let at = |k: &str| {
            b.sheets[0]
                .views
                .iter()
                .find(|v| v.key.to_string() == k)
                .unwrap()
                .at
        };
        let (f, t, r) = (at("v1"), at("v2"), at("v3"));
        assert!(t[1] < f[1] && t[0] == f[0], "{f:?} {t:?}");
        assert!(r[0] < f[0] && r[1] == f[1], "{f:?} {r:?}");
        // Every view lies in the ISO sheet's area, clear of its title block.
        let sheet = &b.sheets[0];
        assert_eq!(sheet.template, "iso-a3");
        let t = template::compiled("iso-a3", Size::IsoA3, Orientation::Landscape);
        for v in &sheet.views {
            let [lo, hi] = v.bounds;
            assert!(
                lo[0] >= t.area[0][0] && hi[1] <= t.area[1][1],
                "{:?}",
                v.bounds
            );
            assert!(!overlap(v.bounds, t.keep_out[0]), "{:?}", v.bounds);
        }
        // And its text is ISO 7200's.
        assert!(
            sheet
                .items
                .iter()
                .any(|it| matches!(it, Item::Text { text, .. } if text == "Document number"))
        );
    }

    #[test]
    fn a_drawing_the_part_hasnt_is_not_found() {
        let (_d, mut s) = drawn(10.0, 10.0, 10.0, json!({}));
        let e = build(&mut s, &id("p"), &id("other")).unwrap_err();
        assert_eq!(e.code, Code::DrawingNotFound);
        assert_eq!(e.details["drawings"], json!(["d"]));
    }
}
