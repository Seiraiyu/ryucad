//! Other projects a project depends on: the `dependencies` of
//! `ryucad.json`, by project ID, and where a project was forked from.
//! Files refer to a dependency by its project ID only; its `name` is a
//! handle for people and the CLI, so renaming one rewrites nothing else.

use super::files::{Obj, invalid};
use super::id::{Id, ProjectId};
use crate::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A dependency: another ryucad project, in the Git repository at `url`,
/// in its folder `path` (`""` for the repository's root).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    /// A part-ID-like handle, unique among the dependencies.
    pub name: String,
    pub url: String,
    pub path: String,
}

/// The dependencies, by project ID.
pub type Dependencies = BTreeMap<ProjectId, Dependency>;

/// Where `ryucad pdm new-id` forked a project from: the ID it had and the
/// commit it was at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkedFrom {
    pub project: ProjectId,
    pub commit: String,
}

impl Dependency {
    pub fn to_json(&self) -> Value {
        json!({"name": self.name, "path": self.path, "url": self.url})
    }

    /// `{name, url, path}` at `at`: strings, judged by [`faults`] rather
    /// than refused here, so a project whose dependency is badly written
    /// still opens.
    pub fn from_json(v: &Value, at: &str) -> Result<Dependency> {
        let mut o = Obj::new(v, at)?;
        let mut text = |key: &str| -> Result<String> {
            o.take(key)?
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(&format!("{at}/{key}"), &format!("{key} is a string")))
        };
        let name = text("name")?;
        let url = text("url")?;
        let path = text("path")?;
        o.finish()?;
        Ok(Dependency { name, url, path })
    }
}

impl ForkedFrom {
    pub fn to_json(&self) -> Value {
        json!({"commit": self.commit, "project": self.project})
    }

    pub fn from_json(v: &Value) -> Result<ForkedFrom> {
        let mut o = Obj::new(v, "/forked_from")?;
        let project = o.take("project")?;
        let project = project
            .as_str()
            .ok_or_else(|| invalid("/forked_from/project", "a project ID is a string"))
            .and_then(ProjectId::parse)
            .map_err(|e| e.with("pointer", "/forked_from/project"))?;
        let commit = o.take("commit")?;
        let commit = commit
            .as_str()
            .filter(|c| is_commit(c))
            .ok_or_else(|| invalid("/forked_from/commit", "a commit is 40 lowercase hex digits"))?
            .to_owned();
        o.finish()?;
        Ok(ForkedFrom { project, commit })
    }
}

/// Whether `s` is a commit's full hex name.
pub fn is_commit(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// The dependencies as `ryucad.json` holds them, by project ID.
pub fn to_json(deps: &Dependencies) -> Value {
    Value::Object(
        deps.iter()
            .map(|(id, d)| (id.to_string(), d.to_json()))
            .collect(),
    )
}

/// `dependencies` at `/dependencies`: an object of [`Dependency`] by
/// project ID.
pub fn from_json(v: &Value) -> Result<Dependencies> {
    let map = v.as_object().ok_or_else(|| {
        invalid(
            "/dependencies",
            "dependencies is an object of dependencies by project ID",
        )
    })?;
    let mut out = Dependencies::new();
    for (k, d) in map {
        let at = format!("/dependencies/{k}");
        let id = ProjectId::parse(k).map_err(|e| e.with("pointer", &at))?;
        out.insert(id, Dependency::from_json(d, &at)?);
    }
    Ok(out)
}

/// Why `url` can't name a dependency's repository, if it can't: it must
/// be a URL Git takes (`https://`, `http://`, `ssh://`, `git://`,
/// `file://`, `user@host:path`, or an absolute path), on one line, with
/// no password in it (the manifest is committed).
pub fn url_fault(url: &str) -> Option<&'static str> {
    if url.is_empty() {
        return Some("a dependency's url is empty");
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Some("a dependency's url has spaces or control characters");
    }
    let schemes = ["https://", "http://", "ssh://", "git://", "file://"];
    if let Some(scheme) = schemes.iter().find(|s| url.starts_with(**s)) {
        let rest = &url[scheme.len()..];
        let authority = rest.split('/').next().unwrap_or_default();
        if authority.contains(':') && authority.contains('@') && *scheme != "ssh://" {
            return Some("a dependency's url holds a password: use Git's credential helper");
        }
        if rest.is_empty() || (*scheme != "file://" && authority.is_empty()) {
            return Some("a dependency's url names no host");
        }
        return None;
    }
    // `user@host:path`, Git's scp-like form, or an absolute local path.
    let scp = url
        .split_once(':')
        .is_some_and(|(host, path)| !host.is_empty() && !host.contains('/') && !path.is_empty());
    let absolute = url.starts_with('/')
        || url.as_bytes().get(1) == Some(&b':') && url.as_bytes()[0].is_ascii_alphabetic();
    if scp || absolute {
        return None;
    }
    Some(
        "a dependency's url is https://, http://, ssh://, git://, file://, user@host:path or an absolute path",
    )
}

