//! How the app ends, always written down (#799): the trace
//! (`ryucad-app.log` in the runtime folder) gets a line for every way the
//! process can stop, and on Windows a fault leaves a minidump beside it.
//!
//! - A normal end: `exited: …` from `main`, after `on_exit`'s "the window
//!   is closing".
//! - A panic, on any thread: `panicked: …` (and a crash report).
//! - An allocation that failed: `allocation of N bytes failed`. A null
//!   from the allocator isn't always the end (`try_reserve`'s callers
//!   recover), so [`TracedAlloc`] only notes it on its thread; on Unix the
//!   `SIGABRT` that `handle_alloc_error` raises writes the line. On Windows
//!   that abort is a fail-fast no handler sees: the null takes a minidump
//!   of its own (`…-alloc.dmp`), removed if the thread allocates again
//!   [`RECOVERED_AFTER_MS`] later (so it recovered), and
//!   Rust's `memory allocation of N bytes failed` goes to standard error,
//!   the trace when there's no console.
//! - The GPU's device lost, or an error wgpu would panic on: `wgpu …`.
//! - `exit()` called by anything but `main`: `exited: exit() called …`
//!   with where from (an `atexit` handler on Unix, a TLS callback when
//!   the process detaches on Windows).
//! - A fault: on Windows an unhandled exception (and a stack overflow, by
//!   a vectored handler) writes `fault: …` with the module it was in, and
//!   a minidump, then the module it was in; on Unix a fatal signal writes
//!   `fault: signal …`.
//! - Every [`HEARTBEAT`] while the window runs, `alive: …`: frames, the
//!   model's counts and memory. A process killed from outside, or one that
//!   failed fast (a `/GS` check, the CRT's `abort`, which no handler sees),
//!   is the one that ends with neither `exited` nor a fault, its last
//!   heartbeat saying when it was last alive.
//!
//! Lines written while the process may be failing ([`emergency`]) are
//! formatted on the stack into the file opened at the start, with no
//! allocation.

use std::cell::Cell;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The trace's cap: past it the old one becomes `ryucad-app.old.log`.
const TRACE_CAP: u64 = 1 << 20;

/// The trace's path, fixed at the first line.
static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
/// The trace, opened once: a failing process can still write to it.
static FILE: Mutex<Option<File>> = Mutex::new(None);
/// `main` has written how the process exited: anything ending it after
/// that is the ordinary end, not an `exit()` from elsewhere.
static ENDED: AtomicBool = AtomicBool::new(false);
/// The main thread panicked: the process ends with it.
static MAIN_PANICKED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The size of the last allocation that returned null on this thread,
    /// not yet recovered from: what an abort on it was for.
    static NULLED: Cell<usize> = const { Cell::new(0) };
    /// When it did, in [`millis`].
    static NULLED_AT: Cell<u64> = const { Cell::new(0) };
}

/// How long after a null the thread's next allocation means its caller
/// recovered. Rust's `handle_alloc_error` allocates before it aborts (it
/// reads `RUST_BACKTRACE`, and may print a backtrace), so an allocation
/// straight after a null proves nothing; a fatal one is long dead by
/// then.
const RECOVERED_AFTER_MS: u64 = 10_000;

/// Threads whose [`NULLED`] is set: allocations look only when it isn't 0.
static NULLS: AtomicUsize = AtomicUsize::new(0);

/// The allocation that returned null on this thread, if the thread hasn't
/// recovered from it: an abort now is `handle_alloc_error`'s.
pub fn nulled() -> Option<usize> {
    NULLED.try_with(Cell::get).ok().filter(|n| *n != 0)
}

/// The app's trace (#799): `ryucad-app.log` in the runtime folder
/// (`%LOCALAPPDATA%\ryucad\run` on Windows), where the crash reports go.
pub fn trace_path() -> Option<PathBuf> {
    PATH.get_or_init(|| {
        ryucad::endpoint::runtime_dir()
            .ok()
            .map(|d| d.join("ryucad-app.log"))
    })
    .clone()
}

