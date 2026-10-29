//! `import`: an ODB++ job read whole and checked, then committed as a
//! board as it was given, byte for byte; with `update`, the next revision
//! of a board, keeping its ID, name and step.
//!
//! Nothing is written until the job has been read: a path that isn't a
//! job, or a job the reader refuses, writes nothing. A dry run reads and
//! answers the same and writes nothing.

use crate::codes;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::input::{Container, Input};
use crate::odb::{self, Job, Limits, ReadOptions};
use crate::project::check_revision;
use crate::project::{BOARD_FILE, BoardFile, Format, InputRef, SourceInfo, board_dir};
use crate::session::{Session, path_of};
use crate::source::{join, write_atomic};
use rayon::prelude::*;
use ryu_board::model::Board;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// What to import.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The job: a folder, `.tgz`, `.tar` or `.zip`, absolute.
    pub path: PathBuf,
    /// The new board's ID; from the file's name when `None`.
    pub board: Option<String>,
    /// The board step, where the rule can't choose one (or, updating, to
    /// choose another).
    pub step: Option<String>,
    /// The board this is the next revision of.
    pub update: Option<String>,
    pub dry_run: bool,
}

/// A job read from a path, with the bytes to commit.
pub struct Read {
    /// The file's or folder's name less an archive's extension: what the
    /// board is called, since exporters write the job's name generically
    /// (KiCad always `job`, Altium none).
    pub stem: String,
    pub input: Input,
    pub container: Container,
    pub board: Board,
}

/// Reads the job at `path` with `step` as the board step, keeping its
/// bytes as given.
pub fn read(path: &std::path::Path, step: Option<&str>) -> Result<Read> {
    let meta = std::fs::metadata(path).map_err(|e| {
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            Code::NotFound
        } else {
            Code::Io
        };
        Error::new(code, format!("{}: {e}", path.display())).with("path", path)
    })?;
    let limits = Limits::default();
    let stem = odb::job::stem(path);
    let refused = |e: odb::Error| codes::error(&e).with("path", path);
    let (input, job) = if meta.is_dir() {
        let files = odb::job::folder_files(path, limits).map_err(refused)?;
        let input = Input::Folder(
            files
                .iter()
                .map(|(p, b)| (p.clone(), Arc::from(&b[..])))
                .collect(),
        );
        let job = Job::from_files(odb::Container::Folder, &stem, files, limits).map_err(refused)?;
        (input, job)
    } else {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, &e))?;
        let job = Job::from_archive(&stem, &bytes, limits).map_err(refused)?;
        (Input::Archive(bytes.into()), job)
    };
    let options = ReadOptions {
        step: step.map(str::to_owned),
        limits,
    };
    let board = odb::read(&job, &options).map_err(refused)?;
    Ok(Read {
        stem,
        container: Container::of(job.container),
        input,
        board,
    })
}

/// The files a board's folder holds for `input`, relative to the
/// project.
fn input_files(dir: &str, container: Container, input: &Input) -> Vec<(String, Arc<[u8]>)> {
    let at = join(dir, container.file_name());
    match input {
        Input::Archive(b) => vec![(at, Arc::clone(b))],
        Input::Folder(files) => files
            .iter()
            .map(|(p, b)| (join(&at, p), Arc::clone(b)))
            .collect(),
    }
}

/// A board's counts, for answers.
pub fn summary(b: &Board) -> Value {
    let step = &b.steps[b.board_step];
    let features: usize = step.layers.iter().map(|f| f.features().len()).sum();
    json!({
        "name": b.name,
        "source": {
            "format": b.source.format,
            "version": b.source.version,
            "application": b.source.application,
        },
        "steps": b.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        "step": step.name,
        "layers": b.layers.len(),
        "features": features,
        "components": step.components.len(),
        "nets": step.nets.len(),
        "capabilities": crate::inspect::capabilities(&b.capabilities),
    })
}

