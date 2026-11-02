//! Recent projects, for the start window. The list is the
//! `app.recent_projects` preference: the daemon puts a project first when
//! a window says hello, and the start window reads it with the daemon's
//! own code (with no project open there is no daemon to ask). Their
//! thumbnails are `core::thumbs`'.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// The recent projects, newest first.
pub fn load() -> Vec<PathBuf> {
    parse(&ryucad::config::user("app.recent_projects"))
}

/// The paths in the preference's value.
pub fn parse(v: &Value) -> Vec<PathBuf> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().map(PathBuf::from))
        .collect()
}

/// What to call each project: its folder's name, and where two different
/// projects share one, as much of their parent folders as tells them
/// apart: `bracket (acceptance)`, `bracket (old)`.
pub fn names(paths: &[PathBuf]) -> Vec<String> {
    // Folder names, innermost first.
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .rev()
            .collect()
    };
    let all: Vec<Vec<String>> = paths.iter().map(|p| parts(p)).collect();
    all.iter()
        .enumerate()
        .map(|(i, mine)| {
            let Some(name) = mine.first() else {
                return paths[i].display().to_string();
            };
            let clash = |n: usize| {
                all.iter().enumerate().any(|(j, other)| {
                    paths[j] != paths[i]
                        && other.len() >= n
                        && other[..n] == mine[..n.min(mine.len())]
                })
            };
            let mut depth = 0;
            while depth + 1 < mine.len() && clash(depth + 1) {
                depth += 1;
            }
            if depth == 0 {
                name.clone()
            } else {
                let parents: Vec<&str> = mine[1..=depth].iter().rev().map(String::as_str).collect();
                format!("{name} ({})", parents.join("/"))
            }
        })
        .collect()
}

#[cfg(test)]
mod names_tests {
    use super::*;

    #[test]
    fn same_named_projects_are_told_apart_by_their_parents() {
        let paths = [
            "/home/u/acceptance/bracket",
            "/home/u/old/bracket",
            "/home/u/lamp",
            "/x/a/b/lamp2",
            "/y/a/b/lamp2",
        ]
        .map(PathBuf::from);
        assert_eq!(
            names(&paths),
            [
                "bracket (acceptance)",
                "bracket (old)",
                "lamp",
                "lamp2 (x/a/b)",
                "lamp2 (y/a/b)"
            ]
        );
        // The same project twice is still one name.
        assert_eq!(
            names(&[PathBuf::from("/a/p"), PathBuf::from("/a/p")]),
            ["p", "p"]
        );
    }
}
