//! Package attestations (design 436): what the service signs once it has
//! built an official release's package, carried as the message of the
//! annotated tag `ryupdm/<folder><part>/<label>.package` at the official commit
//! (`<folder>` the project's, `""` at a repository's root).
//! The message and the keys are a release attestation's
//! ([`crate::attest`]); the signature is over
//! `ryupdm.package_attestation.1\0`, so neither kind can be replayed as
//! the other. It names the package's hash ([`super::Manifest::hash`]),
//! not its approvals: those are the release tag's.

use super::BuiltWith;
use crate::attest::{Clause, Signature, check_signature, is_hex, is_time, split_message};
use crate::header;
use ryu_doc::canonical;
use ryu_doc::error::Result;
use ryu_doc::files::{Obj, invalid};
use ryu_doc::id::{Id, ProjectId};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const KIND: &str = "ryupdm.package_attestation";
pub const FORMAT: u64 = 1;
/// What the signature covers before the canonical bytes.
pub const DOMAIN: &[u8] = b"ryupdm.package_attestation.1\0";
/// What a package tag's key adds to its release's: `<part>/<label>.package`.
pub const TAG_SUFFIX: &str = ".package";

/// The key of a release's package tag below the project's tag prefix
/// (`ryupdm/<folder>`).
pub fn tag_key(part: &Id, label: &str) -> String {
    format!("{part}/{label}{TAG_SUFFIX}")
}

/// The package tag's name in the project at `prefix` (`""` or ending in
/// `/`), without `refs/tags/`.
pub fn tag_name(prefix: &str, part: &Id, label: &str) -> String {
    format!(
        "{}{prefix}{}",
        crate::history::TAG_PREFIX,
        tag_key(part, label)
    )
}

/// `ryupdm.package_attestation`, format 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attestation {
    pub built_with: BuiltWith,
    pub commit: String,
    /// The package hash.
    pub hash: String,
    pub label: String,
    pub part: Id,
    pub project: ProjectId,
    /// BLAKE3 of the record's bytes at the official commit.
    pub record: String,
    pub signed_at: String,
}

impl Attestation {
    pub fn to_json(&self) -> Value {
        json!({
            "built_with": {"ryucad": self.built_with.ryucad, "commit": self.built_with.commit},
            "commit": self.commit,
            "format": FORMAT,
            "kind": KIND,
            "hash": self.hash,
            "label": self.label,
            "part": self.part,
            "project": self.project,
            "record": self.record,
            "signed_at": self.signed_at,
        })
    }

    pub fn canonical(&self) -> String {
        canonical::to_string(&self.to_json())
    }

    pub fn from_json(v: &Value) -> Result<Attestation> {
        let mut o = header(v, KIND, FORMAT)?;
        let t = |o: &mut Obj, key: &str, ok: &dyn Fn(&str) -> bool, what: &str| {
            o.take(key)?
                .as_str()
                .filter(|s| ok(s))
                .map(str::to_owned)
                .ok_or_else(|| invalid(&format!("/{key}"), &format!("{key} must be {what}")))
        };
        let bv = o.take("built_with")?;
        let mut b = Obj::new(&bv, "/built_with")?;
        let built_with = BuiltWith {
            ryucad: t(&mut b, "ryucad", &|s| !s.is_empty(), "a version")?,
            commit: t(
                &mut b,
                "commit",
                &super::commit_ok,
                "a commit's hex, or unknown",
            )?,
        };
        b.finish()?;
        let commit = t(&mut o, "commit", &|s| is_hex(s, 40), "40 hex")?;
        let hash = t(&mut o, "hash", &|s| is_hex(s, 64), "64 hex")?;
        let label = t(
            &mut o,
            "label",
            &|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric()),
            "a label",
        )?;
        let part = Id::parse(o.take("part")?.as_str().unwrap_or(""), "part")
            .map_err(|_| invalid("/part", "part must be a part ID"))?;
        let project = ProjectId::parse(o.take("project")?.as_str().unwrap_or(""))
            .map_err(|_| invalid("/project", "project must be a project ID"))?;
        let record = t(&mut o, "record", &|s| is_hex(s, 64), "64 hex")?;
        let signed_at = t(&mut o, "signed_at", &is_time, "a UTC time")?;
        o.finish()?;
        Ok(Attestation {
            built_with,
            commit,
            hash,
            label,
            part,
            project,
            record,
            signed_at,
        })
    }
}

/// The tag message of `attestation` with `signature`.
pub fn message(attestation: &Attestation, signature: &Signature) -> String {
    format!(
        "{}{}\n{{\"ed25519\":\"{}\",\"key\":\"{}\"}}\n{}\n",
        attestation.canonical(),
        crate::attest::BEGIN,
        crate::attest::hex(&signature.ed25519),
        signature.key,
        crate::attest::END,
    )
}

