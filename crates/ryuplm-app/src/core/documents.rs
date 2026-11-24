//! The Documents tab's rows (design 937 phase 3, story 14555): a
//! catalogue's documents (`document.list`) and each one's card
//! (`document.show`: fields, file, state, faults and the items naming it).

use crate::core::lists::Finding;
use serde_json::Value;

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

/// A byte count as people read it.
pub fn size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} bytes"),
        1024..1_048_576 => format!("{:.1} KiB", n as f64 / 1024.0),
        _ => format!("{:.1} MiB", n as f64 / 1_048_576.0),
    }
}

/// A row of `document.list`.
#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub id: String,
    pub number: String,
    pub title: String,
    pub doc_type: String,
    pub size: u64,
    pub storage: String,
}

impl Doc {
    pub fn text(&self) -> String {
        format!(
            "{} {} ({}, {})",
            self.number,
            self.title,
            self.doc_type,
            size(self.size)
        )
    }
}

pub fn list(v: &Value) -> Vec<Doc> {
    v["documents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(Doc {
                id: text(&d["id"])?,
                number: text(&d["number"])?,
                title: text(&d["title"]).unwrap_or_default(),
                doc_type: text(&d["type"]).unwrap_or_else(|| "other".into()),
                size: d["size"].as_u64().unwrap_or(0),
                storage: text(&d["storage"]).unwrap_or_else(|| "git".into()),
            })
        })
        .collect()
}

/// A document, as `document.show` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: String,
    pub number: String,
    pub title: String,
    pub doc_type: String,
    pub description: Option<String>,
    pub file_name: String,
    pub media_type: String,
    pub size: u64,
    /// `git` or `service`.
    pub storage: String,
    pub state: String,
    /// The items naming it: `(ID, number)`.
    pub items: Vec<(String, String)>,
    pub faults: Vec<Finding>,
}

impl Card {
    pub fn read(v: &Value) -> Option<Card> {
        let d = &v["document"];
        Some(Card {
            id: text(&d["id"])?,
            number: text(&d["number"])?,
            title: text(&d["title"]).unwrap_or_default(),
            doc_type: text(&d["type"]).unwrap_or_else(|| "other".into()),
            description: text(&d["description"]),
            file_name: text(&d["file"]["name"]).unwrap_or_else(|| "file".into()),
            media_type: text(&d["file"]["media_type"]).unwrap_or_default(),
            size: d["file"]["size"].as_u64().unwrap_or(0),
            storage: text(&d["storage"]).unwrap_or_else(|| "git".into()),
            state: text(&v["state"]["state"]).unwrap_or_default(),
            items: v["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| Some((text(&i["id"])?, text(&i["number"])?)))
                .collect(),
            faults: v["faults"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Finding::read)
                .collect(),
        })
    }

    pub fn text(&self) -> String {
        format!(
            "{} {} ({}, {})",
            self.number,
            self.title,
            self.doc_type,
            size(self.size)
        )
    }
}

/// The references of an item's documents that are this catalogue's
/// (an ID alone): those `document.show` reads.
pub fn local(refs: &[String]) -> Vec<String> {
    refs.iter().filter(|r| !r.contains(':')).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_list_and_a_document_s_card() {
        let l = list(&json!({"count": 1, "documents": [
            {"id": "d000000000001", "number": "DOC-1", "title": "Spec", "type": "specification",
             "size": 2048, "storage": "git"}]}));
        assert_eq!(l[0].text(), "DOC-1 Spec (specification, 2.0 KiB)");
        let c = Card::read(&json!({"document": {"id": "d000000000001", "number": "DOC-1",
                "title": "Spec", "type": "specification", "description": "the spec",
                "file": {"name": "spec.pdf", "media_type": "application/pdf", "size": 2048, "blake3": "ab"},
                "storage": "service"},
            "path": "documents/d0/d000000000001.json", "bytes": null, "revision": "r",
            "state": {"state": "in_work", "text": "in work"},
            "faults": [], "items": [{"id": "k3v9q2m8x4c1p", "number": "100-0042"}]}))
        .unwrap();
        assert_eq!(
            (c.file_name.as_str(), c.storage.as_str()),
            ("spec.pdf", "service")
        );
        assert_eq!(
            c.items,
            [("k3v9q2m8x4c1p".to_owned(), "100-0042".to_owned())]
        );
        assert_eq!(size(5 << 20), "5.0 MiB");
        assert_eq!(size(12), "12 bytes");
    }

    #[test]
    fn an_item_s_own_documents_are_read_and_others_listed() {
        let refs = [
            "d000000000001".to_owned(),
            "c0ffee:d000000000002".to_owned(),
        ];
        assert_eq!(local(&refs), ["d000000000001"]);
    }
}
