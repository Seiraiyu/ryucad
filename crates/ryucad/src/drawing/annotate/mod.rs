//! A drawing's annotations built against the model (design 417, plan
//! 9.C): each entity resolved by stable name in what its view draws, each
//! value measured from the model's exact geometry, laid out on the sheet
//! and drawn as [`ryu_draw::Item`]s. An annotation whose entity is gone, or
//! not shown, is kept and drawn in the warning colour at its place, with a
//! finding; one that can't be measured (foreshortened, a fit the standard
//! doesn't define) likewise.

pub mod feat;
pub mod text;
pub mod value;

use super::{DIM_ROOM, Drawn, Plan, SheetBuilt, Source, ViewBuilt, WARNING};
use crate::doc::drawing::{DrawingFile, Key, Standard};
use crate::doc::drawing_annotation::{Along, Annotation, Kind, Process, Row, Style};
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use feat::{Feat, Resolved, Shown};
use ryu_draw::symbol::{self, Run, Symbol};
use ryu_draw::{Anchor, Colour, Item, LineKind, P, Path};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use value::{Map, Round};

/// How far apart stacked dimensions stand, in sheet millimetres (design
/// 417).
pub const DIM_GAP: f64 = 8.0;

/// The gap between a feature and its extension line, in millimetres.
pub const EXT_GAP: f64 = 1.5;

/// What a drawing's annotations are built from.
pub(super) struct Context<'a> {
    pub part: &'a Id,
    pub file: &'a DrawingFile,
    pub plans: &'a [(Plan, Source)],
    pub drawn: &'a [Option<&'a Drawn>],
    pub standard: Standard,
    pub style: ryu_draw::Standard,
}

/// What a dimension measured, for the lint: along which way, the ends'
/// coordinates along it (image metres), and its entities.
#[derive(Clone, Debug)]
pub struct Measure {
    pub along: String,
    pub ends: [f64; 2],
    pub refs: Vec<String>,
    /// A reference or basic dimension: no tolerance asked, none counted
    /// in a chain.
    pub free: bool,
    pub toleranced: bool,
    pub value: f64,
}

/// One annotation as built.
#[derive(Clone, Debug)]
pub struct Built {
    pub key: Key,
    pub kind: &'static str,
    pub sheet: Key,
    pub view: Option<Key>,
    /// Its value and tolerance (a dimension's), or what else it says.
    pub report: Value,
    /// What it reads, as one line.
    pub text: String,
    /// Its entities: `{role, reference, kind, shown}`.
    pub entities: Vec<Value>,
    /// Its place on the sheet, and whether ryucad chose it.
    pub at: P,
    pub at_placed: bool,
    /// A dimension's offset, and whether ryucad chose it.
    pub offset: Option<f64>,
    pub offset_placed: bool,
    /// The sheet direction a linear dimension's offset is measured along
    /// (unit): a drag moves its line by the drag along it.
    pub normal: Option<P>,
    /// Its items, as indices into its sheet's.
    pub items: std::ops::Range<usize>,
    pub error: Option<Error>,
    /// Its dimension, extension and leader lines, and its text boxes, on
    /// the sheet: what the lint reads.
    pub lines: Vec<[P; 2]>,
    pub boxes: Vec<[P; 2]>,
    /// The entities it stands on that the view draws only hidden.
    pub hidden: Vec<String>,
    pub measure: Option<Measure>,
    /// A hole callout's hole feature (and its part), for the lint.
    pub hole: Option<(Id, Id)>,
    /// A balloon's occurrence path.
    pub balloon: Option<String>,
}

/// What one annotation drew before it is placed in its sheet.
#[derive(Default)]
struct Out {
    items: Vec<Item>,
    report: Value,
    text: String,
    at: P,
    at_placed: bool,
    offset: Option<f64>,
    offset_placed: bool,
    normal: Option<P>,
    lines: Vec<[P; 2]>,
    boxes: Vec<[P; 2]>,
    measure: Option<Measure>,
    hole: Option<(Id, Id)>,
}

/// A text's size in the drawing's standard: ISO 3.5 mm, ASME 3 mm (0.12
/// in).
fn size(standard: Standard) -> f64 {
    match standard {
        Standard::Iso => 3.5,
        Standard::Asme => 3.0,
    }
}

/// How far an extension line runs past its dimension line.
fn ext_past(standard: Standard) -> f64 {
    match standard {
        Standard::Iso => 2.0,
        Standard::Asme => 3.0,
    }
}

const ARROW: f64 = 3.0;
const ARROW_W: f64 = 1.0;

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn addp(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: P, k: f64) -> P {
    [a[0] * k, a[1] * k]
}

