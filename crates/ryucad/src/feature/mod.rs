#![allow(dead_code)] // ryucad 27.C: until feature.types (Task 12) uses every item
//! Feature kinds (ryucad 27.C, design 541): what ryucad does with each
//! feature type a request names, one module per kind, registered in
//! `KINDS`. The file formats stay in `ryucad-doc`, which ryupdm reads.

pub(crate) mod blend;
pub(crate) mod boolean;
pub(crate) mod delete_body;
pub(crate) mod extrude;
pub(crate) mod hole;
pub(crate) mod import;
pub(crate) mod insert;
pub(crate) mod instance;
pub(crate) mod instance_pattern;
pub(crate) mod mate;
pub(crate) mod pattern;
pub(crate) mod planar;
pub(crate) mod primitive;
pub(crate) mod sketch;

use crate::doc::files::{Feature, Placement, PrimitiveType, number};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::sweep::RevolveAxis;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::kind::Want;
use crate::proto::{FeatureFields, PlacementArg};
use crate::rebuild;
use crate::session::Session;
use crate::solid::Solids;
use crate::units::{Quantity, Unit};
use serde_json::{Map, Value, json};

/// What ryucad does with one kind of feature: one or several `type`s
/// sharing a module (`fillet` and `chamfer`, the five primitives).
/// `feature.add` and `feature.edit` keep their shared steps and call the
/// kind where they matched on the type. Every method after `report` does
/// nothing unless its kind needs it.
pub(crate) trait FeatureKind: Sync {
    /// The `type` strings this kind owns, as `feature.add` takes them.
    fn types(&self) -> &'static [&'static str];

    /// The request fields `ty` takes (`given.only`'s list): `params` and
    /// `placement` besides `FeatureFields`' own.
    fn fields(&self, ty: &str) -> &'static [&'static str];

    /// The body `ty` needs and whether it founds one (`feature.types`).
    fn body(&self, ty: &str) -> BodyUse;

    /// The feature a request makes: a new one (`current` `None`) or an
    /// edit of `current`, from `cx.given`, which `refuse_others` passed.
    /// (Named for the modules' own `from_given`, which it calls.)
    #[allow(clippy::wrong_self_convention)]
    fn from_given(
        &self,
        ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature>;

    /// Its part of `report`: its file's fields, every quantity as written
    /// and in meters, every reference as `<part>/<feature>/<name>`.
    fn report(&self, part: &Part, f: &Feature) -> Value;

    /// What a new feature fills in, or reads, before the shared target
    /// default (`one_target`): a pattern's targets, a mirror's solid and
    /// operation, an insert's newest label.
    fn before_targets(&self, _ty: &str, _cx: &mut Write<'_>) -> Result<()> {
        Ok(())
    }

    /// Whether it acts on targets whatever its operation (a hole), so a
    /// new one naming none acts on the part's one solid.
    fn acts_without_operation(&self) -> bool {
        false
    }

    /// What a request sends in other fields, read as this kind's own: a
    /// tweak's `face`, a flange's extent length, a primitive's `radius`.
    fn take_given(&self, _ty: &str, _given: &mut Given) -> Result<()> {
        Ok(())
    }

    /// The target a new feature takes when it names none (a tweak, a
    /// shell, a flange), after the shared default.
    fn default_target(&self, _ty: &str, _cx: &mut Write<'_>) -> Result<()> {
        Ok(())
    }

    /// What an edit of `current` reads of the session before its part's
    /// revision is checked: an insert's newest label.
    fn before_revision(&self, _ty: &str, _current: &Feature, _cx: &mut Write<'_>) -> Result<()> {
        Ok(())
    }

    /// Refuses the fields `ty` doesn't take.
    fn refuse_others(&self, ty: &str, given: &Given) -> Result<()> {
        given.only(ty, self.fields(ty))
    }

    /// Whether an edit from `old` to `new` has its references checked
    /// again, besides a change of its references, geometry or source.
    fn rechecks(&self, _old: &Feature, _new: &Feature) -> bool {
        false
    }

    /// Whether `f` is checked against the rebuild even when it names no
    /// reference, geometry or body (a loft's sections, an unfold).
    fn always_checked(&self, _f: &Feature) -> bool {
        false
    }

    /// Checks of `f` against the current rebuild beyond the shared ones:
    /// what its references are; what the kernel makes of them only
    /// rebuild can tell.
    fn check(&self, _cx: &Check<'_>, _f: &Feature) -> Result<()> {
        Ok(())
    }
}

