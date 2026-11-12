//! Materials and properties from STEP files (design 448, plan 24.C): what
//! an imported product's committed file says of its material, density
//! and user-defined attributes, as ryupdm's property values. An import
//! writes them only where ryupdm needs them written — `material` and the
//! custom properties `ryupdm.json` declares — and judges each three ways
//! on update; it compares the file's density with the library and never
//! changes the library. Every other attribute is a *file property*, read
//! from the committed bytes each time it is asked for (`props.get`).

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::materials::Materials;
use crate::doc::project::Project;
use crate::doc::source::Source;
use crate::import::plan::Planned;
use crate::units::Unit;
use ryupdm::config::{Config, Decl, Type};
use ryustep::Opened;
use ryustep::properties::Value as FileValue;
use ryustep::schema::MeasureUnit;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// An attribute's name as a property key: trimmed, lower case, spaces
/// and hyphens read as `_`.
pub fn fold(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| match c {
            ' ' | '-' => '_',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// The shortest decimal that reads back as `x`.
fn decimal(x: f64) -> String {
    format!("{x}")
}

/// The text a quantity property holds for `value` in `unit`: in the unit
/// where ryucad names it (`2.5 mm`, `0.5 in`, `15 deg`), else in SI
/// (`0.0025 m`, `1e-6 m^2`); `None` for a unit of mass or anything
/// besides length and plane angle.
pub fn quantity_text(value: f64, unit: &MeasureUnit) -> Option<String> {
    if unit.other || unit.mass != 0 {
        return None;
    }
    let si = match unit.decimal {
        Some(d) => ryustep::schema::scaled(value, d),
        None => value * unit.size,
    };
    Some(match (unit.length, unit.angle) {
        (1, 0) => match Unit::ALL.iter().find(|u| u.factor() == unit.size) {
            Some(u) => format!("{} {}", decimal(value), u.as_str()),
            None => format!("{} m", decimal(si)),
        },
        (0, 1) if unit.size == 1.0 => format!("{} rad", decimal(value)),
        (0, 1) if unit.size == std::f64::consts::PI / 180.0 => format!("{} deg", decimal(value)),
        (0, 1) => format!("{} rad", decimal(si)),
        (p, 0) if p != 0 => format!("{} m^{p}", decimal(si)),
        _ => return None,
    })
}

fn dimension_words(u: &MeasureUnit) -> &'static str {
    match (u.length, u.angle, u.mass, u.other) {
        (1, 0, 0, false) => "length",
        (0, 1, 0, false) => "angle",
        (0, 0, 1, false) => "mass",
        (-3, 0, 1, false) => "density",
        _ => "measure",
    }
}

/// `v` as declaration `decl`'s value, or why not: `("unit_dimension",
/// …)` for a measure of another dimension, `("type", …)` for a value
/// `ryupdm::props::fits` refuses. Text is read as the type wants first,
/// as a command line's is.
pub fn candidate(decl: &Decl, v: &FileValue, unit: Unit) -> Result<Value, (&'static str, String)> {
    let raw = match (&decl.ty, v) {
        (Type::Quantity(dim), FileValue::Measure { value, unit: u }) => {
            let same = !u.other
                && u.mass == 0
                && dim.mass == 0
                && (u.length, u.angle) == (dim.length, dim.angle);
            if !same {
                return Err((
                    "unit_dimension",
                    format!(
                        "a {} in {} isn't a {}",
                        dimension_words(u),
                        u.name,
                        dim.name()
                    ),
                ));
            }
            let text = quantity_text(*value, u).ok_or_else(|| {
                (
                    "unit_dimension",
                    format!("{} isn't a unit ryucad writes", u.name),
                )
            })?;
            json!(text)
        }
        // Mass and the rest fill only a number, and only in the SI base
        // unit: ryupdm's quantities are lengths and angles.
        (Type::Number, FileValue::Measure { value, unit: u })
            if (u.length, u.angle) != (1, 0) && (u.length, u.angle) != (0, 1) =>
        {
            if u.size != 1.0 || u.decimal != Some(0) {
                return Err((
                    "unit_dimension",
                    format!(
                        "a {} in {} fills a number only in its SI unit",
                        dimension_words(u),
                        u.name
                    ),
                ));
            }
            json!(value)
        }
        (_, FileValue::Measure { unit: u, .. }) => {
            return Err((
                "unit_dimension",
                format!(
                    "a {} in {} isn't a {}",
                    dimension_words(u),
                    u.name,
                    decl.ty.name()
                ),
            ));
        }
        (_, FileValue::Text(s)) => ryupdm::props::coerce(decl, json!(s)),
        (_, FileValue::Integer(n)) => json!(n),
        (_, FileValue::Number(x)) => serde_json::Number::from_f64(*x)
            .map(Value::Number)
            .ok_or(("type", "not a finite number".to_owned()))?,
        (_, FileValue::Boolean(b)) => json!(b),
    };
    match ryupdm::props::fits(decl, &raw, unit) {
        None => Ok(raw),
        Some(why) => Err(("type", why)),
    }
}

/// What definition `k` of a file maps to where the project uses PDM:
/// `values` (the product's name and description, `material`, and each
/// declared custom property an attribute fills), the file's density of
/// its material, `import_property_invalid` warnings, and each mapped
/// value's instances by key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mapped {
    pub values: BTreeMap<String, Value>,
    pub density: Option<f64>,
    pub invalid: Vec<String>,
    pub instances: BTreeMap<String, Vec<u64>>,
}