fn open(path: &std::path::Path) -> Option<File> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Appends a line to the trace: seconds since the epoch, the process,
/// the line. Never fails the app. A line beginning `exited:` is how the
/// process ends.
pub fn trace(line: &str) {
    if line.starts_with("exited:") {
        ENDED.store(true, Ordering::SeqCst);
    }
    let Some(path) = trace_path() else { return };
    let mut file = FILE.lock().unwrap_or_else(|e| e.into_inner());
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > TRACE_CAP) {
        *file = None;
        let _ = std::fs::rename(&path, path.with_file_name("ryucad-app.old.log"));
    }
    if file.is_none() {
        *file = open(&path);
    }
    if let Some(f) = file.as_mut() {
        let _ = writeln!(f, "{} {} {line}", now(), std::process::id());
    }
}

/// A line formatted on the stack, cut at its size: [`emergency`]'s.
struct Line {
    buf: [u8; 768],
    len: usize,
}

impl std::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let room = self.buf.len() - 1 - self.len;
        let n = s.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Writes a line to the trace without allocating, for when the process
/// is failing: an allocation that failed, a fault, a signal, the process
/// detaching. Skipped only if another thread holds the trace.
pub fn emergency(args: std::fmt::Arguments<'_>) {
    let mut line = Line {
        buf: [0; 768],
        len: 0,
    };
    let _ = write!(line, "{} {} ", now(), std::process::id());
    let _ = line.write_fmt(args);
    line.buf[line.len] = b'\n';
    let bytes = &line.buf[..=line.len];
    if let Ok(mut file) = FILE.try_lock() {
        if let Some(f) = file.as_mut() {
            let _ = f.write_all(bytes);
            return;
        }
        // Not opened yet: a thread whose allocation failed can't open it.
        if nulled().is_none()
            && let Some(path) = trace_path()
        {
            *file = open(&path);
            if let Some(f) = file.as_mut() {
                let _ = f.write_all(bytes);
            }
        }
    }
}

/// What the window is doing, for the heartbeat and the failure lines.
static FRAMES: AtomicU64 = AtomicU64::new(0);
static FRAME_BEGAN: AtomicU64 = AtomicU64::new(0);
static IN_FRAME: AtomicBool = AtomicBool::new(false);
static PARTS_LISTED: AtomicU64 = AtomicU64::new(0);
static PARTS_LOADED: AtomicU64 = AtomicU64::new(0);
static MESHES: AtomicU64 = AtomicU64::new(0);
static ASKED: AtomicU64 = AtomicU64::new(0);

fn started() -> Instant {
    static AT: OnceLock<Instant> = OnceLock::new();
    *AT.get_or_init(Instant::now)
}

fn millis() -> u64 {
    started().elapsed().as_millis() as u64
}

/// Notes a frame beginning.
pub fn frame_begins() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
    FRAME_BEGAN.store(millis(), Ordering::Relaxed);
    IN_FRAME.store(true, Ordering::Relaxed);
}

/// Notes the frame ended, with the model it showed: parts the daemon
/// lists and the window has loaded, meshes held and asked for.
pub fn frame_ends(listed: usize, loaded: usize, meshes: usize, asked: usize) {
    IN_FRAME.store(false, Ordering::Relaxed);
    PARTS_LISTED.store(listed as u64, Ordering::Relaxed);
    PARTS_LOADED.store(loaded as u64, Ordering::Relaxed);
    MESHES.store(meshes as u64, Ordering::Relaxed);
    ASKED.store(asked as u64, Ordering::Relaxed);
}

/// What the window was doing, in a line: `frames N (…)`.
fn doing(out: &mut impl std::fmt::Write) {
    let frames = FRAMES.load(Ordering::Relaxed);
    let ago = millis().saturating_sub(FRAME_BEGAN.load(Ordering::Relaxed)) / 1000;
    let _ = write!(out, "frames {frames}");
    if frames > 0 {
        let still = if IN_FRAME.load(Ordering::Relaxed) {
            ", still drawing it"
        } else {
            ""
        };
        let _ = write!(out, " (the last began {ago} s ago{still})");
    }
    let _ = write!(
        out,
        ", parts {}/{}, meshes {}, asked {}",
        PARTS_LOADED.load(Ordering::Relaxed),
        PARTS_LISTED.load(Ordering::Relaxed),
        MESHES.load(Ordering::Relaxed),
        ASKED.load(Ordering::Relaxed),
    );
}

/// Formats [`doing`] for an [`emergency`] line.
struct Doing;

