//! Placement by constraint (design 924): a placement's constraints judged
//! on every check, exactly, and the legaliser that finds a place where
//! they hold (Task 5).

use super::drc::{Finding, finding};
use super::file::{Constraint, EdgeSide, Placement, PlacementSide};
use super::load::BoardFiles;
use super::occ::Occ;
use super::preset::Preset;
use super::rules::Rules;
use super::shapes::BoardShapes;
use super::shapes::PadInfo;
use crate::error::Code;
use crate::error::{Error, Result};
use crate::fault::Severity;
use ryu_board::geom::Rect;
use ryu_board::geom::{Mdeg, Place, Point};
use ryu_board::shape::Shape;
use ryu_copper::measure::{self, Body, BodyOptions, Verdict};
use serde_json::{Value, json};

/// Every listed shape as one.
pub(crate) fn union<'a>(shapes: impl IntoIterator<Item = &'a Shape>) -> Shape {
    let mut out = Shape::default();
    for s in shapes {
        out.parts.extend(s.parts.iter().cloned());
    }
    out
}

/// A part's pads' copper, every layer as one shape.
pub(crate) fn part_copper(s: &BoardShapes, occurrence: &str) -> Shape {
    union(
        s.pads
            .iter()
            .filter(|p| p.occurrence == occurrence)
            .map(|p| &p.copper),
    )
}

/// A part's courtyard, else its pads' copper.
pub(crate) fn part_body_shape(s: &BoardShapes, owner: &str, occurrence: &str) -> Shape {
    s.courtyards
        .iter()
        .find(|c| c.owner == owner)
        .map(|c| c.shape.clone())
        .unwrap_or_else(|| part_copper(s, occurrence))
}

/// What `near`'s target names: a pad (`U1/7`) or a part (`U1`), with its
/// full reference; `None` when the board has neither.
pub(crate) fn target(s: &BoardShapes, board: &str, r: &str) -> Option<(String, Shape)> {
    let full = if r.contains(':') {
        r.to_owned()
    } else {
        format!("{board}:{r}")
    };
    if let Some(p) = s.pads.iter().find(|p| p.reference == full) {
        return Some((full, p.copper.clone()));
    }
    let prefix = format!("{full}/");
    let pads: Vec<&Shape> = s
        .pads
        .iter()
        .filter(|p| p.reference.starts_with(&prefix))
        .map(|p| &p.copper)
        .collect();
    (!pads.is_empty()).then(|| (full, union(pads)))
}

/// How far a box lies inside the outline's box from `side`, measured
/// perpendicular to it.
fn from_side(outline: Rect, b: Rect, side: EdgeSide) -> i64 {
    match side {
        EdgeSide::Left => b.min.x - outline.min.x,
        EdgeSide::Right => outline.max.x - b.max.x,
        EdgeSide::Bottom => b.min.y - outline.min.y,
        EdgeSide::Top => outline.max.y - b.max.y,
        EdgeSide::Any => unreachable!("any is measured against the edges"),
    }
}

fn unmet(
    board: &str,
    owner: &str,
    other: String,
    c: &Constraint,
    message: String,
    details: Value,
) -> Finding {
    let mut d = details;
    d["constraint"] = json!(c);
    finding(
        Code::PlacementConstraintUnmet,
        Severity::Error,
        board,
        vec![owner.to_owned(), other],
        message,
        d,
    )
}