/// Definition `k` of `opened` (stored at `path`) mapped onto `config`'s
/// properties; `unit` is the project's default.
pub fn mapped(opened: &Opened, k: usize, config: &Config, unit: Unit, path: &str) -> Mapped {
    let mut m = Mapped {
        values: crate::import::plan::properties(opened, Some(k), true),
        ..Mapped::default()
    };
    let Some(p) = opened.properties().products.get(&k) else {
        return m;
    };
    if let Some(mat) = &p.material {
        match crate::doc::materials::check_name(&mat.name) {
            Ok(()) => {
                m.values.insert("material".into(), json!(mat.name));
                m.density = mat.density;
                m.instances.insert("material".into(), mat.instances.clone());
            }
            Err(e) => m.invalid.push(format!(
                "import_property_invalid: {path}: material {:?} (#{}) for material: type: {}",
                mat.name, mat.instance, e.message
            )),
        }
    }
    let mut by_key: BTreeMap<String, Vec<&ryustep::properties::Attribute>> = BTreeMap::new();
    for a in &p.attributes {
        by_key.entry(fold(&a.name)).or_default().push(a);
    }
    for (key, list) in by_key {
        let Some(decl) = config.properties.get(&key) else {
            continue;
        };
        if let [a] = list.as_slice() {
            match candidate(decl, &a.value, unit) {
                Ok(v) => {
                    m.values.insert(key.clone(), v);
                    m.instances.insert(key, a.instances.clone());
                }
                Err((reason, why)) => m.invalid.push(format!(
                    "import_property_invalid: {path}: attribute {:?} (#{}) for {key}: {reason}: {why}",
                    a.name, a.item
                )),
            }
        } else {
            let names: Vec<String> = list
                .iter()
                .map(|a| format!("{:?} (#{})", a.name, a.item))
                .collect();
            m.invalid.push(format!(
                "import_property_invalid: {path}: attributes {} for {key}: duplicate: they all read as {key}, so none is written",
                names.join(", ")
            ));
        }
    }
    m
}

/// A material name as a shell word: as it is when it is only letters,
/// digits and `-_.`, else single-quoted.
fn shell_word(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// What the library says of a file's material `name` with `density`
/// (kg/m³): `import_material_conflict` for a different density (relative
/// 1e-9), `import_material_unknown` with the command that would add it
/// for a name the library lacks; `None` when they agree or the file gives
/// no density. The library is never changed (#592).
pub fn library_warning(
    lib: &Materials,
    name: &str,
    density: Option<f64>,
    path: &str,
) -> Option<String> {
    let d = density?;
    match lib.materials.get(name) {
        Some(m) => {
            let have = m.density.si();
            ((d - have).abs() > 1e-9 * have.abs()).then(|| {
                format!(
                    "import_material_conflict: {path} gives {name} a density of {} kg/m^3, the library {} ({} kg/m^3): the library is unchanged",
                    decimal(d),
                    m.density.text(),
                    decimal(have)
                )
            })
        }
        None => Some(format!(
            "import_material_unknown: {path} names material {name}, which the library hasn't, with a density of {} kg/m^3: ryucad materials set {} --density \"{} kg/m^3\" adds it",
            decimal(d),
            shell_word(name),
            decimal(d)
        )),
    }
}

/// What an update does with one mapped property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judgement {
    /// Leave the part's value.
    Keep,
    /// Write the new file's value.
    Take,
    /// The file dropped a value the part holds unedited: it stays, said.
    Dropped,
    /// A person changed it and the file changed otherwise: the person's
    /// stays, said.
    Conflict,
}

