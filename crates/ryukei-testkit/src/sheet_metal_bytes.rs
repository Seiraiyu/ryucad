//! Sheet-metal parts from bytes (design 298, plan 11.D), for the
//! `sheet_metal` fuzz target and `tests/sheet_metal_props.rs`: a plate,
//! one to four edge flanges on its walls (on its top or bottom edge, at
//! seeded angles, lengths, radii, positions and measures, set back with
//! reliefs where a neighbour is flanged), holes drilled in the plate and
//! in the flanges' straight parts, and holes and slots to cut across bend
//! lines in the flat. [`trip`] unfolds the part, cuts the flat, refolds
//! it and unfolds it again.

use crate::euler_script::Bytes;
use core::f64::consts::PI;
use ryukei::Error;
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::check::{Fault, check, check_intersections};
use ryukei::geom::{Frame, Point3, Rigid, UnitVector3, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Name, OpKey, ToolKey};
use ryukei::primitive::{block, cylinder};
use ryukei::sheet_metal::{
    Allowance, EdgeFlange, FlangePosition, FlatMap, Measure, RefoldSpec, Relief, SheetModel,
    SheetOptions, UnfoldSpec, flange, recognize, refold, unfold,
};
use ryukei::topo::{Body, EntityId};

/// The keys every case is made with.
const PLATE: OpKey = OpKey(0x534d_0001);
const FLANGE: OpKey = OpKey(0x534d_0002);
const DRILL: OpKey = OpKey(0x534d_0003);
const UNFOLD: OpKey = OpKey(0x534d_0004);
const CUT: OpKey = OpKey(0x534d_0005);
const REFOLD: OpKey = OpKey(0x534d_0006);
const TOOL: OpKey = OpKey(0x534d_0007);

/// The angles a flange is bent through.
const ANGLES: [f64; 6] = [
    PI / 6.0,
    PI / 4.0,
    PI / 3.0,
    PI / 2.0,
    2.0 * PI / 3.0,
    3.0 * PI / 4.0,
];
const THICKNESSES: [f64; 4] = [0.0005, 0.001, 0.0015, 0.002];

/// A seeded part, made and recognized, with the cuts to make in its flat.
pub struct Case {
    /// The flanged, drilled part.
    pub folded: Body,
    pub model: SheetModel,
    /// Unfolding every bend about the plate.
    pub spec: UnfoldSpec,
    /// Tools to subtract from the flat, across its bend lines (none, now
    /// and then).
    pub cuts: Vec<(Body, Rigid)>,
    /// The sheet's thickness, the smallest feature, for a chord.
    pub thickness: f64,
}

/// What [`trip`] made of a case.
pub struct Trip {
    /// The part unfolded.
    pub flat: Body,
    pub map: FlatMap,
    /// The flat after its cuts (the flat itself when there are none).
    pub cut: Body,
    /// The cut flat refolded.
    pub refolded: Body,
    /// The refolded part unfolded again.
    pub again: Body,
}

/// Whether `e` is a refusal a seeded case may meet: anything typed but a
/// topology fault, which is a bug.
pub fn typed(e: &Error) -> bool {
    matches!(
        e,
        Error::InvalidSheetMetal(_)
            | Error::InvalidBoolean(_)
            | Error::InvalidProfile(_)
            | Error::InvalidSweep(_)
    )
}

/// The case the bytes make: `None` when they make none, `Some(Err)` for a
/// refusal on the way (see [`typed`]).
pub fn case(data: &[u8]) -> Option<Result<Case, Error>> {
    if data.len() < 4 {
        return None;
    }
    let mut b = Bytes::new(data);
    Some(make(&mut b))
}

/// One wall's flange: which edge, and its spec but for the edge.
#[derive(Clone, Copy)]
struct Wall {
    top: bool,
    angle: f64,
    radius: f64,
    straight: f64,
    measure: Measure,
    position: FlangePosition,
}

