//! Documents (design 937): their findings and their commands.

use crate::document::Storage;
use crate::document::{DocRef, Document, File};
use crate::error::Result;
use crate::error::{Code, Error};
use crate::fault::{Fault, Severity};
use crate::id::Id;
use crate::ids::{bytes_path, document_path, item_path};
use crate::items::check_expected;
use crate::load::Catalogue;
use crate::session::Session;
use crate::source::Source;
use crate::write::{Writes, commit, commit_with};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::json;
use std::path::Path;
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

/// The fields `document add` and `document edit` set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct New {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `add` only: the media type, else the one the name gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// The source file `path`: its name made safe and its bytes. Refused
/// before reading when over [`TOO_LARGE`].
fn read_source(path: &Path) -> Result<(String, Vec<u8>)> {
    let meta = std::fs::metadata(path).map_err(|e| Error::io(path, &e))?;
    if !meta.is_file() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{} isn't a file", path.display()),
        )
        .with("file", path));
    }
    if meta.len() > TOO_LARGE {
        return Err(Error::new(
            Code::DocumentTooLarge,
            format!(
                "{} is {} bytes, over the 100 MiB GitHub takes in a file",
                path.display(),
                meta.len()
            ),
        )
        .with("file", path)
        .with("size", meta.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, &e))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((name, bytes))
}

/// The record text, checked as the reader checks it.
fn text_of(id: &Id, d: &Document) -> Result<String> {
    let text = d.to_text();
    Document::read(id, text.as_bytes())?;
    Ok(text)
}

fn number_for(session: &Session, number: Option<String>, next: Option<String>) -> Result<String> {
    match (number, next) {
        (Some(n), None) => Ok(n),
        (None, Some(scheme)) => {
            let config =
                ryupdm::config::Config::read_in(&session.snapshot(), crate::catalogue::NAME)?;
            let name = (!scheme.is_empty()).then_some(scheme.as_str());
            crate::numbers::next_document(&session.snapshot(), session.catalogue(), &config, name)
        }
        _ => Err(Error::new(
            Code::Usage,
            "give a number or --next [<scheme>], not both",
        )),
    }
}

/// A document as answers show it: its ID, record path, bytes path (Git
/// storage) and revision.
pub(crate) fn shown(session: &Session, id: &Id) -> Value {
    let d = &session.catalogue().documents[id];
    let mut v = d.to_json();
    if let Some(o) = v.as_object_mut() {
        o.remove("format");
        o.remove("kind");
        o.insert("id".into(), json!(id));
    }
    let bytes = (d.storage == Storage::Git).then(|| bytes_path(id, &d.file.name));
    json!({"document": v, "path": document_path(id), "bytes": bytes, "revision": session.document_revision(id)})
}

/// The document `s` names, its revision checked against `expected`.
fn current(session: &Session, s: &str, expected: Option<&str>) -> Result<(Id, Document)> {
    let id = session.catalogue().find_document(s)?;
    let rev = session.document_revision(&id).unwrap_or_default();
    check_expected(&format!("document {id}"), &rev, expected)?;
    Ok((id.clone(), session.catalogue().documents[&id].clone()))
}

fn set_type(d: &mut Document, t: Option<String>) -> Result<()> {
    if let Some(t) = t {
        if !crate::document::TYPES.contains(&t.as_str()) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("type is one of {}", crate::document::TYPES.join(", ")),
            )
            .with("type", &t));
        }
        d.doc_type = t;
    }
    Ok(())
}

pub fn add(
    session: &mut Session,
    file: &Path,
    number: Option<String>,
    next: Option<String>,
    fields: New,
    expected: Option<&str>,
) -> Result<Value> {
    check_expected("the catalogue", &session.revision(), expected)?;
    let title = fields
        .title
        .clone()
        .ok_or_else(|| Error::new(Code::Usage, "a document needs --title"))?;
    let number = number_for(session, number, next)?;
    let (name, bytes) = read_source(file)?;
    let mut d = Document::new(
        &number,
        &title,
        File::of(&name, &bytes, fields.media_type.as_deref())?,
    );
    d.description = fields.description;
    set_type(&mut d, fields.doc_type)?;
    // One ID space with items (Decision 1).
    let id = crate::ids::random(|s| {
        Id::parse(s, "document").is_ok_and(|i| session.catalogue().holds(&i))
    })?;
    let text = text_of(&id, &d)?;
    commit_with(
        session,
        &format!("adding document {number}"),
        Writes(vec![(document_path(&id), Some(text))]),
        vec![(bytes_path(&id, &d.file.name), Arc::from(bytes))],
    )?;
    Ok(shown(session, &id))
}

