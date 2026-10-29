//! ryueda's release line beside ryucad's: `scripts/stamp-version.sh` reads
//! either line's tags.

#[cfg(unix)]
use std::path::{Path, PathBuf};

/// The repository's root.
#[cfg(unix)]
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
fn stamp_version_reads_either_lines_tags() {
    for (arg, version) in [
        ("1.2.3", "1.2.3"),
        ("v1.2.3", "1.2.3"),
        ("ryueda-v1.2.3", "1.2.3"),
        ("ryueda-v0.1.0-pre.4", "0.1.0-pre.4"),
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
    for arg in ["ryucad-v1.2.3", "ryueda-1.2.3", "vx"] {
        let (code, printed, _) = stamp(arg);
        assert_eq!(code, 2, "{arg}: {printed}");
        assert!(printed.contains("not a version"), "{arg}: {printed}");
    }
}
