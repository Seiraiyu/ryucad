//! A job in memory: the files of an ODB++ product model, from a folder or
//! from a tar, gzipped tar or zip, found by their paths inside the job.
//!
//! The job is the folder holding `matrix/matrix`: the container's root, or
//! its one top folder. Paths are matched without regard to case (writers
//! disagree), and a file may be stored compressed on its own (`.Z` or
//! `.gz`), which is undone when it is read. Archives are read whole into
//! memory and refused when unsafe: an absolute path, `..`, a link or a
//! device, or more entries or bytes than [`Limits`] allow.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use super::error::{Code, Error};
use super::lzw;

/// What a job came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Container {
    Folder,
    Tar,
    Tgz,
    Zip,
}

impl Container {
    pub fn as_str(self) -> &'static str {
        match self {
            Container::Folder => "folder",
            Container::Tar => "tar",
            Container::Tgz => "tgz",
            Container::Zip => "zip",
        }
    }
}

/// How much an archive may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Entries in the container, folders included.
    pub entries: usize,
    /// Bytes of all its files, unpacked from the container.
    pub bytes: u64,
    /// Bytes one singly compressed file (`.Z`, `.gz`) may unpack to: past
    /// it, that file fails to read and the rest of the job loads.
    pub file_bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            entries: 1_000_000,
            bytes: 8 << 30,
            file_bytes: 2 << 30,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Packing {
    Plain,
    Lzw,
    Gzip,
}

#[derive(Clone, Debug)]
struct Entry {
    /// The path inside the job as stored, compression suffix and all.
    path: String,
    packing: Packing,
    data: Vec<u8>,
}

/// An ODB++ job's files.
#[derive(Clone, Debug)]
pub struct Job {
    pub container: Container,
    /// The top folder the job sat in, as stored, if not at the root.
    pub top: Option<String>,
    /// The file's or folder's name without its extension.
    pub stem: String,
    files: BTreeMap<String, Entry>,
    file_bytes: u64,
}

impl Job {
    /// Reads a folder, or an archive file, at `path`.
    pub fn open(path: &Path, limits: Limits) -> Result<Job, Error> {
        let stem = stem(path);
        // The path itself may be a link; links inside a folder are refused.
        let meta = std::fs::metadata(path)
            .map_err(|e| Error::new(Code::NotABoard, format!("{}: {e}", path.display())))?;
        if meta.is_dir() {
            let mut files = Vec::new();
            walk(path, "", &mut files, limits)?;
            Job::from_files(Container::Folder, &stem, files, limits)
        } else {
            let bytes = std::fs::read(path)
                .map_err(|e| Error::new(Code::NotABoard, format!("{}: {e}", path.display())))?;
            Job::from_archive(&stem, &bytes, limits)
        }
    }

