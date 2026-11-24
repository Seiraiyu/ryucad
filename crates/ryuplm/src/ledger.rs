//! The ledger (design 936): the organization's part numbers, held by the
//! hosted service, asked by the CLI process through `ryu-review`'s client
//! and never by the daemon, whose answers stay deterministic. A catalogue
//! whose `ryupdm.json` has `part_number.ledger` on takes `--next`'s and
//! imports' numbers from it. When it can't be asked they come from the
//! local rule with a `ledger_unavailable` warning, and the hosted
//! `ryuplm / check` catches a clash before it merges.

use crate::error::{Code, Error, Result};
use crate::proto::Request;
use serde_json::{Value, json};
use std::collections::BTreeMap;
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
        let mut b =
            json!({"catalogue": catalogue, "template": template, "count": count, "floor": floor});
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

/// What sends a request to the catalogue's daemon, or answers it in
/// process: the CLI's own path for every request.
pub type Sender<'a> = dyn FnMut(Request) -> Result<Value> + 'a;

/// The warning a fallback adds to the CLI's `warnings`.
pub fn warning(e: &Error) -> String {
    format!("ledger_unavailable: {}; numbered locally", e.message)
}

/// `item.create` or `item.renumber` with `--next` and no number: on a
/// catalogue whose ledger is on, one number from the ledger (above the
/// local floor) sent as the request's number, the answer's `ledger`
/// saying so. When the ledger can't be asked, the request goes as it is,
/// the daemon takes the local next number, and `warnings` says why. Any
/// other request, and any on a catalogue whose ledger is off, is sent
/// unchanged, with no call to the service and no token looked for.
pub fn numbered(
    send: &mut Sender,
    open: &dyn Fn() -> Result<Ledger>,
    req: Request,
    warnings: &mut Vec<String>,
) -> Result<Value> {
    let (next, class, item) = match &req {
        Request::ItemCreate {
            next: Some(n),
            number: None,
            fields,
            ..
        } => (n.clone(), fields.class.clone(), None),
        Request::ItemRenumber {
            next: Some(n),
            number: None,
            item,
            ..
        } => (n.clone(), None, Some(item.clone())),
        _ => return send(req),
    };
    let local = send(Request::NumberLocal {
        next: Some(next),
        class,
        item,
    })?;
    if local["ledger"] != json!(true) {
        return send(req);
    }
    let asked = open().and_then(|l| {
        l.next(
            local["catalogue"].as_str().unwrap_or_default(),
            local["template"].as_str().unwrap_or_default(),
            1,
            local["floor"].as_u64().unwrap_or(0),
            None,
        )
    });
    match asked {
        Ok(v) => {
            let n = v["numbers"][0]
                .as_str()
                .ok_or_else(|| Error::new(Code::ReviewFailed, "the ledger answered no number"))?
                .to_owned();
            let req = match req {
                Request::ItemCreate {
                    fields,
                    expected_revision,
                    ..
                } => Request::ItemCreate {
                    fields,
                    number: Some(n.clone()),
                    next: None,
                    expected_revision,
                },
                Request::ItemRenumber {
                    item,
                    expected_revision,
                    ..
                } => Request::ItemRenumber {
                    item,
                    number: Some(n.clone()),
                    next: None,
                    expected_revision,
                },
                other => other,
            };
            let mut out = send(req)?;
            out["ledger"] = json!({"reserved": true, "number": n, "expires_at": v["expires_at"]});
            Ok(out)
        }
        Err(e) if e.code == Code::LedgerUnavailable => {
            warnings.push(warning(&e));
            let mut out = send(req)?;
            out["ledger"] =
                json!({"reserved": false, "code": "ledger_unavailable", "why": e.details["why"]});
            Ok(out)
        }
        Err(e) => Err(e),
    }
}