/// The body a feature type needs and whether it founds one, whatever its
/// fields say (`feature.types`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BodyUse {
    /// What every body it acts on must be; `None` when that depends on
    /// its fields (a join wants a solid, a cut either) or it acts on none.
    pub(crate) needs: Option<Want>,
    pub(crate) founds: Founds,
}

impl BodyUse {
    /// Needs nothing, founds nothing.
    pub(crate) const NONE: BodyUse = BodyUse {
        needs: None,
        founds: Founds::Never,
    };
    /// Always founds its own body.
    pub(crate) const FOUNDS: BodyUse = BodyUse {
        needs: None,
        founds: Founds::Always,
    };
    /// Founds a body when its operation is `new` (the default).
    pub(crate) const WHEN_NEW: BodyUse = BodyUse {
        needs: None,
        founds: Founds::WhenNew,
    };
    /// Acts on solids only.
    pub(crate) const SOLID: BodyUse = BodyUse {
        needs: Some(Want::Solid),
        founds: Founds::Never,
    };
}

/// Whether a feature type founds a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Founds {
    Always,
    /// When its operation is `new`, the default.
    WhenNew,
    /// A split, when it keeps both sides.
    WhenBothKept,
    Never,
}

impl Founds {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Founds::Always => "always",
            Founds::WhenNew => "when_new",
            Founds::WhenBothKept => "when_both_kept",
            Founds::Never => "never",
        }
    }
}

/// Where a request's feature goes.
#[derive(Clone, Copy)]
pub(crate) enum Place<'a> {
    /// A new feature, to go at `at` in `order`, with the ID it was given.
    New {
        order: &'a [Id],
        at: usize,
        id: Option<&'a str>,
    },
    /// An edit of feature `id`.
    Edit { id: &'a Id },
}

/// A `feature.add` or `feature.edit` as a kind reads it.
pub(crate) struct Write<'a> {
    pub(crate) session: &'a mut Session,
    pub(crate) part: &'a Id,
    pub(crate) unit: Unit,
    pub(crate) env: &'a Env,
    pub(crate) given: Given,
    pub(crate) place: Place<'a>,
    /// A feature read before the part was borrowed for the rest (an
    /// insert, which may look up its source's newest label).
    pub(crate) read: Option<Feature>,
    /// What a new insert took for its revision, and why (design 521).
    pub(crate) took: Option<Value>,
}

impl Write<'_> {
    /// The solids a new feature could act on: founded before it and not
    /// consumed. Only a new feature's defaults ask.
    pub(crate) fn solids_before(&self) -> Result<Vec<Id>> {
        let Place::New { order, at, .. } = self.place else {
            unreachable!("only a new feature's defaults ask what is before it")
        };
        Ok(solids_before(
            self.session.project().part(self.part)?,
            order,
            at,
        ))
    }

    /// For an edit, refuses `f` whose profile names what isn't there
    /// ([`check_sweep`]); a new feature's is checked once it has an ID.
    pub(crate) fn check_profile(&self, f: &Feature) -> Result<()> {
        let Place::Edit { id } = self.place else {
            return Ok(());
        };
        let p = self.session.project().part(self.part)?;
        check_sweep(p, &crate::part::order(p)?, id, f)
    }
}

/// A feature as the reference checks see it: in `part`, with the list
/// as the write leaves it (`order`), against the current rebuild.
pub(crate) struct Check<'a> {
    pub(crate) part: &'a Part,
    pub(crate) order: &'a [Id],
    pub(crate) id: &'a Id,
    pub(crate) results: &'a crate::rebuild::PartResult,
}

impl Check<'_> {
    /// The part as writing `f` would leave it.
    pub(crate) fn proposed(&self, f: &Feature) -> Part {
        crate::part::proposed(self.part, self.order, &[(self.id, f)])
    }
}

