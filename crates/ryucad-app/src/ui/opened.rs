//! Folders the desktop asks the app to open while it runs or as it
//! starts: Finder's Open With and a folder dropped on the Dock icon, which
//! reach the bundle's executable as an Apple event, not as an argument
//! (#1022). Each is queued here and taken by the next frame: the start
//! window opens it as the project, and with a project open it goes to
//! `ryucad open`, which brings that folder's window forward or starts one,
//! as the launcher did (#411).

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static WAKE: Mutex<Option<egui::Context>> = Mutex::new(None);

/// Queues `paths` to open, and wakes the window if it is up.
pub fn push(paths: impl IntoIterator<Item = PathBuf>) {
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend(paths);
    if let Some(ctx) = WAKE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        ctx.request_repaint();
    }
}

/// The window to wake when a folder is queued.
pub fn wake_with(ctx: &egui::Context) {
    *WAKE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ctx.clone());
}

/// Every folder queued since the last call, in the order they came.
pub fn take() -> Vec<PathBuf> {
    std::mem::take(
        &mut *PENDING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Where a queued folder goes.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Into this window, which shows the start window.
    Here(PathBuf),
    /// To `ryucad open`.
    Elsewhere(PathBuf),
}

/// Where each of `paths` goes: the first into this window if it shows
/// the start window (`start`), every other to `ryucad open`.
pub fn route(start: bool, paths: Vec<PathBuf>) -> Vec<Route> {
    let mut here = start;
    paths
        .into_iter()
        .map(|p| {
            if std::mem::take(&mut here) {
                Route::Here(p)
            } else {
                Route::Elsewhere(p)
            }
        })
        .collect()
}

/// `ryucad open dir`, by `ryucad` at `exe`, this app the window it starts:
/// not waited on, and nothing said if it can't start (the folder is the
/// desktop's, and `ryucad open` reports its own faults in its window).
pub fn elsewhere(exe: &Path, dir: &Path) {
    let mut c = std::process::Command::new(exe);
    c.arg("open").arg(dir);
    if let Ok(me) = std::env::current_exe() {
        c.env("RYUCAD_APP", me);
    }
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = c.spawn();
}

/// On macOS, takes the desktop's open requests (`application:openURLs:`)
/// as the app's own delegate. Call before the event loop starts: the
/// delegate is set as AppKit finishes launching, before it hands over the
/// folders a launch by Open With was for. winit sets no delegate of its
/// own.
#[cfg(target_os = "macos")]
pub fn listen() {
    mac::listen();
}

/// Elsewhere the folder comes as an argument.
#[cfg(not(target_os = "macos"))]
pub fn listen() {}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSApplication, NSApplicationDelegate, NSApplicationWillFinishLaunchingNotification,
    };
    use objc2_foundation::{NSArray, NSNotification, NSNotificationCenter, NSURL};

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and the
        // delegate holds no state of its own.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "RyucadAppDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl NSApplicationDelegate for Delegate {
            #[unsafe(method(application:openURLs:))]
            fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
                super::push(urls.iter().filter_map(|u| u.to_file_path()));
            }
        }

        impl Delegate {
            #[unsafe(method(willFinishLaunching:))]
            fn will_finish_launching(&self, _note: &NSNotification) {
                let app = NSApplication::sharedApplication(self.mtm());
                app.setDelegate(Some(ProtocolObject::from_ref(self)));
            }
        }
    );

    pub(super) fn listen() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // SAFETY: `init` on a fresh NSObject subclass with no ivars.
        let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(mtm), init] };
        // SAFETY: the selector is the delegate's own method above, taking
        // the notification; the name is AppKit's.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &delegate,
                sel!(willFinishLaunching:),
                Some(NSApplicationWillFinishLaunchingNotification),
                None,
            );
        }
        // Neither the notification centre nor the application keeps it: it
        // lives as long as the process.
        std::mem::forget(delegate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_window_takes_the_first_folder_and_ryucad_open_the_rest() {
        let (a, b) = (PathBuf::from("/a"), PathBuf::from("/b"));
        assert_eq!(
            route(true, vec![a.clone(), b.clone()]),
            [Route::Here(a.clone()), Route::Elsewhere(b.clone())]
        );
        assert_eq!(
            route(false, vec![a.clone(), b.clone()]),
            [Route::Elsewhere(a), Route::Elsewhere(b)]
        );
        assert!(route(true, Vec::new()).is_empty());
    }

    #[test]
    fn queued_folders_are_taken_once_in_order() {
        push([PathBuf::from("/x"), PathBuf::from("/y")]);
        push([PathBuf::from("/z")]);
        assert_eq!(take(), ["/x", "/y", "/z"].map(PathBuf::from));
        assert!(take().is_empty());
    }
}
