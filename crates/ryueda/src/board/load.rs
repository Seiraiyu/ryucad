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
    /// By ID.
    pub tracks: BTreeMap<Id, Loaded<Track>>,
    pub vias: BTreeMap<Id, Loaded<Via>>,
    pub holes: BTreeMap<Id, Loaded<Hole>>,
    /// `rules.json`, when the board has one (design 924).
    pub rules: Option<Loaded<super::rules::RulesFile>>,
    /// By ID (ryueda 7.C).
    pub keepouts: BTreeMap<Id, Loaded<super::keepout::Keepout>>,
    /// By ID (ryueda 7.C).
    pub drawings: BTreeMap<Id, Loaded<super::drawing::Graphic>>,
    /// By ID (ryueda 7.D).
    pub pours: BTreeMap<Id, Loaded<super::pour::Pour>>,
}

fn unknown(path: &str) -> Fault {
    Fault::warning(
        Code::UnknownFile,
        format!("{path} isn't a file ryueda's boards hold"),
    )
    .with("path", path)
}

/// Board files read before, by their bytes (ryueda 7.E): a session's
/// sources share one buffer per file while it is unchanged, so the
/// buffer itself is the key (held, so its address can't be reused while
/// the entry lives) and nothing is hashed. Each load keeps the entries it
/// used and drops the rest, so the cache holds about one project's board
/// files; loads side by side only miss more.
struct Parsed {
    now: std::collections::HashMap<Key, Entry>,
    next: std::collections::HashMap<Key, Entry>,
}

type Key = (usize, usize, &'static str);
type Entry = (
    std::sync::Arc<[u8]>,
    std::sync::Arc<dyn std::any::Any + Send + Sync>,
);

static PARSED: std::sync::Mutex<Option<Parsed>> = std::sync::Mutex::new(None);

/// `bytes` read as a `T` of `kind`, or the copy parsed from the same
/// buffer before. Errors aren't kept: they parse again.
fn parse<T: serde::de::DeserializeOwned + Clone + Send + Sync + 'static>(
    bytes: &std::sync::Arc<[u8]>,
    path: &str,
    kind: &'static str,
    newest: u64,
) -> Result<T> {
    let key = (bytes.as_ptr() as usize, bytes.len(), kind);
    {
        let mut g = PARSED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = g.as_mut() {
            let hit = c.next.get(&key).or_else(|| c.now.get(&key)).cloned();
            if let Some((b, v)) = hit
                && let Some(t) = v.downcast_ref::<T>()
            {
                let t = t.clone();
                c.next.insert(key, (b, v));
                return Ok(t);
            }
        }
    }
    let t: T = doc::read(bytes, path, kind, newest)?;
    let mut g = PARSED.lock().unwrap_or_else(|e| e.into_inner());
    let c = g.get_or_insert_with(|| Parsed {
        now: Default::default(),
        next: Default::default(),
    });
    c.next
        .insert(key, (bytes.clone(), std::sync::Arc::new(t.clone())));
    Ok(t)
}

/// Ends a load: what it used is what the next one looks in first.
fn parsed_done() {
    let mut g = PARSED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = g.as_mut() {
        c.now = std::mem::take(&mut c.next);
    }
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
    parsed_done();
    Ok((boards, faults))
}

