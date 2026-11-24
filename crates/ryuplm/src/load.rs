//! A catalogue read whole from its files: the manifest and every item,
//! each file's fault kept beside the rest.

use crate::catalogue::{MANIFEST, Manifest};
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::ids::{ITEMS, item_of, item_path, shard_of};
use crate::item::Item;
use crate::source::{Kind, Source, join};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Catalogue {
    pub manifest: Manifest,
    /// Every item that reads, by ID.
    pub items: BTreeMap<Id, Item>,
    /// Every file under `items/` that doesn't read as an item at its
    /// shard, in path order; each has `details.path`.
    pub faults: Vec<Error>,
    /// Every document record that reads, by ID (design 937).
    pub documents: BTreeMap<Id, crate::document::Document>,
    /// The names of the files in each document's folder, sorted: what is
    /// there, whatever the records say. Bytes are never read here.
    pub document_files: BTreeMap<Id, Vec<String>>,
}

/// Every file path under `dir`, sorted.
fn walk(src: &dyn Source, dir: &str, out: &mut Vec<String>) -> Result<()> {
    for (name, kind) in src.list(dir)? {
        let path = join(dir, &name);
        match kind {
            Kind::Dir => walk(src, &path, out)?,
            Kind::File => out.push(path),
            Kind::Other => {}
        }
    }
    Ok(())
}

impl Catalogue {
    pub fn load(src: &dyn Source) -> Result<Catalogue> {
        let bytes = src.read(MANIFEST)?.ok_or_else(|| {
            Error::new(
                Code::ProjectNotFound,
                format!("no {MANIFEST}; run `ryuplm init`"),
            )
            .with("path", MANIFEST)
        })?;
        let manifest = Manifest::read(&bytes)?;
        let mut paths = Vec::new();
        walk(src, ITEMS, &mut paths)?;
        paths.sort();
        // Read in path order (a source needn't be shared across threads),
        // then parsed on threads, then gathered in path order again: the
        // same items and faults as one by one, in a fraction of the time
        // at 100,000 items (design 734's bar).
        let mut read: Vec<Read> = Vec::with_capacity(paths.len());
        for path in paths {
            let got = match item_of(&path) {
                Some(id) => src.read_shared(&path)?.map(|b| (id, b)),
                None => None,
            };
            read.push((path, got));
        }
        let parsed = parse_all(&read);
        let mut items = BTreeMap::new();
        let mut faults = Vec::new();
        for ((path, got), item) in read.iter().zip(parsed) {
            let Some((id, _)) = got else {
                if item_of(path).is_some() {
                    continue; // listed, then gone
                }
                let mut e = ryu_doc::files::invalid(
                    "",
                    "not an item file: items/<first two characters of the ID>/<ID>.json",
                )
                .with("path", path);
                if let Some(name) = shard_of(path).and_then(|n| Id::parse(n, "item").ok()) {
                    e = e.with("expected", item_path(&name));
                }
                faults.push(e);
                continue;
            };
            match item {
                Some(Ok(item)) => {
                    items.insert(id.clone(), item);
                }
                Some(Err(e)) => faults.push(e),
                None => {}
            }
        }
        let mut documents = BTreeMap::new();
        let mut document_files: BTreeMap<Id, Vec<String>> = BTreeMap::new();
        let mut doc_paths = Vec::new();
        walk(src, crate::ids::DOCUMENTS, &mut doc_paths)?;
        doc_paths.sort();
        for path in doc_paths {
            if let Some(id) = crate::ids::document_of(&path) {
                let Some(bytes) = src.read_shared(&path)? else {
                    continue; // listed, then gone
                };
                match crate::document::Document::read(&id, &bytes) {
                    Ok(d) => {
                        documents.insert(id, d);
                    }
                    Err(e) => faults.push(e),
                }
            } else if let Some((id, name)) = crate::ids::bytes_of(&path) {
                document_files.entry(id).or_default().push(name.to_owned());
            } else {
                let mut e = ryu_doc::files::invalid(
                    "",
                    "not a document file: documents/<first two characters of the ID>/<ID>.json, its bytes in documents/<first two>/<ID>/",
                )
                .with("path", &path);
                if let Some(name) =
                    crate::ids::doc_shard_of(&path).and_then(|n| Id::parse(n, "document").ok())
                {
                    e = e.with("expected", crate::ids::document_path(&name));
                }
                faults.push(e);
            }
        }
        faults.sort_by(|a, b| a.details["path"].as_str().cmp(&b.details["path"].as_str()));
        Ok(Catalogue {
            manifest,
            items,
            faults,
            documents,
            document_files,
        })
    }

