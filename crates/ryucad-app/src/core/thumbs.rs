//! Thumbnails of recent projects (design 199, Start screen): a 256×192
//! iso render the window asks its project's daemon for after the
//! project's first full rebuild in a session, and again when it closes,
//! kept in the OS cache folder under `thumbnails/<BLAKE3 of the
//! canonical path>.png`, never in a project. A thumbnail is a cache: one
//! that can't be rendered or written is skipped, and the start screen
//! shows the project's initial instead. When it was written is when the
//! project was last open.

use ryucad::client::Daemon;
use ryucad::proto::Request;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A thumbnail's size, in pixels.
pub const SIZE: (u32, u32) = (256, 192);

/// The folder thumbnails live in: `thumbnails` in ryucad's cache folder
/// (`RYUCAD_CACHE_DIR` moves it, as it moves the disk cache).
pub fn folder() -> Option<PathBuf> {
    ryucad::cache::cache_dir()
        .ok()
        .map(|d| d.join("thumbnails"))
}

/// Where the thumbnail of the project at `root` lives: named by the
/// BLAKE3 of its canonical path, so two paths to one folder share one.
pub fn path_in(folder: &Path, root: &Path) -> PathBuf {
    let canonical = root
        .canonicalize()
        .map_or_else(|_| root.to_path_buf(), ryucad::doc::project::plain);
    let hash = blake3::hash(canonical.to_string_lossy().as_bytes());
    folder.join(format!("{}.png", hash.to_hex()))
}

/// A thumbnail read back: RGBA pixels and when it was written.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumbnail {
    pub size: (u32, u32),
    pub rgba: Vec<u8>,
    pub written: SystemTime,
}

/// Reads the thumbnail at `path`, if there is a readable one.
pub fn read(path: &Path) -> Option<Thumbnail> {
    let bytes = std::fs::read(path).ok()?;
    let written = std::fs::metadata(path).ok()?.modified().ok()?;
    let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut r = d.read_info().ok()?;
    let mut rgba = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut rgba).ok()?;
    rgba.truncate(info.buffer_size());
    Some(Thumbnail {
        size: (info.width, info.height),
        rgba,
        written,
    })
}

/// Writes `png` as the thumbnail at `path`: to a temporary file beside it,
/// then renamed, so a start screen never reads half of one.
pub fn write(path: &Path, png: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, png)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The request for a thumbnail: the whole project's model, iso, at
/// [`SIZE`], as an agent's `ryucad render` would draw it (nobody's
/// hidden set, sections or explode).
pub fn request() -> Request {
    let mut req: Request = serde_json::from_value(serde_json::json!({
        "method": "render",
        "params": {"view": "iso", "width": SIZE.0, "height": SIZE.1, "sketches": false},
    }))
    .expect("a render request");
    if let Request::Render {
        hidden,
        section,
        explode,
        ..
    } = &mut req
    {
        *hidden = Some(Vec::new());
        *section = Some(Vec::new());
        *explode = Some(String::new());
    }
    req
}

/// Asks the daemon of the project at `root` for its thumbnail and writes
/// it to `path`. Errors are the caller's to drop: a thumbnail is a cache.
pub fn make(daemon: &Daemon, root: &Path, path: &Path) -> Result<(), String> {
    let mut conn = daemon.connect(root).map_err(|e| e.message)?;
    let answer = conn
        .call_answer(&request())
        .map_err(|e| e.to_string())?
        .map_err(|e| e.message)?;
    let png = answer.binary.ok_or("the render carried no image")?;
    write(path, &png).map_err(|e| e.to_string())
}

/// Makes the thumbnail of `root` on a thread of its own, so a window
/// never waits for a render. Returns the thread, for a closing window to
/// wait on.
pub fn make_in_background(daemon: &Daemon, root: &Path) -> Option<std::thread::JoinHandle<()>> {
    let path = path_in(&folder()?, root);
    let (daemon, root) = (daemon.clone(), root.to_path_buf());
    Some(std::thread::spawn(move || {
        let _ = make(&daemon, &root, &path);
    }))
}

