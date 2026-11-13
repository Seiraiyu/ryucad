//! Instances: a part placed in an assembly (ryucad 27.C).

use super::Given;
use super::{BodyUse, FeatureKind, Write};
use crate::doc::assembly::{Instance, Pose, Rev};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::part::proposed;
use crate::session::Session;
use crate::units::{Quantity, Unit};
use serde_json::Value;

pub(crate) const INSTANCE: [&str; 4] = ["of", "rev", "placement", "fixed"];

/// An instance: `current`'s fields (none for a new one), with what was
/// given laid over them. A new instance needs `of`: a part, or another
/// project's as `<dependency>:<part>` (a dependency of `deps` by name or
/// project ID), which needs a release label. Its revision is `working`
/// and its pose the assembly's frame unless given. The origin takes
/// quantities and expressions, held as meters.
pub(crate) fn instance_from(
    current: Option<&Instance>,
    given: &Given,
    unit: Unit,
    env: &Env,
    deps: &crate::doc::deps::Dependencies,
) -> Result<Instance> {
    let f = &given.fields;
    // Another project's: by `of`, else as the current instance is.
    let mut project = current.and_then(|c| c.rev.project().cloned());
    let part = match (&f.of, current) {
        (Some(of), _) => match of.split_once(':') {
            Some((dep, part)) => {
                let (id, _) =
                    crate::doc::deps::find(deps, dep).map_err(|e| e.with("param", "of"))?;
                project = Some(id.clone());
                Id::parse(part, "part").map_err(|e| e.with("param", "of"))?
            }
            None => {
                project = None;
                Id::parse(of, "part").map_err(|e| e.with("param", "of"))?
            }
        },
        (None, Some(c)) => c.part.clone(),
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                "an instance names the part it places (of)",
            )
            .with("param", "of"));
        }
    };
    let rev = match f.rev.as_deref() {
        None => current.map_or(Rev::Working, |c| c.rev.clone()),
        Some("working") => Rev::Working,
        Some(l) => {
            Rev::Label(crate::doc::assembly::label_text(l).map_err(|e| e.with("param", "rev"))?)
        }
    };
    let rev = match (project, rev.label()) {
        (None, _) => match rev {
            Rev::Foreign { label, .. } => Rev::Label(label),
            r => r,
        },
        (Some(project), Some(label)) => Rev::Foreign {
            label: label.to_owned(),
            project,
        },
        (Some(project), None) => {
            return Err(Error::new(
                Code::InstanceForeignWorking,
                format!(
                    "{part} is another project's ({project}): place one of its official release labels (rev)"
                ),
            )
            .with("param", "rev")
            .with("project", &project)
            .with("part", &part));
        }
    };
    let mut pose = current.map_or(Pose::IDENTITY, |c| c.pose);
    if let Some(arg) = &given.placement {
        if let Some(origin) = &arg.origin {
            for (i, v) in origin.iter().enumerate() {
                let q = Quantity::from_json_in(v, unit, &|n| env.get(n))
                    .map_err(|e| e.with("pointer", format!("/placement/origin/{i}")))?;
                if q.dim() != crate::units::Dim::LENGTH {
                    return Err(Error::new(
                        Code::UnitMismatch,
                        format!("an instance's origin is a length, not {}", q.text()),
                    )
                    .with("pointer", format!("/placement/origin/{i}")));
                }
                pose.origin_m[i] = crate::doc::sketch::clean(q.meters());
            }
        }
        if let Some(z) = arg.z {
            pose.z = z.map(crate::doc::sketch::clean);
        }
        if let Some(x) = arg.x {
            pose.x = x.map(crate::doc::sketch::clean);
        }
    }
    // Refuse a pose that could never build, before it is written.
    crate::assembly::pose_rigid(&pose)?;
    // Which use of an imported product it is, while it places that part.
    let import = current
        .filter(|c| c.part == part)
        .and_then(|c| c.import.clone());
    Ok(Instance {
        part,
        rev,
        pose,
        fixed: f.fixed.unwrap_or(current.is_some_and(|c| c.fixed)),
        import,
    })
}

/// Refuses instance `id` of assembly `pid`, listed in `list`, if its part
/// doesn't exist (`not_found`), if placing it would make the assembly
/// place itself (`assembly_cycle`), or if its label doesn't resolve now
/// (`label_not_found`, `label_not_official`).
pub(crate) fn check_instance(
    session: &mut Session,
    pid: &Id,
    list: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<()> {
    let i = f.instance().expect("an instance");
    if let Rev::Foreign { .. } = &i.rev {
        // The lock the write would need, from what was fetched: never the
        // network. Only what the write adds is judged; what the lock holds
        // now stands.
        let mut project = session.project().clone();
        let q = proposed(project.part(pid)?, list, &[(id, f)]);
        project.parts.insert(pid.clone(), q);
        crate::deps::relock_keeping(session, &project).map_err(|e| e.with("feature", id))?;
        return Ok(());
    }
    if !session.project().parts.contains_key(&i.part) {
        return Err(
            Error::new(Code::NotFound, format!("no part {} to place", i.part))
                .with("part", &i.part)
                .with("param", "of"),
        );
    }
    match &i.rev {
        Rev::Working => {
            let mut project = session.project().clone();
            let q = proposed(project.part(pid)?, list, &[(id, f)]);
            project.parts.insert(pid.clone(), q);
            let cycles = crate::assembly::Cycles::of(&project, &crate::assembly::Pins::new());
            let (a, c) = ((pid.clone(), Rev::Working), (i.part.clone(), Rev::Working));
            if cycles.closes(&a, &c) {
                return Err(crate::assembly::cycle_error(&cycles, &a, &c).with("feature", id));
            }
        }
        Rev::Label(l) => {
            crate::release::pinned(session, &i.part, l).map_err(|e| e.with("feature", id))?;
        }
        Rev::Foreign { .. } => unreachable!("judged above"),
    }
    Ok(())
}

/// Instances: a part placed in an assembly.
pub(crate) struct Instances;

impl FeatureKind for Instances {
    fn types(&self) -> &'static [&'static str] {
        &["instance"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &INSTANCE
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::NONE
    }

    fn from_given(
        &self,
        _ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        let deps = cx.session.project().manifest.dependencies.clone();
        let current = current.and_then(Feature::instance);
        Ok(Feature::Instance(instance_from(
            current, &cx.given, cx.unit, cx.env, &deps,
        )?))
    }

    fn report(&self, _part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        f.instance().expect("an instance").fields()
    }

    /// Its part's model comes from outside the part: see
    /// [`crate::rebuild::rebuild_part_in`]; alone it places nothing.
    fn rebuild(&self, cx: &super::RebuildCx<'_>, f: &Feature) -> super::Outcome {
        let Feature::Instance(i) = f else {
            unreachable!("an instance")
        };
        let r = crate::assembly::place(cx.id, i, &crate::assembly::Children::new())
            .map(|p| crate::rebuild::Output::Instance(std::sync::Arc::new(p)));
        (r, None)
    }
}
