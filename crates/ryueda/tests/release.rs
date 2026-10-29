//! The repository's release lines, ryucad's, ryueda's and ryudfm's: each
//! release workflow is triggered by its own tags only, ryueda's and
//! ryudfm's releases never take ryucad's "latest", and
//! `scripts/stamp-version.sh` reads every line's tags.

use std::path::{Path, PathBuf};

/// The repository's root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs `scripts/stamp-version.sh ARG` on a copy of the workspace's
/// manifests: its exit code, what it printed, and the root manifest after.
#[cfg(unix)]
fn stamp(arg: &str) -> (i32, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let root = root();
    std::fs::copy(root.join("Cargo.toml"), tmp.path().join("Cargo.toml")).unwrap();
    std::fs::copy(root.join("Cargo.lock"), tmp.path().join("Cargo.lock")).unwrap();
    for crate_dir in std::fs::read_dir(root.join("crates")).unwrap() {
        let crate_dir = crate_dir.unwrap().path();
        let manifest = crate_dir.join("Cargo.toml");
        if manifest.is_file() {
            let to = tmp
                .path()
                .join("crates")
                .join(crate_dir.file_name().unwrap());
            std::fs::create_dir_all(&to).unwrap();
            std::fs::copy(&manifest, to.join("Cargo.toml")).unwrap();
        }
    }
    let out = std::process::Command::new("sh")
        .arg(root.join("scripts/stamp-version.sh"))
        .arg(arg)
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let manifest = std::fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
        manifest,
    )
}

#[cfg(unix)]
#[test]
fn stamp_version_reads_every_lines_tags() {
    for (arg, version) in [
        ("1.2.3", "1.2.3"),
        ("v1.2.3", "1.2.3"),
        ("ryueda-v1.2.3", "1.2.3"),
        ("ryueda-v0.1.0-pre.4", "0.1.0-pre.4"),
        ("ryudfm-v1.2.3", "1.2.3"),
        ("ryudfm-v0.1.0-pre.2", "0.1.0-pre.2"),
    ] {
        let (code, printed, manifest) = stamp(arg);
        assert_eq!(code, 0, "{arg}: {printed}");
        assert!(
            printed.contains(&format!("stamped {version}")),
            "{arg}: {printed}"
        );
        assert!(
            manifest.contains(&format!("\nversion = \"{version}\"")),
            "{arg}: the workspace isn't stamped"
        );
    }
    // Another product's tag is no version.
    for arg in ["ryucad-v1.2.3", "ryueda-1.2.3", "ryudfm-1.2.3", "vx"] {
        let (code, printed, _) = stamp(arg);
        assert_eq!(code, 2, "{arg}: {printed}");
        assert!(printed.contains("not a version"), "{arg}: {printed}");
    }
}

/// A workflow file under `.github/workflows`.
fn workflow(name: &str) -> String {
    std::fs::read_to_string(root().join(".github/workflows").join(name)).unwrap()
}

/// The tag patterns a workflow's `push` trigger lists: its `tags: [...]`.
fn tag_patterns(workflow: &str) -> Vec<String> {
    let line = workflow
        .lines()
        .find(|l| l.trim_start().starts_with("tags:"))
        .expect("a tags trigger");
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// One piece of a filter pattern and how many times it repeats.
enum Atom {
    Char(char),
    Class(Vec<(char, char)>),
    /// `*`: anything but `/`.
    Star,
}

impl Atom {
    fn takes(&self, c: char) -> bool {
        match self {
            Atom::Char(x) => *x == c,
            Atom::Class(ranges) => ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi),
            Atom::Star => c != '/',
        }
    }
}

