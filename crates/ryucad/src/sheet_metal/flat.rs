//! The flat pattern of a solid as its part shows it (design 346's The
//! flat pattern; plan 16.B's decision "The flat of a solid as shown").
//! While an unfold of the solid is in force (no refold of it since), the
//! shown body is the flat, cut since perhaps: its pattern is redrawn from
//! it through the unfold's (`ryukei::sheet_metal::redraw`), the bend
//! lines the unfold's. Otherwise the shown body is recognized and unfolded
//! whole in memory about `fixed` (else the kernel's default flange), each
//! bend at its override's allowance, else the founder's; nothing is
//! written. The session keeps the last [`KEPT`].

use crate::doc::id::Id;
use crate::doc::reference::Reference;
use crate::doc::sheet_metal::Allowance;
use crate::error::{Code, Error, Result};
use crate::rebuild::Built;
use crate::session::Session;
use crate::solid::Solids;
use ryukei::name::Name;
use ryukei::sheet_metal::{FlatPattern, SheetOptions, UnfoldSpec};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

/// How many flats a session keeps.
pub const KEPT: usize = 16;

/// What the caller asks of a folded solid's flat: the side face held
/// still, and allowance overrides `{bend, k_factor | deduction |
/// allowance}`, each `bend` a reference `<part>/<solid>/<name>` to a face
/// of the bend, the sizes literals. Neither applies to a flat in force.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub fixed: Option<String>,
    pub bends: Vec<Value>,
}

/// Where a flat came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum From {
    /// The unfold in force, by its ID.
    Unfold(Id),
    /// The folded solid, unfolded whole in memory.
    Whole,
}

/// A solid's flat.
#[derive(Debug)]
pub struct FlatOf {
    pub part: Id,
    pub solid: Id,
    /// The feature whose result shows.
    pub feature: Id,
    pub from: From,
    /// The flat body: the shown one, or the one unfolded in memory, every
    /// name the shown body's.
    pub body: Arc<Built>,
    pub pattern: FlatPattern,
    /// Each bend line's bend, as `sheet.get` names it (its least inner
    /// face), in the pattern's order.
    pub bends: Vec<String>,
    /// Its display mesh, made the first time a render asks.
    pub mesh: OnceLock<Arc<crate::mesh::DisplayMesh>>,
}

impl FlatOf {
    /// A reference to `name` of the solid.
    pub fn at(&self, name: Name) -> String {
        format!("{}/{}/{name}", self.part, self.solid)
    }
}

/// The last flats asked for, by key, the oldest first.
#[derive(Debug, Default)]
pub struct Cache {
    held: VecDeque<(String, Arc<FlatOf>)>,
}

impl Cache {
    fn get(&self, key: &str) -> Option<Arc<FlatOf>> {
        self.held
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, f)| Arc::clone(f))
    }

    fn put(&mut self, key: String, f: Arc<FlatOf>) {
        if self.held.len() == KEPT {
            self.held.pop_front();
        }
        self.held.push_back((key, f));
    }
}

fn invalid(pointer: &str, message: String) -> Error {
    Error::new(Code::InvalidParam, message).with("pointer", pointer)
}

