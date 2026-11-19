//! The ledger (design 936): the organization's part numbers, held by the
//! hosted service, asked by the CLI process through `ryu-review`'s client
//! and never by the daemon, whose answers stay deterministic. A catalogue
//! whose `ryupdm.json` has `part_number.ledger` on takes `--next`'s and
//! imports' numbers from it. When it can't be asked they come from the
//! local rule with a `ledger_unavailable` warning, and the hosted
//! `ryuplm / check` catches a clash before it merges.

use crate::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::path::Path;

/// Overrides the `review.url` preference.
pub const URL_VAR: &str = "RYUPLM_REVIEW_URL";
/// The GitHub token to send, before `GH_TOKEN`, `GITHUB_TOKEN` and `gh
/// auth token`.
pub const TOKEN_VAR: &str = "RYUPLM_REVIEW_TOKEN";

/// ryuplm as the service's client names it.
pub const PLM: ryu_review::Product = ryu_review::Product {
    name: "ryuplm",
    version: env!("CARGO_PKG_VERSION"),
    url_var: URL_VAR,
    token_var: TOKEN_VAR,
    url_pref,
};

/// The `review.url` preference, when the preferences set it.
fn url_pref() -> Option<String> {
    ryu_session::prefs::Preferences::<crate::Plm>::load()
        .ok()?
        .values
        .get(ryu_review::URL_PREF)?
        .as_str()
        .map(str::to_owned)
}

/// The ledger of the organization owning a catalogue's repository.
pub struct Ledger {
    client: ryu_review::Client,
    slug: String,
}

impl Ledger {
    /// The ledger for the clone holding `root`, whose `origin` names the
    /// repository. No Git repository, no GitHub remote and no token are
    /// `ledger_unavailable`.
    pub fn open(root: &Path) -> Result<Ledger> {
        let s = ryu_review::Session::<crate::Plm>::open(root, &PLM).map_err(unavailable)?;
        Ok(Ledger {
            slug: s.slug,
            client: s.client,
        })
    }

    /// The ledger of `slug` (`acme/widgets`) at `base` with `token`: a
    /// test's stand-in.
    pub fn at(base: &str, token: &str, slug: &str) -> Ledger {
        Ledger {
            client: ryu_review::Client::new(base, token),
            slug: slug.to_owned(),
        }
    }

    /// `owner/name`.
    pub fn slug(&self) -> &str {
        &self.slug
    }

    fn path(&self, rest: &str) -> String {
        format!("/repos/{}/numbers{rest}", self.slug)
    }

    fn ask(r: Result<Value>) -> Result<Value> {
        let v = r.map_err(unavailable)?;
        if v["schema"] != json!(1) {
            return Err(Error::new(
                Code::ReviewFailed,
                format!(
                    "the service answered the ledger in schema {}, which this build doesn't read",
                    v["schema"]
                ),
            )
            .with("schema", &v["schema"]));
        }
        Ok(v)
    }

    /// `count` numbers from `template`'s sequence, above `floor` (the
    /// highest sequence the catalogue holds locally), each reserved to
    /// `catalogue` for `days` (30 when `None`).
    pub fn next(
        &self,
        catalogue: &str,
        template: &str,
        count: u64,
        floor: u64,
        days: Option<u64>,
    ) -> Result<Value> {
        let mut b = json!({"catalogue": catalogue, "template": template, "count": count, "floor": floor});
        if let Some(d) = days {
            b["days"] = json!(d);
        }
        Ledger::ask(self.client.post(&self.path("/next"), &b))
    }

    /// `numbers` typed by hand, reserved to `catalogue`, all or none:
    /// `part_number_taken` naming each holder when any is held for
    /// something else.
    pub fn reserve(&self, catalogue: &str, numbers: &[String], days: Option<u64>) -> Result<Value> {
        let mut b = json!({"catalogue": catalogue, "numbers": numbers});
        if let Some(d) = days {
            b["days"] = json!(d);
        }
        Ledger::ask(self.client.post(&self.path("/reserve"), &b))
    }

    /// Who holds `number`: its bindings, then its live reservation.
    pub fn who(&self, number: &str) -> Result<Value> {
        if number == "next" || number == "reserve" {
            return Err(Error::new(
                Code::InvalidParam,
                format!("the service can't be asked who holds {number:?}: number list shows a reservation of it"),
            )
            .with("number", number));
        }
        Ledger::ask(
            self.client
                .get(&self.path(&format!("/{}", ryu_review::escape(number)))),
        )
    }

    /// The live reservations: of `catalogue`, or the organization's; only
    /// the caller's when `mine`; the page after `after`.
    pub fn list(&self, catalogue: Option<&str>, mine: bool, after: Option<&str>) -> Result<Value> {
        let mut q = Vec::new();
        if let Some(c) = catalogue {
            q.push(format!("catalogue={}", ryu_review::escape(c)));
        }
        if mine {
            q.push("mine=true".to_owned());
        }
        if let Some(a) = after {
            q.push(format!("after={}", ryu_review::escape(a)));
        }
        let rest = if q.is_empty() {
            String::new()
        } else {
            format!("?{}", q.join("&"))
        };
        Ledger::ask(self.client.get(&self.path(&rest)))
    }
}

