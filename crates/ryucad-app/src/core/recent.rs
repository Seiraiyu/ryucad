//! Recent projects and their thumbnails, for the start window. The list
//! is the `app.recent_projects` preference: the daemon puts a project
//! first when a window says hello, and the start window reads it with the
//! daemon's own code (with no project open there is no daemon to ask).

use crate::core::doctor::cli;
use crate::core::link::Wake;
use ryucad::client::Daemon;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};

/// The recent projects, newest first.
pub fn load() -> Vec<PathBuf> {
    parse(&ryucad::config::user("app.recent_projects"))
}

/// The paths in the preference's value.
pub fn parse(v: &Value) -> Vec<PathBuf> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().map(PathBuf::from))
        .collect()
}

/// A render of a project, `ryucad render` at thumbnail size: RGBA pixels,
/// or why there are none.
pub type Thumbnail = Result<(u32, u32, Vec<u8>), String>;

/// The size of a thumbnail.
pub const SIZE: (u32, u32) = (192, 144);

fn decode(png_bytes: &[u8]) -> Thumbnail {
    let mut d = png::Decoder::new(std::io::Cursor::new(png_bytes));
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut r = d.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("too large")?];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    Ok((info.width, info.height, buf))
}

/// Renders a project with the CLI in process (no daemon is started for a
/// thumbnail), through a temporary file of this process's own.
pub fn render(daemon: &Daemon, root: &Path) -> Thumbnail {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!("ryucad-thumb-{}-{n}.png", std::process::id()));
    let size = format!("{}x{}", SIZE.0, SIZE.1);
    let args = ["render", "--size", &size, "--out"]
        .map(str::to_owned)
        .into_iter()
        .chain([out.display().to_string()])
        .collect::<Vec<_>>();
    let r = cli(daemon, root, &args).map_err(|e| e.message);
    let bytes = r.and_then(|_| std::fs::read(&out).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&out);
    decode(&bytes?)
}

/// Thumbnails being made, one thread each, and those made.
pub struct Thumbnails {
    pub done: BTreeMap<PathBuf, Thumbnail>,
    tx: Sender<(PathBuf, Thumbnail)>,
    rx: Receiver<(PathBuf, Thumbnail)>,
}

impl Default for Thumbnails {
    fn default() -> Thumbnails {
        let (tx, rx) = std::sync::mpsc::channel();
        Thumbnails {
            done: BTreeMap::new(),
            tx,
            rx,
        }
    }
}

impl Thumbnails {
    /// Starts rendering every project in `roots` not rendered yet.
    pub fn request(&mut self, daemon: &Daemon, roots: &[PathBuf], wake: &Wake) {
        for root in roots {
            if self.done.contains_key(root) {
                continue;
            }
            // Marked so it is asked for once.
            self.done.insert(root.clone(), Err("rendering".into()));
            let (tx, daemon, root, wake) =
                (self.tx.clone(), daemon.clone(), root.clone(), wake.clone());
            std::thread::spawn(move || {
                let t = render(&daemon, &root);
                let _ = tx.send((root, t));
                wake();
            });
        }
    }

    /// Takes finished renders; returns whether any arrived.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((root, t)) = self.rx.try_recv() {
            self.done.insert(root, t);
            any = true;
        }
        any
    }
}
