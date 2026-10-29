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
use crate::doc::appearance::{APPEARANCE, AppearanceFile, Look, check_opacity, face_key};
use crate::doc::assembly::Rev;
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::materials::check_colour;
use crate::doc::project::{part_dir, remove_file, write_json};
use crate::error::{Code, Error, Result};
use crate::rebuild::Built;
use crate::server::{check_revision, part_id, part_kind};
use crate::session::Session;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
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

/// The keys an assembly's entries may name: each instance path (and
/// every path above it) of its occurrences, and each occurrence's
/// solids.
fn occurrence_keys(session: &mut Session, part: &Id) -> Result<BTreeSet<String>> {
    let model = session.model(part, &Rev::Working)?;
    let mut keys = BTreeSet::new();
    for o in &model.occurrences {
        for k in 1..=o.path.len() {
            if o.path[k - 1].copy_of().is_none() {
                keys.insert(path_text(&o.path[..k]));
            }
        }
        let path = path_text(&o.path);
        for s in &o.solids {
            keys.insert(format!("{path}/{}", s.founder));
        }
    }
    Ok(keys)
}

/// A part's solids as shown, by founder.
fn shown_solids(session: &mut Session, part: &Id) -> Result<BTreeMap<Id, Arc<Built>>> {
    let p = session.project().part(part)?;
    let solids = crate::solid::Solids::of(p);
    let built = session.rebuilt(part)?;
    Ok(built
        .shown(&solids)
        .into_iter()
        .filter_map(|s| Some((s.solid.clone(), Arc::clone(s.state?.1))))
        .collect())
}

/// `appearance_unresolved`, for `key` of `part`'s file at `pointer`.
fn unresolved(part: &Id, pointer: &str, key: &str, what: &str) -> Value {
    json!({
        "code": Code::AppearanceUnresolved,
        "message": format!("{part}'s appearance names {what} {key}, which the model no longer has: it is kept, and colours nothing"),
        "part": part,
        "pointer": pointer,
        "key": key,
    })
}

fn escape(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

/// What `part`'s appearance names that its model no longer has, as
/// `appearance_unresolved` warnings: solids not shown, faces gone (with
/// `removed_by`, the feature that took one away, where the session's
/// rebuild says), occurrences and their solids the assembly doesn't
/// place. Empty for a part whose rebuild fails as a whole.
pub fn unresolved_entries(session: &mut Session, part: &Id) -> Vec<Value> {
    let Some(f) = file(session, part).cloned() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if !f.occurrences.is_empty() {
        let Ok(keys) = occurrence_keys(session, part) else {
            return out;
        };
        for k in f.occurrences.keys().filter(|k| !keys.contains(*k)) {
            out.push(unresolved(
                part,
                &format!("/occurrences/{}", escape(k)),
                k,
                "the occurrence",
            ));
        }
        return out;
    }
    let Ok(shown) = shown_solids(session, part) else {
        return out;
    };
    for k in f.solids.keys() {
        if Id::parse(k, "solid").is_ok_and(|s| !shown.contains_key(&s)) {
            out.push(unresolved(
                part,
                &format!("/solids/{}", escape(k)),
                k,
                "the solid",
            ));
        }
    }
    for k in f.faces.keys() {
        let Some((solid, name)) = k.split_once('/') else {
            continue;
        };
        let solid = Id::parse(solid, "solid").ok();
        let alive = solid.as_ref().and_then(|s| shown.get(s)).is_some_and(|b| {
            crate::doc::reference::parse_name(name).is_some_and(|n| b.body.by_name(n).is_some())
        });
        if alive {
            continue;
        }
        let mut w = unresolved(part, &format!("/faces/{}", escape(k)), k, "the face");
        if let Some(by) = solid.and_then(|s| session.removed_from(part, &s, name)) {
            w["removed_by"] = json!(by);
        }
        out.push(w);
    }
    out
}

/// The parts `appearance.get` and `render` answer for: the one asked for,
/// or the project's model.
fn parts_of(session: &Session, part: Option<&str>) -> Result<Vec<Id>> {
    match part {
        Some(p) => {
            let id = part_id(p)?;
            session.project().part(&id)?;
            Ok(vec![id])
        }
        None => Ok(crate::assembly::shown_parts(session.project())),
    }
}

/// `appearance.get`: every solid `part` draws (else the project's model),
/// each with its appearance (`colour` left out for the viewer's own) and
/// the faces drawn otherwise, by name; each part's revision; and the
/// entries naming what the model no longer has as warnings.
pub fn get(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let parts = parts_of(session, part)?;
    let mut solids = Vec::new();
    let mut revisions = Map::new();
    let mut warnings = Vec::new();
    let mut reached: BTreeSet<Id> = BTreeSet::new();
    for p in &parts {
        for (key, look) in looks(session, p)? {
            let mut v = look.base.to_json();
            v["part"] = json!(p);
            v["solid"] = json!(key);
            if !look.faces.is_empty() {
                let faces: Map<String, Value> = look
                    .faces
                    .iter()
                    .map(|(n, e)| (n.clone(), e.to_json()))
                    .collect();
                v["faces"] = Value::Object(faces);
            }
            solids.push(v);
        }
        reached.insert(p.clone());
        if part_kind(session.project().part(p)?) == PartKind::Assembly {
            let model = session.model(p, &Rev::Working)?;
            for o in &model.occurrences {
                reached.insert(o.part.clone());
                reached.extend(assemblies(session, p, &o.path).into_iter().map(|(a, _)| a));
            }
        }
    }
    for p in &reached {
        revisions.insert(p.to_string(), json!(session.project().part(p)?.revision));
        warnings.extend(unresolved_entries(session, p));
    }
    Ok(json!({"solids": solids, "revisions": revisions, "warnings": warnings}))
}

/// What an entry of a part's file a reference names: `part`, a solid,
/// a face, or (in an assembly) an occurrence or an occurrence's solid.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Entry {
    Part,
    Solid(String),
    Face(String),
    Occurrence(String),
}

