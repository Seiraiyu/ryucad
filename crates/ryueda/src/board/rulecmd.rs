//! `board rules show | set | unset | explain` (design 924): a board's
//! `rules.json` written by key, and its resolved rules read back.

use super::cmd::{board_of, find};
use super::rules::{self, PourRule, RuleSet, RulesFile, ViaRule};
use super::{RULES_FILE, RULES_KIND};
use crate::coord;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::Project;
use crate::server::check_revision;
use crate::session::Session;
use crate::source::join;
use crate::write::{Writes, commit};
use serde_json::{Value, json};

/// Every key `set` and `unset` take.
pub const KEYS: [&str; 11] = [
    "clearance",
    "track_width",
    "via",
    "hole_clearance",
    "edge_clearance",
    "courtyard_clearance",
    "placement_grid",
    "pour.clearance",
    "pour.min_width",
    "pour.thermal_gap",
    "pour.thermal_spoke",
];

fn invalid(msg: impl Into<String>, param: &str) -> Error {
    Error::new(Code::InvalidParam, msg.into()).with("param", param)
}

fn path(id: &Id) -> String {
    join(&super::file::board_dir(id), RULES_FILE)
}

fn file_of(b: &super::load::BoardFiles) -> Result<RulesFile> {
    match &b.rules {
        None => Ok(RulesFile::default()),
        Some(l) => l.item.clone(),
    }
}

/// The rule set `class` names (the defaults without one).
fn set_of<'a>(f: &'a mut RulesFile, class: Option<&str>) -> Result<&'a mut RuleSet> {
    match class {
        None => Ok(&mut f.defaults),
        Some(c) => Ok(f.classes.entry(Id::parse(c, "class")?).or_default()),
    }
}

fn apply(s: &mut RuleSet, key: &str, value: Option<&str>, unit: crate::units::Unit) -> Result<()> {
    let len = |v: &str| coord::nm(&json!(v), unit);
    match (key, value) {
        ("clearance", v) => s.clearance_nm = v.map(len).transpose()?,
        ("track_width", v) => s.track_width_nm = v.map(len).transpose()?,
        ("hole_clearance", v) => s.hole_clearance_nm = v.map(len).transpose()?,
        ("edge_clearance", v) => s.edge_clearance_nm = v.map(len).transpose()?,
        ("courtyard_clearance", v) => s.courtyard_clearance_nm = v.map(len).transpose()?,
        ("placement_grid", v) => s.placement_grid_nm = v.map(len).transpose()?,
        ("via", None) => s.via = None,
        ("via", Some(v)) => {
            let (d, dia) = v
                .split_once(',')
                .ok_or_else(|| invalid("via=DRILL,DIAMETER", "via"))?;
            s.via = Some(ViaRule {
                drill_nm: len(d)?,
                diameter_nm: len(dia)?,
            });
        }
        (k, v) if k.starts_with("pour.") => {
            // Take the pour, edit it, put it back (or none when emptied).
            let mut p = s.pour.unwrap_or_default();
            let x = v.map(len).transpose()?;
            match &k[5..] {
                "clearance" => p.clearance_nm = x,
                "min_width" => p.min_width_nm = x,
                "thermal_gap" => p.thermal_gap_nm = x,
                "thermal_spoke" => p.thermal_spoke_nm = x,
                _ => {
                    return Err(invalid(
                        format!("{k} isn't a rule; rules are {}", KEYS.join(", ")),
                        "set",
                    ));
                }
            }
            s.pour = (p != PourRule::default()).then_some(p);
        }
        (k, _) => {
            return Err(invalid(
                format!("{k} isn't a rule; rules are {}", KEYS.join(", ")),
                "set",
            ));
        }
    }
    Ok(())
}

fn write(session: &mut Session, id: &Id, f: &RulesFile, what: &str) -> Result<()> {
    let mut w = Writes::default();
    if *f == RulesFile::default() {
        w.delete(path(id));
    } else {
        w.put(path(id), f, RULES_KIND);
    }
    commit(session, what, w)
}

/// `board.rules.show`: the file as written and the rules resolved.
pub fn show(p: &Project, board: &str) -> Result<Value> {
    let b = find(p, board)?;
    let r = rules::of(p, b)?;
    let file = match &b.rules {
        None => Value::Null,
        Some(l) => {
            serde_json::to_value(l.item.as_ref().map_err(Clone::clone)?).expect("serialises")
        }
    };
    Ok(
        json!({"board": format!("board/{}", b.id), "revision": b.revision, "path": path(&b.id), "file": file, "rules": r.to_json()}),
    )
}

/// `board.rules.set` and `board.rules.unset`: keys set (`key=value`) or
/// removed on the defaults or a class; a class left empty goes, and an
/// empty file is deleted. The write is refused when it adds a fault (a
/// value under the preset, a class the design lacks).
pub fn set(
    session: &mut Session,
    board: &str,
    class: Option<&str>,
    pairs: &[String],
    unset: bool,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let unit = p.manifest.default_unit;
    let b = find(p, board)?;
    check_revision(&format!("board {}", b.id), &b.revision, expected)?;
    let id = b.id.clone();
    board_of(b)?;
    let mut f = file_of(b)?;
    if pairs.is_empty() {
        return Err(invalid(
            if unset {
                "give the keys to remove"
            } else {
                "give key=value pairs"
            },
            "set",
        ));
    }
    {
        let s = set_of(&mut f, class)?;
        for kv in pairs {
            if unset {
                apply(s, kv, None, unit)?;
            } else {
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| invalid(format!("{kv} isn't key=value"), "set"))?;
                apply(s, k, Some(v), unit)?;
            }
        }
    }
    f.classes.retain(|_, s| !s.is_empty());
    let before = super::drc::now(session, &id);
    write(session, &id, &f, &format!("board {id} rules"))?;
    let mut v = show(session.project(), id.as_str())?;
    v["drc"] = super::drc::delta(session, &id, before);
    Ok(v)
}