pub fn show(session: &mut Session, document: &str) -> Result<Value> {
    let id = session.catalogue().find_document(document)?;
    let mut v = shown(session, &id);
    let releases = crate::release::releases_or_none(session);
    v["state"] =
        ryupdm::state::derive_for::<crate::Plm>(&session.snapshot(), &id, &releases).to_json();
    let faults: Vec<Value> = crate::judge::all(
        &session.snapshot(),
        session.catalogue(),
        &Default::default(),
    )
    .into_iter()
    .filter(|f| f.error.details["document"] == json!(id) && f.error.details.get("item").is_none())
    .map(|f| f.to_json())
    .collect();
    v["faults"] = json!(faults);
    let own = session.catalogue().manifest.project.clone();
    let items: Vec<Value> = session
        .catalogue()
        .items
        .iter()
        .filter(|(_, i)| {
            i.documents
                .iter()
                .any(|r| r.is_in(&own) && r.document == id)
        })
        .map(|(iid, i)| json!({"id": iid, "number": i.number}))
        .collect();
    v["items"] = json!(items);
    Ok(v)
}

pub fn list(
    session: &Session,
    doc_type: Option<String>,
    number: Option<String>,
    text: Option<String>,
    limit: Option<usize>,
) -> Value {
    let c = session.catalogue();
    let ids: Vec<Id> = match &number {
        Some(n) => c.documents_by_number(n),
        None => c.documents.keys().cloned().collect(),
    };
    let needle = text.as_deref().map(str::to_lowercase);
    let rows: Vec<Value> = ids
        .into_iter()
        .filter(|id| {
            doc_type
                .as_ref()
                .is_none_or(|t| c.documents[id].doc_type == *t)
        })
        .filter(|id| {
            needle.as_ref().is_none_or(|t| {
                let d = &c.documents[id];
                [Some(&d.number), Some(&d.title), d.description.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|s| s.to_lowercase().contains(t))
            })
        })
        .take(limit.unwrap_or(usize::MAX))
        .map(|id| {
            let d = &c.documents[&id];
            json!({"id": id, "number": d.number, "title": d.title, "type": d.doc_type,
                   "size": d.file.size, "storage": d.storage.as_str()})
        })
        .collect();
    json!({"count": rows.len(), "documents": rows})
}

pub fn edit(
    session: &mut Session,
    document: &str,
    fields: New,
    unset: Vec<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut d) = current(session, document, expected)?;
    for u in &unset {
        match u.as_str() {
            "description" => d.description = None,
            other => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("can't unset {other:?}: description"),
                )
                .with("field", other));
            }
        }
    }
    if fields.media_type.is_some() {
        return Err(Error::new(
            Code::Usage,
            "a media type comes with the bytes: `document replace --media-type`",
        ));
    }
    if let Some(t) = fields.title {
        d.title = t;
    }
    if fields.description.is_some() {
        d.description = fields.description;
    }
    set_type(&mut d, fields.doc_type)?;
    let text = text_of(&id, &d)?;
    commit(
        session,
        &format!("editing document {id}"),
        Writes(vec![(document_path(&id), Some(text))]),
    )?;
    Ok(shown(session, &id))
}

pub fn renumber(
    session: &mut Session,
    document: &str,
    number: Option<String>,
    next: Option<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut d) = current(session, document, expected)?;
    let n = number_for(session, number, next)?;
    d.renumber(&n)?;
    let text = text_of(&id, &d)?;
    commit(
        session,
        &format!("renumbering document {id} as {n}"),
        Writes(vec![(document_path(&id), Some(text))]),
    )?;
    Ok(shown(session, &id))
}