fn dotp(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn unit(a: P) -> P {
    let l = ryu_math::hypot(a[0], a[1]);
    if l > 0.0 {
        [a[0] / l, a[1] / l]
    } else {
        [1.0, 0.0]
    }
}

fn mm(q: &crate::units::Quantity) -> f64 {
    q.meters() * 1000.0
}

/// A line item of `kind`.
fn stroke(style: ryu_draw::Standard, kind: LineKind, pts: &[P], colour: Colour) -> Item {
    let mut s = style.stroke(kind);
    s.colour = colour;
    Item::Path {
        path: Path::polyline(pts, false),
        stroke: Some(s),
        fill: None,
    }
}

/// The text box of `runs` at `at` (on its baseline at `anchor`), at
/// `angle`.
fn text_box(runs: &[Run], at: P, size: f64, angle: f64, anchor: Anchor) -> [P; 2] {
    let w = symbol::width(runs, size);
    let x0 = match anchor {
        Anchor::Start => 0.0,
        Anchor::Middle => -w / 2.0,
        Anchor::End => -w,
    };
    let (asc, desc) = ryu_draw::font::extent(size);
    let (s, c) = ryu_math::sin_cos(angle);
    let corners = [[x0, desc], [x0 + w, desc], [x0 + w, asc], [x0, asc]]
        .map(|q| [at[0] + c * q[0] - s * q[1], at[1] + s * q[0] + c * q[1]]);
    let mut lo = corners[0];
    let mut hi = corners[0];
    for q in corners {
        lo = [lo[0].min(q[0]), lo[1].min(q[1])];
        hi = [hi[0].max(q[0]), hi[1].max(q[1])];
    }
    [lo, hi]
}

/// Builds every annotation of `ctx.file` onto `sheets`, adding findings.
pub(super) fn all(
    session: &mut Session,
    ctx: &Context,
    sheets: &mut [SheetBuilt],
    findings: &mut Vec<Value>,
) -> Result<()> {
    let manifest_unit = session.project().manifest.default_unit;
    let unit = ctx.file.unit.unwrap_or(manifest_unit);
    let units = text::Units {
        unit,
        precision: ctx.file.precision.unwrap_or(match unit {
            crate::units::Unit::In | crate::units::Unit::Ft => 3,
            _ => 2,
        }),
        standard: ctx.standard,
    };
    // Dimensions first (a frame stands under one), frames last.
    let order = |a: &Annotation| match a.kind {
        Kind::FeatureControlFrame { .. } => 2,
        _ if a.style.is_some() => 0,
        _ => 1,
    };
    let mut keys: Vec<(&Key, &Annotation)> = ctx.file.annotations.iter().collect();
    keys.sort_by_key(|(k, a)| (order(a), **k));
    // How many placed dimensions stand on each side of each view.
    let mut stacked: BTreeMap<(Key, i8, i8), usize> = BTreeMap::new();
    // The text boxes of dimensions built, for frames under them.
    let mut dim_boxes: BTreeMap<Key, (Key, [P; 2])> = BTreeMap::new();
    // Notes on a sheet placed by ryucad, per sheet.
    let mut notes_on: BTreeMap<Key, usize> = BTreeMap::new();
    for (key, a) in keys {
        let sheet_key = match (a.view, a.sheet) {
            (Some(v), _) => ctx.file.views[&v].sheet,
            (None, Some(s)) => s,
            (None, None) => unreachable!("read with one"),
        };
        let Some(si) = sheets.iter().position(|s| s.key == sheet_key) else {
            continue;
        };
        let view = a.view.and_then(|v| {
            let pi = ctx.plans.iter().position(|(p, _)| p.key == v)?;
            let vb = sheets[si].views.iter().find(|x| x.key == v)?.clone();
            Some((pi, vb))
        });
        let mut entities = Vec::new();
        let mut hidden = Vec::new();
        let result = build_one(
            session,
            ctx,
            &units,
            *key,
            a,
            view.as_ref(),
            &sheets[si],
            &mut stacked,
            &dim_boxes,
            &mut notes_on,
            &mut entities,
            &mut hidden,
        );
        let first = sheets[si].items.len();
        let built = match result {
            Ok(mut out) => {
                if let Some(b) = out.boxes.first()
                    && a.is_dimension()
                {
                    dim_boxes.insert(*key, (a.view.unwrap_or(*key), *b));
                }
                if a.is_dimension() && !hidden.is_empty() {
                    out.report["hidden"] = json!(hidden);
                }
                sheets[si].items.append(&mut out.items);
                Built {
                    key: *key,
                    kind: a.type_name(),
                    sheet: sheet_key,
                    view: a.view,
                    report: out.report,
                    text: out.text,
                    entities,
                    at: out.at,
                    at_placed: out.at_placed,
                    offset: out.offset,
                    offset_placed: out.offset_placed,
                    normal: out.normal,
                    items: first..first,
                    error: None,
                    lines: out.lines,
                    boxes: out.boxes,
                    hidden,
                    measure: out.measure,
                    hole: out.hole,
                    balloon: match &a.kind {
                        Kind::Balloon { of } => Some(of.clone()),
                        _ => None,
                    },
                }
            }
            Err(e) => {
                let e = e.with("annotation", key.to_string());
                // Kept, and drawn in the warning colour at its place.
                let at = place_of(a, view.as_ref().map(|(_, v)| v), &sheets[si]);
                let label = format!("{key}?");
                let size = size(ctx.standard);
                let mut items = vec![Item::Text {
                    text: label.clone(),
                    at,
                    size,
                    angle: 0.0,
                    anchor: Anchor::Middle,
                    colour: WARNING,
                }];
                sheets[si].items.append(&mut items);
                let severity = "error";
                findings.push(json!({
                    "code": e.code, "severity": severity,
                    "annotation": key.to_string(), "sheet": sheet_key.to_string(),
                    "view": a.view.map(|v| v.to_string()),
                    "message": format!("annotation {key}: {}", e.message),
                    "details": e.details,
                }));
                Built {
                    key: *key,
                    kind: a.type_name(),
                    sheet: sheet_key,
                    view: a.view,
                    report: Value::Null,
                    text: label,
                    entities,
                    at,
                    at_placed: false,
                    offset: None,
                    offset_placed: false,
                    normal: None,
                    items: first..first,
                    error: Some(e),
                    lines: Vec::new(),
                    boxes: vec![text_box(
                        &[Run::Text(format!("{key}?"))],
                        at,
                        size,
                        0.0,
                        Anchor::Middle,
                    )],
                    hidden: Vec::new(),
                    measure: None,
                    hole: None,
                    balloon: None,
                }
            }
        };
        let mut built = built;
        built.items = first..sheets[si].items.len();
        sheets[si].annotations.push(built);
    }
    // The hole features each sheet's views show, by their walls drawn.
    for (pi, (plan, src)) in ctx.plans.iter().enumerate() {
        let Some(d) = ctx.drawn[pi] else { continue };
        let Some(si) = sheets.iter().position(|s| s.key == plan.sheet) else {
            continue;
        };
        for ((b, name), n) in &d.named {
            if !n.visible {
                continue;
            }
            // A face drawn by its silhouettes, or by its edges.
            let body = &src[*b].0.body;
            let mut faces = vec![name.clone()];
            if let Some(ryukei::topo::EntityId::Edge(e)) =
                crate::doc::reference::parse_name(name).and_then(|x| body.by_name(x))
            {
                for fin in body.edge_fins(e) {
                    if let Some(f) = body
                        .fin(fin)
                        .and_then(|f| body.loop_(f.loop_))
                        .and_then(|l| body.face(l.face))
                    {
                        faces.push(f.name.to_string());
                    }
                }
            }
            let Some((name, o)) = faces.iter().find_map(|f| {
                src[*b]
                    .0
                    .origins
                    .get(f)
                    .filter(|o| o["segment"] == "wall")
                    .map(|o| (f.clone(), o))
            }) else {
                continue;
            };
            let name = &name;
            let hole = o["seed"]
                .as_str()
                .filter(|_| o.get("instance").is_some())
                .or(o["feature"].as_str())
                .and_then(|f| Id::parse(f, "feature").ok());
            let Some(hole) = hole else { continue };
            let Ok((part, _)) = part_of(session, plan, src, *b) else {
                continue;
            };
            if sheets[si]
                .holes
                .iter()
                .any(|(p, h, _, _)| *p == part && *h == hole)
            {
                continue;
            }
            let label = src[*b].2.split_once('/').map_or("", |x| x.1);
            sheets[si]
                .holes
                .push((part, hole, plan.key, format!("{label}/{name}")));
        }
    }
    // The general tolerance, above the title block.
    if let Some(g) = &ctx.file.general_tolerance {
        for s in sheets.iter_mut() {
            let size = 2.5;
            let text = match ctx.standard {
                Standard::Asme => format!("UNLESS OTHERWISE SPECIFIED: {g}"),
                Standard::Iso => format!("General tolerances: {g}"),
            };
            let at = match s.keep_out.first() {
                Some([lo, hi]) => [lo[0], hi[1] + 2.0],
                None => [s.area[0][0] + 2.0, s.area[0][1] + 2.0],
            };
            s.items.push(Item::Text {
                text,
                at,
                size,
                angle: 0.0,
                anchor: Anchor::Start,
                colour: Colour::BLACK,
            });
        }
    }
    Ok(())
}

/// Where an annotation that can't be built is drawn: its written place,
/// else below its view, else at the area's lower left.
fn place_of(a: &Annotation, view: Option<&ViewBuilt>, sheet: &SheetBuilt) -> P {
    let w = a.at.as_ref().map(|[x, y]| [mm(x), mm(y)]);
    match (view, w) {
        (Some(v), Some(w)) => addp(v.at, w),
        (Some(v), None) => [v.at[0], v.bounds[0][1] - 4.0],
        (None, Some(w)) => w,
        (None, None) => [sheet.area[0][0] + 10.0, sheet.area[0][1] + 10.0],
    }
}

/// The map of view `pi`, built as `vb`.
fn map_of(ctx: &Context, pi: usize, vb: &ViewBuilt) -> Map {
    let plan = &ctx.plans[pi].0;
    Map {
        towards: plan.frame.towards,
        x: plan.frame.x,
        y: plan.frame.y,
        centre: plan.frame.image(plan.anchor),
        at: vb.at,
        k: vb.scale.mm_per_m(),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_one(
    session: &mut Session,
    ctx: &Context,
    units: &text::Units,
    key: Key,
    a: &Annotation,
    view: Option<&(usize, ViewBuilt)>,
    sheet: &SheetBuilt,
    stacked: &mut BTreeMap<(Key, i8, i8), usize>,
    dim_boxes: &BTreeMap<Key, (Key, [P; 2])>,
    notes_on: &mut BTreeMap<Key, usize>,
    entities: &mut Vec<Value>,
    hidden: &mut Vec<String>,
) -> Result<Out> {
    let written = a.at.as_ref().map(|[x, y]| [mm(x), mm(y)]);
    // Notes and tables on a sheet stand on no view.
    let Some((pi, vb)) = view else {
        return match &a.kind {
            Kind::Note { text, .. } => {
                let n = notes_on.entry(sheet.key).or_default();
                let at = written.unwrap_or([
                    sheet.area[0][0] + 8.0,
                    sheet.area[0][1] + 8.0 + 20.0 * *n as f64,
                ]);
                *n += 1;
                let mut out = note(session, ctx, sheet, text, at, None)?;
                out.at_placed = written.is_none();
                Ok(out)
            }
            Kind::BomTable { .. } => bom_table(session, ctx, a, ctx.part, None, sheet, written),
            _ => unreachable!("read on a view"),
        };
    };
    if let Some(e) = &vb.error {
        return Err(Error::new(
            Code::DependencyFailed,
            format!("view {} failed: {}", vb.key, e.message),
        )
        .with("view", vb.key.to_string()));
    }
    let (plan, src) = &ctx.plans[*pi];
    if matches!(plan.extra, super::Extra::Flat { .. })
        && !matches!(a.kind, Kind::Note { leader: None, .. })
    {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "view {} is a flat pattern: it takes notes, not {}s",
                vb.key,
                a.type_name()
            ),
        )
        .with("reason", "flat_view"));
    }
    let m = map_of(ctx, *pi, vb);
    let named = ctx.drawn[*pi].map(|d| &d.named);
    let mut get = |role: &str, text: &str| -> Result<Resolved> {
        let r = feat::resolve(session, plan, src, named, role, text)?;
        let shown = match r.shown {
            Shown::Visible => "visible",
            Shown::Hidden => "hidden",
            Shown::No => "no",
        };
        entities
            .push(json!({"role": role, "reference": text, "kind": r.feat.kind(), "shown": shown}));
        if r.shown == Shown::Hidden {
            hidden.push(text.to_owned());
        }
        // A detail shows what lies inside its circle.
        if let super::Extra::Detail { centre, radius, .. } = plan.extra {
            let p = m.sheet(r.feat.point());
            let c = m.sheet(centre);
            if r.body.is_some()
                && ryu_math::hypot(p[0] - c[0], p[1] - c[1]) > m.k * radius + 1e-6
                && r.paths.is_empty()
            {
                return Err(Error::new(
                    Code::DrawingReferenceNotShown,
                    format!("{text} lies outside detail {}'s circle", plan.key),
                )
                .with("role", role)
                .with("reference", text)
                .with("reason", "outside_detail"));
            }
        }
        Ok(r)
    };
    let st = ctx.style;
    let sz = size(ctx.standard);
    let rel = |p: P| sub(p, vb.at);
    // The sheet point a leader meets `r` at: the middle of the longest
    // path drawn of it, else its point.
    let meet = |r: &Resolved| -> P {
        let best = r.paths.iter().max_by(|x, y| {
            let l = |p: &Path| {
                let b = p.bounds();
                ryu_math::hypot(b[1][0] - b[0][0], b[1][1] - b[0][1])
            };
            l(x).total_cmp(&l(y))
        });
        match best {
            Some(p) => m.sheet_of(addp(p.middle(), m.centre)),
            None => m.sheet(r.feat.point()),
        }
    };
    match &a.kind {
        Kind::Linear {
            a: ea,
            b: eb,
            along,
        } => {
            let ra = get("a", ea)?;
            let rb = match eb {
                Some(b) => Some(get("b", b)?),
                None => None,
            };
            let sa = value::shape(&m, &ra.feat, "a")?;
            let sb = match &rb {
                Some(r) => Some(value::shape(&m, &r.feat, "b")?),
                None => None,
            };
            let lin = value::linear(&m, &sa, sb.as_ref(), *along)?;
            let style = a.style.as_ref().expect("a dimension");
            let shown = text::shown(units, style, lin.value, false, &[])?;
            let refs: Vec<String> = std::iter::once(ea.clone()).chain(eb.clone()).collect();
            let coords = [dotp(lin.feet[0], lin.dir), dotp(lin.feet[1], lin.dir)];
            let mut out = linear_dim(
                ctx, vb, key, style, &shown, lin.feet, lin.dir, lin.normal, written, stacked,
                lin.value,
            );
            out.measure = Some(measure(
                along.as_str(),
                coords,
                refs,
                style,
                lin.value,
                vb.scale.mm_per_m(),
            ));
            out.report = shown.report.clone();
            out.text = shown.plain();
            Ok(out)
        }
        Kind::Ordinate { datum, of, along } => {
            let rd = get("datum", datum)?;
            let ro = get("of", of)?;
            let sd = value::shape(&m, &rd.feat, "datum")?;
            let so = value::shape(&m, &ro.feat, "of")?;
            let lin = value::linear(&m, &sd, Some(&so), *along)?;
            let style = a.style.as_ref().expect("a dimension");
            let shown = text::shown(units, style, lin.value, false, &[])?;
            let mut out = ordinate(
                ctx,
                vb,
                key,
                style,
                &shown,
                lin.feet[1],
                *along,
                written,
                stacked,
            );
            out.measure = Some(measure(
                &format!("ordinate_{}", along.as_str()),
                [0.0, lin.value],
                vec![datum.clone(), of.clone()],
                style,
                lin.value,
                vb.scale.mm_per_m(),
            ));
            out.report = shown.report.clone();
            out.text = shown.plain();
            Ok(out)
        }
        Kind::Angular {
            a: ea,
            b: eb,
            vertex,
        } => {
            let ra = get("a", ea)?;
            let rb = get("b", eb)?;
            let rv = match vertex {
                Some(v) => Some(get("vertex", v)?),
                None => None,
            };
            let ang = value::angular(&m, &ra.feat, &rb.feat, rv.as_ref().map(|r| &r.feat))?;
            let style = a.style.as_ref().expect("a dimension");
            let shown = text::shown(units, style, ang.value, true, &[])?;
            let mut out = angular_dim(ctx, vb, style, &shown, &ang, written);
            out.measure = Some(measure(
                "angle",
                [0.0, ang.value],
                vec![ea.clone(), eb.clone()],
                style,
                ang.value,
                1.0,
            ));
            out.report = shown.report.clone();
            out.text = shown.plain();
            Ok(out)
        }
        Kind::Radius { of } | Kind::Diameter { of } => {
            let diameter = matches!(a.kind, Kind::Diameter { .. });
            let r = get("of", of)?;
            let round = value::round(&m, &r.feat, diameter)?;
            let style = a.style.as_ref().expect("a dimension");
            let (sphere, rad) = match round {
                Round::Circle { sphere, r, .. } => (sphere, r),
                Round::Side { r, .. } => (false, r),
            };
            let mut lead = Vec::new();
            if sphere {
                lead.push(Run::Text("S".into()));
            }
            if diameter {
                lead.push(Run::Symbol(Symbol::Diameter));
            } else {
                lead.push(Run::Text("R".into()));
            }
            let v = if diameter { 2.0 * rad } else { rad };
            let shown = text::shown(units, style, v, false, &lead)?;
            let mut out = match round {
                Round::Circle {
                    centre, r_sheet, ..
                } => leader_dim(ctx, vb, sheet, &shown, centre, r_sheet, written),
                Round::Side { feet, dir, .. } => linear_dim(
                    ctx,
                    vb,
                    key,
                    style,
                    &shown,
                    feet,
                    dir,
                    [-dir[1], dir[0]],
                    written,
                    stacked,
                    v,
                ),
            };
            // A diameter on a hole's wall gives the hole's size: the lint
            // counts it as calling the hole out.
            if diameter {
                out.hole = hole_wall(session, plan, src, &r);
            }
            out.measure = Some(measure(
                if diameter { "diameter" } else { "radius" },
                [0.0, v],
                vec![of.clone()],
                style,
                v,
                1.0,
            ));
            out.report = shown.report.clone();
            out.text = shown.plain();
            Ok(out)
        }
        Kind::Chamfer { of } => {
            let r = get("of", of)?;
            let runs = chamfer_text(session, plan, src, &r, units, ctx.standard)?;
            let tip = meet(&r);
            let mut out = leader_text(
                ctx,
                vb,
                sheet,
                std::slice::from_ref(&runs),
                tip,
                written,
                None,
                sz,
            );
            out.text = plain(&runs);
            out.report = json!({"text": out.text});
            Ok(out)
        }
        Kind::HoleCallout { of } => {
            let r = get("of", of)?;
            let (lines, hole, report, tip) =
                hole_callout(session, plan, src, &r, &m, units, ctx.standard)?;
            let style = a.style.as_ref().expect("a callout");
            let mut lines = lines;
            if let Some(p) = &style.prefix {
                lines[0].insert(0, Run::Text(format!("{p} ")));
            }
            if let Some(s) = &style.suffix
                && let Some(l) = lines.last_mut()
            {
                l.push(Run::Text(format!(" {s}")));
            }
            let tip = tip.unwrap_or_else(|| meet(&r));
            let mut out = leader_text(ctx, vb, sheet, &lines, tip, written, None, sz);
            out.text = lines.iter().map(|l| plain(l)).collect::<Vec<_>>().join(" ");
            out.report = report;
            out.report["text"] = json!(out.text);
            out.hole = Some(hole);
            Ok(out)
        }
        Kind::CentreMark { of } => {
            let r = get("of", of)?;
            let (c, rs) = match value::round(&m, &r.feat, true)? {
                Round::Circle {
                    centre, r_sheet, ..
                } => (centre, r_sheet),
                Round::Side { .. } => {
                    return Err(Error::new(
                        Code::InvalidParam,
                        "a centre mark is on a circle seen as one; a cylinder seen side on takes a centre line",
                    )
                    .with("reason", "side_on"));
                }
            };
            let e = rs + 3.0;
            let lines = vec![
                [[c[0] - e, c[1]], [c[0] + e, c[1]]],
                [[c[0], c[1] - e], [c[0], c[1] + e]],
            ];
            let items = lines
                .iter()
                .map(|l| stroke(st, LineKind::Centre, l, Colour::BLACK))
                .collect();
            Ok(Out {
                items,
                report: json!({"centre": rel(c)}),
                text: String::new(),
                at: rel(c),
                lines,
                ..Out::default()
            })
        }
        Kind::CentreLine { of, to } => {
            let r = get("of", of)?;
            let (p, q) = match to {
                Some(t) => {
                    let r2 = get("to", t)?;
                    let c = |f: &Feat, w: &str| -> Result<P> {
                        match f {
                            Feat::Circle { c, .. } | Feat::Sphere { c, .. } => Ok(m.sheet(*c)),
                            Feat::Point(p) => Ok(m.sheet(*p)),
                            other => Err(Error::new(
                                Code::InvalidParam,
                                format!(
                                    "{w} is a {}: a centre line between two runs between circles",
                                    other.kind()
                                ),
                            )
                            .with("reason", "not_round")),
                        }
                    };
                    (c(&r.feat, "of")?, c(&r2.feat, "to")?)
                }
                None => match value::shape(&m, &r.feat, "of")? {
                    value::Shape::Line { a, b, .. } if matches!(r.feat, Feat::Round { .. }) => {
                        (m.sheet_of(a), m.sheet_of(b))
                    }
                    _ => {
                        return Err(Error::new(
                            Code::InvalidParam,
                            "a centre line alone is on a cylindrical or conical face seen side on",
                        )
                        .with("reason", "not_side_on"));
                    }
                },
            };
            let d = unit(sub(q, p));
            let (p, q) = (sub(p, scale(d, 3.0)), addp(q, scale(d, 3.0)));
            Ok(Out {
                items: vec![stroke(st, LineKind::Centre, &[p, q], Colour::BLACK)],
                report: json!({"from": rel(p), "to": rel(q)}),
                at: rel(scale(addp(p, q), 0.5)),
                lines: vec![[p, q]],
                ..Out::default()
            })
        }
        Kind::Note { text, leader } => {
            let default = [
                vb.at[0] - (vb.bounds[1][0] - vb.bounds[0][0]) / 2.0,
                vb.bounds[0][1] - 16.0,
            ];
            let at = written.map_or(default, |w| addp(vb.at, w));
            let tip = match leader {
                Some(l) => {
                    let r = get("leader", l)?;
                    Some(meet(&r))
                }
                None => None,
            };
            let mut out = note(session, ctx, sheet, text, at, tip)?;
            out.at = rel(at);
            out.at_placed = written.is_none();
            Ok(out)
        }
        Kind::Balloon { of } => balloon(session, ctx, plan, src, vb, sheet, of, written, &m),
        Kind::BomTable { .. } => {
            // Written relative to the view's centre, as notes are.
            let corner = written.map(|w| addp(vb.at, w));
            bom_table(session, ctx, a, &plan.of, Some(&plan.rev), sheet, corner)
        }
        Kind::FeatureControlFrame {
            characteristic,
            rows,
            on,
            to,
        } => {
            let h = 2.0 * sz;
            let (lower_left, tip, placed) = match (on, to) {
                (Some(e), _) => {
                    let r = get("on", e)?;
                    let tip = meet(&r);
                    let away = unit(sub(tip, vb.at));
                    let d = written.map_or(addp(tip, scale(away, 15.0)), |w| addp(vb.at, w));
                    (d, Some(tip), written.is_none())
                }
                (None, Some(d)) => {
                    let Some((_, b)) = dim_boxes.get(d) else {
                        return Err(Error::new(
                            Code::DependencyFailed,
                            format!("the frame stands under {d}, which couldn't be built"),
                        ));
                    };
                    (
                        [b[0][0], b[0][1] - 1.0 - h * rows.len() as f64],
                        None,
                        false,
                    )
                }
                (None, None) => unreachable!("read with one"),
            };
            let mut out = frame(ctx, units, *characteristic, rows, lower_left, h, tip);
            out.at = rel(lower_left);
            out.at_placed = placed;
            out.report = json!({"characteristic": characteristic.as_str(), "datums":
                rows.iter().flat_map(|r| r.datums.iter().map(|d| d.0.clone())).collect::<Vec<_>>()});
            Ok(out)
        }
        Kind::DatumFeature { letter, on } => {
            let r = get("on", on)?;
            let tip = meet(&r);
            let away = unit(sub(tip, vb.at));
            let at = written.map_or(addp(tip, scale(away, 12.0)), |w| addp(vb.at, w));
            let h = 2.0 * sz;
            let mut items = Vec::new();
            let lo = [at[0] - h / 2.0, at[1] - h / 2.0];
            let hi = [at[0] + h / 2.0, at[1] + h / 2.0];
            items.push(stroke(
                st,
                LineKind::Dimension,
                &[lo, [hi[0], lo[1]], hi, [lo[0], hi[1]], lo],
                Colour::BLACK,
            ));
            items.push(Item::Text {
                text: letter.clone(),
                at: [at[0], at[1] - sz * 0.36],
                size: sz,
                angle: 0.0,
                anchor: Anchor::Middle,
                colour: Colour::BLACK,
            });
            // The leader from the box's side nearest the feature to a
            // filled triangle standing on it.
            let d = unit(sub(at, tip));
            let start = clip_box(at, h / 2.0, scale(d, -1.0));
            let t = 3.0;
            let base_mid = tip;
            let apex = addp(tip, scale(d, t * 0.866));
            let n = [-d[1], d[0]];
            items.push(stroke(
                st,
                LineKind::Dimension,
                &[start, apex],
                Colour::BLACK,
            ));
            items.push(Item::Path {
                path: Path::polyline(
                    &[
                        addp(base_mid, scale(n, t / 2.0)),
                        sub(base_mid, scale(n, t / 2.0)),
                        apex,
                    ],
                    true,
                ),
                stroke: None,
                fill: Some(Colour::BLACK),
            });
            Ok(Out {
                items,
                report: json!({"letter": letter}),
                text: letter.clone(),
                at: rel(at),
                at_placed: written.is_none(),
                lines: vec![[start, apex]],
                boxes: vec![[lo, hi]],
                ..Out::default()
            })
        }
        Kind::SurfaceFinish { on, ra, process } => {
            let r = get("on", on)?;
            let tip0 = meet(&r);
            let tip = written.map_or(tip0, |w| addp(vb.at, w));
            let mut out = finish(ctx, units, tip, ra.as_ref().map(|q| q.si()), *process);
            if written.is_some() && ryu_math::hypot(tip[0] - tip0[0], tip[1] - tip0[1]) > 1e-6 {
                out.items
                    .push(stroke(st, LineKind::Dimension, &[tip, tip0], Colour::BLACK));
                out.items.push(symbol::arrow(
                    tip0,
                    unit(sub(tip0, tip)),
                    ARROW,
                    ARROW_W,
                    Colour::BLACK,
                ));
                out.lines.push([tip, tip0]);
            }
            out.at = rel(tip);
            out.at_placed = written.is_none();
            Ok(out)
        }
    }
}

