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

/// Each release line's workflow and the tags it must take.
const LINES: [(&str, &[&str]); 3] = [
    (
        "release.yml",
        &["v0.7.0", "v0.7.0-pre.2", "v1.0.0", "v12.34.56-rc.1"],
    ),
    (
        "ryueda-release.yml",
        &["ryueda-v0.1.0", "ryueda-v0.1.0-pre.1", "ryueda-v2.10.3"],
    ),
    (
        "ryudfm-release.yml",
        &["ryudfm-v0.1.0", "ryudfm-v0.1.0-pre.1", "ryudfm-v2.10.3"],
    ),
];

#[test]
fn each_release_workflow_takes_only_its_own_tags() {
    let fires = |patterns: &[String], tag: &str| patterns.iter().any(|p| matches(p, tag));
    let patterns: Vec<Vec<String>> = LINES
        .iter()
        .map(|(file, _)| tag_patterns(&workflow(file)))
        .collect();
    for (i, (file, tags)) in LINES.iter().enumerate() {
        for tag in *tags {
            assert!(fires(&patterns[i], tag), "{file} misses {tag}");
            for (j, (other, _)) in LINES.iter().enumerate() {
                if j != i {
                    assert!(!fires(&patterns[j], tag), "{other} takes {tag}");
                }
            }
        }
    }
    for tag in [
        "ryupdm/bracket/A",
        "ryueda-0.1.0",
        "ryudfm-0.1.0",
        "ryucad-v0.1.0",
        "v0.1",
    ] {
        for (j, (file, _)) in LINES.iter().enumerate() {
            assert!(!fires(&patterns[j], tag), "{file} takes {tag}");
        }
    }
}

#[test]
fn other_lines_leave_ryucads_latest_alone() {
    // install.sh and install.ps1 fetch releases/latest: a ryueda or ryudfm
    // release marked latest, or carrying ryucad's installers, would break
    // them.
    for file in ["ryueda-release.yml", "ryudfm-release.yml"] {
        let text = workflow(file);
        assert!(text.contains("--latest=false"), "{file} may take latest");
        for word in ["install.sh", "install.ps1"] {
            assert!(
                !text
                    .lines()
                    .filter(|l| !l.trim_start().starts_with('#'))
                    .flat_map(str::split_whitespace)
                    .any(|w| w == word),
                "{file}'s release carries {word}"
            );
        }
    }
}

/// Runs `scripts/latest-release.sh TAG` with `released` on stdin: what it
/// prints, `true` or `false`.
#[cfg(unix)]
fn latest(tag: &str, released: &str) -> String {
    use std::io::Write;
    let mut c = std::process::Command::new("sh")
        .arg(root().join("scripts/latest-release.sh"))
        .arg(tag)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin
        .take()
        .unwrap()
        .write_all(released.as_bytes())
        .unwrap();
    let out = c.wait_with_output().unwrap();
    assert!(out.status.success(), "latest-release.sh {tag} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// Design 312: a v0.7.x patch published after v0.8.0 never becomes GitHub's
/// latest release, which `install.sh` and `install.ps1` fetch.
#[cfg(unix)]
#[test]
fn a_final_tag_is_latest_only_when_it_is_the_highest() {
    let released = "v0.7.2\nv0.7.10\nv0.8.0\nv0.8.1-pre.1\nryueda-v0.2.0\n";
    assert_eq!(latest("v0.8.1", released), "true");
    assert_eq!(latest("v0.8.0", released), "true"); // itself, already listed
    assert_eq!(latest("v0.7.11", released), "false");
    assert_eq!(latest("v0.9.0-pre.1", released), "false"); // a pre-release never is
    assert_eq!(latest("v0.10.0", "v0.9.9\n"), "true"); // numbers, not text
    assert_eq!(latest("v0.9.9", "v0.10.0\n"), "false");
    assert_eq!(latest("v1.0.0", ""), "true");
}

#[test]
fn ryucads_release_asks_whether_a_tag_is_latest() {
    let w = workflow("release.yml");
    assert!(w.contains("sh scripts/latest-release.sh \"$GITHUB_REF_NAME\""));
    assert!(w.contains("--latest=\"$latest\""));
}

/// The devcontainer's .debs are cached from main, whose caches every tag's
/// run can restore, under the path and key full.yml restores: a tag's own
/// save serves only that tag (#677).
#[test]
fn the_devcontainers_debs_are_cached_from_main() {
    let full = workflow("full.yml");
    let debs = workflow("debs.yml");
    let key = "key: devcontainer-debs-${{ hashFiles('.devcontainer/debs.txt') }}";
    let path = "path: .devcontainer/debs/*.deb";
    assert!(
        full.contains(key) && full.contains(path),
        "full.yml no longer restores the .debs under {key}"
    );
    assert!(
        debs.contains(key) && debs.contains(path),
        "debs.yml saves another path or key than full.yml restores"
    );
    assert!(
        debs.contains("uses: actions/cache/save@"),
        "debs.yml saves no cache"
    );
    assert!(
        debs.contains("branches: [main]") && debs.contains("workflow_dispatch:"),
        "debs.yml must run on main, by push and by hand"
    );
    assert!(
        debs.contains("runs-on: ubuntu-latest"),
        "a cache's version includes its compression: save it on full.yml's runner"
    );
}

/// ryueda's archives carry the app (plan 3.D): built for the app's
/// target, staged beside the CLI with the fonts' licences and, on Linux
/// and macOS, share/; its smoke frame must draw; and the installers'
/// job finds it.
#[test]
fn ryuedas_release_carries_the_app() {
    let text = std::fs::read_to_string(root().join(".github/workflows/ryueda-release.yml")).unwrap();
    for want in [
        "app: x86_64-unknown-linux-gnu",
        "app: aarch64-unknown-linux-gnu",
        "cargo build --locked --release -p ryueda-app --target \"$APP\"",
        "\"target/$APP/release/ryueda-app$x\"",
        "crates/ryu-app/fonts/LICENSES.md",
        "packaging/linux/ryueda.desktop packaging/linux/ryueda.xml",
        "packaging/macos/ryueda.applescript",
        "RYUEDA_REQUIRE_GPU: \"1\"",
        "mesa-vulkan-drivers",
        "ryueda-app --version",
    ] {
        assert!(text.contains(want), "ryueda-release.yml lacks {want}");
    }
    for file in ["packaging/linux/ryueda.desktop", "packaging/linux/ryueda.xml", "packaging/macos/ryueda.applescript"] {
        assert!(root().join(file).is_file(), "{file}");
    }
}