    /// The items whose number, or else whose previous number, is `n`:
    /// current matches first, each list in ID order.
    pub fn by_number(&self, n: &str) -> Vec<Id> {
        let now = self
            .items
            .iter()
            .filter(|(_, i)| i.number == n)
            .map(|(id, _)| id.clone());
        let before = self
            .items
            .iter()
            .filter(|(_, i)| i.number != n && i.previous_numbers.iter().any(|p| p == n))
            .map(|(id, _)| id.clone());
        now.chain(before).collect()
    }

    /// The item `s` names: its ID, or its number (current first), or
    /// `item_not_found`.
    pub fn find(&self, s: &str) -> Result<Id> {
        if let Ok(id) = Id::parse(s, "item")
            && self.items.contains_key(&id)
        {
            return Ok(id);
        }
        self.by_number(s).into_iter().next().ok_or_else(|| {
            Error::new(
                Code::ItemNotFound,
                format!("no item with ID or number {s:?}"),
            )
            .with("item", s)
        })
    }

    /// The documents whose number, or else whose previous number, is `n`.
    pub fn documents_by_number(&self, n: &str) -> Vec<Id> {
        let now = self
            .documents
            .iter()
            .filter(|(_, d)| d.number == n)
            .map(|(id, _)| id.clone());
        let before = self
            .documents
            .iter()
            .filter(|(_, d)| d.number != n && d.previous_numbers.iter().any(|p| p == n))
            .map(|(id, _)| id.clone());
        now.chain(before).collect()
    }

    /// The document `s` names: its ID, or its number (current first), or
    /// `document_not_found`.
    pub fn find_document(&self, s: &str) -> Result<Id> {
        if let Ok(id) = Id::parse(s, "document")
            && self.documents.contains_key(&id)
        {
            return Ok(id);
        }
        self.documents_by_number(s)
            .into_iter()
            .next()
            .ok_or_else(|| {
                Error::new(
                    Code::DocumentNotFound,
                    format!("no document with ID or number {s:?}"),
                )
                .with("document", s)
            })
    }

    /// The item or document `s` names, items first: what release commands
    /// take. Neither is `item_not_found`, its message naming both.
    pub fn find_part(&self, s: &str) -> Result<Id> {
        self.find(s).or_else(|e| {
            self.find_document(s).map_err(|_| {
                Error::new(
                    Code::ItemNotFound,
                    format!("no item or document with ID or number {s:?}"),
                )
                .with("item", s)
                .with("cause", &e.message)
            })
        })
    }

    /// Whether `id` is an item's or a document's: one ID space (ryupdm's
    /// parts).
    pub fn holds(&self, id: &Id) -> bool {
        self.items.contains_key(id) || self.documents.contains_key(id)
    }
}

/// An item file's path, and its ID and bytes when it is one that reads.
type Read = (String, Option<(Id, std::sync::Arc<[u8]>)>);

/// Items parsed at or above this many go out on threads.
const PARALLEL_FROM: usize = 256;

