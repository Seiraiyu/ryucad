//! A document, `documents/<ab>/<id>.json` (kind `ryuplm.document`, design
//! 937): a controlled file (a specification, a datasheet, a drawing PDF, a
//! certificate) whose record names its bytes by BLAKE3. The bytes are in
//! Git beside it (`documents/<ab>/<id>/<name>`) or on the service.

use crate::error::{Code, Error, Result};
use crate::id::{Id, ProjectId};
use crate::item::number_ok;
use ryu_doc::files::{Obj, header_in, invalid};
use serde_json::{Value, json};

/// The longest file name, in bytes.
pub const MAX_NAME: usize = 255;
/// Characters no file name may hold on any OS ryuplm runs on.
const REFUSED: &str = "/\\:*?\"<>|";
/// Windows' reserved stems, compared case-insensitively.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Whether `n`'s stem (before its first `.`, trailing spaces dropped) is one
/// Windows reserves.
fn reserved(n: &str) -> bool {
    let stem = n.split('.').next().unwrap_or("").trim_end_matches(' ');
    RESERVED.contains(&stem.to_ascii_lowercase().as_str())
}

/// Whether `n` may name a document's bytes: 1 to 255 bytes, none of
/// `/ \ : * ? " < > |` or control characters, no trailing dot or space, not
/// `.` or `..`, and no reserved stem (`CON`, `NUL`, `COM1` …).
pub fn name_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= MAX_NAME
        && n != "."
        && n != ".."
        && !n.chars().any(|c| c.is_control() || REFUSED.contains(c))
        && !n.ends_with('.')
        && !n.ends_with(' ')
        && !reserved(n)
}

/// `n` made safe: each refused character becomes `_`, each trailing dot or
/// space becomes `_`, a reserved stem gains `_`, an empty name is `file`,
/// and the whole is cut to 254 bytes (room for the `_`) at a character.
pub fn safe_name(n: &str) -> String {
    let mut s: String = n
        .chars()
        .map(|c| {
            if c.is_control() || REFUSED.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    while s.len() > MAX_NAME - 1 {
        s.pop();
    }
    if s.is_empty() {
        return "file".to_owned();
    }
    // Every trailing dot and space (one byte each) becomes `_`.
    let keep = s.trim_end_matches(['.', ' ']).len();
    let tail = s.len() - keep;
    s.truncate(keep);
    s.push_str(&"_".repeat(tail));
    if reserved(&s) {
        let at = s.find('.').unwrap_or(s.len());
        s.insert(at, '_');
    }
    s
}

/// The media type of a file named `name`, by its extension (any case):
/// design 937's fixed table, else `application/octet-stream`.
pub fn media_type(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "html" => "text/html",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "step" | "stp" => "model/step",
        "dxf" => "image/vnd.dxf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Whether `m` is a media type as a record holds one: `type/subtype`, each
/// of lowercase ASCII letters, digits and `!#$&^_.+-`, 127 bytes at most.
pub fn media_type_ok(m: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"!#$&^_.+-".contains(&b))
    };
    m.len() <= 127 && matches!(m.split_once('/'), Some((t, s)) if part(t) && part(s))
}

pub const KIND: &str = "ryuplm.document";
pub const FORMAT: u64 = 1;
/// What a document is.
pub const TYPES: [&str; 6] = [
    "specification",
    "datasheet",
    "drawing",
    "certificate",
    "procedure",
    "other",
];

/// Where a document's bytes are: committed beside the record, or in the
/// service's store under their hash (3.B). The record is the same either
/// way, and the content hash leaves this out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Storage {
    #[default]
    Git,
    Service,
}

impl Storage {
    pub fn as_str(self) -> &'static str {
        match self {
            Storage::Git => "git",
            Storage::Service => "service",
        }
    }

    pub fn parse(s: &str) -> Option<Storage> {
        [Storage::Git, Storage::Service]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
}

/// The bytes a record names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub name: String,
    pub media_type: String,
    pub size: u64,
    /// BLAKE3 of the bytes, 64 lowercase hex digits.
    pub blake3: String,
}