/// Every kind, in the order `feature.types` lists them: `feature.add`'s
/// first types (primitives to datums), then paths, the whole-body
/// operations, sheet metal, the tweaks and imports.
pub(crate) static KINDS: &[&dyn FeatureKind] = &[
    &primitive::Primitives,
    &sketch::Sketches,
    &extrude::Extrusions,
    &crate::along::Sweeps,
    &crate::loft::Lofts,
    &planar::Planars,
    &crate::surface::Surfaces,
    &boolean::Booleans,
    &blend::Blends,
    &hole::Holes,
    &pattern::Patterns,
    &instance::Instances,
    &mate::Mates,
    &instance_pattern::InstancePatterns,
    &insert::Inserts,
    &delete_body::Deletes,
    &crate::datum::Datums,
    &crate::path::Paths,
    &crate::body::WholeBodies,
    &crate::sheet_metal::SheetMetal,
    &crate::tweak::Tweaks,
    &import::Imports,
];

/// The kind that owns feature type `ty`, if any does.
pub(crate) fn kind(ty: &str) -> Option<&'static dyn FeatureKind> {
    KINDS.iter().copied().find(|k| k.types().contains(&ty))
}

/// The kind of a feature a file holds: every type has one (the
/// registry's tests).
pub(crate) fn kind_of(f: &Feature) -> &'static dyn FeatureKind {
    kind(f.type_name()).expect("every feature type has a kind")
}

/// `unknown_feature_type` for a type no kind owns: ryucad-doc's own
/// refusal, as `feature.add` has always given it.
pub(crate) fn unknown_type(ty: &str) -> Error {
    match PrimitiveType::parse(ty) {
        Err(e) => e,
        Ok(_) => unreachable!("the primitives have a kind"),
    }
}

/// A placement as output reports it: quantities as written and in meters.
pub(crate) fn placement_report(p: &Placement) -> Value {
    json!({
        "origin": p.origin.iter().map(Quantity::report).collect::<Vec<_>>(),
        "z": p.z.map(number),
        "x": p.x.map(number),
    })
}

/// Applies the parts of `arg` that are given to `p`, its expressions
/// reading `env`.
pub(crate) fn apply_placement(
    p: &mut Placement,
    arg: PlacementArg,
    unit: Unit,
    env: &Env,
) -> Result<()> {
    if let Some(origin) = arg.origin {
        for (i, v) in origin.iter().enumerate() {
            p.origin[i] = Quantity::from_json_in(v, unit, &|n| env.get(n))
                .map_err(|e| e.with("pointer", format!("/placement/origin/{i}")))?;
        }
    }
    if let Some(z) = arg.z {
        p.z = z;
    }
    if let Some(x) = arg.x {
        p.x = x;
    }
    // Refuse a placement that could never build, before it is written.
    rebuild::placement(p).map(|_| ())
}

/// What a `feature.add` or `feature.edit` gives besides the feature's
/// place: each part left out keeps its current or default value.
pub(crate) struct Given {
    pub(crate) params: Map<String, Value>,
    pub(crate) placement: Option<PlacementArg>,
    pub(crate) fields: FeatureFields,
}

impl Given {
    /// The names of the fields given.
    pub(crate) fn present(&self) -> Vec<String> {
        let mut out: Vec<String> = self.fields.given().keys().cloned().collect();
        if !self.params.is_empty() {
            out.push("params".into());
        }
        if self.placement.is_some() {
            out.push("placement".into());
        }
        out
    }

    /// Refuses the fields `ty` doesn't take.
    pub(crate) fn only(&self, ty: &str, allowed: &[&str]) -> Result<()> {
        match self
            .present()
            .into_iter()
            .find(|f| !allowed.contains(&f.as_str()))
        {
            None => Ok(()),
            Some(name) => Err(
                Error::new(Code::InvalidParam, format!("a {ty} has no {name}")).with("param", name),
            ),
        }
    }
}

