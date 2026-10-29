//! Records the git commit and target triple for `ryueda version`.

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
    // Release builds set RYUEDA_COMMIT; local builds ask git, and a build
    // from a source archive reports `unknown`.
    let commit = std::env::var("RYUEDA_COMMIT")
        .ok()
        .or_else(|| git(&["rev-parse", "--short=12", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=RYUEDA_COMMIT={commit}");
    println!(
        "cargo:rustc-env=RYUEDA_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
    println!("cargo:rerun-if-env-changed=RYUEDA_COMMIT");
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
    if let Some(packed) = git(&["rev-parse", "--git-path", "packed-refs"]) {
        println!("cargo:rerun-if-changed={packed}");
    }
}
