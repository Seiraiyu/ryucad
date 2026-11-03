//! A stand-in for `cox` in step-hunt's tests. It answers `cox [--json]
//! files show <set>[@vN]` and `cox [--json] files pull <set>@vN` from the
//! folders `FAKE_COX_SETS` holds: `<set>@v<k>/`, each one a version's
//! files. With `FAKE_COX_SETS` unset it refuses as `cox` does when not
//! signed in, and `FAKE_COX_LIE` names a file whose SHA-256 `show` gives
//! wrongly (all zeros).

use std::path::{Path, PathBuf};

use serde_json::json;
use step_hunt::cox_files::sha256_hex;

fn refuse(code: &str, message: &str, exit: i32) -> ! {
    println!(
        "{}",
        json!({"error": {"code": code, "hint": null, "message": message}})
    );
    std::process::exit(exit);
}

/// The files under `dir`, `/`-separated and sorted.
fn files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = format!("{prefix}{}", e.file_name().to_string_lossy());
        if e.file_type().unwrap().is_dir() {
            files(&e.path(), &format!("{name}/"), out);
        } else {
            out.push((name, e.path()));
        }
    }
}

/// The latest version of `set` under `sets`.
fn latest(sets: &Path, set: &str) -> Option<u32> {
    std::fs::read_dir(sets)
        .ok()?
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_prefix(&format!("{set}@v"))?.parse().ok()
        })
        .max()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--json").collect();
    let Some(sets) = std::env::var_os("FAKE_COX_SETS").map(PathBuf::from) else {
        refuse("unauthenticated", "not signed in: run cox login", 3)
    };
    let (cmd, reference) = match args.as_slice() {
        [files, cmd, r] if files == "files" => (cmd.as_str(), r.as_str()),
        _ => refuse("usage", "fake cox: files show|pull <ref>", 2),
    };
    let (set, asked) = match reference.split_once("@v") {
        Some((s, v)) => (s, v.parse::<u32>().ok()),
        None => (reference, None),
    };
    let Some(version) = asked.or_else(|| latest(&sets, set)) else {
        refuse("not_found", "file set not found", 4)
    };
    let dir = sets.join(format!("{set}@v{version}"));
    if !dir.is_dir() {
        refuse("not_found", "file set version not found", 4);
    }
    let lie = std::env::var("FAKE_COX_LIE").unwrap_or_default();
    match cmd {
        "show" => {
            let mut all = Vec::new();
            files(&dir, "", &mut all);
            let entries: Vec<_> = all
                .iter()
                .map(|(path, file)| {
                    let bytes = std::fs::read(file).unwrap();
                    let sha = if *path == lie {
                        "0".repeat(64)
                    } else {
                        sha256_hex(&bytes)
                    };
                    json!({"path": path, "sha256": sha, "size": bytes.len()})
                })
                .collect();
            println!(
                "{}",
                json!({"name": set, "version": version, "note": "", "entries": entries})
            );
        }
        "pull" => println!(
            "{}",
            json!({"name": set, "version": version, "path": dir, "files": null})
        ),
        _ => refuse("usage", "fake cox: files show|pull <ref>", 2),
    }
}
