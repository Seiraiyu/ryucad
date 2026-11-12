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
use std::collections::BTreeMap;
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

/// Every ryu product that writes a line in the shared hook: a line of
/// someone's own hook whose program is one of them is that product's.
pub const PRODUCTS: [&str; 2] = ["ryucad", "ryueda"];

/// The line that marks the shared hook.
const HEADER: &str = "# ryu: refuse a merge commit while a ryu project's merge is unresolved (`ryucad git install`, `ryueda git install`)";

/// The line that marked ryucad's own hook before the shared one.
const RYUCAD_HEADER: &str = "# ryucad: refuse a merge commit while a ryucad project's merge is unresolved (`ryucad git install`)";

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

/// The product lines of a shared hook (or of ryucad's own, before it),
/// product to the command naming its binary as the hook has it; `None`
/// for a hook that is neither.
fn managed(text: &str) -> Option<BTreeMap<String, String>> {
    let lines: Vec<&str> = text.lines().collect();
    let command = |l: &str| {
        let l = l.trim();
        let l = l.strip_prefix("exec ").unwrap_or(l);
        let l = l.strip_suffix(" || exit 1").unwrap_or(l);
        l.strip_suffix(&format!(" {HOOK_CHECK}")).map(str::to_owned)
    };
    if lines.contains(&HEADER) {
        let mut out = BTreeMap::new();
        for w in lines.windows(2) {
            if let Some(p) = w[0].strip_prefix("# ")
                && PRODUCTS.contains(&p)
                && let Some(exe) = command(w[1])
            {
                out.insert(p.to_owned(), exe);
            }
        }
        return Some(out);
    }
    if lines.contains(&RYUCAD_HEADER) {
        let exe = lines.iter().find_map(|l| command(l));
        return Some(exe.map(|e| ("ryucad".to_owned(), e)).into_iter().collect());
    }
    None
}

/// The program a hook line runs: its first word, unquoted, as a file
/// name without `.exe`.
fn program(line: &str) -> String {
    let l = line.trim();
    let l = l.strip_prefix("exec ").unwrap_or(l);
    let word = match l.chars().next() {
        Some(q @ ('\'' | '"')) => l[1..].split(q).next().unwrap_or_default(),
        _ => l.split_whitespace().next().unwrap_or_default(),
    };
    let name = word.rsplit(['/', '\\']).next().unwrap_or_default();
    name.strip_suffix(".exe").unwrap_or(name).to_owned()
}

/// Whether someone's own hook runs `product`'s check: a line running
/// [`HOOK_CHECK`] whose program isn't another ryu product.
fn runs_check(text: &str, product: &str) -> bool {
    text.lines().any(|l| {
        l.contains(HOOK_CHECK) && {
            let p = program(l);
            p == product || !PRODUCTS.contains(&p.as_str())
        }
    })
}

/// The hook `name` of `repo`, as it stands for `product`.
fn hook(repo: &gix::Repository, name: &str, product: &str) -> Hook {
    let (dir, shared) = hooks_dir(repo);
    let path = dir.join(name);
    match std::fs::read_to_string(&path) {
        Ok(text) => match managed(&text) {
            Some(lines) if lines.contains_key(product) => Hook::Installed(path),
            Some(_) if shared => Hook::HooksPath(path),
            Some(_) => Hook::Missing(path),
            None if runs_check(&text, product) => Hook::Installed(path),
            None if shared => Hook::HooksPath(path),
            None => Hook::Foreign(path),
        },
        Err(_) if shared => Hook::HooksPath(path),
        Err(_) => Hook::Missing(path),
    }
}

/// The shared hook holding `lines` (product to its command), in name
/// order: every line but the last stops the hook on failure, and the last
/// is `exec`'d.
fn script(lines: &BTreeMap<String, String>) -> String {
    let mut out = format!("#!/bin/sh\n{HEADER}\n");
    let n = lines.len();
    for (i, (product, exe)) in lines.iter().enumerate() {
        out.push_str(&format!("# {product}\n"));
        if i + 1 == n {
            out.push_str(&format!("exec {exe} {HOOK_CHECK}\n"));
        } else {
            out.push_str(&format!("{exe} {HOOK_CHECK} || exit 1\n"));
        }
    }
    out
}

/// The shared hook a clone with no hook gets from `product`'s install.
pub fn hook_script(product: &str) -> Result<String> {
    Ok(script(
        &[(product.to_owned(), exe(product)?)].into_iter().collect(),
    ))
}

/// Writes a hook at `path`, executable.
fn write_hook(path: &Path, text: &str) -> Result<()> {
    ryu_doc::source::write_atomic(path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| Error::io(path, &e))?;
    }
    Ok(())
}