/// Whether GitHub's filter pattern matches `text`, as far as the release
/// triggers use its syntax: `*`, classes such as `[0-9]`, `+` (one or more
/// of what precedes it) and `?` (zero or one); anything else is literal.
fn matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let mut pieces = Vec::new();
    let mut i = 0;
    while i < p.len() {
        let atom = match p[i] {
            '*' => {
                i += 1;
                Atom::Star
            }
            '[' => {
                let end = i + p[i..].iter().position(|&c| c == ']').expect("a class ends");
                let body = &p[i + 1..end];
                let mut ranges = Vec::new();
                let mut j = 0;
                while j < body.len() {
                    if j + 2 < body.len() && body[j + 1] == '-' {
                        ranges.push((body[j], body[j + 2]));
                        j += 3;
                    } else {
                        ranges.push((body[j], body[j]));
                        j += 1;
                    }
                }
                i = end + 1;
                Atom::Class(ranges)
            }
            c => {
                i += 1;
                Atom::Char(c)
            }
        };
        let (min, max) = match p.get(i) {
            Some('+') => {
                i += 1;
                (1, usize::MAX)
            }
            Some('?') => {
                i += 1;
                (0, 1)
            }
            _ if matches!(atom, Atom::Star) => (0, usize::MAX),
            _ => (1, 1),
        };
        pieces.push((atom, min, max));
    }
    fn go(pieces: &[(Atom, usize, usize)], text: &[char]) -> bool {
        let Some(((atom, min, max), rest)) = pieces.split_first() else {
            return text.is_empty();
        };
        let mut n = 0;
        loop {
            if n >= *min && go(rest, &text[n..]) {
                return true;
            }
            if n == *max || n == text.len() || !atom.takes(text[n]) {
                return false;
            }
            n += 1;
        }
    }
    go(&pieces, &text.chars().collect::<Vec<_>>())
}

#[test]
fn filter_patterns_match_as_github_reads_them() {
    assert!(matches("v[0-9]+.[0-9]+.[0-9]+*", "v0.7.0"));
    assert!(matches("v[0-9]+.[0-9]+.[0-9]+*", "v10.2.33-pre.1"));
    assert!(!matches("v[0-9]+.[0-9]+.[0-9]+*", "v0.7"));
    assert!(!matches("v[0-9]+.[0-9]+.[0-9]+*", "xv0.7.0"));
    assert!(!matches("v[0-9]+.[0-9]+.[0-9]+*", "v0.7.0/x"));
    assert!(matches("a?b", "b") && matches("a?b", "ab") && !matches("a?b", "aab"));
}

#[test]
fn each_release_workflow_takes_only_its_own_tags() {
    let ryucad = tag_patterns(&workflow("release.yml"));
    let ryueda = tag_patterns(&workflow("ryueda-release.yml"));
    let fires = |patterns: &[String], tag: &str| patterns.iter().any(|p| matches(p, tag));
    for tag in ["v0.7.0", "v0.7.0-pre.2", "v1.0.0", "v12.34.56-rc.1"] {
        assert!(fires(&ryucad, tag), "release.yml misses {tag}");
        assert!(!fires(&ryueda, tag), "ryueda-release.yml takes {tag}");
    }
    for tag in ["ryueda-v0.1.0", "ryueda-v0.1.0-pre.1", "ryueda-v2.10.3"] {
        assert!(fires(&ryueda, tag), "ryueda-release.yml misses {tag}");
        assert!(!fires(&ryucad, tag), "release.yml takes {tag}");
    }
    for tag in ["ryupdm/bracket/A", "ryueda-0.1.0", "ryucad-v0.1.0", "v0.1"] {
        assert!(!fires(&ryucad, tag), "release.yml takes {tag}");
        assert!(!fires(&ryueda, tag), "ryueda-release.yml takes {tag}");
    }
}

#[test]
fn ryueda_releases_leave_ryucads_latest_alone() {
    // install.sh and install.ps1 fetch releases/latest: a ryueda release
    // marked latest, or carrying ryucad's installers, would break them.
    let ryueda = workflow("ryueda-release.yml");
    assert!(ryueda.contains("--latest=false"));
    for word in ["install.sh", "install.ps1"] {
        assert!(
            !ryueda
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .flat_map(str::split_whitespace)
                .any(|w| w == word),
            "ryueda's release carries {word}"
        );
    }
}