/// The rule (design 448): `old` is the replaced file's mapped value, `now`
/// the part's, `new` the new file's.
pub fn judge(old: Option<&Value>, now: Option<&Value>, new: Option<&Value>) -> Judgement {
    if new == old {
        return Judgement::Keep;
    }
    if now == old {
        return if new.is_none() {
            Judgement::Dropped
        } else {
            Judgement::Take
        };
    }
    if now == new {
        Judgement::Keep
    } else {
        Judgement::Conflict
    }
}

fn shown(v: Option<&Value>) -> String {
    match v {
        None => "nothing".to_owned(),
        Some(Value::String(s)) => format!("{s:?}"),
        Some(v) => v.to_string(),
    }
}

/// What kind of import a plan is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A file imported whole, or flattened: new parts.
    Fresh,
    /// One product `--into` an existing part.
    Into,
    /// `import --update`: the old bytes are judged against.
    Update,
}

/// What [`apply`] did beyond the plan's properties: its warnings, and the
/// instances whose values the parts end up holding (the transfer's
/// `interpreted`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    pub warnings: Vec<String>,
    pub instances: Vec<u64>,
}

/// Fills `plan`'s properties from `opened` (stored at `path`) where the
/// project uses PDM: a new part takes every mapped value. `files` are the
/// project's files now.
pub fn apply(
    project: &Project,
    files: &dyn Source,
    opened: &Opened,
    old: Option<&Opened>,
    path: &str,
    plan: &mut [Planned],
    mode: Mode,
) -> Applied {
    let mut out = Applied::default();
    if !project.root.join(crate::doc::project::PDM_CONFIG).is_file() {
        return out;
    }
    // A settings file that doesn't read is ryupdm's fault to report.
    let Ok(config) = Config::read(files) else {
        return out;
    };
    let unit = project.manifest.default_unit;
    let lib = crate::doc::materials::read(files)
        .ok()
        .flatten()
        .unwrap_or_default();
    for pl in plan.iter_mut() {
        let Some(k) = pl.props_of else {
            continue;
        };
        let new = mapped(opened, k, &config, unit, path);
        out.warnings.extend(new.invalid.iter().cloned());
        if let Some(name) = new.values.get("material").and_then(Value::as_str) {
            out.warnings
                .extend(library_warning(&lib, name, new.density, path));
        }
        if pl.new {
            pl.properties = new.values.clone();
            for v in new.instances.values() {
                out.instances.extend(v);
            }
            continue;
        }
        let current: BTreeMap<String, Value> = ryupdm::props::read(files, &pl.part)
            .map(|p| p.values)
            .unwrap_or_default();
        let mut merged = current.clone();
        if mode == Mode::Into {
            for (key, v) in &new.values {
                if !current.contains_key(key) {
                    merged.insert(key.clone(), v.clone());
                }
            }
        }
        if mode == Mode::Update {
            let before = old
                .and_then(|o| {
                    let j = source_definition(project, &pl.part, path, o)?;
                    Some(mapped(o, j, &config, unit, path).values)
                })
                .unwrap_or_default();
            let keys: BTreeSet<&String> = before.keys().chain(new.values.keys()).collect();
            for key in keys {
                let (o, n) = (before.get(key), new.values.get(key));
                match judge(o, current.get(key), n) {
                    Judgement::Take => {
                        merged.insert(key.clone(), n.expect("taken values exist").clone());
                    }
                    Judgement::Keep => {}
                    Judgement::Dropped => out.warnings.push(format!(
                        "import_property_dropped: {}/{key}: {path} no longer gives it; the part keeps {}",
                        pl.part,
                        shown(current.get(key))
                    )),
                    Judgement::Conflict => out.warnings.push(format!(
                        "import_property_conflict: {}/{key}: the file had {}, now has {}, and the part holds {}, set since: the part's stays",
                        pl.part,
                        shown(o),
                        shown(n),
                        shown(current.get(key))
                    )),
                }
            }
        }
        for (key, v) in &new.values {
            if merged.get(key) == Some(v) {
                out.instances
                    .extend(new.instances.get(key).into_iter().flatten());
            }
        }
        if merged != current {
            pl.properties = merged;
        }
    }
    out.instances.sort_unstable();
    out.instances.dedup();
    out
}