/// The flat of `part`'s solid (`solid`, else its one sheet metal solid)
/// as it shows at `rev` (a Git revision, else the working files), and the
/// commit read. Refusals: the solid's choice as `sheet.get` refuses it,
/// `fixed` or `bends` given while an unfold is in force or naming no face
/// of the right kind (`invalid_param` at its pointer), a bend with no
/// allowance where no base flange founded the solid (`invalid_param` at
/// `/bends`), and the kernel's as `sheet_metal_failed`.
pub fn of(
    session: &mut Session,
    part: &str,
    solid: Option<&str>,
    rev: Option<&str>,
    o: &Options,
) -> Result<(Option<String>, Arc<FlatOf>)> {
    let (commit, s) = session.at(rev)?;
    let pid = crate::server::part_id(part)?;
    let p = s.project().part(&pid)?.clone();
    let solids = Solids::of(&p);
    let r = s.rebuilt(&pid)?.clone();
    let (solid, feature, built) = super::report::pick(&p, &r, &solids, solid)?;
    let founder = super::founder(&p, &solid);
    let in_force = super::unfold::in_force(&p, &solids, &solid, &feature, &r);
    let key = json!({
        "body": built.hash,
        "solid": solid,
        "unfold": in_force.as_ref().map(|(u, _)| u),
        "allowance": founder.map(|f| f.allowance.to_json()),
        "fixed": o.fixed,
        "bends": o.bends,
    })
    .to_string();
    if let Some(f) = s.flats.get(&key) {
        return Ok((commit, f));
    }
    let options = SheetOptions::default();
    let kernel =
        |e: ryukei::Error| super::refusal(&pid, &solid, "flat", (&solid, built.as_ref()), e);
    let f = match in_force {
        Some((u, flat)) => {
            if o.fixed.is_some() {
                return Err(invalid(
                    "/fixed",
                    format!("unfold {u} of {solid} is in force: its own fixed face made this flat"),
                ));
            }
            if !o.bends.is_empty() {
                return Err(invalid(
                    "/bends",
                    format!("unfold {u} of {solid} is in force: its own allowances made this flat"),
                ));
            }
            let pattern = ryukei::sheet_metal::redraw(&built.body, &flat.pattern, &options)
                .map_err(kernel)?;
            let bends = pattern
                .bend_lines
                .iter()
                .map(|l| {
                    let least = flat
                        .bends
                        .iter()
                        .find(|b| b.name == l.bend)
                        .and_then(|b| b.inner.first().copied())
                        .unwrap_or(l.bend);
                    format!("{pid}/{solid}/{least}")
                })
                .collect();
            FlatOf {
                part: pid.clone(),
                solid: solid.clone(),
                feature,
                from: From::Unfold(u),
                body: built,
                pattern,
                bends,
                mesh: OnceLock::new(),
            }
        }
        None => {
            let model = ryukei::sheet_metal::recognize(&built.body, &options).map_err(kernel)?;
            // A name of the solid from a reference given as text.
            let name = |text: &str, pointer: &str| -> Result<Name> {
                let r = Reference::parse(text, &pid).map_err(|e| e.with("pointer", pointer))?;
                if r.feature != solid {
                    return Err(invalid(pointer, format!("{text} isn't of solid {solid}")));
                }
                Ok(r.name)
            };
            let fixed = match &o.fixed {
                None => None,
                Some(t) => {
                    let n = name(t, "/fixed")?;
                    let side = model
                        .flanges
                        .iter()
                        .any(|f| f.sides.iter().any(|s| s.faces.contains(&n)));
                    if !side {
                        return Err(invalid(
                            "/fixed",
                            format!("{t} is no side face of a flange of {solid}"),
                        ));
                    }
                    Some(n)
                }
            };
            let mut given: Vec<(Name, Allowance)> = Vec::new();
            for (i, v) in o.bends.iter().enumerate() {
                let at = format!("/bends/{i}");
                let mut rest = v.as_object().cloned().ok_or_else(|| {
                    invalid(
                        &at,
                        "an override is {bend, k_factor | deduction | allowance}".into(),
                    )
                })?;
                let text = rest
                    .remove("bend")
                    .and_then(|b| b.as_str().map(str::to_owned))
                    .ok_or_else(|| {
                        invalid(&format!("{at}/bend"), "an override names its bend".into())
                    })?;
                let n = name(&text, &format!("{at}/bend"))?;
                let bend = model
                    .bends
                    .iter()
                    .find(|b| b.inner.contains(&n) || b.outer.contains(&n))
                    .ok_or_else(|| {
                        invalid(
                            &format!("{at}/bend"),
                            format!("{text} is no face of a bend of {solid}"),
                        )
                    })?;
                let a = Allowance::from_json(
                    &Value::Object(rest),
                    &at,
                    s.project().manifest.default_unit,
                    &p.env,
                    false,
                )?;
                given.push((bend.name, a));
            }
            let t = model.thickness;
            let mut overrides = Vec::with_capacity(model.bends.len());
            for b in &model.bends {
                let reference = format!("{pid}/{solid}/{}", b.inner[0]);
                let a = match given.iter().rev().find(|(n, _)| *n == b.name) {
                    Some((_, a)) => a,
                    None => match founder {
                        Some(f) => &f.allowance,
                        None => {
                            return Err(invalid("/bends", format!(
                                "bend {reference} has no allowance: {solid} wasn't founded by a base flange, so give every bend its own"
                            ))
                            .with("bend", &reference));
                        }
                    },
                };
                let k = super::allowance::kernel(a, b.radius, t, b.angle).map_err(|e| {
                    invalid("/bends", format!("bend {reference}: {}", e.message()))
                        .with("bend", &reference)
                })?;
                overrides.push((b.name, k));
            }
            let spec = UnfoldSpec {
                fixed,
                bends: None,
                // Every bend is overridden.
                allowance: ryukei::sheet_metal::Allowance::KFactor(0.5),
                overrides,
            };
            let key = crate::rebuild::op_key(&pid, &solid);
            s.job.check()?;
            let u = ryukei::sheet_metal::unfold(key, &built.body, &model, &spec).map_err(kernel)?;
            s.job.check()?;
            let body = crate::rebuild::finish(
                &solid,
                "flat",
                (u.body, u.journal),
                built.origins.as_ref().clone(),
            )?;
            let bends = u
                .pattern
                .bend_lines
                .iter()
                .map(|l| {
                    let least = model.bend(l.bend).map_or(l.bend, |b| b.inner[0]);
                    format!("{pid}/{solid}/{least}")
                })
                .collect();
            FlatOf {
                part: pid.clone(),
                solid: solid.clone(),
                feature,
                from: From::Whole,
                body: Arc::new(body),
                pattern: u.pattern,
                bends,
                mesh: OnceLock::new(),
            }
        }
    };
    let f = Arc::new(f);
    s.flats.put(key, Arc::clone(&f));
    Ok((commit, f))
}