/// Imports a job: a new board, or with `update` the next revision of one.
/// Guarded by the project's revision for a new board and by the board's
/// for an update.
pub fn import(session: &mut Session, o: &Options, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    let kept = match &o.update {
        Some(b) => {
            if o.board.is_some() {
                return Err(Error::new(
                    Code::InvalidParam,
                    "--board names a new board; an update keeps its board's ID",
                )
                .with("param", "board"));
            }
            let e = p.board(b)?;
            check_revision(&format!("board {b}"), &e.revision, expected)?;
            Some((e.id.clone(), e.file.clone()?))
        }
        None => {
            check_revision("the project", &p.revision, expected)?;
            None
        }
    };
    let step = o
        .step
        .clone()
        .or_else(|| kept.as_ref().map(|(_, f)| f.step.clone()));
    let read = read(&o.path, step.as_deref())?;
    let p = session.project();
    let id = match (&kept, &o.board) {
        (Some((id, _)), _) => id.clone(),
        (None, Some(b)) => {
            let id = Id::parse(b, "board")?;
            if p.boards.contains_key(&id) {
                return Err(Error::new(
                    Code::DuplicateId,
                    format!("the project already has a board {b}; import --update {b} replaces it"),
                )
                .with("board", b));
            }
            id
        }
        (None, None) => free(&read.stem, |s| p.boards.keys().any(|k| k.as_str() == s)),
    };
    let hash = read.input.hash();
    let b = &read.board;
    let file = BoardFile {
        name: kept
            .as_ref()
            .map_or_else(|| read.stem.clone(), |(_, f)| f.name.clone()),
        input: InputRef {
            path: read.container.file_name().into(),
            blake3: hash.clone(),
            format: Format::Odb,
            container: read.container,
        },
        step: b.steps[b.board_step].name.clone(),
        source: SourceInfo {
            version: b.source.version.clone(),
            application: b.source.application.clone(),
        },
    };
    let dir = board_dir(id.as_str());
    let mut answer = json!({
        "board": id,
        "update": kept.is_some(),
        "dry_run": o.dry_run,
        "file": serde_json::from_str::<Value>(&file.to_text()).expect("JSON"),
        "summary": summary(b),
        "faults": b.faults.iter().map(codes::notice).collect::<Vec<_>>(),
        "warnings": b.warnings.iter().map(codes::notice).collect::<Vec<_>>(),
    });
    if o.dry_run {
        return Ok(answer);
    }
    let root = session.root().to_path_buf();
    let new = input_files(&dir, read.container, &read.input);
    let stale: Vec<String> = session
        .snapshot()
        .keys()
        .filter(|k| k.starts_with(&format!("{dir}/")) && k.as_str() != join(&dir, BOARD_FILE))
        .filter(|k| !new.iter().any(|(n, _)| n == *k))
        .cloned()
        .collect();
    // The input, then the file that names it, then what the input
    // replaced: a crash part way leaves at worst a board whose input
    // isn't the one its file names, which says so.
    new.par_iter()
        .map(|(rel, bytes)| write_atomic(&path_of(&root, rel), bytes))
        .collect::<Result<Vec<()>>>()?;
    write_atomic(&path_of(&root, &join(&dir, BOARD_FILE)), file.to_text())?;
    for rel in &stale {
        crate::session::remove_file(&root, &path_of(&root, rel))?;
    }
    session.reload()?;
    let p = session.project();
    answer["revision"] = json!(p.board(id.as_str())?.revision);
    answer["project_revision"] = json!(p.revision);
    Ok(answer)
}

/// The board ID for a job in a file called `name`: its readable form, or with
/// `-2`, `-3`, … where `taken` refuses that.
pub fn free(name: &str, taken: impl Fn(&str) -> bool) -> Id {
    let base = readable(name);
    let text = std::iter::once(base.clone())
        .chain((2u64..).map(|n| format!("{base}-{n}")))
        .find(|s| !taken(s))
        .expect("some n is free");
    Id::parse(&text, "board").expect("readable IDs are IDs")
}