/// A dimension's measure for the lint.
fn measure(
    along: &str,
    ends: [f64; 2],
    mut refs: Vec<String>,
    style: &Style,
    value: f64,
    _k: f64,
) -> Measure {
    use crate::doc::drawing_annotation::Tolerance;
    refs.sort();
    Measure {
        along: along.to_owned(),
        ends,
        refs,
        free: matches!(
            style.tolerance,
            Some(Tolerance::Reference | Tolerance::Basic)
        ),
        toleranced: style.tolerance.is_some(),
        value,
    }
}

fn plain(runs: &[Run]) -> String {
    text::Shown {
        runs: runs.to_vec(),
        ..text::Shown::default()
    }
    .plain()
}

/// Where a ray from a box's centre `c` along unit `d` leaves the box of
/// half-size `h`.
fn clip_box(c: P, h: f64, d: P) -> P {
    let t = (h / d[0].abs().max(1e-12)).min(h / d[1].abs().max(1e-12));
    addp(c, scale(d, t))
}

/// Draws a dimension's text `shown` with its baseline's middle at `at`
/// along unit `along` (ISO: along it, reading from below or the right;
/// ASME: level), the text `lift` above its line. Its items and text box.
fn dim_text(
    ctx: &Context,
    shown: &text::Shown,
    mid: P,
    along: P,
    lift: f64,
) -> (Vec<Item>, [P; 2]) {
    let sz = size(ctx.standard);
    let mut angle = match ctx.standard {
        Standard::Asme => 0.0,
        Standard::Iso => ryu_math::atan2(along[1], along[0]),
    };
    // Read from below or from the right.
    if angle > std::f64::consts::FRAC_PI_2 + 1e-9 {
        angle -= std::f64::consts::PI;
    } else if angle <= -std::f64::consts::FRAC_PI_2 + 1e-9 {
        angle += std::f64::consts::PI;
    }
    let (s, c) = ryu_math::sin_cos(angle);
    let up = [-s, c];
    let base = match ctx.standard {
        Standard::Iso => addp(mid, scale(up, lift)),
        // ASME: centred on the line, which is broken round it.
        Standard::Asme => sub(mid, [0.0, sz * 0.36]),
    };
    let stroke = ctx.style.stroke(LineKind::Dimension);
    let mut items = symbol::draw(
        &shown.runs,
        base,
        sz,
        angle,
        Anchor::Middle,
        Colour::BLACK,
        &stroke,
    );
    let mut bx = text_box(&shown.runs, base, sz, angle, Anchor::Middle);
    let w = symbol::width(&shown.runs, sz);
    if let Some((u, l)) = &shown.stack {
        let small = sz * 0.7;
        let x = addp(base, [c * (w / 2.0 + 0.8), s * (w / 2.0 + 0.8)]);
        for (t, dy) in [(u, small * 0.55), (l, -small * 0.75)] {
            let at = addp(x, scale(up, dy));
            items.push(Item::Text {
                text: t.clone(),
                at,
                size: small,
                angle,
                anchor: Anchor::Start,
                colour: Colour::BLACK,
            });
            let b = text_box(&[Run::Text(t.clone())], at, small, angle, Anchor::Start);
            bx = [
                [bx[0][0].min(b[0][0]), bx[0][1].min(b[0][1])],
                [bx[1][0].max(b[1][0]), bx[1][1].max(b[1][1])],
            ];
        }
    }
    if shown.boxed {
        let [lo, hi] = bx;
        let (lo, hi) = ([lo[0] - 1.0, lo[1] - 1.0], [hi[0] + 1.0, hi[1] + 1.0]);
        items.push(Item::Path {
            path: Path::polyline(&[lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]], true),
            stroke: Some(ctx.style.stroke(LineKind::Dimension)),
            fill: None,
        });
        bx = [lo, hi];
    }
    (items, bx)
}

