//! Git run in a folder its caller names, never in a repository the
//! environment names (#721): `git bisect run`, hooks and worktrees export
//! `GIT_DIR` and its kin, and a `git init` under them rewrites that
//! repository's config.

use std::ffi::OsString;
use std::process::Command;

/// The variables that point Git at a repository other than the one its
/// working folder is in.
pub const LOCATION: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// `git` finding its repository from its working folder alone: for code
/// that runs Git in a folder it was given.
pub fn here() -> Command {
    let mut git = Command::new("git");
    for v in LOCATION {
        git.env_remove(v);
    }
    git
}

/// Every `GIT_` variable this process has, and [`LOCATION`]: what a test
/// removes from any command it spawns (git or a ryu binary), then sets
/// what it needs.
pub fn inherited() -> Vec<OsString> {
    let mut out: Vec<OsString> = LOCATION.iter().map(OsString::from).collect();
    for (k, _) in std::env::vars_os() {
        if k.to_str()
            .is_some_and(|s| s.to_ascii_uppercase().starts_with("GIT_"))
        {
            out.push(k);
        }
    }
    out
}

/// `git` for a test: no `GIT_` variable inherited (location, config,
/// identity, dates); the caller sets what it needs after.
pub fn isolated() -> Command {
    let mut git = Command::new("git");
    for v in inherited() {
        git.env_remove(v);
    }
    git
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_git_dir_set_on_the_command_is_removed() {
        let (decoy, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut git = super::here();
        // `here` removed it; a GIT_DIR set before that would be gone too.
        git.env("GIT_DIR", decoy.path());
        for v in super::LOCATION {
            git.env_remove(v);
        }
        let st = git
            .args(["init", "-q"])
            .current_dir(work.path())
            .status()
            .unwrap();
        assert!(st.success());
        assert!(work.path().join(".git").is_dir());
        assert!(std::fs::read_dir(decoy.path()).unwrap().next().is_none());
    }
}