/// Why `path` can't be a project's folder in a repository, if it can't:
/// `""` or `/`-separated folder names, none empty, `.` or `..`, with no
/// `\` or `:`.
pub fn path_fault(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return None;
    }
    let bad = path
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == ".." || p.contains(['\\', ':']));
    bad.then_some(
        "a dependency's path is \"\" or a folder in its repository: names separated by /, none empty, . or ..",
    )
}

/// What is wrong with the manifest's dependencies, judged whole: each
/// `dependency_invalid` (a name that isn't a part-ID-like handle or that
/// another uses, a bad url or path) and `project_id_duplicate` (the
/// project's own ID as a dependency), each with its JSON pointer.
pub fn faults(own: Option<&ProjectId>, deps: &Dependencies) -> Vec<Error> {
    let mut out = Vec::new();
    let mut names: BTreeMap<&str, &ProjectId> = BTreeMap::new();
    for (id, d) in deps {
        let at = format!("/dependencies/{id}");
        if own == Some(id) {
            out.push(
                Error::new(
                    Code::ProjectIdDuplicate,
                    format!(
                        "dependency {} has this project's own ID {id}: a copied project needs an ID of its own (`ryucad pdm new-id`)",
                        d.name
                    ),
                )
                .with("project", id)
                .with("pointer", &at),
            );
        }
        if let Err(e) = Id::parse(&d.name, "dependency") {
            out.push(
                Error::new(Code::DependencyInvalid, e.message)
                    .with("project", id)
                    .with("pointer", format!("{at}/name")),
            );
        } else if let Some(other) = names.insert(&d.name, id) {
            out.push(
                Error::new(
                    Code::DependencyInvalid,
                    format!("two dependencies are named {}: {other} and {id}", d.name),
                )
                .with("project", id)
                .with("pointer", format!("{at}/name")),
            );
        }
        if let Some(why) = url_fault(&d.url) {
            out.push(
                Error::new(Code::DependencyInvalid, why)
                    .with("project", id)
                    .with("pointer", format!("{at}/url")),
            );
        }
        if let Some(why) = path_fault(&d.path) {
            out.push(
                Error::new(Code::DependencyInvalid, why)
                    .with("project", id)
                    .with("pointer", format!("{at}/path")),
            );
        }
    }
    out
}

