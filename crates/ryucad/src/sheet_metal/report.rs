//! The sheet metal report (`sheet.get`): a solid recognized as the part
//! shows it, in the agent's terms. Every face is a reference
//! (`<part>/<solid>/<name>`); a bend is named by its least inner face.

use crate::doc::sheet_metal::Allowance;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use crate::solid::Solids;
use core::f64::consts::PI;
use ryukei::sheet_metal::{BendDirection, SheetOptions};
use serde_json::{Value, json};

pub fn get(
    session: &mut Session,
    part: &str,
    solid: Option<&str>,
    rev: Option<&str>,
) -> Result<Value> {
    let (commit, s) = session.at(rev)?;
    let pid = crate::server::part_id(part)?;
    let p = s.project().part(&pid)?.clone();
    let solids = Solids::of(&p);
    let r = s.rebuilt(&pid)?.clone();
    let shown = r.shown(&solids);
    let pick = match solid {
        Some(x) => shown
            .iter()
            .find(|y| y.solid.as_str() == x)
            .ok_or_else(|| {
                Error::new(Code::NotFound, format!("part {pid} has no solid {x}"))
                    .with("solid", x)
                    .with(
                        "solids",
                        shown.iter().map(|y| y.solid.as_str()).collect::<Vec<_>>(),
                    )
            })?,
        None => {
            let sheet: Vec<_> = shown
                .iter()
                .filter(|y| super::founder(&p, y.solid).is_some())
                .collect();
            match (shown.as_slice(), sheet.as_slice()) {
                ([one], _) => one,
                (_, [one]) => *one,
                _ => {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!("part {pid} has {} solids, {} of them founded by a base flange: name one", shown.len(), sheet.len()),
                    )
                    .with("param", "solid")
                    .with("solids", shown.iter().map(|y| y.solid.as_str()).collect::<Vec<_>>()));
                }
            }
        }
    };
    let solid = pick.solid.clone();
    let Some((_, built)) = pick.state else {
        return Err(pick.error.cloned().unwrap_or_else(|| {
            Error::new(
                Code::DependencyFailed,
                format!("solid {solid} didn't build"),
            )
            .with("solid", &solid)
        }));
    };
    let options = SheetOptions::default();
    let model = ryukei::sheet_metal::recognize(&built.body, &options)
        .map_err(|e| super::refusal(&pid, &solid, "sheet", (&solid, built), e))?;
    let at = |n: &ryukei::name::Name| json!(format!("{pid}/{solid}/{n}"));
    let all = |ns: &[ryukei::name::Name]| Value::Array(ns.iter().map(at).collect());
    let founder = super::founder(&p, &solid);
    let t = model.thickness;
    let bends: Vec<Value> = model
        .bends
        .iter()
        .map(|b| {
            let (developed, note) = match founder.map(|f| &f.allowance) {
                None => (None, Some("no base flange founded the solid, so it has no allowance")),
                Some(Allowance::KFactor(k)) => (Some(b.angle * (b.radius + k.number() * t)), None),
                Some(Allowance::Deduction(bd)) if b.angle < PI - 1e-9 => {
                    let ba = 2.0 * (b.radius + t) * ryukei::math::tan(b.angle / 2.0) - bd.meters();
                    if ba > ryukei::tol::LINEAR_RESOLUTION {
                        (Some(ba), None)
                    } else {
                        (None, Some("the deduction is longer than the bend's own length to its mould lines"))
                    }
                }
                Some(Allowance::Deduction(_)) => (None, Some("a bend deduction is defined under a half turn: give this bend its allowance when unfolding")),
                Some(Allowance::Length(q)) => (Some(q.meters()), None),
            };
            let mut v = json!({
                "bend": at(&b.inner[0]),
                "faces": {"inner": all(&b.inner), "outer": all(&b.outer)},
                "radius_m": b.radius,
                "angle_rad": b.angle,
                "angle_deg": b.angle.to_degrees(),
                "direction": match b.direction { BendDirection::Up => "up", BendDirection::Down => "down" },
                "flanges": b.flanges.map(|i| at(&model.flanges[i].name)),
                "developed_m": developed,
            });
            if let Some(n) = note {
                v["developed_note"] = json!(n);
            }
            v
        })
        .collect();
    let flanges: Vec<Value> = model
        .flanges
        .iter()
        .map(|f| json!({"flange": at(&f.name), "sides": [all(&f.sides[0].faces), all(&f.sides[1].faces)]}))
        .collect();
    let allowance = founder.map(|f| match &f.allowance {
        Allowance::KFactor(k) => json!({"k_factor": k.number()}),
        Allowance::Deduction(q) => json!({"deduction_m": q.meters()}),
        Allowance::Length(q) => json!({"allowance_m": q.meters()}),
    });
    let mut v = json!({
        "schema": 1,
        "part": pid,
        "solid": solid,
        "thickness_m": t,
        "tolerance_m": model.tolerance,
        "state": "folded",
        "tree": model.is_tree(),
        "allowance": allowance,
        "allowance_from": founder.map(|_| solid.clone()),
        "flanges": flanges,
        "bends": bends,
    });
    if let Some(c) = &model.cycle {
        v["cycle"] = json!({"bends": all(&c.bends), "faces": all(&c.faces)});
    }
    if let Some(c) = commit {
        v["commit"] = json!(c);
    }
    Ok(v)
}