pub fn replace(
    session: &mut Session,
    document: &str,
    file: &Path,
    media_type: Option<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut d) = current(session, document, expected)?;
    if d.storage == Storage::Service {
        // 3.A writes only Git storage (Decision 7): 3.B uploads.
        return Err(Error::new(
            Code::InvalidParam,
            format!("document {id}'s bytes are on the service, which this build doesn't write"),
        )
        .with("document", &id)
        .with("storage", "service"));
    }
    let (name, bytes) = read_source(file)?;
    let old = bytes_path(&id, &d.file.name);
    d.file = File::of(&name, &bytes, media_type.as_deref())?;
    let new = bytes_path(&id, &d.file.name);
    let text = text_of(&id, &d)?;
    let mut writes = vec![(document_path(&id), Some(text))];
    if old != new {
        writes.push((old, None));
    }
    commit_with(
        session,
        &format!("replacing document {id}'s bytes"),
        Writes(writes),
        vec![(new, Arc::from(bytes))],
    )?;
    Ok(shown(session, &id))
}

pub fn remove(
    session: &mut Session,
    document: &str,
    detach: bool,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, d) = current(session, document, expected)?;
    let files = session.snapshot();
    let records = ryupdm::record::Records::read_in(&files, crate::catalogue::NAME)?;
    if records.entries.iter().any(|e| e.part == id) {
        return Err(Error::new(
            Code::DocumentReleased,
            format!("document {id} has releases: obsolete them (`ryuplm release obsolete`) instead of removing it"),
        )
        .with("document", &id));
    }
    let own = session.catalogue().manifest.project.clone();
    let naming: Vec<Id> = session
        .catalogue()
        .items
        .iter()
        .filter(|(_, i)| {
            i.documents
                .iter()
                .any(|r| r.is_in(&own) && r.document == id)
        })
        .map(|(iid, _)| iid.clone())
        .collect();
    if !naming.is_empty() && !detach {
        return Err(Error::new(
            Code::DocumentInUse,
            format!(
                "document {id} is named by {} item(s); detach it, or pass --detach",
                naming.len()
            ),
        )
        .with("document", &id)
        .with("items", &naming));
    }
    let mut writes = Vec::new();
    for iid in &naming {
        let mut item = session.catalogue().items[iid].clone();
        item.documents
            .retain(|r| !(r.is_in(&own) && r.document == id));
        writes.push((item_path(iid), Some(crate::items::text_of(iid, &item)?)));
    }
    writes.push((document_path(&id), None));
    let names = session
        .catalogue()
        .document_files
        .get(&id)
        .cloned()
        .unwrap_or_default();
    for n in names {
        writes.push((bytes_path(&id, &n), None));
    }
    commit(session, &format!("removing document {id}"), Writes(writes))?;
    Ok(json!({"removed": id, "number": d.number, "detached": naming}))
}

/// The files of the catalogue at `rev`: a Git revision, or `label:<L>`, a
/// document's official release.
fn files_at(
    session: &mut Session,
    id: &Id,
    rev: &str,
) -> Result<std::collections::BTreeMap<String, Vec<u8>>> {
    let (repo, releases) = crate::release::history(session)?;
    let commit = match rev.strip_prefix("label:") {
        None => rev.to_owned(),
        Some(label) => {
            let config =
                ryupdm::config::Config::read_in(&session.snapshot(), crate::catalogue::NAME)?;
            ryupdm::deps::parts::released(&releases, config.revisions)
                .get(id)
                .and_then(|labels| labels.iter().find(|r| r.label == label))
                .map(|r| r.commit.clone())
                .ok_or_else(|| {
                    Error::new(
                        Code::LabelNotFound,
                        format!("document {id} has no official release {label}"),
                    )
                    .with("document", id)
                    .with("label", label)
                })?
        }
    };
    ryupdm::history::files_for::<crate::Plm>(repo.gix(), &commit, repo.prefix())
}

