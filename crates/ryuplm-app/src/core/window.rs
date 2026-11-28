//! The window without egui (design 938, The window): the link to the
//! catalogue's daemon, what was asked and why, the Items list's rows and
//! every tile's answer, the person's selection and view, and what others
//! show. `pump` takes events; `tick` sends what is due and reconnects.

use crate::core::command::{Place, places};
use crate::core::documents;
use crate::core::flash::Flash;
use crate::core::link::{Daemon, Event, Link, Wake};
use crate::core::lists::{self, BomLine, Card, Change, Entry, Finding, Import, Release, Used};
use crate::core::opener;
use crate::core::people::{ItemView, People};
use crate::core::pointing::{Sent, Throttle};
use crate::core::product::Plm;
use crate::core::reader::Readers;
use crate::core::rows::{Ask, Filter, Rows};
use ryu_doc::error::{Code, Error, Result};
use ryu_session::rpc::Answer;
use ryuplm::proto::Request;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// How many activity entries the tile lists.
pub const ACTIVITY: usize = 100;
/// The lifecycle states the Navigator counts.
pub const STATES: [&str; 4] = ["in_work", "released", "modified", "obsolete"];

pub const RETRY_FIRST: Duration = Duration::from_millis(250);
pub const RETRY_MAX: Duration = Duration::from_secs(5);

/// The next wait after a failed reconnect: double, at most [`RETRY_MAX`].
pub fn next_delay(d: Duration) -> Duration {
    (d * 2).min(RETRY_MAX)
}

struct Retry {
    at: Instant,
    delay: Duration,
    trying: Option<Receiver<Result<Link>>>,
}

pub type Toasts = ryu_app::toast::Toasts<Plm>;

/// A request this build knows, from its method and params.
pub fn req(method: &str, params: Value) -> Request {
    Request::from_rpc(method, Some(params)).expect("a request this build knows")
}

fn result(a: Result<Answer>) -> Result<Value> {
    a.and_then(Answer::into_json).map(|a| a.result)
}

/// A tile's answer: the last that read, and why the newest didn't (the
/// tile greys the rows it keeps and says so over them).
#[derive(Debug)]
pub struct Got<T> {
    pub value: Option<T>,
    pub error: Option<Error>,
}

impl<T> Default for Got<T> {
    fn default() -> Self {
        Got {
            value: None,
            error: None,
        }
    }
}

impl<T> Got<T> {
    pub fn set(&mut self, r: Result<T>) {
        match r {
            Ok(v) => {
                self.value = Some(v);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn clear(&mut self) {
        self.value = None;
        self.error = None;
    }
}

/// The readers a tile's answers come from.
pub fn keys_of(tile: &str) -> &'static [&'static str] {
    match tile {
        "navigator" => &["info", "imports", "counts"],
        "item" => &["card", "settings", "reps"],
        "bom" => &["bom"],
        "where_used" => &["used"],
        "findings" => &["check"],
        "changes" => &["diff", "bom_diff"],
        "releases" => &["releases"],
        "imports" => &["imports"],
        "activity" => &["activity"],
        "documents" => &["documents"],
        _ => &[],
    }
}

/// The Navigator's counts from `item.list --counts` (one request for
/// all of them), by the keys the Navigator reads: `count:all`,
/// `count:class:<c>`, `count:state:<s>`, `count:faults:errors|warnings`
/// and `count:source:<mapping>`.
pub fn counts_from(v: &Value) -> BTreeMap<String, usize> {
    let c = &v["counts"];
    let n = |x: &Value| x.as_u64().unwrap_or(0) as usize;
    let mut out = BTreeMap::new();
    out.insert("count:all".to_owned(), n(&c["all"]));
    for (group, key) in [("class", "class"), ("state", "state"), ("source", "source")] {
        for (name, k) in c[group].as_object().into_iter().flatten() {
            out.insert(format!("count:{key}:{name}"), n(k));
        }
    }
    for s in STATES {
        out.entry(format!("count:state:{s}")).or_insert(0);
    }
    out.insert("count:faults:errors".to_owned(), n(&c["errors"]));
    out.insert("count:faults:warnings".to_owned(), n(&c["warnings"]));
    out
}

/// The view's new tab once a frame drew `drawn`: none while `tab` is
/// among them (or nothing was drawn), else the first drawn.
fn next_tab<'a>(tab: Option<&str>, drawn: &[&'a str]) -> Option<&'a str> {
    match drawn.first() {
        Some(first) if !tab.is_some_and(|t| drawn.contains(&t)) => Some(first),
        _ => None,
    }
}

