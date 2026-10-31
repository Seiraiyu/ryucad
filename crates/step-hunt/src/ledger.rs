//! The ledger, `known.txt` beside this crate's manifest: what each finding
//! was judged to be.
//!
//! ```text
//! case <id> #<issue> <signature>          filed as that issue
//! case <id> file <signature> <why…>       the file itself is wrong
//! case <id> limit <signature> <why…>      a limit the kernel has on purpose
//! ```
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
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
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
        Ok(Ledger { lines })
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