pub fn get(session: &mut Session, document: &str, rev: Option<&str>, out: &Path) -> Result<Value> {
    let (id, d, bytes): (Id, Document, Option<Arc<[u8]>>) = match rev {
        None => {
            let id = session.catalogue().find_document(document)?;
            let d = session.catalogue().documents[&id].clone();
            let b = session.files().get(&bytes_path(&id, &d.file.name)).cloned();
            (id, d, b)
        }
        Some(r) => {
            let id = Id::parse(document, "document")
                .ok()
                .filter(|i| session.catalogue().documents.contains_key(i))
                .map_or_else(|| session.catalogue().find_document(document), Ok)?;
            let files = files_at(session, &id, r)?;
            let cat = Catalogue::load(&files)?;
            let d = cat.documents.get(&id).cloned().ok_or_else(|| {
                Error::new(Code::DocumentNotFound, format!("no document {id} at {r}"))
                    .with("document", &id)
            })?;
            let b = files
                .get(&bytes_path(&id, &d.file.name))
                .map(|b| Arc::from(b.as_slice()));
            (id, d, b)
        }
    };
    if d.storage == Storage::Service {
        return Err(Error::new(
            Code::DocumentBytesMissing,
            format!("document {id}'s bytes are on the service, which this build doesn't fetch"),
        )
        .with("document", &id));
    }
    let b = bytes.ok_or_else(|| {
        Error::new(
            Code::DocumentBytesMissing,
            format!("no {}", bytes_path(&id, &d.file.name)),
        )
        .with("document", &id)
        .with("path", bytes_path(&id, &d.file.name))
    })?;
    let h = crate::digest::of(&b);
    if b.len() as u64 != d.file.size || h != d.file.blake3 {
        return Err(Error::new(
            Code::DocumentBytesChanged,
            format!("document {id}'s bytes aren't the ones its record names"),
        )
        .with("document", &id)
        .with(
            "expected",
            json!({"size": d.file.size, "blake3": d.file.blake3}),
        )
        .with("actual", json!({"size": b.len(), "blake3": h})));
    }
    crate::source::write_atomic(out, &*b)?;
    Ok(
        json!({"document": id, "number": d.number, "path": out, "size": d.file.size,
              "blake3": d.file.blake3, "rev": rev.unwrap_or("working")}),
    )
}

/// The reference `s` makes: this catalogue's document (ID or number), or
/// another catalogue's `<project>:<id>`.
fn reference(session: &Session, s: &str) -> Result<DocRef> {
    let own = &session.catalogue().manifest.project;
    if let Some(r) = DocRef::parse(s).filter(|r| !r.is_in(own)) {
        return Ok(r);
    }
    let local = s.split_once(':').map_or(s, |(_, d)| d);
    Ok(DocRef {
        catalogue: None,
        document: session.catalogue().find_document(local)?,
    })
}

/// Attaches (`on`) or detaches the document `document` names to or from
/// each of `items`. Items that already have it (or lack it) are left alone.
pub fn attach(
    session: &mut Session,
    document: &str,
    items: &[String],
    on: bool,
    expected: Option<&str>,
) -> Result<Value> {
    check_expected("the catalogue", &session.revision(), expected)?;
    let r = reference(session, document)?;
    let mut ids: Vec<Id> = items
        .iter()
        .map(|s| session.catalogue().find(s))
        .collect::<Result<_>>()?;
    ids.sort();
    ids.dedup();
    let mut writes = Vec::new();
    let mut changed = Vec::new();
    for iid in &ids {
        let mut item = session.catalogue().items[iid].clone();
        let had = item.documents.contains(&r);
        if on && !had {
            item.documents.push(r.clone());
            item.documents.sort();
        } else if !on && had {
            item.documents.retain(|x| *x != r);
        } else {
            continue;
        }
        writes.push((item_path(iid), Some(crate::items::text_of(iid, &item)?)));
        changed.push(iid.clone());
    }
    if !writes.is_empty() {
        let what = if on { "attaching" } else { "detaching" };
        commit(
            session,
            &format!("{what} document {}", r.text()),
            Writes(writes),
        )?;
    }
    Ok(json!({"document": r.text(), "items": changed, "revision": session.revision()}))
}

pub struct Ask<'a> {
    pub document: &'a str,
    pub within: &'a [String],
    pub no_dependents: bool,
    pub working: bool,
}