/// Why a request was sent: what to do with its answer.
#[derive(Clone, Debug)]
enum Why {
    Rows(Ask),
    /// A reader's request; `item` the item it was about, so the answer
    /// for an item no longer open is dropped.
    Read {
        key: String,
        item: Option<String>,
    },
    /// The window's own Go to (`view.show`) of a reference.
    GoTo(String),
    /// A write (undo, redo): its failure toasts.
    Write(&'static str),
    People,
    /// `selection.set`: the next waits for its answer.
    Selected,
    /// `document.get` into this file, then the opener.
    Opened(PathBuf),
    Quiet,
}

pub struct Window {
    pub root: PathBuf,
    name: String,
    daemon: Daemon,
    wake: Wake,
    link: Link,
    pub lost: bool,
    pub lost_why: Option<Error>,
    retry: Option<Retry>,
    /// A service-stored document's bytes on their way through the CLI.
    opening: Option<(PathBuf, Receiver<Result<()>>)>,
    pending: BTreeMap<u64, Why>,
    readers: Readers,
    /// How many times each reader's request was sent: the window tests
    /// read it to see a burst cost one round.
    pub asks: BTreeMap<String, u32>,
    /// Tiles drawn at least once: their readers are asked, and asked
    /// again as the catalogue moves.
    shown: BTreeSet<&'static str>,
    pub people: People,
    pub toasts: Toasts,
    pub info: Got<Value>,
    pub counts: BTreeMap<String, usize>,
    pub rows: Rows,
    /// The open item's ID.
    pub item: Option<String>,
    pub card: Got<Card>,
    settings: Option<Value>,
    reps: Option<Value>,
    pub bom: Got<Vec<BomLine>>,
    /// Child items' own BOMs, opened a level at a time.
    pub children: BTreeMap<String, Got<Vec<BomLine>>>,
    pub used: Got<Vec<Used>>,
    pub findings: Got<Vec<Finding>>,
    /// Whether a check is running (Run check is disabled meanwhile).
    pub checking: bool,
    pub changes: Got<Vec<Change>>,
    diff: Option<Value>,
    bom_diff: Option<Value>,
    pub releases: Got<Vec<Release>>,
    /// Releases of every item, not only the open one's.
    pub releases_all: bool,
    pub imports: Got<Vec<Import>>,
    pub activity: Got<Vec<Entry>>,
    /// The Documents tab: every document (`document.list`), when it lists
    /// them all or no item is open ([`Window::docs_listing`]).
    pub documents: Got<Vec<documents::Doc>>,
    /// Each document's card by ID (`document.show`), as the tab needs them.
    pub doc_cards: BTreeMap<String, Got<documents::Card>>,
    /// Every document, not only the open item's: the person's choice.
    pub docs_all: bool,
    pub docs_query: String,
    /// The bottom tab shown.
    pub tab: Option<String>,
    /// The view's tabs drawn this frame, in the order drawn.
    tabs_drawn: Vec<&'static str>,
    /// A tab to bring to the front at the next frame (a show named it).
    pub open_tab: Option<String>,
    pub selection: Vec<String>,
    pub hover: Option<String>,
    pub flash: Flash,
    pub flash_on: bool,
    select: Throttle,
    view: Throttle,
    /// The views this window sent with `view.set`.
    sent: Sent,
    /// Whom the People strip follows.
    pub following: Option<String>,
    pub follow_shows: bool,
    /// The palette's `@` answer.
    pub places: Vec<Place>,
    /// Whether the first tick has asked for everything (after the rows).
    started: bool,
    places_query: Option<String>,
    /// The query of the `@` request out, so an answer to one no longer
    /// typed is dropped.
    places_out: Option<String>,
}

impl Window {
    /// Connects to the daemon of the catalogue at `root` as the human
    /// `name`, and asks for what every window shows.
    pub fn open(root: &Path, daemon: &Daemon, name: &str, wake: Wake) -> Result<Window> {
        let link = Link::open(root, daemon, name, wake.clone())?;
        let mut people = People::default();
        people.reset(&link.session["participants"]);
        let w = Window {
            root: root.to_owned(),
            name: name.to_owned(),
            daemon: daemon.clone(),
            wake,
            link,
            lost: false,
            lost_why: None,
            retry: None,
            opening: None,
            pending: BTreeMap::new(),
            readers: Readers::default(),
            asks: BTreeMap::new(),
            shown: BTreeSet::new(),
            people,
            toasts: Toasts::with(ryu_app::toast::Notify::parse(&crate::core::prefs::get(
                "ui.notify",
            ))),
            info: Got::default(),
            counts: BTreeMap::new(),
            rows: Rows::default(),
            item: None,
            card: Got::default(),
            settings: None,
            reps: None,
            bom: Got::default(),
            children: BTreeMap::new(),
            used: Got::default(),
            findings: Got::default(),
            checking: false,
            changes: Got::default(),
            diff: None,
            bom_diff: None,
            releases: Got::default(),
            releases_all: false,
            imports: Got::default(),
            activity: Got::default(),
            documents: Got::default(),
            doc_cards: BTreeMap::new(),
            docs_all: false,
            docs_query: String::new(),
            tab: None,
            tabs_drawn: Vec::new(),
            open_tab: None,
            selection: Vec::new(),
            hover: None,
            flash: Flash::default(),
            flash_on: crate::core::prefs::flag("ui.flash_changes"),
            select: Throttle::ordered(),
            view: Throttle::default(),
            sent: Sent::default(),
            following: None,
            follow_shows: crate::core::prefs::flag("ui.follow_shows"),
            places: Vec::new(),
            started: false,
            places_query: None,
            places_out: None,
        };
        Ok(w)
    }

