//! A lock: which commit each label of another project resolved to, and
//! the record's content hash there, per project. One format for every
//! product; its kind names the product's file (`ryucad.lock`,
//! `ryueda.lock`). ryucad's notes: one entry per project the build
//! reaches, direct or transitive, with where its repository is, the
//! release branch it was read on and, per `<part>/<label>`, the official
//! commit and the record's content hash.

use crate::error::Result;
use crate::files::{Obj, header_in, invalid};
use crate::id::{Id, ProjectId};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The lock's format, the same for every product's kind.
pub const FORMAT: u64 = 1;

/// Whether `s` is a commit as a lock writes it: 40 lowercase hex digits.
pub fn is_commit(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// A release label: 1 to 12 capital letters and digits (as ryucad's
/// assemblies check it).
fn label_text(s: &str) -> Result<String> {
    if s.is_empty()
        || s.len() > 12
        || !s
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(invalid(
            "",
            &format!("{s:?} isn't a release label: 1 to 12 capital letters and digits"),
        ));
    }
    Ok(s.to_owned())
}

/// The lock: every project the working files reach, by project ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lock {
    pub projects: BTreeMap<ProjectId, Locked>,
}

/// One project in the lock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Locked {
    /// Its repository, from the root's manifest, else from the manifest
    /// of the first project in `via` that names it.
    pub url: String,
    /// Its folder in the repository (`""` for the root).
    pub path: String,
    /// The branch whose history made its labels official.
    pub release_branch: String,
    /// The shortest chain of projects from the root that reaches it,
    /// empty for a direct dependency.
    pub via: Vec<ProjectId>,
    /// Each label reached, by part and label.
    pub labels: BTreeMap<(Id, String), LockedLabel>,
}

/// Where a label resolved: the commit its record became official at, and
/// the content hash the record names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedLabel {
    pub commit: String,
    pub content: String,
}

impl Lock {
    /// The label `part`/`label` of `project`, if the lock holds it.
    pub fn label(&self, project: &ProjectId, part: &Id, label: &str) -> Option<&LockedLabel> {
        self.projects
            .get(project)?
            .labels
            .get(&(part.clone(), label.to_owned()))
    }

    pub fn is_empty(&self) -> bool {
        self.projects.is_empty()
    }

    /// The lock as JSON of kind `kind`.
    pub fn to_json_as(&self, kind: &str) -> Value {
        let projects: Map<String, Value> = self
            .projects
            .iter()
            .map(|(id, p)| {
                let labels: Map<String, Value> = p
                    .labels
                    .iter()
                    .map(|((part, label), l)| {
                        (
                            format!("{part}/{label}"),
                            json!({"commit": l.commit, "content": l.content}),
                        )
                    })
                    .collect();
                (
                    id.to_string(),
                    json!({"labels": labels, "path": p.path,
                           "release_branch": p.release_branch, "url": p.url, "via": p.via}),
                )
            })
            .collect();
        json!({"format": FORMAT, "kind": kind, "projects": projects})
    }

    /// A lock of kind `kind` read from JSON. A format too new names the
    /// product the kind starts with (`ryucad` for `ryucad.lock`).
    pub fn from_json_as(v: &Value, kind: &str) -> Result<Lock> {
        let product = kind.split('.').next().unwrap_or(kind);
        let mut o = header_in(v, kind, FORMAT, product)?;
        let projects = o.take("projects")?;
        let map = projects
            .as_object()
            .ok_or_else(|| invalid("/projects", "projects is an object by project ID"))?;
        let mut out = BTreeMap::new();
        for (k, p) in map {
            let at = format!("/projects/{k}");
            let id = ProjectId::parse(k).map_err(|e| e.with("pointer", &at))?;
            out.insert(id, Locked::from_json(p, &at)?);
        }
        o.finish()?;
        Ok(Lock { projects: out })
    }
}

impl Locked {
    fn from_json(v: &Value, at: &str) -> Result<Locked> {
        let mut o = Obj::new(v, at)?;
        let text = |o: &mut Obj, key: &str| -> Result<String> {
            o.take(key)?
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(&format!("{at}/{key}"), &format!("{key} is a string")))
        };
        let url = text(&mut o, "url")?;
        let path = text(&mut o, "path")?;
        let release_branch = text(&mut o, "release_branch")?;
        let via_at = format!("{at}/via");
        let via = o
            .take("via")?
            .as_array()
            .ok_or_else(|| invalid(&via_at, "via is an array of project IDs"))?
            .iter()
            .enumerate()
            .map(|(i, p)| {
                p.as_str()
                    .ok_or_else(|| invalid(&format!("{via_at}/{i}"), "a project ID is a string"))
                    .and_then(ProjectId::parse)
                    .map_err(|e| e.with("pointer", format!("{via_at}/{i}")))
            })
            .collect::<Result<Vec<_>>>()?;
        let labels_at = format!("{at}/labels");
        let mut labels = BTreeMap::new();
        for (k, l) in o
            .take("labels")?
            .as_object()
            .ok_or_else(|| invalid(&labels_at, "labels is an object by <part>/<label>"))?
        {
            let here = format!("{labels_at}/{}", k.replace('~', "~0").replace('/', "~1"));
            let (part, label) = k
                .split_once('/')
                .ok_or_else(|| invalid(&here, "a lock label is <part>/<label>"))?;
            let part = Id::parse(part, "part").map_err(|e| e.with("pointer", &here))?;
            let label = label_text(label).map_err(|e| e.with("pointer", &here))?;
            let mut lo = Obj::new(l, &here)?;
            let mut hex = |key: &str, ok: fn(&str) -> bool, what: &str| -> Result<String> {
                lo.take(key)?
                    .as_str()
                    .filter(|s| ok(s))
                    .map(str::to_owned)
                    .ok_or_else(|| invalid(&format!("{here}/{key}"), what))
            };
            let commit = hex("commit", is_commit, "a commit is 40 lowercase hex digits")?;
            let content = hex(
                "content",
                |s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()),
                "a content hash is 64 hex digits",
            )?;
            lo.finish()?;
            labels.insert((part, label), LockedLabel { commit, content });
        }
        o.finish()?;
        Ok(Locked {
            url,
            path,
            release_branch,
            via,
            labels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock() -> Lock {
        let mut labels = BTreeMap::new();
        labels.insert(
            (Id::parse("r_4k7", "part").unwrap(), "B".to_owned()),
            LockedLabel {
                commit: "a".repeat(40),
                content: "c".repeat(64),
            },
        );
        let mut projects = BTreeMap::new();
        projects.insert(
            ProjectId::parse(&"1".repeat(32)).unwrap(),
            Locked {
                url: "file:///libs".into(),
                path: "lib".into(),
                release_branch: "main".into(),
                via: Vec::new(),
                labels,
            },
        );
        Lock { projects }
    }

    #[test]
    fn a_lock_reads_and_writes_as_its_kind() {
        let v = lock().to_json_as("ryueda.lock");
        assert_eq!(v["kind"], "ryueda.lock");
        assert_eq!(v["format"], FORMAT);
        assert_eq!(Lock::from_json_as(&v, "ryueda.lock").unwrap(), lock());
        let e = Lock::from_json_as(&v, "ryucad.lock").unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidDocument);
        let mut newer = v.clone();
        newer["format"] = json!(2);
        let e = Lock::from_json_as(&newer, "ryueda.lock").unwrap_err();
        assert!(e.message.contains("this ryueda reads"), "{}", e.message);
    }
}
