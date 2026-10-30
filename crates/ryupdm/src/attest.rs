//! Attestations: what the hosted service signs when a release record
//! becomes official under approval rules, carried as the message of the
//! annotated tag `ryupdm/<part>/<label>` (`…/<label>.obsolete` for an
//! obsolete record). A clone verifies them offline against the keys this
//! build trusts ([`HOSTED_KEYS`] and `ryupdm.json`'s `approvals.keys`).
//!
//! The message is the attestation's canonical JSON, then
//! `-----BEGIN RYUPDM SIGNATURE-----`, `{"ed25519": "<128 hex>", "key":
//! "<16 hex>"}` on one line and `-----END RYUPDM SIGNATURE-----`. The
//! signature is Ed25519 over `ryupdm.attestation.1\0` followed by the
//! canonical bytes, so the same attestation and key give the same tag.

use crate::approvals::{Rule, Unmet};
use crate::record::Kind;
use ryucad_doc::doc::canonical;
use ryucad_doc::doc::files::{Obj, header, invalid};
use ryucad_doc::doc::id::{Id, ProjectId};
use ryucad_doc::error::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The hosted service's public keys: `(ID, 64 hex)`. Never remove one:
/// old tags must still verify. A new key needs a ryucad release before the
/// service signs with it (design 261). Task "The hosted key" fills it.
pub const HOSTED_KEYS: &[(&str, &str)] = &[];

/// `bytes` as lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|c| format!("{c:02x}")).collect()
}

/// `N` bytes from exactly `2 N` lower-case hex digits.
pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let b = s.as_bytes();
    if b.len() != 2 * N {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (digit(b[2 * i])? << 4) | digit(b[2 * i + 1])?;
    }
    Some(out)
}

/// A public key's ID: the first 16 hex digits of the BLAKE3 hash of its
/// 32 bytes.
pub fn key_id(public: &[u8; 32]) -> String {
    hex(&blake3::hash(public).as_bytes()[..8])
}

/// The keys an attestation may name: the hosted ones and `extra`
/// (`approvals.keys` as read where the rules are read).
pub fn trusted(extra: &BTreeMap<String, [u8; 32]>) -> BTreeMap<String, [u8; 32]> {
    let mut out: BTreeMap<String, [u8; 32]> = HOSTED_KEYS
        .iter()
        .filter_map(|(id, k)| Some(((*id).to_owned(), unhex::<32>(k)?)))
        .collect();
    out.extend(extra.iter().map(|(id, k)| (id.clone(), *k)));
    out
}

pub const KIND: &str = "ryupdm.attestation";
pub const FORMAT: u64 = 1;

/// A GitHub account, by its numeric ID and the login it had then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: u64,
    pub login: String,
}

/// One approving review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    /// When it was submitted, UTC to the second (`2026-11-02T09:14:03Z`).
    pub at: String,
    /// The head it reviewed.
    pub commit: String,
    pub github: User,
    /// GitHub's review ID.
    pub review: u64,
}

/// The pull request that brought the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pull {
    pub number: u64,
    /// `owner/name`.
    pub repo: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The rules were met.
    Approved,
    /// The record became official without them (a bypassed check, a
    /// direct push).
    Unapproved,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Approved => "approved",
            Outcome::Unapproved => "unapproved",
        }
    }
}

/// `ryupdm.attestation`, format 1: what the service signs for one record
/// made official under rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attestation {
    pub approvals: Vec<Approval>,
    /// The official commit.
    pub commit: String,
    pub label: String,
    /// Each rule not met; empty when approved.
    pub missing: Vec<Unmet>,
    pub outcome: Outcome,
    pub part: Id,
    pub project: ProjectId,
    /// The pull request's author.
    pub proposed_by: User,
    /// `None` for a direct push.
    pub pull: Option<Pull>,
    /// BLAKE3 (64 hex) of the record file's bytes at the official commit.
    pub record: String,
    pub record_kind: Kind,
    /// The rules that applied, read at `rules_at`.
    pub rules: Vec<Rule>,
    /// The official commit's first parent.
    pub rules_at: String,
    /// When the service signed it, UTC to the second.
    pub signed_at: String,
}

