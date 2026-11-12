//! Inserts: another part's solids, founded in this part's frame (ryucad 27.C).

use super::{BodyUse, FeatureKind, Write};
use super::{Given, apply_placement, new_placement};
use crate::doc::assembly::Rev;
use crate::doc::files::{Feature, PartKind, Placement};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::part::{part_kind, proposed};
use crate::rebuild;
use crate::session::Session;
use crate::units::Unit;
use serde_json::{Value, json};

/// The fields an insert takes: its source (`of`), the revision it pins
/// and its placement.
pub(crate) const INSERT: [&str; 3] = ["of", "rev", "placement"];

/// An insert from a request: `of` (a part, or `<dependency>:<part>`),
/// `rev` (`working`, a label; left out: as `current` has it, or for a
/// new insert the source's newest official label, else `working`) and
/// `placement`. Answers the insert and, for a new one with no `rev`,
/// which it took and why (design 521, question 624).
pub(crate) fn insert_from(
    session: &mut Session,
    current: Option<&crate::doc::insert::Insert>,
    given: &Given,
    unit: Unit,
    env: &Env,
) -> Result<(crate::doc::insert::Insert, Option<Value>)> {
    let deps = session.project().manifest.dependencies.clone();
    let f = &given.fields;
    let (part, project) = match (f.of.as_deref(), current) {
        (Some(of), _) => match of.split_once(':') {
            Some((dep, part)) => {
                let (id, _) =
                    crate::doc::deps::find(&deps, dep).map_err(|e| e.with("param", "of"))?;
                let id = id.clone();
                (
                    Id::parse(part, "part").map_err(|e| e.with("param", "of"))?,
                    Some(id),
                )
            }
            None => (
                Id::parse(of, "part").map_err(|e| e.with("param", "of"))?,
                None,
            ),
        },
        (None, Some(c)) => (c.part.clone(), c.rev.project().cloned()),
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                "an insert names the part it inserts (of)",
            )
            .with("param", "of"));
        }
    };
    let mut took = None;
    let rev = match (f.rev.as_deref(), current) {
        (Some("working"), _) => Rev::Working,
        (Some(l), _) => {
            Rev::Label(crate::doc::assembly::label_text(l).map_err(|e| e.with("param", "rev"))?)
        }
        (None, Some(c)) if c.part == part => match &c.rev {
            Rev::Foreign { label, .. } => Rev::Label(label.clone()),
            r => r.clone(),
        },
        (None, _) => match crate::release::newest_official(session, &part, project.as_ref())? {
            Some(l) => {
                took = Some(json!({"rev": {"label": l}, "why": "newest_official_label"}));
                Rev::Label(l)
            }
            None => {
                took = Some(json!({"rev": "working", "why": "no_official_label"}));
                Rev::Working
            }
        },
    };
    let rev = match (project, rev) {
        (Some(project), Rev::Label(label)) => Rev::Foreign { project, label },
        (Some(project), _) => {
            return Err(Error::new(
                Code::InstanceForeignWorking,
                format!(
                    "{part} is another project's ({project}): insert one of its official release labels (rev)"
                ),
            )
            .with("param", "rev")
            .with("project", &project)
            .with("part", &part));
        }
        (None, r) => r,
    };
    let placement = match (&given.placement, current) {
        (Some(arg), Some(c)) => {
            let mut p = c
                .placement
                .clone()
                .unwrap_or_else(|| Placement::identity(unit));
            apply_placement(&mut p, arg.clone(), unit, env)?;
            Some(p)
        }
        (Some(arg), None) => Some(new_placement(arg.clone(), unit, env)?),
        (None, Some(c)) => c.placement.clone(),
        (None, None) => None,
    };
    // Refuse a placement that could never build, before it is written.
    if let Some(p) = &placement {
        rebuild::placement(p)?;
    }
    let placement = placement.filter(|p| !p.is_identity());
    Ok((
        crate::doc::insert::Insert {
            part,
            rev,
            placement,
        },
        took,
    ))
}