    pub fn me(&self) -> &str {
        &self.link.participant
    }

    fn send(&mut self, r: Request, why: Why) -> u64 {
        let t = self.link.send(r);
        self.pending.insert(t, why);
        t
    }

    fn send_aside(&mut self, r: Request, why: Why) {
        let t = self.link.send_aside(r, Default::default());
        self.pending.insert(t, why);
    }

    /// What every window reads, and every shown tile's readers.
    fn ask_all(&mut self) {
        self.want("info");
        self.want("activity");
        self.refresh();
    }

    /// Asks every shown tile's readers again, and the opened BOM rows.
    fn refresh(&mut self) {
        let keys: Vec<&'static str> = self
            .shown
            .iter()
            .flat_map(|t| keys_of(t).iter().copied())
            .collect();
        for k in keys {
            self.want(k);
        }
        let kids: Vec<String> = self.children.keys().map(|c| format!("child:{c}")).collect();
        for k in kids {
            self.want(&k);
        }
        let docs: Vec<String> = self.doc_cards.keys().map(|d| format!("doc:{d}")).collect();
        for k in docs {
            self.want(&k);
        }
    }

    /// The tile `tile` is drawn this frame: its readers are asked the
    /// first time, and a bottom tab drawn is the view's tab.
    pub fn shows(&mut self, tile: &'static str) {
        if self.shown.insert(tile) {
            for k in keys_of(tile) {
                self.want(k);
            }
            if tile == "documents" {
                self.ask_item_docs();
            }
        }
        if ryuplm::view::TABS.contains(&tile) {
            self.tabs_drawn.push(tile);
        }
    }

    /// The frame's tiles are drawn: the view's tab stays while it is drawn
    /// (two bottom tabs split side by side are both drawn every frame),
    /// and is the first drawn otherwise.
    pub fn drawn(&mut self) {
        let drawn = std::mem::take(&mut self.tabs_drawn);
        if let Some(t) = next_tab(self.tab.as_deref(), &drawn) {
            self.tab = Some(t.to_owned());
            self.view_due();
        }
    }

    /// Asks reader `key`: now, or once more when the one out is answered.
    fn want(&mut self, key: &str) {
        if self.readers.ask(key) {
            self.send_for(key);
        }
    }

    /// The reader's request was answered: sends it again for what was
    /// asked meanwhile.
    fn done(&mut self, key: &str) {
        if self.readers.answered(key) {
            self.send_for(key);
        }
    }

    fn send_for(&mut self, key: &str) {
        let Some((r, item)) = self.request_for(key) else {
            // Nothing to ask (no item is open): nothing is out.
            self.readers.answered(key);
            return;
        };
        *self.asks.entry(key.to_owned()).or_default() += 1;
        if key == "places" {
            self.places_out = self.places_query.clone();
        }
        let why = Why::Read {
            key: key.to_owned(),
            item,
        };
        // Every reader on the aside connection: the main one carries the
        // Items list's windows and pointing, which never queue behind a
        // tile's answer (at 100,000 items each request is a few hundred
        // milliseconds of the daemon's).
        self.send_aside(r, why);
    }

    fn request_for(&self, key: &str) -> Option<(Request, Option<String>)> {
        let item = || self.item.clone();
        let of = |m: &str, id: String| (req(m, json!({"item": id})), Some(id));
        Some(match key {
            "info" => (req("project.info", json!({})), None),
            "settings" => (req("settings.show", json!({})), None),
            "reps" => (req("reps.list", json!({})), None),
            "imports" => (req("import.list", json!({})), None),
            "check" => (req("check", json!({})), None),
            "diff" => (req("diff", json!({})), None),
            "bom_diff" => (req("bom.diff", json!({})), None),
            "activity" => (req("activity.log", json!({"limit": ACTIVITY})), None),
            "card" => of("item.show", item()?),
            "bom" => of("bom.get", item()?),
            "used" => of("bom.where_used", item()?),
            "releases" => match (self.releases_all, item()) {
                (false, Some(id)) => of("release.list", id),
                _ => (req("release.list", json!({})), None),
            },
            "places" => {
                let q = self.places_query.clone()?;
                (req("item.list", json!({"text": q, "limit": 12})), None)
            }
            "documents" => {
                let mut p = json!({"limit": 200});
                if !self.docs_query.trim().is_empty() {
                    p["text"] = json!(self.docs_query.trim());
                }
                (req("document.list", p), None)
            }
            k if k.starts_with("doc:") => {
                let id = k["doc:".len()..].to_owned();
                (req("document.show", json!({"document": id})), None)
            }
            k if k.starts_with("child:") => {
                let id = k["child:".len()..].to_owned();
                (req("bom.get", json!({"item": id})), Some(id))
            }
            "counts" => (req("item.list", json!({"limit": 0, "counts": true})), None),
            _ => return None,
        })
    }

