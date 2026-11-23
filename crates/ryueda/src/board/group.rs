//! `board group list | move | flip` (design 924): a group is a name on
//! its members' placements; moves are rigid, exact through `Place`, and
//! all or nothing. Tracks don't follow: the DRC delta says what broke.

use super::cmd::{board_of, find};
use super::file::{PLACEMENT_KIND, Placement, PlacementSide, placement_path};
use crate::coord;
use crate::error::{Code, Error, Result};
use crate::project::Project;
use crate::server::check_revision;
use crate::session::Session;
use crate::write::{Writes, commit};
use ryu_board::geom::{Mdeg, Place, Point};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub enum Move {
    By([i64; 2]),
    Rotate(i64),
    Flip,
}

/// The integer midpoint of the box of `pts`, rounding down.
pub fn centre(pts: &[[i64; 2]]) -> Point {
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for p in pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    Point::new((x0 + x1).div_euclid(2), (y0 + y1).div_euclid(2))
}

/// One member moved.
pub fn moved(pl: &Placement, m: &Move, c: Point) -> Placement {
    let mut out = pl.clone();
    match m {
        Move::By(d) => out.at = [pl.at[0] + d[0], pl.at[1] + d[1]],
        Move::Rotate(a) => {
            let turn = Place {
                at: c,
                rot: Mdeg::new(*a),
                mirror: false,
                scale: None,
            };
            let q = turn.point(Point::new(pl.at[0] - c.x, pl.at[1] - c.y));
            out.at = [q.x, q.y];
            out.rotation_mdeg = (pl.rotation_mdeg + a).rem_euclid(360_000);
        }
        Move::Flip => {
            out.at = [2 * c.x - pl.at[0], pl.at[1]];
            out.side = match pl.side {
                PlacementSide::Top => PlacementSide::Bottom,
                PlacementSide::Bottom => PlacementSide::Top,
            };
            out.rotation_mdeg = (-pl.rotation_mdeg).rem_euclid(360_000);
        }
    }
    out
}

/// `board.group.list`: each group and its members, sorted.
pub fn list(p: &Project, board: &str) -> Result<Value> {
    let b = find(p, board)?;
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (occ, l) in &b.placements {
        if let Ok(pl) = &l.item
            && let Some(g) = &pl.group
        {
            groups.entry(g).or_default().push(occ);
        }
    }
    Ok(json!({
        "board": format!("board/{}", b.id),
        "groups": groups.iter().map(|(g, m)| json!({"group": g, "members": m})).collect::<Vec<_>>(),
    }))
}

/// `board.group.move` and `board.group.flip`.
pub fn apply(
    session: &mut Session,
    board: &str,
    group: &str,
    m: Move,
    about: Option<&Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = find(p, board)?;
    check_revision(&format!("board {}", b.id), &b.revision, expected)?;
    board_of(b)?;
    let id = b.id.clone();
    let members: Vec<(String, Placement)> = b
        .placements
        .iter()
        .filter_map(|(o, l)| {
            l.item
                .as_ref()
                .ok()
                .filter(|pl| pl.group.as_deref() == Some(group))
                .map(|pl| (o.clone(), pl.clone()))
        })
        .collect();
    if members.is_empty() {
        return Err(Error::new(
            Code::NotFound,
            format!("board {id} has no group {group}; `ryueda board group list {id}` lists them"),
        )
        .with("group", group));
    }
    let c = match about {
        Some(v) => {
            let q = coord::point(v, unit)?;
            Point::new(q[0], q[1])
        }
        None => centre(&members.iter().map(|(_, pl)| pl.at).collect::<Vec<_>>()),
    };
    let before = super::drc::now(session, &id);
    let mut w = Writes::default();
    let mut out = Vec::new();
    for (o, pl) in &members {
        let n = moved(pl, &m, c);
        w.put(placement_path(&id, o), &n, PLACEMENT_KIND);
        out.push(json!({"occurrence": o, "at": n.at, "rotation_mdeg": n.rotation_mdeg, "side": n.side.name()}));
    }
    commit(session, &format!("board {id} group {group}"), w)?;
    let b = &session.project().boards[&id];
    let mut v = json!({"board": format!("board/{id}"), "revision": b.revision, "group": group, "moved": out});
    v["drc"] = super::drc::delta(session, &id, before);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: i64, y: i64, r: i64) -> Placement {
        Placement {
            at: [x, y],
            rotation_mdeg: r,
            ..Placement::default()
        }
    }

    #[test]
    fn moves_turns_and_flips_are_exact_and_rigid() {
        let c = Point::new(12, 10);
        assert_eq!(moved(&at(10, 10, 0), &Move::By([1, 2]), c).at, [11, 12]);
        let t = moved(&at(10, 10, 270_000), &Move::Rotate(90_000), c);
        assert_eq!((t.at, t.rotation_mdeg), ([12, 8], 0));
        let f = moved(&at(10, 10, 90_000), &Move::Flip, c);
        assert_eq!(
            (f.at, f.side, f.rotation_mdeg),
            ([14, 10], PlacementSide::Bottom, 270_000)
        );
        let back = moved(&f, &Move::Flip, c);
        assert_eq!(
            (back.at, back.side, back.rotation_mdeg),
            ([10, 10], PlacementSide::Top, 90_000)
        );
    }

    #[test]
    fn the_centre_is_the_members_box_midpoint_rounded_down() {
        assert_eq!(centre(&[[0, 0], [5, 3]]), Point::new(2, 1));
        assert_eq!(centre(&[[-5, -3], [0, 0]]), Point::new(-3, -2));
    }
}