/// The dependency `name` names (a dependency's name or its project ID),
/// `not_found` with the names when none does.
pub fn find<'a>(deps: &'a Dependencies, name: &str) -> Result<(&'a ProjectId, &'a Dependency)> {
    deps.iter()
        .find(|(id, d)| d.name == name || id.as_str() == name)
        .ok_or_else(|| {
            let names: Vec<&str> = deps.values().map(|d| d.name.as_str()).collect();
            Error::new(
                Code::NotFound,
                format!("no dependency {name}; the project's: {}", names.join(", ")),
            )
            .with("dependency", name)
            .with("dependencies", names)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(name: &str, url: &str, path: &str) -> Dependency {
        Dependency {
            name: name.into(),
            url: url.into(),
            path: path.into(),
        }
    }

    #[test]
    fn urls_are_ones_git_takes_without_passwords() {
        for ok in [
            "https://github.com/acme/fasteners.git",
            "http://git.local/x",
            "ssh://git@github.com/acme/x.git",
            "ssh://git:22@host/x",
            "git://host/x",
            "file:///srv/git/x.git",
            "git@github.com:acme/fasteners.git",
            "/srv/git/x.git",
            "C:/git/x",
        ] {
            assert_eq!(url_fault(ok), None, "{ok}");
        }
        for bad in [
            "",
            "https://",
            "https://me:secret@github.com/x",
            "fasteners",
            "https://github.com/a b",
            "relative/path",
        ] {
            assert!(url_fault(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn paths_are_folders_in_the_repository() {
        for ok in ["", "lib", "cad/fasteners"] {
            assert_eq!(path_fault(ok), None, "{ok}");
        }
        for bad in ["/", "/lib", "lib/", "a//b", ".", "a/../b", "a\\b", "c:x"] {
            assert!(path_fault(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn faults_name_their_pointers() {
        let own = ProjectId::from_bytes([1; 16]);
        let other = ProjectId::from_bytes([2; 16]);
        let third = ProjectId::from_bytes([3; 16]);
        let mut deps = Dependencies::new();
        deps.insert(own.clone(), dep("me", "https://h/me", ""));
        deps.insert(other.clone(), dep("lib", "https://h/lib", "a/../b"));
        deps.insert(third.clone(), dep("lib", "", ""));
        let f = faults(Some(&own), &deps);
        let got: Vec<(Code, String)> = f
            .iter()
            .map(|e| (e.code, e.details["pointer"].as_str().unwrap().to_owned()))
            .collect();
        assert_eq!(
            got,
            [
                (Code::ProjectIdDuplicate, format!("/dependencies/{own}")),
                (
                    Code::DependencyInvalid,
                    format!("/dependencies/{other}/path")
                ),
                (
                    Code::DependencyInvalid,
                    format!("/dependencies/{third}/name")
                ),
                (
                    Code::DependencyInvalid,
                    format!("/dependencies/{third}/url")
                ),
            ]
        );
        deps.remove(&own);
        deps.remove(&third);
        deps.get_mut(&other).unwrap().path = "lib".into();
        assert!(faults(Some(&own), &deps).is_empty());
        assert_eq!(find(&deps, "lib").unwrap().0, &other);
        assert_eq!(find(&deps, other.as_str()).unwrap().1.name, "lib");
        assert_eq!(find(&deps, "x").unwrap_err().code, Code::NotFound);
    }

    #[test]
    fn dependencies_read_and_write() {
        let id = ProjectId::from_bytes([9; 16]);
        let v = json!({id.to_string(): {"name": "fasteners", "path": "", "url": "file:///x"}});
        let d = from_json(&v).unwrap();
        assert_eq!(d[&id], dep("fasteners", "file:///x", ""));
        assert_eq!(to_json(&d), v);
        let e = from_json(&json!({"x": {}})).unwrap_err();
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidId, &json!("/dependencies/x"))
        );
        let e = from_json(&json!({id.to_string(): {"name": "a", "url": "u"}})).unwrap_err();
        assert_eq!(e.details["pointer"], format!("/dependencies/{id}/path"));
        let f = ForkedFrom::from_json(&json!({"project": id, "commit": "a".repeat(40)})).unwrap();
        assert_eq!(
            f.to_json(),
            json!({"project": id, "commit": "a".repeat(40)})
        );
        let e = ForkedFrom::from_json(&json!({"project": id, "commit": "HEAD"})).unwrap_err();
        assert_eq!(e.details["pointer"], "/forked_from/commit");
    }
}