fn board_faults(b: &Board, path: &str) -> Vec<Fault> {
    super::outline::judge(b)
        .into_iter()
        .chain(b.stackup.iter().flat_map(super::stackup::judge))
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
    let mut tracks = BTreeMap::new();
    let mut vias = BTreeMap::new();
    let mut holes = BTreeMap::new();
    let mut rules = None;
    let mut keepouts = BTreeMap::new();
    let mut drawings = BTreeMap::new();
    let mut pours = BTreeMap::new();
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
                    match doc::read::<Board>(&bytes, &path, BOARD_KIND, NEWEST) {
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
                    let loaded = match parse::<Placement>(&bytes, &at, PLACEMENT_KIND, NEWEST) {
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
            (TRACKS, Kind::Dir) => {
                any |= routes(
                    src,
                    &path,
                    TRACKS,
                    TRACK_KIND,
                    "track",
                    &mut h,
                    faults,
                    &mut tracks,
                    track_faults,
                )?;
            }
            (VIAS, Kind::Dir) => {
                any |= routes(
                    src, &path, VIAS, VIA_KIND, "via", &mut h, faults, &mut vias, via_faults,
                )?;
            }
            (HOLES, Kind::Dir) => {
                any |= routes(
                    src,
                    &path,
                    HOLES,
                    HOLE_KIND,
                    "hole",
                    &mut h,
                    faults,
                    &mut holes,
                    hole_faults,
                )?;
            }
            (DRAWINGS, Kind::Dir) => {
                any |= routes(
                    src,
                    &path,
                    DRAWINGS,
                    DRAWING_KIND,
                    "drawing",
                    &mut h,
                    faults,
                    &mut drawings,
                    super::drawing::judge,
                )?;
            }
            (KEEPOUTS, Kind::Dir) => {
                any |= routes(
                    src,
                    &path,
                    KEEPOUTS,
                    KEEPOUT_KIND,
                    "keepout",
                    &mut h,
                    faults,
                    &mut keepouts,
                    |_| Vec::new(),
                )?;
            }
            (POURS, Kind::Dir) => {
                any |= routes(
                    src,
                    &path,
                    POURS,
                    POUR_KIND,
                    "pour",
                    &mut h,
                    faults,
                    &mut pours,
                    |_| Vec::new(),
                )?;
            }
            (RULES_FILE, Kind::File) => {
                any = true;
                let bytes = read(src, &path)?;
                piece(&mut h, name.as_bytes());
                piece(&mut h, &bytes);
                rules = Some(
                    match doc::read::<super::rules::RulesFile>(
                        &bytes,
                        &path,
                        RULES_KIND,
                        super::rules::FORMAT,
                    ) {
                        Ok(r) => Loaded {
                            item: Ok(r),
                            faults: Vec::new(),
                        },
                        Err(e) => Loaded {
                            item: Err(e),
                            faults: Vec::new(),
                        },
                    },
                );
            }
            _ => faults.push(unknown(&path)),
        }
    }
    if !any {
        return Ok(None);
    }
    // Routes judged against the board's stackup.
    if let Some(Loaded { item: Ok(b), .. }) = &board {
        let n = super::stackup::copper_count(b);
        for (kid, k) in keepouts.iter_mut() {
            if let Ok(ko) = &k.item {
                let path = route_path(&id, KEEPOUTS, kid);
                k.faults.extend(
                    super::keepout::judge(ko, n)
                        .into_iter()
                        .map(|f| f.with("path", &path)),
                );
            }
        }
        for (pid, p) in pours.iter_mut() {
            if let Ok(pour) = &p.item {
                let path = route_path(&id, POURS, pid);
                p.faults.extend(
                    super::pour::judge(pour, n)
                        .into_iter()
                        .map(|f| f.with("path", &path)),
                );
            }
        }
        for (tid, t) in tracks.iter_mut() {
            if let Ok(track) = &t.item
                && track.layer.index(n).is_none()
            {
                t.faults.push(
                    no_layer(&track.layer.short(), n, "/layer")
                        .with("path", route_path(&id, TRACKS, tid)),
                );
            }
        }
        for (vid, v) in vias.iter_mut() {
            let Ok(via) = &v.item else { continue };
            let path = route_path(&id, VIAS, vid);
            let mut missing = false;
            for (l, pointer) in [(via.from, "/from"), (via.to, "/to")] {
                if let Some(l) = l
                    && l.index(n).is_none()
                {
                    missing = true;
                    v.faults
                        .push(no_layer(&l.short(), n, pointer).with("path", &path));
                }
            }
            if !missing && via.span(n).is_none() {
                v.faults.push(
                    invalid("a via runs from a layer to one below it", "/from").with("path", &path),
                );
            }
        }
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
        tracks,
        vias,
        holes,
        rules,
        keepouts,
        drawings,
        pours,
    }))
}

/// One route folder's files, each `<id>.json`, hashed into the board's
/// revision; whether it held any.
#[allow(clippy::too_many_arguments)]
fn routes<T: serde::de::DeserializeOwned + Clone + Send + Sync + 'static>(
    src: &dyn Source,
    dir: &str,
    folder: &str,
    kind: &'static str,
    what: &str,
    h: &mut blake3::Hasher,
    faults: &mut Vec<Fault>,
    out: &mut BTreeMap<Id, Loaded<T>>,
    check: fn(&T) -> Vec<Fault>,
) -> Result<bool> {
    let mut any = false;
    for (file, k) in src.list(dir)? {
        let at = join(dir, &file);
        let id = file
            .strip_suffix(".json")
            .and_then(|s| Id::parse(s, what).ok());
        let (Kind::File, Some(id)) = (k, id) else {
            faults.push(unknown(&at));
            continue;
        };
        any = true;
        let bytes = read(src, &at)?;
        piece(h, join(folder, &file).as_bytes());
        piece(h, &bytes);
        // Holes have no format 2.
        let newest = if [HOLE_KIND, KEEPOUT_KIND, DRAWING_KIND, POUR_KIND].contains(&kind) {
            FORMAT
        } else {
            NEWEST
        };
        let loaded = match parse::<T>(&bytes, &at, kind, newest) {
            Ok(x) => Loaded {
                faults: check(&x).into_iter().map(|f| f.with("path", &at)).collect(),
                item: Ok(x),
            },
            Err(e) => Loaded {
                item: Err(e),
                faults: Vec::new(),
            },
        };
        out.insert(id, loaded);
    }
    Ok(any)
}