/// Refuses insert `id` of part `pid`, listed in `list`, that can't build:
/// a part that isn't there (`not_found`), an assembly (`wrong_part_kind`,
/// question 625), a cycle through inserts and instances
/// (`insert_cycle`), a label that doesn't resolve now
/// (`label_not_found`, `label_not_official`), a source that shows no
/// solid (`insert_source_empty`).
pub(crate) fn check_insert(
    session: &mut Session,
    pid: &Id,
    list: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<()> {
    let ins = f.insert().expect("an insert");
    if let Rev::Foreign { .. } = &ins.rev {
        // The lock the write would need, from what was fetched, as a
        // foreign instance's.
        let mut project = session.project().clone();
        let q = proposed(project.part(pid)?, list, &[(id, f)]);
        project.parts.insert(pid.clone(), q);
        crate::deps::relock_keeping(session, &project).map_err(|e| e.with("feature", id))?;
        return Ok(());
    }
    let Some(src) = session.project().parts.get(&ins.part) else {
        return Err(Error::new(
            Code::NotFound,
            format!(
                "{id} inserts {}, which isn't a part of the project",
                ins.part
            ),
        )
        .with("feature", id)
        .with("part", &ins.part)
        .with("param", "of"));
    };
    if part_kind(src) == PartKind::Assembly {
        return Err(crate::insert::assembly_error(id, &ins.part));
    }
    match &ins.rev {
        Rev::Working => {
            let mut project = session.project().clone();
            let q = proposed(project.part(pid)?, list, &[(id, f)]);
            project.parts.insert(pid.clone(), q);
            let cycles = crate::assembly::Cycles::of(&project, &crate::assembly::Pins::new());
            let (a, c) = (
                (pid.clone(), Rev::Working),
                (ins.part.clone(), Rev::Working),
            );
            if cycles.closes(&a, &c) {
                return Err(crate::assembly::cycle_error(&cycles, &a, &c).with("feature", id));
            }
        }
        Rev::Label(l) => {
            crate::release::pinned(session, &ins.part, l).map_err(|e| e.with("feature", id))?;
        }
        Rev::Foreign { .. } => unreachable!("judged above"),
    }
    let model = session.model(&ins.part, &ins.rev)?;
    if model
        .occurrences
        .first()
        .is_none_or(|o| o.solids.is_empty())
    {
        return Err(Error::new(
            Code::InsertSourceEmpty,
            format!("{id} inserts {}, which shows no solid", ins.part),
        )
        .with("feature", id)
        .with("part", &ins.part));
    }
    Ok(())
}

/// Inserts: another part's solids, founded as a family in this part's
/// frame (ryucad 25).
pub(crate) struct Inserts;

impl FeatureKind for Inserts {
    fn types(&self) -> &'static [&'static str] {
        &["insert"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &INSERT
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::FOUNDS
    }

    // A new insert may look up its source's newest label: read it before
    // the part is borrowed for the rest.
    fn before_targets(&self, ty: &str, cx: &mut Write<'_>) -> Result<()> {
        cx.given.only(ty, &INSERT)?;
        let (ins, took) = insert_from(cx.session, None, &cx.given, cx.unit, cx.env)?;
        cx.read = Some(Feature::Insert(Box::new(ins)));
        cx.took = took;
        Ok(())
    }

    // So may an edit: read before the part's revision is checked.
    fn before_revision(&self, ty: &str, current: &Feature, cx: &mut Write<'_>) -> Result<()> {
        let i = current.insert().expect("an insert").clone();
        cx.given.only(ty, &INSERT)?;
        let (ins, _) = insert_from(cx.session, Some(&i), &cx.given, cx.unit, cx.env)?;
        cx.read = Some(Feature::Insert(Box::new(ins)));
        Ok(())
    }

    fn from_given(
        &self,
        _ty: &str,
        _current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        Ok(cx.read.take().expect("read before the part was borrowed"))
    }

    fn report(&self, _part: &Part, f: &Feature) -> Value {
        f.insert().expect("an insert").fields()
    }
}