/// The nets a reference's copper claims: a pad's, a track's, a via's, or
/// `net:<name>`'s own.
fn nets_of(
    s: &super::shapes::BoardShapes,
    board: &str,
    r: &str,
) -> Option<(String, Option<String>)> {
    let full = if r.contains(':') && !r.starts_with("net:") {
        r.to_owned()
    } else {
        format!("{board}:{r}")
    };
    if let Some(n) = r.strip_prefix("net:") {
        return Some((r.to_owned(), Some(n.to_owned())));
    }
    if let Some(pd) = s.pads.iter().find(|p| p.reference == full) {
        return Some((full, pd.net.clone()));
    }
    if let Some(t) = s
        .tracks
        .iter()
        .find(|t| full == format!("{board}:track/{}", t.id))
    {
        return Some((full, Some(t.net.clone())));
    }
    if let Some(v) = s
        .vias
        .iter()
        .find(|v| full == format!("{board}:via/{}", v.id))
    {
        return Some((full, Some(v.net.clone())));
    }
    None
}

/// `board.rules.explain`: each reference's net and class with the rules
/// that apply to it and where each came from; for two, the pair's
/// clearance and the side that set it.
pub fn explain(session: &mut Session, board: &str, refs: &[String]) -> Result<Value> {
    if refs.is_empty() || refs.len() > 2 {
        return Err(invalid("give one reference or two", "refs"));
    }
    let id = find(session.project(), board)?.id.clone();
    let shapes = session.board_shapes(&id)?;
    let p = session.project();
    let r = rules::of(p, &p.boards[&id])?;
    let mut items = Vec::new();
    let mut nets = Vec::new();
    for x in refs {
        let (full, net) = nets_of(&shapes, id.as_str(), x).ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("board {id} has no pad, track, via or net {x}"),
            )
            .with("ref", x)
        })?;
        let class = net.as_ref().and_then(|n| r.class_of.get(n)).cloned();
        items.push(json!({
            "reference": full,
            "net": net,
            "class": class,
            "clearance_nm": r.clearance(net.as_deref()),
            "track_width_nm": r.track_width_min(net.as_deref()),
            "edge_clearance_nm": r.edge_clearance(net.as_deref()),
            "hole_clearance_nm": r.hole_clearance(net.as_deref()),
        }));
        nets.push(net);
    }
    let mut v = json!({"board": format!("board/{id}"), "preset": r.preset.name, "items": items});
    if let [a, b] = nets.as_slice() {
        fn side(n: &Option<String>) -> Vec<&str> {
            n.iter().map(String::as_str).collect()
        }
        v["pair"] = json!({"clearance_nm": r.pair_clearance(&side(a), &side(b))});
    }
    Ok(v)
}

/// `board.waive`: a finding waived with a reason (its measurement kept as
/// the limit), or its waiver removed.
pub fn waive(
    session: &mut Session,
    board: &str,
    finding: &str,
    reason: Option<&str>,
    remove: bool,
    by: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let b = find(p, board)?;
    check_revision(&format!("board {}", b.id), &b.revision, expected)?;
    let id = b.id.clone();
    board_of(b)?;
    let mut f = file_of(b)?;
    if remove {
        if f.waivers.remove(finding).is_none() {
            return Err(Error::new(
                Code::FindingNotFound,
                format!("board {id} has no waiver {finding}"),
            )
            .with("finding", finding));
        }
    } else {
        let reason = reason
            .filter(|r| !r.trim().is_empty())
            .ok_or_else(|| invalid("a waiver needs --reason", "reason"))?;
        let c = session.board_check(&id)?;
        let found = c.findings.iter().find(|x| x.id == finding).ok_or_else(|| {
            Error::new(
                Code::FindingNotFound,
                format!("board {id} has no finding {finding}; `ryueda board drc {id}` lists them"),
            )
            .with("finding", finding)
        })?;
        if rules::NOT_WAIVABLE.contains(&found.code) {
            return Err(Error::new(
                Code::FindingNotWaivable,
                format!(
                    "{} can't be waived: fix it",
                    json!(found.code).as_str().unwrap_or("")
                ),
            )
            .with("finding", finding));
        }
        let limit = rules::measurement(found.code, &found.details).map(|(v, _)| v);
        f.waivers.insert(
            finding.to_owned(),
            rules::Waiver {
                code: found.code,
                refs: found.refs.clone(),
                reason: reason.to_owned(),
                by: by.to_owned(),
                limit,
            },
        );
    }
    let before = super::drc::now(session, &id);
    write(session, &id, &f, &format!("board {id} waiver"))?;
    let mut v = show(session.project(), id.as_str())?;
    v["drc"] = super::drc::delta(session, &id, before);
    Ok(v)
}