/// Names Windows reserves for devices: an ID equal to one gets a `_`.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The readable form of a job's file name, as ryueda makes library IDs: ASCII
/// letters lowercased, digits and `-` kept, every run of anything else one
/// `_`, trimmed to start and end with a letter or digit, at most 56
/// characters; `board` when nothing is left.
pub fn readable(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let edge = |c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit());
    let mut s = out.trim_matches(edge).to_owned();
    if s.len() > 56 {
        s.truncate(56);
        s = s.trim_end_matches(edge).to_owned();
    }
    if s.is_empty() {
        s = "board".to_owned();
    }
    if RESERVED.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::{MANIFEST, Manifest};

    pub fn corpus(name: &str) -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus")
            .join(name)
    }

    /// A project in a temporary folder, and a session over it.
    pub fn project() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        let m = Manifest {
            project: crate::id::ProjectId::from_bytes([3; 16]),
            default_unit: crate::units::Unit::Mm,
        };
        write_atomic(&dir.path().join(MANIFEST), m.to_text()).unwrap();
        let s = Session::open(dir.path()).unwrap();
        (dir, s)
    }

    pub fn import_of(path: PathBuf) -> Options {
        Options {
            path,
            ..Options::default()
        }
    }

    fn files(root: &std::path::Path) -> Vec<String> {
        crate::project::Project::tracked(root)
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    }

    #[test]
    fn a_job_is_committed_as_given() {
        let (dir, mut s) = project();
        let tgz = corpus("minimal-clean.tgz");
        let dry = Options {
            dry_run: true,
            ..import_of(tgz.clone())
        };
        let v = import(&mut s, &dry, None).unwrap();
        assert_eq!(v["board"], json!("minimal-clean"));
        assert_eq!(files(dir.path()), [MANIFEST]);
        let rev = s.project().revision.clone();
        assert_eq!(
            import(&mut s, &import_of(tgz.clone()), Some("stale"))
                .unwrap_err()
                .code,
            Code::Conflict
        );
        let v = import(&mut s, &import_of(tgz.clone()), Some(&rev)).unwrap();
        assert_eq!(v["file"]["input"]["container"], json!("tgz"));
        assert_eq!(v["summary"]["components"], json!(11));
        assert_eq!(
            files(dir.path()),
            [
                MANIFEST,
                "boards/minimal-clean/board.json",
                "boards/minimal-clean/input.tgz"
            ]
        );
        assert_eq!(
            std::fs::read(dir.path().join("boards/minimal-clean/input.tgz")).unwrap(),
            std::fs::read(&tgz).unwrap()
        );
        assert_eq!(
            v["revision"],
            json!(s.project().board("minimal-clean").unwrap().revision)
        );
        // The same file again is a board of its own.
        let again = import(&mut s, &import_of(tgz.clone()), None).unwrap();
        assert_eq!(again["board"], json!("minimal-clean-2"));
        let named = Options {
            board: Some("minimal-clean".into()),
            ..import_of(tgz)
        };
        assert_eq!(
            import(&mut s, &named, None).unwrap_err().code,
            Code::DuplicateId
        );
    }

    #[test]
    fn what_isnt_a_job_writes_nothing() {
        let (dir, mut s) = project();
        let junk = dir.path().join("junk.zip");
        std::fs::write(&junk, b"PK\x03\x04 not really").unwrap();
        let e = import(&mut s, &import_of(junk), None).unwrap_err();
        assert_eq!(e.code, Code::NotABoard);
        let e = import(&mut s, &import_of(dir.path().join("nothing.tgz")), None).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = import(
            &mut s,
            &Options {
                board: Some("Bad Id".into()),
                ..import_of(corpus("minimal-clean.tgz"))
            },
            None,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidId);
        assert_eq!(files(dir.path()), [MANIFEST]);
    }

    #[test]
    fn an_update_replaces_the_input_and_keeps_the_rest() {
        let (dir, mut s) = project();
        import(&mut s, &import_of(corpus("minimal-clean.tgz")), None).unwrap();
        crate::boards::edit(&mut s, "minimal-clean", Some("Mine"), None, None).unwrap();
        let rev = s.project().board("minimal-clean").unwrap().revision.clone();
        let update = |step: Option<&str>| Options {
            update: Some("minimal-clean".into()),
            step: step.map(str::to_owned),
            ..import_of(corpus("minimal-clean"))
        };
        assert_eq!(
            import(&mut s, &update(None), Some("stale"))
                .unwrap_err()
                .code,
            Code::Conflict
        );
        assert_eq!(
            import(&mut s, &update(Some("panel")), None)
                .unwrap_err()
                .code,
            Code::OdbStepNotFound
        );
        let v = import(&mut s, &update(None), Some(&rev)).unwrap();
        assert_eq!(
            (v["update"].clone(), v["file"]["name"].clone()),
            (json!(true), json!("Mine"))
        );
        let f = files(dir.path());
        assert!(
            !f.contains(&"boards/minimal-clean/input.tgz".to_owned()),
            "{f:?}"
        );
        assert!(f.contains(&"boards/minimal-clean/input/matrix/matrix".to_owned()));
        assert!(s.project().board("minimal-clean").unwrap().ready().is_ok());
        let e = import(
            &mut s,
            &Options {
                update: Some("nope".into()),
                ..import_of(corpus("minimal-clean"))
            },
            None,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    #[test]
    fn ids_from_job_names() {
        assert_eq!(readable("GB-CART256K-A"), "gb-cart256k-a");
        assert_eq!(readable("My Board (rev 2)"), "my_board_rev_2");
        assert_eq!(readable("  "), "board");
        assert_eq!(readable("CON"), "con_");
        assert_eq!(free("pcb", |s| s == "pcb").as_str(), "pcb-2");
        assert_eq!(
            free("pcb", |s| s == "pcb" || s == "pcb-2").as_str(),
            "pcb-3"
        );
    }
}