/// Whether body `x` of part `pid`, as a feature at `at` of `list` would
/// see it, is a sheet in the current rebuild: the tip of its chain among
/// the features before `at`. A part no feature of which may make a
/// sheet is never rebuilt for this.
pub(crate) fn source_is_sheet(
    session: &mut Session,
    pid: &Id,
    list: &[Id],
    at: usize,
    x: &Id,
) -> bool {
    let Ok(p) = session.project().part(pid).cloned() else {
        return false;
    };
    let may = p
        .features
        .values()
        .any(|g| g.as_ref().is_ok_and(|g| g.may_make_sheet()));
    if !may {
        return false;
    }
    let solids = Solids::of_order(&p, &list[..at]);
    let Some(tip) = solids.tips().find(|(s, _)| *s == x).map(|(_, t)| t.clone()) else {
        return false;
    };
    let Ok(results) = session.rebuilt(pid) else {
        return false;
    };
    crate::rebuild::Results::result(results, &tip)
        .and_then(|r| r.as_ref().ok())
        .and_then(|o| o.solid(x))
        .is_some_and(|b| b.body.kind() == ryukei::topo::BodyKind::Sheet)
}

/// The solids a feature put at `at` in `order` could act on: founded
/// before it and not consumed.
pub(crate) fn solids_before(p: &Part, order: &[Id], at: usize) -> Vec<Id> {
    Solids::of_order(p, &order[..at])
        .tips()
        .map(|(s, _)| s.clone())
        .collect()
}