impl std::fmt::Display for Doing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        doing(f);
        Ok(())
    }
}

/// How often the window says it is alive: `RYUCAD_APP_HEARTBEAT` seconds
/// (tests), else a minute.
pub const HEARTBEAT: Duration = Duration::from_secs(60);

/// Starts the heartbeat: `alive: …` every [`HEARTBEAT`], from a thread of
/// its own, so a window whose frames stopped still says so.
pub fn heartbeat() {
    let every = std::env::var("RYUCAD_APP_HEARTBEAT")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| *s > 0.0)
        .map_or(HEARTBEAT, Duration::from_secs_f64);
    let _ = std::thread::Builder::new()
        .name("ryucad-app heartbeat".into())
        .spawn(move || {
            loop {
                std::thread::sleep(every);
                let mut line = String::from("alive: ");
                doing(&mut line);
                if let Some((now, peak)) = memory() {
                    let _ = write!(line, ", memory {} MiB (peak {} MiB)", now >> 20, peak >> 20);
                }
                trace(&line);
            }
        });
}

/// The process's memory and its peak, in bytes: private bytes on
/// Windows, resident on Linux.
pub fn memory() -> Option<(u64, u64)> {
    #[cfg(windows)]
    return win::memory();
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        let kib = |key: &str| -> Option<u64> {
            let l = s.lines().find(|l| l.starts_with(key))?;
            l[key.len()..]
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .ok()
        };
        Some((kib("VmRSS:")? << 10, kib("VmHWM:")? << 10))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    None
}

/// Starts recording how the process ends: the trace file opened, and
/// the hooks for exits, faults and (on Windows) dumps.
pub fn install() {
    started();
    if let Some(path) = trace_path() {
        let mut file = FILE.lock().unwrap_or_else(|e| e.into_inner());
        if file.is_none() {
            *file = open(&path);
        }
    }
    #[cfg(unix)]
    unix::install();
    #[cfg(windows)]
    win::install();
}

/// Called as the process exits by any means but a kill: says so if
/// `main` didn't, which means something called `exit()`.
fn exiting_unexpectedly() -> Option<&'static str> {
    if ENDED.load(Ordering::SeqCst) {
        None
    } else if MAIN_PANICKED.load(Ordering::SeqCst) {
        Some("exited: after the main thread's panic")
    } else {
        Some("exited: exit() called, not by main")
    }
}

/// Ends the process with `code`, saying so and from where.
#[track_caller]
pub fn exit(code: i32) -> ! {
    let at = std::panic::Location::caller();
    trace(&format!("exited: process::exit({code}) at {at}"));
    std::process::exit(code)
}

/// A panic, for the trace: where, on which thread. On Windows a panic on
/// the main thread, which ends the process, leaves a minidump too.
pub fn panicked(what: &str, at: Option<&std::panic::Location<'_>>, report: &str) {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unnamed");
    let at = at.map(|l| format!(" at {l}")).unwrap_or_default();
    if name == "main" {
        MAIN_PANICKED.store(true, Ordering::SeqCst);
    }
    let mut line = format!("panicked on thread '{name}'{at}: {what}; {report}; ");
    doing(&mut line);
    trace(&line);
    #[cfg(windows)]
    if name == "main" {
        win::dump(std::ptr::null_mut(), "a panic");
    }
}

/// The global allocator's wrapper: the system's, noting on its thread an
/// allocation that returned null, which is fatal only if the caller then
/// calls `handle_alloc_error` (`try_reserve`'s callers recover). On Unix
/// the abort that follows writes the line; on Windows the null takes a
/// minidump, removed if the thread allocates again
/// [`RECOVERED_AFTER_MS`] later.
pub struct TracedAlloc;

impl TracedAlloc {
    #[cold]
    fn returned_null(&self, size: usize) {
        let Ok(was) = NULLED.try_with(|c| c.replace(size.max(1))) else {
            return;
        };
        let _ = NULLED_AT.try_with(|c| c.set(millis()));
        if was == 0 {
            NULLS.fetch_add(1, Ordering::SeqCst);
        }
        #[cfg(windows)]
        win::alloc_dump();
    }

    /// An allocation succeeded: if this thread's last one returned null
    /// [`RECOVERED_AFTER_MS`] ago or more, its caller recovered.
    #[inline]
    fn succeeded(&self) {
        if NULLS.load(Ordering::Relaxed) != 0 {
            self.recovered();
        }
    }

