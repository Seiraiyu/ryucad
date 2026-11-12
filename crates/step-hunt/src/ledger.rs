//! The ledger, `known.txt` beside this crate's manifest: what each finding
//! was judged to be.
//!
//! ```text
//! case <id> #<issue> <signature>          filed as that issue
//! case <id> file <signature> <why…>       the file itself is wrong
//! case <id> limit <signature> <why…>      a limit the kernel has on purpose
//! file <source>@vN <sha256> <size>        a pinned cox-files source's file
//! count <source> <stage> <passed> <of>     of a source's cases, how many passed a stage
//! ```
//!
//! A `count` line is compared by [`Ledger::judge_count`]: fewer passing
//! than recorded, of the same number of cases, is a regression a run
//! reports (`count_regressed`); more is printed, to raise the line.
//!
//! `file` lines pin what a `cox-files:` source at `@vN` holds; `run`
//! refuses the source when what it pulled differs (see
//! [`Ledger::check_files`]).
//!
//! A finding is `new` with no line for its case and signature, else what
//! its line says; a line for a case that ran and no longer has that
//! finding is `fixed`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What a ledger line says a finding is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Judged {
    Issue(u64),
    File(String),
    Limit(String),
}

/// The ledger: by case and signature.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ledger {
    pub lines: BTreeMap<(String, String), Judged>,
    /// `file` lines: by `<source>@vN`, each file's size by its SHA-256.
    pub files: BTreeMap<String, BTreeMap<String, u64>>,
    /// `count` lines: `(passed, of)` by source and stage.
    pub counts: BTreeMap<(String, String), (u64, u64)>,
}

/// A run's count against its `count` line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Count {
    /// No line, or another number of cases: nothing to judge.
    Unrecorded,
    Same,
    Improved,
    Regressed,
}

/// A finding's state against the ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    New,
    Known { judged: Judged },
    Fixed { judged: Judged },
}

