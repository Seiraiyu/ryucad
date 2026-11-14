//! Judging a package someone was sent, offline (design 436): its files
//! against its manifest ([`files`]), the manifest against the release
//! this clone holds ([`release`]), and the package tag's attestation
//! against both ([`super::attest::of_release`]).

use super::{MANIFEST, MAX_FILE, MAX_MANIFEST, Manifest, RECORD, zip};
use ryu_doc::canonical;

/// Why a package doesn't verify. Each has a stable name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clause {
    /// The zip doesn't read.
    Unreadable(String),
    /// No `package.json` first in the zip.
    NoManifest,
    /// `package.json` that isn't a canonical manifest.
    ManifestUnreadable(String),
    /// A file the manifest lists that the zip lacks.
    FileMissing(String),
    /// A file whose bytes aren't those the manifest lists.
    FileChanged(String),
    /// A file in the zip the manifest doesn't list.
    FileExtra(String),
    /// `record.json` isn't the record the manifest names.
    RecordMismatch,
    /// This clone holds no official record of that part and label.
    NotOfficial,
    /// The record became official at another commit than the manifest says.
    CommitMismatch,
    /// The record's bytes at its official commit aren't `record.json`.
    RecordChanged,
    /// The project, part or label isn't the record's.
    WrongRelease(&'static str),
    /// The package tag doesn't hold.
    Tag(crate::attest::Clause),
    /// Built again, it isn't the same bytes.
    RebuildDiffers(String),
}

impl Clause {
    pub fn name(&self) -> String {
        match self {
            Clause::Unreadable(_) => "unreadable".into(),
            Clause::NoManifest => "no_manifest".into(),
            Clause::ManifestUnreadable(_) => "manifest_unreadable".into(),
            Clause::FileMissing(_) => "file_missing".into(),
            Clause::FileChanged(_) => "file_changed".into(),
            Clause::FileExtra(_) => "file_extra".into(),
            Clause::RecordMismatch => "record_mismatch".into(),
            Clause::NotOfficial => "not_official".into(),
            Clause::CommitMismatch => "commit_mismatch".into(),
            Clause::RecordChanged => "record_changed".into(),
            Clause::WrongRelease(f) => format!("wrong_{f}"),
            Clause::Tag(c) => format!("tag_{}", c.name()),
            Clause::RebuildDiffers(_) => "rebuild_differs".into(),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Clause::Unreadable(why) => format!("the file {why}"),
            Clause::NoManifest => format!("the zip has no {MANIFEST} first"),
            Clause::ManifestUnreadable(why) => format!("{MANIFEST} doesn't read: {why}"),
            Clause::FileMissing(p) => format!("{p} is listed but missing"),
            Clause::FileChanged(p) => format!("{p} isn't the file the manifest lists"),
            Clause::FileExtra(p) => format!("{p} isn't in the manifest"),
            Clause::RecordMismatch => format!("{RECORD} isn't the record the manifest names"),
            Clause::NotOfficial => "this clone holds no official release of it".into(),
            Clause::CommitMismatch => "the release became official at another commit".into(),
            Clause::RecordChanged => format!("{RECORD} isn't the record at its official commit"),
            Clause::WrongRelease(f) => format!("its {f} isn't the record's"),
            Clause::Tag(c) => format!("its package tag's attestation {}", c.text()),
            Clause::RebuildDiffers(p) => format!("built again, {p} differs"),
        }
    }
}

/// A package's files by path, as the zip holds them.
pub type Files = Vec<(String, Vec<u8>)>;