/// One constraint of one part, judged: a finding when it isn't met.
pub(crate) fn judge_one(
    board: &str,
    label: &str,
    occurrence: &str,
    s: &BoardShapes,
    pl: &Placement,
    c: &Constraint,
) -> Option<Finding> {
    let owner = format!("{board}:{label}");
    let opts = BodyOptions::default();
    match c {
        Constraint::Side { side } => (pl.side != *side).then(|| {
            unmet(
                board,
                &owner,
                format!("{board}:side/{}", side.name()),
                c,
                format!("{owner} must be on the {}", side.name()),
                json!({}),
            )
        }),
        Constraint::Near { near, within_nm } => {
            let Some((full, shape)) = target(s, board, near) else {
                return Some(unmet(
                    board,
                    &owner,
                    format!("{board}:{near}"),
                    c,
                    format!("{owner} must be near {near}, which isn't on the board"),
                    json!({"missing": true}),
                ));
            };
            let a = Body::of_shape(&part_copper(s, occurrence), &opts);
            let b = Body::of_shape(&shape, &opts);
            let v = measure::within(&a, &b, *within_nm);
            (v != Verdict::Yes).then(|| {
                let d = measure::distance(&a, &b).map(|d| d.nm);
                unmet(board, &owner, full.clone(), c,
                    format!("{owner} must be within {within_nm} nm of {full}"),
                    json!({"distance_nm": d, "within_nm": within_nm, "uncertain": v == Verdict::Uncertain}))
            })
        }
        Constraint::Edge { edge, within_nm } => {
            let body = part_body_shape(s, &owner, occurrence);
            let other = format!("{board}:edge/{}", edge.name());
            let message = format!(
                "{owner} must be within {within_nm} nm of the board's {} edge",
                edge.name()
            );
            if *edge == EdgeSide::Any {
                let a = Body::of_shape(&body, &opts);
                let e = Body::of_shape(&s.edges, &opts);
                let v = measure::within(&a, &e, *within_nm);
                return (v != Verdict::Yes).then(|| {
                    let d = measure::distance(&a, &e).map(|d| d.nm);
                    unmet(board, &owner, other, c, message,
                        json!({"distance_nm": d, "within_nm": within_nm, "uncertain": v == Verdict::Uncertain}))
                });
            }
            let (Some(o), Some(b)) = (s.outline.bounds(), body.bounds()) else {
                return Some(unmet(
                    board,
                    &owner,
                    other,
                    c,
                    message,
                    json!({"missing": true}),
                ));
            };
            let d = from_side(o, b, *edge);
            (d > *within_nm).then(|| {
                unmet(
                    board,
                    &owner,
                    other,
                    c,
                    message,
                    json!({"distance_nm": d, "within_nm": within_nm, "uncertain": false}),
                )
            })
        }
    }
}

/// Every placed part's constraints, judged (a DRC rule).
pub fn constraints(board: &str, b: &BoardFiles, occs: &[Occ], s: &BoardShapes) -> Vec<Finding> {
    let mut out = Vec::new();
    for o in occs {
        let Some(Ok(pl)) = b.placements.get(&o.reference).map(|l| l.item.as_ref()) else {
            continue;
        };
        for c in &pl.constraints {
            out.extend(judge_one(board, o.label(), &o.reference, s, pl, c));
        }
    }
    out
}

/// The most candidates the legaliser tries.
pub const MAX_CANDIDATES: usize = 250_000;

/// What the legaliser is asked to place.
pub struct Ask<'a> {
    pub occ: &'a Occ<'a>,
    pub rotation_mdeg: i64,
    pub side: PlacementSide,
    pub constraints: &'a [Constraint],
    /// Where the part is now, if it is placed.
    pub now: Option<[i64; 2]>,
}

fn apart(a: Rect, b: Rect, d: i64) -> bool {
    a.max.x.saturating_add(d) < b.min.x
        || b.max.x.saturating_add(d) < a.min.x
        || a.max.y.saturating_add(d) < b.min.y
        || b.max.y.saturating_add(d) < a.min.y
}

/// Another item a candidate must clear.
struct Other {
    reference: String,
    net: Option<String>,
    /// Copper layers it is on, by index.
    on: Vec<bool>,
    body: Body,
    bounds: Rect,
}

fn others(board: &str, s: &BoardShapes, occurrence: &str, opts: &BodyOptions) -> Vec<Other> {
    let n = s.n();
    let mut out = Vec::new();
    let mut push = |reference: String, net: Option<String>, on: Vec<bool>, shape: &Shape| {
        if let Some(bounds) = shape.bounds() {
            out.push(Other {
                reference,
                net,
                on,
                body: Body::of_shape(shape, opts),
                bounds,
            });
        }
    };
    for p in s.pads.iter().filter(|p| p.occurrence != occurrence) {
        push(
            p.reference.clone(),
            p.net.clone(),
            (0..n).map(|l| p.on(l, n)).collect(),
            &p.copper,
        );
    }
    for t in &s.tracks {
        let l = t.layer.index(n).unwrap_or(0);
        push(
            format!("{board}:track/{}", t.id),
            Some(t.net.clone()),
            (0..n).map(|i| i == l).collect(),
            &t.shape,
        );
    }
    for v in &s.vias {
        push(
            format!("{board}:via/{}", v.id),
            Some(v.net.clone()),
            (0..n).map(|i| (v.from..=v.to).contains(&i)).collect(),
            &v.shape,
        );
    }
    out
}

