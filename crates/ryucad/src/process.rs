//! What a `ryucad` process sets up before the CLI runs: the `ryucad`
//! binary's and ryucad-app's `ryucad-test` example's (#555), so the
//! daemons the app's tests start are the release's program. Each binary
//! still names its own allocator, which a library can't.

use std::process::ExitCode;

/// The CLI's own stack: a debug build parses commands and dispatches
/// requests in frames that outgrow Windows' 1 MiB main thread (and come
/// close on macOS), so it runs with Linux's default 8 MiB everywhere.
pub const STACK: usize = 8 << 20;

/// Sets the process up and runs `run` (`ryucad::cli::main`) on a thread
/// of [`STACK`] bytes, giving back its exit code.
///
/// # Safety
///
/// It sets environment variables, so it must be called first thing in
/// `main`, before any thread has been started.
pub unsafe fn start(run: fn() -> ExitCode) -> ExitCode {
    // #236: mimalloc asks for transparent huge pages, and a 2 MiB page
    // stays resident while any of it is in use. A daemon over design
    // 192's benchmark lets thousands of parts' rebuilds go between
    // requests, and what they leave free inside huge pages was about
    // 0.3 GiB of its resident memory that the budget can't see. Turned
    // off for the process, mimalloc's purges hand it back.
    #[cfg(target_os = "linux")]
    // SAFETY: PR_SET_THP_DISABLE takes no pointers and only marks the
    // process; it fails harmlessly on kernels without it.
    unsafe {
        libc::prctl(libc::PR_SET_THP_DISABLE, 1, 0, 0, 0);
    }
    // #382: a credential helper never waits on a sign-in nobody can answer
    // (Git Credential Manager's window in a session with no desktop, or a
    // terminal prompt in a daemon). Credentials come only from what the
    // helpers hold; a person signs in once with `git ls-remote <url>`.
    for (k, v) in [("GCM_INTERACTIVE", "never"), ("GIT_TERMINAL_PROMPT", "0")] {
        if std::env::var_os(k).is_none() {
            // SAFETY: nothing else runs yet: no thread has been started
            // (this function's contract).
            unsafe { std::env::set_var(k, v) };
        }
    }
    std::thread::Builder::new()
        .name("main".into())
        .stack_size(STACK)
        .spawn(run)
        .expect("the CLI's thread starts")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}
