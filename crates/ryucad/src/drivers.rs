//! Registering ryucad with Git, so that `git diff` reads feature files as
//! values and `git merge` merges them value by value: the project's
//! `.gitattributes`, which says which files Git hands to ryucad; the driver
//! entries in Git's config, which say how; and, per clone, the hooks that
//! refuse a merge commit while a project's merge is unresolved:
//! `pre-merge-commit`, which Git runs before committing a merge itself, and
//! `pre-commit`, which it runs when someone commits a merge it stopped
//! (and a cherry-pick or revert it stopped), and where the check costs
//! nothing outside a merge. The attributes are a project file, committed;
//! the config and the hooks are each clone's or user's, written by the
//! installers and `ryucad git install`. These are, with nothing else, the
//! Git config ryucad writes.
//!
//! Git's merge follows renames by default, and feature files are small and
//! alike, so a feature deleted on one side and another added there looks
//! like a rename: the merge driver never sees the real three-way merge, an
//! edit is lost and an ID collision goes unseen. Each clone therefore gets
//! `merge.renames=false` (Git 2.47 ignored it; 2.50 honours it), never the
//! user's global config, which every other repository reads too; `merge
//! status` still reports a merge that followed a rename.

use crate::error::{Code, Error, Result};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The project's attributes file, beside its manifest; its patterns are
/// relative to the project folder.
pub use crate::doc::project::ATTRIBUTES;

/// The lines ryucad needs in it: its own files (the material library
/// among them) and ryupdm's, and the
/// attributes file itself, which ryucad writes too. `eol=lf` keeps them
/// canonical in every checkout: Git for Windows would otherwise check them
/// out with CRLF.
pub const LINES: [&str; 7] = [
    "ryucad.json merge=ryucad diff=ryucad text eol=lf",
    "ryupdm.json merge=ryucad diff=ryucad text eol=lf",
    "materials.json merge=ryucad diff=ryucad text eol=lf",
    "ryucad.lock merge=ryucad diff=ryucad text eol=lf",
    "parts/**/*.json merge=ryucad diff=ryucad text eol=lf",
    "releases/**/*.json merge=ryucad diff=ryucad text eol=lf",
    ".gitattributes text eol=lf",
];

/// Lines an earlier ryucad wrote, which [`LINES`] replace.
const OLD_LINES: [&str; 6] = [
    "ryucad.json diff=ryucad",
    "parts/**/*.json diff=ryucad",
    "ryucad.json merge=ryucad diff=ryucad",
    "ryupdm.json merge=ryucad diff=ryucad",
    "parts/**/*.json merge=ryucad diff=ryucad",
    "releases/**/*.json merge=ryucad diff=ryucad",
];

/// The comment that heads ryucad's lines.
const HEADER: &str = "# ryucad: Git reads these files through ryucad (`ryucad git install`)";

/// The config entry Git's diff runs.
pub const TEXTCONV: &str = "diff.ryucad.textconv";

/// The config entry naming the merge driver, for Git's messages.
pub const MERGE_NAME: &str = "merge.ryucad.name";

/// The config entry Git's merge runs.
pub const MERGE_DRIVER: &str = "merge.ryucad.driver";

/// The config entry that stops Git's merge following renames, so every
/// feature file reaches the merge driver as itself.
pub const RENAMES: &str = "merge.renames";

/// The hooks that run ryucad's merge check: before Git records a merge
/// commit itself, and before any commit (one that concludes a merge Git
/// stopped for conflicts runs only this one).
pub const HOOKS: [&str; 2] = ["pre-merge-commit", "pre-commit"];

/// The line that marks a hook as ryucad's.
const HOOK_HEADER: &str = "# ryucad: refuse a merge commit while a ryucad project's merge is unresolved (`ryucad git install`)";

/// The line a project needs once it imports STEP files: Git must keep
/// their bytes exactly (no line endings converted, no line merges, no text
/// diffs), since import features name them by their hash.
pub const IMPORT_LINE: &str = "imports/** binary";

/// The lines of [`LINES`] that `text` doesn't have.
pub fn missing(text: &str) -> Vec<&'static str> {
    missing_for(text, false)
}

