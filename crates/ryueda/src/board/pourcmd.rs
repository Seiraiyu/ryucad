//! `board pour add | edit | delete` (design 924). A pour's net and layer
//! are checked here; a contour that doesn't close is refused by the
//! commit with its fault's code. Every write refills the board's pours.

use super::cmd::board_of;
use super::file::{POUR_KIND, POURS, route_path};
use super::pour::{Connect, Islands, Pour, ThermalRule};
use super::route::{answer, field, named_layer, net_of, nets, positive, start};
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::session::Session;
use crate::units::Unit;
use crate::write::{Writes, commit, new_id};
use serde_json::{Value, json};

fn invalid(msg: impl Into<String>, param: &str) -> Error {
    Error::new(Code::InvalidParam, msg.into()).with("param", param)
}

/// The boundary an ask gives: `Some(None)` for the whole board,
/// `Some(Some(c))` for a contour or rectangle, `None` when it gives none.
fn boundary_of(a: &Value, unit: Unit) -> Result<Option<Option<super::file::Contour>>> {
    let whole = field(a, "whole").and_then(Value::as_bool).unwrap_or(false);
    let contour = match (field(a, "contour"), field(a, "rect")) {
        (Some(_), Some(_)) => {
            return Err(invalid("give --contour or --rect, not both", "contour"));
        }
        (Some(c), None) => Some(super::cmd::contour_from(c, unit)?),
        (None, Some(r)) => Some(super::keepcmd::rect(r, unit)?),
        (None, None) => None,
    };
    match (whole, contour) {
        (true, Some(_)) => Err(invalid(
            "a pour is --whole or a --rect or --contour, not both",
            "whole",
        )),
        (true, None) => Ok(Some(None)),
        (false, Some(c)) => Ok(Some(Some(c))),
        (false, None) => Ok(None),
    }
}

fn parsed<T: serde::de::DeserializeOwned>(a: &Value, key: &str, one_of: &str) -> Result<Option<T>> {
    field(a, key)
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|_| invalid(format!("--{key} is {one_of}, not {v}"), key))
        })
        .transpose()
}

/// The optional settings an ask gives, laid over `p`.
fn settle(p: &mut Pour, a: &Value, unit: Unit, rules: &super::rules::Rules) -> Result<()> {
    if let Some(v) = field(a, "priority") {
        p.priority = v
            .as_i64()
            .ok_or_else(|| invalid("--priority is a whole number", "priority"))?;
    }
    if let Some(v) = field(a, "clearance") {
        p.clearance_nm = Some(positive(v, unit, "clearance")?);
    }
    if let Some(v) = field(a, "min_width") {
        p.min_width_nm = Some(positive(v, unit, "min-width")?);
    }
    if let Some(c) = parsed::<Connect>(a, "connect", "thermal or solid")? {
        p.connect = c;
    }
    if let Some(i) = parsed::<Islands>(a, "islands", "remove or keep")? {
        p.islands = i;
    }
    let gap = field(a, "thermal_gap")
        .map(|v| positive(v, unit, "thermal-gap"))
        .transpose()?;
    let spoke = field(a, "thermal_spoke")
        .map(|v| positive(v, unit, "thermal-spoke"))
        .transpose()?;
    if gap.is_some() || spoke.is_some() {
        // One given: the other as the rules resolve it now.
        let now = rules.pour(p);
        p.thermal = Some(ThermalRule {
            gap_nm: gap.unwrap_or(now.gap.value),
            spoke_nm: spoke.unwrap_or(now.spoke.value),
        });
    }
    Ok(())
}

fn entry(id: &Id, pid: &Id, p: &Pour) -> Value {
    json!({"id": pid, "reference": format!("{id}:pour/{pid}"), "pour": super::file::value(p, POUR_KIND)})
}

/// `board.pour.add`: one pour or several (all or nothing).
pub fn add(
    session: &mut Session,
    board: &str,
    asks: Vec<Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    if asks.is_empty() {
        return Err(invalid(
            "give --net, --layer and --whole, --rect or --contour, or --ops",
            "ops",
        ));
    }
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = &p.boards[&id];
    let file = board_of(b)?;
    let n = super::stackup::copper_count(file);
    let rules = super::rules::of(p, b)?;
    let nets = nets(p, &file.design)?;
    let mut taken: std::collections::BTreeSet<String> =
        b.pours.keys().map(ToString::to_string).collect();
    let mut w = Writes::default();
    let mut made = Vec::new();
    for (i, a) in asks.iter().enumerate() {
        let net = field(a, "net")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("pour {i} needs --net"), "net"))?;
        let net = net_of(&nets, &file.design, net)?;
        let layer = field(a, "layer").and_then(Value::as_str).ok_or_else(|| {
            invalid(
                format!("pour {i} needs --layer (top, in1 …, bottom)"),
                "layer",
            )
        })?;
        let layer = named_layer(layer, n, "layer")?;
        let boundary = boundary_of(a, unit)?
            .ok_or_else(|| invalid("a pour needs --whole, --rect or --contour", "whole"))?;
        let mut pour = Pour::whole(&net, layer);
        if let Some(c) = boundary {
            pour.whole = false;
            pour.contour = Some(c);
        }
        settle(&mut pour, a, unit, &rules)?;
        let pid = new_id(field(a, "id").and_then(Value::as_str), "pour", |s| {
            taken.contains(s)
        })?;
        taken.insert(pid.to_string());
        w.put(route_path(&id, POURS, &pid), &pour, POUR_KIND);
        made.push(entry(&id, &pid, &pour));
    }
    commit(session, &format!("board {id} pour"), w)?;
    Ok(answer(session, &id, json!({"pours": made}), before))
}

/// `board.pour.edit`: the fields given replaced; `whole` and a contour
/// replace each other.
pub fn edit(
    session: &mut Session,
    board: &str,
    pour: &str,
    a: &Value,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, before) = start(session, board, expected)?;
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = &p.boards[&id];
    let file = board_of(b)?;
    let rules = super::rules::of(p, b)?;
    let pid = Id::parse(pour, "pour")?;
    let mut x = b
        .pours
        .get(&pid)
        .and_then(|l| l.item.as_ref().ok())
        .cloned()
        .ok_or_else(|| {
            Error::new(Code::NotFound, format!("board {id} has no pour {pour}")).with("pour", pour)
        })?;
    if let Some(l) = field(a, "layer").and_then(Value::as_str) {
        x.layer = named_layer(l, super::stackup::copper_count(file), "layer")?;
    }
    match boundary_of(a, unit)? {
        Some(None) => {
            x.whole = true;
            x.contour = None;
        }
        Some(Some(c)) => {
            x.whole = false;
            x.contour = Some(c);
        }
        None => {}
    }
    if let Some(v) = field(a, "cutouts") {
        let list = v
            .as_array()
            .ok_or_else(|| invalid("--cutouts is a JSON list of contours", "cutouts"))?;
        x.cutouts = list
            .iter()
            .map(|c| super::cmd::contour_from(c, unit))
            .collect::<Result<_>>()?;
    }
    settle(&mut x, a, unit, &rules)?;
    let mut w = Writes::default();
    w.put(route_path(&id, POURS, &pid), &x, POUR_KIND);
    commit(session, &format!("board {id} pour {pid}"), w)?;
    Ok(answer(
        session,
        &id,
        json!({"pours": [entry(&id, &pid, &x)]}),
        before,
    ))
}

pub fn delete(
    session: &mut Session,
    board: &str,
    ids: &[String],
    expected: Option<&str>,
) -> Result<Value> {
    super::route::delete(session, board, POURS, "pour", ids, expected)
}
