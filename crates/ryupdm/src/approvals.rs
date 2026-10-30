//! Approval rules (`approvals` in `ryupdm.json`, format 2) and how
//! reviews meet them. [`judge`] is the one ryupdm rule the hosted service
//! links: the service's verdict on a pull request and a clone's
//! verification of a release tag ([`crate::attest`]) agree by
//! construction.

use ryucad_doc::doc::files::Obj;
use ryucad_doc::doc::id::Id;
use ryucad_doc::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// `approval_rule_invalid` at the JSON pointer `at`.
pub(crate) fn fault(at: &str, message: impl Into<String>) -> Error {
    Error::new(Code::ApprovalRuleInvalid, message).with("pointer", at)
}

/// A shape fault inside `approvals` (`invalid_document` from [`Obj`]) as
/// an approval rule's, keeping its pointer.
fn recode(mut e: Error) -> Error {
    e.code = Code::ApprovalRuleInvalid;
    e
}

/// Whether `s` is a GitHub login: 1 to 39 letters, digits and single
/// hyphens, starting and ending with a letter or digit.
pub fn login_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 39
        && b[0].is_ascii_alphanumeric()
        && b.iter().enumerate().all(|(i, c)| {
            c.is_ascii_alphanumeric()
                || (*c == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_alphanumeric))
        })
}

/// Whether `s` is a pattern of part IDs: what an ID holds, with `*` and
/// `?`, 1 to 63 characters.
pub fn glob_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-' | b'*' | b'?')
        })
}

/// Whether `id` matches `pattern`: `*` any run of characters, `?` one.
pub fn glob(pattern: &str, id: &str) -> bool {
    let (p, s) = (pattern.as_bytes(), id.as_bytes());
    let (mut i, mut j) = (0, 0);
    // The last star and where in `s` it has matched up to.
    let mut star: Option<(usize, usize)> = None;
    while j < s.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == s[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some((i, j));
            i += 1;
        } else if let Some((si, sj)) = star {
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == b'*')
}

/// One rule: at least `count` of `from` (GitHub logins) approve releases
/// of the parts `parts` match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub parts: Vec<String>,
    pub count: u64,
    pub from: Vec<String>,
}

impl Rule {
    /// Whether the rule applies to `part`.
    pub fn applies(&self, part: &Id) -> bool {
        self.parts.iter().any(|p| glob(p, part.as_str()))
    }

    /// Whether the rule names `login`, regardless of case.
    pub fn names(&self, login: &str) -> bool {
        self.from.iter().any(|f| f.eq_ignore_ascii_case(login))
    }

    pub fn to_json(&self) -> Value {
        json!({"count": self.count, "from": self.from, "parts": self.parts})
    }

    /// As people read it: `2 of dave, erin, frank for motor-*, gearbox`.
    pub fn text(&self) -> String {
        format!(
            "{} of {} for {}",
            self.count,
            self.from.join(", "),
            self.parts.join(", ")
        )
    }

    /// Reads the rule at `at` (`/approvals/rules/<i>`).
    pub fn from_json(v: &Value, at: &str) -> Result<Rule> {
        let mut o = Obj::new(v, at).map_err(recode)?;
        let strings = |o: &mut Obj, key: &str| -> Result<Vec<String>> {
            let v = o.take(key).map_err(recode)?;
            let list: Option<Vec<String>> = v
                .as_array()
                .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_owned)).collect());
            list.filter(|l| !l.is_empty()).ok_or_else(|| {
                fault(
                    &format!("{at}/{key}"),
                    format!("{key} must be a non-empty array of strings"),
                )
            })
        };
        let parts = strings(&mut o, "parts")?;
        for (i, p) in parts.iter().enumerate() {
            if !glob_ok(p) {
                return Err(fault(
                    &format!("{at}/parts/{i}"),
                    format!(
                        "{p:?} isn't a pattern of part IDs: use a-z, 0-9, _ and -, with * and ?"
                    ),
                ));
            }
        }
        let from = strings(&mut o, "from")?;
        let mut seen: Vec<String> = Vec::new();
        for (i, f) in from.iter().enumerate() {
            let here = format!("{at}/from/{i}");
            if f.starts_with('@') {
                return Err(fault(
                    &here,
                    format!("{f} is a GitHub team: teams come in a later ryupdm, so name the people for now"),
                )
                .with("team", f));
            }
            if !login_ok(f) {
                return Err(fault(&here, format!("{f:?} isn't a GitHub login")));
            }
            let lower = f.to_ascii_lowercase();
            if seen.contains(&lower) {
                return Err(fault(&here, format!("{f} is named twice")));
            }
            seen.push(lower);
        }
        let count = o.take("count").map_err(recode)?;
        let count = count
            .as_u64()
            .filter(|c| (1..=from.len() as u64).contains(c))
            .ok_or_else(|| {
                fault(
                    &format!("{at}/count"),
                    format!("count must be a whole number from 1 to {}", from.len()),
                )
            })?;
        o.finish().map_err(recode)?;
        Ok(Rule { parts, count, from })
    }
}

