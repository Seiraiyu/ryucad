//! File events on Linux through inotify directly (#755).
//!
//! notify 8's inotify backend watches with `IN_OPEN` in its mask, and
//! offers no way to leave it out. A full check lists every folder of the
//! project, and each `opendir` raises `IN_OPEN` on the folder's own watch
//! and on its parent's: at design 192's benchmark (20,445 folders) about
//! 40,000 events, past the kernel's queue (`fs.inotify.max_queued_events`,
//! 16,384 by default), so every full check overflowed it and caused the
//! next, and every request checked all 107,500 files (190 ms). This
//! watcher asks for notify's mask less `IN_OPEN`: reading the project
//! raises nothing, and only changes are heard.

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// What the watcher reports, in the order the kernel gave it.
pub(super) enum Raw<'a> {
    /// The kernel's queue overflowed: events were lost.
    Overflow,
    /// A watch couldn't be added (a folder made since, past a limit).
    Failed,
    /// Something changed at this path (a file or a folder, a watched
    /// folder itself included).
    Path(&'a Path),
}

/// notify 8.2's mask, less `IN_OPEN`.
fn mask(top: bool) -> WatchMask {
    let mut m = WatchMask::ATTRIB
        | WatchMask::CREATE
        | WatchMask::DELETE
        | WatchMask::CLOSE_WRITE
        | WatchMask::MODIFY
        | WatchMask::MOVED_FROM
        | WatchMask::MOVED_TO;
    if top {
        m |= WatchMask::DELETE_SELF | WatchMask::MOVE_SELF;
    }
    m
}

/// How long the event thread waits for events before looking whether it
/// should stop.
const POLL_MS: i32 = 100;

struct Watches {
    inotify: Inotify,
    /// Each watch's folder, and whether folders made in it are watched.
    paths: HashMap<WatchDescriptor, (PathBuf, bool)>,
}

impl Watches {
    fn add(&mut self, path: &Path, top: bool, recursive: bool) -> std::io::Result<()> {
        let wd = self.inotify.watches().add(path, mask(top))?;
        self.paths.insert(wd, (path.to_path_buf(), recursive));
        Ok(())
    }

    /// Watches `dir` and, when `recursive`, every folder under it,
    /// following symbolic links to folders as notify does (each folder
    /// once).
    fn add_tree(&mut self, dir: &Path, top: bool, recursive: bool) -> std::io::Result<()> {
        self.add(dir, top, recursive)?;
        if !recursive {
            return Ok(());
        }
        let mut seen = HashSet::new();
        if let Ok(m) = fs::metadata(dir) {
            seen.insert((m.dev(), m.ino()));
        }
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = fs::read_dir(&d) else {
                continue; // gone since: its parent's event says so
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(m) = fs::metadata(&path) else { continue };
                if m.is_dir() && seen.insert((m.dev(), m.ino())) {
                    match self.add(&path, false, true) {
                        Ok(()) => stack.push(path),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(())
    }

    /// Forgets the watches of `dir` and the folders under it (moved away:
    /// if moved within the tree, its new place is watched again).
    fn forget(&mut self, dir: &Path) {
        self.paths.retain(|_, (p, _)| !p.starts_with(dir));
    }
}

/// A watch of some folders, reporting on a thread of its own until
/// dropped.
pub(super) struct Watcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Watches each `(folder, recursive)` (the first also for itself being
    /// removed or moved), calling `on` for each event. An error says why a
    /// folder can't be watched.
    pub(super) fn start(
        dirs: &[(PathBuf, bool)],
        mut on: impl FnMut(Raw) + Send + 'static,
    ) -> Result<Watcher, String> {
        let inotify = Inotify::init().map_err(|e| e.to_string())?;
        let mut w = Watches {
            inotify,
            paths: HashMap::new(),
        };
        for (k, (dir, recursive)) in dirs.iter().enumerate() {
            w.add_tree(dir, k == 0, *recursive)
                .map_err(|e| why(dir, &e))?;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("ryu-watch".into())
            .spawn(move || run(w, &stopping, &mut on))
            .map_err(|e| e.to_string())?;
        Ok(Watcher {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Why `dir` can't be watched, naming the limit to raise where the
/// kernel's limits refused it.
fn why(dir: &Path, e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(libc::ENOSPC) => format!(
            "{}: OS file watch limit reached (raise fs.inotify.max_user_watches)",
            dir.display()
        ),
        Some(libc::EMFILE) => format!(
            "{}: too many inotify instances (raise fs.inotify.max_user_instances, or stop idle \
             daemons)",
            dir.display()
        ),
        _ => format!("{}: {e}", dir.display()),
    }
}

fn run(mut w: Watches, stop: &AtomicBool, on: &mut dyn FnMut(Raw)) {
    let mut buffer = vec![0u8; 64 * 1024];
    let fd = w.inotify.as_raw_fd();
    while !stop.load(Ordering::Relaxed) {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd, owned here, for the inotify descriptor this
        // thread owns.
        let ready = unsafe { libc::poll(&mut pfd, 1, POLL_MS) };
        if ready <= 0 {
            continue; // a timeout, or EINTR
        }
        loop {
            let events: Vec<(WatchDescriptor, EventMask, Option<std::ffi::OsString>)> =
                match w.inotify.read_events(&mut buffer) {
                    Ok(events) => events
                        .map(|e| (e.wd, e.mask, e.name.map(OsStr::to_os_string)))
                        .collect(),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        on(Raw::Failed);
                        break;
                    }
                };
            if events.is_empty() {
                break;
            }
            for (wd, mask, name) in events {
                if mask.contains(EventMask::Q_OVERFLOW) {
                    on(Raw::Overflow);
                    continue;
                }
                let Some((dir, recursive)) = w.paths.get(&wd).cloned() else {
                    continue; // a watch forgotten: its folder moved away
                };
                if mask.contains(EventMask::IGNORED) {
                    w.paths.remove(&wd);
                    continue;
                }
                let path = match &name {
                    Some(n) => dir.join(n),
                    None => dir.clone(),
                };
                if mask.contains(EventMask::ISDIR) && recursive {
                    if mask.contains(EventMask::MOVED_FROM) {
                        w.forget(&path);
                    }
                    if mask.intersects(EventMask::CREATE | EventMask::MOVED_TO)
                        && let Err(e) = w.add_tree(&path, false, true)
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        on(Raw::Failed);
                    }
                }
                on(Raw::Path(&path));
            }
        }
    }
}