/// What a clone reads about one official release, which its package
/// attestation must say.
pub struct Expect<'a> {
    pub project: &'a ProjectId,
    pub part: &'a Id,
    pub label: &'a str,
    pub commit: &'a str,
    pub record: &'a str,
    /// The package's hash, when a package is at hand; `None` to judge the
    /// tag alone (`release list`).
    pub hash: Option<&'a str>,
    pub keys: &'a BTreeMap<String, [u8; 32]>,
    /// The keys revoked, with their days: [`crate::attest::revoked`] of
    /// `approvals.keys`' revoked entries where the record arrived and at
    /// the release branch's tip, as for its release tag.
    pub revoked: &'a BTreeMap<String, String>,
}

/// How a release's package tag stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Attested {
        attestation: Box<Attestation>,
        key: String,
    },
    Invalid(Clause),
    /// Signed by a revoked key: whatever it says, it proves nothing.
    Revoked {
        key: String,
        revoked: String,
    },
    Missing,
}

impl Verdict {
    pub fn status(&self) -> &'static str {
        match self {
            Verdict::Attested { .. } => "attested",
            Verdict::Invalid(_) => "invalid",
            Verdict::Revoked { .. } => "revoked",
            Verdict::Missing => "missing",
        }
    }
}

/// Judges a package tag's message.
pub fn verify_message(bytes: &[u8], expect: &Expect) -> Verdict {
    let (v, sig, text) = match split_message(bytes) {
        Ok(x) => x,
        Err(c) => return Verdict::Invalid(c),
    };
    let a = match Attestation::from_json(&v) {
        Ok(a) => a,
        Err(e) => return Verdict::Invalid(Clause::Unreadable(e.message)),
    };
    if let Err(c) = check_signature(expect.keys, &sig, DOMAIN, &text) {
        return Verdict::Invalid(c);
    }
    // A revoked key's signature proves nothing, whatever date it claims.
    if let Some(day) = expect.revoked.get(&sig.key) {
        return Verdict::Revoked {
            key: sig.key,
            revoked: day.clone(),
        };
    }
    let fields = [
        ("project", a.project == *expect.project),
        ("part", a.part == *expect.part),
        ("label", a.label == expect.label),
        ("commit", a.commit == expect.commit),
        ("record", a.record == expect.record),
        ("hash", expect.hash.is_none_or(|h| a.hash == h)),
    ];
    if let Some((field, _)) = fields.iter().find(|(_, ok)| !ok) {
        return Verdict::Invalid(Clause::Mismatch(field));
    }
    Verdict::Attested {
        attestation: Box::new(a),
        key: sig.key,
    }
}

/// The verdict on the package tag of the official record at `path`
/// (`releases/<part>/<label>.json`): `Missing` without one, a tag at
/// another commit `Invalid` (`commit`).
pub fn of_release(
    repo: &gix::Repository,
    releases: &crate::history::Releases,
    prefix: &str,
    path: &str,
    hash: Option<&str>,
) -> Verdict {
    let Some(o) = releases.official.get(path) else {
        return Verdict::Missing;
    };
    let Some((part, label)) = path
        .strip_prefix("releases/")
        .and_then(|r| r.strip_suffix(".json"))
        .and_then(|r| r.split_once('/'))
        .and_then(|(p, l)| Some((Id::parse(p, "part").ok()?, l.to_owned())))
    else {
        return Verdict::Missing;
    };
    let key = tag_key(&part, &label);
    let Some(object) = releases.objects.get(&key) else {
        return Verdict::Missing;
    };
    if releases.tags.get(&key).and_then(Clone::clone).as_deref() != Some(o.commit.as_str()) {
        return Verdict::Invalid(Clause::Mismatch("commit"));
    }
    let Ok(blob) = repo.find_object(o.blob) else {
        return Verdict::Invalid(Clause::Unreadable("the record's blob is missing".into()));
    };
    let bytes = blob.data.clone();
    let record = blake3::hash(&bytes).to_hex().to_string();
    let Some(project) = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|t| canonical::parse(t).ok())
        .and_then(|v| crate::record::Record::from_json(&v, crate::record::Kind::Release).ok())
        .map(|r| r.project().clone())
    else {
        return Verdict::Invalid(Clause::Unreadable("the record doesn't read".into()));
    };
    // The keys trusted where the record arrived, as for its release tag.
    let at = repo
        .find_commit(match gix::ObjectId::from_hex(o.commit.as_bytes()) {
            Ok(id) => id,
            Err(_) => return Verdict::Missing,
        })
        .ok()
        .and_then(|c| c.parent_ids().next().map(|p| p.to_string()))
        .and_then(|parent| crate::history::config_at(repo, &parent, prefix).ok())
        .and_then(|c| c.approvals);
    let keys = crate::attest::trusted(&at.as_ref().map(|a| a.keys.clone()).unwrap_or_default());
    // A key revoked now proves nothing it signed earlier.
    let mut revoked =
        crate::attest::revoked(&at.as_ref().map(|a| a.revoked.clone()).unwrap_or_default());
    for (id, day) in releases.tip_approvals.iter().flat_map(|a| &a.revoked) {
        revoked.entry(id.clone()).or_insert_with(|| day.clone());
    }
    let Ok(obj) = repo.find_object(*object) else {
        return Verdict::Invalid(Clause::Unreadable("the tag's object is missing".into()));
    };
    if obj.kind != gix::object::Kind::Tag {
        return Verdict::Invalid(Clause::Lightweight);
    }
    let data = &obj.data;
    match data.windows(2).position(|w| w == b"\n\n") {
        None => Verdict::Invalid(Clause::NoAttestation),
        Some(at) => verify_message(
            &data[at + 2..],
            &Expect {
                project: &project,
                part: &part,
                label: &label,
                commit: &o.commit,
                record: &record,
                hash,
                keys: &keys,
                revoked: &revoked,
            },
        ),
    }
}