/// `approvals`: the rules, and keys trusted besides the hosted ones.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Approvals {
    /// Key IDs to Ed25519 public keys.
    pub keys: BTreeMap<String, [u8; 32]>,
    pub rules: Vec<Rule>,
}

impl Approvals {
    /// The rules that apply to `part`, in order.
    pub fn applicable(&self, part: &Id) -> Vec<Rule> {
        self.rules
            .iter()
            .filter(|r| r.applies(part))
            .cloned()
            .collect()
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"rules": self.rules.iter().map(Rule::to_json).collect::<Vec<_>>()});
        if !self.keys.is_empty() {
            v["keys"] = Value::Object(
                self.keys
                    .iter()
                    .map(|(id, k)| (id.clone(), json!(crate::attest::hex(k))))
                    .collect(),
            );
        }
        v
    }

    /// Reads `approvals` at `at` (`/approvals`).
    pub fn from_json(v: &Value, at: &str) -> Result<Approvals> {
        let mut o = Obj::new(v, at).map_err(recode)?;
        let rules = o.take("rules").map_err(recode)?;
        let list = rules
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| fault(&format!("{at}/rules"), "rules must be a non-empty array"))?;
        let rules = list
            .iter()
            .enumerate()
            .map(|(i, r)| Rule::from_json(r, &format!("{at}/rules/{i}")))
            .collect::<Result<Vec<_>>>()?;
        let mut keys = BTreeMap::new();
        if let Some(k) = o.take_opt("keys") {
            let map = k.as_object().ok_or_else(|| {
                fault(
                    &format!("{at}/keys"),
                    "keys must be an object of key IDs to public keys",
                )
            })?;
            for (id, key) in map {
                let here = format!("{at}/keys/{id}");
                let bytes = key
                    .as_str()
                    .and_then(crate::attest::unhex::<32>)
                    .ok_or_else(|| fault(&here, "a key is 64 hex digits: an Ed25519 public key"))?;
                let want = crate::attest::key_id(&bytes);
                if *id != want {
                    return Err(fault(
                        &here,
                        format!("{id} isn't this key's ID: it is {want}"),
                    ));
                }
                if ed25519_dalek::VerifyingKey::from_bytes(&bytes).is_err() {
                    return Err(fault(&here, "not an Ed25519 public key"));
                }
                keys.insert(id.clone(), bytes);
            }
        }
        o.finish().map_err(recode)?;
        Ok(Approvals { keys, rules })
    }
}

/// What a review says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewState {
    Approve,
    RequestChanges,
}

/// One reviewer's review, as GitHub gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub login: String,
    pub state: ReviewState,
}

/// A rule not met, and how many more approvals it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unmet {
    pub rule: Rule,
    pub needed: u64,
}

impl Unmet {
    pub fn to_json(&self) -> Value {
        json!({"needed": self.needed, "rule": self.rule.to_json()})
    }

    pub fn from_json(v: &Value, at: &str) -> Result<Unmet> {
        let mut o = Obj::new(v, at)?;
        let needed = o.take("needed")?;
        let needed = needed.as_u64().filter(|n| *n >= 1).ok_or_else(|| {
            ryucad_doc::doc::files::invalid(
                &format!("{at}/needed"),
                "needed must be a whole number from 1",
            )
        })?;
        let rule = Rule::from_json(&o.take("rule")?, &format!("{at}/rule"))?;
        o.finish()?;
        Ok(Unmet { rule, needed })
    }
}

/// How reviews stand against rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Judgement {
    /// Every rule met and nothing blocking.
    pub met: bool,
    /// The approvers that count (a rule names them), as they wrote their
    /// logins, by login regardless of case.
    pub counted: Vec<String>,
    /// Each rule not met.
    pub missing: Vec<Unmet>,
    /// Logins a rule names whose review requests changes.
    pub blockers: Vec<String>,
}