/// A package's manifest and files (the record among them, not the
/// manifest), each checked against the manifest's hash and size.
pub fn files(bytes: &[u8]) -> Result<(Manifest, Files), Clause> {
    let mut entries = zip::read(bytes, MAX_FILE, &[(MANIFEST, MAX_MANIFEST)])
        .map_err(|u| Clause::Unreadable(u.text()))?;
    if entries.first().map(|(n, _)| n.as_str()) != Some(MANIFEST) {
        return Err(Clause::NoManifest);
    }
    let (_, text) = entries.remove(0);
    let text =
        String::from_utf8(text).map_err(|_| Clause::ManifestUnreadable("not UTF-8".into()))?;
    let v = canonical::parse(&text).map_err(|e| Clause::ManifestUnreadable(e.message))?;
    if canonical::to_string(&v) != text {
        return Err(Clause::ManifestUnreadable("not canonical JSON".into()));
    }
    let m = Manifest::from_json(&v).map_err(|e| Clause::ManifestUnreadable(e.message))?;
    for f in &m.files {
        let Some((_, data)) = entries.iter().find(|(n, _)| *n == f.path) else {
            return Err(Clause::FileMissing(f.path.clone()));
        };
        if data.len() as u64 != f.size || blake3::hash(data).to_hex().as_str() != f.blake3 {
            return Err(Clause::FileChanged(f.path.clone()));
        }
    }
    if let Some((n, _)) = entries
        .iter()
        .find(|(n, _)| !m.files.iter().any(|f| f.path == *n))
    {
        return Err(Clause::FileExtra(n.clone()));
    }
    let record = entries
        .iter()
        .find(|(n, _)| n == RECORD)
        .ok_or_else(|| Clause::FileMissing(RECORD.into()))?;
    if blake3::hash(&record.1).to_hex().as_str() != m.record {
        return Err(Clause::RecordMismatch);
    }
    Ok((m, entries))
}

/// The manifest against the release this clone holds: the record official
/// at the manifest's commit, its bytes there `record.json`'s, and its
/// project, part, label and content the manifest's.
pub fn release(
    repo: &gix::Repository,
    releases: &crate::history::Releases,
    m: &Manifest,
) -> Result<(), Clause> {
    let path = format!("releases/{}/{}.json", m.part, m.label);
    let o = releases.official.get(&path).ok_or(Clause::NotOfficial)?;
    if o.commit != m.commit {
        return Err(Clause::CommitMismatch);
    }
    let bytes = repo
        .find_object(o.blob)
        .map_err(|_| Clause::NotOfficial)?
        .data
        .clone();
    if blake3::hash(&bytes).to_hex().as_str() != m.record {
        return Err(Clause::RecordChanged);
    }
    let r = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|t| canonical::parse(t).ok())
        .and_then(|v| crate::record::Record::from_json(&v, crate::record::Kind::Release).ok())
        .ok_or(Clause::RecordChanged)?;
    if *r.project() != m.project {
        return Err(Clause::WrongRelease("project"));
    }
    if r.release().map(|x| x.content.as_str()) != Some(m.content.as_str()) {
        return Err(Clause::WrongRelease("content"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::manifest;
    use super::super::{File, archive};
    use super::*;

    fn package() -> (Manifest, Files, Vec<u8>) {
        let m = manifest();
        let files = vec![
            (RECORD.to_owned(), br#"{"kind":"ryupdm.release"}"#.to_vec()),
            ("step/100-B.step".to_owned(), b"ISO-10303-21;".to_vec()),
        ];
        let z = archive(&m, &files).unwrap();
        (m, files, z)
    }

    #[test]
    fn a_package_verifies_and_each_tampering_has_its_clause() {
        let (m, files, z) = package();
        assert_eq!(super::files(&z).unwrap(), (m.clone(), files.clone()));
        let rezip = |m: &Manifest, f: &[(String, Vec<u8>)]| archive(m, f).unwrap();
        let mut changed = files.clone();
        changed[1].1 = b"ISO-10303-21; tampered".to_vec();
        assert_eq!(
            super::files(&rezip(&m, &changed)),
            Err(Clause::FileChanged("step/100-B.step".into()))
        );
        let mut extra = files.clone();
        extra.push(("x.txt".into(), b"x".to_vec()));
        assert_eq!(
            super::files(&rezip(&m, &extra)),
            Err(Clause::FileExtra("x.txt".into()))
        );
        assert_eq!(
            super::files(&rezip(&m, &files[..1])),
            Err(Clause::FileMissing("step/100-B.step".into()))
        );
        let mut wrong = m.clone();
        wrong.record = "e".repeat(64);
        assert_eq!(
            super::files(&rezip(&wrong, &files)),
            Err(Clause::RecordMismatch)
        );
        let mut listed = m.clone();
        listed.files[0] = File::of(RECORD, b"other");
        assert_eq!(
            super::files(&rezip(&listed, &files)),
            Err(Clause::FileChanged(RECORD.into()))
        );
        let first = zip::write(&[("a.txt", b"a"), (MANIFEST, m.canonical().as_bytes())]).unwrap();
        assert_eq!(super::files(&first), Err(Clause::NoManifest));
        assert!(matches!(
            super::files(b"PK\x03\x04"),
            Err(Clause::Unreadable(_))
        ));
    }
}