    #[cold]
    fn recovered(&self) {
        let pending = NULLED.try_with(Cell::get).is_ok_and(|n| n != 0);
        let at = NULLED_AT.try_with(Cell::get).unwrap_or(u64::MAX);
        if !pending || millis().saturating_sub(at) < RECOVERED_AFTER_MS {
            return;
        }
        if NULLED.try_with(|c| c.replace(0)).is_ok_and(|was| was != 0) {
            NULLS.fetch_sub(1, Ordering::SeqCst);
            #[cfg(windows)]
            win::alloc_recovered();
        }
    }

    #[inline]
    fn seen(&self, p: *mut u8, size: usize) -> *mut u8 {
        if p.is_null() {
            self.returned_null(size);
        } else {
            self.succeeded();
        }
        p
    }
}

// SAFETY: every call is the system allocator's, unchanged.
unsafe impl std::alloc::GlobalAlloc for TracedAlloc {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        self.seen(unsafe { std::alloc::System.alloc(layout) }, layout.size())
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        self.seen(
            unsafe { std::alloc::System.alloc_zeroed(layout) },
            layout.size(),
        )
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new: usize) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        self.seen(unsafe { std::alloc::System.realloc(ptr, layout, new) }, new)
    }
}

/// The window's device: a lost device and the errors wgpu would panic on
/// are written to the trace first (the panic stays: [`super::platform`]
/// tells one before the first frame from a fault).
pub fn watch_device(device: &eframe::wgpu::Device) {
    device.set_device_lost_callback(|reason, message| {
        trace(&format!("wgpu device lost ({reason:?}): {message}"));
    });
    device.on_uncaptured_error(std::sync::Arc::new(|e: eframe::wgpu::Error| {
        let kind = match &e {
            eframe::wgpu::Error::OutOfMemory { .. } => "out of memory",
            eframe::wgpu::Error::Validation { .. } => "validation",
            eframe::wgpu::Error::Internal { .. } => "internal",
        };
        trace(&format!("wgpu error ({kind}): {e}"));
        // What wgpu does without a handler.
        panic!("wgpu error: {e}\n");
    }));
}

/// `exit()` on Unix: an `atexit` handler, which `main`'s ordinary return
/// runs too (after its `exited` line), and fatal signals.
#[cfg(unix)]
mod unix {
    use super::*;

    extern "C" fn at_exit() {
        if let Some(how) = exiting_unexpectedly() {
            trace(&format!(
                "{how}; {}\n{}",
                Doing,
                std::backtrace::Backtrace::force_capture()
            ));
        }
    }

    const SIGNALS: [(libc::c_int, &str); 5] = [
        (libc::SIGSEGV, "SIGSEGV"),
        (libc::SIGBUS, "SIGBUS"),
        (libc::SIGILL, "SIGILL"),
        (libc::SIGFPE, "SIGFPE"),
        (libc::SIGABRT, "SIGABRT"),
    ];

    /// The handlers there were before ours, put back once ours has run.
    static OLD: [OnceLock<libc::sigaction>; 5] = [const { OnceLock::new() }; 5];

    extern "C" fn on_signal(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
        let Some(i) = SIGNALS.iter().position(|(s, _)| *s == sig) else {
            return;
        };
        // SAFETY: the kernel passes a valid siginfo to an SA_SIGINFO
        // handler.
        let addr = unsafe { info.as_ref().map_or(std::ptr::null_mut(), |i| i.si_addr()) };
        // `handle_alloc_error` aborts on the thread whose allocation failed.
        if sig == libc::SIGABRT
            && let Some(size) = nulled()
        {
            emergency(format_args!("allocation of {size} bytes failed; {}", Doing));
        }
        emergency(format_args!(
            "fault: signal {} at {addr:p}; {}",
            SIGNALS[i].1, Doing
        ));
        // The handler before ours (Rust's stack-overflow message, or the
        // default) takes the signal again: a fault repeats when this
        // returns, and abort raises again.
        // SAFETY: a sigaction `install` read, or the default.
        unsafe {
            match OLD[i].get() {
                Some(old) => libc::sigaction(sig, old, std::ptr::null_mut()),
                None => {
                    libc::signal(sig, libc::SIG_DFL);
                    0
                }
            };
        }
    }