/// The property items of `opened` that don't read, as the transfer's
/// `failed` (`import_property_invalid`), PDM or not.
pub fn failed(opened: &Opened) -> Vec<ryustep::census::Failed> {
    opened
        .properties()
        .failed
        .iter()
        .map(|(instance, _)| ryustep::census::Failed {
            instance: *instance,
            code: "import_property_invalid".to_owned(),
        })
        .collect()
}

/// The definition of `opened` (stored at `path`) part `part`'s
/// properties come from: the product of its first import feature of
/// `path` without a place, else its first instance imported from
/// `path` (an imported assembly's own product), else, for a flattened
/// part, the file's one root.
pub fn source_definition(
    project: &Project,
    part: &Id,
    path: &str,
    opened: &Opened,
) -> Option<usize> {
    let p = project.parts.get(part)?;
    let named = p.order().iter().find_map(|fid| match p.features.get(fid) {
        Some(Ok(Feature::Import(i))) if i.file.path == path && i.place.is_none() => {
            i.product.as_ref()
        }
        Some(Ok(Feature::Instance(i))) => i
            .import
            .as_ref()
            .filter(|u| u.file == path)
            .map(|u| &u.product),
        _ => None,
    });
    if let Some(r) = named {
        return crate::file_styles::definition_of(opened, r);
    }
    let flat = p.order().iter().any(|fid| {
        matches!(p.features.get(fid), Some(Ok(Feature::Import(i))) if i.file.path == path && i.place.is_some())
    });
    let makes = crate::import::plan::makes(opened);
    let roots: Vec<usize> = opened
        .products
        .roots
        .iter()
        .copied()
        .filter(|&k| makes[k])
        .collect();
    match (flat, roots.as_slice()) {
        (true, [k]) => Some(*k),
        _ => None,
    }
}

