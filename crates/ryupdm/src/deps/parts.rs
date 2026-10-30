//! What another project has released, as fetched: each part with an
//! official release on its release branch, its number, name and kind at
//! its latest release, and its labels latest first. What an agent places
//! from, and what the app's picker lists.

use super::read::{Reader, not_fetched};
use super::{HEAD_REF, config_at};
use crate::label;
use crate::record::{Kind, parse_name};
use ryucad_doc::doc::files::PartKind;
use ryucad_doc::doc::id::{Id, ProjectId};
use ryucad_doc::doc::project::Project;
use ryucad_doc::error::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// One label of a part, as the branch has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Released {
    pub label: String,
    /// `released` (the latest that isn't obsolete), `superseded` or
    /// `obsolete`.
    pub state: &'static str,
    /// Where it became official.
    pub commit: String,
}

/// A part another project has released.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleasedPart {
    pub part: Id,
    /// Its PDM number and name at its latest release, if it has them.
    pub number: Option<String>,
    pub name: Option<String>,
    /// `part` or `assembly`, at its latest release.
    pub kind: Option<&'static str>,
    /// Latest first, in the project's label order.
    pub labels: Vec<Released>,
}

/// Each part's latest official label that isn't obsolete, as `releases`
/// has them, in `scheme`'s order: what an instance is outdated against.
pub fn latest(
    releases: &crate::history::Releases,
    scheme: crate::config::Scheme,
) -> BTreeMap<Id, String> {
    let mut by: BTreeMap<Id, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for p in releases.official.keys() {
        let mut seg = p.split('/').skip(1);
        let (Some(part), Some(file)) = (seg.next(), seg.next()) else {
            continue;
        };
        let (Ok(part), Some((label, kind))) = (Id::parse(part, "part"), parse_name(file)) else {
            continue;
        };
        let e = by.entry(part).or_default();
        match kind {
            Kind::Release => e.0.push(label),
            Kind::Obsolete => e.1.push(label),
        }
    }
    by.into_iter()
        .filter_map(|(part, (labels, obsolete))| {
            let best = labels
                .into_iter()
                .filter(|l| !obsolete.contains(l))
                .max_by_key(|l| (label::index(scheme, l).unwrap_or(0), l.clone()))?;
            Some((part, best))
        })
        .collect()
}

