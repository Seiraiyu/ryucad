mod cli;

#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// The CLI's own stack: a debug build parses commands and dispatches
/// requests in frames that outgrow Windows' 1 MiB main thread (and come
/// close on macOS), so it runs with Linux's default 8 MiB everywhere.
const STACK: usize = 8 << 20;

fn main() -> std::process::ExitCode {
    std::thread::Builder::new()
        .name("main".into())
        .stack_size(STACK)
        .spawn(cli::main)
        .expect("the CLI's thread starts")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}