fn user_json(u: &User) -> Value {
    json!({"id": u.id, "login": u.login})
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// `YYYY-MM-DDTHH:MM:SSZ`, by shape.
fn is_time(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

fn take_str(
    o: &mut Obj,
    key: &str,
    at: &str,
    ok: impl Fn(&str) -> bool,
    what: &str,
) -> Result<String> {
    let v = o.take(key)?;
    v.as_str()
        .filter(|s| ok(s))
        .map(str::to_owned)
        .ok_or_else(|| invalid(&format!("{at}/{key}"), &format!("{key} must be {what}")))
}

fn take_u64(o: &mut Obj, key: &str, at: &str) -> Result<u64> {
    o.take(key)?.as_u64().ok_or_else(|| {
        invalid(
            &format!("{at}/{key}"),
            &format!("{key} must be a whole number"),
        )
    })
}

fn user_from(v: &Value, at: &str) -> Result<User> {
    let mut o = Obj::new(v, at)?;
    let id = take_u64(&mut o, "id", at)?;
    let login = take_str(
        &mut o,
        "login",
        at,
        crate::approvals::login_ok,
        "a GitHub login",
    )?;
    o.finish()?;
    Ok(User { id, login })
}

impl Attestation {
    pub fn to_json(&self) -> Value {
        let approvals: Vec<Value> = self
            .approvals
            .iter()
            .map(|a| {
                json!({"at": a.at, "commit": a.commit, "github": user_json(&a.github),
                            "review": a.review})
            })
            .collect();
        let mut v = json!({
            "approvals": approvals,
            "commit": self.commit,
            "format": FORMAT,
            "kind": KIND,
            "label": self.label,
            "outcome": self.outcome.as_str(),
            "part": self.part,
            "project": self.project,
            "proposed_by": user_json(&self.proposed_by),
            "record": self.record,
            "record_kind": self.record_kind.as_str(),
            "rules": self.rules.iter().map(Rule::to_json).collect::<Vec<_>>(),
            "rules_at": self.rules_at,
            "signed_at": self.signed_at,
        });
        if let Some(p) = &self.pull {
            v["pull"] = json!({"number": p.number, "repo": p.repo});
        }
        if !self.missing.is_empty() {
            v["missing"] = json!(self.missing.iter().map(Unmet::to_json).collect::<Vec<_>>());
        }
        v
    }

    /// Its canonical JSON: the bytes signed, and the tag message's start.
    pub fn canonical(&self) -> String {
        canonical::to_string(&self.to_json())
    }

    pub fn from_json(v: &Value) -> Result<Attestation> {
        let mut o = header(v, KIND, FORMAT)?;
        let list = |o: &mut Obj, key: &str| -> Result<Vec<Value>> {
            o.take(key)?
                .as_array()
                .cloned()
                .ok_or_else(|| invalid(&format!("/{key}"), &format!("{key} must be an array")))
        };
        let approvals = list(&mut o, "approvals")?
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let at = format!("/approvals/{i}");
                let mut a = Obj::new(a, &at)?;
                let out = Approval {
                    at: take_str(&mut a, "at", &at, is_time, "a UTC time")?,
                    commit: take_str(&mut a, "commit", &at, |s| is_hex(s, 40), "40 hex")?,
                    github: user_from(&a.take("github")?, &format!("{at}/github"))?,
                    review: take_u64(&mut a, "review", &at)?,
                };
                a.finish()?;
                Ok(out)
            })
            .collect::<Result<Vec<_>>>()?;
        let commit = take_str(&mut o, "commit", "", |s| is_hex(s, 40), "40 hex")?;
        let label = take_str(
            &mut o,
            "label",
            "",
            |s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric()),
            "a label",
        )?;
        let missing = match o.take_opt("missing") {
            None => Vec::new(),
            Some(m) => m
                .as_array()
                .ok_or_else(|| invalid("/missing", "missing must be an array"))?
                .iter()
                .enumerate()
                .map(|(i, m)| Unmet::from_json(m, &format!("/missing/{i}")))
                .collect::<Result<Vec<_>>>()?,
        };
        let outcome = match o.take("outcome")?.as_str() {
            Some("approved") => Outcome::Approved,
            Some("unapproved") => Outcome::Unapproved,
            _ => {
                return Err(invalid(
                    "/outcome",
                    "outcome must be approved or unapproved",
                ));
            }
        };
        let part = Id::parse(o.take("part")?.as_str().unwrap_or(""), "part")
            .map_err(|_| invalid("/part", "part must be a part ID"))?;
        let project = ProjectId::parse(o.take("project")?.as_str().unwrap_or(""))
            .map_err(|_| invalid("/project", "project must be a project ID"))?;
        let proposed_by = user_from(&o.take("proposed_by")?, "/proposed_by")?;
        let pull = match o.take_opt("pull") {
            None => None,
            Some(p) => {
                let mut p = Obj::new(&p, "/pull")?;
                let number = take_u64(&mut p, "number", "/pull")?;
                let repo = take_str(
                    &mut p,
                    "repo",
                    "/pull",
                    |s| s.split('/').count() == 2 && !s.starts_with('/') && !s.ends_with('/'),
                    "owner/name",
                )?;
                p.finish()?;
                Some(Pull { number, repo })
            }
        };
        let record = take_str(&mut o, "record", "", |s| is_hex(s, 64), "64 hex")?;
        let record_kind = match o.take("record_kind")?.as_str() {
            Some("release") => Kind::Release,
            Some("obsolete") => Kind::Obsolete,
            _ => {
                return Err(invalid(
                    "/record_kind",
                    "record_kind must be release or obsolete",
                ));
            }
        };
        let rules = list(&mut o, "rules")?
            .iter()
            .enumerate()
            .map(|(i, r)| Rule::from_json(r, &format!("/rules/{i}")))
            .collect::<Result<Vec<_>>>()?;
        let rules_at = take_str(&mut o, "rules_at", "", |s| is_hex(s, 40), "40 hex")?;
        let signed_at = take_str(&mut o, "signed_at", "", is_time, "a UTC time")?;
        o.finish()?;
        Ok(Attestation {
            approvals,
            commit,
            label,
            missing,
            outcome,
            part,
            project,
            proposed_by,
            pull,
            record,
            record_kind,
            rules,
            rules_at,
            signed_at,
        })
    }
}

