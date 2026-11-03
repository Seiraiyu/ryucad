//! The owner's files as a source: a root `cox-files:<set>[@vN]` names a
//! `cox files` set, pulled at split time (see [`materialize`]). Design 277,
//! "12.C: The owner's STEP files".

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The prefix of a `cox-files:` root.
pub const PREFIX: &str = "cox-files:";

/// A `cox-files:<set>[@vN]` root: the set, and the version it is pinned at
/// (`None`: the latest, for exploratory runs only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub set: String,
    pub version: Option<u32>,
}

impl Root {
    /// `None` for a root of another form; else the root, or why it doesn't
    /// read.
    pub fn parse(root: &str) -> Option<Result<Root, String>> {
        let rest = root.strip_prefix(PREFIX)?;
        let (set, version) = match rest.split_once('@') {
            Some((set, v)) => match v.strip_prefix('v').and_then(|n| n.parse::<u32>().ok()) {
                Some(n) if n > 0 => (set, Some(n)),
                _ => return Some(Err(format!("{root}: a version is written @v1, @v2, …"))),
            },
            None => (rest, None),
        };
        let named = !set.is_empty()
            && !set.starts_with('.')
            && set
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        if !named {
            return Some(Err(format!("{root}: not a set's name")));
        }
        Some(Ok(Root {
            set: set.to_owned(),
            version,
        }))
    }
}

/// How to run `cox`: `$HUNT_COX`, else `cox` on `PATH`, with `env` added
/// (tests give their fake its folders this way).
#[derive(Clone, Debug)]
pub struct Cox {
    pub exe: PathBuf,
    pub env: Vec<(String, OsString)>,
}

impl Cox {
    pub fn from_env() -> Cox {
        Cox {
            exe: std::env::var_os("HUNT_COX")
                .filter(|v| !v.is_empty())
                .map_or_else(|| PathBuf::from("cox"), PathBuf::from),
            env: Vec::new(),
        }
    }

    /// `cox --json <args>`'s answer, or why there is none.
    fn json(&self, args: &[&str]) -> Result<Value, String> {
        let what = format!("cox files {}", args[1..].join(" "));
        let o = Command::new(&self.exe)
            .arg("--json")
            .args(args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .output()
            .map_err(|e| format!("{what}: {} can't be run: {e}", self.exe.display()))?;
        let v: Value = serde_json::from_slice(&o.stdout).map_err(|_| {
            format!(
                "{what}: no answer ({})",
                String::from_utf8_lossy(&o.stderr).trim()
            )
        })?;
        if let Some(m) = v.get("error") {
            return Err(format!(
                "{what}: {}",
                m["message"].as_str().unwrap_or("refused")
            ));
        }
        if !o.status.success() {
            return Err(format!("{what}: exited with {}", o.status));
        }
        Ok(v)
    }
}

/// One file of the collection: where it is linked (`name`, under the
/// root), where it lies in its version (`path`), its hash, size and the
/// first version holding it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub version: u32,
}

/// A pulled file whose bytes aren't what `cox` listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mismatch {
    pub name: String,
    pub version: u32,
    pub expected: String,
    pub found: String,
    /// The pulled file, in cox's folder.
    pub file: PathBuf,
}

/// A set made into a root.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pulled {
    pub set: String,
    pub version: u32,
    /// Whether the root named the version (else it is the latest).
    pub pinned: bool,
    /// Where the files are linked: the source's root.
    pub root: PathBuf,
    /// The collection, files whose hash differs included.
    pub entries: Vec<Entry>,
    pub mismatched: Vec<Mismatch>,
    pub notes: Vec<String>,
}

/// Whether `path` is a plain relative path, safe to join under a folder.
pub fn plain(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// Every version's files `(path, sha256, size)`, versions in order, as one
/// collection: the first version holding a hash names it, and a file under
/// a name already taken by other bytes goes under `v<k>/`.
pub fn entries(listed: &[(u32, Vec<(String, String, u64)>)]) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for (k, files) in listed {
        for (path, sha, size) in files {
            if out.iter().any(|e| &e.sha256 == sha) {
                continue;
            }
            let name = if out.iter().any(|e| &e.name == path) {
                format!("v{k}/{path}")
            } else {
                path.clone()
            };
            out.push(Entry {
                name,
                path: path.clone(),
                sha256: sha.clone(),
                size: *size,
                version: *k,
            });
        }
    }
    out
}

fn link(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(drop))
}

