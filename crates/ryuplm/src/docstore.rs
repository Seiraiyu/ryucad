//! The service's document store (design 937), asked by the CLI process
//! only, never by the session: `has`, `PUT`, `GET` and the signed link,
//! and Drive's pin. The routes are ryuplm 3.B's contract, served by
//! `ryu_review::testing::Store` here and by ryupdm-service (plan 3.C).

use crate::error::{Code, Error, Result};
use crate::proto::Request;
use crate::store::Cache;
use serde_json::{Value, json};
use std::path::Path;

/// At most this many hashes in one `has`.
pub const HAS_BATCH: usize = 10_000;

pub struct DocStore {
    client: ryu_review::Client,
    slug: String,
}

/// What an upload did.
#[derive(Debug, Default, PartialEq)]
pub struct Uploaded {
    pub uploaded: Vec<String>,
    pub held: Vec<String>,
    /// Hashes neither the service nor the cache holds.
    pub missing: Vec<String>,
}

/// A Drive file's revision as the pin route answers it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrivePin {
    pub drive_id: String,
    pub revision: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub blake3: String,
}

/// Whether `e` means the service can't be asked (offline, no token, no
/// GitHub remote, not a repository, not in the beta), as the ledger reads
/// it, rather than a refusal of the request.
pub fn offline(e: &Error) -> bool {
    crate::ledger::unavailable(e.clone()).code == Code::LedgerUnavailable
}

fn schema1(v: Value) -> Result<Value> {
    if v["schema"] != json!(1) {
        return Err(Error::new(
            Code::ReviewFailed,
            format!(
                "the service answered schema {}, which this build doesn't read",
                v["schema"]
            ),
        ));
    }
    Ok(v)
}

/// A refusal of a PUT for its size is the document's own code.
fn sized(e: Error, blake3: &str) -> Error {
    if e.code == Code::ReviewLimitExceeded || e.details["status"] == json!(413) {
        Error::new(
            Code::DocumentTooLarge,
            format!("the service refuses {blake3}: {}", e.message),
        )
        .with("blake3", blake3)
        .with("service", &e)
    } else {
        e
    }
}

impl DocStore {
    /// The store for the clone holding `root`, whose `origin` names the
    /// repository; errors as `ryu_review::Session::open` gives them.
    pub fn open(root: &Path) -> Result<DocStore> {
        let s = ryu_review::Session::<crate::Plm>::open(root, &crate::ledger::PLM)?;
        Ok(DocStore {
            slug: s.slug,
            client: s.client,
        })
    }

    /// `slug`'s store at `base` with `token`: a test's stand-in.
    pub fn at(base: &str, token: &str, slug: &str) -> DocStore {
        DocStore {
            client: ryu_review::Client::new(base, token),
            slug: slug.to_owned(),
        }
    }

    fn path(&self, rest: &str) -> String {
        format!("/repos/{}{rest}", self.slug)
    }

    /// Which of `hashes` the service holds.
    pub fn has(&self, hashes: &[String]) -> Result<Vec<String>> {
        let mut held = Vec::new();
        for chunk in hashes.chunks(HAS_BATCH) {
            let v = schema1(self.client.post(
                &self.path("/documents/has"),
                &json!({"schema": 1, "blake3": chunk}),
            )?)?;
            held.extend(
                v["held"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|h| h.as_str().map(str::to_owned)),
            );
        }
        Ok(held)
    }

    /// Uploads each of `want` (hash, size) the service lacks, from `cache`.
    pub fn upload(&self, cache: &Cache, want: &[(String, u64)]) -> Result<Uploaded> {
        let mut hashes: Vec<String> = want.iter().map(|(h, _)| h.clone()).collect();
        hashes.sort();
        hashes.dedup();
        let held = self.has(&hashes)?;
        let mut out = Uploaded {
            held: held.clone(),
            ..Uploaded::default()
        };
        for (h, size) in want {
            if held.contains(h) || out.uploaded.contains(h) || out.missing.contains(h) {
                continue;
            }
            if !cache.has(h, *size) {
                out.missing.push(h.clone());
                continue;
            }
            schema1(
                self.client
                    .put_file(&self.path(&format!("/documents/{h}")), &cache.path(h))
                    .map_err(|e| sized(e, h))?,
            )?;
            out.uploaded.push(h.clone());
        }
        Ok(out)
    }