fn make(b: &mut Bytes) -> Result<Case, Error> {
    let t = THICKNESSES[b.pick(THICKNESSES.len())];
    let px = b.range(0.03, 0.08);
    let py = b.range(0.03, 0.08);
    let plate = block(PLATE, px, py, t, &Rigid::IDENTITY)?.0;
    // Walls +x, +y, -x, -y, each `None`, its top edge or its bottom one.
    let mut walls: [Option<Wall>; 4] = [None; 4];
    for w in &mut walls {
        let pick = b.pick(3);
        if pick == 0 {
            continue;
        }
        let angle = ANGLES[b.pick(ANGLES.len())];
        let radius = b.range(0.5, 3.0) * t;
        let straight = b.range(2.0 * t, 0.3 * px.min(py));
        let measure = [Measure::Tangent, Measure::OuterMould, Measure::InnerMould][b.pick(3)];
        let position = [
            FlangePosition::MaterialInside,
            FlangePosition::MaterialOutside,
            FlangePosition::BendOutside,
        ][b.pick(3)];
        *w = Some(Wall {
            top: pick == 1,
            angle,
            radius,
            straight,
            measure,
            position,
        });
    }
    if walls.iter().all(Option::is_none) {
        walls[0] = Some(Wall {
            top: true,
            angle: PI / 2.0,
            radius: t,
            straight: 5.0 * t,
            measure: Measure::Tangent,
            position: FlangePosition::MaterialInside,
        });
    }
    // How far a flange reaches along its neighbours' walls: out past its
    // bend, and back over the plate when it leans over it.
    let reach = |w: &Wall| w.radius + t + (-w.angle.cos()).max(0.0) * w.straight + 0.001;
    let lengths = [py, px, py, px];
    let edges = [
        Point3::new(px, 0.5 * py, 0.0),
        Point3::new(0.5 * px, py, 0.0),
        Point3::new(0.0, 0.5 * py, 0.0),
        Point3::new(0.5 * px, 0.0, 0.0),
    ];
    let mut specs = Vec::new();
    for i in 0..4 {
        let Some(w) = walls[i] else { continue };
        let neighbours = [walls[(i + 1) % 4], walls[(i + 3) % 4]];
        let needed = neighbours
            .iter()
            .flatten()
            .map(reach)
            .fold(0.0_f64, f64::max);
        let free = b.range(0.0, 0.2) * lengths[i];
        let setback = if needed > 0.0 { needed } else { free };
        if 2.0 * setback + 2.0 * t > lengths[i] {
            continue;
        }
        let width = b.range(0.5, 1.0) * t;
        let depth = b.range(0.5, 2.0) * t;
        let relief = match b.pick(3) {
            0 => Relief::Rectangular { width, depth },
            1 => Relief::Obround {
                width,
                depth: depth.max(0.5 * width),
            },
            _ => Relief::Tear { width },
        };
        let length = w.straight
            + match w.measure {
                Measure::Tangent => 0.0,
                Measure::OuterMould => (w.radius + t) * ryukei::math::tan(0.5 * w.angle),
                Measure::InnerMould => w.radius * ryukei::math::tan(0.5 * w.angle),
            };
        let mut at = edges[i];
        if w.top {
            at.z = t;
        }
        specs.push(EdgeFlange {
            edge: edge_at(&plate, at),
            side: None,
            angle: w.angle,
            length,
            measured: w.measure,
            radius: w.radius,
            position: w.position,
            setbacks: [setback; 2],
            reliefs: if setback > 0.0 {
                [relief; 2]
            } else {
                [Relief::None; 2]
            },
        });
    }
    let flanged = flange(FLANGE, &plate, &specs, &SheetOptions::default())?.0;
    let model = recognize(&flanged, &SheetOptions::default())?;
    let allowance = if b.pick(4) == 0 {
        Allowance::Length(b.range(0.5, 2.0) * t)
    } else {
        Allowance::KFactor(b.range(0.0, 1.0))
    };
    let spec = about_the_plate(&model, allowance);
    let u = unfold(UNFOLD, &flanged, &model, &spec)?;
    // Holes in the plate, clear of its walls and of whatever a flange
    // reaches over it.
    let mut tools = Vec::new();
    let ext = 0.2 * t;
    let margin = walls.iter().flatten().map(reach).fold(0.0_f64, f64::max) + 0.001;
    for _ in 0..b.pick(4) {
        let h = b.range(0.0003, 0.002);
        let lo = margin + h;
        let (hx, hy) = (px - margin - h, py - margin - h);
        if hx <= lo || hy <= lo {
            break;
        }
        let at = Point3::new(b.range(lo, hx), b.range(lo, hy), -ext);
        tools.push((
            cylinder(TOOL, h, t + 2.0 * ext, &Rigid::IDENTITY)?.0,
            Rigid::translation(at - Point3::ORIGIN),
        ));
    }
    // A hole in a flange's straight part, placed in the flat and carried
    // back by its flange's motion.
    for strip in &u.map.strips {
        if b.pick(2) == 0 {
            continue;
        }
        let Some(w) = walls.iter().flatten().find(|w| {
            (w.radius - strip.radius).abs() < 1e-12 && (w.angle - strip.angle).abs() < 1e-12
        }) else {
            continue;
        };
        let span = strip.span[1] - strip.span[0];
        let h = 0.3 * w.straight.min(span) * b.range(0.3, 1.0);
        if h < 0.0003 || w.straight < 2.0 * h + 2.0 * t {
            continue;
        }
        let x = strip.length + 0.5 * w.straight;
        let y = strip.span[0] + 0.5 * span;
        let Some(motion) = u.map.flanges.iter().find(|(n, _)| *n == strip.far) else {
            continue;
        };
        let flat = Rigid::from_frame(&frame_at(&strip.frame, x, y, -ext));
        tools.push((
            cylinder(TOOL, h, t + 2.0 * ext, &Rigid::IDENTITY)?.0,
            motion.1.inverse().then_after(&flat),
        ));
    }
    let folded = if tools.is_empty() {
        flanged
    } else {
        subtract(DRILL, &flanged, &tools)?
    };
    let model = recognize(&folded, &SheetOptions::default())?;
    let spec = about_the_plate(&model, allowance);
    // Cuts across bend lines in the flat: a hole or a slot on a strip's
    // near or far line.
    let mut cuts = Vec::new();
    for strip in &u.map.strips {
        if b.pick(3) == 0 {
            continue;
        }
        let span = strip.span[1] - strip.span[0];
        let hi = (0.8 * strip.length).min(0.2 * span).min(0.002);
        if hi < 0.0003 {
            continue;
        }
        let h = b.range(0.0003, hi);
        let x = if b.pick(2) == 0 { 0.0 } else { strip.length };
        let y = strip.span[0] + span * b.range(0.3, 0.7);
        let at = frame_at(&strip.frame, x, y, -ext);
        if b.pick(2) == 0 {
            cuts.push((
                cylinder(TOOL, h, t + 2.0 * ext, &Rigid::IDENTITY)?.0,
                Rigid::from_frame(&at),
            ));
        } else {
            // A slot `2h` wide and `6h` long, turned by 0, 30 or 60
            // degrees about the flat's normal, centred on the line.
            let turn = [0.0, PI / 6.0, PI / 3.0][b.pick(3)];
            let slot = block(TOOL, 6.0 * h, 2.0 * h, t + 2.0 * ext, &Rigid::IDENTITY)?.0;
            let centre = Rigid::translation(Vector3::new(-3.0 * h, -h, 0.0));
            let turned = Rigid::rotation(UnitVector3::Z, turn);
            cuts.push((
                slot,
                Rigid::from_frame(&at).then_after(&turned.then_after(&centre)),
            ));
        }
    }
    Ok(Case {
        folded,
        model,
        spec,
        cuts,
        thickness: t,
    })
}

