// Linux's releases are musl, whose allocator is slow under multithreaded
// allocation; glibc builds (development, tests, the bars) use the same
// allocator, so what they measure is what ships: at design 192's
// benchmark a warm daemon's rebuild took 12.1 s under glibc's, 9.1 s.
// macOS's own allocator is slower again at a daemon's work, which is
// mostly allocating and freeing across threads: the warm daemon of #238
// took about twice an 18-thread Linux box's time on a 10-core M4.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    // SAFETY: first thing in main: no thread has been started.
    unsafe { ryucad::process::start(ryucad::cli::main) }
}