    pub fn install() {
        // SAFETY: `at_exit` is a plain C function that never unwinds.
        unsafe { libc::atexit(at_exit) };
        for (i, (sig, _)) in SIGNALS.iter().enumerate() {
            // SAFETY: a zeroed sigaction is valid; the old one is read
            // before ours goes in.
            unsafe {
                let mut old: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(*sig, std::ptr::null(), &mut old) != 0 {
                    continue;
                }
                let _ = OLD[i].set(old);
                let mut new: libc::sigaction = std::mem::zeroed();
                new.sa_sigaction = on_signal
                    as extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void)
                    as usize;
                new.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
                libc::sigemptyset(&mut new.sa_mask);
                libc::sigaction(*sig, &new, std::ptr::null_mut());
            }
        }
    }
}

/// Windows: an unhandled-exception filter and a vectored handler for
/// stack overflows, which write the fault and a minidump; the process
/// detaching without `main`'s `exited` line; private bytes.
#[cfg(windows)]
pub mod win {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::atomic::{AtomicPtr, AtomicU32};
    use windows_sys::Win32::Foundation::{
        CloseHandle, EXCEPTION_STACK_OVERFLOW, GENERIC_WRITE, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_ALWAYS, CreateFileW, DeleteFileW, FILE_ATTRIBUTE_NORMAL,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
        MINIDUMP_EXCEPTION_INFORMATION, MiniDumpWithIndirectlyReferencedMemory,
        MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump,
        RtlCaptureStackBackTrace, SetUnhandledExceptionFilter,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, INFINITE,
        SetEvent, WaitForSingleObject,
    };

    /// Dumps kept in the runtime folder; older ones are removed at start.
    const DUMPS_KEPT: usize = 4;

    /// Allocation dumps a process may take and remove again, when their
    /// callers recover: past it, a null takes none.
    const ALLOC_DUMPS: u32 = 4;

    static GO: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
    static DONE: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
    static POINTERS: AtomicPtr<EXCEPTION_POINTERS> = AtomicPtr::new(std::ptr::null_mut());
    static THREAD: AtomicU32 = AtomicU32::new(0);
    /// What the dumping thread is asked for: a fault's dump, or an
    /// allocation's.
    static FOR_ALLOC: AtomicBool = AtomicBool::new(false);
    /// One dump at a time: held from the request until it's written.
    static BUSY: AtomicBool = AtomicBool::new(false);
    /// The fault's dump (faults and the main thread's panic), once.
    static DUMPED: AtomicBool = AtomicBool::new(false);
    /// The allocation's dump, and the thread whose null took it.
    static ALLOC_DUMPED: AtomicBool = AtomicBool::new(false);
    static ALLOC_THREAD: AtomicU32 = AtomicU32::new(0);
    static ALLOC_DUMPS_LEFT: AtomicU32 = AtomicU32::new(ALLOC_DUMPS);

    /// A dump's file: its wide, null-ended path for `CreateFileW`, made at
    /// the start (a process out of memory can't make it), and its name.
    struct DumpFile {
        wide: Vec<u16>,
        name: String,
    }

    impl DumpFile {
        fn new(path: PathBuf) -> Self {
            Self {
                wide: path.as_os_str().encode_wide().chain([0]).collect(),
                name: path.display().to_string(),
            }
        }
    }

    static DUMP: OnceLock<DumpFile> = OnceLock::new();
    static ALLOC_DUMP: OnceLock<DumpFile> = OnceLock::new();

    /// A module and the offset in it of `addr`, to find in its symbols:
    /// `name+0x…`.
    struct At(usize);