impl Entry {
    /// Its key as `appearance.set` answers it: `part`, or the entry's
    /// pointer.
    pub fn pointer(&self) -> String {
        match self {
            Entry::Part => "/part".into(),
            Entry::Solid(k) => format!("/solids/{}", escape(k)),
            Entry::Face(k) => format!("/faces/{}", escape(k)),
            Entry::Occurrence(k) => format!("/occurrences/{}", escape(k)),
        }
    }

    fn get<'a>(&self, f: &'a AppearanceFile) -> Option<&'a Look> {
        match self {
            Entry::Part => f.part.as_ref(),
            Entry::Solid(k) => f.solids.get(k),
            Entry::Face(k) => f.faces.get(k),
            Entry::Occurrence(k) => f.occurrences.get(k),
        }
    }

    fn put(&self, f: &mut AppearanceFile, look: Option<Look>) {
        match (self, look) {
            (Entry::Part, l) => f.part = l,
            (Entry::Solid(k), Some(l)) => drop(f.solids.insert(k.clone(), l)),
            (Entry::Solid(k), None) => drop(f.solids.remove(k)),
            (Entry::Face(k), Some(l)) => drop(f.faces.insert(k.clone(), l)),
            (Entry::Face(k), None) => drop(f.faces.remove(k)),
            (Entry::Occurrence(k), Some(l)) => drop(f.occurrences.insert(k.clone(), l)),
            (Entry::Occurrence(k), None) => drop(f.occurrences.remove(k)),
        }
    }
}

fn not_colourable(reference: &str, why: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("{reference} can't be coloured: {why}"),
    )
    .with("entity", reference)
}