/// The items of `cat` naming document `doc` of catalogue `of`: rows
/// `{item, number}`.
fn naming(cat: &Catalogue, of: &crate::id::ProjectId, doc: &Id) -> Vec<Value> {
    let own = &cat.manifest.project;
    cat.items
        .iter()
        .filter(|(_, i)| {
            i.documents
                .iter()
                .any(|r| r.document == *doc && r.catalogue.as_ref().unwrap_or(own) == of)
        })
        .map(|(iid, i)| json!({"item": iid, "number": i.number}))
        .collect()
}

/// The rows of each official item release of `s` naming the document, read
/// at the commit that made it official: `{item, number, release, state,
/// commit}`. None outside a repository.
fn released(
    s: &mut Session,
    of: &crate::id::ProjectId,
    doc: &Id,
    problems: &mut Vec<Value>,
    shown: &str,
) -> Vec<Value> {
    let Ok((repo, releases)) = crate::release::history(s) else {
        return Vec::new();
    };
    let config =
        ryupdm::config::Config::read_in(&s.snapshot(), crate::catalogue::NAME).unwrap_or_default();
    let mut at: std::collections::BTreeMap<String, Option<Catalogue>> = Default::default();
    let mut out = Vec::new();
    for (part, labels) in ryupdm::deps::parts::released(&releases, config.revisions) {
        for r in labels {
            let cat = at.entry(r.commit.clone()).or_insert_with(|| {
                match ryupdm::history::files_for::<crate::Plm>(repo.gix(), &r.commit, repo.prefix())
                    .and_then(|f| Catalogue::load(&f))
                {
                    Ok(c) => Some(c),
                    Err(e) => {
                        problems.push(json!(e.with("folder", shown).with("commit", &r.commit)));
                        None
                    }
                }
            });
            let Some(cat) = cat else { continue };
            if !cat.items.contains_key(&part) {
                continue;
            }
            for mut row in naming(cat, of, doc)
                .into_iter()
                .filter(|x| x["item"] == json!(part))
            {
                row["release"] = json!(r.label);
                row["state"] = json!(r.state);
                row["commit"] = json!(r.commit);
                out.push(row);
            }
        }
    }
    out
}