/// What the signature covers before the canonical bytes.
pub const DOMAIN: &[u8] = b"ryupdm.attestation.1\0";
pub const BEGIN: &str = "-----BEGIN RYUPDM SIGNATURE-----";
pub const END: &str = "-----END RYUPDM SIGNATURE-----";
/// The longest tag message read: anything longer is refused unparsed.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// A signature and the ID of the key that made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub key: String,
    pub ed25519: [u8; 64],
}

/// The tag message: the attestation's canonical JSON, then its signature
/// between the markers, each on its own line.
pub fn message(attestation: &Attestation, signature: &Signature) -> String {
    format!(
        "{}{BEGIN}\n{{\"ed25519\":\"{}\",\"key\":\"{}\"}}\n{END}\n",
        attestation.canonical(),
        hex(&signature.ed25519),
        signature.key
    )
}

/// Why a tag's attestation doesn't hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clause {
    /// The tag is lightweight: no message at all.
    Lightweight,
    /// A message with no signature block.
    NoAttestation,
    TooLarge,
    /// The message, the attestation or the signature line doesn't read.
    Unreadable(String),
    /// Signed by a key this build doesn't trust (its ID).
    UnknownKey(String),
    BadSignature,
    /// A field that isn't what this clone reads: `project`, `part`,
    /// `label`, `record_kind`, `commit`, `record`, `rules` or `rules_at`.
    Mismatch(&'static str),
    /// It says approved, but its approvals don't meet its rules.
    NotMet,
    /// It counts the proposer as an approver.
    ProposerApproved,
}

impl Clause {
    pub fn name(&self) -> &'static str {
        match self {
            Clause::Lightweight => "lightweight",
            Clause::NoAttestation => "no_attestation",
            Clause::TooLarge => "too_large",
            Clause::Unreadable(_) => "unreadable",
            Clause::UnknownKey(_) => "unknown_key",
            Clause::BadSignature => "bad_signature",
            Clause::Mismatch(f) => f,
            Clause::NotMet => "not_met",
            Clause::ProposerApproved => "proposer_approved",
        }
    }

    /// As people read it, after "the attestation …".
    pub fn text(&self) -> String {
        match self {
            Clause::Lightweight => "is missing: the tag is lightweight".into(),
            Clause::NoAttestation => "is missing: the tag's message holds none".into(),
            Clause::TooLarge => {
                format!("is too large: tag messages are read up to {MAX_MESSAGE} bytes")
            }
            Clause::Unreadable(why) => format!("doesn't read: {why}"),
            Clause::UnknownKey(k) => {
                format!("is signed by key {k}, which this ryucad doesn't trust")
            }
            Clause::BadSignature => "has a signature that doesn't verify".into(),
            Clause::Mismatch(f) => format!("names another {f} than this clone reads"),
            Clause::NotMet => "says approved, but its approvals don't meet its rules".into(),
            Clause::ProposerApproved => "counts the proposer as an approver".into(),
        }
    }
}

