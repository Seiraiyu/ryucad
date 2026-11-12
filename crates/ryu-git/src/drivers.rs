//! Registering a ryu product with Git: the project's `.gitattributes`
//! lines (each product's own), the driver entries in Git's config, which
//! say how Git reads and merges its files, `merge.renames=false` in each
//! clone (feature and design files are small and alike, so Git takes a
//! deleted file and an added one for a rename and merges past the driver;
//! never written globally, which every repository reads), and the shared
//! hook (see [`hook_script`]) that refuses a merge commit while any
//! product's merge is unresolved.

use ryu_doc::error::{Code, Error, Result};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// What one product registers with Git.
#[derive(Clone, Copy, Debug)]
pub struct Drivers {
    /// `ryucad`, `ryueda`: in messages and as its line's mark in the hook.
    pub product: &'static str,
    /// The textconv entry, `diff.<product>.textconv`.
    pub textconv: &'static str,
    /// The merge driver's name and command entries
    /// (`merge.<product>.name`, `merge.<product>.driver`), once the
    /// product has a driver.
    pub merge: Option<(&'static str, &'static str)>,
    /// Whether the product's check runs from the shared hook.
    pub hook: bool,
}

/// A product's lines in a project's `.gitattributes`.
#[derive(Clone, Copy, Debug)]
pub struct Attributes {
    /// The comment above them.
    pub header: &'static str,
    pub lines: &'static [&'static str],
    /// Lines an earlier build wrote, which `lines` replace.
    pub old: &'static [&'static str],
}

impl Attributes {
    /// The lines, and `extra`, that `text` doesn't have (each compared
    /// trimmed, so CRLF and spaces don't hide one).
    pub fn missing(&self, text: &str, extra: Option<&'static str>) -> Vec<&'static str> {
        self.lines
            .iter()
            .copied()
            .chain(extra)
            .filter(|l| !text.lines().any(|t| t.trim() == *l))
            .collect()
    }

    /// `text` with every missing line added at its end under the header,
    /// and the old lines taken out; `text` itself when nothing is missing.
    pub fn with_lines(&self, text: &str, extra: Option<&'static str>) -> String {
        let lines = self.missing(text, extra);
        if lines.is_empty() {
            return text.to_owned();
        }
        let kept: Vec<&str> = text
            .lines()
            .filter(|l| !self.old.contains(&l.trim()))
            .collect();
        let mut out = kept.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        if !out.lines().any(|l| l == self.header) {
            out.push_str(self.header);
            out.push('\n');
        }
        for l in lines {
            out.push_str(l);
            out.push('\n');
        }
        out
    }
}

/// The config entry that stops Git's merge following renames, so every
/// file reaches the merge driver as itself.
pub const RENAMES: &str = "merge.renames";

/// The hooks that run the products' checks: before Git records a merge
/// commit itself, and before any commit (one that concludes a merge Git
/// stopped for conflicts runs only this one).
pub const HOOKS: [&str; 2] = ["pre-merge-commit", "pre-commit"];

/// What every hook line that runs a product's check runs.
pub const HOOK_CHECK: &str = "merge status --hook";

/// The repository holding `here`, if any; with `isolated`, reading only
/// its own config (tests: the user's global config, where installers put
/// the drivers, must not decide what they see).
pub fn discover(here: &Path, isolated: bool) -> Option<gix::Repository> {
    if isolated {
        gix::discover_opts(here, Default::default(), gix::open::Options::isolated()).ok()
    } else {
        gix::discover(here).ok()
    }
}

/// How Git, as configured for the repository at `root`, is set up for a
/// product.
pub struct Setup {
    /// The driver entries Git doesn't have, and [`RENAMES`] where it isn't
    /// false.
    pub missing: Vec<&'static str>,
    /// Each of [`HOOKS`] as it stands; empty for a product with no hook
    /// line.
    pub hooks: Vec<(&'static str, Hook)>,
}

/// The repository's setup for `d`, or `None` outside one.
pub fn setup(root: &Path, d: &Drivers, isolated: bool) -> Option<Setup> {
    let repo = discover(root, isolated)?;
    let snapshot = repo.config_snapshot();
    let mut missing: Vec<&str> = std::iter::once(d.textconv)
        .chain(d.merge.map(|(_, driver)| driver))
        .filter(|k| snapshot.string(*k).is_none())
        .collect();
    if snapshot.boolean(RENAMES) != Some(false) {
        missing.push(RENAMES);
    }
    let hooks = if d.hook {
        HOOKS
            .into_iter()
            .map(|h| (h, hook(&repo, h, d.product)))
            .collect()
    } else {
        Vec::new()
    };
    Some(Setup { missing, hooks })
}

/// This binary, quoted for the shell Git runs commands in, with forward
/// slashes even on Windows.
pub fn exe(product: &str) -> Result<String> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::new(Code::Io, format!("finding the {product} binary: {e}")))?;
    let exe = ryu_doc::source::plain(exe.canonicalize().unwrap_or(exe));
    let path = exe
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', r"'\''");
    Ok(format!("'{path}'"))
}

