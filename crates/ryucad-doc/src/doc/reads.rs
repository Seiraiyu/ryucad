//! What reads each parameter (design 447, ryucad 23.B), closed over the
//! parameters in between. A part's parameter is read when one of the
//! part's listed features (a sketch's dimensions included) or explodes
//! reaches it, directly or through the part's other parameters. A
//! project parameter is read when some part's features or explodes reach
//! it, through the part's parameters and the project's. A parameter
//! nothing reaches is unused: changing it moves no result. A project
//! parameter reached only through a part parameter nothing reads is
//! unused too.
//!
//! Readers are named as `diff` and doctor print them: a feature by its
//! ID, an explode as `explodes/<id>` (an ID never holds `/`), and a part
//! by its ID.

use super::canonical;
use super::explode;
use super::files::{ParametersFile, PartFile, feature_names};
use super::id::Id;
use super::project::{PARAMETERS, Part, Project};
use super::source::{Kind, Source, join};
use crate::expr::Expr;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Who reaches each parameter of one scope, by name: every parameter of
/// the scope has an entry, empty when nothing reaches it.
pub type ReadBy = BTreeMap<String, BTreeSet<String>>;

/// A reader: its name, and the parameter names its text reads.
pub type Reader = (String, BTreeSet<String>);

/// What reaches every parameter of a project: each part's own, by part,
/// and the project's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    pub parts: BTreeMap<Id, ReadBy>,
    pub project: ReadBy,
}

/// The parameters `names` reach, as `(the part's own, the project's)`. A
/// name the part has is its own, as evaluation reads it; a project
/// parameter's names are only ever the project's.
pub fn reach(
    own: &BTreeMap<String, Expr>,
    project: &BTreeMap<String, Expr>,
    names: &BTreeSet<String>,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut mine = BTreeSet::new();
    let mut theirs = BTreeSet::new();
    // Each name, and whether it was named from the project's scope.
    let mut todo: Vec<(String, bool)> = names.iter().map(|n| (n.clone(), false)).collect();
    while let Some((n, outer)) = todo.pop() {
        match own.get(&n).filter(|_| !outer) {
            Some(e) => {
                if mine.insert(n) {
                    todo.extend(e.names().into_iter().map(|m| (m, false)));
                }
            }
            None => {
                if let Some(e) = project.get(&n)
                    && theirs.insert(n)
                {
                    todo.extend(e.names().into_iter().map(|m| (m, true)));
                }
            }
        }
    }
    (mine, theirs)
}

/// A part's readers' reach: who reaches each of its own parameters, and
/// the project parameters any of them reaches.
pub fn reach_part(
    own: &ParametersFile,
    project: &ParametersFile,
    readers: &[Reader],
) -> (ReadBy, BTreeSet<String>) {
    let mut by: ReadBy = own
        .parameters
        .keys()
        .map(|k| (k.clone(), BTreeSet::new()))
        .collect();
    let mut outer = BTreeSet::new();
    for (who, names) in readers {
        let (mine, theirs) = reach(&own.parameters, &project.parameters, names);
        for n in mine {
            by.entry(n).or_default().insert(who.clone());
        }
        outer.extend(theirs);
    }
    (by, outer)
}

/// Who reaches each project parameter: the parts whose readers reach it,
/// given each part's reach ([`reach_part`]'s second half).
pub fn reach_project(project: &ParametersFile, parts: &BTreeMap<Id, BTreeSet<String>>) -> ReadBy {
    let mut by: ReadBy = project
        .parameters
        .keys()
        .map(|k| (k.clone(), BTreeSet::new()))
        .collect();
    for (id, reached) in parts {
        for n in reached {
            by.entry(n.clone()).or_default().insert(id.to_string());
        }
    }
    by
}

/// The parameters of `file` that name `name` directly, in name order.
pub fn named_by(file: &ParametersFile, name: &str) -> Vec<String> {
    file.parameters
        .iter()
        .filter(|(_, e)| e.names().contains(name))
        .map(|(k, _)| k.clone())
        .collect()
}

/// A loaded part's readers: its listed features in order, then its
/// explodes by ID, then its motions by ID.
pub fn readers(part: &Part) -> Vec<Reader> {
    let features = part
        .order()
        .iter()
        .map(|f| (f.to_string(), part.uses.get(f).cloned().unwrap_or_default()));
    let explodes = part
        .explode_uses
        .iter()
        .map(|(e, names)| (format!("{}/{e}", explode::EXPLODES), names.clone()));
    let motions = part
        .motion_uses
        .iter()
        .map(|(m, names)| (format!("{}/{m}", super::motion::MOTIONS), names.clone()));
    features.chain(explodes).chain(motions).collect()
}