/// The instances of `opened` (at `path`) whose mapped values `parts`
/// hold now: `import.report`'s `interpreted`.
pub fn held(
    project: &Project,
    files: &dyn Source,
    opened: &Opened,
    path: &str,
    parts: &[Id],
) -> Vec<u64> {
    let mut out = Vec::new();
    if !project.root.join(crate::doc::project::PDM_CONFIG).is_file() {
        return out;
    }
    let Ok(config) = Config::read(files) else {
        return out;
    };
    let unit = project.manifest.default_unit;
    for part in parts {
        let Some(k) = source_definition(project, part, path, opened) else {
            continue;
        };
        let now = ryupdm::props::read(files, part)
            .map(|p| p.values)
            .unwrap_or_default();
        let m = mapped(opened, k, &config, unit, path);
        for (key, v) in &m.values {
            if now.get(key) == Some(v) {
                out.extend(m.instances.get(key).into_iter().flatten());
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ryupdm::config::{Config, Decl, Type};
    use ryustep::properties::Value as FileValue;
    use ryustep::schema::MeasureUnit;

    /// ryustep's vendor fixtures (`vendor.stp`, or its next version).
    pub(crate) fn vendor_path(v2: bool) -> String {
        format!(
            "{}/../ryustep/tests/data/properties/{}",
            env!("CARGO_MANIFEST_DIR"),
            if v2 { "vendor_v2.stp" } else { "vendor.stp" }
        )
    }

    fn decl(ty: Type) -> Decl {
        Decl {
            ty,
            required: false,
            revising: true,
        }
    }

    fn unit(name: &str, length: i8, mass: i8, size: f64, decimal: Option<i32>) -> MeasureUnit {
        MeasureUnit {
            length,
            angle: 0,
            mass,
            other: false,
            size,
            decimal,
            name: name.into(),
            instances: Vec::new(),
        }
    }

    #[test]
    fn names_fold_to_keys() {
        assert_eq!(fold("Wall-Thickness"), "wall_thickness");
        assert_eq!(fold(" Batch Size "), "batch_size");
        assert_eq!(fold("RoHS"), "rohs");
    }

    #[test]
    fn values_fit_their_declarations() {
        let mm = unit("mm", 1, 0, 0.001, Some(-3));
        let inch = unit("inch", 1, 0, 0.0254, None);
        let kg = unit("kg", 0, 1, 1.0, Some(0));
        let g = unit("g", 0, 1, 0.001, Some(-3));
        let length = decl(Type::Quantity(crate::units::Dim::LENGTH));
        let fit = |d: &Decl, v: FileValue| candidate(d, &v, crate::units::Unit::Mm);
        assert_eq!(
            fit(
                &length,
                FileValue::Measure {
                    value: 2.5,
                    unit: mm.clone()
                }
            ),
            Ok(json!("2.5 mm"))
        );
        assert_eq!(
            fit(
                &length,
                FileValue::Measure {
                    value: 0.5,
                    unit: inch
                }
            ),
            Ok(json!("0.5 in"))
        );
        assert_eq!(
            fit(
                &decl(Type::Number),
                FileValue::Measure {
                    value: 0.12,
                    unit: kg.clone()
                }
            ),
            Ok(json!(0.12))
        );
        assert_eq!(
            fit(
                &decl(Type::Number),
                FileValue::Measure {
                    value: 120.0,
                    unit: g
                }
            )
            .unwrap_err()
            .0,
            "unit_dimension"
        );
        assert_eq!(
            fit(
                &length,
                FileValue::Measure {
                    value: 1.0,
                    unit: kg
                }
            )
            .unwrap_err()
            .0,
            "unit_dimension"
        );
        assert_eq!(
            fit(&decl(Type::Integer), FileValue::Text("4".into())),
            Ok(json!(4))
        );
        assert_eq!(
            fit(&decl(Type::Integer), FileValue::Number(4.5))
                .unwrap_err()
                .0,
            "type"
        );
        assert_eq!(
            fit(&decl(Type::Number), FileValue::Integer(4)),
            Ok(json!(4))
        );
        assert_eq!(
            fit(&decl(Type::Boolean), FileValue::Text("true".into())),
            Ok(json!(true))
        );
        assert_eq!(
            fit(&decl(Type::String), FileValue::Text("ACME".into())),
            Ok(json!("ACME"))
        );
        assert_eq!(
            fit(&decl(Type::String), FileValue::Text("a\nb".into()))
                .unwrap_err()
                .0,
            "type"
        );
        let finish = decl(Type::Enum(vec!["bare".into(), "red".into()]));
        assert_eq!(
            fit(&finish, FileValue::Text("red".into())),
            Ok(json!("red"))
        );
        assert_eq!(
            fit(&finish, FileValue::Text("blue".into())).unwrap_err().0,
            "type"
        );
        let _ = mm;
    }

    #[test]
    fn the_vendors_file_maps_what_is_declared() {
        let bytes = std::fs::read(vendor_path(false)).unwrap();
        let o = ryustep::open(&bytes).unwrap();
        let mut c = Config::default();
        for (k, ty) in [
            ("supplier", Type::String),
            ("batch_size", Type::Integer),
            ("wall_thickness", Type::Quantity(crate::units::Dim::LENGTH)),
            ("net_mass", Type::Number),
            ("rohs", Type::Integer),
        ] {
            c.properties.insert(k.into(), decl(ty));
        }
        let m = mapped(&o, 0, &c, crate::units::Unit::Mm, "imports/vendor.step");
        assert_eq!(
            m.values,
            BTreeMap::from([
                ("batch_size".to_owned(), json!(250)),
                ("material".to_owned(), json!("6061-T6")),
                ("name".to_owned(), json!("vendor bracket")),
                ("net_mass".to_owned(), json!(0.12)),
                ("supplier".to_owned(), json!("ACME Metals")),
                ("wall_thickness".to_owned(), json!("2.5 mm")),
            ])
        );
        assert_eq!(m.density, Some(2700.0));
        // RoHS is a boolean; an integer was declared.
        assert_eq!(m.invalid.len(), 1, "{:?}", m.invalid);
        assert!(
            m.invalid[0]
                .starts_with("import_property_invalid: imports/vendor.step: attribute \"RoHS\""),
            "{}",
            m.invalid[0]
        );
        assert!(
            m.invalid[0].contains(" for rohs: type: "),
            "{}",
            m.invalid[0]
        );
        assert!(!m.instances["supplier"].is_empty());
        assert!(
            m.instances.get("name").is_none(),
            "the product's name is the tree's"
        );
    }

    use crate::server::tests::{call, fresh};
    use crate::session::Session;

    /// The vendor's attributes as a project would declare them.
    const DECLARED: [(&str, &str); 7] = [
        ("supplier", r#"{"type": "string"}"#),
        ("batch_size", r#"{"type": "integer"}"#),
        ("yield", r#"{"type": "number"}"#),
        ("rohs", r#"{"type": "boolean"}"#),
        ("wall_thickness", r#"{"type": "quantity", "dim": "length"}"#),
        ("net_mass", r#"{"type": "number"}"#),
        ("revision_count", r#"{"type": "integer"}"#),
    ];

    /// A fresh project using PDM, the vendor's attributes declared but
    /// `undeclared`.
    pub(crate) fn declared(undeclared: &[&str]) -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        let props: serde_json::Map<String, Value> = DECLARED
            .iter()
            .filter(|(k, _)| !undeclared.contains(k))
            .map(|(k, d)| ((*k).to_owned(), serde_json::from_str(d).unwrap()))
            .collect();
        call(&mut s, "pdm.configure", json!({"properties": props})).unwrap();
        (dir, s)
    }

    /// Part `part`'s properties as `props.get` gives their values.
    pub(crate) fn values(s: &mut Session, part: &str) -> BTreeMap<String, Value> {
        let v = call(s, "props.get", json!({"part": part})).unwrap();
        v["properties"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| Some((p["name"].as_str()?.to_owned(), p.get("value")?.clone())))
            .collect()
    }

    fn warnings(v: &Value) -> Vec<String> {
        v["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| w.as_str().map(str::to_owned))
            .collect()
    }

    #[test]
    fn a_fresh_import_maps_the_vendors_material_and_declared_properties() {
        let (_dir, mut s) = declared(&[]);
        let v = call(
            &mut s,
            "import",
            json!({"path": vendor_path(false), "solid": true}),
        )
        .unwrap();
        let part = v["parts"][0]["part"].as_str().unwrap().to_owned();
        let got = values(&mut s, &part);
        for (k, want) in [
            ("name", json!("vendor bracket")),
            ("material", json!("6061-T6")),
            ("supplier", json!("ACME Metals")),
            ("batch_size", json!(250)),
            ("yield", json!(0.95)),
            ("rohs", json!(true)),
            ("wall_thickness", json!("2.5 mm")),
            ("net_mass", json!(0.12)),
            ("revision_count", json!(3)),
        ] {
            assert_eq!(got.get(k), Some(&want), "{k}: {got:?}");
        }
        assert!(got.get("number").is_none(), "never the number");
        // The dry run says the same and writes nothing.
        let (_dir, mut s) = declared(&[]);
        let dry = call(
            &mut s,
            "import",
            json!({"path": vendor_path(false), "solid": true, "dry_run": true}),
        )
        .unwrap();
        assert_eq!(dry["parts"][0]["properties"]["supplier"], "ACME Metals");
        assert!(!s.root().join("parts").join(&part).exists());
        let _ = warnings(&dry);
    }

    #[test]
    fn without_pdm_nothing_is_mapped() {
        let (_dir, mut s) = fresh();
        let v = call(
            &mut s,
            "import",
            json!({"path": vendor_path(false), "solid": true}),
        )
        .unwrap();
        assert!(v["parts"][0].get("properties").is_none(), "{v}");
    }

    #[test]
    fn the_library_is_compared_and_never_changed() {
        let unknown = |s: &mut Session| {
            let v = call(
                s,
                "import",
                json!({"path": vendor_path(false), "solid": true}),
            )
            .unwrap();
            warnings(&v)
        };
        // Not in the library: the command that would add it.
        let (_dir, mut s) = declared(&[]);
        let w = unknown(&mut s);
        let u: Vec<&String> = w
            .iter()
            .filter(|w| w.starts_with("import_material_unknown:"))
            .collect();
        assert_eq!(u.len(), 1, "{w:?}");
        assert!(
            u[0].contains("ryucad materials set 6061-T6 --density \"2700 kg/m^3\""),
            "{}",
            u[0]
        );
        let lib = call(&mut s, "materials.list", json!({})).unwrap();
        assert_eq!(lib["materials"], json!([]), "the library is unchanged");
        // The same density: nothing to say.
        let (_dir, mut s) = declared(&[]);
        call(
            &mut s,
            "materials.set",
            json!({"name": "6061-T6", "density": "2.7 g/cm^3"}),
        )
        .unwrap();
        assert!(
            unknown(&mut s)
                .iter()
                .all(|w| !w.starts_with("import_material")),
            "2.7 g/cm³ is 2700 kg/m³"
        );
        // Another density: both named, the library as it was.
        let (_dir, mut s) = declared(&[]);
        call(
            &mut s,
            "materials.set",
            json!({"name": "6061-T6", "density": "2650 kg/m^3"}),
        )
        .unwrap();
        let w = unknown(&mut s);
        let c: Vec<&String> = w
            .iter()
            .filter(|w| w.starts_with("import_material_conflict:"))
            .collect();
        assert_eq!(c.len(), 1, "{w:?}");
        assert!(
            c[0].contains("2700 kg/m^3") && c[0].contains("2650 kg/m^3"),
            "{}",
            c[0]
        );
        let lib = call(&mut s, "materials.list", json!({})).unwrap();
        assert_eq!(lib["materials"][0]["density"]["value"], "2650 kg/m^3");
        assert_eq!(
            values(&mut s, "vb-100")["material"],
            "6061-T6",
            "set either way"
        );
    }

    #[test]
    fn a_name_without_a_density_says_nothing() {
        let lib = Materials::default();
        assert_eq!(library_warning(&lib, "Steel", None, "imports/x.step"), None);
    }

    #[test]
    fn into_a_part_fills_only_what_it_lacks() {
        let (_dir, mut s) = declared(&[]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"supplier": "Mine", "name": "mine"}}),
        )
        .unwrap();
        call(
            &mut s,
            "import",
            json!({"path": vendor_path(false), "into": "p", "solid": true}),
        )
        .unwrap();
        let got = values(&mut s, "p");
        assert_eq!(got["supplier"], "Mine");
        assert_eq!(got["name"], "mine");
        assert_eq!(got["batch_size"], 250);
        assert_eq!(got["material"], "6061-T6");
    }

    fn bucket(v: &Value, category: &str, bucket: &str) -> u64 {
        v["categories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["category"] == category)
            .map_or(0, |c| c[bucket]["count"].as_u64().unwrap())
    }

    #[test]
    fn mapped_values_are_interpreted_and_what_doesnt_read_failed() {
        let (_dir, mut s) = declared(&[]);
        let v = call(
            &mut s,
            "import",
            json!({"path": vendor_path(false), "solid": true}),
        )
        .unwrap();
        let t = &v["transfer"];
        assert!(bucket(t, "materials", "interpreted") >= 5, "{t}");
        let props = bucket(t, "properties", "interpreted");
        assert!(props >= 9, "{t}");
        assert_eq!(bucket(t, "properties", "failed"), 1, "Broken: {t}");
        let codes = t["categories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["category"] == "properties")
            .unwrap()["codes"]
            .clone();
        assert_eq!(
            codes,
            json!([{"code": "import_property_invalid", "count": 1}])
        );
        // import.report says the same of what the parts hold now, and
        // less once a person changes a value.
        let stored = v["file"]["path"].as_str().unwrap().to_owned();
        let r = call(&mut s, "import.report", json!({"file": stored})).unwrap();
        assert_eq!(bucket(&r["transfer"], "properties", "interpreted"), props);
        call(
            &mut s,
            "props.set",
            json!({"part": "vb-100", "properties": {"supplier": "Other"}}),
        )
        .unwrap();
        let r = call(&mut s, "import.report", json!({"file": stored})).unwrap();
        assert!(bucket(&r["transfer"], "properties", "interpreted") < props);
        // Without PDM: kept, not interpreted, and still failed.
        let (_dir, mut s) = fresh();
        let v = call(
            &mut s,
            "import",
            json!({"path": vendor_path(false), "solid": true}),
        )
        .unwrap();
        assert_eq!(bucket(&v["transfer"], "properties", "interpreted"), 0);
        assert_eq!(bucket(&v["transfer"], "properties", "failed"), 1);
    }

    #[test]
    fn the_three_way_rule() {
        let (a, b, c) = (json!("a"), json!("b"), json!("c"));
        use Judgement::*;
        assert_eq!(
            judge(Some(&a), Some(&a), Some(&a)),
            Keep,
            "the file didn't change"
        );
        assert_eq!(
            judge(Some(&a), Some(&b), Some(&a)),
            Keep,
            "a person's, the file unchanged"
        );
        assert_eq!(
            judge(Some(&a), Some(&a), Some(&b)),
            Take,
            "unedited: the new value"
        );
        assert_eq!(judge(None, None, Some(&b)), Take, "new in the file");
        assert_eq!(
            judge(Some(&a), Some(&a), None),
            Dropped,
            "gone from the file, kept"
        );
        assert_eq!(
            judge(Some(&a), Some(&c), Some(&b)),
            Conflict,
            "both changed"
        );
        assert_eq!(
            judge(Some(&a), Some(&b), Some(&b)),
            Keep,
            "both changed alike"
        );
        assert_eq!(
            judge(Some(&a), None, Some(&b)),
            Conflict,
            "cleared by a person"
        );
        assert_eq!(judge(Some(&a), None, None), Keep, "cleared, and gone");
    }

    /// The vendor's v1 imported, its stored path; then `before` run.
    fn updated(before: &dyn Fn(&mut Session)) -> (tempfile::TempDir, Session, Value) {
        let (dir, mut s) = declared(&[]);
        let v = call(
            &mut s,
            "import",
            json!({"path": vendor_path(false), "solid": true}),
        )
        .unwrap();
        let stored = v["file"]["path"].as_str().unwrap().to_owned();
        before(&mut s);
        let u = call(
            &mut s,
            "import",
            json!({"path": vendor_path(true), "update": stored}),
        )
        .unwrap();
        (dir, s, u)
    }

    #[test]
    fn an_update_takes_the_files_changes_where_nobody_edited() {
        let (_dir, mut s, _) = updated(&|s| {
            call(
                s,
                "props.set",
                json!({"part": "vb-100", "properties": {"yield": 0.9, "wall_thickness": "4 mm"}}),
            )
            .unwrap();
        });
        let got = values(&mut s, "vb-100");
        assert_eq!(got["supplier"], "Globex", "unedited, changed in the file");
        assert_eq!(got["yield"], 0.9, "edited, unchanged in the file");
        assert_eq!(
            got["wall_thickness"], "4 mm",
            "edited and changed: the person's"
        );
        assert_eq!(got["batch_size"], 250, "gone from the file: kept");
        assert_eq!(got["material"], "6061-T6");
    }

    #[test]
    fn an_update_says_what_it_kept_and_why() {
        let (_dir, _s, u) = updated(&|s| {
            call(
                s,
                "props.set",
                json!({"part": "vb-100", "properties": {"wall_thickness": "4 mm"}}),
            )
            .unwrap();
            call(
                s,
                "materials.set",
                json!({"name": "6061-T6", "density": "2700 kg/m^3"}),
            )
            .unwrap();
        });
        let w = warnings(&u);
        let one = |code: &str| -> String {
            let found: Vec<&String> = w.iter().filter(|x| x.starts_with(code)).collect();
            assert_eq!(found.len(), 1, "{code}: {w:?}");
            found[0].clone()
        };
        let conflict = one("import_property_conflict:");
        assert!(conflict.contains("vb-100/wall_thickness"), "{conflict}");
        for v in ["\"2.5 mm\"", "\"3 mm\"", "\"4 mm\""] {
            assert!(conflict.contains(v), "{v}: {conflict}");
        }
        let dropped = one("import_property_dropped:");
        assert!(
            dropped.contains("vb-100/batch_size") && dropped.contains("250"),
            "{dropped}"
        );
        let lib = one("import_material_conflict:");
        assert!(lib.contains("2710 kg/m^3"), "{lib}");
    }
}
