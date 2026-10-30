//! What fails a job, what fails part of one, and what the reader warns of.
//!
//! Every code is stable text. The codes that fail a whole job are
//! [`Code`]s; plan 0.C adds them to `ryu_doc::error::Code` under the same
//! names. Faults and warnings are [`Notice`]s on the board, coded by the
//! constants in [`fault`] and [`warning`].

use std::fmt;

use ryu_board::model::{Location, Notice};

/// Why a job couldn't be read at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    /// Not an ODB++ job: no `matrix/matrix` at the root or under one top
    /// folder, or not a folder, tar, gzipped tar or zip.
    NotABoard,
    /// The matrix, or the board step, couldn't be read.
    Unreadable,
    /// `misc/info` names a version other than 7.0, 8.0 or 8.1.
    VersionUnsupported,
    /// More than one step, or none, could be the board: name one.
    StepAmbiguous,
    /// The step asked for isn't in the matrix.
    StepNotFound,
    /// An archive entry with an absolute path, `..` or a link, or an
    /// archive past a million entries or 8 GiB unpacked.
    ArchiveUnsafe,
}

impl Code {
    pub const ALL: [Code; 6] = [
        Code::NotABoard,
        Code::Unreadable,
        Code::VersionUnsupported,
        Code::StepAmbiguous,
        Code::StepNotFound,
        Code::ArchiveUnsafe,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Code::NotABoard => "not_a_board",
            Code::Unreadable => "odb_unreadable",
            Code::VersionUnsupported => "odb_version_unsupported",
            Code::StepAmbiguous => "odb_step_ambiguous",
            Code::StepNotFound => "odb_step_not_found",
            Code::ArchiveUnsafe => "archive_unsafe",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A job that couldn't be read, why, and where in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub code: Code,
    pub message: String,
    pub location: Option<Location>,
    /// The candidate steps, for [`Code::StepAmbiguous`] and
    /// [`Code::StepNotFound`].
    pub steps: Vec<String>,
}

impl Error {
    pub fn new(code: Code, message: impl Into<String>) -> Error {
        Error {
            code,
            message: message.into(),
            location: None,
            steps: Vec::new(),
        }
    }

