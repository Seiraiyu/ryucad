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
}