fn out_of_range(p: &crate::symbol::Point) -> bool {
    let lim = ryu_board::geom::COORD_LIMIT;
    p[0].abs() > lim || p[1].abs() > lim
}

fn no_layer(layer: &str, n: usize, pointer: &str) -> Fault {
    Fault::error(
        Code::BoardLayerNotFound,
        format!(
            "the board has {n} copper layers and no {layer}; they are {}",
            layer_list(n)
        ),
    )
    .with("pointer", pointer)
    .with("layer", layer)
}

/// A board's copper layers by name, `top, in1, …, bottom`.
pub(crate) fn layer_list(n: usize) -> String {
    (0..n)
        .map(|i| CopperLayer::at(i, n).short())
        .collect::<Vec<_>>()
        .join(", ")
}

fn invalid(msg: &str, pointer: &str) -> Fault {
    Fault::error(Code::InvalidDocument, msg).with("pointer", pointer)
}

fn range(pointer: &str) -> Fault {
    Fault::error(Code::BoardOutOfRange, "a coordinate past ±2^40 nm").with("pointer", pointer)
}

fn track_faults(t: &Track) -> Vec<Fault> {
    let mut out = Vec::new();
    if t.width_nm <= 0 {
        out.push(invalid("a track's width is positive", "/width_nm"));
    }
    if t.points.iter().any(out_of_range) {
        out.push(range("/points"));
    } else if t.points.len() < 2 || t.points.iter().all(|p| *p == t.points[0]) {
        out.push(invalid(
            "a track has two or more distinct points",
            "/points",
        ));
    }
    out
}

fn via_faults(v: &Via) -> Vec<Fault> {
    let mut out = Vec::new();
    if v.drill_nm <= 0 {
        out.push(invalid("a via's drill is positive", "/drill_nm"));
    }
    if v.diameter_nm <= 0 {
        out.push(invalid("a via's diameter is positive", "/diameter_nm"));
    }
    if out_of_range(&v.at) {
        out.push(range("/at"));
    }
    out
}

fn hole_faults(h: &Hole) -> Vec<Fault> {
    let mut out = Vec::new();
    if h.drill_nm <= 0 {
        out.push(invalid("a hole's drill is positive", "/drill_nm"));
    }
    if out_of_range(&h.at) {
        out.push(range("/at"));
    }
    out
}