/// `import.run` (not a dry run): on a catalogue whose ledger is on, a dry
/// run at the catalogue's revision says how many numbers each scheme
/// needs (`would_reserve`), the ledger hands them out in one `next` call
/// per scheme, and the run takes them at that same revision. A dry run,
/// a request already carrying numbers, and any request on a catalogue
/// whose ledger is off are sent unchanged. When the ledger can't be
/// asked, the run numbers locally and `warnings` says why.
pub fn import_run(
    send: &mut Sender,
    open: &dyn Fn() -> Result<Ledger>,
    req: Request,
    warnings: &mut Vec<String>,
) -> Result<Value> {
    let (name, file, prefer, limit, expected) = match req {
        Request::ImportRun {
            name,
            file,
            dry_run: false,
            prefer,
            limit,
            expected_revision,
            numbers,
        } if numbers.is_empty() => (name, file, prefer, limit, expected_revision),
        other => return send(other),
    };
    let request = |dry_run: bool, numbers: BTreeMap<String, Vec<String>>, at: Option<String>| {
        Request::ImportRun {
            name: name.clone(),
            file: file.clone(),
            dry_run,
            prefer: prefer.clone(),
            limit,
            expected_revision: at,
            numbers,
        }
    };
    let local = send(Request::NumberLocal {
        next: None,
        class: None,
        item: None,
    })?;
    if local["ledger"] != json!(true) {
        return send(request(false, BTreeMap::new(), expected));
    }
    let at = expected.or_else(|| local["revision"].as_str().map(str::to_owned));
    let dry = send(request(true, BTreeMap::new(), at.clone()))?;
    let wanted: Vec<Value> = dry["would_reserve"].as_array().cloned().unwrap_or_default();
    if wanted.is_empty() {
        return send(request(false, BTreeMap::new(), at));
    }
    let catalogue = local["catalogue"].as_str().unwrap_or_default().to_owned();
    let mut given: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut expires = Value::Null;
    let asked = open().and_then(|l| {
        for w in &wanted {
            let v = l.next(
                &catalogue,
                w["template"].as_str().unwrap_or_default(),
                w["count"].as_u64().unwrap_or(0),
                w["floor"].as_u64().unwrap_or(0),
                None,
            )?;
            let numbers: Vec<String> =
                serde_json::from_value(v["numbers"].clone()).map_err(|e| {
                    Error::new(
                        Code::ReviewFailed,
                        format!("the ledger's numbers don't read: {e}"),
                    )
                })?;
            given.insert(w["scheme"].as_str().unwrap_or_default().to_owned(), numbers);
            expires = v["expires_at"].clone();
        }
        Ok(())
    });
    match asked {
        Ok(()) => {
            let (count, requests) = (given.values().map(Vec::len).sum::<usize>(), given.len());
            let mut out = send(request(false, given, at))?;
            out["ledger"] = json!({"reserved": true, "numbers": count, "requests": requests,
                                   "expires_at": expires});
            Ok(out)
        }
        Err(e) if e.code == Code::LedgerUnavailable => {
            warnings.push(warning(&e));
            let mut out = send(request(false, BTreeMap::new(), at))?;
            out["ledger"] =
                json!({"reserved": false, "code": "ledger_unavailable", "why": e.details["why"]});
            Ok(out)
        }
        Err(e) => Err(e),
    }
}

/// What `ryuplm number` asks.
pub enum Ask {
    /// `count` numbers of `scheme` (or of the one that applies, by
    /// `class`), reserved without creating items.
    Next {
        scheme: Option<String>,
        class: Option<String>,
        count: u64,
        days: Option<u64>,
    },
    /// Numbers typed by hand, reserved to this catalogue, all or none.
    Reserve {
        numbers: Vec<String>,
        days: Option<u64>,
    },
    /// Who holds a number in the organization.
    Who { number: String },
    /// The live reservations: this catalogue's, or the organization's
    /// with `all`; only the caller's with `mine`.
    List {
        mine: bool,
        all: bool,
        after: Option<String>,
    },
}