    /// Opens item `id` (or none) in the Item tile and the item's tabs.
    pub fn open_item(&mut self, id: Option<String>) {
        if self.set_item(id) {
            self.view_due();
        }
    }

    /// [`Window::open_item`] without sending the view: what a show or a
    /// follow moved. Whether it moved.
    fn set_item(&mut self, id: Option<String>) -> bool {
        if self.item == id {
            return false;
        }
        self.item = id;
        self.card.clear();
        self.bom.clear();
        self.used.clear();
        self.children.clear();
        self.doc_cards.clear();
        let keys: Vec<&'static str> = ["item", "bom", "where_used", "releases"]
            .into_iter()
            .filter(|t| self.shown.contains(t))
            .flat_map(|t| keys_of(t).iter().copied())
            .filter(|k| matches!(*k, "card" | "bom" | "used" | "releases"))
            .collect();
        for k in keys {
            self.want(k);
        }
        if self.item.is_none() && self.shown.contains("documents") {
            // The tab lists every document again.
            self.want("documents");
        }
        true
    }

    /// Reads the open item's own documents' cards.
    fn ask_item_docs(&mut self) {
        let ids = self
            .card
            .value
            .as_ref()
            .map(|c| documents::local(&c.documents))
            .unwrap_or_default();
        for id in ids {
            self.want(&format!("doc:{id}"));
        }
    }

    /// Every document (and its search), or the open item's: the person's
    /// choice, kept as items open and close.
    pub fn set_docs(&mut self, all: bool, query: &str) {
        let moved = self.docs_all != all || self.docs_query != query;
        self.docs_all = all;
        self.docs_query = query.to_owned();
        if moved && self.docs_listing() {
            self.want("documents");
        }
    }

    /// Whether the Documents tab lists every document: chosen, or no item
    /// is open.
    pub fn docs_listing(&self) -> bool {
        self.docs_all || self.item.is_none()
    }

    /// The card of document `id`, read once it is wanted.
    pub fn doc_card(&mut self, id: &str) {
        if !self.doc_cards.contains_key(id) {
            self.doc_cards.insert(id.to_owned(), Got::default());
            self.want(&format!("doc:{id}"));
        }
    }

    /// Opens document `id`'s bytes with the OS's opener: written by the
    /// daemon for a Git-stored document, by the CLI (which fetches them
    /// first) for a service-stored one.
    pub fn open_document(&mut self, id: &str) {
        let Some(c) = self.doc_cards.get(id).and_then(|g| g.value.clone()) else {
            return;
        };
        let out = opener::temp_path(&c.id, &c.file_name);
        if let Some(dir) = out.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            self.toasts
                .error("Open", &Error::io(dir, &e), 0, Instant::now());
            return;
        }
        if c.storage == "git" {
            self.send(
                req(
                    "document.get",
                    json!({"document": c.id, "out": out.display().to_string()}),
                ),
                Why::Opened(out),
            );
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (exe, root, wake, file) = (
            self.daemon.exe.clone(),
            self.root.clone(),
            self.wake.clone(),
            out.clone(),
        );
        std::thread::spawn(move || {
            let _ = tx.send(opener::fetch_cli(&exe, &root, &c.id, &file));
            wake();
        });
        self.opening = Some((out, rx));
    }

    /// Whether a document is being opened.
    pub fn opening(&self) -> bool {
        self.opening.is_some()
    }

    /// The Items list's filter and sort.
    pub fn set_filter(&mut self, f: Filter) {
        if f != self.rows.filter {
            self.rows.filter(f);
            self.view_due();
        }
    }

    /// The palette's `@query`: one `item.list` by text, the last query
    /// winning.
    pub fn search_places(&mut self, query: &str) {
        if self.places_query.as_deref() == Some(query) {
            return;
        }
        self.places_query = Some(query.to_owned());
        if query.trim().is_empty() {
            self.places.clear();
        } else {
            self.want("places");
        }
    }