fn unreadable(why: &str) -> Clause {
    Clause::Unreadable(why.to_owned())
}

/// Reads a tag message: the attestation, its signature and the canonical
/// text signed.
pub fn parse_message(
    bytes: &[u8],
) -> std::result::Result<(Attestation, Signature, String), Clause> {
    if bytes.len() > MAX_MESSAGE {
        return Err(Clause::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| unreadable("the message isn't UTF-8"))?;
    let begin = format!("{BEGIN}\n");
    let Some(at) = text.find(&begin) else {
        return Err(Clause::NoAttestation);
    };
    let (body, rest) = text.split_at(at);
    let rest = &rest[begin.len()..];
    let (line, after) = rest
        .split_once('\n')
        .ok_or_else(|| unreadable("the signature has no end marker"))?;
    if after.trim_end() != END {
        return Err(unreadable("the signature has no end marker"));
    }
    let sig: Value =
        serde_json::from_str(line).map_err(|_| unreadable("the signature line isn't JSON"))?;
    let sig = sig.as_object().filter(|o| o.len() == 2).and_then(|o| {
        Some(Signature {
            key: o.get("key")?.as_str().filter(|k| is_hex(k, 16))?.to_owned(),
            ed25519: unhex::<64>(o.get("ed25519")?.as_str()?)?,
        })
    });
    let sig = sig.ok_or_else(|| unreadable("the signature line isn't {\"ed25519\", \"key\"}"))?;
    let v = canonical::parse(body).map_err(|e| Clause::Unreadable(e.message))?;
    if canonical::to_string(&v) != body {
        return Err(unreadable("the attestation isn't canonical JSON"));
    }
    let a = Attestation::from_json(&v).map_err(|e| Clause::Unreadable(e.message))?;
    Ok((a, sig, body.to_owned()))
}

/// What a clone reads about one official record under rules, which its
/// attestation must say.
pub struct Expect<'a> {
    /// The record's own `project`.
    pub project: &'a ProjectId,
    pub part: &'a Id,
    pub label: &'a str,
    pub kind: Kind,
    /// The official commit.
    pub commit: &'a str,
    /// BLAKE3 (64 hex) of the record's bytes at the official commit.
    pub record: &'a str,
    /// The rules that applied, read at `rules_at`.
    pub rules: &'a [Rule],
    /// The official commit's first parent.
    pub rules_at: &'a str,
    /// The keys trusted: [`trusted`] of `approvals.keys` at `rules_at`.
    pub keys: &'a BTreeMap<String, [u8; 32]>,
}

/// How an official record's attestation stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Approved {
        attestation: Attestation,
        key: String,
    },
    Unapproved {
        attestation: Attestation,
        key: String,
    },
    Invalid(Clause),
    /// No tag here: not fetched, or the service hasn't tagged it yet.
    Missing,
}

impl Verdict {
    pub fn status(&self) -> &'static str {
        match self {
            Verdict::Approved { .. } => "approved",
            Verdict::Unapproved { .. } => "unapproved",
            Verdict::Invalid(_) => "invalid",
            Verdict::Missing => "missing",
        }
    }

    pub fn attestation(&self) -> Option<(&Attestation, &str)> {
        match self {
            Verdict::Approved { attestation, key } | Verdict::Unapproved { attestation, key } => {
                Some((attestation, key))
            }
            _ => None,
        }
    }
}