impl File {
    /// The file for `bytes` named `name` (made safe), its media type `media`
    /// or the one its name gives.
    pub fn of(name: &str, bytes: &[u8], media: Option<&str>) -> Result<File> {
        let name = safe_name(name);
        let media_type = match media {
            Some(m) if media_type_ok(m) => m.to_owned(),
            Some(m) => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{m:?} isn't a media type: type/subtype in lowercase"),
                )
                .with("media_type", m));
            }
            None => media_type(&name).to_owned(),
        };
        Ok(File {
            media_type,
            size: bytes.len() as u64,
            blake3: blake3::hash(bytes).to_hex().to_string(),
            name,
        })
    }
}

/// A Google Drive source (3.B): the file and the revision `file` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub drive_id: String,
    pub revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub number: String,
    pub previous_numbers: Vec<String>,
    pub title: String,
    pub description: Option<String>,
    /// One of [`TYPES`]: the file's `type`.
    pub doc_type: String,
    pub file: File,
    pub storage: Storage,
    pub pin: Option<Pin>,
}

/// A document: one of this catalogue's, or `<project>:<id>` another's.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocRef {
    pub catalogue: Option<ProjectId>,
    pub document: Id,
}

impl DocRef {
    pub fn parse(s: &str) -> Option<DocRef> {
        match s.split_once(':') {
            Some((p, d)) => Some(DocRef {
                catalogue: Some(ProjectId::parse(p).ok()?),
                document: Id::parse(d, "document").ok()?,
            }),
            None => Some(DocRef {
                catalogue: None,
                document: Id::parse(s, "document").ok()?,
            }),
        }
    }

    pub fn text(&self) -> String {
        match &self.catalogue {
            Some(p) => format!("{p}:{}", self.document),
            None => self.document.to_string(),
        }
    }

    /// Whether it names a document of the catalogue `own`.
    pub fn is_in(&self, own: &ProjectId) -> bool {
        self.catalogue.as_ref().is_none_or(|p| p == own)
    }
}

/// Whether `t` may be a title: 1 to 200 characters, no control characters.
pub fn title_ok(t: &str) -> bool {
    !t.is_empty() && t.chars().count() <= 200 && !t.chars().any(char::is_control)
}

fn text_at(v: Value, at: &str) -> Result<String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid(at, "expected a string"))
}