    /// Reads an archive from its bytes, by what they start with: gzip
    /// (a gzipped tar), zip, or tar.
    pub fn from_archive(stem: &str, bytes: &[u8], limits: Limits) -> Result<Job, Error> {
        if bytes.starts_with(&[0x1f, 0x8b]) {
            let gz = flate2::read::MultiGzDecoder::new(bytes);
            let files = untar(gz, limits)?;
            Job::from_files(Container::Tgz, stem, files, limits)
        } else if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
            let files = unzip(bytes, limits)?;
            Job::from_files(Container::Zip, stem, files, limits)
        } else if bytes.len() >= 512 && &bytes[257..262] == b"ustar" {
            let files = untar(bytes, limits)?;
            Job::from_files(Container::Tar, stem, files, limits)
        } else {
            Err(Error::new(
                Code::NotABoard,
                "not a folder, tar, gzipped tar or zip",
            ))
        }
    }

    /// A job from its container's files: paths relative to the container,
    /// `/` between folders.
    pub fn from_files(
        container: Container,
        stem: &str,
        files: Vec<(String, Vec<u8>)>,
        limits: Limits,
    ) -> Result<Job, Error> {
        // Where matrix/matrix is: the root, else one top folder.
        let mut tops = BTreeSet::new();
        let mut root = false;
        for (path, _) in &files {
            let (key, _) = key_of(path);
            if key == "matrix/matrix" {
                root = true;
            } else if let Some((top, rest)) = key.split_once('/')
                && rest == "matrix/matrix"
            {
                tops.insert(top.to_string());
            }
        }
        let top_key = match (root, tops.len()) {
            (true, _) => None,
            (false, 1) => tops.pop_first(),
            (false, 0) => {
                return Err(Error::new(
                    Code::NotABoard,
                    "no matrix/matrix at the root or under one top folder",
                ));
            }
            (false, _) => {
                return Err(Error::new(
                    Code::NotABoard,
                    format!(
                        "more than one job: matrix/matrix under {}",
                        tops.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                ));
            }
        };
        let mut top = None;
        let mut map = BTreeMap::new();
        for (path, data) in files {
            let rel = match &top_key {
                None => path.as_str(),
                Some(t) => match path.split_once('/') {
                    Some((first, rest)) if first.to_ascii_lowercase() == *t => {
                        top.get_or_insert_with(|| first.to_string());
                        rest
                    }
                    _ => continue,
                },
            };
            if rel.is_empty() {
                continue;
            }
            let (key, packing) = key_of(rel);
            let entry = Entry {
                path: rel.to_string(),
                packing,
                data,
            };
            if let Some(old) = map.insert(key, entry) {
                return Err(Error::new(
                    Code::NotABoard,
                    format!("{} and {} are one file", old.path, rel),
                ));
            }
        }
        Ok(Job {
            container,
            top,
            stem: stem.to_string(),
            files: map,
            file_bytes: limits.file_bytes,
        })
    }

    /// The file at `path` inside the job (any case, without a compression
    /// suffix), unpacked; `None` if there is none.
    pub fn read(&self, path: &str) -> Option<Result<Vec<u8>, String>> {
        let e = self.files.get(&path.to_ascii_lowercase())?;
        Some(match e.packing {
            Packing::Plain => Ok(e.data.clone()),
            Packing::Lzw => lzw::decompress(&e.data, self.file_bytes as usize)
                .map_err(|err| format!("{}: {err}", e.path)),
            Packing::Gzip => {
                let mut out = Vec::new();
                let gz = flate2::read::MultiGzDecoder::new(e.data.as_slice());
                match gz.take(self.file_bytes + 1).read_to_end(&mut out) {
                    Ok(n) if n as u64 > self.file_bytes => {
                        Err(format!("{}: unpacks too large", e.path))
                    }
                    Ok(_) => Ok(out),
                    Err(err) => Err(format!("{}: {err}", e.path)),
                }
            }
        })
    }

    /// The path of `path` as stored in the job, compression suffix and
    /// case as written, for messages.
    pub fn stored(&self, path: &str) -> String {
        match self.files.get(&path.to_ascii_lowercase()) {
            Some(e) => e.path.clone(),
            None => path.to_string(),
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.files.contains_key(&path.to_ascii_lowercase())
    }

    /// The names (lower case) of the files and folders directly in `dir`.
    pub fn children(&self, dir: &str) -> BTreeSet<String> {
        let prefix = format!("{}/", dir.trim_end_matches('/').to_ascii_lowercase());
        self.files
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .map(|(k, _)| {
                k[prefix.len()..]
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .collect()
    }

    /// Every file's path (lower case, without compression suffixes).
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

/// The lower-case lookup key of a stored path, and how it is packed.
fn key_of(path: &str) -> (String, Packing) {
    let lower = path.to_ascii_lowercase();
    if let Some(k) = lower.strip_suffix(".z") {
        (k.to_string(), Packing::Lzw)
    } else if let Some(k) = lower.strip_suffix(".gz") {
        (k.to_string(), Packing::Gzip)
    } else {
        (lower, Packing::Plain)
    }
}

fn stem(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    for ext in [".tar.gz", ".tgz", ".tar", ".zip"] {
        if lower.ends_with(ext) {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    name
}

fn unsafe_(message: String) -> Error {
    Error::new(Code::ArchiveUnsafe, message)
}

/// `name` made a relative path with `/` between folders, or refused.
fn safe_path(name: &str) -> Result<String, Error> {
    let p = name.replace('\\', "/");
    let drive = p.len() >= 2 && p.as_bytes()[1] == b':';
    if p.starts_with('/') || drive {
        return Err(unsafe_(format!("{name}: an absolute path")));
    }
    let parts: Vec<&str> = p
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if parts.contains(&"..") {
        return Err(unsafe_(format!("{name}: a path through ..")));
    }
    Ok(parts.join("/"))
}

struct Budget {
    limits: Limits,
    entries: usize,
    bytes: u64,
}

impl Budget {
    fn new(limits: Limits) -> Budget {
        Budget {
            limits,
            entries: 0,
            bytes: 0,
        }
    }

    fn entry(&mut self, size: u64) -> Result<(), Error> {
        self.entries += 1;
        if self.entries > self.limits.entries {
            return Err(unsafe_(format!(
                "more than {} entries",
                self.limits.entries
            )));
        }
        self.more(size)
    }

    fn more(&mut self, size: u64) -> Result<(), Error> {
        self.bytes = self.bytes.saturating_add(size);
        if self.bytes > self.limits.bytes {
            return Err(unsafe_(format!(
                "more than {} bytes unpacked",
                self.limits.bytes
            )));
        }
        Ok(())
    }

    /// What is left for the next file.
    fn left(&self) -> u64 {
        self.limits.bytes - self.bytes
    }
}

fn untar(r: impl Read, limits: Limits) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let bad = |e: std::io::Error| Error::new(Code::NotABoard, format!("not a readable tar: {e}"));
    let mut archive = tar::Archive::new(r);
    let mut budget = Budget::new(limits);
    let mut out = Vec::new();
    for entry in archive.entries().map_err(bad)? {
        let mut entry = entry.map_err(bad)?;
        let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let kind = entry.header().entry_type();
        let path = safe_path(&name)?;
        if kind.is_dir() {
            budget.entry(0)?;
            continue;
        }
        if !kind.is_file() {
            return Err(unsafe_(format!("{name}: a link or device")));
        }
        let size = entry.size();
        budget.entry(size)?;
        let mut data = Vec::with_capacity(size.min(budget.left()) as usize);
        entry.read_to_end(&mut data).map_err(bad)?;
        out.push((path, data));
    }
    Ok(out)
}

fn unzip(bytes: &[u8], limits: Limits) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let bad =
        |e: zip::result::ZipError| Error::new(Code::NotABoard, format!("not a readable zip: {e}"));
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(bad)?;
    if archive.len() > limits.entries {
        return Err(unsafe_(format!("more than {} entries", limits.entries)));
    }
    let mut budget = Budget::new(limits);
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(bad)?;
        let name = String::from_utf8_lossy(f.name_raw()).into_owned();
        let path = safe_path(&name)?;
        if f.is_symlink() {
            return Err(unsafe_(format!("{name}: a link")));
        }
        if f.is_dir() {
            budget.entry(0)?;
            continue;
        }
        // The size the zip claims may lie: read at most what is left.
        budget.entry(f.size())?;
        let left = budget.left();
        let mut data = Vec::new();
        let n = (&mut f)
            .take(left + 1)
            .read_to_end(&mut data)
            .map_err(|e| Error::new(Code::NotABoard, format!("{name}: {e}")))?;
        if n as u64 > f.size() {
            budget.more(n as u64 - f.size())?;
        }
        out.push((path, data));
    }
    Ok(out)
}

/// A folder's files, refusing links.
fn walk(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, Vec<u8>)>,
    limits: Limits,
) -> Result<(), Error> {
    let io = |e: std::io::Error| Error::new(Code::NotABoard, format!("{}: {e}", dir.display()));
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .map_err(io)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<Result<_, _>>()
        .map_err(io)?;
    names.sort();
    for name in names {
        let name = name.to_string_lossy().into_owned();
        let path = dir.join(&name);
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let meta = std::fs::symlink_metadata(&path).map_err(io)?;
        if out.len() >= limits.entries {
            return Err(unsafe_(format!("more than {} files", limits.entries)));
        }
        if meta.file_type().is_symlink() {
            return Err(unsafe_(format!("{rel}: a link")));
        } else if meta.is_dir() {
            walk(&path, &rel, out, limits)?;
        } else if meta.is_file() {
            out.push((rel, std::fs::read(&path).map_err(io)?));
        } else {
            return Err(unsafe_(format!("{rel}: a device")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn files() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("odb/matrix/matrix", b"STEP {\n}\n"),
            ("odb/misc/info", b"UNITS=MM\n"),
            ("odb/steps/pcb/layers/top/features", b"UNITS=MM\n"),
        ]
    }

    fn tar_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            b.append_data(&mut h, path, *data).unwrap();
        }
        b.into_inner().unwrap()
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (path, data) in files {
            w.start_file(*path, o).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn read(job: &Job, path: &str) -> Vec<u8> {
        job.read(path).unwrap().unwrap()
    }

    #[test]
    fn every_container_holds_the_same_job() {
        let f = files();
        let tar = tar_of(&f);
        let jobs = [
            Job::from_archive("j", &tar, Limits::default()).unwrap(),
            Job::from_archive("j", &gzip(&tar), Limits::default()).unwrap(),
            Job::from_archive("j", &zip_of(&f), Limits::default()).unwrap(),
        ];
        let kinds: Vec<_> = jobs.iter().map(|j| j.container).collect();
        assert_eq!(kinds, [Container::Tar, Container::Tgz, Container::Zip]);
        for j in &jobs {
            assert_eq!(j.top.as_deref(), Some("odb"));
            assert_eq!(read(j, "misc/info"), b"UNITS=MM\n");
            assert_eq!(read(j, "MISC/Info"), b"UNITS=MM\n");
            assert!(j.read("misc/nothing").is_none());
            assert_eq!(
                j.children("steps/pcb/layers"),
                BTreeSet::from(["top".to_string()])
            );
            assert_eq!(j.paths().count(), 3);
        }
    }

    #[test]
    fn folders_and_their_tops() {
        let dir = tempfile::tempdir().unwrap();
        for (path, data) in files() {
            let p = dir.path().join("Board.odb").join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, data).unwrap();
        }
        // The folder holding the top folder, and the job folder itself.
        let outer = Job::open(&dir.path().join("Board.odb"), Limits::default()).unwrap();
        assert_eq!(
            (outer.container, outer.top.as_deref()),
            (Container::Folder, Some("odb"))
        );
        assert_eq!(outer.stem, "Board.odb");
        let inner = Job::open(&dir.path().join("Board.odb/odb"), Limits::default()).unwrap();
        assert_eq!(inner.top, None);
        assert_eq!(read(&inner, "steps/pcb/layers/top/features"), b"UNITS=MM\n");
        assert_eq!(stem(Path::new("/x/job.tar.gz")), "job");
        assert_eq!(stem(Path::new("job.TGZ")), "job");
        assert_eq!(stem(Path::new("b.zip")), "b");
    }

    #[test]
    fn files_compressed_singly_are_unpacked() {
        let features = b"UNITS=INCH\nL 0 0 1 1 0 P 0\n".repeat(50);
        let gz = gzip(&features);
        let f: Vec<(&str, &[u8])> = vec![
            ("matrix/matrix", b""),
            ("steps/pcb/layers/top/features.gz", &gz),
            ("steps/pcb/layers/bot/features.Z", b"\x1f\x9d\x90\x41\x00"),
            ("steps/pcb/layers/bad/features.gz", b"\x1f\x8b\x08\x00junk"),
        ];
        let j = Job::from_archive("j", &zip_of(&f), Limits::default()).unwrap();
        assert_eq!(read(&j, "steps/pcb/layers/top/features"), features);
        assert_eq!(read(&j, "steps/pcb/layers/bot/features"), b"A");
        assert_eq!(
            j.stored("steps/pcb/layers/bot/features"),
            "steps/pcb/layers/bot/features.Z"
        );
        assert!(j.read("steps/pcb/layers/bad/features").unwrap().is_err());
        // A file that unpacks past its limit fails alone.
        let small = Limits {
            file_bytes: 100,
            ..Limits::default()
        };
        let j = Job::from_archive("j", &zip_of(&f), small).unwrap();
        let e = j
            .read("steps/pcb/layers/top/features")
            .unwrap()
            .unwrap_err();
        assert!(e.contains("too large"), "{e}");
    }

    #[test]
    fn what_isnt_a_job() {
        let code = |r: Result<Job, Error>| r.unwrap_err().code;
        assert_eq!(
            code(Job::from_archive("j", b"hello", Limits::default())),
            Code::NotABoard
        );
        let none = tar_of(&[("readme.txt", b"hi")]);
        assert_eq!(
            code(Job::from_archive("j", &none, Limits::default())),
            Code::NotABoard
        );
        let two = tar_of(&[("a/matrix/matrix", b""), ("b/matrix/matrix", b"")]);
        let e = Job::from_archive("j", &two, Limits::default()).unwrap_err();
        assert!(e.message.contains("a, b"), "{}", e.message);
        let twice = tar_of(&[("matrix/matrix", b""), ("Matrix/Matrix", b"")]);
        assert_eq!(
            code(Job::from_archive("j", &twice, Limits::default())),
            Code::NotABoard
        );
        // The root wins over a top folder; files beside the top are left.
        let both = tar_of(&[("matrix/matrix", b"r"), ("x/matrix/matrix", b"x")]);
        let j = Job::from_archive("j", &both, Limits::default()).unwrap();
        assert_eq!(read(&j, "matrix/matrix"), b"r");
        let beside = tar_of(&[("x/matrix/matrix", b"x"), ("readme", b"")]);
        assert_eq!(
            Job::from_archive("j", &beside, Limits::default())
                .unwrap()
                .paths()
                .count(),
            1
        );
    }

    #[test]
    fn unsafe_archives_are_refused() {
        let code = |bytes: &[u8], l: Limits| Job::from_archive("j", bytes, l).unwrap_err().code;
        let d = Limits::default();
        assert_eq!(
            safe_path("/etc/passwd").unwrap_err().code,
            Code::ArchiveUnsafe
        );
        assert_eq!(
            safe_path("C:\\job\\x").unwrap_err().code,
            Code::ArchiveUnsafe
        );
        assert_eq!(
            safe_path("odb/../../x").unwrap_err().code,
            Code::ArchiveUnsafe
        );
        assert_eq!(
            safe_path("odb\\.\\matrix//matrix").unwrap(),
            "odb/matrix/matrix"
        );
        // tar's builder refuses `..`, so write the name into the header.
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_entry_type(tar::EntryType::Regular);
        h.as_gnu_mut().unwrap().name[..12].copy_from_slice(b"../matrix/mx");
        h.set_cksum();
        let mut evil = h.as_bytes().to_vec();
        evil.extend([b'x'; 512]);
        evil.extend([0; 1024]);
        assert_eq!(code(&evil, d), Code::ArchiveUnsafe);
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        b.append_link(&mut h, "matrix/matrix", "/etc/passwd")
            .unwrap();
        assert_eq!(code(&b.into_inner().unwrap(), d), Code::ArchiveUnsafe);
        let f = files();
        let few = Limits { entries: 2, ..d };
        assert_eq!(code(&tar_of(&f), few), Code::ArchiveUnsafe);
        assert_eq!(code(&zip_of(&f), few), Code::ArchiveUnsafe);
        let small = Limits { bytes: 10, ..d };
        assert_eq!(code(&gzip(&tar_of(&f)), small), Code::ArchiveUnsafe);
        assert_eq!(code(&zip_of(&f), small), Code::ArchiveUnsafe);
        assert!(Job::from_archive("j", &zip_of(&f), Limits { bytes: 50, ..d }).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn folders_with_links_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("matrix")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("matrix/matrix")).unwrap();
        assert_eq!(
            Job::open(dir.path(), Limits::default()).unwrap_err().code,
            Code::ArchiveUnsafe
        );
    }
}