/// Judges a tag message against what this clone reads.
pub fn verify_message(message: &[u8], expect: &Expect) -> Verdict {
    let (a, sig, text) = match parse_message(message) {
        Ok(x) => x,
        Err(c) => return Verdict::Invalid(c),
    };
    let Some(public) = expect.keys.get(&sig.key) else {
        return Verdict::Invalid(Clause::UnknownKey(sig.key));
    };
    let Ok(public) = ed25519_dalek::VerifyingKey::from_bytes(public) else {
        return Verdict::Invalid(Clause::UnknownKey(sig.key));
    };
    let mut signed = DOMAIN.to_vec();
    signed.extend_from_slice(text.as_bytes());
    let signature = ed25519_dalek::Signature::from_bytes(&sig.ed25519);
    if public.verify_strict(&signed, &signature).is_err() {
        return Verdict::Invalid(Clause::BadSignature);
    }
    let fields = [
        ("project", a.project == *expect.project),
        ("part", a.part == *expect.part),
        ("label", a.label == expect.label),
        ("record_kind", a.record_kind == expect.kind),
        ("commit", a.commit == expect.commit),
        ("record", a.record == expect.record),
        ("rules", a.rules == expect.rules),
        ("rules_at", a.rules_at == expect.rules_at),
    ];
    if let Some((field, _)) = fields.iter().find(|(_, ok)| !ok) {
        return Verdict::Invalid(Clause::Mismatch(field));
    }
    match a.outcome {
        Outcome::Unapproved => Verdict::Unapproved {
            attestation: a,
            key: sig.key,
        },
        Outcome::Approved => {
            let proposer = &a.proposed_by;
            if a.approvals.iter().any(|x| {
                x.github.id == proposer.id || x.github.login.eq_ignore_ascii_case(&proposer.login)
            }) {
                return Verdict::Invalid(Clause::ProposerApproved);
            }
            let reviews: Vec<crate::approvals::Review> = a
                .approvals
                .iter()
                .map(|x| crate::approvals::Review {
                    login: x.github.login.clone(),
                    state: crate::approvals::ReviewState::Approve,
                })
                .collect();
            if !crate::approvals::judge(&a.rules, &reviews, &proposer.login).met {
                return Verdict::Invalid(Clause::NotMet);
            }
            Verdict::Approved {
                attestation: a,
                key: sig.key,
            }
        }
    }
}

/// The seed of the key ryupdm's tests and ryucad's scenarios sign with.
/// Never trusted unless a project's `approvals.keys` names it.
#[cfg(any(test, feature = "testing"))]
pub const TEST_SEED: [u8; 32] = [7; 32];

/// The public key of the Ed25519 seed `seed`.
#[cfg(any(test, feature = "testing"))]
pub fn public_key(seed: &[u8; 32]) -> [u8; 32] {
    ed25519_dalek::SigningKey::from_bytes(seed)
        .verifying_key()
        .to_bytes()
}

/// The test key's ID.
#[cfg(any(test, feature = "testing"))]
pub fn test_key_id() -> String {
    key_id(&public_key(&TEST_SEED))
}

/// Ed25519 over `bytes`, as RFC 8032 defines it.
#[cfg(any(test, feature = "testing"))]
pub fn raw_sign(seed: &[u8; 32], bytes: &[u8]) -> [u8; 64] {
    use ed25519_dalek::Signer;
    ed25519_dalek::SigningKey::from_bytes(seed)
        .sign(bytes)
        .to_bytes()
}