/// The command Git runs for the diff driver.
pub fn textconv_command(product: &str) -> Result<String> {
    Ok(format!("{} git textconv", exe(product)?))
}

/// The command Git runs for the merge driver: base, ours, theirs and the
/// path.
pub fn merge_command(product: &str) -> Result<String> {
    Ok(format!("{} git merge-driver %O %A %B %P", exe(product)?))
}

/// The line that runs this binary's check from someone's own hook.
pub fn hook_line(product: &str) -> Result<String> {
    Ok(format!("{} {HOOK_CHECK} || exit 1", exe(product)?))
}

/// Runs `git config` with `args` in `dir` (any folder for `--global`).
fn git_config(dir: Option<&Path>, args: &[&str]) -> Result<()> {
    let mut git = std::process::Command::new("git");
    if let Some(d) = dir {
        git.current_dir(d);
    }
    let out = git
        .arg("config")
        .args(args)
        .output()
        .map_err(|e| Error::new(Code::Io, format!("running git: {e}; is Git installed?")))?;
    if !out.status.success() {
        return Err(Error::new(
            Code::Io,
            format!(
                "git config failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    Ok(())
}

/// `git install` for `d`: the driver entries in the user's global config,
/// or with `local` in the config of the repository at `here`, with
/// [`RENAMES`] off and, for a product with a hook line, the clone's
/// hooks (someone else's is left alone and reported with the line to add).
pub fn install(here: &Path, local: bool, d: &Drivers, isolated: bool) -> Result<Value> {
    let mut entries = vec![(d.textconv, textconv_command(d.product)?)];
    if let Some((name, driver)) = d.merge {
        entries.push((name, d.product.to_owned()));
        entries.push((driver, merge_command(d.product)?));
    }
    let mut config = Map::new();
    if !local {
        for (k, v) in &entries {
            git_config(None, &["--global", k, v])?;
            config.insert((*k).to_owned(), json!(v));
        }
        return Ok(json!({"scope": "global", "config": config}));
    }
    let repo = discover(here, isolated).ok_or_else(|| {
        Error::new(
            Code::NotAGitRepo,
            format!("{} is not in a Git repository", here.display()),
        )
        .with("path", here)
    })?;
    let dir = repo.workdir().unwrap_or(repo.git_dir()).to_path_buf();
    // Only what differs is written: a clone already set up takes no lock,
    // so projects made side by side in one repository don't collide.
    let snapshot = repo.config_snapshot();
    for (k, v) in &entries {
        if snapshot.string(*k).map(|s| s.to_string()) != Some(v.clone()) {
            git_config(Some(&dir), &["--local", k, v])?;
        }
        config.insert((*k).to_owned(), json!(v));
    }
    if snapshot.boolean(RENAMES) != Some(false) {
        git_config(Some(&dir), &["--local", RENAMES, "false"])?;
    }
    config.insert(RENAMES.to_owned(), json!("false"));
    let hooks = if d.hook {
        install_hooks(&repo, d.product)?
    } else {
        Map::new()
    };
    Ok(json!({"scope": "local", "config": config, "hooks": hooks}))
}

// HOOKS-BEGIN
/// A clone's hook as it stands for one product.
#[derive(Clone, Debug, PartialEq)]
pub enum Hook {
    /// The shared hook with the product's line, or one that runs its check.
    Installed(PathBuf),
    /// None at all, or the shared hook without the product's line.
    Missing(PathBuf),
    /// Someone else's, which doesn't run the product's check: left alone.
    Foreign(PathBuf),
    /// `core.hooksPath` points elsewhere and its hook doesn't run the
    /// check: no product writes into a folder others share or commit.
    HooksPath(PathBuf),
}

fn hook(repo: &gix::Repository, name: &str, _: &str) -> Hook {
    // Task 6 judges what is there.
    Hook::Missing(repo.common_dir().join("hooks").join(name))
}

fn install_hooks(_: &gix::Repository, _: &str) -> Result<Map<String, Value>> {
    unimplemented!("Task 6")
}
// HOOKS-END

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::git;

    const LINES: [&str; 2] = [
        "toy.json merge=toy diff=toy text eol=lf",
        ".gitattributes text eol=lf",
    ];
    const OLD: [&str; 1] = ["toy.json diff=toy"];
    const ATTRS: Attributes = Attributes {
        header: "# toy: Git reads these files through toy",
        lines: &LINES,
        old: &OLD,
    };
    const TEXT_ONLY: Drivers = Drivers {
        product: "toy",
        textconv: "diff.toy.textconv",
        merge: None,
        hook: false,
    };
    const FULL: Drivers = Drivers {
        product: "toy",
        textconv: "diff.toy.textconv",
        merge: Some(("merge.toy.name", "merge.toy.driver")),
        hook: true,
    };

    #[test]
    fn lines_are_added_once_others_kept_and_old_ones_replaced() {
        assert_eq!(ATTRS.missing("", None), LINES);
        let ours = ATTRS.with_lines("", None);
        assert_eq!(ours, format!("{}\n{}\n", ATTRS.header, LINES.join("\n")));
        assert_eq!(ATTRS.with_lines(&ours, None), ours);
        let old = format!("*.png binary\n{}\ntoy.json diff=toy\n", ATTRS.header);
        assert_eq!(
            ATTRS.with_lines(&old, None),
            format!("*.png binary\n{}\n{}\n", ATTRS.header, LINES.join("\n"))
        );
        assert_eq!(ATTRS.missing(&ours, Some("x/** binary")), ["x/** binary"]);
    }

    #[test]
    fn setup_and_install_with_and_without_a_merge_driver() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        git(r, &["init", "-q"]);
        assert_eq!(
            setup(r, &TEXT_ONLY, true).unwrap().missing,
            ["diff.toy.textconv", RENAMES]
        );
        assert!(setup(r, &TEXT_ONLY, true).unwrap().hooks.is_empty());
        let out = install(r, true, &TEXT_ONLY, true).unwrap();
        assert_eq!(out["scope"], "local");
        assert_eq!(out["hooks"], serde_json::json!({}));
        assert!(
            out["config"]["diff.toy.textconv"]
                .as_str()
                .unwrap()
                .ends_with("' git textconv")
        );
        assert!(setup(r, &TEXT_ONLY, true).unwrap().missing.is_empty());
        assert_eq!(setup(r, &FULL, true).unwrap().missing, ["merge.toy.driver"]);
        assert!(
            std::fs::read_to_string(r.join(".git/config"))
                .unwrap()
                .contains("renames = false")
        );
    }
}