/// Pulls every version of `root`'s set up to the one it names (the latest
/// when it names none) and links the collection into `links/<set>@vN/`,
/// emptied first. A file whose bytes aren't the hash `cox` lists is in
/// `mismatched` and isn't linked. An error is a reason to skip the source:
/// no `cox`, not signed in, no such set or version.
pub fn materialize(cox: &Cox, root: &Root, links: &Path) -> Result<Pulled, String> {
    let top = match root.version {
        Some(n) => n,
        None => {
            let v = cox.json(&["files", "show", &root.set])?;
            v["version"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("cox files show {}: no version", root.set))?
        }
    };
    let mut notes = Vec::new();
    let mut listed = Vec::new();
    let mut folders = std::collections::BTreeMap::new();
    for k in 1..=top {
        let r = format!("{}@v{k}", root.set);
        let shown = match cox.json(&["files", "show", &r]) {
            Ok(v) => v,
            Err(e) if k < top => {
                notes.push(format!("{e}: skipped"));
                continue;
            }
            Err(e) => return Err(e),
        };
        let mut files = Vec::new();
        for e in shown["entries"].as_array().into_iter().flatten() {
            let (Some(path), Some(sha), Some(size)) =
                (e["path"].as_str(), e["sha256"].as_str(), e["size"].as_u64())
            else {
                return Err(format!(
                    "cox files show {r}: an entry without path, sha256 and size"
                ));
            };
            if !plain(path) {
                notes.push(format!("{r}: {path} isn't a plain relative path: skipped"));
                continue;
            }
            files.push((path.to_owned(), sha.to_owned(), size));
        }
        let pulled = cox.json(&["files", "pull", &r])?;
        let folder = pulled["path"]
            .as_str()
            .ok_or_else(|| format!("cox files pull {r}: no path"))?;
        folders.insert(k, PathBuf::from(folder));
        listed.push((k, files));
    }
    let entries = entries(&listed);
    let out = links.join(format!("{}@v{top}", root.set));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut mismatched = Vec::new();
    for e in &entries {
        let from = folders[&e.version].join(&e.path);
        let bytes = std::fs::read(&from).map_err(|err| format!("{}: {err}", from.display()))?;
        let found = sha256_hex(&bytes);
        if found != e.sha256 {
            mismatched.push(Mismatch {
                name: e.name.clone(),
                version: e.version,
                expected: e.sha256.clone(),
                found,
                file: from,
            });
            continue;
        }
        let to = out.join(&e.name);
        link(&from, &to).map_err(|err| format!("{}: {err}", to.display()))?;
    }
    Ok(Pulled {
        set: root.set.clone(),
        version: top,
        pinned: root.version.is_some(),
        root: out,
        entries,
        mismatched,
        notes,
    })
}

/// The SHA-256 of `bytes` in lower-case hex, as `cox files` lists it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_read_with_and_without_a_version() {
        assert_eq!(Root::parse("crates/x"), None);
        assert_eq!(
            Root::parse("cox-files:step-files@v2"),
            Some(Ok(Root {
                set: "step-files".into(),
                version: Some(2)
            }))
        );
        assert_eq!(
            Root::parse("cox-files:step-files"),
            Some(Ok(Root {
                set: "step-files".into(),
                version: None
            }))
        );
        for bad in [
            "cox-files:",
            "cox-files:a@2",
            "cox-files:a@v0",
            "cox-files:a@vx",
            "cox-files:a b",
            "cox-files:../a",
        ] {
            assert!(Root::parse(bad).unwrap().is_err(), "{bad}");
        }
    }

    fn listed(k: u32, files: &[(&str, &str)]) -> (u32, Vec<(String, String, u64)>) {
        (
            k,
            files
                .iter()
                .map(|(p, h)| ((*p).to_owned(), (*h).to_owned(), 1))
                .collect(),
        )
    }

    #[test]
    fn versions_up_to_n_are_one_collection() {
        let e = entries(&[
            listed(1, &[("a.STEP", "aa"), ("b.STEP", "bb")]),
            listed(2, &[("c.STEP", "cc"), ("a2.STEP", "aa")]),
            listed(3, &[("b.STEP", "dd")]),
        ]);
        let got: Vec<(&str, &str, u32)> = e
            .iter()
            .map(|e| (e.name.as_str(), e.sha256.as_str(), e.version))
            .collect();
        assert_eq!(
            got,
            [
                ("a.STEP", "aa", 1),
                ("b.STEP", "bb", 1),
                ("c.STEP", "cc", 2),
                ("v3/b.STEP", "dd", 3),
            ],
            "the first version holding a hash names it; a later file under a taken name goes under v<k>/"
        );
        assert_eq!(e[3].path, "b.STEP", "where it lies in its own version");
    }

    #[test]
    fn only_plain_relative_paths_are_taken() {
        assert!(plain("a.STEP"));
        assert!(plain("d/a.STEP"));
        assert!(!plain("../a.STEP"));
        assert!(!plain("/etc/a.STEP"));
        assert!(!plain(""));
    }
}
