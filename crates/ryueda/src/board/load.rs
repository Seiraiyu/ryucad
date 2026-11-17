//! Every board's folder read from a [`Source`]: `board.json` and
//! `placements/*.json`, each with what its own checks find, and the
//! folder's revision (BLAKE3 over its files' names and bytes).

use super::file::*;
use crate::design::piece;
use crate::doc;
use crate::error::{Code, Error, Result};
use crate::fault::{Fault, Severity};
use crate::id::Id;
use crate::library::Loaded;
use crate::source::{Kind, Source, join};
use std::collections::BTreeMap;

/// One board's files as read.
#[derive(Clone, Debug)]
pub struct BoardFiles {
    pub id: Id,
    /// BLAKE3 over the folder's files: board writes are guarded by it.
    pub revision: String,
    pub board: Loaded<Board>,
    /// By occurrence (`s1/u1`).
    pub placements: BTreeMap<String, Loaded<Placement>>,
}

fn unknown(path: &str) -> Fault {
    Fault::warning(
        Code::UnknownFile,
        format!("{path} isn't a file ryueda's boards hold"),
    )
    .with("path", path)
}

fn read(src: &dyn Source, path: &str) -> Result<std::sync::Arc<[u8]>> {
    src.read_shared(path)?
        .ok_or_else(|| Error::new(Code::NotFound, format!("{path} vanished")))
}

/// Every board under `boards/`, and the faults of files that aren't a
/// board's. A folder with no files (what undoing a creation leaves) is
/// no board.
pub fn load(src: &dyn Source) -> Result<(BTreeMap<Id, BoardFiles>, Vec<Fault>)> {
    let mut boards = BTreeMap::new();
    let mut faults = Vec::new();
    for (name, kind) in src.list(BOARDS)? {
        let dir = join(BOARDS, &name);
        let id = match (kind, Id::parse(&name, "board")) {
            (Kind::Dir, Ok(id)) => id,
            _ => {
                faults.push(unknown(&dir));
                continue;
            }
        };
        if let Some(b) = load_one(src, id, &dir, &mut faults)? {
            boards.insert(b.id.clone(), b);
        }
    }
    Ok((boards, faults))
}

fn board_faults(b: &Board, path: &str) -> Vec<Fault> {
    let preset = super::preset::by_name(&b.preset).is_none().then(|| {
        Fault::error(
            Code::InvalidDocument,
            format!(
                "preset {} isn't known; presets are {}",
                b.preset,
                super::preset::names()
            ),
        )
        .with("pointer", "/preset")
    });
    super::outline::judge(b)
        .into_iter()
        .chain(preset)
        .map(|f| f.with("path", path))
        .collect()
}

fn load_one(
    src: &dyn Source,
    id: Id,
    dir: &str,
    faults: &mut Vec<Fault>,
) -> Result<Option<BoardFiles>> {
    let mut h = blake3::Hasher::new();
    h.update(b"ryueda.board.1");
    let mut board = None;
    let mut placements = BTreeMap::new();
    let mut any = false;
    for (name, kind) in src.list(dir)? {
        let path = join(dir, &name);
        match (name.as_str(), kind) {
            (BOARD_FILE, Kind::File) => {
                any = true;
                let bytes = read(src, &path)?;
                piece(&mut h, name.as_bytes());
                piece(&mut h, &bytes);
                board = Some(
                    match doc::read::<Board>(&bytes, &path, BOARD_KIND, FORMAT) {
                        Ok(b) => Loaded {
                            faults: board_faults(&b, &path),
                            item: Ok(b),
                        },
                        Err(e) => Loaded {
                            item: Err(e),
                            faults: Vec::new(),
                        },
                    },
                );
            }
            (PLACEMENTS, Kind::Dir) => {
                for (file, kind) in src.list(&path)? {
                    let at = join(&path, &file);
                    let occ = file.strip_suffix(".json").and_then(occurrence_of);
                    let (Kind::File, Some(occ)) = (kind, occ) else {
                        faults.push(unknown(&at));
                        continue;
                    };
                    any = true;
                    let bytes = read(src, &at)?;
                    piece(&mut h, join(PLACEMENTS, &file).as_bytes());
                    piece(&mut h, &bytes);
                    let loaded = match doc::read::<Placement>(&bytes, &at, PLACEMENT_KIND, FORMAT) {
                        Ok(p) => Loaded {
                            faults: placement_faults(&p)
                                .into_iter()
                                .map(|f| f.with("path", &at))
                                .collect(),
                            item: Ok(p),
                        },
                        Err(e) => Loaded {
                            item: Err(e),
                            faults: Vec::new(),
                        },
                    };
                    placements.insert(occ, loaded);
                }
            }
            _ => faults.push(unknown(&path)),
        }
    }
    if !any {
        return Ok(None);
    }
    let board = board.unwrap_or_else(|| Loaded {
        item: Err(Error::new(
            Code::InvalidDocument,
            format!("{dir} has placements but no {BOARD_FILE}"),
        )
        .with("path", join(dir, BOARD_FILE))),
        faults: Vec::new(),
    });
    Ok(Some(BoardFiles {
        id,
        revision: h.finalize().to_hex()[..32].to_owned(),
        board,
        placements,
    }))
}

