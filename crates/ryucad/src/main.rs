mod cli;

// Linux's releases are musl, whose allocator is slow under multithreaded
// allocation; glibc builds (development, tests, the bars) use the same
// allocator, so what they measure is what ships: at design 192's
// benchmark a warm daemon's rebuild took 12.1 s under glibc's, 9.1 s.
#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// The CLI's own stack: a debug build parses commands and dispatches
/// requests in frames that outgrow Windows' 1 MiB main thread (and come
/// close on macOS), so it runs with Linux's default 8 MiB everywhere.
const STACK: usize = 8 << 20;

fn main() -> std::process::ExitCode {
    // #382: a credential helper never waits on a sign-in nobody can answer
    // (Git Credential Manager's window in a session with no desktop, or a
    // terminal prompt in a daemon). Credentials come only from what the
    // helpers hold; a person signs in once with `git ls-remote <url>`.
    for (k, v) in [("GCM_INTERACTIVE", "never"), ("GIT_TERMINAL_PROMPT", "0")] {
        if std::env::var_os(k).is_none() {
            // SAFETY: nothing else runs yet: no thread has been started.
            unsafe { std::env::set_var(k, v) };
        }
    }
    std::thread::Builder::new()
        .name("main".into())
        .stack_size(STACK)
        .spawn(cli::main)
        .expect("the CLI's thread starts")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}
