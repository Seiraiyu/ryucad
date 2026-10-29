//! Appearance: the colours and opacities models are drawn in, from
//! `parts/<part>/appearance.json` ([`crate::doc::appearance`]) and the
//! material library, for `render`, the app and agents.
//!
//! Each face's colour and opacity are worked out one at a time, from the
//! most specific entry that sets them: an assembly's entries for the
//! occurrence's solid, then for its instance path from the longest to the
//! shortest (the outermost assembly first, then each subassembly's own),
//! then the part's entry for the face, its solid and the part itself. A
//! colour ends at the first entry with `colour` or `material` (the part's
//! material's colour); with none, the material's colour, else the
//! viewer's own default. An opacity with none is 1. An occurrence pinned
//! at a release label is drawn with its part's appearance as the working
//! files have it: appearance is never released.
//!
//! `appearance.get` gives every drawn solid's appearance, `appearance.set`
//! and `appearance.clear` write entries, one logged, undoable change of
//! one part each, guarded by its revision. An entry naming a solid, face
//! or occurrence the model no longer has is kept and reported
//! (`appearance_unresolved`), never moved to another.

use crate::assembly::path_text;
use crate::doc::appearance::{AppearanceFile, Look};
use crate::doc::assembly::Rev;
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::error::Result;
use crate::rebuild::Built;
use crate::server::part_kind;
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A face's appearance as drawn: its colour (`#rrggbb`; `None` for the
/// viewer's default) and its opacity.
#[derive(Clone, Debug, PartialEq)]
pub struct Effective {
    pub colour: Option<String>,
    pub opacity: f64,
}

impl Default for Effective {
    fn default() -> Self {
        Effective {
            colour: None,
            opacity: 1.0,
        }
    }
}

impl Effective {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"opacity": crate::doc::files::number(self.opacity)});
        if let Some(c) = &self.colour {
            v["colour"] = json!(c);
        }
        v
    }

    /// Its colour as `render` draws it, `default` when it has none.
    pub fn rgb(&self, default: crate::render::Rgb) -> crate::render::Rgb {
        self.colour.as_deref().and_then(rgb).unwrap_or(default)
    }
}

/// `#rrggbb` as `render`'s colours: sRGB in `[0, 1]`.
pub fn rgb(c: &str) -> Option<crate::render::Rgb> {
    let c = c.strip_prefix('#').filter(|c| c.len() == 6)?;
    let byte = |k: usize| u8::from_str_radix(&c[k..k + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?].map(|b| f32::from(b) / 255.0))
}

/// What entries, most specific first, make of a face whose part's
/// material's colour is `material`.
pub fn effective(chain: &[&Look], material: Option<&str>) -> Effective {
    let colour = chain
        .iter()
        .find(|l| l.colour.is_some() || l.material)
        .map_or(material.map(str::to_owned), |l| {
            l.colour.clone().or_else(|| material.map(str::to_owned))
        });
    let opacity = chain.iter().find_map(|l| l.opacity).unwrap_or(1.0);
    Effective { colour, opacity }
}

/// A drawn solid's appearance: its faces', all `base` but those in
/// `faces`, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SolidLook {
    pub base: Effective,
    pub faces: BTreeMap<String, Effective>,
}

impl SolidLook {
    /// The appearance of the face named `name`.
    pub fn face(&self, name: &str) -> &Effective {
        self.faces.get(name).unwrap_or(&self.base)
    }

    /// Whether every face is drawn opaque in the viewer's own colour.
    pub fn is_plain(&self) -> bool {
        self.base == Effective::default() && self.faces.is_empty()
    }
}

/// A part's appearance file, when it has one that reads.
fn file<'a>(session: &'a Session, part: &Id) -> Option<&'a AppearanceFile> {
    session
        .project()
        .parts
        .get(part)?
        .appearance
        .as_ref()?
        .as_ref()
        .ok()
}

/// The colour of `part`'s material, if it has one with a colour.
fn material_colour(
    session: &Session,
    library: &crate::doc::materials::Materials,
    part: &Id,
) -> Option<String> {
    let named = crate::materials::material_of(session, part);
    library.of(named.as_deref())?.1.colour.clone()
}

/// One solid as a view draws it: its key (a part's founder, or an
/// assembly's `<path>/<founder>`), the part it is a solid of, its founder,
/// its body, and the assemblies' entries over it, most specific first.
#[derive(Clone, Debug)]
pub struct Drawn {
    pub key: String,
    pub part: Id,
    pub founder: Id,
    pub built: Arc<Built>,
    pub above: Vec<Look>,
}

/// The part an instance step of `assembly` places: its own instance's,
/// or for a pattern's copy (`<pattern>:<i>`), its seed's, the next step.
/// Returns the part and how many steps it took.
fn placed(session: &Session, assembly: &Id, steps: &[Id]) -> Option<(Id, usize)> {
    let p = session.project().parts.get(assembly)?;
    let (instance, used) = match steps {
        [copy, seed, ..] if copy.copy_of().is_some() => (seed, 2),
        [one, ..] => (one, 1),
        [] => return None,
    };
    match p.features.get(instance)? {
        Ok(Feature::Instance(i)) => Some((i.part.clone(), used)),
        _ => None,
    }
}