impl Ledger {
    pub fn parse(text: &str) -> Result<Ledger, String> {
        let mut lines = BTreeMap::new();
        let mut files = BTreeMap::new();
        let mut counts = BTreeMap::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("count ") {
                let bad = || format!("ledger line {}: {line}", n + 1);
                let w: Vec<&str> = rest.split_whitespace().collect();
                let [source, stage, passed, of] = w.as_slice() else {
                    return Err(bad());
                };
                let (Ok(passed), Ok(of)) = (passed.parse::<u64>(), of.parse::<u64>()) else {
                    return Err(bad());
                };
                if passed > of {
                    return Err(format!("{}: more passing than cases", bad()));
                }
                if counts
                    .insert(((*source).to_owned(), (*stage).to_owned()), (passed, of))
                    .is_some()
                {
                    return Err(format!(
                        "ledger line {}: count {source} {stage} listed twice",
                        n + 1
                    ));
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("file ") {
                let bad = || format!("ledger line {}: {line}", n + 1);
                let w: Vec<&str> = rest.split_whitespace().collect();
                let [pinned, sha, size] = w.as_slice() else {
                    return Err(bad());
                };
                let pinned_ok = pinned
                    .rsplit_once("@v")
                    .is_some_and(|(s, v)| !s.is_empty() && v.parse::<u32>().is_ok_and(|v| v > 0));
                let sha_ok = sha.len() == 64
                    && sha
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
                let Some(size) = size.parse::<u64>().ok().filter(|_| pinned_ok && sha_ok) else {
                    return Err(bad());
                };
                if files
                    .entry((*pinned).to_owned())
                    .or_insert_with(BTreeMap::new)
                    .insert((*sha).to_owned(), size)
                    .is_some()
                {
                    return Err(format!(
                        "ledger line {}: file {pinned} {sha} listed twice",
                        n + 1
                    ));
                }
                continue;
            }
            let bad = || format!("ledger line {}: {line}", n + 1);
            let mut w = line.splitn(5, ' ');
            let (Some("case"), Some(id), Some(tag), Some(sig)) =
                (w.next(), w.next(), w.next(), w.next())
            else {
                return Err(bad());
            };
            let why = w.next().unwrap_or("").trim().to_owned();
            let judged = match tag {
                "file" if !why.is_empty() => Judged::File(why),
                "limit" if !why.is_empty() => Judged::Limit(why),
                _ => match tag.strip_prefix('#').and_then(|t| t.parse().ok()) {
                    Some(n) => Judged::Issue(n),
                    None => return Err(bad()),
                },
            };
            if lines
                .insert((id.to_owned(), sig.to_owned()), judged)
                .is_some()
            {
                return Err(format!(
                    "ledger line {}: case {id} {sig} listed twice",
                    n + 1
                ));
            }
        }
        Ok(Ledger {
            lines,
            files,
            counts,
        })
    }

    /// Whether a source pinned at `pinned` (`<source>@vN`) pulled exactly
    /// the files the ledger's `file` lines list, as `(sha256, size)`.
    pub fn check_files(&self, pinned: &str, have: &[(String, u64)]) -> Result<(), String> {
        let have: BTreeMap<String, u64> = have.iter().cloned().collect();
        let lines = |m: &BTreeMap<String, u64>| -> String {
            m.iter()
                .map(|(sha, size)| format!("file {pinned} {sha} {size}\n"))
                .collect()
        };
        let Some(want) = self.files.get(pinned) else {
            return Err(format!(
                "{pinned} isn't pinned in the ledger: add these lines to known.txt, in the commit with the campaign's findings:\n{}",
                lines(&have)
            ));
        };
        if *want == have {
            return Ok(());
        }
        let mut why = Vec::new();
        for (sha, size) in want {
            match have.get(sha) {
                None => why.push(format!("in the ledger, not pulled: {sha}")),
                Some(s) if s != size => {
                    why.push(format!("{sha}: size {s}, the ledger says {size}"))
                }
                Some(_) => {}
            }
        }
        for sha in have.keys().filter(|s| !want.contains_key(*s)) {
            why.push(format!("pulled, not in the ledger: {sha}"));
        }
        Err(format!(
            "{pinned} isn't what the ledger pins:\n{}\nPin a new version in a ledger of its own lines, or pull the version the ledger names.",
            why.join("\n")
        ))
    }

    /// How `passed` of `of` cases of `source` passing `stage` compare with
    /// its `count` line.
    pub fn judge_count(&self, source: &str, stage: &str, passed: u64, of: u64) -> Count {
        match self.counts.get(&(source.to_owned(), stage.to_owned())) {
            Some(&(p, o)) if o == of => match passed.cmp(&p) {
                std::cmp::Ordering::Less => Count::Regressed,
                std::cmp::Ordering::Equal => Count::Same,
                std::cmp::Ordering::Greater => Count::Improved,
            },
            _ => Count::Unrecorded,
        }
    }

    /// The state of finding `sig` of case `id`.
    pub fn state(&self, id: &str, sig: &str) -> State {
        match self.lines.get(&(id.to_owned(), sig.to_owned())) {
            Some(j) => State::Known { judged: j.clone() },
            None => State::New,
        }
    }

    /// The lines for case `id` whose signature isn't among `found`: what
    /// the run fixed.
    pub fn fixed(&self, id: &str, found: &[String]) -> Vec<(String, Judged)> {
        self.lines
            .iter()
            .filter(|((c, s), _)| c == id && !found.contains(s))
            .map(|((_, s), j)| (s.clone(), j.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed ledger parses: `step-hunt run` refuses one with a
    /// case listed twice, as FTC 10's surface model (#708) and ctc_02's
    /// construction sheets (#695) each were.
    #[test]
    fn the_committed_ledger_parses() {
        Ledger::parse(include_str!("../known.txt")).unwrap();
    }

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn file_lines_pin_a_sets_version() {
        let l = Ledger::parse(&format!(
            "file owner@v2 {SHA_A} 10\nfile owner@v2 {SHA_B} 20\ncase 0123456789abcdef #1 open/x\n"
        ))
        .unwrap();
        let have = [(SHA_A.to_owned(), 10), (SHA_B.to_owned(), 20)];
        assert_eq!(l.check_files("owner@v2", &have), Ok(()));
        let e = l.check_files("owner@v2", &have[..1]).unwrap_err();
        assert!(
            e.contains(&format!("in the ledger, not pulled: {SHA_B}")),
            "{e}"
        );
        let e = l
            .check_files(
                "owner@v2",
                &[(SHA_A.to_owned(), 10), (SHA_B.to_owned(), 21)],
            )
            .unwrap_err();
        assert!(e.contains("size"), "{e}");
        let e = l.check_files("owner@v3", &have).unwrap_err();
        assert!(e.contains(&format!("file owner@v3 {SHA_A} 10")), "{e}");
    }

    #[test]
    fn count_lines_read_and_judge() {
        let l = Ledger::parse("count kicad-models ref_import 7000 7047\n").unwrap();
        assert_eq!(
            l.counts[&("kicad-models".into(), "ref_import".into())],
            (7000, 7047)
        );
        assert_eq!(
            l.judge_count("kicad-models", "ref_import", 6999, 7047),
            Count::Regressed
        );
        assert_eq!(
            l.judge_count("kicad-models", "ref_import", 7001, 7047),
            Count::Improved
        );
        assert_eq!(
            l.judge_count("kicad-models", "ref_import", 7000, 7047),
            Count::Same
        );
        // Another number of cases is another campaign: not judged.
        assert_eq!(
            l.judge_count("kicad-models", "ref_import", 10, 20),
            Count::Unrecorded
        );
        assert!(Ledger::parse("count kicad-models ref_import 9 8\n").is_err());
        assert!(Ledger::parse("count kicad-models ref_import 9\n").is_err());
    }

    #[test]
    fn bad_file_lines_are_refused() {
        assert!(
            Ledger::parse("file owner@v2 abc 10").is_err(),
            "a SHA-256 is 64 hex digits"
        );
        assert!(
            Ledger::parse(&format!("file owner {SHA_A} 10")).is_err(),
            "pinned at @vN"
        );
        assert!(Ledger::parse(&format!("file owner@v2 {SHA_A} ten")).is_err());
        assert!(
            Ledger::parse(&format!(
                "file owner@v2 {SHA_A} 10\nfile owner@v2 {SHA_A} 10"
            ))
            .unwrap_err()
            .contains("twice")
        );
    }

    const TEXT: &str = "# the ledger\n\
        case 0123456789abcdef #258 reimport/import_failed/uncertain\n\
        case 0123456789abcdef file import/slow/faces<50/PLANE a test file, slow on purpose\n\
        case fedcba9876543210 limit import/import_failed/open_shell sheets come in phase 9\n";

    #[test]
    fn lines_read_and_judge() {
        let l = Ledger::parse(TEXT).unwrap();
        assert_eq!(
            l.state("0123456789abcdef", "reimport/import_failed/uncertain"),
            State::Known {
                judged: Judged::Issue(258)
            }
        );
        assert_eq!(
            l.state("fedcba9876543210", "import/import_failed/open_shell"),
            State::Known {
                judged: Judged::Limit("sheets come in phase 9".into())
            }
        );
        assert_eq!(
            l.state("0123456789abcdef", "check/check_failed/mesh_faults"),
            State::New
        );
        let fixed = l.fixed("0123456789abcdef", &["import/slow/faces<50/PLANE".into()]);
        assert_eq!(
            fixed,
            [(
                "reimport/import_failed/uncertain".to_owned(),
                Judged::Issue(258)
            )]
        );
    }

    #[test]
    fn bad_lines_are_refused() {
        assert!(Ledger::parse("case x").is_err());
        assert!(Ledger::parse("case x nope sig").is_err());
        assert!(
            Ledger::parse("case x file sig").is_err(),
            "a file line says why"
        );
        assert!(
            Ledger::parse("case x #1 sig\ncase x #2 sig")
                .unwrap_err()
                .contains("twice")
        );
    }
}