    /// Fetches `blake3` (`size` bytes) into `cache` through the signed
    /// link, checked: `document_bytes_changed` when the bytes aren't those,
    /// which are never cached.
    pub fn fetch(&self, cache: &Cache, blake3: &str, size: u64) -> Result<()> {
        let v = schema1(
            self.client
                .get(&self.path(&format!("/documents/{blake3}")))?,
        )?;
        let url = v["url"]
            .as_str()
            .ok_or_else(|| Error::new(Code::ReviewFailed, "the service answered no link"))?;
        let changed = |got: Value| {
            Error::new(
                Code::DocumentBytesChanged,
                format!("the service's bytes for {blake3} aren't those; nothing was kept"),
            )
            .with("expected", json!({"size": size, "blake3": blake3}))
            .with("actual", got)
        };
        // ureq's limit is exclusive: `size + 1` reads exactly `size` bytes,
        // and a longer body is refused as longer.
        let bytes = match self.client.download_up_to(url, size + 1) {
            Err(e) if e.details.get("limit").is_some() => {
                return Err(changed(json!({"size": format!("over {size}")})));
            }
            r => r?,
        };
        let got = blake3::hash(&bytes).to_hex().to_string();
        if bytes.len() as u64 != size || got != blake3 {
            return Err(changed(json!({"size": bytes.len(), "blake3": got})));
        }
        cache.put_bytes(&bytes)?;
        Ok(())
    }

    /// Pins Drive file `drive_id`'s head revision into the store.
    pub fn pin(&self, drive_id: &str) -> Result<DrivePin> {
        let v = schema1(self.client.post(
            &self.path(&format!(
                "/drive/files/{}/pin",
                ryu_review::escape(drive_id)
            )),
            &json!({"schema": 1}),
        )?)?;
        let mut o = v.clone();
        o.as_object_mut().map(|m| m.remove("schema"));
        serde_json::from_value(o).map_err(|e| {
            Error::new(
                Code::ReviewFailed,
                format!("the service's pin doesn't read: {e}"),
            )
            .with("answer", &v)
        })
    }
}

/// What sends a request to the catalogue's daemon or answers it in
/// process (the ledger's `Sender`).
pub type Sender<'a> = crate::ledger::Sender<'a>;

/// The warning for bytes left to upload.
pub fn not_uploaded_warning(e: &Error) -> String {
    format!(
        "document_not_uploaded: {}; run ryuplm document upload",
        e.message
    )
}

/// The CLI's side of a request touching service-stored bytes. Before
/// `document.get` and `document.move --storage git` of a service-stored
/// document whose bytes this machine's cache lacks, the bytes are fetched.
/// After any answer with `to_upload`, they are uploaded; when the service
/// can't be asked, the answer says `not_uploaded` and `warnings` says why.
/// Other requests are sent unchanged, with no call to the service and no
/// token looked for.
pub fn around(
    send: &mut Sender,
    open: &dyn Fn() -> Result<DocStore>,
    req: Request,
    warnings: &mut Vec<String>,
) -> Result<Value> {
    let fetch_for = match &req {
        Request::DocumentGet { document, rev, .. }
            if !rev.as_deref().is_some_and(|r| r.starts_with("label:")) =>
        {
            Some((document.clone(), rev.clone()))
        }
        Request::DocumentMove {
            document, storage, ..
        } if storage == "git" => Some((document.clone(), None)),
        _ => None,
    };
    if let Some((document, rev)) = fetch_for {
        let v = send(Request::DocumentLocate {
            documents: vec![document],
            rev,
            storage: Some("service".into()),
            pinned: false,
        })?;
        if let Some(row) = v["documents"].as_array().and_then(|r| r.first())
            && row["cached"] == json!(false)
        {
            let (h, size) = (
                row["file"]["blake3"].as_str().unwrap_or_default(),
                row["file"]["size"].as_u64().unwrap_or(0),
            );
            open()?.fetch(&Cache::open()?, h, size)?;
        }
    }
    let mut out = send(req)?;
    let want: Vec<(String, u64)> = out["to_upload"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((r["blake3"].as_str()?.to_owned(), r["size"].as_u64()?)))
        .collect();
    if want.is_empty() {
        return Ok(out);
    }
    match open().and_then(|s| s.upload(&Cache::open()?, &want)) {
        Ok(u) => {
            out["uploaded"] = json!(u.uploaded);
            if !u.held.is_empty() {
                out["held"] = json!(u.held);
            }
        }
        Err(e) if offline(&e) => {
            warnings.push(not_uploaded_warning(&e));
            out["not_uploaded"] = json!(want.iter().map(|(h, _)| h).collect::<Vec<_>>());
        }
        Err(e) => return Err(e),
    }
    Ok(out)
}