/// The part a reference is in and the entry it names, read from its
/// text and the part's kind alone (so an entry the model has lost can be
/// cleared).
pub fn entry_of(session: &Session, reference: &str) -> Result<(Id, Entry)> {
    let (part, rest) = match reference.split_once('/') {
        Some((p, r)) => (part_id(p)?, Some(r)),
        None => (part_id(reference)?, None),
    };
    let p = session.project().part(&part)?;
    let assembly = part_kind(p) == PartKind::Assembly;
    let Some(rest) = rest else {
        if assembly {
            return Err(not_colourable(
                reference,
                "an assembly colours its occurrences (<assembly>/<instance>…) and their solids",
            ));
        }
        return Ok((part, Entry::Part));
    };
    let segments: Vec<&str> = rest.split('/').collect();
    let is_name = |s: &str| crate::doc::reference::parse_name(s).is_some();
    if assembly {
        if segments.len() > 1 && is_name(segments[segments.len() - 1]) {
            return Err(not_colourable(
                reference,
                "an assembly colours occurrences and their solids, not faces: colour the face in its part",
            ));
        }
        for s in &segments {
            crate::doc::id::parse_step(s, "").map_err(|mut e| {
                e.code = Code::InvalidParam;
                e.with("entity", reference)
            })?;
        }
        return Ok((part, Entry::Occurrence(rest.to_owned())));
    }
    match segments.as_slice() {
        [solid] => Ok((part, Entry::Solid(Id::parse(solid, "solid")?.to_string()))),
        [solid, name] if is_name(name) => Ok((
            part,
            Entry::Face(face_key(Id::parse(solid, "solid")?.as_str(), name)),
        )),
        _ => Err(not_colourable(
            reference,
            "a part colours itself, its solids (<part>/<solid>) and their faces (<part>/<solid>/<name>); not edges, vertices or sketches",
        )),
    }
}

/// Checks that `entry` of `part` names something the model has now.
fn check_resolves(session: &mut Session, part: &Id, entry: &Entry, reference: &str) -> Result<()> {
    let missing = |what: &str| {
        Error::new(
            Code::NotFound,
            format!("{reference} names no {what} of {part}'s model"),
        )
        .with("entity", reference)
    };
    match entry {
        Entry::Part => Ok(()),
        Entry::Solid(k) => {
            let shown = shown_solids(session, part)?;
            Id::parse(k, "solid")
                .ok()
                .filter(|s| shown.contains_key(s))
                .map(drop)
                .ok_or_else(|| missing("solid"))
        }
        Entry::Face(k) => {
            let (solid, name) = k.split_once('/').expect("a face key has a solid");
            let shown = shown_solids(session, part)?;
            let body = Id::parse(solid, "solid")
                .ok()
                .and_then(|s| shown.get(&s).cloned())
                .ok_or_else(|| missing("solid"))?;
            let n = crate::doc::reference::parse_name(name).ok_or_else(|| missing("face"))?;
            match body.body.by_name(n) {
                Some(ryukei::topo::EntityId::Face(_)) => Ok(()),
                Some(_) => Err(not_colourable(
                    reference,
                    "it is an edge or a vertex; colour faces",
                )),
                None => Err(missing("face")),
            }
        }
        Entry::Occurrence(k) => {
            if occurrence_keys(session, part)?.contains(k) {
                Ok(())
            } else {
                Err(missing("occurrence or occurrence's solid"))
            }
        }
    }
}

/// What `appearance.set` changes in an entry.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub colour: Option<String>,
    pub material: bool,
    pub opacity: Option<f64>,
    /// Fields taken out: `colour` (and `material`) or `opacity`.
    pub unset: Vec<String>,
}

impl Change {
    fn check(&self) -> Result<()> {
        if let Some(c) = &self.colour {
            check_colour(c)?;
        }
        if self.colour.is_some() && self.material {
            return Err(Error::new(
                Code::InvalidParam,
                "give a colour or the material's colour, not both",
            ));
        }
        if let Some(o) = self.opacity
            && check_opacity(Some(o)).is_none()
        {
            return Err(Error::new(
                Code::InvalidParam,
                format!("bad opacity {o}: expected above 0 and at most 1"),
            )
            .with("opacity", o));
        }
        for u in &self.unset {
            if !matches!(u.as_str(), "colour" | "opacity") {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("can't unset {u:?}: unset colour or opacity"),
                )
                .with("unset", u));
            }
        }
        if self.colour.is_none()
            && !self.material
            && self.opacity.is_none()
            && self.unset.is_empty()
        {
            return Err(Error::new(
                Code::InvalidParam,
                "nothing to set: give a colour, the material's colour, an opacity or what to unset",
            ));
        }
        Ok(())
    }

    fn apply(&self, look: Option<&Look>) -> Option<Look> {
        let mut l = look.cloned().unwrap_or_default();
        for u in &self.unset {
            match u.as_str() {
                "colour" => {
                    l.colour = None;
                    l.material = false;
                }
                _ => l.opacity = None,
            }
        }
        if let Some(c) = &self.colour {
            l.colour = Some(c.to_ascii_lowercase());
            l.material = false;
        }
        if self.material {
            l.colour = None;
            l.material = true;
        }
        if let Some(o) = self.opacity {
            l.opacity = Some(o);
        }
        (!l.is_empty()).then_some(l)
    }
}

