//! `ryucad.lock`: which commit each label of another project that the
//! working files reach resolved to, committed beside the manifest so
//! every clone and the service build the same thing. One entry per
//! project the build reaches, direct or transitive, with where its
//! repository is, the release branch it was read on and, per
//! `<part>/<label>`, the official commit and the record's content hash.

use super::deps::is_commit;
use super::files::{Obj, header, invalid};
use super::id::{Id, ProjectId};
use crate::error::Result;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The lock's file name, beside the manifest.
pub const LOCK: &str = "ryucad.lock";

pub const KIND: &str = "ryucad.lock";

/// The lock's format.
pub const FORMAT: u64 = 1;

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

    pub fn to_json(&self) -> Value {
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
        json!({"format": FORMAT, "kind": KIND, "projects": projects})
    }

    pub fn from_json(v: &Value) -> Result<Lock> {
        let mut o = header(v, KIND, FORMAT)?;
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
            let label = super::assembly::label_text(label).map_err(|e| e.with("pointer", &here))?;
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
    use crate::error::Code;

    fn lock() -> Lock {
        let (a, b) = (
            ProjectId::from_bytes([7; 16]),
            ProjectId::from_bytes([9; 16]),
        );
        let mut labels = BTreeMap::new();
        labels.insert(
            (Id::parse("m6-bolt", "part").unwrap(), "B".to_owned()),
            LockedLabel {
                commit: "1".repeat(40),
                content: "2".repeat(64),
            },
        );
        let mut projects = BTreeMap::new();
        projects.insert(
            a.clone(),
            Locked {
                url: "https://github.com/acme/fasteners.git".into(),
                path: "".into(),
                release_branch: "main".into(),
                via: vec![],
                labels: labels.clone(),
            },
        );
        projects.insert(
            b,
            Locked {
                url: "file:///srv/lib".into(),
                path: "lib".into(),
                release_branch: "release".into(),
                via: vec![a],
                labels,
            },
        );
        Lock { projects }
    }

    #[test]
    fn a_lock_reads_and_writes_back() {
        let l = lock();
        let v = l.to_json();
        let a = ProjectId::from_bytes([7; 16]);
        assert_eq!(v["kind"], "ryucad.lock");
        assert_eq!(
            v["projects"][a.as_str()]["labels"]["m6-bolt/B"],
            json!({"commit": "1".repeat(40), "content": "2".repeat(64)})
        );
        assert_eq!(Lock::from_json(&v).unwrap(), l);
        let bolt = Id::parse("m6-bolt", "part").unwrap();
        assert_eq!(l.label(&a, &bolt, "B").unwrap().commit, "1".repeat(40));
        assert!(l.label(&a, &bolt, "C").is_none());
    }

    #[test]
    fn a_lock_is_checked() {
        let a = ProjectId::from_bytes([7; 16]).to_string();
        let good = lock().to_json();
        let refused = |change: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            change(&mut v);
            Lock::from_json(&v).unwrap_err()
        };
        let e = refused(&|v| v["projects"] = json!([]));
        assert_eq!(e.details["pointer"], "/projects");
        let e = refused(&|v| v["projects"]["x"] = json!({}));
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidId, &json!("/projects/x"))
        );
        let e = refused(&|v| v["projects"][&a]["labels"]["m6-bolt/B"]["commit"] = json!("HEAD"));
        assert_eq!(
            e.details["pointer"],
            format!("/projects/{a}/labels/m6-bolt~1B/commit")
        );
        let e = refused(&|v| v["projects"][&a]["labels"]["bolt"] = json!({}));
        assert_eq!(e.details["pointer"], format!("/projects/{a}/labels/bolt"));
        let e = refused(&|v| v["projects"][&a]["via"] = json!(["x"]));
        assert_eq!(e.details["pointer"], format!("/projects/{a}/via/0"));
        let e = refused(&|v| v["projects"][&a]["extra"] = json!(1));
        assert_eq!(e.details["pointer"], format!("/projects/{a}/extra"));
        let e = refused(&|v| v["format"] = json!(2));
        assert_eq!(e.code, Code::FormatTooNew);
    }
}