/// `ryuplm document upload [<document>…]`: every service-stored document
/// named (all when none are) whose bytes the service lacks, uploaded from
/// this machine's cache. `{uploaded, held, missing: []}`, or
/// `document_bytes_missing` naming each document neither holds.
pub fn upload_all(
    send: &mut Sender,
    open: &dyn Fn() -> Result<DocStore>,
    documents: &[String],
) -> Result<Value> {
    let v = send(Request::DocumentLocate {
        documents: documents.to_vec(),
        rev: None,
        storage: Some("service".into()),
        pinned: false,
    })?;
    let rows = v["documents"].as_array().cloned().unwrap_or_default();
    let want: Vec<(String, u64)> = rows
        .iter()
        .filter_map(|r| {
            Some((
                r["file"]["blake3"].as_str()?.to_owned(),
                r["file"]["size"].as_u64()?,
            ))
        })
        .collect();
    let u = open()?.upload(&Cache::open()?, &want)?;
    let missing: Vec<Value> = rows
        .iter()
        .filter(|r| {
            r["file"]["blake3"]
                .as_str()
                .is_some_and(|h| u.missing.iter().any(|m| m == h))
        })
        .map(|r| json!({"id": r["id"], "number": r["number"], "blake3": r["file"]["blake3"]}))
        .collect();
    if !missing.is_empty() {
        return Err(Error::new(
            Code::DocumentBytesMissing,
            format!("{} document(s) have bytes neither the service nor this machine's cache holds; upload from the machine that added them, or `document replace`", missing.len()),
        )
        .with("missing", &missing)
        .with("uploaded", &u.uploaded));
    }
    Ok(json!({"uploaded": u.uploaded, "held": u.held, "missing": []}))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ryu_review::testing::Store as Fake;

    #[test]
    fn bytes_go_up_once_and_come_back_checked() {
        let (fake, state) = Fake::start("acme/w");
        let s = DocStore::at(&fake.url, "t", "acme/w");
        let d = tempfile::tempdir().unwrap();
        let cache = crate::store::Cache::at(d.path());
        let h = cache.put_bytes(b"%PDF one").unwrap();
        let up = s.upload(&cache, &[(h.clone(), 8)]).unwrap();
        assert_eq!(
            (up.uploaded.clone(), up.missing.len()),
            (vec![h.clone()], 0)
        );
        let again = s.upload(&cache, &[(h.clone(), 8)]).unwrap();
        assert_eq!(again.held, vec![h.clone()], "has answers it: no second PUT");
        assert_eq!(state.lock().unwrap().puts, 1);
        let other = tempfile::tempdir().unwrap();
        let fresh = crate::store::Cache::at(other.path());
        s.fetch(&fresh, &h, 8).unwrap();
        assert_eq!(&*fresh.get(&h, 8).unwrap().unwrap(), b"%PDF one");
        state.lock().unwrap().tamper = true;
        let third = crate::store::Cache::at(tempfile::tempdir().unwrap().path());
        let e = s.fetch(&third, &h, 8).unwrap_err();
        assert_eq!(e.code, Code::DocumentBytesChanged);
        assert!(!third.has(&h, 8), "tampered bytes are never cached");
    }

    #[test]
    fn an_unreachable_service_is_said_once() {
        let s = DocStore::at("http://127.0.0.1:9", "t", "acme/w");
        let cache = crate::store::Cache::at(tempfile::tempdir().unwrap().path());
        let h = cache.put_bytes(b"x").unwrap();
        let e = s.upload(&cache, &[(h, 1)]).unwrap_err();
        assert!(offline(&e), "{e:?}");
    }

    #[test]
    fn writes_upload_and_reads_fetch_through_the_cli_side() {
        let _l = crate::documents::tests::CACHE_LOCK.lock().unwrap();
        let _c = crate::documents::tests::cache_env();
        let (fake, state) = Fake::start("acme/widgets");
        let (_d, root) = crate::ledger::tests::catalogue(false);
        let outside = tempfile::tempdir().unwrap();
        let src = outside.path().join("a.pdf");
        std::fs::write(&src, b"%PDF a").unwrap();
        let open = || Ok(DocStore::at(&fake.url, "t", "acme/widgets"));
        let mut send = crate::ledger::tests::sender(&root);
        let mut warnings = Vec::new();
        let add = Request::DocumentAdd {
            file: Some(src.display().to_string()),
            number: Some("D-1".into()),
            next: None,
            fields: crate::documents::New {
                title: Some("A".into()),
                ..Default::default()
            },
            storage: Some("service".into()),
            drive: None,
            expected_revision: None,
        };
        let v = around(&mut send, &open, add, &mut warnings).unwrap();
        let h = blake3::hash(b"%PDF a").to_hex().to_string();
        assert_eq!(v["uploaded"], json!([h]));
        assert!(state.lock().unwrap().blobs.contains_key(&h));
        // Another machine: an empty cache, so get fetches first.
        let _other = crate::documents::tests::cache_env();
        let out = outside.path().join("got.pdf");
        let get = Request::DocumentGet {
            document: "D-1".into(),
            rev: None,
            out: out.display().to_string(),
        };
        around(&mut send, &open, get, &mut warnings).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"%PDF a");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn an_offline_upload_is_a_warning_and_the_record_stays() {
        let _l = crate::documents::tests::CACHE_LOCK.lock().unwrap();
        let _c = crate::documents::tests::cache_env();
        let (_d, root) = crate::ledger::tests::catalogue(false);
        let outside = tempfile::tempdir().unwrap();
        let src = outside.path().join("a.pdf");
        std::fs::write(&src, b"%PDF a").unwrap();
        let open = || Ok(DocStore::at("http://127.0.0.1:9", "t", "acme/widgets"));
        let mut send = crate::ledger::tests::sender(&root);
        let mut warnings = Vec::new();
        let v = around(
            &mut send,
            &open,
            Request::DocumentAdd {
                file: Some(src.display().to_string()),
                number: Some("D-1".into()),
                next: None,
                fields: crate::documents::New {
                    title: Some("A".into()),
                    ..Default::default()
                },
                storage: Some("service".into()),
                drive: None,
                expected_revision: None,
            },
            &mut warnings,
        )
        .unwrap();
        assert_eq!(v["not_uploaded"].as_array().unwrap().len(), 1);
        assert!(
            warnings[0].starts_with("document_not_uploaded:"),
            "{warnings:?}"
        );
        assert_eq!(v["document"]["number"], "D-1", "the record is written");
    }

    #[test]
    fn upload_sends_what_the_service_lacks_and_names_what_no_one_has() {
        let _l = crate::documents::tests::CACHE_LOCK.lock().unwrap();
        let _c = crate::documents::tests::cache_env();
        let (fake, state) = Fake::start("acme/widgets");
        let (_d, root) = crate::ledger::tests::catalogue(false);
        let outside = tempfile::tempdir().unwrap();
        let mut send = crate::ledger::tests::sender(&root);
        for (n, b) in [("D-1", b"one".as_slice()), ("D-2", b"two")] {
            let src = outside.path().join(format!("{n}.pdf"));
            std::fs::write(&src, b).unwrap();
            send(Request::DocumentAdd {
                file: Some(src.display().to_string()),
                number: Some(n.into()),
                next: None,
                fields: crate::documents::New {
                    title: Some(n.into()),
                    ..Default::default()
                },
                storage: Some("service".into()),
                drive: None,
                expected_revision: None,
            })
            .unwrap();
        }
        let open = || Ok(DocStore::at(&fake.url, "t", "acme/widgets"));
        let v = upload_all(&mut send, &open, &[]).unwrap();
        assert_eq!(v["uploaded"].as_array().unwrap().len(), 2);
        assert_eq!(
            upload_all(&mut send, &open, &[]).unwrap()["held"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // A third document whose bytes left this machine before upload.
        let src = outside.path().join("D-3.pdf");
        std::fs::write(&src, b"three").unwrap();
        send(Request::DocumentAdd {
            file: Some(src.display().to_string()),
            number: Some("D-3".into()),
            next: None,
            fields: crate::documents::New {
                title: Some("D-3".into()),
                ..Default::default()
            },
            storage: Some("service".into()),
            drive: None,
            expected_revision: None,
        })
        .unwrap();
        let _empty = crate::documents::tests::cache_env();
        let e = upload_all(&mut send, &open, &["D-3".into()]).unwrap_err();
        assert_eq!(e.code, Code::DocumentBytesMissing);
        assert_eq!(e.details["missing"][0]["number"], "D-3");
        assert_eq!(state.lock().unwrap().puts, 2);
    }
}
