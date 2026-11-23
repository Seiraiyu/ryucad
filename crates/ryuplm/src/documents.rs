//! Documents (design 937): their findings, and (Task 13) their commands.

use crate::document::Storage;
use crate::error::{Code, Error};
use crate::fault::{Fault, Severity};
use crate::id::Id;
use crate::ids::{bytes_path, document_path, item_path};
use crate::load::Catalogue;
use crate::source::Source;
use serde_json::json;
use std::sync::Arc;

/// A Git-stored file over this is `document_large` (a warning).
pub const LARGE: u64 = 50 << 20;
/// A Git-stored file over this is `document_too_large` (an error): GitHub
/// refuses files over 100 MiB.
pub const TOO_LARGE: u64 = 100 << 20;

fn err(e: Error) -> Fault {
    Fault::of(Severity::Error, e)
}

fn warn(e: Error) -> Fault {
    Fault::of(Severity::Warning, e)
}

/// Every document finding of the catalogue's own files, in a fixed order:
/// per document in ID order its size, its bytes' presence and its folder's
/// other files; folders with no record; then the bytes whose length or
/// hash isn't the record's; then items naming documents this catalogue
/// lacks.
pub fn judge(src: &dyn Source, cat: &Catalogue) -> Vec<Fault> {
    let mut out = Vec::new();
    let mut hashed: Vec<(&Id, Arc<[u8]>)> = Vec::new();
    for (id, d) in &cat.documents {
        let names = cat.document_files.get(id).map(Vec::as_slice).unwrap_or(&[]);
        if d.storage == Storage::Git {
            if d.file.size > TOO_LARGE {
                out.push(err(Error::new(
                    Code::DocumentTooLarge,
                    format!(
                        "document {id}: {} bytes is over 100 MiB, which GitHub refuses",
                        d.file.size
                    ),
                )
                .with("document", id)
                .with("path", document_path(id))
                .with("pointer", "/file/size")
                .with("size", d.file.size)));
            } else if d.file.size > LARGE {
                out.push(warn(Error::new(
                    Code::DocumentLarge,
                    format!("document {id}: {} bytes is over 50 MiB; a large file that changes often belongs on the service", d.file.size),
                )
                .with("document", id)
                .with("path", document_path(id))
                .with("pointer", "/file/size")
                .with("size", d.file.size)));
            }
            let at = bytes_path(id, &d.file.name);
            match src.read_shared(&at) {
                Ok(Some(b)) => hashed.push((id, b)),
                _ => out.push(err(Error::new(
                    Code::DocumentBytesMissing,
                    format!(
                        "document {id}: no {at}; restore it from Git, or `ryuplm document replace`"
                    ),
                )
                .with("document", id)
                .with("path", &at)
                .with("pointer", "/file"))),
            }
        }
        for n in names {
            if d.storage == Storage::Service || *n != d.file.name {
                let why = if d.storage == Storage::Service {
                    "its record says its bytes are on the service"
                } else {
                    "it isn't the file the record names"
                };
                out.push(warn(
                    Error::new(
                        Code::DocumentFileUnknown,
                        format!("document {id}: {}: {why}", bytes_path(id, n)),
                    )
                    .with("document", id)
                    .with("path", bytes_path(id, n)),
                ));
            }
        }
    }
    for (id, names) in &cat.document_files {
        if cat.documents.contains_key(id) {
            continue;
        }
        for n in names {
            out.push(warn(
                Error::new(
                    Code::DocumentFileUnknown,
                    format!(
                        "{}: no document record {}",
                        bytes_path(id, n),
                        document_path(id)
                    ),
                )
                .with("document", id)
                .with("path", bytes_path(id, n)),
            ));
        }
    }
    let bytes: Vec<Arc<[u8]>> = hashed.iter().map(|(_, b)| b.clone()).collect();
    for ((id, b), h) in hashed.iter().zip(crate::digest::all(&bytes)) {
        let d = &cat.documents[*id];
        if b.len() as u64 != d.file.size || h != d.file.blake3 {
            out.push(err(Error::new(
                Code::DocumentBytesChanged,
                format!(
                    "document {id}: {} isn't the bytes its record names; restore it, or `ryuplm document replace`",
                    bytes_path(id, &d.file.name)
                ),
            )
            .with("document", *id)
            .with("path", bytes_path(id, &d.file.name))
            .with("expected", json!({"size": d.file.size, "blake3": d.file.blake3}))
            .with("actual", json!({"size": b.len(), "blake3": h}))));
        }
    }
    let own = &cat.manifest.project;
    for (iid, item) in &cat.items {
        for (i, r) in item.documents.iter().enumerate() {
            if r.is_in(own) && !cat.documents.contains_key(&r.document) {
                out.push(err(Error::new(
                    Code::DocumentMissing,
                    format!("item {iid}: no document {}", r.document),
                )
                .with("item", iid)
                .with("path", item_path(iid))
                .with("pointer", format!("/documents/{i}"))
                .with("document", r.document.as_str())));
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::document::{Document, File, Storage};
    use std::collections::BTreeMap;

    pub(crate) fn did(s: &str) -> Id {
        Id::parse(s, "document").unwrap()
    }

    /// A catalogue of item A-1 naming documents d1 (bytes right), d2
    /// (bytes changed), d3 (bytes missing), d4 (on the service, with stray
    /// bytes in Git) and d9 (no such document), plus a folder with no
    /// record and a stray file beside d1's bytes.
    pub(crate) fn mixed() -> BTreeMap<String, Vec<u8>> {
        let mut f = crate::load::tests::files(&[("0000000000001", "A-1")]);
        let mut put = |id: &str, bytes: &[u8], storage: Storage, written: Option<&[u8]>| {
            let mut d = Document::new(
                &format!("DOC-{id}"),
                "T",
                File::of("f.pdf", bytes, None).unwrap(),
            );
            d.storage = storage;
            f.insert(
                crate::ids::document_path(&did(id)),
                d.to_text().into_bytes(),
            );
            if let Some(w) = written {
                f.insert(crate::ids::bytes_path(&did(id), "f.pdf"), w.to_vec());
            }
        };
        put("d1000000000000", b"one", Storage::Git, Some(b"one"));
        put("d2000000000000", b"two", Storage::Git, Some(b"TWO"));
        put("d3000000000000", b"three", Storage::Git, None);
        put("d4000000000000", b"four", Storage::Service, Some(b"four"));
        f.insert(
            crate::ids::bytes_path(&did("d1000000000000"), "x.txt"),
            b"x".to_vec(),
        );
        f.insert(
            crate::ids::bytes_path(&did("d5000000000000"), "orphan.pdf"),
            b"o".to_vec(),
        );
        let item_id = Id::parse("0000000000001", "item").unwrap();
        let mut item = crate::item::Item::new("A-1");
        for d in [
            "d1000000000000",
            "d9000000000000",
            "0123456789abcdef0123456789abcdef:d1000000000000",
        ] {
            item.documents
                .push(crate::document::DocRef::parse(d).unwrap());
        }
        item.documents.sort();
        f.insert(crate::ids::item_path(&item_id), item.to_text().into_bytes());
        f
    }

    #[test]
    fn bytes_and_references_are_judged() {
        let f = mixed();
        let cat = Catalogue::load(&f).unwrap();
        let got: Vec<(Code, String, &str)> = judge(&f, &cat)
            .iter()
            .map(|x| {
                (
                    x.error.code,
                    x.error.details["path"].as_str().unwrap_or("").to_owned(),
                    if x.is_error() { "error" } else { "warning" },
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    Code::DocumentFileUnknown,
                    "documents/d1/d1000000000000/x.txt".into(),
                    "warning"
                ),
                (
                    Code::DocumentBytesMissing,
                    "documents/d3/d3000000000000/f.pdf".into(),
                    "error"
                ),
                (
                    Code::DocumentFileUnknown,
                    "documents/d4/d4000000000000/f.pdf".into(),
                    "warning"
                ),
                (
                    Code::DocumentFileUnknown,
                    "documents/d5/d5000000000000/orphan.pdf".into(),
                    "warning"
                ),
                (
                    Code::DocumentBytesChanged,
                    "documents/d2/d2000000000000/f.pdf".into(),
                    "error"
                ),
                (
                    Code::DocumentMissing,
                    "items/00/0000000000001.json".into(),
                    "error"
                ),
            ]
        );
        // Another catalogue's document is check's (Task 16), never judge's.
        let all = crate::judge::all(&f, &cat, &Default::default());
        assert!(all.iter().all(|x| x.error.code != Code::DocumentUnresolved));
    }

    #[test]
    fn sizes_are_judged_from_the_record() {
        let mut f = crate::load::tests::files(&[]);
        for (id, size) in [
            ("d6000000000000", 51u64 << 20),
            ("d7000000000000", 101 << 20),
        ] {
            let mut d = Document::new(id, "T", File::of("f.bin", b"x", None).unwrap());
            d.file.size = size;
            f.insert(
                crate::ids::document_path(&did(id)),
                d.to_text().into_bytes(),
            );
        }
        let cat = Catalogue::load(&f).unwrap();
        let codes: Vec<Code> = judge(&f, &cat).iter().map(|x| x.error.code).collect();
        assert!(codes.contains(&Code::DocumentLarge));
        assert!(codes.contains(&Code::DocumentTooLarge));
    }
}