/// Judges `reviews` (in the order they were made: a login's last review
/// stands) against `rules`, every one of which must be met. The proposer's
/// reviews never count; one approval counts towards every rule naming it;
/// a change request from a login a rule names blocks until it is replaced.
pub fn judge(rules: &[Rule], reviews: &[Review], proposer: &str) -> Judgement {
    let mut last: BTreeMap<String, &Review> = BTreeMap::new();
    for r in reviews {
        if !r.login.eq_ignore_ascii_case(proposer) {
            last.insert(r.login.to_ascii_lowercase(), r);
        }
    }
    let named = |l: &str| rules.iter().any(|r| r.names(l));
    let mut counted = Vec::new();
    let mut blockers = Vec::new();
    for r in last.values().filter(|r| named(&r.login)) {
        match r.state {
            ReviewState::Approve => counted.push(r.login.clone()),
            ReviewState::RequestChanges => blockers.push(r.login.clone()),
        }
    }
    let missing: Vec<Unmet> = rules
        .iter()
        .filter_map(|r| {
            let n = counted.iter().filter(|l| r.names(l)).count() as u64;
            (n < r.count).then(|| Unmet {
                rule: r.clone(),
                needed: r.count - n,
            })
        })
        .collect();
    Judgement {
        met: missing.is_empty() && blockers.is_empty(),
        counted,
        missing,
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn part(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    #[test]
    fn globs_take_star_and_question_mark() {
        for (p, id, ok) in [
            ("*", "anything", true),
            ("motor-*", "motor-12", true),
            ("motor-*", "motor-", true),
            ("motor-*", "gearbox", false),
            ("gearbox", "gearbox", true),
            ("gearbox", "gearbox2", false),
            ("b?x", "box", true),
            ("b?x", "bx", false),
            ("*-a*b", "x-aqqb", true),
            ("*-a*b", "x-aqqc", false),
            ("**", "", true),
        ] {
            assert_eq!(glob(p, id), ok, "{p} {id}");
        }
        assert!(glob_ok("motor-*") && glob_ok("p?_2"));
        assert!(!glob_ok("") && !glob_ok("Motor") && !glob_ok("a/b") && !glob_ok("[ab]"));
    }

    #[test]
    fn logins_are_githubs() {
        for ok in ["alice", "a", "dave-2", "A-b-C", &"x".repeat(39)] {
            assert!(login_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "-a",
            "a-",
            "a--b",
            "a_b",
            "a.b",
            &"x".repeat(40),
            "@org/team",
        ] {
            assert!(!login_ok(bad), "{bad}");
        }
    }

    #[test]
    fn a_rule_reads_and_writes_itself() {
        let v =
            json!({"count": 2, "from": ["dave", "erin", "frank"], "parts": ["motor-*", "gearbox"]});
        let r = Rule::from_json(&v, "/approvals/rules/0").unwrap();
        assert_eq!(r.count, 2);
        assert_eq!(r.to_json(), v);
        assert_eq!(r.text(), "2 of dave, erin, frank for motor-*, gearbox");
        assert!(r.applies(&part("motor-1")) && !r.applies(&part("frame")));
        assert!(r.names("Dave") && !r.names("alice"));
    }

    #[test]
    fn rule_faults_point_at_their_field() {
        let at = "/approvals/rules/1";
        let cases = [
            (
                "/approvals/rules/1/parts",
                json!({"count": 1, "from": ["a"], "parts": []}),
            ),
            (
                "/approvals/rules/1/parts/0",
                json!({"count": 1, "from": ["a"], "parts": ["A"]}),
            ),
            (
                "/approvals/rules/1/from",
                json!({"count": 1, "from": "a", "parts": ["*"]}),
            ),
            (
                "/approvals/rules/1/from/1",
                json!({"count": 1, "from": ["a", "-b"], "parts": ["*"]}),
            ),
            (
                "/approvals/rules/1/from/1",
                json!({"count": 1, "from": ["al", "AL"], "parts": ["*"]}),
            ),
            (
                "/approvals/rules/1/count",
                json!({"count": 0, "from": ["a"], "parts": ["*"]}),
            ),
            (
                "/approvals/rules/1/count",
                json!({"count": 2, "from": ["a"], "parts": ["*"]}),
            ),
            (
                "/approvals/rules/1/count",
                json!({"count": 1.5, "from": ["a", "b"], "parts": ["*"]}),
            ),
            (
                "/approvals/rules/1/count",
                json!({"from": ["a"], "parts": ["*"]}),
            ),
            (
                "/approvals/rules/1/extra",
                json!({"count": 1, "extra": 1, "from": ["a"], "parts": ["*"]}),
            ),
            ("/approvals/rules/1", json!([1])),
        ];
        for (pointer, v) in cases {
            let e = Rule::from_json(&v, at).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (Code::ApprovalRuleInvalid, Some(pointer)),
                "{v}"
            );
        }
        let team = json!({"count": 1, "from": ["@acme/reviewers"], "parts": ["*"]});
        let e = Rule::from_json(&team, at).unwrap_err();
        assert_eq!(e.details["team"], "@acme/reviewers");
        assert!(
            e.message.contains("teams come in a later ryupdm"),
            "{}",
            e.message
        );
    }

    fn rules() -> Vec<Rule> {
        vec![
            Rule {
                parts: vec!["*".into()],
                count: 1,
                from: vec!["alice".into(), "bob".into(), "carol".into()],
            },
            Rule {
                parts: vec!["motor-*".into()],
                count: 2,
                from: vec!["dave".into(), "erin".into(), "alice".into()],
            },
        ]
    }

    fn approve(login: &str) -> Review {
        Review {
            login: login.into(),
            state: ReviewState::Approve,
        }
    }

    #[test]
    fn approvals_read_rules_and_keys() {
        let public = ed25519_dalek::SigningKey::from_bytes(&[9; 32])
            .verifying_key()
            .to_bytes();
        let id = crate::attest::key_id(&public);
        let v = json!({
            "keys": {id.clone(): crate::attest::hex(&public)},
            "rules": [{"count": 1, "from": ["alice"], "parts": ["*"]}]
        });
        let a = Approvals::from_json(&v, "/approvals").unwrap();
        assert_eq!(a.keys[&id], public);
        assert_eq!(a.to_json(), v);
        assert_eq!(a.applicable(&part("x")).len(), 1);
        // Without keys, none are written.
        let bare = json!({"rules": [{"count": 1, "from": ["alice"], "parts": ["*"]}]});
        assert_eq!(
            Approvals::from_json(&bare, "/approvals").unwrap().to_json(),
            bare
        );
        let faults = [
            ("/approvals/rules", json!({"rules": []})),
            ("/approvals/rules", json!({})),
            (
                "/approvals/rules/0/count",
                json!({"rules": [{"count": 3, "from": ["a"], "parts": ["*"]}]}),
            ),
            (
                "/approvals/keys",
                json!({"keys": [], "rules": bare["rules"]}),
            ),
            (
                &*format!("/approvals/keys/{id}"),
                json!({"keys": {id.clone(): "00"}, "rules": bare["rules"]}),
            ),
            (
                "/approvals/keys/0123456789abcdef",
                json!({"keys": {"0123456789abcdef": crate::attest::hex(&public)}, "rules": bare["rules"]}),
            ),
            ("/approvals/x", json!({"rules": bare["rules"], "x": 1})),
            ("/approvals", json!("all")),
        ];
        for (pointer, v) in faults {
            let e = Approvals::from_json(&v, "/approvals").unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (Code::ApprovalRuleInvalid, Some(pointer)),
                "{v}"
            );
        }
    }

    #[test]
    fn judging_counts_named_approvers_but_never_the_proposer() {
        let r = rules();
        // A part matching only the first rule: one of alice, bob, carol.
        let one = &r[..1];
        assert!(!judge(one, &[], "dstonely").met);
        assert!(judge(one, &[approve("Bob")], "dstonely").met);
        // Someone the rule doesn't name doesn't count.
        let j = judge(one, &[approve("mallory")], "dstonely");
        assert!(!j.met && j.counted.is_empty());
        assert_eq!(j.missing[0].needed, 1);
        // The proposer never counts, whatever the case.
        assert!(!judge(one, &[approve("alice")], "ALICE").met);
        // One approval counts towards every rule naming it.
        let j = judge(&r, &[approve("alice"), approve("dave")], "x");
        assert!(j.met, "{j:?}");
        let j = judge(&r, &[approve("alice")], "x");
        assert_eq!(j.missing.len(), 1);
        assert_eq!((j.missing[0].needed, &j.missing[0].rule), (1, &r[1]));
        assert_eq!(j.missing[0].to_json()["needed"], 1);
    }

    #[test]
    fn a_change_request_blocks_until_the_reviewer_approves() {
        let r = rules();
        let no = Review {
            login: "carol".into(),
            state: ReviewState::RequestChanges,
        };
        let j = judge(&r[..1], &[approve("alice"), no.clone()], "x");
        assert!(!j.met);
        assert_eq!(j.blockers, ["carol"]);
        // Her later approval stands.
        let j = judge(&r[..1], &[no, approve("Carol")], "x");
        assert!(j.met && j.blockers.is_empty());
        // A change request from someone no rule names blocks nothing.
        let other = Review {
            login: "mallory".into(),
            state: ReviewState::RequestChanges,
        };
        assert!(judge(&r[..1], &[approve("bob"), other], "x").met);
    }
}