/// The lines of [`LINES`], and with `imports` [`IMPORT_LINE`], that
/// `text` doesn't have.
pub fn missing_for(text: &str, imports: bool) -> Vec<&'static str> {
    LINES
        .into_iter()
        .chain(imports.then_some(IMPORT_LINE))
        .filter(|l| !text.lines().any(|t| t.trim() == *l))
        .collect()
}

/// Whether the project at `root` holds a file under `imports/`.
pub fn has_imports(root: &Path) -> bool {
    std::fs::read_dir(root.join(crate::doc::project::IMPORTS))
        .is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.path().is_file())))
}

/// `text` (the attributes file as it is, or empty) with every missing line
/// added at its end, and the lines an earlier ryucad wrote taken out.
pub fn with_lines(text: &str) -> String {
    with_lines_for(text, false)
}

/// [`with_lines`], with [`IMPORT_LINE`] too when `imports`.
pub fn with_lines_for(text: &str, imports: bool) -> String {
    let lines = missing_for(text, imports);
    if lines.is_empty() {
        return text.to_owned();
    }
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !OLD_LINES.contains(&l.trim()))
        .collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    if !out.lines().any(|l| l == HEADER) {
        out.push_str(HEADER);
        out.push('\n');
    }
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Writes the project's attributes file with ryucad's lines (and the
/// imports' when it has any), keeping whatever else it holds. Writes
/// nothing when they are all there.
pub fn write_attributes(root: &Path) -> Result<()> {
    let path = root.join(ATTRIBUTES);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(&path, &e)),
    };
    let new = with_lines_for(&text, has_imports(root));
    if new != text {
        crate::doc::project::write_atomic(&path, new)?;
    }
    Ok(())
}

/// The repository holding `here`, if any.
#[cfg(not(test))]
fn discover(here: &Path) -> Option<gix::Repository> {
    gix::discover(here).ok()
}

/// The repository holding `here`, if any, reading only its own config:
/// the user's global config (where the installers put the drivers) and
/// the system's must not decide what tests see.
#[cfg(test)]
fn discover(here: &Path) -> Option<gix::Repository> {
    gix::discover_opts(here, Default::default(), gix::open::Options::isolated()).ok()
}

/// How Git, as configured for the repository at `root`, is set up for
/// ryucad; `None` outside a repository.
pub struct Setup {
    /// The driver entries Git doesn't have, and [`RENAMES`] where it isn't
    /// false.
    pub missing: Vec<&'static str>,
    /// Each of [`HOOKS`] as it stands.
    pub hooks: Vec<(&'static str, Hook)>,
}

/// The `pre-merge-commit` hook of a clone.
#[derive(Clone, Debug, PartialEq)]
pub enum Hook {
    /// ryucad's, or one that runs ryucad's check.
    Installed(PathBuf),
    /// None at all.
    Missing(PathBuf),
    /// Someone else's, which doesn't run ryucad's check: it is left alone.
    Foreign(PathBuf),
    /// `core.hooksPath` points elsewhere and has no hook that runs the
    /// check: ryucad doesn't write into a folder others share or commit.
    HooksPath(PathBuf),
}

/// Where Git looks for the clone's hooks, and whether that is
/// `core.hooksPath` rather than the repository's own folder.
fn hooks_dir(repo: &gix::Repository) -> (PathBuf, bool) {
    let snapshot = repo.config_snapshot();
    match snapshot.string("core.hooksPath") {
        Some(p) => {
            let p = PathBuf::from(p.to_string());
            let base = repo.workdir().unwrap_or(repo.git_dir());
            (if p.is_absolute() { p } else { base.join(p) }, true)
        }
        None => (repo.common_dir().join("hooks"), false),
    }
}

/// The hook `name` of the repository `repo`.
fn hook(repo: &gix::Repository, name: &str) -> Hook {
    let (dir, shared) = hooks_dir(repo);
    let path = dir.join(name);
    match std::fs::read_to_string(&path) {
        Ok(text) if text.lines().any(|l| l == HOOK_HEADER) || text.contains(HOOK_CHECK) => {
            Hook::Installed(path)
        }
        Ok(_) if shared => Hook::HooksPath(path),
        Ok(_) => Hook::Foreign(path),
        Err(_) if shared => Hook::HooksPath(path),
        Err(_) => Hook::Missing(path),
    }
}

/// What every hook that runs ryucad's check runs.
const HOOK_CHECK: &str = "merge status --hook";

/// The repository's setup for ryucad, or `None` outside one.
pub fn setup(root: &Path) -> Option<Setup> {
    let repo = discover(root)?;
    let snapshot = repo.config_snapshot();
    let mut missing: Vec<&str> = [TEXTCONV, MERGE_DRIVER]
        .into_iter()
        .filter(|k| snapshot.string(*k).is_none())
        .collect();
    if snapshot.boolean(RENAMES) != Some(false) {
        missing.push(RENAMES);
    }
    let hooks = HOOKS.into_iter().map(|h| (h, hook(&repo, h))).collect();
    Some(Setup { missing, hooks })
}

/// This binary, quoted for the shell Git runs commands in, with forward
/// slashes even on Windows.
fn exe() -> Result<String> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::new(Code::Io, format!("finding the ryucad binary: {e}")))?;
    let exe = crate::doc::project::plain(exe.canonicalize().unwrap_or(exe));
    let path = exe
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', r"'\''");
    Ok(format!("'{path}'"))
}

