//! Records the git commit and target triple for `ryucad version`.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
}

fn main() {
    // Release builds set RYUCAD_COMMIT; local builds ask git, and a build
    // from a source archive reports `unknown`.
    let commit = std::env::var("RYUCAD_COMMIT")
        .ok()
        .or_else(|| git(&["rev-parse", "--short=12", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=RYUCAD_COMMIT={commit}");
    // A commit the release workflow gave, not one read from a checkout:
    // with a stamped version, what makes a build a release whose cache
    // entries other machines share (`cache::build_identity`).
    let given = std::env::var("RYUCAD_COMMIT").is_ok_and(|c| !c.is_empty());
    println!(
        "cargo:rustc-env=RYUCAD_COMMIT_GIVEN={}",
        if given { "1" } else { "0" }
    );
    println!(
        "cargo:rustc-env=RYUCAD_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
    println!("cargo:rerun-if-env-changed=RYUCAD_COMMIT");
    // Rerun when HEAD moves: a checkout changes HEAD, a commit changes the
    // branch's ref (loose or packed).
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &r])
    {
        println!("cargo:rerun-if-changed={path}");
    }
    // Only if it exists: Cargo reruns a build script whose watched file is
    // missing on every build, and a fresh fetch (actions/checkout) has no
    // packed-refs, so each app test's `cargo build -p ryucad` rebuilt
    // ryucad. Packing a loose ref deletes it, which the watch above sees.
    if let Some(packed) = git(&["rev-parse", "--git-path", "packed-refs"])
        && std::path::Path::new(&packed).exists()
    {
        println!("cargo:rerun-if-changed={packed}");
    }
}