/// What reaches every parameter of a loaded project. A parameters file
/// that doesn't read has no parameters here.
pub fn in_project(project: &Project) -> Reads {
    let none = ParametersFile::default();
    let file = match &project.parameters {
        Some(Ok(f)) => f,
        _ => &none,
    };
    let mut parts = BTreeMap::new();
    let mut reached = BTreeMap::new();
    for (id, part) in &project.parts {
        let own = match &part.parameters {
            Some(Ok(f)) => f,
            _ => &none,
        };
        let (by, outer) = reach_part(own, file, &readers(part));
        parts.insert(id.clone(), by);
        reached.insert(id.clone(), outer);
    }
    Reads {
        project: reach_project(file, &reached),
        parts,
    }
}

fn json_in(src: &dyn Source, at: &str) -> Option<Value> {
    let b = src.read(at).ok()??;
    canonical::parse(std::str::from_utf8(&b).ok()?).ok()
}

/// The parameters file at `at` in `src`, or none when it is missing or
/// doesn't read.
pub fn parameters_in(src: &dyn Source, at: &str) -> ParametersFile {
    json_in(src, at)
        .and_then(|v| ParametersFile::from_json(&v).ok())
        .unwrap_or_default()
}

/// A part's own parameters and readers as `src` holds its files: its
/// listed features and its explodes, each read from its text (one that
/// doesn't read as JSON reads nothing).
pub fn readers_in(src: &dyn Source, part: &Id) -> (ParametersFile, Vec<Reader>) {
    let dir = join("parts", part.as_str());
    let own = parameters_in(src, &join(&dir, PARAMETERS));
    let mut out = Vec::new();
    if let Some(list) =
        json_in(src, &join(&dir, "part.json")).and_then(|v| PartFile::from_json(&v).ok())
    {
        let features = join(&dir, "features");
        for f in &list.features {
            let names = json_in(src, &join(&features, &format!("{f}.json")))
                .map(|v| feature_names(&v))
                .unwrap_or_default();
            out.push((f.to_string(), names));
        }
    }
    let explodes = join(&dir, explode::EXPLODES);
    for (name, kind) in src.list(&explodes).unwrap_or_default() {
        let Some(e) = name
            .strip_suffix(".json")
            .filter(|e| kind == Kind::File && explode::check_id(e).is_ok())
        else {
            continue;
        };
        if let Some(v) = json_in(src, &join(&explodes, &name)) {
            out.push((format!("{}/{e}", explode::EXPLODES), explode::names(&v)));
        }
    }
    let motions = join(&dir, super::motion::MOTIONS);
    for (name, kind) in src.list(&motions).unwrap_or_default() {
        let Some(m) = name
            .strip_suffix(".json")
            .filter(|m| kind == Kind::File && super::motion::check_id(m).is_ok())
        else {
            continue;
        };
        if let Some(v) = json_in(src, &join(&motions, &name)) {
            out.push((
                format!("{}/{m}", super::motion::MOTIONS),
                super::motion::names(&v),
            ));
        }
    }
    (own, out)
}

