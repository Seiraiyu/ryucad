//! `ryucad` for the app's tests (#555): the `ryucad` binary's program
//! (crates/ryucad/src/main.rs), built by `cargo test -p ryucad-app` and
//! nextest with the tests' own features, so no test process runs Cargo
//! again to get it. `tests/common::ryucad()` finds it.

// As the `ryucad` binary's: see its main.rs.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    // SAFETY: first thing in main: no thread has been started.
    unsafe { ryucad::process::start(ryucad::cli::main) }
}