/// How long ago a thumbnail was written, as a card says it: `opened just
/// now`, `opened 5 min ago`, `opened 2 h ago`, `opened yesterday`,
/// `opened 3 days ago`. The app knows no time zone, so no clock times.
pub fn opened(written: SystemTime, now: SystemTime) -> String {
    let s = now
        .duration_since(written)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    match s {
        0..60 => "opened just now".to_owned(),
        60..3600 => format!("opened {} min ago", s / 60),
        3600..86_400 => format!("opened {} h ago", s / 3600),
        86_400..172_800 => "opened yesterday".to_owned(),
        _ => format!("opened {} days ago", s / 86_400),
    }
}

/// The letter a card without a thumbnail shows: the name's first letter
/// or digit, in capitals.
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_s_thumbnail_is_named_by_its_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bracket");
        std::fs::create_dir(&root).unwrap();
        let folder = dir.path().join("thumbnails");
        let a = path_in(&folder, &root);
        // Another way to the same folder: the same file.
        let b = path_in(&folder, &root.join("..").join("bracket"));
        assert_eq!(a, b);
        assert_eq!(a.parent(), Some(folder.as_path()));
        let name = a.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 64 + 4, "{name}");
        assert!(name.ends_with(".png"));
        assert_ne!(a, path_in(&folder, dir.path()));
    }

    #[test]
    fn a_thumbnail_written_reads_back_with_its_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t").join("x.png");
        assert_eq!(read(&path), None);
        // A 2×1 PNG: red, then blue.
        let mut png = Vec::new();
        {
            let mut e = png::Encoder::new(&mut png, 2, 1);
            e.set_color(png::ColorType::Rgb);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&[255, 0, 0, 0, 0, 255]).unwrap();
        }
        write(&path, &png).unwrap();
        let t = read(&path).unwrap();
        assert_eq!(t.size, (2, 1));
        assert_eq!(t.rgba, [255, 0, 0, 255, 0, 0, 255, 255]);
        let age = SystemTime::now().duration_since(t.written).unwrap();
        assert!(age < Duration::from_secs(60), "{age:?}");
        // Only the thumbnail is left: no temporary file.
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        // Something that isn't a PNG is no thumbnail.
        std::fs::write(&path, b"not a png").unwrap();
        assert_eq!(read(&path), None);
    }

    #[test]
    fn cards_say_when_the_project_was_open() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        let ago = |s: u64| opened(now - Duration::from_secs(s), now);
        assert_eq!(ago(5), "opened just now");
        assert_eq!(ago(5 * 60), "opened 5 min ago");
        assert_eq!(ago(2 * 3600 + 5), "opened 2 h ago");
        assert_eq!(ago(30 * 3600), "opened yesterday");
        assert_eq!(ago(3 * 86_400 + 10), "opened 3 days ago");
        // A clock set back is "just now", never a panic.
        assert_eq!(opened(now + Duration::from_secs(9), now), "opened just now");
    }

    #[test]
    fn a_card_without_a_thumbnail_shows_an_initial() {
        assert_eq!(initial("gearbox"), "G");
        assert_eq!(initial("_enclosure-v2"), "E");
        assert_eq!(initial("2-stroke"), "2");
        assert_eq!(initial("ölpumpe"), "Ö");
        assert_eq!(initial("--"), "?");
    }

    #[test]
    fn the_request_renders_the_whole_model_iso() {
        let v = serde_json::to_value(request()).unwrap();
        assert_eq!(v["method"], "render");
        let p = &v["params"];
        assert_eq!(p["view"], "iso");
        assert_eq!(
            (p["width"].clone(), p["height"].clone()),
            (256.into(), 192.into())
        );
        assert_eq!(p["hidden"], serde_json::json!([]));
        assert_eq!(p["section"], serde_json::json!([]));
        assert_eq!(p["explode"], "");
        assert_eq!(p["sketches"], false);
        assert!(p.get("part").is_none());
    }
}