/// The tag message of `attestation` signed with the seed `seed`, as the
/// service makes it.
#[cfg(any(test, feature = "testing"))]
pub fn sign(attestation: &Attestation, seed: &[u8; 32]) -> String {
    sign_in(attestation, seed, DOMAIN)
}

/// [`sign`] over another domain: for the tests that a release signature
/// never verifies as a package one.
#[cfg(any(test, feature = "testing"))]
pub fn sign_in(attestation: &Attestation, seed: &[u8; 32], domain: &[u8]) -> String {
    use crate::attest::{key_id, public_key, raw_sign};
    let mut signed = domain.to_vec();
    signed.extend_from_slice(attestation.canonical().as_bytes());
    let signature = Signature {
        key: key_id(&public_key(seed)),
        ed25519: raw_sign(seed, &signed),
    };
    message(attestation, &signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attest::{TEST_SEED, public_key, test_key_id};

    fn attestation() -> Attestation {
        Attestation {
            built_with: BuiltWith {
                ryucad: "0.10.0".into(),
                commit: "1".repeat(40),
            },
            commit: "a".repeat(40),
            hash: "b".repeat(64),
            label: "B".into(),
            part: Id::parse("bracket", "part").unwrap(),
            project: ProjectId::from_bytes([1; 16]),
            record: "c".repeat(64),
            signed_at: "2026-12-01T10:02:44Z".into(),
        }
    }

    fn keys() -> BTreeMap<String, [u8; 32]> {
        BTreeMap::from([(test_key_id(), public_key(&TEST_SEED))])
    }

    static NONE_REVOKED: BTreeMap<String, String> = BTreeMap::new();

    fn expect<'a>(
        a: &'a Attestation,
        keys: &'a BTreeMap<String, [u8; 32]>,
        hash: Option<&'a str>,
    ) -> Expect<'a> {
        Expect {
            project: &a.project,
            part: &a.part,
            label: &a.label,
            commit: &a.commit,
            record: &a.record,
            hash,
            keys,
            revoked: &NONE_REVOKED,
        }
    }

    #[test]
    fn a_revoked_keys_package_tag_proves_nothing() {
        let a = attestation();
        let k = keys();
        let msg = sign(&a, &TEST_SEED);
        let revoked = BTreeMap::from([(test_key_id(), "2026-11-01".to_owned())]);
        let e = Expect {
            revoked: &revoked,
            ..expect(&a, &k, Some(&a.hash))
        };
        assert_eq!(
            verify_message(msg.as_bytes(), &e),
            Verdict::Revoked {
                key: test_key_id(),
                revoked: "2026-11-01".into()
            }
        );
        assert_eq!(verify_message(msg.as_bytes(), &e).status(), "revoked");
    }

    #[test]
    fn a_signed_attestation_verifies_and_round_trips() {
        let a = attestation();
        let k = keys();
        let msg = sign(&a, &TEST_SEED);
        assert_eq!(msg, sign(&a, &TEST_SEED));
        let v = verify_message(msg.as_bytes(), &expect(&a, &k, Some(&a.hash)));
        assert_eq!(v.status(), "attested");
        let bad = "d".repeat(64);
        assert_eq!(
            verify_message(msg.as_bytes(), &expect(&a, &k, Some(&bad))),
            Verdict::Invalid(Clause::Mismatch("hash"))
        );
        assert_eq!(
            verify_message(msg.as_bytes(), &expect(&a, &BTreeMap::new(), None)),
            Verdict::Invalid(Clause::UnknownKey(test_key_id()))
        );
    }

    #[test]
    fn neither_kind_of_signature_verifies_as_the_other() {
        let a = attestation();
        let k = keys();
        // A package attestation signed over the release domain.
        let crossed = sign_in(&a, &TEST_SEED, crate::attest::DOMAIN);
        assert_eq!(
            verify_message(crossed.as_bytes(), &expect(&a, &k, None)),
            Verdict::Invalid(Clause::BadSignature)
        );
        // Its signature line on a release attestation's body.
        let sig_line = sign(&a, &TEST_SEED);
        let sig = &sig_line[sig_line.find(crate::attest::BEGIN).unwrap()..];
        let (_, signature, text) = crate::attest::split_message(sig_line.as_bytes()).unwrap();
        assert_eq!(
            check_signature(&k, &signature, crate::attest::DOMAIN, &text),
            Err(Clause::BadSignature)
        );
        assert!(sig.starts_with(crate::attest::BEGIN));
    }
}