/// Unfolds the case's part, cuts the flat, refolds it and unfolds that.
pub fn trip(c: &Case) -> Result<Trip, Error> {
    let u = unfold(UNFOLD, &c.folded, &c.model, &c.spec)?;
    let cut = if c.cuts.is_empty() {
        u.body.clone()
    } else {
        subtract(CUT, &u.body, &c.cuts)?
    };
    let refolded = refold(REFOLD, &cut, &u.map, &RefoldSpec::default())?.0;
    let model = recognize(&refolded, &SheetOptions::default())?;
    let again = unfold(UNFOLD, &refolded, &model, &c.spec)?.body;
    Ok(Trip {
        flat: u.body,
        map: u.map,
        cut,
        refolded,
        again,
    })
}

/// The frame at `(x, y, z)` of `f`, with `f`'s axes.
fn frame_at(f: &Frame, x: f64, y: f64, z: f64) -> Frame {
    Frame::new(f.to_world(x, y, z), f.z().get(), f.x().get()).expect("a frame's axes")
}

fn subtract(key: OpKey, target: &Body, tools: &[(Body, Rigid)]) -> Result<Body, Error> {
    let tools: Vec<Tool> = tools
        .iter()
        .enumerate()
        .map(|(i, (body, place))| Tool {
            key: ToolKey(i as u128 + 1),
            body,
            place: *place,
        })
        .collect();
    Ok(boolean(key, BooleanOp::Subtract, target, &tools)?.0)
}