/// The parts `project` (at `url`, folder `path`) has released on its
/// release branch as fetched, in part ID order: `dependency_not_fetched`
/// before a fetch.
pub fn parts(
    reader: &mut Reader,
    project: &ProjectId,
    url: &str,
    path: &str,
) -> Result<(String, Option<String>, Vec<ReleasedPart>)> {
    let Some(repo) = reader.repo(url)? else {
        return Err(not_fetched(project, url, &format!("project {project}")));
    };
    let config = config_at(&repo, HEAD_REF, path)?;
    let branch = config.release_branch.clone();
    let Some(releases) = reader.releases(project, url, path, &branch)?.cloned() else {
        return Ok((branch, None, Vec::new()));
    };
    let tip = releases.branch.as_ref().map(|b| b.tip.clone());
    // Each part's labels, and which are obsolete.
    let mut by: BTreeMap<Id, BTreeMap<String, (Option<String>, bool)>> = BTreeMap::new();
    for (p, o) in &releases.official {
        let mut seg = p.split('/').skip(1);
        let (Some(part), Some(file)) = (seg.next(), seg.next()) else {
            continue;
        };
        let (Ok(part), Some((label, kind))) = (Id::parse(part, "part"), parse_name(file)) else {
            continue;
        };
        let e = by.entry(part).or_default().entry(label).or_default();
        match kind {
            Kind::Release => e.0 = Some(o.commit.clone()),
            Kind::Obsolete => e.1 = true,
        }
    }
    let prefix = super::prefix_of(path);
    let mut out = Vec::new();
    for (part, labels) in by {
        let mut list: Vec<(String, String, bool)> = labels
            .into_iter()
            .filter_map(|(l, (commit, obsolete))| Some((l, commit?, obsolete)))
            .collect();
        list.sort_by_key(|(l, _, _)| {
            std::cmp::Reverse((label::index(config.revisions, l).unwrap_or(0), l.clone()))
        });
        let latest = list.iter().position(|(_, _, obsolete)| !obsolete);
        let labels: Vec<Released> = list
            .iter()
            .enumerate()
            .map(|(i, (l, c, obsolete))| Released {
                label: l.clone(),
                state: if *obsolete {
                    "obsolete"
                } else if Some(i) == latest {
                    "released"
                } else {
                    "superseded"
                },
                commit: c.clone(),
            })
            .collect();
        let Some(first) = labels.first() else {
            continue;
        };
        let files = crate::history::part_files(&repo, &first.commit, &prefix, &part)?;
        let props = crate::props::read(&files, &part).ok();
        let text = |k: &str| {
            props
                .as_ref()
                .and_then(|p| p.values.get(k))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        let kind = Project::load_from(Path::new(""), &files)
            .ok()
            .and_then(|p| p.parts.get(&part)?.file.as_ref().ok().map(|f| f.kind))
            .map(PartKind::as_str);
        out.push(ReleasedPart {
            number: text("number"),
            name: text("name"),
            kind,
            part,
            labels,
        });
    }
    Ok((branch, tip, out))
}

#[cfg(test)]
mod tests {
    use super::super::Cache;
    use super::super::read::tests::{commit, library};
    use super::super::tests::url;
    use super::*;
    use crate::state::tests::{PROJECT, add, id, obsolete, project, release_now};

    #[test]
    fn a_projects_releases_are_listed_latest_first() {
        let up = tempfile::tempdir().unwrap();
        let a = library(up.path(), "lib");
        let cached = tempfile::tempdir().unwrap();
        let cache = Cache::new(cached.path());
        let u = url(up.path());
        let pid = ProjectId::parse(PROJECT).unwrap();
        let e = parts(&mut cache.reader(), &pid, &u, "lib").unwrap_err();
        assert_eq!(e.code, ryucad_doc::error::Code::DependencyNotFetched);
        // B released, A made obsolete.
        let mut f = project();
        f.insert("parts/p/features/a.json".into(), b"{\"x\": 2}\n".to_vec());
        let b = release_now(&f, "B", Some("A"));
        add(&mut f, &b);
        add(&mut f, &release_now(&project(), "A", None));
        add(&mut f, &obsolete("A"));
        let at_b = commit(up.path(), "lib", &f);
        cache.fetch(&u, &["lib"]).unwrap();
        let (branch, tip, list) = parts(&mut cache.reader(), &pid, &u, "lib").unwrap();
        assert_eq!(
            (branch.as_str(), tip.as_deref()),
            ("main", Some(at_b.as_str()))
        );
        assert_eq!(list.len(), 1);
        let p = &list[0];
        assert_eq!(p.part, id("p"));
        assert_eq!(
            (p.number.as_deref(), p.name.as_deref()),
            (Some("100"), Some("P"))
        );
        let releases = cache
            .reader()
            .releases(&pid, &u, "lib", "main")
            .unwrap()
            .cloned()
            .unwrap();
        let newest = latest(&releases, crate::config::Scheme::Letters);
        assert_eq!(newest[&id("p")], "B");
        assert_eq!(
            p.labels,
            [
                Released {
                    label: "B".into(),
                    state: "released",
                    commit: at_b.clone()
                },
                Released {
                    label: "A".into(),
                    state: "obsolete",
                    commit: a
                },
            ]
        );
    }

    /// A fork lists only its own releases: the records it inherited name
    /// the ID it had, and are its upstream's.
    #[test]
    fn a_forks_dependents_see_only_its_own_releases() {
        let up = tempfile::tempdir().unwrap();
        library(up.path(), "");
        let fork = ProjectId::from_bytes([8; 16]);
        let mut f = project();
        f.insert(
            "ryucad.json".into(),
            ryucad_doc::doc::canonical::to_string(&serde_json::json!({
                "default_unit": "mm", "format": 3, "kind": "ryucad.project", "project": fork,
                "forked_from": {"project": PROJECT, "commit": "a".repeat(40)}}))
            .into_bytes(),
        );
        f.insert("parts/p/features/a.json".into(), b"{\"x\": 3}\n".to_vec());
        crate::bom::tests::released(&mut f, &fork, "p", "B");
        commit(up.path(), "", &f);
        let cached = tempfile::tempdir().unwrap();
        let cache = Cache::new(cached.path());
        let u = url(up.path());
        cache.fetch(&u, &[""]).unwrap();
        let labels = |pid: &ProjectId| -> Vec<String> {
            let (_, _, list) = parts(&mut cache.reader(), pid, &u, "").unwrap();
            list.iter()
                .flat_map(|p| p.labels.iter().map(|l| l.label.clone()))
                .collect()
        };
        assert_eq!(labels(&fork), ["B"]);
        assert_eq!(labels(&ProjectId::parse(PROJECT).unwrap()), ["A"]);
    }
}