/// Each read file parsed as its item, in order, on up to eight threads
/// once there are many.
fn parse_all(read: &[Read]) -> Vec<Option<Result<Item>>> {
    let one = |(_, got): &Read| got.as_ref().map(|(id, b)| Item::read(id, b));
    if read.len() < PARALLEL_FROM {
        return read.iter().map(one).collect();
    }
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    let per = read.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = read
            .chunks(per)
            .map(|chunk| s.spawn(move || chunk.iter().map(one).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a parsing thread doesn't panic"))
            .collect()
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::item::Item;
    use std::collections::BTreeMap;

    pub(crate) fn files(items: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        let mut f = BTreeMap::new();
        let m = crate::catalogue::Manifest::new(
            crate::catalogue::tests::pid(1),
            crate::units::Unit::Mm,
        );
        f.insert(
            crate::catalogue::MANIFEST.to_owned(),
            m.to_text().into_bytes(),
        );
        for (id, number) in items {
            let id = Id::parse(id, "item").unwrap();
            f.insert(
                crate::ids::item_path(&id),
                Item::new(number).to_text().into_bytes(),
            );
        }
        f
    }

    #[test]
    fn every_item_is_read_and_each_bad_file_is_a_fault() {
        let mut f = files(&[("k3v9q2m8x4c1p", "A-1"), ("0000000000000", "A-2")]);
        f.insert("items/zz/zzzzzzzzzzzzf.json".into(), b"{".to_vec());
        f.insert(
            "items/k4/k3aaaaaaaaaaa.json".into(),
            Item::new("A-3").to_text().into_bytes(),
        );
        f.insert("items/README.md".into(), b"hi".to_vec());
        let c = Catalogue::load(&f).unwrap();
        assert_eq!(
            c.items.keys().map(Id::as_str).collect::<Vec<_>>(),
            ["0000000000000", "k3v9q2m8x4c1p"]
        );
        let faults: Vec<(Code, String)> = c
            .faults
            .iter()
            .map(|e| (e.code, e.details["path"].as_str().unwrap().to_owned()))
            .collect();
        assert_eq!(
            faults,
            [
                (Code::InvalidDocument, "items/README.md".to_owned()),
                (
                    Code::InvalidDocument,
                    "items/k4/k3aaaaaaaaaaa.json".to_owned()
                ),
                (Code::InvalidJson, "items/zz/zzzzzzzzzzzzf.json".to_owned()),
            ]
        );
        assert_eq!(
            c.faults[1].details["expected"],
            "items/k3/k3aaaaaaaaaaa.json"
        );
        assert_eq!(
            c.by_number("A-2"),
            vec![Id::parse("0000000000000", "item").unwrap()]
        );
    }

    #[test]
    fn no_manifest_is_no_catalogue() {
        let f: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        assert_eq!(Catalogue::load(&f).unwrap_err().code, Code::ProjectNotFound);
    }
    #[test]
    fn old_numbers_still_find_their_items() {
        let mut f = files(&[("0000000000001", "B-1"), ("0000000000002", "A-1")]);
        let id = Id::parse("0000000000001", "item").unwrap();
        let mut i = Item::new("B-1");
        i.previous_numbers.push("A-1".into());
        f.insert(crate::ids::item_path(&id), i.to_text().into_bytes());
        let c = Catalogue::load(&f).unwrap();
        // The current holder of A-1 first, then the item that had it.
        assert_eq!(
            c.by_number("A-1")
                .iter()
                .map(Id::as_str)
                .collect::<Vec<_>>(),
            ["0000000000002", "0000000000001"]
        );
        assert_eq!(c.find("0000000000001").unwrap(), id);
        assert_eq!(c.find("B-1").unwrap(), id);
        assert_eq!(c.find("Z-9").unwrap_err().code, Code::ItemNotFound);
    }

    #[test]
    fn documents_are_read_and_their_folders_listed() {
        let mut f = files(&[("0000000000001", "A-1")]);
        let d = Id::parse("k3v9q2m8x4c1p", "document").unwrap();
        let rec = crate::document::Document::new(
            "DOC-1",
            "Spec",
            crate::document::File::of("Spec.pdf", b"%PDF", None).unwrap(),
        );
        f.insert(crate::ids::document_path(&d), rec.to_text().into_bytes());
        f.insert(crate::ids::bytes_path(&d, "Spec.pdf"), b"%PDF".to_vec());
        f.insert(crate::ids::bytes_path(&d, "notes.txt"), b"x".to_vec());
        f.insert("documents/zz/zzzzzzzzzzzzf.json".into(), b"{".to_vec());
        f.insert("documents/README.md".into(), b"hi".to_vec());
        let c = Catalogue::load(&f).unwrap();
        assert_eq!(c.documents.keys().collect::<Vec<_>>(), [&d]);
        assert_eq!(c.document_files[&d], ["Spec.pdf", "notes.txt"]);
        let paths: Vec<&str> = c
            .faults
            .iter()
            .map(|e| e.details["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            ["documents/README.md", "documents/zz/zzzzzzzzzzzzf.json"]
        );
        assert_eq!(c.find_document("DOC-1").unwrap(), d);
        assert_eq!(c.find_document("k3v9q2m8x4c1p").unwrap(), d);
        assert_eq!(
            c.find_document("A-1").unwrap_err().code,
            Code::DocumentNotFound
        );
        assert_eq!(c.find_part("DOC-1").unwrap(), d);
        assert_eq!(c.find_part("A-1").unwrap().as_str(), "0000000000001");
    }
}