    /// Opens (or closes) child item `id`'s own lines in the BOM tile.
    pub fn expand(&mut self, id: &str, open: bool) {
        if !open {
            self.children.remove(id);
        } else if !self.children.contains_key(id) {
            self.children.insert(id.to_owned(), Got::default());
            self.want(&format!("child:{id}"));
        }
    }

    /// Lists every item's releases, or only the open one's.
    pub fn set_releases_all(&mut self, all: bool) {
        if self.releases_all != all {
            self.releases_all = all;
            self.releases.clear();
            self.want("releases");
        }
    }

    pub fn run_check(&mut self) {
        self.shown.insert("findings");
        self.checking = true;
        self.want("check");
    }

    /// Undoes the newest entry that can be, whoever made it.
    pub fn undo(&mut self) {
        let p = self
            .activity
            .value
            .iter()
            .flatten()
            .find(|e| e.undo)
            .map_or_else(|| json!({}), |e| json!({"entry": e.seq}));
        self.send(req("undo", p), Why::Write("Undo"));
    }

    /// Redoes the newest undo not redone yet.
    pub fn redo(&mut self) {
        let p = self
            .activity
            .value
            .iter()
            .flatten()
            .find(|e| e.redo)
            .map_or_else(|| json!({}), |e| json!({"entry": e.seq}));
        self.send(req("redo", p), Why::Write("Redo"));
    }

    /// Takes every event that has arrived; whether any did.
    pub fn pump(&mut self) -> bool {
        let mut any = false;
        while let Some(e) = self.link.try_event() {
            self.handle(e);
            any = true;
        }
        any
    }

    /// Waits up to `timeout` for `done`, taking events: for tests and
    /// `--smoke`.
    pub fn wait_until(&mut self, timeout: Duration, mut done: impl FnMut(&Window) -> bool) -> bool {
        let end = Instant::now() + timeout;
        while !done(self) {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            match self.link.wait_event(left.min(Duration::from_millis(50))) {
                Some(e) => self.handle(e),
                None => {
                    self.tick(Instant::now());
                }
            }
        }
        true
    }

    /// Whether nothing is waiting on the daemon.
    pub fn idle(&self) -> bool {
        self.pending.values().all(|w| matches!(w, Why::Quiet)) && !self.rows.busy()
    }

    fn handle(&mut self, e: Event) {
        match e {
            Event::Answer { ticket, answer } => {
                if let Some(why) = self.pending.remove(&ticket) {
                    self.answered(why, result(answer));
                }
            }
            Event::Note { method, params } => self.noted(&method, &params),
            Event::Progress { .. } => {}
            Event::Lost => self.lost_link(),
        }
    }

    fn answered(&mut self, why: Why, a: Result<Value>) {
        let now = Instant::now();
        match why {
            Why::Rows(ask) => match a {
                Ok(v) => self.rows.answered(&ask, &v),
                Err(_) => self.rows.failed(&ask),
            },
            Why::Read { key, item } => {
                let current = item.is_none() || key.starts_with("child:") || item == self.item;
                if current {
                    self.read(&key, a);
                }
                self.done(&key);
            }
            Why::GoTo(r) => match a {
                // The session selected it with the show: the window takes
                // the same, its view moved or not.
                Ok(_) => self.selection = vec![r],
                Err(e) => self.toasts.error("Show", &e, 0, now),
            },
            Why::Write(title) => {
                if let Err(e) = a {
                    self.toasts.error(title, &e, 0, now);
                }
            }
            Why::Opened(out) => match a.and_then(|_| opener::open(&out)) {
                Ok(()) => {}
                Err(e) => self.toasts.error("Open", &e, 0, now),
            },
            Why::People => {
                if let Ok(v) = a {
                    self.people.reset(&v["participants"]);
                }
            }
            Why::Selected => {
                self.select.answered();
                self.send_selection(now);
            }
            Why::Quiet => {}
        }
    }

