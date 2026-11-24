//! The service's document store (design 937), asked by the CLI process
//! only, never by the session: `has`, `PUT`, `GET` and the signed link,
//! and Drive's pin. The routes are ryuplm 3.B's contract, served by
//! `ryu_review::testing::Store` here and by ryupdm-service (plan 3.C).

use crate::error::{Code, Error, Result};
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
}