/// A linear dimension between `feet` on the sheet, measured along unit
/// `dir`, its line standing along `normal` from them by its offset.
#[allow(clippy::too_many_arguments)]
fn linear_dim(
    ctx: &Context,
    vb: &ViewBuilt,
    _key: Key,
    style: &Style,
    shown: &text::Shown,
    feet: [P; 2],
    dir: P,
    normal: P,
    written: Option<P>,
    stacked: &mut BTreeMap<(Key, i8, i8), usize>,
    _value: f64,
) -> Out {
    let st = ctx.style;
    let sz = size(ctx.standard);
    // Which side: the written offset's sign, else the side of the view's
    // centre the feet lie on.
    let reach = |s: f64| -> f64 {
        // How far along `s * normal` the view's box reaches past the feet.
        let corners = [
            vb.bounds[0],
            [vb.bounds[1][0], vb.bounds[0][1]],
            vb.bounds[1],
            [vb.bounds[0][0], vb.bounds[1][1]],
        ];
        let box_most = corners
            .iter()
            .map(|c| s * dotp(*c, normal))
            .fold(f64::NEG_INFINITY, f64::max);
        let feet_most = feet
            .iter()
            .map(|f| s * dotp(*f, normal))
            .fold(f64::NEG_INFINITY, f64::max);
        (box_most - feet_most).max(0.0)
    };
    let (offset, placed) = match &style.offset {
        Some(o) => (mm(o), false),
        None => {
            let mid = scale(addp(feet[0], feet[1]), 0.5);
            let side = if dotp(sub(mid, vb.at), normal) >= 0.0 {
                1.0
            } else {
                -1.0
            };
            let slot = stacked
                .entry((
                    vb.key,
                    (normal[0].round() as i8) * side as i8,
                    (normal[1].round() as i8) * side as i8,
                ))
                .or_default();
            let o = side * (reach(side) + DIM_GAP * (*slot as f64 + 1.0));
            *slot += 1;
            ((o * 1000.0).round() / 1000.0, true)
        }
    };
    let s = if offset >= 0.0 { 1.0 } else { -1.0 };
    let line_at = feet
        .iter()
        .map(|f| s * dotp(*f, normal))
        .fold(f64::NEG_INFINITY, f64::max)
        * s
        + offset;
    let d = feet.map(|f| addp(f, scale(normal, line_at - dotp(f, normal))));
    let mut items = Vec::new();
    let mut lines = Vec::new();
    // Extension lines, from a gap off each foot to past the line.
    for (f, p) in feet.iter().zip(d.iter()) {
        let run = dotp(sub(*p, *f), normal) * s;
        if run > EXT_GAP {
            let a = addp(*f, scale(normal, s * EXT_GAP));
            let b = addp(*p, scale(normal, s * ext_past(ctx.standard)));
            items.push(stroke(st, LineKind::Dimension, &[a, b], Colour::BLACK));
            lines.push([a, b]);
        }
    }
    let span = sub(d[1], d[0]);
    let len = ryu_math::hypot(span[0], span[1]);
    let u = if len > 0.0 {
        scale(span, 1.0 / len)
    } else {
        dir
    };
    let tw = symbol::width(&shown.runs, sz);
    // Arrows inside where the text and both fit, else outside.
    let inside = len >= tw + 2.0 * ARROW + 2.0;
    let mid = match written {
        Some(w) => {
            let p = addp(vb.at, w);
            let t = dotp(sub(p, d[0]), u);
            addp(d[0], scale(u, t))
        }
        None if inside => scale(addp(d[0], d[1]), 0.5),
        None => addp(d[1], scale(u, ARROW + 2.0 + tw / 2.0)),
    };
    let lift = 1.0;
    let (text_items, tb) = dim_text(ctx, shown, mid, u, lift);
    // The dimension line, broken round ASME's level text.
    let mut segs: Vec<[P; 2]> = Vec::new();
    let (a0, a1) = if inside {
        (d[0], d[1])
    } else {
        (
            sub(d[0], scale(u, ARROW + 3.0)),
            addp(d[1], scale(u, ARROW + 3.0)),
        )
    };
    // Run on to the text when it stands outside.
    let (lo_t, hi_t) = {
        let t = dotp(sub(mid, d[0]), u);
        (t - tw / 2.0 - 1.0, t + tw / 2.0 + 1.0)
    };
    let (mut t0, mut t1) = (dotp(sub(a0, d[0]), u), dotp(sub(a1, d[0]), u));
    t0 = t0.min(lo_t + 1.0).min(0.0);
    t1 = t1.max(hi_t - 1.0).max(len);
    if ctx.standard == Standard::Asme {
        if lo_t > t0 {
            segs.push([addp(d[0], scale(u, t0)), addp(d[0], scale(u, lo_t))]);
        }
        if hi_t < t1 {
            segs.push([addp(d[0], scale(u, hi_t)), addp(d[0], scale(u, t1))]);
        }
    } else {
        segs.push([addp(d[0], scale(u, t0)), addp(d[0], scale(u, t1))]);
    }
    for sgm in &segs {
        items.push(stroke(st, LineKind::Dimension, sgm, Colour::BLACK));
        lines.push(*sgm);
    }
    let (in0, in1) = if inside {
        (scale(u, -1.0), u)
    } else {
        (u, scale(u, -1.0))
    };
    items.push(symbol::arrow(d[0], in0, ARROW, ARROW_W, Colour::BLACK));
    items.push(symbol::arrow(d[1], in1, ARROW, ARROW_W, Colour::BLACK));
    items.extend(text_items);
    Out {
        items,
        at: sub(mid, vb.at),
        at_placed: written.is_none(),
        offset: Some(offset),
        offset_placed: placed,
        normal: Some(normal),
        lines,
        boxes: vec![tb],
        ..Out::default()
    }
}