/// A Drive ID or revision: 1 to 200 of ASCII letters, digits, `_` and `-`.
fn drive_text(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Document {
    pub fn new(number: &str, title: &str, file: File) -> Document {
        Document {
            number: number.to_owned(),
            previous_numbers: Vec::new(),
            title: title.to_owned(),
            description: None,
            doc_type: "other".to_owned(),
            file,
            storage: Storage::Git,
            pin: None,
        }
    }

    pub fn renumber(&mut self, number: &str) -> Result<()> {
        crate::item::renumber_in(&mut self.number, &mut self.previous_numbers, number)
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "format": FORMAT,
            "kind": KIND,
            "number": self.number,
            "title": self.title,
            "type": self.doc_type,
            "file": {
                "name": self.file.name,
                "media_type": self.file.media_type,
                "size": self.file.size,
                "blake3": self.file.blake3,
            },
            "storage": self.storage.as_str(),
        });
        if !self.previous_numbers.is_empty() {
            v["previous_numbers"] = json!(self.previous_numbers);
        }
        if let Some(d) = &self.description {
            v["description"] = json!(d);
        }
        if let Some(p) = &self.pin {
            v["pin"] = json!({"drive_id": p.drive_id, "revision": p.revision});
        }
        v
    }

    pub fn to_text(&self) -> String {
        crate::canonical::to_string(&self.to_json())
    }

    pub fn from_json(v: &Value) -> Result<Document> {
        let mut o = header_in(v, KIND, FORMAT, crate::catalogue::NAME)?;
        let number = text_at(o.take("number")?, "/number")?;
        if !number_ok(&number) {
            return Err(invalid(
                "/number",
                "a number is 1 to 200 characters, no control characters, no space at either end",
            ));
        }
        let mut previous_numbers = Vec::new();
        if let Some(p) = o.take_opt("previous_numbers") {
            let list = p
                .as_array()
                .ok_or_else(|| invalid("/previous_numbers", "a list of numbers"))?;
            for (i, n) in list.iter().enumerate() {
                let n = n
                    .as_str()
                    .filter(|n| number_ok(n))
                    .ok_or_else(|| invalid(&format!("/previous_numbers/{i}"), "a number"))?;
                previous_numbers.push(n.to_owned());
            }
        }
        let title = text_at(o.take("title")?, "/title")?;
        if !title_ok(&title) {
            return Err(invalid(
                "/title",
                "a title is 1 to 200 characters, no control characters",
            ));
        }
        let description = o
            .take_opt("description")
            .map(|d| text_at(d, "/description"))
            .transpose()?;
        let doc_type = text_at(o.take("type")?, "/type")?;
        if !TYPES.contains(&doc_type.as_str()) {
            return Err(invalid(
                "/type",
                &format!("type is one of {}", TYPES.join(", ")),
            ));
        }
        let f = o.take("file")?;
        let mut fo = Obj::new(&f, "/file")?;
        let name = text_at(fo.take("name")?, "/file/name")?;
        if !name_ok(&name) {
            return Err(invalid(
                "/file/name",
                "a file name safe on every OS: no / \\ : * ? \" < > |, no trailing dot or space, no reserved name",
            ));
        }
        let media_type = text_at(fo.take("media_type")?, "/file/media_type")?;
        if !media_type_ok(&media_type) {
            return Err(invalid("/file/media_type", "type/subtype in lowercase"));
        }
        let size = fo
            .take("size")?
            .as_u64()
            .ok_or_else(|| invalid("/file/size", "the size in bytes, a whole number"))?;
        let blake3 = text_at(fo.take("blake3")?, "/file/blake3")?;
        if blake3.len() != 64
            || !blake3
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid("/file/blake3", "BLAKE3 as 64 lowercase hex digits"));
        }
        fo.finish()?;
        let storage = text_at(o.take("storage")?, "/storage")?;
        let storage = Storage::parse(&storage)
            .ok_or_else(|| invalid("/storage", "storage is git or service"))?;
        let pin = match o.take_opt("pin") {
            None => None,
            Some(p) => {
                let mut po = Obj::new(&p, "/pin")?;
                let drive_id = text_at(po.take("drive_id")?, "/pin/drive_id")?;
                if !drive_text(&drive_id) {
                    return Err(invalid("/pin/drive_id", "a Drive file ID"));
                }
                let revision = text_at(po.take("revision")?, "/pin/revision")?;
                if !drive_text(&revision) {
                    return Err(invalid("/pin/revision", "a Drive revision ID"));
                }
                po.finish()?;
                Some(Pin { drive_id, revision })
            }
        };
        o.finish()?;
        Ok(Document {
            number,
            previous_numbers,
            title,
            description,
            doc_type,
            file: File {
                name,
                media_type,
                size,
                blake3,
            },
            storage,
            pin,
        })
    }

    /// The record `bytes` for document `id`: errors name the document.
    pub fn read(id: &Id, bytes: &[u8]) -> Result<Document> {
        let path = crate::ids::document_path(id);
        let at = |e: Error| e.with("document", id).with("path", &path);
        let text = std::str::from_utf8(bytes).map_err(|_| {
            at(Error::new(
                Code::InvalidJson,
                format!("{path} is not UTF-8"),
            ))
        })?;
        let v = crate::canonical::parse(text).map_err(at)?;
        if let Some(e) = ryu_doc::files::conflicted_in(&v, &path, crate::catalogue::NAME) {
            return Err(at(e));
        }
        Document::from_json(&v).map_err(at)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn full() -> serde_json::Value {
        json!({
            "format": 1, "kind": "ryuplm.document",
            "number": "DOC-0001", "previous_numbers": ["SPEC-9"],
            "title": "Bracket specification", "description": "Finish and tolerances",
            "type": "specification",
            "file": {"name": "Spec A.pdf", "media_type": "application/pdf", "size": 482113,
                     "blake3": "a".repeat(64)},
            "storage": "git",
            "pin": {"drive_id": "1AbC_d-9", "revision": "0B7x"}
        })
    }

    fn did() -> Id {
        Id::parse("k3v9q2m8x4c1p", "document").unwrap()
    }

    #[test]
    fn a_document_reads_and_writes_canonically() {
        let d = Document::from_json(&full()).unwrap();
        assert_eq!(d.file.size, 482113);
        assert_eq!(d.storage, Storage::Git);
        assert_eq!(d.to_json(), full());
        let text = d.to_text();
        assert!(crate::canonical::is_canonical(&text));
        assert_eq!(Document::read(&did(), text.as_bytes()).unwrap(), d);
        let bare = Document::new("D-1", "Title", File::of("a.txt", b"hi", None).unwrap());
        assert_eq!(bare.doc_type, "other");
        assert_eq!(bare.file.media_type, "text/plain");
        assert_eq!(bare.file.blake3, blake3::hash(b"hi").to_hex().to_string());
        assert_eq!(bare.to_json()["storage"], "git");
    }

    #[test]
    fn bad_documents_say_where() {
        let cases: Vec<(&str, serde_json::Value)> = vec![
            ("/number", json!(" x")),
            ("/title", json!("")),
            ("/type", json!("memo")),
            (
                "/file/name",
                json!({"name": "a:b", "media_type": "text/plain", "size": 1, "blake3": "a".repeat(64)}),
            ),
            (
                "/file/media_type",
                json!({"name": "a", "media_type": "PDF", "size": 1, "blake3": "a".repeat(64)}),
            ),
            (
                "/file/size",
                json!({"name": "a", "media_type": "text/plain", "size": -1, "blake3": "a".repeat(64)}),
            ),
            (
                "/file/blake3",
                json!({"name": "a", "media_type": "text/plain", "size": 1, "blake3": "A".repeat(64)}),
            ),
            ("/storage", json!("lfs")),
            ("/pin/drive_id", json!({"drive_id": "a b", "revision": "r"})),
        ];
        for (at, value) in cases {
            let mut v = full();
            let key = at.trim_start_matches('/').split('/').next().unwrap();
            v[key] = value;
            let e = Document::read(&did(), crate::canonical::to_string(&v).as_bytes()).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].clone()),
                (Code::InvalidDocument, json!(at)),
                "{v}"
            );
            assert_eq!(e.details["document"], "k3v9q2m8x4c1p");
        }
        let mut v = full();
        v["status"] = json!("released");
        let e = Document::read(&did(), crate::canonical::to_string(&v).as_bytes()).unwrap_err();
        assert_eq!(e.details["pointer"], "/status");
    }

    #[test]
    fn references_name_this_catalogue_or_another() {
        let here = DocRef::parse("k3v9q2m8x4c1p").unwrap();
        assert_eq!(here.catalogue, None);
        let there = DocRef::parse("0123456789abcdef0123456789abcdef:k3v9q2m8x4c1p").unwrap();
        assert_eq!(
            there.text(),
            "0123456789abcdef0123456789abcdef:k3v9q2m8x4c1p"
        );
        assert!(DocRef::parse("x:y").is_none());
        assert!(here < there, "this catalogue's sort first");
    }

    #[test]
    fn names_are_safe_on_every_os() {
        for ok in [
            "Spec A.pdf",
            "a",
            "über-plan.docx",
            ".hidden",
            "console.txt",
            "x.CON",
        ] {
            assert!(name_ok(ok), "{ok}");
        }
        for bad in [
            "", ".", "..", "a/b", "a\\b", "a:b", "a*", "a?", "a\"", "a<", "a>", "a|", "a\u{7}",
            "a.", "a ", "CON", "con.txt", "Lpt9.pdf", "nul .txt",
        ] {
            assert!(!name_ok(bad), "{bad:?}");
        }
        assert!(!name_ok(&"x".repeat(256)));
        assert_eq!(safe_name("a:b?.pdf"), "a_b_.pdf");
        assert_eq!(safe_name("report."), "report_");
        assert_eq!(safe_name("CON.txt"), "CON_.txt");
        assert_eq!(safe_name(""), "file");
        assert_eq!(safe_name(".."), "__");
        let long = safe_name(&"é".repeat(200));
        assert!(long.len() <= MAX_NAME && name_ok(&long));
        for n in ["a:b?.pdf", "report.", "CON.txt", "", "..", "x\u{0}y"] {
            assert!(name_ok(&safe_name(n)), "{n:?}");
        }
    }

    #[test]
    fn media_types_come_from_a_fixed_table() {
        assert_eq!(media_type("Spec.PDF"), "application/pdf");
        assert_eq!(media_type("a.jpeg"), "image/jpeg");
        assert_eq!(media_type("model.stp"), "model/step");
        assert_eq!(media_type("README"), "application/octet-stream");
        assert_eq!(media_type("a.tar.gz"), "application/octet-stream");
        assert!(media_type_ok("application/vnd.ms-excel"));
        assert!(!media_type_ok("Application/PDF"));
        assert!(!media_type_ok("pdf"));
        assert!(!media_type_ok("a/b/c"));
    }
}