/// The part every reference is in, and the entries they name; one part
/// only.
fn entries(session: &Session, references: &[String]) -> Result<(Id, Vec<(String, Entry)>)> {
    if references.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            "name what to colour: a part, a solid, a face or an occurrence",
        ));
    }
    let mut part: Option<Id> = None;
    let mut out = Vec::new();
    for r in references {
        let (p, e) = entry_of(session, r)?;
        if let Some(first) = &part
            && *first != p
        {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{r} is in {p}, not {first}: colour one part at a time"),
            )
            .with("entity", r));
        }
        part = Some(p);
        out.push((r.clone(), e));
    }
    Ok((part.expect("one reference at least"), out))
}

/// Writes `file` as `part`'s appearance, or removes it when empty, and
/// answers each entry now.
fn write(
    session: &mut Session,
    part: &Id,
    file: &AppearanceFile,
    touched: &[(String, Entry)],
) -> Result<Value> {
    let at = part_dir(session.root(), part).join(APPEARANCE);
    if file.is_empty() {
        if at.exists() {
            remove_file(session.root(), &at)?;
        }
    } else {
        write_json(&at, &file.to_json())?;
    }
    session.reload()?;
    let p = session.project().part(part)?;
    let entries: Vec<Value> = touched
        .iter()
        .map(|(r, e)| {
            json!({
                "entity": r,
                "pointer": e.pointer(),
                "look": e.get(file).map_or(Value::Null, Look::to_json),
            })
        })
        .collect();
    Ok(json!({"part": part, "entries": entries, "revision": p.revision}))
}

/// The part's appearance as it stands: its file, or none; refused while
/// the file doesn't read.
fn current(session: &Session, part: &Id) -> Result<AppearanceFile> {
    match &session.project().part(part)?.appearance {
        None => Ok(AppearanceFile::default()),
        Some(Ok(f)) => Ok(f.clone()),
        Some(Err(e)) => Err(e.clone()),
    }
}

/// `appearance.set`: `change` laid over each entry `references` name, all
/// in one part, each naming something its model has now.
pub fn set(
    session: &mut Session,
    references: &[String],
    change: &Change,
    expected: Option<&str>,
) -> Result<Value> {
    change.check()?;
    let (part, touched) = entries(session, references)?;
    check_revision(session.project().part(&part)?, expected)?;
    for (r, e) in &touched {
        check_resolves(session, &part, e, r)?;
    }
    let mut file = current(session, &part)?;
    for (_, e) in &touched {
        let now = change.apply(e.get(&file));
        e.put(&mut file, now);
    }
    write(session, &part, &file, &touched)
}

