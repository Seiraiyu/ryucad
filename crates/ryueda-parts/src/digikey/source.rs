//! The digikey source: an OAuth token by client credentials, kept until a
//! minute before it expires; keyword search (one page of 50) judged by the
//! query here; product details by DigiKey part number. Every API call is
//! counted against DigiKey's limits first. A 401 asks for a new token
//! once; refused keys are `parts_source_refused` (`reason: keys`), a 403
//! `reason: forbidden`, a 429 `parts_rate_limited`, anything else that
//! isn't 2xx `parts_source_unreachable` with its status. No message
//! carries a key, a token or a body.

use super::{record::record, wire};
use crate::category::Category;
use crate::distributor::{Live, Opened};
use crate::error::{Code, Error, Result};
use crate::fetch::{Request, Response, unreachable};
use crate::query::{Hit, Op, Query, rank};
use crate::record::{Record, Terms, squash};
use crate::source::{Call, Source};
use std::sync::Mutex;

/// One keyword page: DigiKey's most.
pub const PAGE: usize = 50;

pub struct DigiKey {
    o: Opened,
    /// The token and when it is no longer to be used (UTC seconds).
    token: Mutex<Option<(String, u64)>>,
}

/// `pairs` as `application/x-www-form-urlencoded`.
pub fn form(pairs: &[(&str, &str)]) -> String {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                b' ' => "+".to_owned(),
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// A path segment, percent-encoded.
fn segment(s: &str) -> String {
    form(&[("", s)])[1..].replace('+', "%20")
}

fn refused(reason: &str, message: &str) -> Error {
    Error::new(Code::PartsSourceRefused, message)
        .with("source", super::NAME)
        .with("reason", reason)
}

/// The words a keyword search sends: the category's, then each `=` and
/// `~` condition's value as asked. `>=` and `<=` are judged here only.
pub fn keywords(q: &Query) -> String {
    let mut words: Vec<String> = Vec::new();
    match q.category {
        Some(Category::Ic | Category::Other) | None => {}
        Some(Category::FerriteBead) => words.push("ferrite bead".into()),
        Some(c) => words.push(c.as_str().to_owned()),
    }
    for c in &q.conditions {
        if matches!(c.op, Op::Eq | Op::Contains) {
            words.push(c.text.trim().to_owned());
        }
    }
    words.join(" ")
}

impl DigiKey {
    pub fn new(o: Opened) -> DigiKey {
        DigiKey {
            o,
            token: Mutex::new(None),
        }
    }

    fn now(&self) -> u64 {
        (self.o.clock)()
    }

    /// A token, asked for when none is kept or it is within a minute of
    /// expiring.
    fn token(&self, call: &Call) -> Result<String> {
        let now = self.now();
        if let Some((t, until)) = &*self.token.lock().unwrap_or_else(|p| p.into_inner())
            && now < *until
        {
            return Ok(t.clone());
        }
        let url = format!("{}/v1/oauth2/token", self.o.base);
        let body = form(&[
            ("client_id", self.o.keys.get("client_id")),
            ("client_secret", self.o.keys.get("client_secret")),
            ("grant_type", "client_credentials"),
        ]);
        let r = self.o.fetch.send(
            &Request::post(&url, body.into_bytes())
                .header("Content-Type", "application/x-www-form-urlencoded")
                .header("Accept", "application/json"),
            call,
        )?;
        match r.status {
            200..=299 => {}
            400 | 401 => {
                return Err(refused(
                    "keys",
                    &format!("DigiKey refused the keys; {}", super::KEYS.how),
                ));
            }
            429 => return Err(self.limited(&r)),
            s => return Err(unreachable(&url, &format!("answered {s}")).with("status", s)),
        }
        let t: wire::Token = wire::read(&r.body, "token")?;
        if t.access_token.is_empty() {
            return Err(refused("keys", "DigiKey gave no token"));
        }
        let until = now + t.expires_in.saturating_sub(60);
        *self.token.lock().unwrap_or_else(|p| p.into_inner()) =
            Some((t.access_token.clone(), until));
        Ok(t.access_token)
    }

    fn limited(&self, r: &Response) -> Error {
        let after = r.header("Retry-After").and_then(|v| v.trim().parse().ok());
        self.o.limiter.refused(super::NAME, after, self.now())
    }

    fn authorized(&self, req: Request, token: &str) -> Request {
        req.header("Authorization", &format!("Bearer {token}"))
            .header("X-DIGIKEY-Client-Id", self.o.keys.get("client_id"))
            .header("X-DIGIKEY-Locale-Site", "US")
            .header("X-DIGIKEY-Locale-Language", "en")
            .header("X-DIGIKEY-Locale-Currency", "USD")
            .header("Accept", "application/json")
    }

    /// One API call, counted, its token asked for again once on a 401.
    /// `Ok(None)` for a 404.
    fn api(&self, req: &Request, call: &Call) -> Result<Option<Vec<u8>>> {
        for first in [true, false] {
            call.go_on()?;
            self.o
                .limiter
                .take(super::NAME, super::LIMITS, self.now())?;
            let token = self.token(call)?;
            let r = self
                .o
                .fetch
                .send(&self.authorized(req.clone(), &token), call)?;
            match r.status {
                200..=299 => return Ok(Some(r.body)),
                404 => return Ok(None),
                401 if first => {
                    *self.token.lock().unwrap_or_else(|p| p.into_inner()) = None;
                }
                401 => {
                    return Err(refused(
                        "keys",
                        &format!("DigiKey refused the keys; {}", super::KEYS.how),
                    ));
                }
                403 => {
                    return Err(refused(
                        "forbidden",
                        "DigiKey refused the call: is the app subscribed to Product Information v4?",
                    ));
                }
                429 => return Err(self.limited(&r)),
                s => {
                    return Err(unreachable(&req.url, &format!("answered {s}")).with("status", s));
                }
            }
        }
        unreachable!("the second try answers")
    }

    /// The products a keyword search gives: its exact matches, then the rest.
    pub fn keyword(&self, words: &str, call: &Call) -> Result<Vec<wire::Product>> {
        let body = serde_json::json!({"Keywords": words, "Limit": PAGE, "Offset": 0});
        let req = Request::post(
            &format!("{}/products/v4/search/keyword", self.o.base),
            body.to_string().into_bytes(),
        )
        .header("Content-Type", "application/json");
        let Some(bytes) = self.api(&req, call)? else {
            return Ok(Vec::new());
        };
        let k: wire::Keyword = wire::read(&bytes, "keyword")?;
        Ok(k.exact_matches.into_iter().chain(k.products).collect())
    }

    fn records(&self, products: &[wire::Product]) -> Vec<Record> {
        let now = self.now();
        let mut out: Vec<Record> = products
            .iter()
            .filter_map(|p| record(p, None, now))
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out.dedup_by(|a, b| a.key == b.key);
        out
    }
}

impl Source for DigiKey {
    fn name(&self) -> &str {
        super::NAME
    }

    fn terms(&self) -> Terms {
        super::DISTRIBUTOR.terms()
    }

    fn search(&self, q: &Query, call: &Call) -> Result<Vec<Hit>> {
        let words = keywords(q);
        if words.is_empty() {
            return Err(Error::new(
                Code::InvalidParam,
                "a DigiKey search needs a category other than ic or other, or a condition with = or ~, to send as keywords",
            )
            .with("source", super::NAME));
        }
        let mut hits: Vec<Hit> = self
            .records(&self.keyword(&words, call)?)
            .iter()
            .filter_map(|r| q.judge(r))
            .collect();
        rank(&mut hits);
        hits.truncate(q.limit);
        Ok(hits)
    }

    fn get(&self, key: &str, call: &Call) -> Result<Option<Record>> {
        let req = Request::get(&format!(
            "{}/products/v4/search/{}/productdetails",
            self.o.base,
            segment(key.trim())
        ));
        let Some(bytes) = self.api(&req, call)? else {
            return Ok(None);
        };
        let d: wire::Details = wire::read(&bytes, "details")?;
        Ok(record(&d.product, Some(key), self.now()))
    }
}

impl Live for DigiKey {
    fn by_mpn(&self, mpn: &str, call: &Call) -> Result<Vec<Record>> {
        let want = squash(mpn);
        if want.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .records(&self.keyword(mpn.trim(), call)?)
            .into_iter()
            .filter(|r| squash(&r.mpn) == want)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digikey::testing::{self, BASE};
    use crate::fetch::{Fake, Method};
    use crate::query::Condition;
    use crate::rate::Limiter;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn leaked() -> &'static Limiter {
        Box::leak(Box::new(Limiter::new()))
    }

    fn dk(fake: Arc<Fake>, clock: Arc<AtomicU64>) -> DigiKey {
        DigiKey::new(testing::opened(
            fake,
            leaked(),
            Arc::new(move || clock.load(Ordering::Relaxed)),
        ))
    }

    fn q(cat: Option<Category>, conds: &[&str]) -> Query {
        Query {
            category: cat,
            conditions: conds.iter().map(|c| Condition::parse(c).unwrap()).collect(),
            stock: None,
            basic: false,
            limit: 20,
        }
    }

    #[test]
    fn a_search_sends_keywords_and_judges_what_comes_back() {
        let fake = Arc::new(testing::fake());
        let d = dk(fake.clone(), Arc::new(AtomicU64::new(1_000)));
        let found = d
            .search(
                &q(
                    Some(Category::Capacitor),
                    &["capacitance=100nF", "voltage_rating>=10V", "package=0402"],
                ),
                &Call::default(),
            )
            .unwrap();
        assert_eq!(
            found
                .iter()
                .map(|h| h.record.key.as_str())
                .collect::<Vec<_>>(),
            ["1276-1001-1-ND"]
        );
        assert_eq!(found[0].meets, 3);
        let sent = fake.sent();
        assert_eq!(
            (sent[0].method, sent[0].url.as_str()),
            (Method::Post, "https://dk.test/v1/oauth2/token")
        );
        assert_eq!(
            String::from_utf8_lossy(&sent[0].body),
            "client_id=test-id&client_secret=te%2Bst%26secret&grant_type=client_credentials"
        );
        let search = &sent[1];
        assert_eq!(search.url, "https://dk.test/products/v4/search/keyword");
        assert_eq!(search.header_value("authorization"), Some("Bearer tok-1"));
        assert_eq!(search.header_value("x-digikey-client-id"), Some("test-id"));
        let body: serde_json::Value = serde_json::from_slice(&search.body).unwrap();
        assert_eq!(body["Keywords"], "capacitor 100nF 0402");
        // The token is kept: a second search asks for none.
        d.search(&q(Some(Category::Capacitor), &[]), &Call::default())
            .unwrap();
        assert_eq!(
            fake.sent()
                .iter()
                .filter(|r| r.url.ends_with("/token"))
                .count(),
            1
        );
        assert_eq!(
            d.search(
                &q(Some(Category::Ic), &["voltage_rating>=1V"]),
                &Call::default()
            )
            .unwrap_err()
            .code,
            Code::InvalidParam
        );
    }

    #[test]
    fn details_and_part_numbers_are_found() {
        let fake = Arc::new(testing::fake());
        let d = dk(fake, Arc::new(AtomicU64::new(1_000)));
        let r = d.get("296-1234-1-ND", &Call::default()).unwrap().unwrap();
        assert_eq!(
            (r.mpn.as_str(), r.lifecycle),
            (
                "TPS7A2033PDBVR",
                Some(crate::record::Lifecycle::NotRecommended)
            )
        );
        assert!(d.get("nothing-ND", &Call::default()).unwrap().is_none());
        let by = d.by_mpn("cl05b104ko5nnnc", &Call::default()).unwrap();
        assert_eq!(
            by.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            ["1276-1001-1-ND"]
        );
        assert!(d.by_mpn(" ", &Call::default()).unwrap().is_empty());
    }

    #[test]
    fn tokens_expire_and_a_401_asks_again_once() {
        let fake = Arc::new(Fake::default());
        for n in 1..=3 {
            fake.answer(
                Method::Post,
                &testing::token_url(),
                testing::token_answer(n),
            );
        }
        let url = testing::details_url("296-1234-1-ND");
        // Answered, answered, refused once, answered.
        for r in [
            testing::details_answer(testing::LDO),
            testing::details_answer(testing::LDO),
            crate::fetch::Response::new(401, ""),
            testing::details_answer(testing::LDO),
        ] {
            fake.answer(Method::Get, &url, r);
        }
        let clock = Arc::new(AtomicU64::new(1_000));
        let d = dk(fake.clone(), clock.clone());
        let tokens = |f: &Fake| {
            f.sent()
                .iter()
                .filter(|r| r.url.ends_with("/token"))
                .count()
        };
        d.get("296-1234-1-ND", &Call::default()).unwrap();
        clock.store(1_000 + 599 - 59, Ordering::Relaxed);
        d.get("296-1234-1-ND", &Call::default()).unwrap();
        assert_eq!(
            tokens(&fake),
            2,
            "a token within a minute of expiring isn't used"
        );
        d.get("296-1234-1-ND", &Call::default()).unwrap();
        assert_eq!(tokens(&fake), 3, "a 401 asks for a new token once");
        let last = fake.sent().pop().unwrap();
        assert_eq!(last.header_value("authorization"), Some("Bearer tok-3"));
    }

    #[test]
    fn refusals_say_why_and_never_what_the_keys_are() {
        let call = Call::default();
        let fake = Arc::new(Fake::default());
        fake.answer(
            Method::Post,
            &format!("{BASE}/v1/oauth2/token"),
            crate::fetch::Response::new(401, "{}"),
        );
        let e = dk(fake, Arc::new(AtomicU64::new(1)))
            .get("x", &call)
            .unwrap_err();
        assert_eq!(
            (e.code, e.details["reason"].as_str()),
            (Code::PartsSourceRefused, Some("keys"))
        );
        let text = format!("{e} {}", e.details);
        assert!(
            !text.contains("test-id") && !text.contains("te+st"),
            "{text}"
        );

        let fake = Arc::new(testing::fake());
        fake.set(
            Method::Get,
            &testing::details_url("x"),
            crate::fetch::Response::new(403, ""),
        );
        let e = dk(fake.clone(), Arc::new(AtomicU64::new(1)))
            .get("x", &call)
            .unwrap_err();
        assert_eq!(e.details["reason"], "forbidden");

        fake.set(
            Method::Get,
            &testing::details_url("y"),
            crate::fetch::Response::new(429, "").with_header("Retry-After", "30"),
        );
        let d = dk(fake.clone(), Arc::new(AtomicU64::new(1)));
        let e = d.get("y", &call).unwrap_err();
        assert_eq!(
            (e.code, e.details["retry_after_s"].as_u64()),
            (Code::PartsRateLimited, Some(30))
        );
        // Held: no call goes out until then.
        let before = fake.sent().len();
        assert_eq!(
            d.get("296-1234-1-ND", &call).unwrap_err().code,
            Code::PartsRateLimited
        );
        assert_eq!(fake.sent().len(), before);

        fake.set(
            Method::Get,
            &testing::details_url("z"),
            crate::fetch::Response::new(502, ""),
        );
        let e = dk(fake.clone(), Arc::new(AtomicU64::new(1)))
            .get("z", &call)
            .unwrap_err();
        assert_eq!(
            (e.code, &e.details["status"]),
            (Code::PartsSourceUnreachable, &serde_json::json!(502))
        );

        fake.set(
            Method::Get,
            &testing::details_url("w"),
            crate::fetch::Response::new(200, "{\"Product\": 7}"),
        );
        let e = dk(fake, Arc::new(AtomicU64::new(1)))
            .get("w", &call)
            .unwrap_err();
        assert_eq!(
            (e.code, e.details["answer"].as_str()),
            (Code::PartsSourceRefused, Some("details"))
        );
    }

    #[test]
    fn calls_are_counted_against_digikeys_limits() {
        let fake = Arc::new(testing::fake());
        let d = dk(fake, Arc::new(AtomicU64::new(1_000)));
        for _ in 0..crate::digikey::LIMITS.per_minute {
            d.get("296-1234-1-ND", &Call::default()).unwrap();
        }
        let e = d.get("296-1234-1-ND", &Call::default()).unwrap_err();
        assert_eq!(
            (e.code, e.details["limit"].as_str()),
            (Code::PartsRateLimited, Some("minute"))
        );
        let stop = std::sync::atomic::AtomicBool::new(true);
        let call = Call {
            cancel: Some(&stop),
            deadline: None,
        };
        assert_eq!(d.get("x", &call).unwrap_err().code, Code::Cancelled);
    }

    #[test]
    fn forms_and_keywords_are_written_plainly() {
        assert_eq!(form(&[("a b", "x&y=z/é")]), "a+b=x%26y%3Dz%2F%C3%A9");
        assert_eq!(segment("311-1338-1-ND"), "311-1338-1-ND");
        assert_eq!(segment("a/b c"), "a%2Fb%20c");
        assert_eq!(
            keywords(&q(Some(Category::FerriteBead), &["package~0603"])),
            "ferrite bead 0603"
        );
        assert_eq!(keywords(&q(None, &[])), "");
    }
}