/// Refuses a sweep or a hole that names what isn't there: its sketch must
/// be a sketch listed before it in `order`, with the entities it names,
/// its axis a line and a hole's points points. Whether the loops make a
/// profile only rebuild can tell.
pub(crate) fn check_sweep(part: &Part, order: &[Id], id: &Id, f: &Feature) -> Result<()> {
    // A loft's sections: each sketch listed before it, with the entities
    // its loops name. How they match only its rebuild check tells.
    if let Feature::Loft(l) = f {
        for (k, s) in l.sections.iter().enumerate() {
            let Some(sketch) = s.sketch() else { continue };
            check_sketch(part, order, id, sketch)?;
            let Some(Ok(Feature::Sketch(sk))) = part.features.get(sketch) else {
                continue;
            };
            for (i, e) in s.loops.iter().enumerate() {
                if !sk.entities.contains_key(e) {
                    return Err(
                        Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                            .with("sketch", sketch)
                            .with("entity", e)
                            .with("pointer", format!("/sections/{k}/loops/{i}")),
                    );
                }
            }
        }
        return Ok(());
    }
    let sketch = f.profiles()[0];
    check_sketch(part, order, id, sketch)?;
    let Some(Ok(Feature::Sketch(sk))) = part.features.get(sketch) else {
        return Ok(());
    };
    // A hole's points must be points of its sketch; whether it has any
    // to drill when it lists none only rebuild can tell.
    if let Feature::Hole(h) = f {
        for (i, e) in h.points.iter().enumerate() {
            match sk.entities.get(e).map(|x| &x.geometry) {
                None => {
                    return Err(
                        Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                            .with("sketch", sketch)
                            .with("entity", e)
                            .with("pointer", format!("/points/{i}")),
                    );
                }
                Some(crate::doc::sketch::Geometry::Point { .. }) => {}
                Some(g) => {
                    return Err(Error::new(
                        Code::ReferenceWrongKind,
                        format!("a hole drills at points; {e} is a {}", g.type_name()),
                    )
                    .with("entity", e)
                    .with("pointer", format!("/points/{i}")));
                }
            }
        }
        return Ok(());
    }
    let (loops, axis) = match f {
        Feature::Extrude(e) => (&e.loops, None),
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Line(l) => (&r.loops, Some(l)),
            RevolveAxis::Ref(_) => (&r.loops, None),
        },
        Feature::Sweep(w) => (&w.profile.loops, None),
        Feature::Planar(x) => (&x.profile.loops, None),
        Feature::BaseFlange(b) => (&b.loops, None),
        _ => unreachable!("a sweep"),
    };
    let at = if f.sweep().is_some() || matches!(f, Feature::Planar(_)) {
        "/profile"
    } else {
        ""
    };
    for (field, e) in loops
        .iter()
        .map(|e| ("loops", e))
        .chain(axis.map(|a| ("axis", a)))
    {
        match sk.entities.get(e).map(|x| &x.geometry) {
            None => {
                return Err(
                    Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                        .with("sketch", sketch)
                        .with("entity", e)
                        .with("pointer", format!("{at}/{field}")),
                );
            }
            Some(g)
                if field == "axis" && !matches!(g, crate::doc::sketch::Geometry::Line { .. }) =>
            {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!("an axis is a line; {e} is a {}", g.type_name()),
                )
                .with("entity", e)
                .with("pointer", "/axis"));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Refuses a sketch that isn't a sketch listed before `id` in `order`.
pub(crate) fn check_sketch(part: &Part, order: &[Id], id: &Id, sketch: &Id) -> Result<()> {
    let at = |f: &Id| order.iter().position(|x| x == f);
    match (at(sketch), at(id)) {
        (None, _) => {
            return Err(Error::new(
                Code::NotFound,
                format!("part {} has no feature {sketch} to sweep", part.id),
            )
            .with("sketch", sketch));
        }
        (Some(s), Some(me)) if s >= me => {
            return Err(Error::new(
                Code::ReferenceForward,
                format!("{id} would sweep {sketch}, which comes after it"),
            )
            .with("feature", id)
            .with("sketch", sketch));
        }
        _ => {}
    }
    match part.features.get(sketch) {
        Some(Ok(f)) if f.sketch().is_none() => Err(Error::new(
            Code::ReferenceWrongKind,
            format!("{sketch} is a {}, not a sketch", f.type_name()),
        )
        .with("sketch", sketch)),
        _ => Ok(()),
    }
}

/// A new feature's placement from what was given. With z given and x left
/// out, x defaults to world X, or to world Y when z lies along X; either
/// way the file records it.
pub(crate) fn new_placement(arg: PlacementArg, unit: Unit, env: &Env) -> Result<Placement> {
    let mut placed = Placement::identity(unit);
    let along_x = arg.x.is_none()
        && arg.z.is_some_and(|z| {
            let mut p = Placement::identity(unit);
            p.z = z;
            rebuild::placement(&p).is_err_and(|e| e.details["kernel"] == "Degenerate")
        });
    if along_x {
        placed.x = [0.0, 1.0, 0.0];
    }
    apply_placement(&mut placed, arg, unit, env)?;
    Ok(placed)
}

/// Refuses an up-to target (`role` in feature `id`) that isn't a plane
/// (a planar face, a datum plane) or a point (a vertex, a datum point) now.
pub(crate) fn check_up_to(
    p: &Part,
    order: &[Id],
    id: &Id,
    role: &str,
    target: &crate::doc::geometry::GeomRef,
    results: &crate::rebuild::PartResult,
) -> Result<()> {
    use crate::geometry::{At, Want, resolve};
    let at = At {
        part: p,
        order,
        owner: id,
        results,
    };
    resolve(&at, role, target, &[Want::Point, Want::Plane]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::Want;
    use std::collections::BTreeSet;

    /// Every type ryucad-doc knows, from its own refusal of one it
    /// doesn't (`unknown_feature_type` lists them).
    fn doc_types() -> Vec<String> {
        let e = PrimitiveType::parse("no such type").unwrap_err();
        let (_, known) = e
            .message
            .split_once("known: ")
            .expect("the refusal lists the known types");
        known.split(", ").map(str::to_owned).collect()
    }

    /// Every field a request's `FeatureFields` takes, by its wire name,
    /// from serde's refusal of one it doesn't (`deny_unknown_fields`
    /// lists them all, renames included).
    fn wire_fields() -> Vec<String> {
        let e = serde_json::from_value::<FeatureFields>(json!({"no such field": 0}))
            .unwrap_err()
            .to_string();
        let (_, expected) = e
            .split_once("expected one of ")
            .expect("serde lists the fields");
        expected
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    fn owned() -> Vec<&'static str> {
        KINDS
            .iter()
            .flat_map(|k| k.types().iter().copied())
            .collect()
    }

    #[test]
    fn every_type_has_exactly_one_kind() {
        let mut owned = owned();
        let n = owned.len();
        owned.sort_unstable();
        owned.dedup();
        assert_eq!(owned.len(), n, "a type two kinds own");
        let mut known = doc_types();
        known.sort();
        assert_eq!(owned, known, "the registry against ryucad-doc's types");
    }

    #[test]
    fn every_type_a_kind_owns_parses_as_that_type() {
        for ty in owned() {
            let v = json!({"format": crate::doc::files::FEATURE_FORMAT,
                           "kind": "ryucad.feature", "type": ty});
            match Feature::from_json(&v, Unit::Mm, &Env::empty()) {
                Ok(f) => assert_eq!(f.type_name(), ty),
                Err(e) => assert_ne!(e.code, Code::UnknownFeatureType, "{ty}: {e}"),
            }
        }
    }

    #[test]
    fn every_field_a_kind_lists_is_a_request_field() {
        let wire = wire_fields();
        for k in KINDS {
            for ty in k.types() {
                for f in k.fields(ty) {
                    assert!(
                        wire.iter().any(|w| w == f) || ["params", "placement"].contains(f),
                        "{ty} lists {f}, which no request has"
                    );
                }
            }
        }
    }

    #[test]
    fn every_request_field_is_taken_by_some_kind() {
        let taken: BTreeSet<&str> = KINDS
            .iter()
            .flat_map(|k| {
                k.types()
                    .iter()
                    .flat_map(move |t| k.fields(t).iter().copied())
            })
            .collect();
        let unread: Vec<String> = wire_fields()
            .into_iter()
            .filter(|f| !taken.contains(f.as_str()))
            .collect();
        assert!(unread.is_empty(), "FeatureFields no kind takes: {unread:?}");
    }

    /// What `feature.types` says each type needs of a body and whether it
    /// founds one, whatever its fields say (design 388's table, read in
    /// `kind::bodies_read`, `Feature::combine` and `Feature::founds`).
    #[test]
    fn what_each_type_needs_and_founds() {
        use Founds::*;
        let (solid, sheet) = (Some(Want::Solid), Some(Want::Sheet));
        let rows = [
            ("box", None, WhenNew),
            ("cylinder", None, WhenNew),
            ("cone", None, WhenNew),
            ("sphere", None, WhenNew),
            ("torus", None, WhenNew),
            ("sketch", None, Never),
            ("extrude", None, WhenNew),
            ("revolve", None, WhenNew),
            ("sweep", None, WhenNew),
            ("loft", None, WhenNew),
            ("planar", None, Always),
            ("fill", None, Always),
            ("extract", None, Always),
            ("sew", sheet, Always),
            ("thicken", None, WhenNew),
            ("boolean", None, Never),
            ("fillet", None, Never),
            ("chamfer", None, Never),
            ("hole", None, Never),
            ("pattern", None, Never),
            ("instance", None, Never),
            ("mate", None, Never),
            ("instance_pattern", None, Never),
            ("insert", None, Always),
            ("delete_body", None, Never),
            ("plane", None, Never),
            ("axis", None, Never),
            ("point", None, Never),
            ("path", None, Never),
            ("helix", None, Never),
            ("shell", solid, Never),
            ("offset", None, Never),
            ("mirror", None, WhenNew),
            ("split", None, WhenBothKept),
            ("base_flange", None, Always),
            ("flange", solid, Never),
            ("unfold", solid, Never),
            ("refold", solid, Never),
            ("move_face", None, Never),
            ("replace_face", None, Never),
            ("draft", None, Never),
            ("delete_face", None, Never),
            ("delete_blend", None, Never),
            ("resize_blend", None, Never),
            ("import", None, Always),
        ];
        assert_eq!(rows.len(), doc_types().len(), "a row for every type");
        for (ty, needs, founds) in rows {
            let k = kind(ty).unwrap_or_else(|| panic!("no kind owns {ty}"));
            assert_eq!(k.body(ty), BodyUse { needs, founds }, "{ty}");
        }
    }

    #[test]
    fn the_registry_lists_add_s_first_types_first() {
        let first: Vec<&str> = owned().into_iter().take(28).collect();
        assert_eq!(
            first,
            [
                "box",
                "cylinder",
                "cone",
                "sphere",
                "torus",
                "sketch",
                "extrude",
                "revolve",
                "sweep",
                "loft",
                "planar",
                "fill",
                "extract",
                "sew",
                "thicken",
                "boolean",
                "fillet",
                "chamfer",
                "hole",
                "pattern",
                "instance",
                "mate",
                "instance_pattern",
                "insert",
                "delete_body",
                "plane",
                "axis",
                "point"
            ]
        );
    }
}