/// Puts `product`'s line in each of the clone's [`HOOKS`]: the shared hook
/// written or brought up to date (another product's lines kept, ryucad's
/// old hook taken in), someone else's left alone with the line to add.
fn install_hooks(repo: &gix::Repository, product: &str) -> Result<Map<String, Value>> {
    let mut hooks = Map::new();
    for name in HOOKS {
        let v = match hook(repo, name, product) {
            Hook::Installed(p) | Hook::Missing(p) => {
                let text = std::fs::read_to_string(&p).ok();
                // Only the shared hook (or none) is written: someone's own
                // that runs the check stays as it is.
                let ours = match text.as_deref() {
                    None => Some(BTreeMap::new()),
                    Some(t) => managed(t),
                };
                if let Some(mut lines) = ours {
                    lines.insert(product.to_owned(), exe(product)?);
                    let new = script(&lines);
                    if text.as_deref() != Some(new.as_str()) {
                        write_hook(&p, &new)?;
                    }
                }
                json!({"path": p, "status": "installed"})
            }
            Hook::Foreign(p) => {
                json!({"path": p, "status": "foreign", "line": hook_line(product)?})
            }
            Hook::HooksPath(p) => {
                json!({"path": p, "status": "hooks_path", "line": hook_line(product)?})
            }
        };
        hooks.insert(name.to_owned(), v);
    }
    Ok(hooks)
}

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

    fn hook_at(r: &Path) -> PathBuf {
        r.join(".git/hooks/pre-merge-commit")
    }

    fn cad() -> Drivers {
        Drivers {
            product: "ryucad",
            textconv: "diff.ryucad.textconv",
            merge: None,
            hook: true,
        }
    }

    fn eda() -> Drivers {
        Drivers {
            product: "ryueda",
            textconv: "diff.ryueda.textconv",
            merge: None,
            hook: true,
        }
    }

    #[test]
    fn one_hook_holds_every_products_line() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        git(r, &["init", "-q"]);
        let (cad, eda) = (cad(), eda());
        install(r, true, &cad, true).unwrap();
        let one = std::fs::read_to_string(hook_at(r)).unwrap();
        assert_eq!(one, hook_script("ryucad").unwrap());
        assert!(
            one.starts_with(&format!("#!/bin/sh\n{HEADER}\n# ryucad\nexec '")),
            "{one}"
        );
        assert!(one.ends_with("' merge status --hook\n"), "{one}");
        assert!(matches!(
            setup(r, &eda, true).unwrap().hooks[0].1,
            Hook::Missing(_)
        ));
        install(r, true, &eda, true).unwrap();
        let both = std::fs::read_to_string(hook_at(r)).unwrap();
        let lines: Vec<&str> = both.lines().collect();
        assert_eq!(lines[2], "# ryucad");
        assert!(
            lines[3].ends_with("' merge status --hook || exit 1"),
            "{both}"
        );
        assert_eq!(lines[4], "# ryueda");
        assert!(
            lines[5].starts_with("exec '") && lines[5].ends_with("' merge status --hook"),
            "{both}"
        );
        for d in [&cad, &eda] {
            assert!(matches!(
                setup(r, d, true).unwrap().hooks[0].1,
                Hook::Installed(_)
            ));
        }
        // Installing again changes nothing.
        install(r, true, &cad, true).unwrap();
        assert_eq!(std::fs::read_to_string(hook_at(r)).unwrap(), both);
    }

    #[test]
    fn ryucads_own_hook_is_rewritten_into_the_block() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        git(r, &["init", "-q"]);
        std::fs::create_dir_all(r.join(".git/hooks")).unwrap();
        let old = format!("#!/bin/sh\n{RYUCAD_HEADER}\nexec '/opt/ryucad' merge status --hook\n");
        std::fs::write(hook_at(r), &old).unwrap();
        let (cad, eda) = (cad(), eda());
        assert!(matches!(
            setup(r, &cad, true).unwrap().hooks[0].1,
            Hook::Installed(_)
        ));
        assert!(matches!(
            setup(r, &eda, true).unwrap().hooks[0].1,
            Hook::Missing(_)
        ));
        install(r, true, &eda, true).unwrap();
        let text = std::fs::read_to_string(hook_at(r)).unwrap();
        assert!(
            text.contains(
                "# ryucad\n'/opt/ryucad' merge status --hook || exit 1\n# ryueda\nexec '"
            ),
            "{text}"
        );
    }

    #[test]
    fn someone_elses_hook_and_hooks_path_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        git(r, &["init", "-q"]);
        std::fs::create_dir_all(r.join(".git/hooks")).unwrap();
        let (cad, eda) = (cad(), eda());
        std::fs::write(hook_at(r), "#!/bin/sh\necho mine\n").unwrap();
        let out = install(r, true, &cad, true).unwrap();
        assert_eq!(out["hooks"]["pre-merge-commit"]["status"], "foreign");
        assert_eq!(
            std::fs::read_to_string(hook_at(r)).unwrap(),
            "#!/bin/sh\necho mine\n"
        );
        // A line of someone's own that runs a check counts for that product
        // only: a bare check counts as ryucad's, as it always did.
        std::fs::write(
            hook_at(r),
            "#!/bin/sh\n\"$HOME/.ryueda/bin/ryueda\" merge status --hook || exit 1\n",
        )
        .unwrap();
        assert!(matches!(
            setup(r, &eda, true).unwrap().hooks[0].1,
            Hook::Installed(_)
        ));
        assert!(matches!(
            setup(r, &cad, true).unwrap().hooks[0].1,
            Hook::Foreign(_)
        ));
        std::fs::write(hook_at(r), "#!/bin/sh\nmytool merge status --hook\n").unwrap();
        assert!(matches!(
            setup(r, &cad, true).unwrap().hooks[0].1,
            Hook::Installed(_)
        ));
        git(r, &["config", "core.hooksPath", "githooks"]);
        assert_eq!(
            setup(r, &cad, true).unwrap().hooks[0].1,
            Hook::HooksPath(r.join("githooks/pre-merge-commit"))
        );
    }

    #[test]
    fn a_hook_lines_program() {
        assert_eq!(program("exec '/a b/ryucad' merge status --hook"), "ryucad");
        assert_eq!(
            program("\"C:\\x\\ryueda.exe\" merge status --hook"),
            "ryueda"
        );
        assert_eq!(program("  mytool merge status --hook"), "mytool");
    }
}