    impl std::fmt::Display for At {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let mut module: HANDLE = std::ptr::null_mut();
            // SAFETY: looks up the module holding an address, without
            // changing its reference count.
            let found = unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                    self.0 as *const u16,
                    &mut module,
                )
            } != 0;
            if !found {
                return write!(f, "{:#x}", self.0);
            }
            let mut name = [0u16; 260];
            // SAFETY: the buffer's length is given.
            let n = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) };
            let name = &name[..n as usize];
            let file = name
                .iter()
                .rposition(|&c| c == u16::from(b'\\'))
                .map_or(name, |i| &name[i + 1..]);
            for c in char::decode_utf16(file.iter().copied()) {
                f.write_char(c.unwrap_or('?'))?;
            }
            write!(f, "+{:#x}", self.0 - module as usize)
        }
    }

    /// This thread's stack, as `module+offset` frames, skipping `skip`.
    struct Stack(u32);

    impl std::fmt::Display for Stack {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let mut frames = [std::ptr::null_mut(); 24];
            // SAFETY: the buffer's length is given.
            let n = unsafe {
                RtlCaptureStackBackTrace(
                    self.0 + 1,
                    frames.len() as u32,
                    frames.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            };
            for (i, a) in frames[..usize::from(n)].iter().enumerate() {
                if i > 0 {
                    f.write_str(" < ")?;
                }
                write!(f, "{}", At(*a as usize))?;
            }
            Ok(())
        }
    }

    /// Asks the dumping thread for a dump and waits for it, one at a time
    /// (giving up after two minutes of another's). `pointers` is the
    /// exception's, or null for the threads as they are.
    fn request(pointers: *mut EXCEPTION_POINTERS, for_alloc: bool) {
        let mut waits = 0;
        while BUSY
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            waits += 1;
            if waits > 120_000 {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        POINTERS.store(pointers, Ordering::SeqCst);
        FOR_ALLOC.store(for_alloc, Ordering::SeqCst);
        // SAFETY: plain Win32 calls on the events `install` made.
        let waited = unsafe {
            THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
            SetEvent(GO.load(Ordering::SeqCst));
            WaitForSingleObject(DONE.load(Ordering::SeqCst), 120_000)
        };
        // A dump still being written after two minutes keeps the dumping
        // thread: no other is asked for.
        if waited == 0 {
            BUSY.store(false, Ordering::SeqCst);
        }
    }

    /// Writes the fault's minidump, once a process: from a thread of its
    /// own (the failing one may have no stack left), waiting for it.
    pub fn dump(pointers: *mut EXCEPTION_POINTERS, why: &str) {
        if GO.load(Ordering::SeqCst).is_null() || DUMPED.swap(true, Ordering::SeqCst) {
            return;
        }
        emergency(format_args!("writing a minidump for {why}"));
        request(pointers, false);
    }

    /// An allocation returned null on this thread: a dump of the process
    /// as it is, in case the caller can't recover and Rust fails fast,
    /// which nothing sees. Nothing is written to the trace (the null may
    /// not matter); [`alloc_recovered`] removes the dump if it didn't.
    pub fn alloc_dump() {
        if GO.load(Ordering::SeqCst).is_null() || ALLOC_DUMPED.swap(true, Ordering::SeqCst) {
            return;
        }
        if ALLOC_DUMPS_LEFT
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_err()
        {
            return;
        }
        // SAFETY: a plain Win32 call.
        ALLOC_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
        request(std::ptr::null_mut(), true);
    }

    /// The thread whose null took the allocation's dump allocated again:
    /// its caller recovered, so the dump goes and the next null may take
    /// one.
    pub fn alloc_recovered() {
        // SAFETY: a plain Win32 call.
        let me = unsafe { GetCurrentThreadId() };
        if !ALLOC_DUMPED.load(Ordering::SeqCst) || ALLOC_THREAD.load(Ordering::SeqCst) != me {
            return;
        }
        // Not while a dump is being written.
        if BUSY
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        if let Some(file) = ALLOC_DUMP.get() {
            // SAFETY: a null-ended path made at the start.
            unsafe { DeleteFileW(file.wide.as_ptr()) };
        }
        ALLOC_THREAD.store(0, Ordering::SeqCst);
        ALLOC_DUMPED.store(false, Ordering::SeqCst);
        BUSY.store(false, Ordering::SeqCst);
    }

    /// Writes `pointers`' dump to `file`, allocating nothing (the process
    /// may be out of memory): the error's number if it can't.
    fn write_dump(file: &DumpFile, pointers: *mut EXCEPTION_POINTERS) -> Result<(), u32> {
        let info = MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: THREAD.load(Ordering::SeqCst),
            ExceptionPointers: pointers,
            ClientPointers: 0,
        };
        // SAFETY: a null-ended path made at the start; the handle is
        // closed once written; `info` lives through the call, and the
        // failing thread waits on it.
        unsafe {
            let h = CreateFileW(
                file.wide.as_ptr(),
                GENERIC_WRITE,
                0,
                std::ptr::null(),
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                return Err(GetLastError());
            }
            let ok = MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                h,
                MiniDumpWithIndirectlyReferencedMemory
                    | MiniDumpWithThreadInfo
                    | MiniDumpWithUnloadedModules,
                if pointers.is_null() {
                    std::ptr::null()
                } else {
                    &info
                },
                std::ptr::null(),
                std::ptr::null(),
            ) != 0;
            let err = if ok { 0 } else { GetLastError() };
            CloseHandle(h);
            if ok { Ok(()) } else { Err(err) }
        }
    }

    /// The dumping thread: waits to be asked, writes, says so for a fault.
    fn dumper() {
        loop {
            // SAFETY: the event `install` made, never closed.
            unsafe { WaitForSingleObject(GO.load(Ordering::SeqCst), INFINITE) };
            let for_alloc = FOR_ALLOC.load(Ordering::SeqCst);
            let file = if for_alloc { &ALLOC_DUMP } else { &DUMP };
            if let Some(file) = file.get() {
                let written = write_dump(file, POINTERS.load(Ordering::SeqCst));
                // An allocation's dump says nothing: the null may not
                // matter, and when it does Rust says so on standard error.
                if !for_alloc {
                    match written {
                        Ok(()) => emergency(format_args!("minidump: {}", file.name)),
                        Err(e) => {
                            emergency(format_args!("minidump: couldn't be written (error {e})"))
                        }
                    }
                }
            }
            // SAFETY: the event `install` made.
            unsafe { SetEvent(DONE.load(Ordering::SeqCst)) };
        }
    }

    /// What every fault writes: its code, where, and what the window was
    /// doing; then the dump; then the module the address is in, which
    /// takes the loader's lock (another thread may hold it, and the dump
    /// comes first).
    unsafe fn fault(pointers: *mut EXCEPTION_POINTERS, how: &str) {
        // SAFETY: Windows passes valid pointers to its handlers.
        let record = unsafe { pointers.as_ref().and_then(|p| p.ExceptionRecord.as_ref()) };
        let (code, addr) = record.map_or((0, 0), |r| {
            (r.ExceptionCode as u32, r.ExceptionAddress as usize)
        });
        emergency(format_args!(
            "fault: {how} {code:#010x} at {addr:#x} on thread {}; {}",
            // SAFETY: a plain Win32 call.
            unsafe { GetCurrentThreadId() },
            Doing
        ));
        dump(pointers, "a fault");
        emergency(format_args!("fault: {addr:#x} is {}", At(addr)));
    }

    unsafe extern "system" fn unhandled(pointers: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: Windows's pointers, passed on.
        unsafe { fault(pointers.cast_mut(), "unhandled exception") };
        EXCEPTION_CONTINUE_SEARCH
    }

    unsafe extern "system" fn vectored(pointers: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: Windows passes valid pointers to its handlers.
        let code = unsafe { pointers.as_ref().and_then(|p| p.ExceptionRecord.as_ref()) }
            .map(|r| r.ExceptionCode);
        if code == Some(EXCEPTION_STACK_OVERFLOW) {
            // SAFETY: Windows's pointers, passed on.
            unsafe { fault(pointers, "stack overflow") };
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    /// The process detaching (`ExitProcess`, which `std::process::exit`
    /// calls) without `main`'s `exited` line: who called it, as frames.
    pub fn detaching() {
        if let Some(how) = exiting_unexpectedly() {
            emergency(format_args!("{how}; {}; from {}", Doing, Stack(0)));
        }
    }

    /// Old dumps out, a dump's path ready, the dumping thread started, and
    /// the handlers in.
    pub fn install() {
        if let Some(log) = trace_path()
            && let Some(dir) = log.parent()
        {
            let mut old: Vec<PathBuf> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("ryucad-app-") && n.ends_with(".dmp"))
                })
                .collect();
            old.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
            let extra = old.len().saturating_sub(DUMPS_KEPT - 1);
            for p in &old[..extra] {
                let _ = std::fs::remove_file(p);
            }
            let stem = format!("ryucad-app-{}-{}", now(), std::process::id());
            let _ = DUMP.set(DumpFile::new(dir.join(format!("{stem}.dmp"))));
            let _ = ALLOC_DUMP.set(DumpFile::new(dir.join(format!("{stem}-alloc.dmp"))));
        }
        // SAFETY: plain Win32 calls; the events live for the process.
        unsafe {
            GO.store(
                CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()),
                Ordering::SeqCst,
            );
            DONE.store(
                CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()),
                Ordering::SeqCst,
            );
        }
        if std::thread::Builder::new()
            .name("ryucad-app dumps".into())
            .spawn(dumper)
            .is_err()
        {
            GO.store(std::ptr::null_mut(), Ordering::SeqCst);
        }
        // SAFETY: the handlers are plain functions that live for the
        // process.
        unsafe {
            SetUnhandledExceptionFilter(Some(unhandled));
            AddVectoredExceptionHandler(1, Some(vectored));
        }
    }

    /// Private bytes and their peak.
    pub fn memory() -> Option<(u64, u64)> {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        // SAFETY: a zeroed counters struct is valid, and its size is given.
        unsafe {
            let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            let ok = GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(),
                c.cb,
            );
            (ok != 0).then_some((c.PrivateUsage as u64, c.PeakPagefileUsage as u64))
        }
    }

    /// Whether the process is in a job that kills it when the job's last
    /// handle closes (a terminal's, or an agent's shell's): such a window
    /// goes when they do, with nothing to say.
    pub fn job() -> &'static str {
        use windows_sys::Win32::System::JobObjects::{
            IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            QueryInformationJobObject,
        };
        let mut in_job = 0;
        // SAFETY: plain Win32 calls; a null job is the process's own.
        unsafe {
            if IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) == 0 {
                return "unknown";
            }
            if in_job == 0 {
                return "none";
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                std::ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            if ok == 0 {
                "a job"
            } else if info.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                != 0
            {
                "a job that kills it when closed"
            } else {
                "a job"
            }
        }
    }
}