/// `appearance.clear`: each entry `references` name taken out, whether or
/// not the model still has what it names.
pub fn clear(
    session: &mut Session,
    references: &[String],
    expected: Option<&str>,
) -> Result<Value> {
    let (part, touched) = entries(session, references)?;
    check_revision(session.project().part(&part)?, expected)?;
    let mut file = current(session, &part)?;
    for (_, e) in &touched {
        e.put(&mut file, None);
    }
    write(session, &part, &file, &touched)
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

    #[test]
    fn changes_set_and_unset_fields() {
        let c =
            |colour: Option<&str>, material: bool, opacity: Option<f64>, unset: &[&str]| Change {
                colour: colour.map(str::to_owned),
                material,
                opacity,
                unset: unset.iter().map(|s| s.to_string()).collect(),
            };
        let red = c(Some("#FF0000"), false, None, &[]);
        red.check().unwrap();
        assert_eq!(red.apply(None), Some(look(Some("#ff0000"), None, false)));
        let mat = c(None, true, Some(0.5), &[]);
        assert_eq!(
            mat.apply(Some(&look(Some("#ff0000"), None, false))),
            Some(look(None, Some(0.5), true))
        );
        assert_eq!(
            c(None, false, None, &["colour"]).apply(Some(&look(None, None, true))),
            None
        );
        assert_eq!(
            c(None, false, None, &["opacity"]).apply(Some(&look(
                Some("#000000"),
                Some(0.5),
                false
            ))),
            Some(look(Some("#000000"), None, false))
        );
        for bad in [
            c(None, false, None, &[]),
            c(Some("#ff0000"), true, None, &[]),
            c(Some("red"), false, None, &[]),
            c(None, false, Some(0.0), &[]),
            c(None, false, Some(1.5), &[]),
            c(None, false, None, &["shine"]),
        ] {
            assert_eq!(bad.check().unwrap_err().code, Code::InvalidParam, "{bad:?}");
        }
    }

    use crate::server::tests::{call, fresh};

    /// A plate (a box `base` and a cylinder `boss`), a bolt (`shank`), a
    /// pair of bolts, and a frame placing the plate and two pairs.
    fn project() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        let mut ok = |m: &str, v: Value| {
            call(&mut s, m, v).unwrap_or_else(|e| panic!("{m}: {e}"));
        };
        ok("part.create", json!({"part": "bolt"}));
        ok(
            "feature.add",
            json!({"part": "bolt", "type": "cylinder", "id": "shank",
                   "params": {"radius": 2, "height": 10}}),
        );
        ok("part.create", json!({"part": "plate"}));
        ok(
            "feature.add",
            json!({"part": "plate", "type": "box", "id": "base",
                   "params": {"x": 60, "y": 30, "z": 6}}),
        );
        ok(
            "feature.add",
            json!({"part": "plate", "type": "cylinder", "id": "boss",
                   "params": {"radius": 5, "height": 4}, "placement": {"origin": [0, 0, 6]}}),
        );
        for a in ["pair", "frame"] {
            ok("part.create", json!({"part": a, "kind": "assembly"}));
        }
        for (id, x) in [("left", -8), ("right", 8)] {
            ok(
                "feature.add",
                json!({"part": "pair", "type": "instance", "id": id, "of": "bolt",
                       "placement": {"origin": [x, 0, 0]}}),
            );
        }
        ok(
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "base", "of": "plate"}),
        );
        for (id, x) in [("p1", 15), ("p2", 45)] {
            ok(
                "feature.add",
                json!({"part": "frame", "type": "instance", "id": id, "of": "pair",
                       "placement": {"origin": [x, 15, 6]}}),
            );
        }
        (dir, s)
    }

    /// The names of `solid`'s faces in `part`, in order.
    fn faces(s: &mut Session, part: &str, solid: &str) -> Vec<String> {
        let d = drawn(s, &Id::parse(part, "part").unwrap()).unwrap();
        let b = &d.iter().find(|d| d.key == solid).unwrap().built.body;
        b.faces()
            .map(|f| b.face(f).unwrap().name.to_string())
            .collect()
    }

    /// `appearance.get`'s solid `key` of `part`.
    fn solid(v: &Value, part: &str, key: &str) -> Value {
        v["solids"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["part"] == part && x["solid"] == key)
            .unwrap_or_else(|| panic!("no {part} {key} in {v}"))
            .clone()
    }

    #[test]
    fn a_part_colours_itself_its_solids_and_its_faces() {
        let (dir, mut s) = project();
        let get = |s: &mut Session| call(s, "appearance.get", json!({"part": "plate"})).unwrap();
        let before = get(&mut s);
        assert_eq!(
            solid(&before, "plate", "base"),
            json!({"part": "plate", "solid": "base", "opacity": 1})
        );
        let rev = before["revisions"]["plate"].clone();
        let top = faces(&mut s, "plate", "base")[0].clone();
        let v = call(
            &mut s,
            "appearance.set",
            json!({"entities": ["plate"], "colour": "#D03030", "expected_revision": rev}),
        )
        .unwrap();
        assert_eq!(
            v["entries"],
            json!([{"entity": "plate", "pointer": "/part", "look": {"colour": "#d03030"}}])
        );
        assert_ne!(v["revision"], rev);
        // Stale, refused.
        let e = call(
            &mut s,
            "appearance.set",
            json!({"entities": ["plate"], "opacity": 0.5, "expected_revision": rev}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        call(
            &mut s,
            "appearance.set",
            json!({"entities": ["plate/boss"], "opacity": 0.5}),
        )
        .unwrap();
        call(
            &mut s,
            "appearance.set",
            json!({"entities": [format!("plate/base/{top}")], "colour": "#2050c0"}),
        )
        .unwrap();
        let v = get(&mut s);
        assert_eq!(
            solid(&v, "plate", "base"),
            json!({"part": "plate", "solid": "base", "colour": "#d03030", "opacity": 1,
                   "faces": {top.clone(): {"colour": "#2050c0", "opacity": 1}}})
        );
        assert_eq!(
            solid(&v, "plate", "boss"),
            json!({"part": "plate", "solid": "boss", "colour": "#d03030", "opacity": 0.5})
        );
        assert_eq!(v["warnings"], json!([]));
        let file = dir.path().join("parts/plate/appearance.json");
        assert_eq!(
            crate::doc::canonical::parse(&std::fs::read_to_string(&file).unwrap()).unwrap(),
            json!({"faces": {format!("base/{top}"): {"colour": "#2050c0"}},
                   "format": 1, "kind": "ryucad.appearance",
                   "part": {"colour": "#d03030"},
                   "solids": {"boss": {"opacity": 0.5}}})
        );
        // What can't be coloured, and what isn't there.
        let edge = {
            let d = drawn(&mut s, &Id::parse("plate", "part").unwrap()).unwrap();
            let b = &d[0].built.body;
            let e = b.edges().next().unwrap();
            format!("plate/{}/{}", d[0].key, b.edge(e).unwrap().name)
        };
        for (entities, code) in [
            (json!([edge]), Code::InvalidParam),
            (json!(["plate/base/p3"]), Code::InvalidParam),
            (json!(["plate", "bolt"]), Code::InvalidParam),
            (json!(["frame"]), Code::InvalidParam),
            (
                json!([format!("frame/base/base/{top}")]),
                Code::InvalidParam,
            ),
            (json!(["plate/nothing"]), Code::NotFound),
            (json!([format!("plate/boss/{top}")]), Code::NotFound),
            (json!(["frame/p3"]), Code::NotFound),
            (json!([]), Code::InvalidParam),
        ] {
            let e = call(
                &mut s,
                "appearance.set",
                json!({"entities": entities, "colour": "#000000"}),
            )
            .unwrap_err();
            assert_eq!(e.code, code, "{entities}");
        }
        // Cleared one by one, the file goes with its last entry.
        call(
            &mut s,
            "appearance.clear",
            json!({"entities": ["plate", "plate/boss", format!("plate/base/{top}")]}),
        )
        .unwrap();
        assert!(!file.exists());
        assert_eq!(get(&mut s)["revisions"]["plate"], rev);
    }

    #[test]
    fn assemblies_override_what_they_place_outermost_first() {
        let (_dir, mut s) = project();
        let set = |s: &mut Session, e: &[&str], v: Value| {
            let mut v = v;
            v["entities"] = json!(e);
            call(s, "appearance.set", v).unwrap_or_else(|e| panic!("{e}"))
        };
        set(&mut s, &["bolt"], json!({"colour": "#101010"}));
        set(
            &mut s,
            &["pair/left"],
            json!({"colour": "#202020", "opacity": 0.5}),
        );
        set(&mut s, &["frame/p1"], json!({"colour": "#303030"}));
        set(&mut s, &["frame/p2/right/shank"], json!({"opacity": 0.25}));
        let v = call(&mut s, "appearance.get", json!({"part": "frame"})).unwrap();
        let look = |key: &str| {
            let x = solid(&v, "frame", key);
            (x["colour"].clone(), x["opacity"].clone())
        };
        assert_eq!(look("p1/left/shank"), (json!("#303030"), json!(0.5)));
        assert_eq!(look("p1/right/shank"), (json!("#303030"), json!(1)));
        assert_eq!(look("p2/left/shank"), (json!("#202020"), json!(0.5)));
        assert_eq!(look("p2/right/shank"), (json!("#101010"), json!(0.25)));
        assert_eq!(look("base/base"), (Value::Null, json!(1)));
        // Every part it reaches gives its revision.
        let revisions: Vec<&String> = v["revisions"].as_object().unwrap().keys().collect();
        assert_eq!(revisions, ["bolt", "frame", "pair", "plate"]);
        // Its occurrences' paths and their solids, and nothing else.
        for (e, code) in [
            ("frame/p1/left/boss", Code::NotFound),
            ("frame/p1/left/shank/x", Code::NotFound),
            (
                "frame/p1/left/shank/0123456789abcdef0123456789abcdef",
                Code::InvalidParam,
            ),
            ("pair/middle", Code::NotFound),
        ] {
            let err = call(
                &mut s,
                "appearance.set",
                json!({"entities": [e], "opacity": 0.5}),
            )
            .unwrap_err();
            assert_eq!(err.code, code, "{e}");
        }
        set(&mut s, &["pair/left/shank"], json!({"opacity": 0.75}));
    }

    #[test]
    fn a_material_colours_what_names_nothing_else() {
        let (_dir, mut s) = project();
        call(
            &mut s,
            "materials.set",
            json!({"name": "brass", "density": "8.5 g/cm^3", "colour": "#b5a642",
                   "default": true}),
        )
        .unwrap();
        call(
            &mut s,
            "appearance.set",
            json!({"entities": ["plate"], "colour": "#d03030"}),
        )
        .unwrap();
        call(
            &mut s,
            "appearance.set",
            json!({"entities": ["plate/boss"], "material": true}),
        )
        .unwrap();
        let get = |s: &mut Session, p: &str| call(s, "appearance.get", json!({"part": p})).unwrap();
        let v = get(&mut s, "plate");
        assert_eq!(solid(&v, "plate", "base")["colour"], "#d03030");
        assert_eq!(solid(&v, "plate", "boss")["colour"], "#b5a642");
        assert_eq!(
            solid(&get(&mut s, "bolt"), "bolt", "shank")["colour"],
            "#b5a642"
        );
        // With no part, the project's model: its one root assembly.
        let v = call(&mut s, "appearance.get", json!({})).unwrap();
        assert_eq!(solid(&v, "frame", "p1/left/shank")["colour"], "#b5a642");
        assert_eq!(solid(&v, "frame", "base/base")["colour"], "#d03030");
    }

    #[test]
    fn entries_the_model_lost_are_kept_and_reported() {
        let (_dir, mut s) = project();
        let names = faces(&mut s, "plate", "base");
        let refs: Vec<String> = names.iter().map(|n| format!("plate/base/{n}")).collect();
        call(
            &mut s,
            "appearance.set",
            json!({"entities": refs, "colour": "#2050c0"}),
        )
        .unwrap();
        call(
            &mut s,
            "appearance.set",
            json!({"entities": ["plate/boss"], "colour": "#2050c0"}),
        )
        .unwrap();
        // A cut takes the plate's top away; deleting the boss its solid.
        call(
            &mut s,
            "feature.add",
            json!({"part": "plate", "type": "box", "id": "cut", "operation": "cut",
                   "targets": ["base"], "params": {"x": 80, "y": 40, "z": 10},
                   "placement": {"origin": [-10, -5, 4]}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.delete",
            json!({"part": "plate", "id": "boss"}),
        )
        .unwrap();
        let v = call(&mut s, "appearance.get", json!({"part": "plate"})).unwrap();
        let w = v["warnings"].as_array().unwrap();
        assert!(
            w.iter().all(|w| w["code"] == "appearance_unresolved"),
            "{v}"
        );
        assert!(
            w.iter()
                .any(|w| w["key"] == "boss" && w["pointer"] == "/solids/boss")
        );
        let faces_gone: Vec<&Value> = w
            .iter()
            .filter(|w| w["pointer"].as_str().unwrap().starts_with("/faces/"))
            .collect();
        assert!(!faces_gone.is_empty(), "{v}");
        assert!(faces_gone.iter().all(|w| w["removed_by"] == "cut"), "{v}");
        // Kept in the file, and clearable though nothing has them.
        let gone = faces_gone[0]["key"].as_str().unwrap().to_owned();
        call(
            &mut s,
            "appearance.clear",
            json!({"entities": [format!("plate/{gone}"), "plate/boss"]}),
        )
        .unwrap();
        let v = call(&mut s, "appearance.get", json!({"part": "plate"})).unwrap();
        assert_eq!(
            v["warnings"].as_array().unwrap().len(),
            faces_gone.len() - 1
        );
    }
}