/// What stops a candidate, if anything: the board's edge, another
/// courtyard, or another net's copper.
#[allow(clippy::too_many_arguments)]
fn blocked(
    s: &BoardShapes,
    rules: &Rules,
    owner: &str,
    side: PlacementSide,
    pads: &[PadInfo],
    body: &Shape,
    others: &[Other],
    opts: &BodyOptions,
) -> Option<String> {
    let n = s.n();
    let b = Body::of_shape(body, opts);
    let outline = Body::of_shape(&s.outline, opts);
    let edges = Body::of_shape(&s.edges, opts);
    let inside = b
        .solids
        .iter()
        .all(|x| measure::contains(&outline, x.probe()).is_yes());
    let ec = rules.edge_clearance(None).value;
    let near_edge = if ec > 0 {
        measure::closer(&b, &edges, ec)
    } else {
        measure::touches(&b, &edges)
    };
    if !inside || near_edge != Verdict::No {
        return Some("the board's edge".into());
    }
    let gap = rules.courtyard_clearance().value;
    let bottom = side == PlacementSide::Bottom;
    for c in s
        .courtyards
        .iter()
        .filter(|c| c.owner != owner && c.bottom == bottom)
    {
        let theirs = Body::of_shape(&c.shape, opts);
        let v = if gap == 0 {
            measure::touches(&b, &theirs)
        } else {
            measure::closer(&b, &theirs, gap)
        };
        if v != Verdict::No {
            return Some(c.owner.clone());
        }
    }
    let outer = if bottom { n - 1 } else { 0 };
    for k in s.keepouts.iter().filter(|k| {
        k.keeps.contains(&crate::footprint::Keep::Placements) && k.layers.contains(&outer)
    }) {
        if measure::touches(&b, &Body::of_shape(&k.shape, opts)) != Verdict::No {
            return Some(k.reference.clone());
        }
    }
    for p in pads {
        let Some(pb) = p.copper.bounds() else {
            continue;
        };
        let mine = Body::of_shape(&p.copper, opts);
        let a: Vec<&str> = p.net.iter().map(String::as_str).collect();
        for o in others {
            if o.net.is_some() && o.net == p.net {
                continue;
            }
            if !(0..n).any(|l| p.on(l, n) && o.on[l]) {
                continue;
            }
            let theirs: Vec<&str> = o.net.iter().map(String::as_str).collect();
            let need = rules.pair_clearance(&a, &theirs).value;
            if apart(pb, o.bounds, need) {
                continue;
            }
            if measure::closer(&mine, &o.body, need) != Verdict::No {
                return Some(o.reference.clone());
            }
        }
    }
    None
}

/// Whether one constraint holds for a candidate's copper and body.
fn holds(
    c: &Constraint,
    copper: &Body,
    body: &Shape,
    side: PlacementSide,
    s: &BoardShapes,
    target: Option<&Body>,
    opts: &BodyOptions,
) -> bool {
    match c {
        Constraint::Side { side: want } => *want == side,
        Constraint::Near { within_nm, .. } => {
            target.is_some_and(|t| measure::within(copper, t, *within_nm) == Verdict::Yes)
        }
        Constraint::Edge {
            edge: EdgeSide::Any,
            within_nm,
        } => {
            measure::within(
                &Body::of_shape(body, opts),
                &Body::of_shape(&s.edges, opts),
                *within_nm,
            ) == Verdict::Yes
        }
        Constraint::Edge { edge, within_nm } => match (s.outline.bounds(), body.bounds()) {
            (Some(o), Some(b)) => from_side(o, b, *edge) <= *within_nm,
            _ => false,
        },
    }
}

fn centre(r: Rect) -> Point {
    Point::new(
        r.min.x + (r.max.x - r.min.x) / 2,
        r.min.y + (r.max.y - r.min.y) / 2,
    )
}