    fn read(&mut self, key: &str, a: Result<Value>) {
        let bad = || Error::new(Code::Internal, "the daemon's answer didn't read");
        match key {
            "info" => {
                if let Ok(v) = &a
                    && let Some(r) = v["revision"].as_str()
                {
                    self.rows.moved(r);
                }
                self.info.set(a);
            }
            "card" => {
                if let Err(e) = &a
                    && e.code == Code::ItemNotFound
                {
                    // Deleted since it was opened.
                    self.set_item(None);
                    return;
                }
                let card = a.and_then(|v| Card::read(&v).ok_or_else(bad)).map(|mut c| {
                    if let Some(s) = &self.settings {
                        c.types(s);
                    }
                    if let Some(r) = &self.reps {
                        c.reps(r);
                    }
                    c
                });
                self.card.set(card);
                if self.shown.contains("documents") {
                    self.ask_item_docs();
                }
            }
            "settings" => {
                if let Ok(v) = a {
                    if let Some(c) = self.card.value.as_mut() {
                        c.types(&v);
                    }
                    self.settings = Some(v);
                }
            }
            "reps" => {
                if let Ok(v) = a {
                    if let Some(c) = self.card.value.as_mut() {
                        c.reps(&v);
                    }
                    self.reps = Some(v);
                }
            }
            "bom" => self.bom.set(a.map(|v| lists::bom(&v))),
            "used" => self.used.set(a.map(|v| lists::where_used(&v))),
            "check" => {
                // A check asked again meanwhile is still to come.
                self.checking = self.readers.again("check");
                let r = match &a {
                    Err(e) if e.code != Code::CheckFailed => Err(e.clone()),
                    _ => Ok(lists::findings(&a)),
                };
                self.findings.set(r);
            }
            "diff" => match a {
                Ok(v) => {
                    self.diff = Some(v);
                    self.changes_now();
                }
                Err(e) => self.changes.set(Err(e)),
            },
            "bom_diff" => {
                self.bom_diff = a.ok();
                self.changes_now();
            }
            "releases" => self.releases.set(a.map(|v| lists::releases(&v))),
            "imports" => self.imports.set(a.map(|v| lists::imports(&v))),
            "activity" => self.activity.set(a.map(|v| lists::activity(&v))),
            "places" => {
                // Typed over or cleared since it was sent: the newer query's
                // answer (or none) stands.
                if let Ok(v) = a
                    && self.places_out == self.places_query
                {
                    self.places = places(&v);
                }
            }
            "documents" => self.documents.set(a.map(|v| documents::list(&v))),
            k if k.starts_with("doc:") => {
                let card = a.and_then(|v| {
                    documents::Card::read(&v).ok_or_else(|| {
                        Error::new(Code::Internal, "the daemon's answer didn't read")
                    })
                });
                self.doc_cards
                    .entry(k["doc:".len()..].to_owned())
                    .or_default()
                    .set(card);
            }
            k if k.starts_with("child:") => {
                if let Some(g) = self.children.get_mut(&k["child:".len()..]) {
                    g.set(a.map(|v| lists::bom(&v)));
                }
            }
            "counts" => {
                if let Ok(v) = a {
                    self.counts = counts_from(&v);
                }
            }
            _ => {}
        }
    }

    fn changes_now(&mut self) {
        if let Some(d) = &self.diff {
            let c = lists::changes(d, self.bom_diff.as_ref());
            self.changes.set(Ok(c));
        }
    }

    fn noted(&mut self, method: &str, params: &Value) {
        let now = Instant::now();
        match method {
            "project.changed" => {
                if let Some(r) = params["revisions"]["catalogue"].as_str() {
                    self.rows.moved(r);
                }
                if self.flash_on {
                    self.flash.changed(&params["changed"], now);
                }
                self.want("info");
                self.refresh();
            }
            "participant.changed" => {
                let me = self.me().to_owned();
                let before = self.people.get(&me).map(|p| p.view.clone());
                self.people.changed(params);
                if let Some(mine) = self.people.get(&me).map(|p| p.view.clone())
                    && self
                        .sent
                        .adopts(before.as_ref(), &mine, &me, self.follow_shows)
                {
                    let selection = self
                        .people
                        .get(&me)
                        .map(|p| p.selection.clone())
                        .unwrap_or_default();
                    self.adopt(&mine);
                    self.selection = selection;
                }
                if let Some(f) = self.following.clone() {
                    match self.people.get(&f).map(|p| p.view.clone()) {
                        Some(v) => self.adopt(&v),
                        None => self.following = None,
                    }
                }
            }
            "activity.appended" => {
                let me = self.me().to_owned();
                self.toasts.activity(params, &me, now);
                self.want("activity");
            }
            "resync" => {
                self.send(req("participant.list", json!({})), Why::People);
                self.ask_all();
            }
            _ => {}
        }
    }

    /// The link went: every tile greys until it is found again.
    fn lost_link(&mut self) {
        if self.lost {
            return;
        }
        self.lost = true;
        self.pending.clear();
        self.select.answered();
        self.readers.reset();
        self.rows.lost();
        self.checking = false;
        self.toasts.lost(Instant::now());
        self.retry = Some(Retry {
            at: Instant::now() + RETRY_FIRST,
            delay: RETRY_FIRST,
            trying: None,
        });
    }

    fn view_due(&mut self) {
        self.view.due();
    }