/// `ryuplm number`: the ledger asked directly, for a catalogue whose
/// ledger is on. The answer is the service's, plus `repo`. There is
/// nothing local to fall back to, so a ledger that can't be asked, or a
/// catalogue whose ledger is off, is `ledger_unavailable`.
pub fn number(send: &mut Sender, open: &dyn Fn() -> Result<Ledger>, ask: Ask) -> Result<Value> {
    let (next, class) = match &ask {
        Ask::Next { scheme, class, .. } => {
            (Some(scheme.clone().unwrap_or_default()), class.clone())
        }
        _ => (None, None),
    };
    let local = send(Request::NumberLocal {
        next,
        class,
        item: None,
    })?;
    if local["ledger"] != json!(true) {
        return Err(off());
    }
    let catalogue = local["catalogue"].as_str().unwrap_or_default().to_owned();
    let l = open()?;
    let mut v = match ask {
        Ask::Next { count, days, .. } => l.next(
            &catalogue,
            local["template"].as_str().unwrap_or_default(),
            count,
            local["floor"].as_u64().unwrap_or(0),
            days,
        )?,
        Ask::Reserve { numbers, days } => l.reserve(&catalogue, &numbers, days)?,
        Ask::Who { number } => l.who(&number)?,
        Ask::List { mine, all, after } => {
            l.list((!all).then_some(catalogue.as_str()), mine, after.as_deref())?
        }
    };
    if let Some(o) = v.as_object_mut() {
        o.insert("repo".into(), json!(l.slug()));
    }
    Ok(v)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ryu_review::testing::Ledger as Fake;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    pub(crate) const CAT: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn the_four_routes() {
        let (fake, state) = Fake::start("acme/widgets");
        let l = Ledger::at(&fake.url, "t", "acme/widgets");
        let v = l.next(CAT, "MECH-{seq:5}", 2, 41, None).unwrap();
        assert_eq!(v["numbers"], json!(["MECH-00042", "MECH-00043"]));
        let v = l.reserve(CAT, &["100-0042 rev".into()], Some(7)).unwrap();
        assert_eq!(v["numbers"], json!(["100-0042 rev"]));
        assert_eq!(
            l.who("100-0042 rev").unwrap()["holders"][0]["catalogue"],
            CAT
        );
        let listed = l.list(Some(CAT), false, None).unwrap();
        assert_eq!(listed["reservations"].as_array().unwrap().len(), 3);
        // The service can't be asked about a number spelled as a route.
        assert_eq!(l.who("next").unwrap_err().code, Code::InvalidParam);
        state.lock().unwrap().bound.insert(
            "B-1".into(),
            (
                "fedcba9876543210fedcba9876543210".into(),
                "k3v9q2m8x4c1p".into(),
            ),
        );
        let e = l.reserve(CAT, &["B-1".into()], None).unwrap_err();
        assert_eq!(e.code, Code::PartNumberTaken);
        assert_eq!(
            e.details["service"]["taken"][0]["holders"][0]["item"],
            "k3v9q2m8x4c1p"
        );
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
            assert_eq!(
                l.next(CAT, "M-{seq:3}", 1, 0, None).unwrap_err().code,
                ours,
                "{code}"
            );
        }
        // No token at all is offline too; a catalogue whose ledger is off.
        let none = ryu_review::token_from(TOKEN_VAR, |_| None, || None).unwrap_err();
        assert_eq!(unavailable(none).details["why"], "review_unauthenticated");
        assert_eq!(off().details["why"], "ledger_off");
        assert_eq!(off().code, Code::LedgerUnavailable);
    }

    /// A catalogue with the `mech` scheme, its ledger on or off.
    pub(crate) fn catalogue(ledger: bool) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        crate::server::init(&root, None).unwrap();
        let mut slot = None;
        for change in [
            json!({"scheme": "mech", "pattern": "MECH-\\d{5}", "next_template": "MECH-{seq:5}"}),
            json!({"ledger": ledger}),
        ] {
            crate::server::handle_safely(
                &mut slot,
                None,
                None,
                &root,
                Request::SettingsEdit {
                    change: serde_json::from_value(change).unwrap(),
                    expected_revision: None,
                },
            )
            .unwrap();
        }
        (d, root)
    }

    /// Requests answered in process, as the CLI's `--no-daemon` does.
    pub(crate) fn sender(root: &Path) -> impl FnMut(Request) -> Result<Value> + '_ {
        let mut slot = None;
        move |r| crate::server::handle_safely(&mut slot, None, None, root, r).map(|a| a.result)
    }

    fn create() -> Request {
        Request::ItemCreate {
            fields: Default::default(),
            number: None,
            next: Some(String::new()),
            expected_revision: None,
        }
    }

    #[test]
    fn next_takes_the_ledgers_number_or_falls_back() {
        let (_d, root) = catalogue(true);
        let mut send = sender(&root);
        let (fake, state) = Fake::start("acme/widgets");
        state
            .lock()
            .unwrap()
            .sequences
            .insert("MECH-{seq:5}".into(), 500);
        let url = fake.url.clone();
        let open = move || -> Result<Ledger> { Ok(Ledger::at(&url, "t", "acme/widgets")) };
        let mut w = Vec::new();
        let v = numbered(&mut send, &open, create(), &mut w).unwrap();
        assert_eq!(v["item"]["number"], "MECH-00500");
        assert_eq!(
            v["ledger"],
            json!({"reserved": true, "number": "MECH-00500", "expires_at": ryu_review::testing::EXPIRES})
        );
        // The local floor goes with the next call.
        let v = numbered(&mut send, &open, create(), &mut w).unwrap();
        assert_eq!(v["item"]["number"], "MECH-00501");
        assert!(fake.seen.lock().unwrap()[1].body.contains("\"floor\":500"));
        assert!(w.is_empty(), "{w:?}");
        // Offline: the local next number, a warning, reserved false.
        let down =
            || -> Result<Ledger> { Ok(Ledger::at("http://127.0.0.1:9", "t", "acme/widgets")) };
        let v = numbered(&mut send, &down, create(), &mut w).unwrap();
        assert_eq!(v["item"]["number"], "MECH-00502");
        assert_eq!(
            (&v["ledger"]["reserved"], &v["ledger"]["why"]),
            (&json!(false), &json!("review_unreachable"))
        );
        assert!(w[0].starts_with("ledger_unavailable: "), "{w:?}");
        // Renumbered from the ledger too.
        let v = numbered(
            &mut send,
            &open,
            Request::ItemRenumber {
                item: "MECH-00500".into(),
                number: None,
                next: Some(String::new()),
                expected_revision: None,
            },
            &mut w,
        )
        .unwrap();
        assert_eq!(v["item"]["number"], "MECH-00503");
    }

    #[test]
    fn a_catalogue_without_the_ledger_never_asks() {
        let (_d, root) = catalogue(false);
        let mut send = sender(&root);
        let never = || -> Result<Ledger> { panic!("a catalogue without the ledger opened it") };
        let mut w = Vec::new();
        let v = numbered(&mut send, &never, create(), &mut w).unwrap();
        assert_eq!(v["item"]["number"], "MECH-00001");
        assert!(v.get("ledger").is_none() && w.is_empty());
        // A number and --next together reach the daemon, which refuses them.
        let both = Request::ItemCreate {
            fields: Default::default(),
            number: Some("X".into()),
            next: Some(String::new()),
            expected_revision: None,
        };
        assert_eq!(
            numbered(&mut send, &never, both, &mut w).unwrap_err().code,
            Code::Usage
        );
    }

    fn import(root: &Path, dry_run: bool) -> Request {
        Request::ImportRun {
            name: "erp".into(),
            file: root.join("master.csv").display().to_string(),
            dry_run,
            prefer: None,
            limit: Some(3),
            expected_revision: None,
            numbers: Default::default(),
        }
    }

    pub(crate) fn assigning(root: &Path, rows: usize) {
        std::fs::create_dir_all(root.join("imports")).unwrap();
        std::fs::write(
            root.join("imports/erp.json"),
            crate::canonical::to_string(&json!({"format": 1, "kind": "ryuplm.import",
                "reader": {"type": "csv"}, "key": "ID", "fields": {"name": {"from": "Name"}},
                "numbers": "assign", "scheme": "mech"})),
        )
        .unwrap();
        let mut csv = String::from("ID,Name\n");
        for k in 0..rows {
            csv += &format!("{k},Part {k}\n");
        }
        std::fs::write(root.join("master.csv"), csv).unwrap();
    }

    #[test]
    fn an_import_reserves_its_numbers_in_one_call_per_scheme() {
        let (_d, root) = catalogue(true);
        assigning(&root, 1000);
        let mut send = sender(&root);
        let (fake, _state) = Fake::start("acme/widgets");
        let url = fake.url.clone();
        let open = move || -> Result<Ledger> { Ok(Ledger::at(&url, "t", "acme/widgets")) };
        let mut w = Vec::new();
        // A dry run reserves nothing and says what a run would.
        let dry = import_run(&mut send, &open, import(&root, true), &mut w).unwrap();
        assert_eq!(
            dry["would_reserve"],
            json!([{"scheme": "mech", "template": "MECH-{seq:5}", "count": 1000, "floor": 0}])
        );
        assert!(fake.seen.lock().unwrap().is_empty());
        let v = import_run(&mut send, &open, import(&root, false), &mut w).unwrap();
        assert_eq!(v["created"]["count"], 1000);
        assert_eq!(
            (
                &v["ledger"]["reserved"],
                &v["ledger"]["numbers"],
                &v["ledger"]["requests"]
            ),
            (&json!(true), &json!(1000), &json!(1))
        );
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].body.contains("\"count\":1000"), "{}", seen[0].body);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn an_import_numbers_locally_when_the_ledger_cant_be_asked() {
        let (_d, root) = catalogue(true);
        assigning(&root, 2);
        let mut send = sender(&root);
        let down =
            || -> Result<Ledger> { Ok(Ledger::at("http://127.0.0.1:9", "t", "acme/widgets")) };
        let mut w = Vec::new();
        let v = import_run(&mut send, &down, import(&root, false), &mut w).unwrap();
        assert_eq!(v["created"]["count"], 2);
        assert_eq!(v["ledger"]["reserved"], false);
        assert_eq!(w.len(), 1);
        let shown = send(Request::ItemShow {
            item: "MECH-00002".into(),
        })
        .unwrap();
        assert_eq!(shown["item"]["name"], "Part 1");
    }
}