/// An ordinate dimension: a leader from `foot` square to its measuring
/// direction, to its offset, with the value at its end.
#[allow(clippy::too_many_arguments)]
fn ordinate(
    ctx: &Context,
    vb: &ViewBuilt,
    _key: Key,
    style: &Style,
    shown: &text::Shown,
    foot: P,
    along: Along,
    written: Option<P>,
    stacked: &mut BTreeMap<(Key, i8, i8), usize>,
) -> Out {
    let st = ctx.style;
    // A horizontal ordinate's leader runs up (or down) to above the view;
    // a vertical one's right (or left).
    let n: P = if along == Along::Horizontal {
        [0.0, 1.0]
    } else {
        [1.0, 0.0]
    };
    let (offset, placed) = match &style.offset {
        Some(o) => (mm(o), false),
        None => {
            let edge = if along == Along::Horizontal {
                vb.bounds[1][1]
            } else {
                vb.bounds[1][0]
            };
            let slot = stacked.entry((vb.key, 9, 9)).or_default();
            let _ = slot;
            (
                ((edge - dotp(foot, n) + DIM_GAP) * 1000.0).round() / 1000.0,
                true,
            )
        }
    };
    // A written place moves the text along the leader (its part across
    // the leader isn't the ordinate's to change), and the leader runs on
    // to it, as a linear dimension's text slides along its line.
    let (offset, placed) = match written {
        Some(w) => {
            let t = dotp(sub(addp(vb.at, w), foot), n);
            let s = if t >= 0.0 { 1.0 } else { -1.0 };
            let o = t - s * 1.0;
            if style.offset.is_some() {
                (o, false)
            } else {
                ((o * 1000.0).round() / 1000.0, true)
            }
        }
        None => (offset, placed),
    };
    let s = if offset >= 0.0 { 1.0 } else { -1.0 };
    let a = addp(foot, scale(n, s * EXT_GAP));
    let b = addp(foot, scale(n, offset));
    let mut items = vec![stroke(st, LineKind::Dimension, &[a, b], Colour::BLACK)];
    let angle = if along == Along::Horizontal {
        std::f64::consts::FRAC_PI_2
    } else {
        0.0
    };
    let sz = size(ctx.standard);
    let at = addp(b, scale(n, s * 1.0));
    let at = [
        at[0]
            + if along == Along::Horizontal {
                sz * 0.36
            } else {
                0.0
            },
        at[1]
            - if along == Along::Horizontal {
                0.0
            } else {
                sz * 0.36
            },
    ];
    let anchor = if s > 0.0 { Anchor::Start } else { Anchor::End };
    let stroke_d = st.stroke(LineKind::Dimension);
    items.extend(symbol::draw(
        &shown.runs,
        at,
        sz,
        angle,
        anchor,
        Colour::BLACK,
        &stroke_d,
    ));
    let tb = text_box(&shown.runs, at, sz, angle, anchor);
    Out {
        items,
        at: sub(at, vb.at),
        at_placed: written.is_none(),
        offset: Some(offset),
        offset_placed: placed,
        lines: vec![[a, b]],
        boxes: vec![tb],
        ..Out::default()
    }
}

/// An angular dimension: an arc about its vertex between its rays, at its
/// offset (else 60 % of the shorter edge, 10 mm at least).
fn angular_dim(
    ctx: &Context,
    vb: &ViewBuilt,
    style: &Style,
    shown: &text::Shown,
    a: &value::Angular,
    written: Option<P>,
) -> Out {
    let st = ctx.style;
    let (rho, placed) = match &style.offset {
        Some(o) => (mm(o).abs(), false),
        None => ((0.6 * a.reach[0].min(a.reach[1])).max(10.0), true),
    };
    let v = a.vertex;
    let t0 = ryu_math::atan2(a.rays[0][1], a.rays[0][0]);
    let mut sweep = ryu_math::atan2(a.rays[1][1], a.rays[1][0]) - t0;
    while sweep > std::f64::consts::PI {
        sweep -= std::f64::consts::TAU;
    }
    while sweep < -std::f64::consts::PI {
        sweep += std::f64::consts::TAU;
    }
    let (s0, c0) = ryu_math::sin_cos(t0);
    let start = [v[0] + rho * c0, v[1] + rho * s0];
    let arc = Path {
        start,
        segments: vec![ryu_draw::Segment::Arc {
            centre: v,
            radius: rho,
            from: t0,
            sweep,
        }],
        closed: false,
    };
    let mut items = vec![Item::Path {
        path: arc,
        stroke: Some(st.stroke(LineKind::Dimension)),
        fill: None,
    }];
    let mut lines = Vec::new();
    // Extension lines where the arc lies past an edge.
    for (i, r) in a.rays.iter().enumerate() {
        if rho > a.reach[i] + EXT_GAP {
            let p = addp(v, scale(*r, a.reach[i] + EXT_GAP));
            let q = addp(v, scale(*r, rho + ext_past(ctx.standard)));
            items.push(stroke(st, LineKind::Dimension, &[p, q], Colour::BLACK));
            lines.push([p, q]);
        }
    }
    // Arrows tangent to the arc at its ends, pointing out of it.
    let sg = sweep.signum();
    let tan = |t: f64, out: f64| {
        let (s, c) = ryu_math::sin_cos(t);
        [-s * out, c * out]
    };
    let (s1, c1) = ryu_math::sin_cos(t0 + sweep);
    let end = [v[0] + rho * c1, v[1] + rho * s1];
    items.push(symbol::arrow(
        start,
        tan(t0, -sg),
        ARROW,
        ARROW_W,
        Colour::BLACK,
    ));
    items.push(symbol::arrow(
        end,
        tan(t0 + sweep, sg),
        ARROW,
        ARROW_W,
        Colour::BLACK,
    ));
    let mid_t = t0 + sweep / 2.0;
    let (sm, cm) = ryu_math::sin_cos(mid_t);
    let mid = match written {
        Some(w) => addp(vb.at, w),
        None => [v[0] + (rho + 1.0) * cm, v[1] + (rho + 1.0) * sm],
    };
    let along = [-sm, cm];
    let (ti, tb) = dim_text(ctx, shown, mid, along, 0.5);
    items.extend(ti);
    // The arc, as chords, for the lint.
    for i in 0..8 {
        let (ta, tb2) = (
            t0 + sweep * i as f64 / 8.0,
            t0 + sweep * (i + 1) as f64 / 8.0,
        );
        let p = |t: f64| {
            let (s, c) = ryu_math::sin_cos(t);
            [v[0] + rho * c, v[1] + rho * s]
        };
        lines.push([p(ta), p(tb2)]);
    }
    Out {
        items,
        at: sub(mid, vb.at),
        at_placed: written.is_none(),
        offset: Some(rho),
        offset_placed: placed,
        lines,
        boxes: vec![tb],
        ..Out::default()
    }
}

/// A radius or diameter on a circle seen as one: a leader from outside
/// to the circle, its arrow on the circle, its text past a shoulder.
fn leader_dim(
    ctx: &Context,
    vb: &ViewBuilt,
    sheet: &SheetBuilt,
    shown: &text::Shown,
    centre: P,
    r: f64,
    written: Option<P>,
) -> Out {
    let sz = size(ctx.standard);
    let w = match written {
        Some(w) => unit(sub(addp(vb.at, w), centre)),
        None => [
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
        ],
    };
    let tip = addp(centre, scale(w, r));
    let first = addp(centre, scale(w, r + 10.0));
    leader_text(
        ctx,
        vb,
        sheet,
        std::slice::from_ref(&shown.runs),
        tip,
        written,
        Some(first),
        sz,
    )
}

/// Lines of text past a shoulder at `written` (relative to the view's
/// centre); else at `first`, or 12 mm out and up from `tip`, moved clear
/// of the sheet's lines where that isn't (#662). A leader goes to `tip`,
/// ending in an arrow.
#[allow(clippy::too_many_arguments)]
fn leader_text(
    ctx: &Context,
    vb: &ViewBuilt,
    sheet: &SheetBuilt,
    lines: &[Vec<Run>],
    tip: P,
    written: Option<P>,
    first: Option<P>,
    sz: f64,
) -> Out {
    let st = ctx.style;
    let away = unit(sub(tip, vb.at));
    let away = if away[0].abs() < 1e-9 && away[1].abs() < 1e-9 {
        [1.0, 1.0]
    } else {
        away
    };
    let t = match written {
        Some(w) => addp(vb.at, w),
        None => {
            let first = first.unwrap_or_else(|| {
                addp(
                    tip,
                    [
                        12.0 * away[0].signum().max(-1.0),
                        12.0 * if away[1] >= 0.0 { 1.0 } else { -1.0 },
                    ],
                )
            });
            leader_place(sheet, vb, tip, first, |t| laid(st, lines, tip, t, sz))
        }
    };
    let (items, boxes, segs) = laid(st, lines, tip, t, sz);
    Out {
        items,
        at: sub(t, vb.at),
        at_placed: written.is_none(),
        lines: segs,
        boxes,
        ..Out::default()
    }
}

/// A leader from `tip` to a shoulder at `t`, `lines` of text past it: its
/// items, the text's boxes, and its two segments.
fn laid(
    st: ryu_draw::Standard,
    lines: &[Vec<Run>],
    tip: P,
    t: P,
    sz: f64,
) -> (Vec<Item>, Vec<[P; 2]>, Vec<[P; 2]>) {
    let right = t[0] >= tip[0];
    let shoulder_end = addp(t, [if right { 3.0 } else { -3.0 }, 0.0]);
    let mut items = vec![stroke(
        st,
        LineKind::Dimension,
        &[tip, t, shoulder_end],
        Colour::BLACK,
    )];
    items.push(symbol::arrow(
        tip,
        unit(sub(tip, t)),
        ARROW,
        ARROW_W,
        Colour::BLACK,
    ));
    let anchor = if right { Anchor::Start } else { Anchor::End };
    let x = addp(shoulder_end, [if right { 1.0 } else { -1.0 }, 0.0]);
    let stroke_d = st.stroke(LineKind::Dimension);
    let mut boxes = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let at = [x[0], x[1] - sz * 0.36 - 1.6 * sz * i as f64];
        items.extend(symbol::draw(
            l,
            at,
            sz,
            0.0,
            anchor,
            Colour::BLACK,
            &stroke_d,
        ));
        boxes.push(text_box(l, at, sz, 0.0, anchor));
    }
    (items, boxes, vec![[tip, t], [t, shoulder_end]])
}

/// Where text on a leader from `tip` goes when its annotation says nothing
/// (#662): `first`, where it always went, when clear; else the nearest
/// place turned from it by 10° steps to 80° either way and on by 4 mm,
/// until 20 mm past the view's box, whose text and leader are clear; else
/// `first`.
fn leader_place(
    sheet: &SheetBuilt,
    vb: &ViewBuilt,
    tip: P,
    first: P,
    lay: impl Fn(P) -> (Vec<Item>, Vec<[P; 2]>, Vec<[P; 2]>),
) -> P {
    let room = Clearance::of(sheet);
    let ok = |t: P| {
        let (_, boxes, segs) = lay(t);
        room.clear(&boxes, &segs)
    };
    let d0 = sub(first, tip);
    let start = ryu_math::hypot(d0[0], d0[1]);
    if start < 1e-9 || ok(first) {
        return first;
    }
    let dir = scale(d0, 1.0 / start);
    let [lo, hi] = vb.bounds;
    let reach = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
        .iter()
        .map(|q| ryu_math::hypot(q[0] - tip[0], q[1] - tip[1]))
        .fold(0.0, f64::max)
        + 20.0;
    let mut d = start;
    while d <= reach.max(start) {
        for k in 0..=16 {
            // 0, +10°, -10°, +20°, … ±80°.
            let turn =
                f64::from((k + 1) / 2) * 10.0f64.to_radians() * if k % 2 == 1 { 1.0 } else { -1.0 };
            let (s, co) = ryu_math::sin_cos(turn);
            let t = addp(
                tip,
                scale([co * dir[0] - s * dir[1], s * dir[0] + co * dir[1]], d),
            );
            if ok(t) {
                return t;
            }
        }
        d += 4.0;
    }
    first
}