/// The name of the edge whose middle is nearest `p`.
pub fn edge_at(b: &Body, p: Point3) -> Name {
    let middle = |e| {
        let ed = b.edge(e).expect("an edge");
        b.curve(ed.curve)
            .expect("its curve")
            .point(0.5 * (ed.interval[0] + ed.interval[1]))
    };
    let e = b
        .edges()
        .min_by(|&x, &y| middle(x).distance(p).total_cmp(&middle(y).distance(p)))
        .expect("edges");
    b.edge(e).expect("an edge").name
}

/// Unfolding every bend about the flange holding the plate's bottom.
pub fn about_the_plate(m: &SheetModel, allowance: Allowance) -> UnfoldSpec {
    let base = m
        .flanges
        .iter()
        .find(|f| {
            f.sides.iter().any(|s| {
                s.plane.frame().z().get().z.abs() > 1.0 - 1e-12
                    && s.plane.frame().origin().z.abs() < 1e-12
            })
        })
        .unwrap_or(&m.flanges[m.fixed]);
    UnfoldSpec {
        fixed: Some(base.sides[0].faces[0]),
        ..UnfoldSpec::all(allowance)
    }
}

fn names(x: &Body) -> Vec<ryukei::name::Name> {
    let mut n: Vec<_> = x
        .faces()
        .map(|f| x.face(f).unwrap().name)
        .chain(x.edges().map(|e| x.edge(e).unwrap().name))
        .chain(x.vertices().map(|v| x.vertex(v).unwrap().name))
        .collect();
    n.sort_unstable();
    n
}

/// `b` has `a`'s faces, edges and vertices by name, each vertex within
/// `tol` of `a`'s.
pub fn same_shape(a: &Body, b: &Body, tol: f64) -> Result<(), String> {
    if names(a) != names(b) {
        return Err("the names differ".into());
    }
    for v in a.vertices() {
        let va = a.vertex(v).unwrap();
        let Some(EntityId::Vertex(w)) = b.by_name(va.name) else {
            return Err(format!("vertex {} is missing", va.name));
        };
        let d = va.point.distance(b.vertex(w).unwrap().point);
        if d > tol {
            return Err(format!("vertex {} is {d} away", va.name));
        }
    }
    Ok(())
}

/// `b` passes `check` and `check_intersections` (pairs it can't check
/// allowed) and meshes at a quarter of the thickness without faults.
pub fn sound(b: &Body, t: f64) -> Result<(), String> {
    let faults = check(b);
    if !faults.is_empty() {
        return Err(format!("check: {faults:?}"));
    }
    for f in check_intersections(b) {
        if !matches!(f, Fault::CannotCheck { .. }) {
            return Err(format!("check_intersections: {f:?}"));
        }
    }
    let params = MeshParams {
        max_triangles: 2_000_000,
        ..MeshParams::new((0.25 * t).max(1e-5), 0.5)
    };
    match tessellate(b, &params) {
        Ok(m) if m.faults.is_empty() && check_mesh(b, &m).is_ok() => Ok(()),
        Ok(m) => Err(format!("mesh faults {:?}", m.faults)),
        Err(Error::MeshTooLarge { .. }) => Ok(()),
        Err(e) => Err(format!("mesh {e:?}")),
    }
}

/// Every rule of the round trip, for the props, the fuzz target and its
/// regressions: see `tests/sheet_metal_props.rs`.
pub fn judge(c: &Case, r: &Trip) -> Result<(), String> {
    if c.cuts.is_empty() {
        same_shape(&c.folded, &r.refolded, 1e-12)?;
    }
    same_shape(&r.cut, &r.again, 1e-12)?;
    for b in [&c.folded, &r.flat, &r.cut, &r.refolded, &r.again] {
        sound(b, c.thickness)?;
    }
    Ok(())
}