    /// The person picked `reference` (or nothing): one, or with `add`
    /// added or taken out. Sent at most every 150 ms.
    pub fn pick(&mut self, reference: Option<String>, add: bool) {
        match (reference, add) {
            (None, false) => self.selection.clear(),
            (None, true) => {}
            (Some(r), true) => {
                if let Some(i) = self.selection.iter().position(|x| *x == r) {
                    self.selection.remove(i);
                } else {
                    self.selection.push(r);
                }
            }
            (Some(r), false) => self.selection = vec![r],
        }
        self.select.due();
        // Sent now when the throttle allows, ahead of the readers the pick
        // starts (a click opens the item: its card and BOM go aside too, and
        // at 100,000 items each costs a freshness scan, #1018). Otherwise
        // the next tick sends the last value.
        self.send_selection(std::time::Instant::now());
    }

    /// The selection to the daemon, when the throttle allows: on the main
    /// connection while the Items list has nothing out there (a pick then
    /// waits behind no view.set or tile reader), else aside, so a search
    /// never waits behind it. The daemon serves the two connections apart,
    /// so one `selection.set` is out at a time: the next (the last value)
    /// goes once it is answered, and the session applies them in order.
    fn send_selection(&mut self, now: Instant) {
        if self.select.ready(now) {
            let r = req("selection.set", json!({"refs": self.selection.clone()}));
            if self.rows.busy() {
                self.send_aside(r, Why::Selected);
            } else {
                self.send(r, Why::Selected);
            }
        }
    }

    /// Go to: the session shows `reference` to this window's participant,
    /// and the window adopts the show.
    pub fn go_to(&mut self, reference: &str) {
        let me = self.me().to_owned();
        self.sent.show();
        self.send(
            req("view.show", json!({"ref": reference, "participant": me})),
            Why::GoTo(reference.to_owned()),
        );
    }

    /// Follows `who` (or stops): their item, tab and filter as they move.
    pub fn follow(&mut self, who: Option<String>) {
        self.following = who;
        if let Some(v) = self
            .following
            .clone()
            .and_then(|id| self.people.get(&id).map(|p| p.view.clone()))
        {
            self.adopt(&v);
        }
    }

    /// Takes `v`'s item, tab and list.
    fn adopt(&mut self, v: &ItemView) {
        self.set_item(v.item.clone());
        if v.tab.is_some() && v.tab != self.tab {
            self.tab = v.tab.clone();
            self.open_tab = v.tab.clone();
        }
        self.rows.filter(v.list.clone());
    }

    fn view_now(&self) -> ItemView {
        ItemView {
            item: self.item.clone(),
            tab: self.tab.clone(),
            list: self.rows.filter.clone(),
            shown_by: Some(self.me().to_owned()),
        }
    }