/// Debug builds only: fails as `how` says, so tests can see each way of
/// ending leave its line (`tests/exits.rs`): `panic`, `alloc` (an
/// allocation that fails), `null` (one that returns null, recovered from,
/// then `exit(5)`), `exit` (`process::exit` from another thread),
/// `fault` (a write through null) and `overflow` (a stack overflow).
#[cfg(debug_assertions)]
pub fn fail_for_tests(how: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::{SEM_NOGPFAULTERRORBOX, SetErrorMode};
        // No Windows Error Reporting dialog to wait on in a test.
        // SAFETY: a plain Win32 call.
        unsafe { SetErrorMode(SEM_NOGPFAULTERRORBOX) };
    }
    match how {
        "panic" => panic!("a panic for the tests"),
        "alloc" => {
            // Black boxes: an allocation nothing uses may be left out.
            let size = std::hint::black_box(1usize << 62);
            let layout = std::alloc::Layout::from_size_align(size, 8).expect("a layout");
            // SAFETY: the layout's size isn't zero.
            let p = std::hint::black_box(unsafe { std::alloc::alloc(layout) });
            if p.is_null() {
                std::alloc::handle_alloc_error(layout);
            }
        }
        "null" => {
            // A null its caller recovers from, then an allocation, then
            // an ordinary end.
            let mut v: Vec<u8> = Vec::new();
            let size = std::hint::black_box(1usize << 62);
            assert!(v.try_reserve(size).is_err(), "a reserve that fails");
            v.push(1);
            // Past the time an abort would have come in: recovered.
            std::thread::sleep(Duration::from_millis(RECOVERED_AFTER_MS + 100));
            v.push(2);
            std::hint::black_box(&v);
            exit(5);
        }
        "exit" => {
            let _ = std::thread::spawn(|| std::process::exit(3)).join();
        }
        "fault" => {
            // SAFETY: none: the fault is the point.
            unsafe { std::ptr::null_mut::<u8>().write_volatile(1) };
        }
        "overflow" => {
            #[allow(unconditional_recursion)]
            fn deeper(n: u64) -> u64 {
                let pad = std::hint::black_box([n; 64]);
                deeper(n + 1) + pad[3]
            }
            let _ = std::thread::spawn(|| deeper(0)).join();
        }
        _ => {}
    }
}