/// `e` as the ledger says it: an error meaning the ledger can't be asked
/// becomes `ledger_unavailable`, with `details.why` its code and
/// `details.cause` the error; any other is returned as it is.
pub fn unavailable(e: Error) -> Error {
    let offline = match e.code {
        Code::ReviewUnreachable
        | Code::ReviewNotInBeta
        | Code::ReviewNotInstalled
        | Code::ReviewGithubUnreachable
        | Code::ReviewNoRemote
        | Code::NotAGitRepo => true,
        // No token found at all, rather than one the service refused.
        Code::ReviewUnauthenticated => e.details.get("status").is_none(),
        _ => false,
    };
    if !offline {
        return e;
    }
    let why = serde_json::to_value(e.code).unwrap_or(Value::Null);
    Error::new(
        Code::LedgerUnavailable,
        format!("the ledger can't be asked: {}", e.message),
    )
    .with("why", &why)
    .with("cause", &e)
}

/// `ledger_unavailable` for a catalogue whose ledger is off.
pub fn off() -> Error {
    Error::new(
        Code::LedgerUnavailable,
        "this catalogue's ledger is off: `ryuplm settings edit --ledger on` turns it on",
    )
    .with("why", "ledger_off")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_review::testing::Ledger as Fake;
    use serde_json::json;

    pub(crate) const CAT: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn the_four_routes() {
        let (fake, state) = Fake::start("acme/widgets");
        let l = Ledger::at(&fake.url, "t", "acme/widgets");
        let v = l.next(CAT, "MECH-{seq:5}", 2, 41, None).unwrap();
        assert_eq!(v["numbers"], json!(["MECH-00042", "MECH-00043"]));
        let v = l.reserve(CAT, &["100-0042 rev".into()], Some(7)).unwrap();
        assert_eq!(v["numbers"], json!(["100-0042 rev"]));
        assert_eq!(l.who("100-0042 rev").unwrap()["holders"][0]["catalogue"], CAT);
        let listed = l.list(Some(CAT), false, None).unwrap();
        assert_eq!(listed["reservations"].as_array().unwrap().len(), 3);
        // The service can't be asked about a number spelled as a route.
        assert_eq!(l.who("next").unwrap_err().code, Code::InvalidParam);
        state.lock().unwrap().bound.insert(
            "B-1".into(),
            ("fedcba9876543210fedcba9876543210".into(), "k3v9q2m8x4c1p".into()),
        );
        let e = l.reserve(CAT, &["B-1".into()], None).unwrap_err();
        assert_eq!(e.code, Code::PartNumberTaken);
        assert_eq!(e.details["service"]["taken"][0]["holders"][0]["item"], "k3v9q2m8x4c1p");
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].auth.as_deref(), Some("Bearer t"));
        assert!(seen[0].body.contains("\"floor\":41"), "{}", seen[0].body);
        assert!(seen[1].body.contains("\"days\":7"), "{}", seen[1].body);
    }

    #[test]
    fn what_means_the_ledger_cant_be_asked() {
        // Nothing listens on port 9 of this machine.
        let down = Ledger::at("http://127.0.0.1:9", "t", "acme/widgets");
        let e = down.next(CAT, "M-{seq:3}", 1, 0, None).unwrap_err();
        assert_eq!(
            (e.code, e.details["why"].as_str()),
            (Code::LedgerUnavailable, Some("review_unreachable"))
        );
        let (fake, state) = Fake::start("acme/widgets");
        let l = Ledger::at(&fake.url, "t", "acme/widgets");
        for (status, code, ours) in [
            (403, "not_in_beta", Code::LedgerUnavailable),
            (404, "not_installed", Code::LedgerUnavailable),
            (502, "github_unreachable", Code::LedgerUnavailable),
            (403, "forbidden", Code::ReviewForbidden),
            (401, "unauthenticated", Code::ReviewUnauthenticated),
            (400, "invalid_request", Code::ReviewInvalidRequest),
        ] {
            state.lock().unwrap().refuse = Some((status, code.into()));
            assert_eq!(l.next(CAT, "M-{seq:3}", 1, 0, None).unwrap_err().code, ours, "{code}");
        }
        // No token at all is offline too; a catalogue whose ledger is off.
        let none = ryu_review::token_from(TOKEN_VAR, |_| None, || None).unwrap_err();
        assert_eq!(unavailable(none).details["why"], "review_unauthenticated");
        assert_eq!(off().details["why"], "ledger_off");
        assert_eq!(off().code, Code::LedgerUnavailable);
    }
}