/// The part (and its revision) whose solid `prefix` (`<solid>`, or
/// `<instance>/…/<solid>`) names, in `plan`'s view.
fn part_of(
    session: &mut Session,
    plan: &Plan,
    src: &Source,
    body: usize,
) -> Result<(Id, crate::doc::assembly::Rev)> {
    let label = &src[body].2;
    let rest = label.split_once('/').map_or("", |x| x.1);
    let steps: Vec<&str> = rest.split('/').collect();
    if steps.len() <= 1 {
        return Ok((plan.of.clone(), plan.rev.clone()));
    }
    let model = session.model(&plan.of, &plan.rev)?;
    let path: Vec<String> = steps[..steps.len() - 1]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    model
        .occurrences
        .iter()
        .find(|o| o.path.iter().map(Id::to_string).collect::<Vec<_>>() == path)
        .map(|o| (o.part.clone(), o.rev.clone()))
        .ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("no occurrence at {}", path.join("/")),
            )
        })
}

/// A face's origin: the feature that made it, as its solid's origins say.
/// The hole feature (and its part) whose wall `r` is, if it is one.
fn hole_wall(session: &mut Session, plan: &Plan, src: &Source, r: &Resolved) -> Option<(Id, Id)> {
    let o = origin_of(src, r)?;
    if o["segment"] != "wall" {
        return None;
    }
    let hid = o["seed"]
        .as_str()
        .filter(|_| o.get("instance").is_some())
        .or(o["feature"].as_str())
        .and_then(|f| Id::parse(f, "feature").ok())?;
    let (pid, _) = part_of(session, plan, src, r.body?).ok()?;
    Some((pid, hid))
}

fn origin_of(src: &Source, r: &Resolved) -> Option<Value> {
    let (b, n) = (r.body?, r.name.as_ref()?);
    src[b].0.origins.get(n).cloned()
}

fn chamfer_text(
    session: &mut Session,
    plan: &Plan,
    src: &Source,
    r: &Resolved,
    units: &text::Units,
    standard: Standard,
) -> Result<Vec<Run>> {
    use crate::doc::blend::ChamferForm;
    let not = || {
        Error::new(
            Code::InvalidParam,
            format!("{} is no chamfer's face", r.text),
        )
        .with("reason", "not_a_chamfer")
    };
    let o = origin_of(src, r).ok_or_else(not)?;
    let fid = o["feature"]
        .as_str()
        .and_then(|f| Id::parse(f, "feature").ok())
        .ok_or_else(not)?;
    let (pid, rev) = part_of(session, plan, src, r.body.ok_or_else(not)?)?;
    let (part, _, _) = session.node(&pid, &rev)?;
    let Some(Ok(crate::doc::files::Feature::Chamfer(c))) = part.features.get(&fid) else {
        return Err(not());
    };
    let replaced: Vec<&str> = o["replaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let group = c
        .groups
        .iter()
        .find(|g| {
            g.edges
                .iter()
                .any(|e| replaced.contains(&e.name.to_string().as_str()))
        })
        .or(c.groups.first())
        .ok_or_else(not)?;
    let x = if standard == Standard::Asme {
        " X "
    } else {
        " × "
    };
    let len = |q: &crate::units::Quantity| {
        let v = q.meters();
        units.number(
            v,
            units.precision.max(text::needs(v / units.unit.factor())),
            false,
        )
    };
    let deg = |q: &crate::units::Quantity| {
        format!(
            "{}°",
            text::decimal(
                q.radians().to_degrees(),
                text::needs(q.radians().to_degrees()).min(2),
                false
            )
        )
    };
    Ok(vec![Run::Text(match &group.form {
        ChamferForm::Equal { d } => format!("{}{x}45°", len(d)),
        ChamferForm::Distances { d1, d2, .. } => format!("{}{x}{}", len(d1), len(d2)),
        ChamferForm::DistanceAngle { d, angle, .. } => format!("{}{x}{}", len(d), deg(angle)),
    })])
}

/// A hole callout: its lines, its hole (part, feature), what it reports,
/// and where its leader meets the hole (on its circle seen end on).
#[allow(clippy::type_complexity)]
fn hole_callout(
    session: &mut Session,
    plan: &Plan,
    src: &Source,
    r: &Resolved,
    m: &Map,
    units: &text::Units,
    standard: Standard,
) -> Result<(Vec<Vec<Run>>, (Id, Id), Value, Option<P>)> {
    use crate::doc::hole::{Depth, HoleForm};
    let not = || {
        Error::new(Code::InvalidParam, format!("{} is no hole's face", r.text))
            .with("reason", "not_a_hole")
    };
    let o = origin_of(src, r).ok_or_else(not)?;
    let hid = o["seed"]
        .as_str()
        .filter(|_| o.get("instance").is_some())
        .or(o["feature"].as_str())
        .and_then(|f| Id::parse(f, "feature").ok())
        .ok_or_else(not)?;
    let body = r.body.ok_or_else(not)?;
    let (pid, rev) = part_of(session, plan, src, body)?;
    let (part, _, _) = session.node(&pid, &rev)?;
    let Some(Ok(crate::doc::files::Feature::Hole(h))) = part.features.get(&hid) else {
        return Err(not());
    };
    // How many of its bores the solid has: walls by hole, instance, point.
    let mut bores = std::collections::BTreeSet::new();
    for (_, origin) in src[body].0.origins.iter() {
        if origin["segment"] != "wall" {
            continue;
        }
        let of_hole = origin["feature"] == hid.as_str() || origin["seed"] == hid.as_str();
        if of_hole {
            bores.insert((
                origin["feature"].to_string(),
                origin["instance"].to_string(),
                origin["point"].to_string(),
            ));
        }
    }
    let n = bores.len().max(1);
    let x = if standard == Standard::Asme {
        "X"
    } else {
        "×"
    };
    let len = |v: f64| {
        units.number(
            v,
            units.precision.max(text::needs(v / units.unit.factor())),
            false,
        )
    };
    let mut first = Vec::new();
    if n > 1 {
        first.push(Run::Text(format!("{n}{x} ")));
    }
    first.push(Run::Symbol(Symbol::Diameter));
    first.push(Run::Text(len(h.diameter.meters())));
    let depth = match &h.depth {
        Depth::ThroughAll => Some("THRU".to_owned()),
        Depth::Blind(d) => Some(len(d.meters())),
        Depth::UpTo { .. } => None,
    };
    match &h.depth {
        Depth::ThroughAll => first.push(Run::Text(" THRU".into())),
        Depth::Blind(_) => {
            first.push(Run::Text(" ".into()));
            first.push(Run::Symbol(Symbol::Depth));
            first.push(Run::Text(depth.clone().unwrap_or_default()));
        }
        Depth::UpTo { .. } => {}
    }
    let mut lines = vec![first];
    match &h.form {
        HoleForm::Simple => {}
        HoleForm::Counterbore { diameter, depth } => lines.push(vec![
            Run::Symbol(Symbol::Counterbore),
            Run::Text(" ".into()),
            Run::Symbol(Symbol::Diameter),
            Run::Text(format!("{} ", len(diameter.meters()))),
            Run::Symbol(Symbol::Depth),
            Run::Text(len(depth.meters())),
        ]),
        HoleForm::Countersink { diameter, angle } => lines.push(vec![
            Run::Symbol(Symbol::Countersink),
            Run::Text(" ".into()),
            Run::Symbol(Symbol::Diameter),
            Run::Text(format!(
                "{} {x} {}°",
                len(diameter.meters()),
                text::decimal(angle.radians().to_degrees(), 0, false)
            )),
        ]),
    }
    // The bore's circle seen end on: the leader meets it at 45°.
    let tip = match &r.feat {
        Feat::Round {
            o, d, r: Some(rr), ..
        } if m.end_on(*d) => {
            let c = m.sheet(*o);
            let w = std::f64::consts::FRAC_1_SQRT_2;
            Some([c[0] + w * m.k * rr, c[1] + w * m.k * rr])
        }
        _ => None,
    };
    let report = json!({
        "hole": hid, "part": pid, "count": n, "form": h.form.type_name(),
        "diameter": h.diameter.meters(), "depth": h.depth.type_name(),
    });
    Ok((lines, (pid, hid), report, tip))
}

/// A note: its lines (fields filled as the title block's) at `at`, the
/// first line's baseline start, with a leader to `tip`.
fn note(
    session: &mut Session,
    ctx: &Context,
    sheet: &SheetBuilt,
    text: &str,
    at: P,
    tip: Option<P>,
) -> Result<Out> {
    let sz = size(ctx.standard);
    let number = 1 + ctx
        .file
        .sheets
        .keys()
        .position(|k| *k == sheet.key)
        .unwrap_or(0);
    let values = super::fields(
        session,
        ctx.part,
        ctx.file,
        &ctx.file.sheets[&sheet.key],
        sheet.scale,
        number,
    );
    let pieces = crate::doc::drawing_template::parse_fields(text)
        .map_err(|e| Error::new(Code::InvalidParam, e))?;
    let filled = crate::doc::drawing_template::fill(&pieces, &|n| values.get(n).cloned());
    let mut items = Vec::new();
    let mut boxes = Vec::new();
    for (i, l) in filled.split('\n').enumerate() {
        let p = [at[0], at[1] - 1.6 * sz * i as f64];
        items.push(Item::Text {
            text: l.to_owned(),
            at: p,
            size: sz,
            angle: 0.0,
            anchor: Anchor::Start,
            colour: Colour::BLACK,
        });
        boxes.push(text_box(
            &[Run::Text(l.to_owned())],
            p,
            sz,
            0.0,
            Anchor::Start,
        ));
    }
    let mut lines = Vec::new();
    if let Some(tip) = tip {
        let from = [at[0] - 1.0, at[1] + sz * 0.36];
        items.push(stroke(
            ctx.style,
            LineKind::Dimension,
            &[from, tip],
            Colour::BLACK,
        ));
        items.push(symbol::arrow(
            tip,
            unit(sub(tip, from)),
            ARROW,
            ARROW_W,
            Colour::BLACK,
        ));
        lines.push([from, tip]);
    }
    let view_at = [0.0, 0.0];
    let _ = view_at;
    Ok(Out {
        items,
        report: json!({"text": filled}),
        text: filled.replace('\n', " "),
        at,
        lines,
        boxes,
        ..Out::default()
    })
}

/// A balloon: the item number its occurrence's part has in the assembly
/// placing it, in a circle, with a leader ending in a dot on the part.
#[allow(clippy::too_many_arguments)]
fn balloon(
    session: &mut Session,
    ctx: &Context,
    plan: &Plan,
    src: &Source,
    vb: &ViewBuilt,
    sheet: &SheetBuilt,
    of: &str,
    written: Option<P>,
    m: &Map,
) -> Result<Out> {
    let sz = size(ctx.standard);
    // Its solids in the view, and the longest piece drawn of them.
    let prefix = format!("{}/{of}/", plan.of);
    let bodies: Vec<usize> = (0..src.len())
        .filter(|i| src[*i].2.starts_with(&prefix))
        .collect();
    if bodies.is_empty() {
        return Err(Error::new(
            Code::DrawingReferenceUnresolved,
            format!("view {} places nothing at {of}", vb.key),
        )
        .with("role", "of")
        .with("reference", of));
    }
    let mut best: Option<(f64, P)> = None;
    if let Some(d) = ctx.drawn[ctx
        .plans
        .iter()
        .position(|(p, _)| p.key == plan.key)
        .expect("listed")]
    {
        for ((b, _), n) in &d.named {
            if !bodies.contains(b) || !n.visible {
                continue;
            }
            for p in &n.paths {
                let bx = p.bounds();
                let l = ryu_math::hypot(bx[1][0] - bx[0][0], bx[1][1] - bx[0][1]);
                if best.is_none_or(|(x, _)| l > x) {
                    best = Some((l, m.sheet_of(addp(p.middle(), m.centre))));
                }
            }
        }
    }
    let Some((_, tip)) = best else {
        return Err(Error::new(
            Code::DrawingReferenceNotShown,
            format!("view {} draws nothing of {of}", vb.key),
        )
        .with("role", "of")
        .with("reference", of)
        .with("reason", "not_drawn"));
    };
    // The item number: in the assembly placing the occurrence's part.
    let item = item_of(session, plan, of)?;
    let r = 1.6 * sz;
    let away = unit(sub(tip, vb.at));
    let c = written.map_or_else(
        || balloon_place(sheet, vb, tip, away, r),
        |w| addp(vb.at, w),
    );
    let (label, colour) = match item {
        Some(n) => (n.to_string(), Colour::BLACK),
        None => ("?".to_owned(), WARNING),
    };
    let mut items = vec![Item::Path {
        path: Path::circle(c, r),
        stroke: Some({
            let mut s = ctx.style.stroke(LineKind::Dimension);
            s.colour = colour;
            s
        }),
        fill: None,
    }];
    items.push(Item::Text {
        text: label.clone(),
        at: [c[0], c[1] - sz * 0.36],
        size: sz,
        angle: 0.0,
        anchor: Anchor::Middle,
        colour,
    });
    let start = addp(c, scale(unit(sub(tip, c)), r));
    items.push(stroke(
        ctx.style,
        LineKind::Dimension,
        &[start, tip],
        colour,
    ));
    items.push(symbol::dot(tip, 0.6, colour));
    let mut out = Out {
        items,
        report: json!({"item": item, "of": of}),
        text: label,
        at: sub(c, vb.at),
        at_placed: written.is_none(),
        lines: vec![[start, tip]],
        boxes: vec![[[c[0] - r, c[1] - r], [c[0] + r, c[1] + r]]],
        ..Out::default()
    };
    if item.is_none() {
        out.report["missing"] = json!(true);
    }
    Ok(out)
}

/// What a default place keeps clear of, as the lint judges it (#645,
/// #662): every view's stroked curves, the annotations placed before, and
/// the drawing area clear of its keep-out boxes.
struct Clearance<'s> {
    curves: Vec<([P; 2], [P; 2])>,
    placed: Vec<&'s Built>,
    area: [P; 2],
    keep_out: &'s [[P; 2]],
}