/// The assemblies an occurrence at `path` in `root` lies in, from the
/// root down, each with where its own part of the path starts.
fn assemblies(session: &Session, root: &Id, path: &[Id]) -> Vec<(Id, usize)> {
    let mut out = vec![(root.clone(), 0)];
    loop {
        let (assembly, start) = out.last().expect("the root at least").clone();
        let Some((part, used)) = placed(session, &assembly, &path[start..]) else {
            break;
        };
        let deeper = start + used < path.len()
            && session
                .project()
                .parts
                .get(&part)
                .is_some_and(|p| part_kind(p) == PartKind::Assembly);
        if !deeper {
            break;
        }
        out.push((part, start + used));
    }
    out
}

/// The assemblies' entries over a solid `founder` of the occurrence at
/// `path` in `root`, most specific first: each assembly from the root
/// down, its entry for the solid, then for the path from the longest to
/// the shortest.
pub fn above(session: &Session, root: &Id, path: &[Id], founder: &Id) -> Vec<Look> {
    let mut out = Vec::new();
    for (assembly, start) in assemblies(session, root, path) {
        let Some(f) = file(session, &assembly) else {
            continue;
        };
        let rest = &path[start..];
        let key = format!("{}/{founder}", path_text(rest));
        out.extend(f.occurrences.get(&key).cloned());
        for k in (1..=rest.len()).rev() {
            out.extend(f.occurrences.get(&path_text(&rest[..k])).cloned());
        }
    }
    out
}

/// Every solid `part` draws: a part's solids as shown, an assembly's
/// occurrences' solids.
pub fn drawn(session: &mut Session, part: &Id) -> Result<Vec<Drawn>> {
    let p = session.project().part(part)?;
    if part_kind(p) == PartKind::Assembly {
        let model = session.model(part, &Rev::Working)?;
        let mut out = Vec::new();
        for o in &model.occurrences {
            let path = path_text(&o.path);
            for s in &o.solids {
                out.push(Drawn {
                    key: format!("{path}/{}", s.founder),
                    part: o.part.clone(),
                    founder: s.founder.clone(),
                    built: Arc::clone(&s.built),
                    above: above(session, part, &o.path, &s.founder),
                });
            }
        }
        return Ok(out);
    }
    let solids = crate::solid::Solids::of(p);
    let built = session.rebuilt(part)?;
    Ok(built
        .shown(&solids)
        .into_iter()
        .filter_map(|s| {
            let (_, b) = s.state?;
            Some(Drawn {
                key: s.solid.to_string(),
                part: part.clone(),
                founder: s.solid.clone(),
                built: Arc::clone(b),
                above: Vec::new(),
            })
        })
        .collect())
}

/// How `d` is drawn, the part's material's colour `material`.
pub fn look_of(session: &Session, d: &Drawn, material: Option<&str>) -> SolidLook {
    let f = file(session, &d.part);
    let mut chain: Vec<&Look> = d.above.iter().collect();
    let solid = f.and_then(|f| f.solids.get(d.founder.as_str()));
    let part = f.and_then(|f| f.part.as_ref());
    let base = {
        let mut c = chain.clone();
        c.extend(solid);
        c.extend(part);
        effective(&c, material)
    };
    let mut faces = BTreeMap::new();
    let prefix = format!("{}/", d.founder);
    for (k, l) in f.into_iter().flat_map(|f| &f.faces) {
        let Some(name) = k.strip_prefix(&prefix) else {
            continue;
        };
        let alive = crate::doc::reference::parse_name(name)
            .is_some_and(|n| d.built.body.by_name(n).is_some());
        if !alive {
            continue;
        }
        chain.truncate(d.above.len());
        chain.push(l);
        chain.extend(solid);
        chain.extend(part);
        let e = effective(&chain, material);
        if e != base {
            faces.insert(name.to_owned(), e);
        }
    }
    SolidLook { base, faces }
}

/// How every solid `part` draws is drawn, by key.
pub fn looks(session: &mut Session, part: &Id) -> Result<BTreeMap<String, SolidLook>> {
    let library = crate::materials::library(session)?;
    let drawn = drawn(session, part)?;
    let mut colours: BTreeMap<Id, Option<String>> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for d in &drawn {
        let material = colours
            .entry(d.part.clone())
            .or_insert_with(|| material_colour(session, &library, &d.part))
            .clone();
        out.insert(d.key.clone(), look_of(session, d, material.as_deref()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(colour: Option<&str>, opacity: Option<f64>, material: bool) -> Look {
        Look {
            colour: colour.map(str::to_owned),
            opacity,
            material,
        }
    }

    #[test]
    fn each_field_comes_from_the_most_specific_entry_that_sets_it() {
        let red = look(Some("#ff0000"), None, false);
        let half = look(None, Some(0.5), false);
        let blue_quarter = look(Some("#0000ff"), Some(0.25), false);
        let material = look(None, None, true);
        let e = |chain: &[&Look], m: Option<&str>| effective(chain, m);
        assert_eq!(e(&[], None), Effective::default());
        assert_eq!(
            e(&[], Some("#b8bcc2")),
            Effective {
                colour: Some("#b8bcc2".into()),
                opacity: 1.0
            }
        );
        assert_eq!(
            e(&[&half, &blue_quarter], None),
            Effective {
                colour: Some("#0000ff".into()),
                opacity: 0.5
            }
        );
        assert_eq!(
            e(&[&red, &blue_quarter], Some("#b8bcc2")),
            Effective {
                colour: Some("#ff0000".into()),
                opacity: 0.25
            }
        );
        // The material's colour ends the search, whatever is below.
        assert_eq!(
            e(&[&material, &red], Some("#b8bcc2")).colour.as_deref(),
            Some("#b8bcc2")
        );
        assert_eq!(e(&[&material, &red], None).colour, None);
        assert_eq!(rgb("#ff8000"), Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(rgb("red"), None);
    }
}