/// The point of `side` of the box `o` nearest `p`.
fn on_side(o: Rect, p: Point, side: EdgeSide) -> Point {
    let cx = p.x.clamp(o.min.x, o.max.x);
    let cy = p.y.clamp(o.min.y, o.max.y);
    match side {
        EdgeSide::Left => Point::new(o.min.x, cy),
        EdgeSide::Right => Point::new(o.max.x, cy),
        EdgeSide::Bottom => Point::new(cx, o.min.y),
        EdgeSide::Top => Point::new(cx, o.max.y),
        EdgeSide::Any => [
            EdgeSide::Left,
            EdgeSide::Right,
            EdgeSide::Bottom,
            EdgeSide::Top,
        ]
        .into_iter()
        .map(|e| on_side(o, p, e))
        .min_by_key(|q| {
            let (dx, dy) = (i128::from(q.x - p.x), i128::from(q.y - p.y));
            (dx * dx + dy * dy, q.x, q.y)
        })
        .expect("four sides"),
    }
}

/// `board place`'s legaliser: the first point of the placement grid,
/// nearest the constraints' target first, where every constraint holds
/// and the part is clear. Answers `{at, searched_nm, tried}`.
pub fn legalise(
    board: &str,
    s: &BoardShapes,
    rules: &Rules,
    preset: &Preset,
    ask: &Ask,
) -> Result<Value> {
    let o = ask.occ;
    let f = o.footprint.ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            format!("{} has no footprint to place", o.reference),
        )
    })?;
    if let Some(Constraint::Side { side }) = ask
        .constraints
        .iter()
        .find(|c| matches!(c, Constraint::Side { side } if *side != ask.side))
    {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "the part must stay on the {}, but --side is {}",
                side.name(),
                ask.side.name()
            ),
        )
        .with("param", "side"));
    }
    let opts = BodyOptions::default();
    let n = s.n();
    let fields = super::file::PartFields::default();
    let place_at = |p: Point| Place {
        at: p,
        rot: Mdeg::new(ask.rotation_mdeg),
        mirror: ask.side == PlacementSide::Bottom,
        scale: None,
    };
    // The part's copper radius about its origin.
    let (pads0, _) =
        super::shapes::part(board, o, f, place_at(Point::new(0, 0)), preset, n, &fields);
    let r_part = union(pads0.iter().map(|p| &p.copper))
        .bounds()
        .map_or(0, |b| {
            [b.min.x, b.max.x, b.min.y, b.max.y]
                .into_iter()
                .map(i64::abs)
                .max()
                .unwrap_or(0)
        });
    // Each near constraint's target body, and the search's target and reach.
    let mut targets: Vec<Option<Body>> = Vec::new();
    let mut aim: Option<(Point, i64)> = None;
    for c in ask.constraints {
        match c {
            Constraint::Near { near, within_nm } => {
                let (_, t) = target(s, board, near).ok_or_else(|| {
                    Error::new(
                        Code::NotFound,
                        format!("board {board} has no pad or part {near}"),
                    )
                    .with("ref", near)
                })?;
                let tb = t.bounds().expect("a target has copper");
                if aim.is_none() {
                    let half = ((tb.max.x - tb.min.x) + (tb.max.y - tb.min.y)) / 2;
                    aim = Some((centre(tb), within_nm + r_part + half));
                }
                targets.push(Some(Body::of_shape(&t, &opts)));
            }
            Constraint::Edge { edge, within_nm } => {
                targets.push(None);
                if aim.is_none() {
                    let ob = s.outline.bounds().ok_or_else(|| {
                        Error::new(Code::InvalidParam, "the board has no outline")
                    })?;
                    let from = ask.now.map_or(centre(ob), |a| Point::new(a[0], a[1]));
                    aim = Some((on_side(ob, from, *edge), within_nm + 2 * r_part));
                }
            }
            Constraint::Side { .. } => targets.push(None),
        }
    }
    let Some((t, reach)) = aim else {
        return Err(Error::new(
            Code::InvalidParam,
            "the legaliser needs --near or --edge",
        ));
    };
    let g = rules.placement_grid().value.max(1);
    let snap = |v: i64| v.div_euclid(g) * g + if v.rem_euclid(g) * 2 >= g { g } else { 0 };
    let (tx, ty) = (snap(t.x), snap(t.y));
    let most = ((MAX_CANDIDATES as f64).sqrt() as i64 - 1) / 2;
    let k = (reach / g + 1).min(most);
    let mut cands: Vec<Point> = Vec::with_capacity(((2 * k + 1) * (2 * k + 1)) as usize);
    for i in -k..=k {
        for j in -k..=k {
            cands.push(Point::new(tx + i * g, ty + j * g));
        }
    }
    cands.sort_by_key(|p| {
        let (dx, dy) = (i128::from(p.x - t.x), i128::from(p.y - t.y));
        (dx * dx + dy * dy, p.x, p.y)
    });
    let owner = format!("{board}:{}", o.label());
    let others = others(board, s, &o.reference, &opts);
    let mut first_block: Option<String> = None;
    for (tried, p) in cands.iter().enumerate() {
        let (pads, court) = super::shapes::part(board, o, f, place_at(*p), preset, n, &fields);
        let copper = union(pads.iter().map(|x| &x.copper));
        let body = court.map(|c| c.shape).unwrap_or_else(|| copper.clone());
        let cb = Body::of_shape(&copper, &opts);
        let ok = ask
            .constraints
            .iter()
            .zip(&targets)
            .all(|(c, tb)| holds(c, &cb, &body, ask.side, s, tb.as_ref(), &opts));
        if !ok {
            continue;
        }
        match blocked(s, rules, &owner, ask.side, &pads, &body, &others, &opts) {
            None => return Ok(json!({"at": [p.x, p.y], "searched_nm": k * g, "tried": tried + 1})),
            Some(b) => {
                first_block.get_or_insert(b);
            }
        }
    }
    let by = first_block.unwrap_or_else(|| "constraint".into());
    Err(Error::new(
        Code::PlacementNoRoom,
        format!("no point of the {g} nm grid within {} nm of the target holds {}'s constraints and is clear; the nearest that held was blocked by {by}", k * g, o.label()),
    )
    .with("blocked_by", by)
    .with("searched_nm", k * g))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::preset::JLC_2L;
    use crate::board::rules::Rules;
    use crate::board::testing::{MM, UM, board};

    fn r(fp: &crate::footprint::Footprint) -> Occ<'_> {
        Occ {
            reference: "r9".into(),
            refdes: Some("R9".into()),
            value: None,
            footprint_ref: Some("lib/x/footprint/r".into()),
            footprint: Some(fp),
            footprint_error: None,
            nets: [
                ("1".to_owned(), "a".to_owned()),
                ("2".to_owned(), "b".to_owned()),
            ]
            .into_iter()
            .collect(),
            dnp: false,
        }
    }

    fn rules() -> Rules {
        Rules::from_parts(std::sync::Arc::new(JLC_2L), None, Default::default())
    }

    #[test]
    fn the_legaliser_takes_the_nearest_grid_point_that_holds_and_is_clear() {
        let fp = crate::footprint::tests::r0402();
        let o = r(&fp);
        let mut s = board(20 * MM, 10 * MM);
        s.smd("u1", "7", (10 * MM, 5 * MM), (MM, MM), Some("a"))
            .courtyard("u1", (10 * MM, 5 * MM), (2 * MM, 2 * MM), false);
        let c = [Constraint::Near {
            near: "u1/7".into(),
            within_nm: MM,
        }];
        let ask = Ask {
            occ: &o,
            rotation_mdeg: 0,
            side: PlacementSide::Top,
            constraints: &c,
            now: None,
        };
        let v = legalise("b", &s, &rules(), &JLC_2L, &ask).unwrap();
        let at = v["at"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_i64().unwrap())
            .collect::<Vec<_>>();
        // On the 0.1 mm grid, clear of u1's courtyard (to x ≥ 11 mm, its
        // own reaching 0.93 mm), within 1 mm of the pad: nothing nearer.
        assert_eq!(at[0] % (100 * UM), 0);
        assert_eq!(at[1] % (100 * UM), 0);
        let placed = |x: i64, y: i64| {
            let place = ryu_board::geom::Place {
                at: ryu_board::geom::Point::new(x, y),
                ..Default::default()
            };
            super::super::shapes::part("b", &o, &fp, place, &JLC_2L, 2, &Default::default())
        };
        let (pads, court) = placed(at[0], at[1]);
        let opts = BodyOptions::default();
        let copper = Body::of_shape(&union(pads.iter().map(|p| &p.copper)), &opts);
        let pad = Body::of_shape(&s.pads[0].copper, &opts);
        assert_eq!(measure::within(&copper, &pad, MM), Verdict::Yes);
        let theirs = Body::of_shape(&s.courtyards[0].shape, &opts);
        assert_eq!(
            measure::touches(&Body::of_shape(&court.unwrap().shape, &opts), &theirs),
            Verdict::No
        );
        // Asked again, the same answer: the order is total.
        assert_eq!(legalise("b", &s, &rules(), &JLC_2L, &ask).unwrap(), v);
    }

    #[test]
    fn the_legaliser_keeps_out_of_placement_keepouts() {
        let fp = crate::footprint::tests::r0402();
        let o = r(&fp);
        let mut s = board(20 * MM, 10 * MM);
        s.smd("u1", "7", (10 * MM, 5 * MM), (MM, MM), Some("a"))
            .courtyard("u1", (10 * MM, 5 * MM), (2 * MM, 2 * MM), false);
        let c = [Constraint::Near {
            near: "u1/7".into(),
            within_nm: MM,
        }];
        let ask = Ask {
            occ: &o,
            rotation_mdeg: 0,
            side: PlacementSide::Top,
            constraints: &c,
            now: None,
        };
        let first = legalise("b", &s, &rules(), &JLC_2L, &ask).unwrap();
        let x = first["at"][0].as_i64().unwrap();
        let y = first["at"][1].as_i64().unwrap();
        let keep = crate::board::testing::rect(ryu_board::geom::Point::new(x, y), MM / 2, MM / 2);
        s.keepouts.push(crate::board::shapes::KeepoutInfo {
            reference: "b:keepout/k1".into(),
            part: None,
            layers: vec![0],
            keeps: vec![crate::footprint::Keep::Placements],
            shape: keep.clone(),
        });
        let second = legalise("b", &s, &rules(), &JLC_2L, &ask).unwrap();
        assert_ne!(second["at"], first["at"]);
        let place = ryu_board::geom::Place {
            at: ryu_board::geom::Point::new(
                second["at"][0].as_i64().unwrap(),
                second["at"][1].as_i64().unwrap(),
            ),
            ..Default::default()
        };
        let (_, court) =
            super::super::shapes::part("b", &o, &fp, place, &JLC_2L, 2, &Default::default());
        let opts = BodyOptions::default();
        assert_eq!(
            measure::touches(
                &Body::of_shape(&court.unwrap().shape, &opts),
                &Body::of_shape(&keep, &opts)
            ),
            Verdict::No
        );
        // A keepout on the bottom doesn't hold a top part off.
        s.keepouts[0].layers = vec![1];
        assert_eq!(legalise("b", &s, &rules(), &JLC_2L, &ask).unwrap(), first);
    }

    #[test]
    fn no_room_names_what_blocked_the_nearest() {
        let fp = crate::footprint::tests::r0402();
        let o = r(&fp);
        let mut s = board(20 * MM, 10 * MM);
        // The target's courtyard is wide: nothing fits within 0.1 mm of its pad.
        s.smd("u1", "7", (10 * MM, 5 * MM), (MM, MM), Some("a"))
            .courtyard("u1", (10 * MM, 5 * MM), (6 * MM, 6 * MM), false);
        let c = [Constraint::Near {
            near: "u1/7".into(),
            within_nm: 100 * UM,
        }];
        let ask = Ask {
            occ: &o,
            rotation_mdeg: 0,
            side: PlacementSide::Top,
            constraints: &c,
            now: None,
        };
        let e = legalise("b", &s, &rules(), &JLC_2L, &ask).unwrap_err();
        assert_eq!(e.code, Code::PlacementNoRoom);
        assert_eq!(e.details["blocked_by"], "b:u1");
        // A side constraint against --side is refused at once.
        let c = [Constraint::Side {
            side: PlacementSide::Bottom,
        }];
        let ask = Ask {
            occ: &o,
            rotation_mdeg: 0,
            side: PlacementSide::Top,
            constraints: &c,
            now: None,
        };
        assert_eq!(
            legalise("b", &s, &rules(), &JLC_2L, &ask).unwrap_err().code,
            Code::InvalidParam
        );
    }

    #[test]
    fn an_edge_constraint_lands_against_its_side_inside_the_edge_clearance() {
        let fp = crate::footprint::tests::r0402();
        let o = r(&fp);
        let s = board(20 * MM, 10 * MM);
        let c = [Constraint::Edge {
            edge: EdgeSide::Left,
            within_nm: MM,
        }];
        let ask = Ask {
            occ: &o,
            rotation_mdeg: 0,
            side: PlacementSide::Top,
            constraints: &c,
            now: Some([10 * MM, 5 * MM]),
        };
        let v = legalise("b", &s, &rules(), &JLC_2L, &ask).unwrap();
        let x = v["at"][0].as_i64().unwrap();
        // The courtyard's left side (x - 0.93 mm) at least the 0.2 mm edge
        // clearance in and at most 1 mm: the nearest grid point is 1.2 mm.
        assert_eq!(x, 1_200_000, "{v}");
        assert_eq!(v["at"][1], 5 * MM, "level with where it was");
    }

    fn near(t: &str, w: i64) -> Constraint {
        Constraint::Near {
            near: t.into(),
            within_nm: w,
        }
    }

    /// Two parts' pads 3 mm apart, edge to edge (each pad 1 mm square).
    fn two() -> BoardShapes {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("c1", "1", (4 * MM, 5 * MM), (MM, MM), Some("a"))
            .smd("u1", "7", (8 * MM, 5 * MM), (MM, MM), Some("a"))
            .courtyard("c1", (4 * MM, 5 * MM), (2 * MM, 2 * MM), false);
        s
    }

    #[test]
    fn near_holds_at_its_distance_and_not_a_nanometre_under() {
        let s = two();
        let at = |w| judge_one("b", "c1", "c1", &s, &Placement::default(), &near("u1/7", w));
        assert!(at(3 * MM).is_none(), "exactly 3 mm holds");
        let f = at(3 * MM - 1).unwrap();
        assert_eq!(f.code, Code::PlacementConstraintUnmet);
        assert_eq!(f.details["distance_nm"], 3 * MM);
        assert_eq!(f.refs, ["b:c1", "b:u1/7"]);
        let gone = judge_one(
            "b",
            "c1",
            "c1",
            &s,
            &Placement::default(),
            &near("u9/1", MM),
        )
        .unwrap();
        assert_eq!(gone.details["missing"], true);
    }

    #[test]
    fn edges_are_measured_from_the_outlines_box() {
        let s = two(); // c1's courtyard spans x 3..5 mm, y 4..6 mm on a 20 x 10 mm board
        let e = |side, w| Constraint::Edge {
            edge: side,
            within_nm: w,
        };
        let j = |c: &Constraint| judge_one("b", "c1", "c1", &s, &Placement::default(), c);
        assert!(j(&e(EdgeSide::Left, 3 * MM)).is_none());
        assert_eq!(
            j(&e(EdgeSide::Left, 3 * MM - 1)).unwrap().details["distance_nm"],
            3 * MM
        );
        assert!(j(&e(EdgeSide::Bottom, 4 * MM)).is_none());
        assert!(j(&e(EdgeSide::Top, 4 * MM - 1)).is_some());
        assert!(j(&e(EdgeSide::Right, 15 * MM)).is_none());
        assert!(
            j(&e(EdgeSide::Any, 3 * MM)).is_none(),
            "the left edge is 3 mm off"
        );
        assert!(j(&e(EdgeSide::Any, 3 * MM - 1)).is_some());
    }

    #[test]
    fn a_side_constraint_is_the_placements_side() {
        let s = two();
        let bottom = Placement {
            side: PlacementSide::Bottom,
            ..Placement::default()
        };
        let c = Constraint::Side {
            side: PlacementSide::Top,
        };
        assert!(judge_one("b", "c1", "c1", &s, &Placement::default(), &c).is_none());
        assert!(judge_one("b", "c1", "c1", &s, &bottom, &c).is_some());
        let _ = UM;
    }
}
