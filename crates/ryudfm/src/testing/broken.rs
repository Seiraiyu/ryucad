//! Jobs broken on purpose, from a good job's files.

use std::io::Write;

/// Files and their bytes, by path inside the job.
pub type Files = Vec<(String, Vec<u8>)>;

/// Reads a folder's files, by path relative to it.
pub fn folder(dir: &std::path::Path) -> Files {
    fn walk(dir: &std::path::Path, prefix: &str, out: &mut Files) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if p.is_dir() {
                walk(&p, &rel, out);
            } else {
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out
}

/// A gzipped tar of `files`, in order, at fixed times and modes.
pub fn tgz(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (path, data) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(0);
        h.set_entry_type(tar::EntryType::Regular);
        if path.contains("..") {
            // tar's builder refuses `..`: write the name into the header.
            h.as_gnu_mut().unwrap().name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_cksum();
            b.append(&h, data.as_slice()).unwrap();
        } else {
            b.append_data(&mut h, path, data.as_slice()).unwrap();
        }
    }
    let tar = b.into_inner().unwrap();
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(&tar).unwrap();
    e.finish().unwrap()
}

/// How a job is broken on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Breakage {
    /// No `matrix/matrix`.
    NoMatrix,
    /// A copper layer's features file that doesn't parse.
    CorruptFeatures,
    /// A layer with an empty features file.
    EmptyLayer,
    /// A layer the matrix lists with no folder.
    MissingLayer,
    /// A pad drawn with a symbol that is neither standard nor the job's.
    UnknownSymbol,
    /// Two user symbols that use each other, one used on a layer.
    SymbolCycle,
    /// An archive with an entry through `..`.
    DotDot,
}

impl Breakage {
    pub const ALL: [Breakage; 7] = [
        Breakage::NoMatrix,
        Breakage::CorruptFeatures,
        Breakage::EmptyLayer,
        Breakage::MissingLayer,
        Breakage::UnknownSymbol,
        Breakage::SymbolCycle,
        Breakage::DotDot,
    ];
}

/// The files of a job (`steps/<step>/layers/<layer>/…`, no top folder)
/// broken by `b` on `layer` of `step`, as a gzipped tar.
pub fn broken(files: &[(String, Vec<u8>)], step: &str, layer: &str, b: Breakage) -> Vec<u8> {
    let dir = format!("steps/{step}/layers/{layer}/");
    let features = format!("{dir}features");
    let mut out: Files = files.to_vec();
    match b {
        Breakage::NoMatrix => out.retain(|(p, _)| !p.eq_ignore_ascii_case("matrix/matrix")),
        Breakage::CorruptFeatures => {
            for (p, d) in &mut out {
                if p.eq_ignore_ascii_case(&features) {
                    d.extend_from_slice(b"L 0 0 1\n");
                }
            }
        }
        Breakage::EmptyLayer => {
            for (p, d) in &mut out {
                if p.eq_ignore_ascii_case(&features) {
                    d.clear();
                }
            }
        }
        Breakage::MissingLayer => out.retain(|(p, _)| !p.to_ascii_lowercase().starts_with(&dir)),
        Breakage::UnknownSymbol | Breakage::SymbolCycle => {
            let name = if b == Breakage::UnknownSymbol {
                "no_such_symbol"
            } else {
                "loop_a"
            };
            for (p, d) in &mut out {
                if p.eq_ignore_ascii_case(&features) {
                    // A new last symbol and a pad drawn with it.
                    let text = String::from_utf8_lossy(d).into_owned();
                    let n = text.lines().filter(|l| l.starts_with('$')).count();
                    let mut t = format!("UNITS=MM\n${n} {name}\n");
                    t.push_str(
                        &text
                            .replace("UNITS=MM\n", "")
                            .replace("UNITS=INCH\n", "UNITS_WAS=INCH\n"),
                    );
                    *d = fix_units(&t, n).into_bytes();
                }
            }
            if b == Breakage::SymbolCycle {
                for (me, other) in [("loop_a", "loop_b"), ("loop_b", "loop_a")] {
                    let body = format!("UNITS=MM\n$0 {other}\nP 0 0 0 P 0 8 0\n");
                    out.push((format!("symbols/{me}/features"), body.into_bytes()));
                }
            }
        }
        Breakage::DotDot => out.push(("../escape".into(), b"x".to_vec())),
    }
    tgz(&out)
}

/// Puts a features file's own units back first, after a symbol was added
/// in millimetres (`M`), and appends the pad using symbol `n`.
fn fix_units(t: &str, n: usize) -> String {
    let inch = t.contains("UNITS_WAS=INCH\n");
    let mut s = t
        .replace("UNITS_WAS=INCH\n", "")
        .replacen("UNITS=MM\n", "", 1);
    let units = if inch { "INCH" } else { "MM" };
    let sym = s
        .lines()
        .find(|l| l.starts_with(&format!("${n} ")))
        .unwrap_or("")
        .to_string();
    s = s.replacen(&format!("{sym}\n"), "", 1);
    format!("UNITS={units}\n{sym} M\n{s}P 0 0 {n} P 0 8 0\n")
}