/// The command Git runs for the diff driver.
pub fn textconv_command() -> Result<String> {
    Ok(format!("{} git textconv", exe()?))
}

/// The command Git runs for the merge driver: base, ours, theirs and the
/// path.
pub fn merge_command() -> Result<String> {
    Ok(format!("{} git merge-driver %O %A %B %P", exe()?))
}

/// The line that runs ryucad's check from a hook.
pub fn hook_line() -> Result<String> {
    Ok(format!("{} {HOOK_CHECK} || exit 1", exe()?))
}

/// ryucad's hook: one check for every project in the repository.
fn hook_script() -> Result<String> {
    Ok(format!(
        "#!/bin/sh\n{HOOK_HEADER}\n{}\n",
        exe().map(|e| format!("exec {e} {HOOK_CHECK}"))?
    ))
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

/// Writes ryucad's hook at `path`, executable.
fn write_hook(path: &Path) -> Result<()> {
    crate::doc::project::write_atomic(path, hook_script()?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| Error::io(path, &e))?;
    }
    Ok(())
}

/// `ryucad git install`: writes the driver entries to the user's global
/// Git config, or with `local` to the config of the repository at `here`,
/// with [`RENAMES`] off and the clone's own hooks (except where someone
/// else's is there, which is left alone and reported with the line to
/// add).
pub fn install(here: &Path, local: bool) -> Result<Value> {
    let entries = [
        (TEXTCONV, textconv_command()?),
        (MERGE_NAME, "ryucad".to_owned()),
        (MERGE_DRIVER, merge_command()?),
    ];
    let mut config = Map::new();
    if !local {
        for (k, v) in &entries {
            git_config(None, &["--global", k, v])?;
            config.insert((*k).to_owned(), json!(v));
        }
        return Ok(json!({"scope": "global", "config": config}));
    }
    let repo = discover(here).ok_or_else(|| {
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
    let mut hooks = Map::new();
    for name in HOOKS {
        let v = match hook(&repo, name) {
            Hook::Installed(p) | Hook::Missing(p) => {
                // Only ours is rewritten: someone's own hook that runs the
                // check (the line `foreign` asks for) is left as it is.
                let text = std::fs::read_to_string(&p).ok();
                let ours = text
                    .as_deref()
                    .is_none_or(|t| t.lines().any(|l| l == HOOK_HEADER));
                if ours && text != Some(hook_script()?) {
                    write_hook(&p)?;
                }
                json!({"path": p, "status": "installed"})
            }
            Hook::Foreign(p) => json!({"path": p, "status": "foreign", "line": hook_line()?}),
            Hook::HooksPath(p) => {
                json!({"path": p, "status": "hooks_path", "line": hook_line()?})
            }
        };
        hooks.insert(name.to_owned(), v);
    }
    Ok(json!({"scope": "local", "config": config, "hooks": hooks}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_added_once_and_others_kept() {
        assert_eq!(missing(""), LINES);
        let ours = with_lines("");
        assert!(missing(&ours).is_empty());
        assert_eq!(with_lines(&ours), ours);
        let theirs = "*.png binary";
        let both = with_lines(theirs);
        assert!(both.starts_with("*.png binary\n# ryucad"), "{both}");
        assert_eq!(both.lines().count(), 9);
        // One line already there, written by hand with spaces.
        let one = with_lines("  ryucad.json merge=ryucad diff=ryucad text eol=lf  \n");
        assert_eq!(missing(&one), Vec::<&str>::new());
        assert_eq!(one.matches("ryucad.json").count(), 1);
        // A project that imports also keeps its STEP files' bytes.
        assert_eq!(missing_for(&ours, true), [IMPORT_LINE]);
        let with = with_lines_for(&ours, true);
        assert!(with.ends_with("imports/** binary\n"), "{with}");
        assert!(missing_for(&with, true).is_empty());
        assert_eq!(with_lines_for(&with, true), with);
    }

    #[test]
    fn imported_files_are_written_down_once_there_are_any() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_attributes(root).unwrap();
        let text = std::fs::read_to_string(root.join(ATTRIBUTES)).unwrap();
        assert!(!text.contains(IMPORT_LINE));
        assert!(!has_imports(root));
        std::fs::create_dir_all(root.join("imports")).unwrap();
        assert!(!has_imports(root));
        std::fs::write(root.join("imports/a.step"), "x").unwrap();
        assert!(has_imports(root));
        write_attributes(root).unwrap();
        let text = std::fs::read_to_string(root.join(ATTRIBUTES)).unwrap();
        assert_eq!(text.matches(IMPORT_LINE).count(), 1);
    }

    #[test]
    fn checkouts_keep_canonical_files_lf() {
        // Git for Windows checks text out with CRLF by default; canonical
        // files must come out as they went in.
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let git = |args: &[&str]| crate::git::tests::git(r, args);
        git(&["init", "-q"]);
        git(&["config", "core.autocrlf", "true"]);
        write_attributes(r).unwrap();
        std::fs::create_dir_all(r.join("parts/p/features")).unwrap();
        let text = "{\n  \"kind\": \"ryucad.feature\"\n}\n";
        for f in [
            "ryucad.json",
            "parts/p/part.json",
            "parts/p/features/a.json",
        ] {
            std::fs::write(r.join(f), text).unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-q", "--no-verify", "-m", "one"]);
        for f in [
            "ryucad.json",
            "parts/p/part.json",
            "parts/p/features/a.json",
        ] {
            std::fs::remove_file(r.join(f)).unwrap();
        }
        git(&["checkout", "--", "."]);
        for f in [
            "ryucad.json",
            "parts/p/part.json",
            "parts/p/features/a.json",
        ] {
            assert_eq!(std::fs::read_to_string(r.join(f)).unwrap(), text, "{f}");
        }
        // Issue #101: the attributes file itself too, or every `git add`
        // after ryucad writes it warns that LF will become CRLF.
        let attributes = std::fs::read_to_string(r.join(ATTRIBUTES)).unwrap();
        std::fs::remove_file(r.join(ATTRIBUTES)).unwrap();
        git(&["checkout", "--", "."]);
        let back = std::fs::read_to_string(r.join(ATTRIBUTES)).unwrap();
        assert_eq!(back, attributes);
        assert!(!back.contains('\r'));
    }

    #[test]
    fn a_file_without_its_own_line_gets_it_in_lf() {
        // Issue #101: what an earlier ryucad wrote, checked out with CRLF.
        let before: String = LINES[..6].iter().map(|l| format!("{l}\r\n")).collect();
        let before = format!("*.png binary\r\n{HEADER}\r\n{before}");
        assert_eq!(missing(&before), [".gitattributes text eol=lf"]);
        let after = with_lines(&before);
        assert_eq!(
            after,
            format!("*.png binary\n{HEADER}\n{}\n", LINES.join("\n"))
        );
    }

    #[test]
    fn an_earlier_ryucads_lines_are_replaced() {
        let old = format!(
            "*.png binary\n{HEADER}\nryucad.json diff=ryucad\nparts/**/*.json diff=ryucad\n"
        );
        let new = with_lines(&old);
        assert_eq!(
            new,
            format!("*.png binary\n{HEADER}\n{}\n", LINES.join("\n"))
        );
        assert!(missing(&new).is_empty());
    }

    #[test]
    fn commands_are_quoted_with_forward_slashes() {
        for (c, tail) in [
            (textconv_command().unwrap(), "' git textconv"),
            (merge_command().unwrap(), "' git merge-driver %O %A %B %P"),
            (hook_line().unwrap(), "' merge status --hook || exit 1"),
        ] {
            assert!(c.starts_with('\'') && c.ends_with(tail), "{c}");
            assert!(!c.contains('\\'), "{c}");
        }
        let script = hook_script().unwrap();
        assert!(script.starts_with("#!/bin/sh\n# ryucad: "), "{script}");
        assert!(script.ends_with("' merge status --hook\n"), "{script}");
    }

    #[test]
    fn writing_keeps_the_file_when_nothing_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_attributes(dir.path()).unwrap();
        let path = dir.path().join(ATTRIBUTES);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(missing(&text).is_empty());
        std::fs::write(&path, format!("{text}*.bin binary\n")).unwrap();
        write_attributes(dir.path()).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with("*.bin binary\n")
        );
        assert!(setup(dir.path()).is_none());
    }

    #[test]
    fn a_clones_hook_is_found_and_judged() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        crate::git::tests::git(r, &["init", "-q"]);
        let s = setup(r).unwrap();
        assert_eq!(s.missing, [TEXTCONV, MERGE_DRIVER, RENAMES]);
        let hooks = r.join(".git/hooks");
        let path = hooks.join(HOOKS[0]);
        let first = |r: &Path| setup(r).unwrap().hooks[0].1.clone();
        assert_eq!(s.hooks[0], (HOOKS[0], Hook::Missing(path.clone())));
        assert_eq!(
            s.hooks[1],
            (HOOKS[1], Hook::Missing(hooks.join("pre-commit")))
        );
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(&path, "#!/bin/sh\necho mine\n").unwrap();
        assert_eq!(first(r), Hook::Foreign(path.clone()));
        // Someone's own hook that runs the check counts.
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho mine\n{}\n", hook_line().unwrap()),
        )
        .unwrap();
        assert_eq!(first(r), Hook::Installed(path.clone()));
        write_hook(&path).unwrap();
        assert_eq!(first(r), Hook::Installed(path.clone()));
        crate::git::tests::git(r, &["config", MERGE_DRIVER, "x"]);
        assert_eq!(setup(r).unwrap().missing, [TEXTCONV, RENAMES]);
        crate::git::tests::git(r, &["config", RENAMES, "true"]);
        assert_eq!(setup(r).unwrap().missing, [TEXTCONV, RENAMES]);
        crate::git::tests::git(r, &["config", RENAMES, "false"]);
        assert_eq!(setup(r).unwrap().missing, [TEXTCONV]);
        // A hooks folder of the user's choosing is never written to.
        crate::git::tests::git(r, &["config", "core.hooksPath", "githooks"]);
        assert_eq!(first(r), Hook::HooksPath(r.join("githooks").join(HOOKS[0])));
    }

    #[test]
    fn install_keeps_someones_own_hook_that_runs_the_check() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        crate::git::tests::git(r, &["init", "-q"]);
        let hooks = r.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        // The line `install` asks for, added to a hook of their own.
        let mine = format!("#!/bin/sh\necho mine\n{}\n", hook_line().unwrap());
        let path = hooks.join(HOOKS[0]);
        std::fs::write(&path, &mine).unwrap();
        let out = install(r, true).unwrap();
        assert_eq!(out["hooks"][HOOKS[0]]["status"], "installed");
        assert_eq!(out["config"][RENAMES], "false");
        assert!(setup(r).unwrap().missing.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);
        // Ours is still brought up to date, and a missing one written.
        let theirs = hooks.join(HOOKS[1]);
        assert_eq!(
            std::fs::read_to_string(theirs).unwrap(),
            hook_script().unwrap()
        );
    }
}