/// The tag message of `attestation` signed with the seed `seed`, as the
/// service makes it.
#[cfg(any(test, feature = "testing"))]
pub fn sign(attestation: &Attestation, seed: &[u8; 32]) -> String {
    let mut signed = DOMAIN.to_vec();
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

    #[test]
    fn hex_round_trips_lower_case_only() {
        let b: [u8; 4] = [0, 0x0f, 0xa0, 0xff];
        assert_eq!(hex(&b), "000fa0ff");
        assert_eq!(unhex::<4>("000fa0ff"), Some(b));
        assert_eq!(unhex::<4>("000FA0FF"), None);
        assert_eq!(unhex::<4>("000fa0f"), None);
        assert_eq!(unhex::<4>("000fa0fg"), None);
    }

    #[test]
    fn a_key_id_is_the_start_of_its_hash() {
        let k = [1u8; 32];
        let id = key_id(&k);
        assert_eq!(id.len(), 16);
        assert_eq!(id, hex(&blake3::hash(&k).as_bytes()[..8]));
    }

    #[test]
    fn hosted_keys_are_well_formed_and_trusted() {
        for (id, key) in HOSTED_KEYS {
            let bytes = unhex::<32>(key).expect("64 hex");
            assert_eq!(key_id(&bytes), *id);
            assert!(ed25519_dalek::VerifyingKey::from_bytes(&bytes).is_ok());
        }
        let extra = BTreeMap::from([("x".to_owned(), [2u8; 32])]);
        let t = trusted(&extra);
        assert_eq!(t.len(), HOSTED_KEYS.len() + 1);
        assert_eq!(t["x"], [2u8; 32]);
    }

    pub(crate) fn sample() -> Attestation {
        Attestation {
            approvals: vec![Approval {
                at: "2026-11-02T09:14:03Z".into(),
                commit: "1".repeat(40),
                github: User {
                    id: 1234567,
                    login: "alice".into(),
                },
                review: 2154874321,
            }],
            commit: "2".repeat(40),
            label: "B".into(),
            missing: vec![],
            outcome: Outcome::Approved,
            part: Id::parse("bracket", "part").unwrap(),
            project: ProjectId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            proposed_by: User {
                id: 7654321,
                login: "dstonely".into(),
            },
            pull: Some(Pull {
                number: 42,
                repo: "acme/widgets".into(),
            }),
            record: "3".repeat(64),
            record_kind: Kind::Release,
            rules: vec![Rule {
                parts: vec!["*".into()],
                count: 1,
                from: vec!["alice".into(), "bob".into()],
            }],
            rules_at: "4".repeat(40),
            signed_at: "2026-11-02T09:20:11Z".into(),
        }
    }

    #[test]
    fn an_attestation_round_trips_canonically() {
        let a = sample();
        let text = a.canonical();
        assert!(ryucad_doc::doc::canonical::is_canonical(&text));
        let v = ryucad_doc::doc::canonical::parse(&text).unwrap();
        assert_eq!(v["kind"], "ryupdm.attestation");
        assert_eq!(v["format"], 1);
        assert_eq!(v["record_kind"], "release");
        assert!(v.get("missing").is_none());
        assert_eq!(Attestation::from_json(&v).unwrap(), a);
        // Unapproved, with what was missing and no pull request.
        let mut u = a.clone();
        u.outcome = Outcome::Unapproved;
        u.approvals.clear();
        u.pull = None;
        u.record_kind = Kind::Obsolete;
        u.missing = vec![Unmet {
            rule: u.rules[0].clone(),
            needed: 1,
        }];
        let v = ryucad_doc::doc::canonical::parse(&u.canonical()).unwrap();
        assert_eq!(
            (v["outcome"].as_str(), v.get("pull")),
            (Some("unapproved"), None)
        );
        assert_eq!(Attestation::from_json(&v).unwrap(), u);
    }

    #[test]
    fn attestation_faults_are_refused() {
        let good = ryucad_doc::doc::canonical::parse(&sample().canonical()).unwrap();
        for (key, bad) in [
            ("commit", json!("abc")),
            ("rules_at", json!("4".repeat(41))),
            ("record", json!("3".repeat(63))),
            ("signed_at", json!("yesterday")),
            ("outcome", json!("maybe")),
            ("record_kind", json!("draft")),
            ("label", json!("")),
            ("project", json!("x")),
            ("kind", json!("ryupdm.release")),
            ("format", json!(2)),
        ] {
            let mut v = good.clone();
            v[key] = bad;
            assert!(Attestation::from_json(&v).is_err(), "{key}");
        }
        let mut v = good;
        v["extra"] = json!(1);
        assert!(Attestation::from_json(&v).is_err());
    }

    #[test]
    fn signing_is_rfc_8032s_ed25519() {
        // RFC 8032 section 7.1, TEST 1: the empty message.
        let seed = unhex::<32>("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
            .unwrap();
        assert_eq!(
            hex(&public_key(&seed)),
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        );
        assert_eq!(
            hex(&raw_sign(&seed, b"")),
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        );
    }

    #[test]
    fn a_message_is_the_attestation_then_its_signature() {
        let a = sample();
        let m = sign(&a, &TEST_SEED);
        // Deterministic: the same attestation and key, the same tag.
        assert_eq!(m, sign(&a, &TEST_SEED));
        let (text, rest) = m.split_at(m.find(BEGIN).unwrap());
        assert_eq!(text, a.canonical());
        let lines: Vec<&str> = rest.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!((lines[0], lines[2]), (BEGIN, END));
        assert!(m.ends_with(&format!("{END}\n")));
        let sig: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(sig["key"], test_key_id());
        assert_eq!(sig["ed25519"].as_str().unwrap().len(), 128);
        let (b, s, t) = parse_message(m.as_bytes()).unwrap();
        assert_eq!((b, s.key, t), (a.clone(), test_key_id(), a.canonical()));
    }

    #[test]
    fn messages_that_dont_read_say_why() {
        let a = sample();
        let m = sign(&a, &TEST_SEED);
        let clause = |bytes: &[u8]| parse_message(bytes).unwrap_err().name();
        assert_eq!(clause(b"release B of bracket\n"), "no_attestation");
        assert_eq!(clause(&vec![b'x'; MAX_MESSAGE + 1]), "too_large");
        assert_eq!(clause(&[0xff, 0xfe]), "unreadable");
        // Not canonical: the same JSON on one line.
        let one_line = m.replacen(&a.canonical(), &format!("{}\n", a.to_json()), 1);
        assert_eq!(clause(one_line.as_bytes()), "unreadable");
        // No end marker.
        assert_eq!(clause(m.replace(END, "").as_bytes()), "unreadable");
        // A signature line that isn't the object.
        let mangled = m.replacen("\"key\"", "\"kee\"", 1);
        assert_eq!(clause(mangled.as_bytes()), "unreadable");
    }

    struct Case {
        a: Attestation,
        keys: BTreeMap<String, [u8; 32]>,
    }

    impl Case {
        fn new() -> Case {
            Case {
                a: sample(),
                keys: BTreeMap::from([(test_key_id(), public_key(&TEST_SEED))]),
            }
        }

        fn verdict(&self, message: &str) -> Verdict {
            let e = sample();
            verify_message(
                message.as_bytes(),
                &Expect {
                    project: &e.project,
                    part: &e.part,
                    label: &e.label,
                    kind: e.record_kind,
                    commit: &e.commit,
                    record: &e.record,
                    rules: &e.rules,
                    rules_at: &e.rules_at,
                    keys: &self.keys,
                },
            )
        }
    }

    #[test]
    fn an_approved_attestation_verifies() {
        let c = Case::new();
        match c.verdict(&sign(&c.a, &TEST_SEED)) {
            Verdict::Approved { attestation, key } => {
                assert_eq!((attestation, key), (c.a.clone(), test_key_id()))
            }
            other => panic!("{other:?}"),
        }
        let mut u = c.a.clone();
        u.outcome = Outcome::Unapproved;
        u.approvals.clear();
        assert_eq!(c.verdict(&sign(&u, &TEST_SEED)).status(), "unapproved");
    }

    #[test]
    fn every_clause_that_fails_is_named() {
        let c = Case::new();
        let invalid = |m: &str| match c.verdict(m) {
            Verdict::Invalid(clause) => clause.name(),
            other => panic!("{other:?}"),
        };
        // An unknown key: signed with another seed.
        assert_eq!(invalid(&sign(&c.a, &[8; 32])), "unknown_key");
        // A bad signature: one byte of the signed text changed after signing.
        let m = sign(&c.a, &TEST_SEED).replacen("\"B\"", "\"C\"", 1);
        assert_eq!(invalid(&m), "bad_signature");
        // Fields this clone reads otherwise, each signed properly.
        let fields: [(&str, fn(&mut Attestation)); 8] = [
            ("project", |a| {
                a.project = ProjectId::parse("ffffffffffffffffffffffffffffffff").unwrap()
            }),
            ("part", |a| a.part = Id::parse("other", "part").unwrap()),
            ("label", |a| a.label = "C".into()),
            ("record_kind", |a| a.record_kind = Kind::Obsolete),
            ("commit", |a| a.commit = "5".repeat(40)),
            ("record", |a| a.record = "6".repeat(64)),
            ("rules", |a| a.rules[0].count = 2),
            ("rules_at", |a| a.rules_at = "7".repeat(40)),
        ];
        for (field, change) in fields {
            let mut a = c.a.clone();
            change(&mut a);
            assert_eq!(invalid(&sign(&a, &TEST_SEED)), field);
        }
        // Approved without the approvals.
        let mut a = c.a.clone();
        a.approvals[0].github.login = "mallory".into();
        assert_eq!(invalid(&sign(&a, &TEST_SEED)), "not_met");
        // The proposer as approver, even when the rules would count them.
        let mut a = c.a.clone();
        a.proposed_by = a.approvals[0].github.clone();
        assert_eq!(invalid(&sign(&a, &TEST_SEED)), "proposer_approved");
        assert_eq!(invalid("just a note\n"), "no_attestation");
    }
}