/// What reaches every parameter of the project whose files `src` holds:
/// the same as [`in_project`] over the project they load as.
pub fn in_source(src: &dyn Source) -> Reads {
    let file = parameters_in(src, PARAMETERS);
    let mut parts = BTreeMap::new();
    let mut reached = BTreeMap::new();
    for (name, kind) in src.list("parts").unwrap_or_default() {
        let Ok(id) = Id::parse(&name, "part") else {
            continue;
        };
        if kind != Kind::Dir {
            continue;
        }
        let (own, readers) = readers_in(src, &id);
        let (by, outer) = reach_part(&own, &file, &readers);
        parts.insert(id.clone(), by);
        reached.insert(id, outer);
    }
    Reads {
        project: reach_project(&file, &reached),
        parts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(v: serde_json::Value) -> ParametersFile {
        ParametersFile::from_json(&json!({"format": 1, "kind": "ryucad.parameters",
                                          "parameters": v}))
        .unwrap()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Files in memory, as a revision's are.
    fn files(list: &[(&str, serde_json::Value)]) -> BTreeMap<String, Vec<u8>> {
        list.iter()
            .map(|(p, v)| ((*p).to_owned(), canonical::to_string(v).into_bytes()))
            .collect()
    }

    fn box_x(x: &str) -> serde_json::Value {
        json!({"format": 2, "kind": "ryucad.feature", "type": "box",
               "params": {"x": x, "y": "1 mm", "z": "1 mm"}})
    }

    #[test]
    fn reach_follows_parameters_in_their_scopes() {
        let own = params(
            json!({"fin_h": "30 mm", "fin_t": "1.6 mm", "fin_k": "fin_t * 2",
                                "r": "wall / 2"}),
        );
        let project = params(json!({"wall": "3 mm", "gap": "wall", "spare": "1 mm"}));
        let (mine, theirs) = reach(&own.parameters, &project.parameters, &set(&["fin_h", "r"]));
        assert_eq!(mine, set(&["fin_h", "r"]));
        assert_eq!(theirs, set(&["wall"]));
        // A project parameter's names are the project's alone.
        let (mine, theirs) = reach(&own.parameters, &project.parameters, &set(&["gap"]));
        assert!(mine.is_empty());
        assert_eq!(theirs, set(&["gap", "wall"]));
        // A name nobody has reaches nothing.
        let (mine, theirs) = reach(&own.parameters, &project.parameters, &set(&["nope"]));
        assert!(mine.is_empty() && theirs.is_empty());
    }

    #[test]
    fn a_parameter_nothing_reaches_has_no_readers() {
        let own = params(
            json!({"fin_h": "30 mm", "fin_t": "1.6 mm", "fin_k": "fin_t * 2",
                                "span": "72 mm", "r": "wall"}),
        );
        let project = params(json!({"wall": "3 mm", "spare": "1 mm"}));
        let readers = vec![
            ("fin".to_owned(), set(&["fin_h", "span"])),
            ("explodes/apart".to_owned(), set(&["span"])),
        ];
        let (by, outer) = reach_part(&own, &project, &readers);
        assert_eq!(by["fin_h"], set(&["fin"]));
        assert_eq!(by["span"], set(&["explodes/apart", "fin"]));
        // `fin_k` reads `fin_t`, and nothing reads `fin_k`; `r` reads
        // `wall`, and nothing reads `r`: all unread, `wall` too.
        for n in ["fin_t", "fin_k", "r"] {
            assert!(by[n].is_empty(), "{n}");
        }
        assert!(outer.is_empty());
        assert_eq!(named_by(&own, "fin_t"), ["fin_k"]);
        let parts = BTreeMap::from([(Id::parse("p", "part").unwrap(), outer)]);
        let project_by = reach_project(&project, &parts);
        assert!(project_by["wall"].is_empty());
        assert!(project_by["spare"].is_empty());
    }

    #[test]
    fn the_same_from_files_or_a_loaded_project() {
        let src = files(&[
            (
                "ryucad.json",
                json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
            ),
            (
                "parameters.json",
                json!({"format": 1, "kind": "ryucad.parameters",
                                       "parameters": {"wall": "3 mm", "spare": "1 mm"}}),
            ),
            (
                "parts/p/part.json",
                json!({"features": ["a"], "format": 1, "kind": "ryucad.part"}),
            ),
            (
                "parts/p/parameters.json",
                json!({"format": 1, "kind": "ryucad.parameters",
                                               "parameters": {"w": "wall * 2", "t": "1 mm"}}),
            ),
            ("parts/p/features/a.json", box_x("w")),
            // Not listed: it reads nothing.
            ("parts/p/features/z.json", box_x("t")),
            (
                "parts/q/part.json",
                json!({"features": [], "format": 1, "kind": "ryucad.part"}),
            ),
        ]);
        let found = in_source(&src);
        let p = Id::parse("p", "part").unwrap();
        let q = Id::parse("q", "part").unwrap();
        assert_eq!(found.parts[&p]["w"], set(&["a"]));
        assert!(found.parts[&p]["t"].is_empty());
        assert!(found.parts[&q].is_empty());
        assert_eq!(found.project["wall"], set(&["p"]));
        assert!(found.project["spare"].is_empty());
        let project =
            super::super::project::Project::load_from(std::path::Path::new("/reads"), &src)
                .unwrap();
        assert_eq!(in_project(&project), found);
    }
}
