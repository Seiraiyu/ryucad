//! A product of this crate's own for its tests: `toy.json` projects with
//! one folder, `things/`, and a Git helper reading no user's config.

use crate::Layout;
use std::path::Path;
use std::process::Command;

pub struct Toy;

impl Layout for Toy {
    const NAME: &'static str = "toy";
    const MANIFEST: &'static str = "toy.json";
    const ROOT_FILES: &'static [&'static str] = &["toy.json", ".gitattributes"];
    const FOLDERS: &'static [&'static str] = &["things"];
    fn project_file(rel: &str) -> bool {
        Self::ROOT_FILES.contains(&rel)
            || rel.strip_prefix("things/").is_some_and(|f| !f.contains('/'))
    }
}

/// `git`, reading neither the user's config nor the system's.
pub fn git_command() -> Command {
    let mut git = Command::new("git");
    git.env(
        "GIT_CONFIG_GLOBAL",
        std::env::temp_dir().join("ryu-git-no-gitconfig"),
    )
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_AUTHOR_NAME", "Test")
    .env("GIT_AUTHOR_EMAIL", "t@example.com")
    .env("GIT_COMMITTER_NAME", "Test")
    .env("GIT_COMMITTER_EMAIL", "t@example.com")
    .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
    .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z");
    git
}

/// Runs git in `dir` with a fixed identity and dates; panics on failure.
pub fn git(dir: &Path, args: &[&str]) {
    let out = git_command()
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Writes `text` at `rel` under `root`, making folders.
pub fn write(root: &Path, rel: &str, text: &str) {
    let p = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}