fn placement_faults(p: &Placement) -> Vec<Fault> {
    let lim = ryu_board::geom::COORD_LIMIT;
    let mut out = Vec::new();
    if p.at[0].abs() > lim || p.at[1].abs() > lim {
        out.push(
            Fault::error(Code::BoardOutOfRange, "a placement past ±2^40 nm").with("pointer", "/at"),
        );
    }
    for (i, c) in p.constraints.iter().enumerate() {
        let w = match c {
            Constraint::Near { within_nm, .. } | Constraint::Edge { within_nm, .. } => *within_nm,
            Constraint::Side { .. } => 0,
        };
        if !(0..=lim).contains(&w) {
            out.push(invalid(
                "a constraint's within_nm is 0 to 2^40 nm",
                &format!("/constraints/{i}/within_nm"),
            ));
        }
    }
    if let Some(g) = &p.group
        && (g.is_empty()
            || g.chars().count() > 64
            || g.contains('/')
            || g.chars().any(char::is_control))
    {
        out.push(invalid(
            "a group is 1 to 64 characters, without / or control characters",
            "/group",
        ));
    }
    for (name, f) in [
        ("reference", &p.fields.reference),
        ("value", &p.fields.value),
    ] {
        if let Some(f) = f
            && f.size_nm.is_some_and(|s| s[0] <= 0 || s[1] <= 0)
        {
            out.push(invalid(
                "a field's size is positive",
                &format!("/fields/{name}/size_nm"),
            ));
        }
        if let Some(f) = f
            && f.at.is_some_and(|a| a[0].abs() > lim || a[1].abs() > lim)
        {
            out.push(
                Fault::error(Code::BoardOutOfRange, "a field's place past ±2^40 nm")
                    .with("pointer", format!("/fields/{name}/at")),
            );
        }
    }
    out
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
        for t in self.tracks.values() {
            one(t.item.as_ref().err(), &t.faults);
        }
        for v in self.vias.values() {
            one(v.item.as_ref().err(), &v.faults);
        }
        for h in self.holes.values() {
            one(h.item.as_ref().err(), &h.faults);
        }
        if let Some(r) = &self.rules {
            one(r.item.as_ref().err(), &r.faults);
        }
        for k in self.keepouts.values() {
            one(k.item.as_ref().err(), &k.faults);
        }
        for d in self.drawings.values() {
            one(d.item.as_ref().err(), &d.faults);
        }
        for p in self.pours.values() {
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
    fn bad_constraints_and_groups_are_faults() {
        for (body, pointer) in [
            (
                r#""constraints":[{"near":"U1","within_nm":-1}]"#,
                "/constraints/0/within_nm",
            ),
            (r#""group":"a/b""#, "/group"),
            (r#""group":"""#, "/group"),
            (
                r#""fields":{"reference":{"size_nm":[0,1]}}"#,
                "/fields/reference/size_nm",
            ),
        ] {
            let placed = format!(r#"{{"at":[1,2],"format":2,"kind":"ryueda.placement",{body}}}"#);
            let (boards, _) = load(&files(&[
                ("boards/b/board.json", BOARD),
                ("boards/b/placements/r1.json", &placed),
            ]))
            .unwrap();
            let f = boards[&Id::parse("b", "board").unwrap()].faults();
            assert!(
                f.iter().any(|x| x.error.details["pointer"] == pointer),
                "{body}: {f:?}"
            );
        }
    }

    #[test]
    fn placements_without_a_board_file_are_a_fault() {
        let (boards, _) = load(&files(&[("boards/b/placements/r1.json", PLACED)])).unwrap();
        let b = &boards[&Id::parse("b", "board").unwrap()];
        assert_eq!(b.faults()[0].error.code, Code::InvalidDocument);
    }

    #[test]
    fn a_bad_stackup_is_a_fault_and_a_good_one_reads() {
        let good = BOARD.replace(r#""format":1"#, r#""format":2"#).replace(
            r#""preset":"jlc-2l""#,
            r#""preset":"jlc-2l","stackup":{"layers":[{"kind":"copper","thickness_nm":35000},{"kind":"dielectric","thickness_nm":1530000},{"kind":"copper","thickness_nm":35000}]}"#,
        );
        let (boards, _) = load(&files(&[("boards/b/board.json", &good)])).unwrap();
        let b = &boards[&Id::parse("b", "board").unwrap()];
        assert!(b.faults().is_empty(), "{:?}", b.faults());
        let bad = good.replace(r#"{"kind":"dielectric","thickness_nm":1530000},"#, "");
        let (boards, _) = load(&files(&[("boards/b/board.json", &bad)])).unwrap();
        let f = boards[&Id::parse("b", "board").unwrap()].faults();
        assert_eq!(f[0].error.code, Code::BoardStackupInvalid);
    }

    const TRACK: &str = r#"{"format":1,"kind":"ryueda.track","layer":"top","net":"sda","points":[[0,0],[10,0]],"width_nm":5}"#;
    const VIA: &str =
        r#"{"at":[1,1],"diameter_nm":6,"drill_nm":3,"format":1,"kind":"ryueda.via","net":"sda"}"#;
    const HOLE: &str = r#"{"at":[2,2],"drill_nm":3,"format":1,"kind":"ryueda.hole"}"#;

    const FOUR: &str = r#""stackup":{"layers":[{"kind":"copper","thickness_nm":35000},{"kind":"dielectric","thickness_nm":400000},{"kind":"copper","thickness_nm":35000},{"kind":"dielectric","thickness_nm":400000},{"kind":"copper","thickness_nm":35000},{"kind":"dielectric","thickness_nm":400000},{"kind":"copper","thickness_nm":35000}]}"#;

    fn four() -> String {
        BOARD.replace(r#""format":1"#, r#""format":2"#).replace(
            r#""preset":"jlc-2l""#,
            &format!(r#""preset":"jlc-2l",{FOUR}"#),
        )
    }

    #[test]
    fn routes_are_judged_against_the_stackup() {
        let inner = TRACK
            .replace(r#""format":1"#, r#""format":2"#)
            .replace(r#""layer":"top""#, r#""layer":"in2""#);
        let blind = VIA
            .replace(r#""format":1"#, r#""format":2"#)
            .replace(r#""net":"sda""#, r#""net":"sda","to":"in1""#);
        let b4 = four();
        let ok = files(&[
            ("boards/b/board.json", &b4),
            ("boards/b/tracks/t1.json", &inner),
            ("boards/b/vias/v1.json", &blind),
        ]);
        let (boards, _) = load(&ok).unwrap();
        let id = Id::parse("b", "board").unwrap();
        assert!(
            boards[&id].faults().is_empty(),
            "{:?}",
            boards[&id].faults()
        );
        // The same routes on a two-layer board: the layers aren't there.
        let two = files(&[
            ("boards/b/board.json", BOARD),
            ("boards/b/tracks/t1.json", &inner),
            ("boards/b/vias/v1.json", &blind),
        ]);
        let f = load(&two).unwrap().0[&id].faults();
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f.iter().all(|x| x.error.code == Code::BoardLayerNotFound));
        assert_eq!(f[0].error.details["path"], "boards/b/tracks/t1.json");
        assert_eq!(f[0].error.details["pointer"], "/layer");
        assert_eq!(f[1].error.details["pointer"], "/to");
        // A span must run downwards and cover two layers.
        for (from, to) in [("in2", "in1"), ("in1", "in1"), ("bottom", "top")] {
            let v = VIA.replace(r#""format":1"#, r#""format":2"#).replace(
                r#""net":"sda""#,
                &format!(r#""from":"{from}","net":"sda","to":"{to}""#),
            );
            let f = load(&files(&[
                ("boards/b/board.json", &b4),
                ("boards/b/vias/v1.json", &v),
            ]))
            .unwrap()
            .0[&id]
                .faults();
            assert_eq!(f.len(), 1, "{from}-{to}: {f:?}");
            assert_eq!(f[0].error.code, Code::InvalidDocument);
            assert_eq!(f[0].error.details["pointer"], "/from");
        }
    }

    #[test]
    fn routes_load_with_their_board() {
        let f = files(&[
            ("boards/b/board.json", BOARD),
            ("boards/b/tracks/t1.json", TRACK),
            ("boards/b/vias/v1.json", VIA),
            ("boards/b/holes/h1.json", HOLE),
            ("boards/b/tracks/T-bad.json", TRACK),
        ]);
        let (boards, faults) = load(&f).unwrap();
        let b = &boards[&Id::parse("b", "board").unwrap()];
        assert_eq!(b.tracks.len(), 1);
        assert_eq!(b.vias.len(), 1);
        assert_eq!(b.holes.len(), 1);
        assert!(b.faults().is_empty(), "{:?}", b.faults());
        assert_eq!(
            faults.len(),
            1,
            "the badly named track is unknown: {faults:?}"
        );
        let without = load(&files(&[("boards/b/board.json", BOARD)])).unwrap().0;
        assert_ne!(
            without[&b.id].revision, b.revision,
            "routes move the revision"
        );
    }

    #[test]
    fn bad_routes_are_faults() {
        let cases = [
            (
                "boards/b/tracks/t1.json",
                TRACK.replace(r#""width_nm":5"#, r#""width_nm":0"#),
                "/width_nm",
            ),
            (
                "boards/b/tracks/t1.json",
                TRACK.replace("[10,0]", "[0,0]"),
                "/points",
            ),
            (
                "boards/b/tracks/t1.json",
                TRACK.replace("[10,0]", "[2000000000000,0]"),
                "/points",
            ),
            (
                "boards/b/vias/v1.json",
                VIA.replace(r#""drill_nm":3"#, r#""drill_nm":-3"#),
                "/drill_nm",
            ),
            (
                "boards/b/vias/v1.json",
                VIA.replace(r#""diameter_nm":6"#, r#""diameter_nm":0"#),
                "/diameter_nm",
            ),
            (
                "boards/b/holes/h1.json",
                HOLE.replace(r#""drill_nm":3"#, r#""drill_nm":0"#),
                "/drill_nm",
            ),
        ];
        for (path, text, pointer) in cases {
            let (boards, _) =
                load(&files(&[("boards/b/board.json", BOARD), (path, &text)])).unwrap();
            let b = &boards[&Id::parse("b", "board").unwrap()];
            let f = b.faults();
            assert_eq!(f.len(), 1, "{path}: {f:?}");
            assert_eq!(f[0].error.details["pointer"], pointer, "{path}");
            assert_eq!(f[0].error.details["path"], path);
        }
    }
}