pub fn where_used(session: &mut Session, ask: &Ask) -> Result<Value> {
    let cat = session.catalogue().clone();
    let id = cat.find_document(ask.document)?;
    let of = cat.manifest.project.clone();
    let here = session.root().display().to_string();
    let tag = |rows: Vec<Value>, project: &crate::id::ProjectId, folder: &str| -> Vec<Value> {
        rows.into_iter()
            .map(|mut r| {
                r["catalogue"] = json!(project);
                r["folder"] = json!(folder);
                r
            })
            .collect()
    };
    let mut problems = Vec::new();
    let mut working = tag(naming(&cat, &of, &id), &of, &here);
    let mut released_rows = Vec::new();
    if !ask.working {
        let r = released(session, &of, &id, &mut problems, &here);
        released_rows.extend(tag(r, &of, &here));
    }
    let folders = crate::where_used::folders(session.root(), ask.within, ask.no_dependents);
    let mut searched = Vec::new();
    // Other checkouts opened and searched on threads, eight at a time, as
    // items' where-used does (design 937's bar); the answer keeps the
    // folders' order.
    type Found = (
        String,
        std::result::Result<Option<(crate::id::ProjectId, Vec<Value>, Vec<Value>)>, Value>,
        Vec<Value>,
    );
    let others = crate::numbers::others(session.root(), &folders);
    let mut results: Vec<Found> = Vec::with_capacity(others.len());
    for wave in others.chunks(8) {
        results.extend(std::thread::scope(|s| {
            let handles: Vec<_> = wave
                .iter()
                .map(|dir| {
                    let (of, id, working_only) = (&of, &id, ask.working);
                    s.spawn(move || -> Found {
                        let shown = dir.display().to_string();
                        let mut problems = Vec::new();
                        let mut s = match Session::open(dir) {
                            Ok(s) => s,
                            Err(e) => {
                                return (
                                    shown.clone(),
                                    Err(json!(e.with("folder", &shown))),
                                    problems,
                                );
                            }
                        };
                        let c = s.catalogue().clone();
                        if c.manifest.project == *of {
                            return (shown, Ok(None), problems);
                        }
                        let w = naming(&c, of, id);
                        let r = if working_only {
                            Vec::new()
                        } else {
                            released(&mut s, of, id, &mut problems, &shown)
                        };
                        (
                            shown,
                            Ok(Some((c.manifest.project.clone(), w, r))),
                            problems,
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("a where-used thread doesn't panic"))
                .collect::<Vec<Found>>()
        }));
    }
    for (shown, r, mut p) in results {
        problems.append(&mut p);
        match r {
            Err(e) => problems.push(e),
            Ok(None) => {}
            Ok(Some((project, w, rel))) => {
                searched.push(shown.clone());
                working.extend(tag(w, &project, &shown));
                released_rows.extend(tag(rel, &project, &shown));
            }
        }
    }
    Ok(json!({
        "schema": 1,
        "document": id,
        "number": cat.documents[&id].number,
        "catalogue": of,
        "working": working,
        "released": released_rows,
        "problems": problems,
        "searched": searched,
    }))
}

/// `document_unresolved` for each item's reference to another catalogue's
/// document that no catalogue in `others` (their folders) holds.
pub fn elsewhere(cat: &Catalogue, others: &[std::path::PathBuf]) -> Vec<Fault> {
    let own = &cat.manifest.project;
    let mut by_project: std::collections::BTreeMap<crate::id::ProjectId, &std::path::PathBuf> =
        Default::default();
    for dir in others {
        if let Ok(b) = std::fs::read(dir.join(crate::catalogue::MANIFEST))
            && let Ok(m) = crate::catalogue::Manifest::read(&b)
        {
            by_project.entry(m.project).or_insert(dir);
        }
    }
    let searched: Vec<String> = others.iter().map(|d| d.display().to_string()).collect();
    let mut out = Vec::new();
    for (iid, item) in &cat.items {
        for (i, r) in item.documents.iter().enumerate() {
            let Some(p) = r.catalogue.as_ref().filter(|p| *p != own) else {
                continue;
            };
            let found = by_project.get(p).is_some_and(|dir| {
                crate::session::path_of(dir, &document_path(&r.document)).is_file()
            });
            if !found {
                out.push(warn(
                    Error::new(
                        Code::DocumentUnresolved,
                        format!(
                            "item {iid}: document {} isn't in any catalogue searched",
                            r.text()
                        ),
                    )
                    .with("item", iid)
                    .with("path", item_path(iid))
                    .with("pointer", format!("/documents/{i}"))
                    .with("document", r.text())
                    .with("searched", &searched),
                ));
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

    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};
    use serde_json::json;

    fn source(dir: &std::path::Path, name: &str, bytes: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.display().to_string()
    }

    #[test]
    fn a_document_from_add_to_remove() {
        let (_d, root) = catalogue();
        let outside = tempfile::tempdir().unwrap();
        let file = source(outside.path(), "Spec: A.pdf", b"%PDF one");
        let made = ask(
            &root,
            Request::DocumentAdd {
                file: file.clone(),
                number: Some("DOC-1".into()),
                next: None,
                fields: New {
                    title: Some("Spec".into()),
                    doc_type: Some("specification".into()),
                    ..Default::default()
                },
                expected_revision: None,
            },
        )
        .unwrap();
        let id = made["document"]["id"].as_str().unwrap().to_owned();
        assert_eq!(made["document"]["file"]["name"], "Spec_ A.pdf");
        assert_eq!(made["document"]["file"]["media_type"], "application/pdf");
        assert_eq!(
            made["bytes"],
            format!("documents/{}/{id}/Spec_ A.pdf", &id[..2])
        );
        assert_eq!(
            std::fs::read(root.join(made["bytes"].as_str().unwrap())).unwrap(),
            b"%PDF one"
        );
        let shown = ask(
            &root,
            Request::DocumentShow {
                document: "DOC-1".into(),
            },
        )
        .unwrap();
        assert_eq!(shown["state"]["state"], "in_work");
        assert_eq!(shown["faults"], json!([]));
        let listed = ask(
            &root,
            Request::DocumentList {
                doc_type: Some("specification".into()),
                number: None,
                text: None,
                limit: None,
            },
        )
        .unwrap();
        assert_eq!(listed["documents"][0]["id"], id);
        ask(
            &root,
            Request::DocumentEdit {
                document: id.clone(),
                fields: New {
                    description: Some("Finish".into()),
                    ..Default::default()
                },
                unset: vec![],
                expected_revision: None,
            },
        )
        .unwrap();
        let file2 = source(outside.path(), "Spec B.pdf", b"%PDF two");
        let replaced = ask(
            &root,
            Request::DocumentReplace {
                document: id.clone(),
                file: file2,
                media_type: None,
                expected_revision: None,
            },
        )
        .unwrap();
        assert_eq!(replaced["document"]["file"]["name"], "Spec B.pdf");
        assert!(
            !root.join(made["bytes"].as_str().unwrap()).exists(),
            "the old bytes go"
        );
        let out = outside.path().join("got.pdf");
        let got = ask(
            &root,
            Request::DocumentGet {
                document: id.clone(),
                rev: None,
                out: out.display().to_string(),
            },
        )
        .unwrap();
        assert_eq!(got["size"], 8);
        assert_eq!(std::fs::read(&out).unwrap(), b"%PDF two");
        ask(
            &root,
            Request::DocumentRenumber {
                document: id.clone(),
                number: Some("DOC-2".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        // A document an item names isn't removed without --detach.
        let item = ask(
            &root,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some("A-1".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        let item = item["item"]["id"].as_str().unwrap().to_owned();
        ask(
            &root,
            Request::DocumentAttach {
                document: id.clone(),
                items: vec![item.clone()],
                expected_revision: None,
            },
        )
        .unwrap();
        let busy = ask(
            &root,
            Request::DocumentRemove {
                document: id.clone(),
                detach: false,
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(busy.code, Code::DocumentInUse);
        ask(
            &root,
            Request::DocumentRemove {
                document: id.clone(),
                detach: true,
                expected_revision: None,
            },
        )
        .unwrap();
        assert!(
            !root.join("documents").exists(),
            "record, bytes and empty folders gone"
        );
        let it = ask(&root, Request::ItemShow { item }).unwrap();
        assert!(it["item"].get("documents").is_none(), "detached");
        let gone = ask(&root, Request::DocumentShow { document: id }).unwrap_err();
        assert_eq!(gone.code, Code::DocumentNotFound);
    }

    #[test]
    fn too_large_files_and_bad_media_types_are_refused() {
        let (_d, root) = catalogue();
        let outside = tempfile::tempdir().unwrap();
        let big = outside.path().join("big.bin");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(TOO_LARGE + 1).unwrap(); // sparse: no 100 MiB written
        let e = ask(
            &root,
            Request::DocumentAdd {
                file: big.display().to_string(),
                number: Some("D-1".into()),
                next: None,
                fields: New {
                    title: Some("Big".into()),
                    ..Default::default()
                },
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, Code::DocumentTooLarge);
        let small = source(outside.path(), "a.txt", b"a");
        let e = ask(
            &root,
            Request::DocumentAdd {
                file: small,
                number: Some("D-1".into()),
                next: None,
                fields: New {
                    title: Some("A".into()),
                    media_type: Some("PDF".into()),
                    ..Default::default()
                },
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn documents_are_used_here_and_in_other_catalogues() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path().canonicalize().unwrap();
        let (a, b) = (top.join("a"), top.join("b"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d).unwrap();
            crate::server::init(d, None).unwrap();
        }
        let file = source(&top, "spec.pdf", b"%PDF");
        let made = ask(
            &a,
            Request::DocumentAdd {
                file,
                number: Some("DOC-1".into()),
                next: None,
                fields: New {
                    title: Some("Spec".into()),
                    ..Default::default()
                },
                expected_revision: None,
            },
        )
        .unwrap();
        let doc = made["document"]["id"].as_str().unwrap().to_owned();
        let pa = ask(&a, Request::ProjectInfo {}).unwrap()["project"]
            .as_str()
            .unwrap()
            .to_owned();
        let ia = ask(
            &a,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some("A-1".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        let ib = ask(
            &b,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some("B-1".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        let (ia, ib) = (
            ia["item"]["id"].as_str().unwrap().to_owned(),
            ib["item"]["id"].as_str().unwrap().to_owned(),
        );
        ask(
            &a,
            Request::DocumentAttach {
                document: "DOC-1".into(),
                items: vec![ia.clone(), ia.clone()],
                expected_revision: None,
            },
        )
        .unwrap();
        ask(
            &b,
            Request::DocumentAttach {
                document: format!("{pa}:{doc}"),
                items: vec![ib.clone()],
                expected_revision: None,
            },
        )
        .unwrap();
        let v = ask(
            &a,
            Request::DocumentWhereUsed {
                document: "DOC-1".into(),
                within: vec![top.display().to_string()],
                no_dependents: true,
                working: true,
            },
        )
        .unwrap();
        let rows: Vec<(&str, &str)> = v["working"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["item"].as_str().unwrap(), r["number"].as_str().unwrap()))
            .collect();
        assert_eq!(rows, [(ia.as_str(), "A-1"), (ib.as_str(), "B-1")]);
        ask(
            &a,
            Request::DocumentDetach {
                document: doc.clone(),
                items: vec![ia.clone()],
                expected_revision: None,
            },
        )
        .unwrap();
        let v = ask(
            &a,
            Request::DocumentWhereUsed {
                document: doc,
                within: vec![],
                no_dependents: true,
                working: true,
            },
        )
        .unwrap();
        assert_eq!(v["working"], json!([]));
    }

    #[test]
    fn another_catalogues_document_is_found_or_a_warning() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path().canonicalize().unwrap();
        let (a, b) = (top.join("a"), top.join("b"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d).unwrap();
            crate::server::init(d, None).unwrap();
        }
        let file = source(&top, "spec.pdf", b"%PDF");
        let doc = ask(
            &a,
            Request::DocumentAdd {
                file,
                number: Some("DOC-1".into()),
                next: None,
                fields: New {
                    title: Some("Spec".into()),
                    ..Default::default()
                },
                expected_revision: None,
            },
        )
        .unwrap()["document"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let pa = ask(&a, Request::ProjectInfo {}).unwrap()["project"]
            .as_str()
            .unwrap()
            .to_owned();
        let ib = ask(
            &b,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some("B-1".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap()["item"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        ask(
            &b,
            Request::DocumentAttach {
                document: format!("{pa}:{doc}"),
                items: vec![ib.clone()],
                expected_revision: None,
            },
        )
        .unwrap();
        ask(
            &b,
            Request::DocumentAttach {
                document: format!("{pa}:k3v9q2m8x4c1p"),
                items: vec![ib],
                expected_revision: None,
            },
        )
        .unwrap();
        let r = ask(
            &b,
            Request::Check {
                within: vec![top.display().to_string()],
                against: None,
            },
        )
        .unwrap();
        let unresolved: Vec<&str> = r["faults"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["code"] == "document_unresolved")
            .map(|f| f["details"]["document"].as_str().unwrap())
            .collect();
        assert_eq!(unresolved, [format!("{pa}:k3v9q2m8x4c1p").as_str()]);
    }

    #[test]
    fn a_service_stored_document_is_never_replaced_here() {
        let (_d, root) = catalogue();
        let outside = tempfile::tempdir().unwrap();
        let file = source(outside.path(), "a.pdf", b"%PDF a");
        let made = ask(
            &root,
            Request::DocumentAdd {
                file,
                number: Some("D-1".into()),
                next: None,
                fields: New {
                    title: Some("A".into()),
                    ..Default::default()
                },
                expected_revision: None,
            },
        )
        .unwrap();
        let path = root.join(made["path"].as_str().unwrap());
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v["storage"] = json!("service");
        std::fs::write(&path, crate::canonical::to_string(&v)).unwrap();
        std::fs::remove_file(root.join(made["bytes"].as_str().unwrap())).unwrap();
        let other = source(outside.path(), "b.pdf", b"%PDF b");
        let e = ask(
            &root,
            Request::DocumentReplace {
                document: "D-1".into(),
                file: other,
                media_type: None,
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["storage"].clone()),
            (Code::InvalidParam, json!("service"))
        );
    }
}