impl<'s> Clearance<'s> {
    fn of(sheet: &'s SheetBuilt) -> Clearance<'s> {
        // Every view's stroked curves on the sheet, as segments with their
        // boxes: the lint's "view's lines".
        let curves: Vec<([P; 2], [P; 2])> = sheet
            .views
            .iter()
            .flat_map(|v| sheet.items[v.items.clone()].iter())
            .filter_map(|it| match it {
                Item::Path {
                    path,
                    stroke: Some(_),
                    ..
                } => Some(path.points()),
                _ => None,
            })
            .flat_map(|pts| {
                pts.windows(2)
                    .map(|w| {
                        let lo = [w[0][0].min(w[1][0]), w[0][1].min(w[1][1])];
                        let hi = [w[0][0].max(w[1][0]), w[0][1].max(w[1][1])];
                        ([w[0], w[1]], [lo, hi])
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let placed = sheet
            .annotations
            .iter()
            .filter(|a| a.error.is_none())
            .collect();
        Clearance {
            curves,
            placed,
            area: sheet.area,
            keep_out: &sheet.keep_out,
        }
    }

    /// Whether `boxes` lie in the area over no curve and no placed text or
    /// line, and `leaders` cross no placed text.
    fn clear(&self, boxes: &[[P; 2]], leaders: &[[P; 2]]) -> bool {
        use super::lint::{grow, hits, inside, overlap};
        boxes.iter().all(|b| {
            if !inside(*b, self.area) || self.keep_out.iter().any(|k| overlap(*b, *k)) {
                return false;
            }
            let tb = grow(*b, -0.2);
            self.curves
                .iter()
                // A segment's box first: most are nowhere near.
                .all(|(l, lb)| !overlap(*lb, tb) || !hits(*l, tb))
                && self.placed.iter().all(|a| {
                    a.boxes.iter().all(|x| !overlap(tb, *x))
                        && a.lines.iter().all(|l| !hits(*l, tb))
                })
        }) && leaders.iter().all(|l| {
            self.placed
                .iter()
                .all(|a| a.boxes.iter().all(|x| !hits(*l, *x)))
        })
    }
}

/// Where a balloon of radius `r` on a leader to `tip` goes when its file
/// says nothing (#645): the nearest place, out from the view's centre
/// through the tip and turned from that by up to 80° either way, whose
/// circle lies over none of the sheet's drawn curves and none of the
/// annotations placed before it, whose leader crosses none of their
/// text, and which stays in the drawing area clear of the title block, as
/// the lint judges them. Where none does, 18 mm out from the tip.
fn balloon_place(sheet: &SheetBuilt, vb: &ViewBuilt, tip: P, away: P, r: f64) -> P {
    let first = addp(tip, scale(away, 18.0));
    let room = Clearance::of(sheet);
    let clear = |c: P| {
        let b = [[c[0] - r, c[1] - r], [c[0] + r, c[1] + r]];
        room.clear(&[b], &[[addp(c, scale(unit(sub(tip, c)), r)), tip]])
    };
    // Far enough to leave the view's box by two radii, from the tip.
    let [lo, hi] = vb.bounds;
    let reach = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
        .iter()
        .map(|q| ryu_math::hypot(q[0] - tip[0], q[1] - tip[1]))
        .fold(0.0, f64::max)
        + 2.0 * r;
    let mut t = 18.0;
    while t <= reach.max(18.0) {
        for k in 0..=16 {
            // 0, +5°, -5°, +10°, … ±80°.
            let turn =
                f64::from((k + 1) / 2) * 5.0f64.to_radians() * if k % 2 == 1 { 1.0 } else { -1.0 };
            let (s, co) = ryu_math::sin_cos(turn);
            let d = [co * away[0] - s * away[1], s * away[0] + co * away[1]];
            let c = addp(tip, scale(d, t));
            if clear(c) {
                return c;
            }
        }
        t += r / 2.0;
    }
    first
}

/// The item number `of`'s part has in the assembly that places it (the
/// view's, or a subassembly along the path); none where it has none.
fn item_of(session: &mut Session, plan: &Plan, of: &str) -> Result<Option<u32>> {
    let model = session.model(&plan.of, &plan.rev)?;
    let o = model
        .occurrences
        .iter()
        .find(|o| {
            o.path
                .iter()
                .map(Id::to_string)
                .collect::<Vec<_>>()
                .join("/")
                == of
        })
        .cloned();
    let steps: Vec<&str> = of.split('/').collect();
    // The assembly placing the last step: walk the instances down.
    let mut asm = (plan.of.clone(), plan.rev.clone());
    for step in &steps[..steps.len().saturating_sub(1)] {
        let (part, _, _) = session.node(&asm.0, &asm.1)?;
        let id = step.split(':').next().unwrap_or(step);
        let Ok(id) = Id::parse(id, "instance") else {
            return Ok(None);
        };
        match part.features.get(&id) {
            Some(Ok(crate::doc::files::Feature::Instance(i))) => {
                asm = (i.part.clone(), i.rev.clone())
            }
            _ => return Ok(None),
        }
    }
    let (part, _, _) = session.node(&asm.0, &asm.1)?;
    let Ok(file) = &part.file else {
        return Ok(None);
    };
    let placed = match o {
        Some(o) => (o.part.clone(), o.rev.clone()),
        None => {
            // A subassembly's instance: the part it places.
            let last = steps.last().and_then(|s| Id::parse(s, "instance").ok());
            match last.and_then(|l| part.features.get(&l).cloned()) {
                Some(Ok(crate::doc::files::Feature::Instance(i))) => {
                    (i.part.clone(), i.rev.clone())
                }
                _ => return Ok(None),
            }
        }
    };
    Ok(file
        .items
        .get(&crate::doc::files::item_key(&placed.0, &placed.1))
        .copied())
}

/// A BOM table: the BOM of `asm` (indented or flat, from `bom.get`'s CSV,
/// its chosen columns), its lower right corner at `at` (else on the title
/// block's upper right), its header at the bottom and items going up,
/// continued in a block to the left where it would pass the area's top.
#[allow(clippy::too_many_arguments)]
fn bom_table(
    session: &mut Session,
    ctx: &Context,
    a: &Annotation,
    asm: &Id,
    rev: Option<&crate::doc::assembly::Rev>,
    sheet: &SheetBuilt,
    written: Option<P>,
) -> Result<Out> {
    let Kind::BomTable {
        flat,
        depth,
        columns,
    } = &a.kind
    else {
        unreachable!()
    };
    let rev_text = match rev {
        Some(crate::doc::assembly::Rev::Label(l)) => Some(format!("label:{l}")),
        _ => None,
    };
    let v = crate::bom::get(
        session,
        &crate::bom::Ask {
            part: Some(asm.as_str()),
            rev: rev_text.as_deref(),
            flat: *flat,
            depth: depth.map(|d| d as usize),
            format: Some("csv"),
        },
    )?;
    let csv = v["csv"].as_str().unwrap_or_default();
    let rows = parse_csv(csv);
    let Some(header) = rows.first() else {
        return Err(Error::new(Code::Internal, "a BOM's CSV has no header"));
    };
    let mut cols = Vec::new();
    for c in columns {
        let i = header.iter().position(|h| h == c).ok_or_else(|| {
            Error::new(Code::InvalidParam, format!("the BOM has no column {c}"))
                .with("reason", "no_column")
                .with("columns", header)
        })?;
        cols.push(i);
    }
    // The root's own row (an indented BOM's first) isn't an item.
    let body: Vec<&Vec<String>> = rows[1..]
        .iter()
        .filter(|r| !(r.first().is_some_and(|l| l == "0")))
        .collect();
    let tsz = 2.5;
    let pad = 1.5;
    let rh = 6.0;
    let widths: Vec<f64> = cols
        .iter()
        .map(|i| {
            std::iter::once(header[*i].to_uppercase().replace('_', " "))
                .chain(body.iter().map(|r| r.get(*i).cloned().unwrap_or_default()))
                .map(|t| ryu_draw::font::width(&t, tsz))
                .fold(0.0, f64::max)
                + 2.0 * pad
        })
        .collect();
    let width: f64 = widths.iter().sum();
    let corner = written.unwrap_or_else(|| match sheet.keep_out.first() {
        Some([_, hi]) => [hi[0], hi[1]],
        None => [sheet.area[1][0], sheet.area[0][1]],
    });
    let top = sheet.area[1][1];
    let per = (((top - corner[1]) / rh).floor() as usize)
        .saturating_sub(1)
        .max(1);
    let st = ctx.style;
    let mut items = Vec::new();
    let mut lines = Vec::new();
    let mut boxes = Vec::new();
    let chunks: Vec<&[&Vec<String>]> = if body.is_empty() {
        vec![&[][..]]
    } else {
        body.chunks(per).collect()
    };
    for (b, chunk) in chunks.iter().enumerate() {
        let right = corner[0] - b as f64 * (width + 4.0);
        let left = right - width;
        let n = chunk.len() + 1;
        let lo = [left, corner[1]];
        let hi = [right, corner[1] + rh * n as f64];
        items.push(stroke(
            st,
            LineKind::Visible,
            &[lo, [hi[0], lo[1]], hi, [lo[0], hi[1]], lo],
            Colour::BLACK,
        ));
        boxes.push([lo, hi]);
        for j in 1..n {
            let y = corner[1] + rh * j as f64;
            items.push(stroke(
                st,
                LineKind::Thin,
                &[[left, y], [right, y]],
                Colour::BLACK,
            ));
        }
        let mut x = left;
        for (ci, w) in widths.iter().enumerate() {
            if ci > 0 {
                items.push(stroke(
                    st,
                    LineKind::Thin,
                    &[[x, lo[1]], [x, hi[1]]],
                    Colour::BLACK,
                ));
            }
            let cells = std::iter::once(header[cols[ci]].to_uppercase().replace('_', " ")).chain(
                chunk
                    .iter()
                    .map(|r| r.get(cols[ci]).cloned().unwrap_or_default()),
            );
            for (j, t) in cells.enumerate() {
                let y = corner[1] + rh * j as f64 + (rh - tsz * 0.727) / 2.0;
                items.push(Item::Text {
                    text: t,
                    at: [x + pad, y],
                    size: tsz,
                    angle: 0.0,
                    anchor: Anchor::Start,
                    colour: Colour::BLACK,
                });
            }
            x += w;
        }
        lines.push([lo, hi]);
    }
    let base = match a.view {
        Some(v) => sheet
            .views
            .iter()
            .find(|x| x.key == v)
            .map_or([0.0, 0.0], |x| x.at),
        None => [0.0, 0.0],
    };
    Ok(Out {
        items,
        report: json!({"rows": body.len(), "columns": columns, "blocks": chunks.len(), "of": asm}),
        text: format!("BOM of {asm}, {} rows", body.len()),
        at: sub(corner, base),
        at_placed: written.is_none(),
        lines: Vec::new(),
        boxes,
        ..Out::default()
    })
}

/// RFC 4180 rows.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                cell.push('"');
            }
            (true, '"') => quoted = false,
            (true, c) => cell.push(c),
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut cell)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (false, c) => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// A feature control frame of `rows` (one, or two for a composite frame
/// sharing its symbol), its lower left at `at`, each row `h` high, with a
/// leader from its left side to `tip`.
fn frame(
    ctx: &Context,
    units: &text::Units,
    c: crate::doc::drawing_annotation::Characteristic,
    rows: &[Row],
    at: P,
    h: f64,
    tip: Option<P>,
) -> Out {
    use crate::doc::drawing_annotation::Characteristic as C;
    let st = ctx.style;
    let sz = size(ctx.standard);
    let stroke_d = st.stroke(LineKind::Dimension);
    let sym = match c {
        C::Straightness => Symbol::Straightness,
        C::Flatness => Symbol::Flatness,
        C::Circularity => Symbol::Circularity,
        C::Cylindricity => Symbol::Cylindricity,
        C::ProfileLine => Symbol::ProfileLine,
        C::ProfileSurface => Symbol::ProfileSurface,
        C::Angularity => Symbol::Angularity,
        C::Perpendicularity => Symbol::Perpendicularity,
        C::Parallelism => Symbol::Parallelism,
        C::Position => Symbol::Position,
        C::Concentricity => Symbol::Concentricity,
        C::Symmetry => Symbol::Symmetry,
        C::CircularRunout => Symbol::CircularRunout,
        C::TotalRunout => Symbol::TotalRunout,
    };
    let mut items = Vec::new();
    let n = rows.len() as f64;
    let total = h * n;
    let mut right_most: f64 = at[0] + h;
    // The symbol's compartment, spanning every row.
    items.push(stroke(
        st,
        LineKind::Dimension,
        &[
            at,
            [at[0] + h, at[1]],
            [at[0] + h, at[1] + total],
            [at[0], at[1] + total],
            at,
        ],
        Colour::BLACK,
    ));
    items.extend(sym.items(
        [
            at[0] + (h - sz * sym.advance()) / 2.0,
            at[1] + total / 2.0 - sz * 0.36,
        ],
        sz,
        0.0,
        &stroke_d,
    ));
    let mut texts = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let y0 = at[1] + total - h * (i + 1) as f64;
        let mut cells: Vec<Vec<Run>> = Vec::new();
        let mut first = Vec::new();
        if r.diameter {
            first.push(Run::Symbol(Symbol::Diameter));
        }
        let t = r.tolerance.meters();
        first.push(Run::Text(units.number(
            t,
            text::needs(t / units.unit.factor()),
            false,
        )));
        if let Some(cond) = r.condition {
            first.push(Run::Text(" ".into()));
            first.push(Run::Symbol(Symbol::Circled(cond.letter())));
        }
        cells.push(first);
        for (l, cond) in &r.datums {
            let mut d = vec![Run::Text(l.clone())];
            if let Some(cond) = cond {
                d.push(Run::Text(" ".into()));
                d.push(Run::Symbol(Symbol::Circled(cond.letter())));
            }
            cells.push(d);
        }
        let mut x = at[0] + h;
        for cell in cells {
            let w = (symbol::width(&cell, sz) + 3.0).max(h);
            items.push(stroke(
                st,
                LineKind::Dimension,
                &[[x, y0], [x + w, y0], [x + w, y0 + h], [x, y0 + h], [x, y0]],
                Colour::BLACK,
            ));
            let base = [x + w / 2.0, y0 + h / 2.0 - sz * 0.36];
            items.extend(symbol::draw(
                &cell,
                base,
                sz,
                0.0,
                Anchor::Middle,
                Colour::BLACK,
                &stroke_d,
            ));
            texts.push(plain(&cell));
            x += w;
        }
        right_most = right_most.max(x);
    }
    let bx = [at, [right_most, at[1] + total]];
    let mut lines = Vec::new();
    if let Some(tip) = tip {
        let from = [at[0], at[1] + total / 2.0];
        let knee = [from[0] - 3.0, from[1]];
        items.push(stroke(
            st,
            LineKind::Dimension,
            &[from, knee, tip],
            Colour::BLACK,
        ));
        items.push(symbol::arrow(
            tip,
            unit(sub(tip, knee)),
            ARROW,
            ARROW_W,
            Colour::BLACK,
        ));
        lines.push([from, knee]);
        lines.push([knee, tip]);
    }
    Out {
        items,
        text: format!("{} {}", c.as_str(), texts.join(" | ")),
        at,
        lines,
        boxes: vec![bx],
        ..Out::default()
    }
}

/// A surface finish symbol standing with its point at `tip` (ISO 1302,
/// ASME Y14.36): a check mark, a bar for material removed, a circle for
/// none, and `Ra` over its long leg.
fn finish(ctx: &Context, units: &text::Units, tip: P, ra: Option<f64>, process: Process) -> Out {
    let st = ctx.style;
    let sz = size(ctx.standard);
    let h1 = 1.4 * sz;
    let h2 = 3.0 * sz;
    let t30 = 1.0 / 3f64.sqrt();
    let left = [tip[0] - h1 * t30, tip[1] + h1];
    let right = [tip[0] + h2 * t30, tip[1] + h2];
    let mut items = vec![stroke(
        st,
        LineKind::Dimension,
        &[left, tip, right],
        Colour::BLACK,
    )];
    match process {
        Process::Any => {}
        Process::Removal => {
            items.push(stroke(
                st,
                LineKind::Dimension,
                &[left, [tip[0] + h1 * t30, tip[1] + h1]],
                Colour::BLACK,
            ));
        }
        Process::NoRemoval => {
            items.push(Item::Path {
                path: Path::circle([tip[0], tip[1] + h1 * 0.55], h1 * 0.3),
                stroke: Some(st.stroke(LineKind::Dimension)),
                fill: None,
            });
        }
    }
    let mut text = String::new();
    let mut boxes = vec![[[left[0], tip[1]], [right[0], right[1]]]];
    if let Some(ra) = ra {
        let um = ra * 1e6;
        text = format!("Ra {}", text::decimal(um, text::needs(um), false));
        let w = ryu_draw::font::width(&text, sz);
        items.push(stroke(
            st,
            LineKind::Dimension,
            &[right, [right[0] + w + 2.0, right[1]]],
            Colour::BLACK,
        ));
        let at = [right[0] + 1.0, right[1] + 0.8];
        items.push(Item::Text {
            text: text.clone(),
            at,
            size: sz,
            angle: 0.0,
            anchor: Anchor::Start,
            colour: Colour::BLACK,
        });
        boxes.push(text_box(
            &[Run::Text(text.clone())],
            at,
            sz,
            0.0,
            Anchor::Start,
        ));
    }
    let _ = units;
    Out {
        items,
        report: json!({"ra": ra, "process": process.as_str()}),
        text,
        at: tip,
        boxes,
        ..Out::default()
    }
}

/// The room a view's annotations may use past its drawing (the layout's).
pub const ROOM: f64 = DIM_ROOM;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_reads_quoted_cells() {
        let r = parse_csv("a,b\n1,\"x, \"\"y\"\"\"\n");
        assert_eq!(r, vec![vec!["a", "b"], vec!["1", "x, \"y\""]]);
    }
}