    pub fn at(mut self, location: Location) -> Error {
        self.location = Some(location);
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(l) = &self.location {
            write!(f, " ({})", where_(l))?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// `path`, `path:line` or `path:line:column`.
pub fn where_(l: &Location) -> String {
    match (l.line, l.column) {
        (Some(line), Some(col)) => format!("{}:{line}:{col}", l.path),
        (Some(line), None) => format!("{}:{line}", l.path),
        _ => l.path.clone(),
    }
}

/// Faults: something the board holds failed, and the rest loaded.
pub mod fault {
    /// A layer's features file that doesn't parse: the layer is present
    /// with no features.
    pub const LAYER_UNREADABLE: &str = "odb_layer_unreadable";
    /// A symbol name that is neither standard nor a user symbol, once per
    /// layer that uses it: its features have no shape.
    pub const SYMBOL_UNKNOWN: &str = "odb_symbol_unknown";
    /// User symbols that use themselves, once per cycle.
    pub const SYMBOL_CYCLE: &str = "odb_symbol_cycle";
    /// A components file, or a component in it, that doesn't read.
    pub const COMPONENT_INVALID: &str = "odb_component_invalid";
    /// An `eda/data` file, or a record in it, that doesn't read.
    pub const EDA_INVALID: &str = "odb_eda_invalid";
    /// Any other file that doesn't read: a step header, profile, attribute
    /// list, tools file, netlist, font, stackup or user symbol.
    pub const FILE_INVALID: &str = "odb_file_invalid";

    pub const ALL: [&str; 6] = [
        LAYER_UNREADABLE,
        SYMBOL_UNKNOWN,
        SYMBOL_CYCLE,
        COMPONENT_INVALID,
        EDA_INVALID,
        FILE_INVALID,
    ];
}

/// Warnings: read, with something to know.
pub mod warning {
    /// A record kind, file or attribute the reader doesn't take, once per
    /// file and kind.
    pub const IGNORED: &str = "odb_ignored";
    /// Numbers finer than a nanometre, once per file, with the count and
    /// the largest change.
    pub const ROUNDED: &str = "odb_rounded";
    /// An arc whose written end is more than 1 µm off its circle.
    pub const ARC_INCONSISTENT: &str = "odb_arc_inconsistent";
    /// Characters a text's font lacks, once per layer and font.
    pub const FONT_MISSING_CHAR: &str = "odb_font_missing_char";
    /// Dynamic text (`$$DATE`), kept as written, once per layer.
    pub const TEXT_DYNAMIC: &str = "odb_text_dynamic";
    /// Barcodes, kept and left out of shapes, once per layer.
    pub const BARCODE_SKIPPED: &str = "odb_barcode_skipped";
    /// No `misc/info`: read as 7.0.
    pub const INFO_MISSING: &str = "odb_info_missing";

    pub const ALL: [&str; 7] = [
        IGNORED,
        ROUNDED,
        ARC_INCONSISTENT,
        FONT_MISSING_CHAR,
        TEXT_DYNAMIC,
        BARCODE_SKIPPED,
        INFO_MISSING,
    ];
}

/// A location in a file of the job.
pub fn location(path: &str, line: Option<u32>, column: Option<u32>) -> Location {
    Location {
        path: path.to_string(),
        line,
        column,
    }
}

/// A notice coded `code`, about `about`, at `location`.
pub fn notice(
    code: &str,
    about: Option<String>,
    message: impl Into<String>,
    location: Option<Location>,
) -> Notice {
    Notice {
        code: code.to_string(),
        about,
        message: message.into(),
        location,
    }
}

/// Warnings from one file, each kind once, with how many times it came up.
#[derive(Clone, Debug, Default)]
pub struct Once {
    seen: std::collections::BTreeMap<(&'static str, String), usize>,
    list: Vec<(Notice, usize)>,
}

impl Once {
    /// Notes `code` for `key` (what it is about within the file); only the
    /// first note of each code and key is kept, at its location.
    pub fn add(
        &mut self,
        code: &'static str,
        key: &str,
        about: Option<String>,
        message: impl FnOnce() -> String,
        at: Location,
    ) {
        let k = (code, key.to_string());
        match self.seen.get(&k) {
            Some(&i) => self.list[i].1 += 1,
            None => {
                self.seen.insert(k, self.list.len());
                self.list
                    .push((notice(code, about, message(), Some(at)), 1));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The notices, in the order first seen, each saying how many times
    /// it came up when more than once.
    pub fn into_vec(self) -> Vec<Notice> {
        self.list
            .into_iter()
            .map(|(mut n, count)| {
                if count > 1 {
                    n.message = format!("{} ({count} times)", n.message);
                }
                n
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_text() {
        let codes: Vec<&str> = Code::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            codes,
            [
                "not_a_board",
                "odb_unreadable",
                "odb_version_unsupported",
                "odb_step_ambiguous",
                "odb_step_not_found",
                "archive_unsafe"
            ]
        );
        assert_eq!(fault::ALL.len(), 6);
        assert_eq!(warning::ALL.len(), 7);
        let mut all: Vec<&str> = codes.clone();
        all.extend(fault::ALL);
        all.extend(warning::ALL);
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n, "a code is used twice");
        assert!(
            all.iter()
                .all(|c| c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
        );
    }

    #[test]
    fn once_per_kind() {
        let mut o = Once::default();
        let at = |l| location("f", Some(l), None);
        o.add(
            warning::IGNORED,
            "X",
            None,
            || "record kind X".into(),
            at(3),
        );
        o.add(
            warning::IGNORED,
            "Y",
            None,
            || "record kind Y".into(),
            at(4),
        );
        o.add(warning::IGNORED, "X", None, || unreachable!(), at(9));
        let v = o.into_vec();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].message, "record kind X (2 times)");
        assert_eq!(v[0].location.as_ref().unwrap().line, Some(3));
        assert_eq!(v[1].message, "record kind Y");
    }

    #[test]
    fn errors_say_where() {
        let e = Error::new(Code::Unreadable, "no LAYER array closes").at(location(
            "matrix/matrix",
            Some(12),
            Some(1),
        ));
        assert_eq!(
            e.to_string(),
            "odb_unreadable: no LAYER array closes (matrix/matrix:12:1)"
        );
        let l = location("misc/info", None, None);
        assert_eq!(where_(&l), "misc/info");
        assert_eq!(where_(&location("a", Some(3), None)), "a:3");
    }
}