fn placement_faults(p: &Placement) -> Vec<Fault> {
    let lim = ryu_board::geom::COORD_LIMIT;
    if p.at[0].abs() > lim || p.at[1].abs() > lim {
        vec![
            Fault::error(Code::BoardOutOfRange, "a placement past ±2^40 nm").with("pointer", "/at"),
        ]
    } else {
        Vec::new()
    }
}

impl BoardFiles {
    /// Every fault of the board's files: unreadable files as errors, and
    /// what each file's checks find.
    pub fn faults(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        let mut one = |e: Option<&Error>, f: &[Fault]| {
            if let Some(e) = e {
                out.push(Fault {
                    severity: Severity::Error,
                    error: e.clone(),
                });
            }
            out.extend(f.iter().cloned());
        };
        one(self.board.item.as_ref().err(), &self.board.faults);
        for p in self.placements.values() {
            one(p.item.as_ref().err(), &p.faults);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as Files;

    fn files(list: &[(&str, &str)]) -> Files<String, Vec<u8>> {
        list.iter()
            .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
            .collect()
    }

    const BOARD: &str = r#"{"design":"d","format":1,"kind":"ryueda.board","name":"Main","outline":{"edges":[{"to":[10,0],"type":"line"},{"to":[10,10],"type":"line"},{"to":[0,10],"type":"line"},{"to":[0,0],"type":"line"}],"start":[0,0]},"preset":"jlc-2l"}"#;
    const PLACED: &str =
        r#"{"at":[1,2],"format":1,"kind":"ryueda.placement","rotation_mdeg":90000}"#;

    #[test]
    fn boards_and_their_placements_load() {
        let f = files(&[
            ("boards/b/board.json", BOARD),
            ("boards/b/placements/s1.u1.json", PLACED),
            ("boards/b/notes.txt", "x"),
            ("boards/b/placements/Bad.json", PLACED),
        ]);
        let (boards, faults) = load(&f).unwrap();
        let b = &boards[&Id::parse("b", "board").unwrap()];
        assert_eq!(b.board.item.as_ref().unwrap().name, "Main");
        assert!(b.faults().is_empty(), "{:?}", b.faults());
        assert_eq!(
            b.placements["s1/u1"].item.as_ref().unwrap().rotation_mdeg,
            90_000
        );
        let paths: Vec<_> = faults
            .iter()
            .map(|f| f.error.details["path"].clone())
            .collect();
        assert_eq!(
            paths,
            vec!["boards/b/notes.txt", "boards/b/placements/Bad.json"]
        );
        assert!(faults.iter().all(|f| f.error.code == Code::UnknownFile));
    }

    #[test]
    fn revisions_follow_the_boards_own_files() {
        let a = load(&files(&[("boards/b/board.json", BOARD)])).unwrap().0;
        let b = load(&files(&[
            ("boards/b/board.json", BOARD),
            ("boards/b/placements/r1.json", PLACED),
        ]))
        .unwrap()
        .0;
        let id = Id::parse("b", "board").unwrap();
        assert_ne!(a[&id].revision, b[&id].revision);
        let again = load(&files(&[("boards/b/board.json", BOARD)])).unwrap().0;
        assert_eq!(a[&id].revision, again[&id].revision);
    }

    #[test]
    fn placements_without_a_board_file_are_a_fault() {
        let (boards, _) = load(&files(&[("boards/b/placements/r1.json", PLACED)])).unwrap();
        let b = &boards[&Id::parse("b", "board").unwrap()];
        assert_eq!(b.faults()[0].error.code, Code::InvalidDocument);
    }

    #[test]
    fn an_unknown_preset_is_a_fault() {
        let other = BOARD.replace("jlc-2l", "acme-6l");
        let (boards, _) = load(&files(&[("boards/b/board.json", &other)])).unwrap();
        let b = &boards[&Id::parse("b", "board").unwrap()];
        assert_eq!(b.faults()[0].error.details["pointer"], "/preset");
    }
}