    /// Sends what is due and expires toasts and flashes. Returns how soon
    /// it needs calling again, if it does.
    pub fn tick(&mut self, now: Instant) -> Option<Duration> {
        self.toasts.expire(now);
        self.flash.expire(now);
        if self.lost {
            self.reconnect(now);
            return Some(
                self.retry
                    .as_ref()
                    .map_or(RETRY_FIRST, |r| r.at.saturating_duration_since(now)),
            );
        }
        if let Some((out, rx)) = &self.opening
            && let Ok(r) = rx.try_recv()
        {
            let out = out.clone();
            self.opening = None;
            if let Err(e) = r.and_then(|()| opener::open(&out)) {
                self.toasts.error("Open", &e, 0, now);
            }
        }
        if let Some(a) = self.rows.ask_at(now) {
            self.send(req("item.list", Rows::params(&a)), Why::Rows(a));
        }
        // The first rows are asked before anything else, so they lead the
        // daemon's queue when the window opens.
        if !self.started {
            self.started = true;
            self.ask_all();
        }
        self.send_selection(now);
        if self.view.ready(now) && self.following.is_none() {
            let v = self.view_now();
            let mut p = json!({"list": v.list});
            if let Some(i) = &v.item {
                p["item"] = json!(i);
            }
            if let Some(t) = &v.tab {
                p["tab"] = json!(t);
            }
            self.sent.set(v);
            self.send_aside(req("view.set", p), Why::Quiet);
        }
        let flash = self.flash.due().then_some(Duration::from_millis(33));
        [
            self.select.wait(now),
            self.view.wait(now),
            self.rows.wait(now),
            flash,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn reconnect(&mut self, now: Instant) {
        let Some(retry) = &mut self.retry else { return };
        if let Some(rx) = &retry.trying {
            match rx.try_recv() {
                Err(TryRecvError::Empty) => return,
                Ok(Ok(link)) => {
                    self.retry = None;
                    self.reconnected(link);
                    return;
                }
                Ok(Err(e)) => self.lost_why = Some(e),
                Err(TryRecvError::Disconnected) => {}
            }
            retry.trying = None;
            retry.at = now + retry.delay;
            retry.delay = next_delay(retry.delay);
            return;
        }
        if now < retry.at {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (root, daemon, name, wake) = (
            self.root.clone(),
            self.daemon.clone(),
            self.name.clone(),
            self.wake.clone(),
        );
        std::thread::spawn(move || {
            let _ = tx.send(Link::open(&root, &daemon, &name, wake.clone()));
            wake();
        });
        retry.trying = Some(rx);
    }

    fn reconnected(&mut self, link: Link) {
        self.people.reset(&link.session["participants"]);
        self.link = link;
        self.lost = false;
        self.lost_why = None;
        self.toasts.found(Instant::now());
        self.select.due();
        self.select.reset();
        self.view.due();
        self.view.reset();
        self.ask_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_the_window_sends_parses() {
        for (m, p) in [
            ("project.info", json!({})),
            (
                "item.list",
                json!({"offset": 0, "limit": 230, "sort": "number", "text": "bolt"}),
            ),
            ("item.list", json!({"limit": 0, "counts": true})),
            ("item.list", json!({"text": "100-00", "limit": 12})),
            ("item.show", json!({"item": "k3v9q2m8x4c1p"})),
            ("settings.show", json!({})),
            ("reps.list", json!({})),
            ("bom.get", json!({"item": "k3v9q2m8x4c1p"})),
            ("bom.where_used", json!({"item": "k3v9q2m8x4c1p"})),
            ("check", json!({})),
            ("diff", json!({})),
            ("bom.diff", json!({})),
            ("release.list", json!({})),
            ("release.list", json!({"item": "k3v9q2m8x4c1p"})),
            ("import.list", json!({})),
            ("activity.log", json!({"limit": ACTIVITY})),
            ("undo", json!({})),
            ("redo", json!({"entry": 3})),
            ("participant.list", json!({})),
            ("selection.set", json!({"refs": ["item/k3v9q2m8x4c1p"]})),
            (
                "view.set",
                json!({"item": "k3v9q2m8x4c1p", "tab": "bom", "list": {"sort": "number"}}),
            ),
            (
                "view.show",
                json!({"ref": "item/k3v9q2m8x4c1p", "participant": "p-1"}),
            ),
        ] {
            assert!(Request::from_rpc(m, Some(p.clone())).is_ok(), "{m} {p}");
        }
    }

    #[test]
    fn the_navigator_s_counts_are_one_answer() {
        let c = counts_from(&json!({"counts": {"all": 5, "class": {"fastener": 3},
            "state": {"in_work": 4, "released": 1}, "source": {"erp": 2},
            "errors": 1, "warnings": 2}}));
        assert_eq!(c["count:all"], 5);
        assert_eq!(c["count:class:fastener"], 3);
        assert_eq!(c["count:state:released"], 1);
        assert_eq!(c["count:state:obsolete"], 0);
        assert_eq!(c["count:source:erp"], 2);
        assert_eq!(
            (c["count:faults:errors"], c["count:faults:warnings"]),
            (1, 2)
        );
    }

    #[test]
    fn a_tile_names_its_readers() {
        assert_eq!(keys_of("item"), ["card", "settings", "reps"]);
        assert_eq!(keys_of("changes"), ["diff", "bom_diff"]);
        assert_eq!(keys_of("findings"), ["check"]);
        assert!(keys_of("items").is_empty(), "the list asks for itself");
        for (id, _) in crate::core::tiles::TILES {
            if ryuplm::view::TABS.contains(&id) {
                assert!(!keys_of(id).is_empty(), "{id} reads nothing");
            }
        }
    }

    #[test]
    fn two_tabs_drawn_side_by_side_keep_the_view_s() {
        assert_eq!(next_tab(None, &["bom", "findings"]), Some("bom"));
        assert_eq!(next_tab(Some("findings"), &["bom", "findings"]), None);
        assert_eq!(next_tab(Some("bom"), &["bom", "findings"]), None);
        assert_eq!(next_tab(Some("bom"), &["releases"]), Some("releases"));
        assert_eq!(next_tab(Some("bom"), &[]), None, "nothing drawn");
    }

    #[test]
    fn retries_back_off_to_five_seconds() {
        let mut d = RETRY_FIRST;
        let mut seen = Vec::new();
        for _ in 0..8 {
            seen.push(d);
            d = next_delay(d);
        }
        assert_eq!(seen[0], Duration::from_millis(250));
        assert_eq!(seen[1], Duration::from_millis(500));
        assert_eq!(*seen.last().unwrap(), RETRY_MAX);
    }

    #[test]
    fn the_documents_tab_reads_lists_and_cards() {
        assert_eq!(keys_of("documents"), ["documents"]);
        for (m, p) in [
            ("document.list", json!({"limit": 200})),
            ("document.list", json!({"text": "spec", "limit": 200})),
            ("document.show", json!({"document": "d000000000001"})),
        ] {
            assert!(Request::from_rpc(m, Some(p.clone())).is_ok(), "{m} {p}");
        }
    }
}
