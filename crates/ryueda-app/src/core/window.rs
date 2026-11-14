//! The window without egui (design 519, The window): the link to the
//! daemon, what was asked and why, the scene and the lists, the person's
//! selection and view, and what others show. `pump` takes events, `tick`
//! sends what is due and reconnects.

use crate::core::flash::Flash;
use crate::core::link::{Daemon, Event, Link, Wake};
use crate::core::lists::{self, Entry, Findings, Instruction, Properties, Subject};
use crate::core::nav::Navigator;
use crate::core::people::{People, SheetView};
use crate::core::product::Eda;
use crate::core::scene::{P, Scene};
use ryu_app::canvas::Camera;
use ryu_doc::error::{Error, Result};
use ryu_session::rpc::Answer;
use ryueda::proto::Request;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// How often the selection and the view go to the daemon, at most.
pub const SELECT_EVERY: Duration = Duration::from_millis(150);
pub const RETRY_FIRST: Duration = Duration::from_millis(250);
pub const RETRY_MAX: Duration = Duration::from_secs(5);
/// How many activity entries the tile lists.
pub const ACTIVITY: usize = 100;

pub type Toasts = ryu_app::toast::Toasts<Eda>;

/// A request this build knows, from its method and params.
pub fn req(method: &str, params: Value) -> Request {
    Request::from_rpc(method, Some(params)).expect("a request this build knows")
}

/// A camera as `view.set` takes it.
pub fn to_nm(c: &Camera) -> Value {
    json!({
        "centre_nm": [(c.centre[0] * 1e6).round() as i64, (c.centre[1] * 1e6).round() as i64],
        "zoom_nm_per_pt": ((1e6 / c.zoom).round() as i64).max(1),
    })
}

pub fn from_nm((centre, zoom): ([i64; 2], i64)) -> Camera {
    Camera {
        centre: [centre[0] as f64 / 1e6, centre[1] as f64 / 1e6],
        zoom: 1e6 / zoom.max(1) as f64,
    }
}

/// Sends the last value at most every [`SELECT_EVERY`].
#[derive(Debug, Default)]
pub struct Throttle {
    due: bool,
    sent: Option<Instant>,
}

impl Throttle {
    pub fn due(&mut self) {
        self.due = true;
    }
    /// Whether to send now; if so, it counts as sent.
    pub fn ready(&mut self, now: Instant) -> bool {
        let ok = self.due
            && self
                .sent
                .is_none_or(|t| now.saturating_duration_since(t) >= SELECT_EVERY);
        if ok {
            self.due = false;
            self.sent = Some(now);
        }
        ok
    }
    /// How long until a due value may go.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        let t = self.sent.filter(|_| self.due)?;
        Some(SELECT_EVERY.saturating_sub(now.saturating_duration_since(t))).filter(|d| !d.is_zero())
    }
    pub fn reset(&mut self) {
        self.sent = None;
    }
}

/// Whether a window adopts its participant's view as the session now has
/// it: when it shows a sheet, differs from what the window last sent,
/// and is the window's own Go to or (with `follow_shows`) another's show.
pub fn adopts(now: &SheetView, sent: Option<&SheetView>, me: &str, follow_shows: bool) -> bool {
    let place = |v: &SheetView| (v.block.clone(), v.sheet.clone(), v.layer.clone(), v.camera);
    if now.block.is_none() || now.sheet.is_none() || sent.is_some_and(|s| place(s) == place(now)) {
        return false;
    }
    now.shown_by.as_deref() == Some(me) || follow_shows
}

fn result(a: Result<Answer>) -> Result<Value> {
    a.and_then(Answer::into_json).map(|a| a.result)
}

/// Why a request was sent: what to do with its answer.
#[derive(Clone, Debug)]
enum Why {
    Scene,
    OtherSheet,
    Designs,
    Netlist(String),
    Libcomps,
    Block,
    Rules,
    Erc,
    Check,
    Instructions(String),
    Activity,
    Component(String),
    Libcomp(String),
    Net(String),
    PropsInstructions(String),
    /// A write (undo, redo): its failure toasts.
    Write(&'static str),
    People,
    Quiet,
}

struct Retry {
    at: Instant,
    delay: Duration,
    trying: Option<Receiver<Result<Link>>>,
}

/// What the window shows: a block's sheet, as one of its layers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    pub design: Option<String>,
    pub block: Option<String>,
    pub sheet: Option<String>,
    pub layer: Option<String>,
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
    pending: BTreeMap<u64, Why>,
    pub people: People,
    pub toasts: Toasts,
    pub nav: Navigator,
    pub shown: Shown,
    pub scene: Option<Scene>,
    /// Why the shown scene couldn't be drawn again: the window greys it.
    pub scene_error: Option<Error>,
    pub dark: bool,
    /// In millimetres; `None` fits the sheet at the next frame.
    pub camera: Option<Camera>,
    /// A box to frame at the next frame (a show, a finding's Show).
    pub frame: Option<[P; 2]>,
    /// A reference to frame once the scene now asked for arrives (a
    /// link followed to another sheet).
    pub frame_ref: Option<String>,
    pub selection: Vec<String>,
    pub hover: Option<String>,
    select: Throttle,
    view: Throttle,
    sent_view: Option<SheetView>,
    pub following: Option<String>,
    pub follow_shows: bool,
    pub flash: Flash,
    pub flash_on: bool,
    /// Frames, notes, intent, pin numbers: `ryusch::paint::Layer::ALL`'s
    /// order.
    pub layers: [bool; 4],
    pub findings: Findings,
    pub instructions: Vec<Instruction>,
    pub activity: Vec<Entry>,
    pub props: Option<Properties>,
    /// The last `check`'s state: `None` never run, `Some(false)` running.
    pub checked: Option<bool>,
}

impl Window {
    /// Connects to the daemon of the project at `root` as the human
    /// `name`, and asks for everything the window shows.
    pub fn open(
        root: &Path,
        daemon: &Daemon,
        name: &str,
        wake: Wake,
        dark: bool,
    ) -> Result<Window> {
        let link = Link::open(root, daemon, name, wake.clone())?;
        let mut people = People::default();
        people.reset(&link.session["participants"]);
        let mut w = Window {
            root: root.to_owned(),
            name: name.to_owned(),
            daemon: daemon.clone(),
            wake,
            link,
            lost: false,
            lost_why: None,
            retry: None,
            pending: BTreeMap::new(),
            people,
            toasts: Toasts::with(ryu_app::toast::Notify::parse(&crate::core::prefs::get(
                "ui.notify",
            ))),
            nav: Navigator::default(),
            shown: Shown::default(),
            scene: None,
            scene_error: None,
            dark,
            camera: None,
            frame: None,
            frame_ref: None,
            selection: Vec::new(),
            hover: None,
            select: Throttle::default(),
            view: Throttle::default(),
            sent_view: None,
            following: None,
            follow_shows: crate::core::prefs::flag("ui.follow_shows"),
            flash: Flash::default(),
            flash_on: crate::core::prefs::flag("ui.flash_changes"),
            layers: [true; 4],
            findings: Findings::default(),
            instructions: Vec::new(),
            activity: Vec::new(),
            props: None,
            checked: None,
        };
        w.ask_all();
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

    fn ask_all(&mut self) {
        self.ask_scene();
        self.send(req("design.list", json!({})), Why::Designs);
        self.send(req("libcomp.list", json!({})), Why::Libcomps);
        self.send(req("lint.get", json!({})), Why::Rules);
        self.send(req("erc", json!({})), Why::Erc);
        self.send(
            req("activity.log", json!({"limit": ACTIVITY})),
            Why::Activity,
        );
    }

    fn scene_params(&self, sheet: Option<&str>) -> Value {
        let mut p = json!({"theme": if self.dark { "dark" } else { "light" }});
        for (k, v) in [
            ("design", self.shown.design.as_deref()),
            ("block", self.shown.block.as_deref()),
            ("sheet", sheet),
            ("layer", self.shown.layer.as_deref()),
        ] {
            if let Some(v) = v {
                p[k] = json!(v);
            }
        }
        p
    }

    /// Asks the shown sheet's scene.
    pub fn ask_scene(&mut self) {
        let p = self.scene_params(self.shown.sheet.clone().as_deref());
        self.send(req("scene", p), Why::Scene);
    }

    /// Asks the shown block's other sheets, aside.
    fn ask_other_sheets(&mut self) {
        let Some(sc) = &self.scene else { return };
        let others: Vec<String> = sc
            .sheets
            .iter()
            .map(|(s, _)| s.clone())
            .filter(|s| *s != sc.sheet)
            .collect();
        for s in others {
            let p = self.scene_params(Some(&s));
            self.send_aside(req("scene", p), Why::OtherSheet);
        }
    }

    /// Shows a block's sheet (its first, unless named) as a layer.
    pub fn show(&mut self, block: Option<String>, sheet: Option<String>, layer: Option<String>) {
        let moved = self.shown.block != block || self.shown.layer != layer;
        self.shown.block = block;
        self.shown.sheet = sheet;
        self.shown.layer = layer;
        if moved {
            self.camera = None;
        }
        self.ask_scene();
        self.view.due();
    }

    /// The next (or previous) sheet in canvas order.
    pub fn step_sheet(&mut self, by: isize) {
        let Some(sc) = &self.scene else { return };
        let Some(i) = sc.sheets.iter().position(|(s, _)| *s == sc.sheet) else {
            return;
        };
        let n = sc.sheets.len() as isize;
        let j = (i as isize + by).rem_euclid(n) as usize;
        let next = sc.sheets[j].0.clone();
        let (block, layer) = (self.shown.block.clone(), self.shown.layer.clone());
        self.camera = None;
        self.show(block, Some(next), layer);
    }

    /// Opens a reference's sheet, selects it and frames it: `view.show` to
    /// this window's own participant, adopted when it comes back. `block:`
    /// places open a block's first sheet.
    pub fn go_to(&mut self, reference: &str) {
        if let Some(b) = reference.strip_prefix("block:") {
            self.show(Some(b.to_owned()), None, None);
            return;
        }
        let me = self.me().to_owned();
        self.send(
            req("view.show", json!({"ref": reference, "participant": me})),
            Why::Write("Show"),
        );
    }

    /// The person picked `reference` (or nothing): one, or with `add`
    /// added or taken out.
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
        self.ask_props();
    }

    /// The canvas moved its camera.
    pub fn moved(&mut self, cam: Camera) {
        if self.camera != Some(cam) {
            self.camera = Some(cam);
            self.view.due();
        }
    }

    /// What kind of owner a reference is on the shown sheet, or by its
    /// shape.
    pub fn kind_of(&self, reference: &str) -> String {
        if let Some(sc) = &self.scene {
            if sc.pins.iter().any(|p| p.reference == reference) {
                return "pin".into();
            }
            if let Some(i) = sc.owner(reference) {
                return sc.owners[i].kind.clone();
            }
        }
        // From a list: nets are listed by the navigator's net rows.
        let block = reference.split('/').next().unwrap_or_default();
        if self
            .nav
            .nets(block, "")
            .iter()
            .any(|n| n.reference == reference)
        {
            "net"
        } else {
            "component"
        }
        .into()
    }

    fn ask_props(&mut self) {
        let Some(r) = self.selection.last().cloned() else {
            self.props = None;
            return;
        };
        self.props = Some(Properties::new(&r));
        match lists::subject(&r, &self.kind_of(&r)) {
            Subject::Component(c) => {
                self.send(
                    req("component.show", json!({"component": c})),
                    Why::Component(r.clone()),
                );
            }
            Subject::Net(n) => {
                self.send(req("net.show", json!({"net": n})), Why::Net(r.clone()));
            }
            Subject::Other => {}
        }
        if let Some(block) = r.split('/').next() {
            self.send(
                req("layout.instructions", json!({"block": block})),
                Why::PropsInstructions(r.clone()),
            );
        }
    }

    /// Undoes the newest entry that can be, whoever made it: the person
    /// judges what the agent did (design 519, Pointing, not editing).
    /// `undo` without an entry would take only this window's own, and a
    /// window makes none.
    pub fn undo(&mut self) {
        let p = self
            .activity
            .iter()
            .find(|e| e.undo)
            .map_or_else(|| json!({}), |e| json!({"entry": e.seq}));
        self.send(req("undo", p), Why::Write("Undo"));
    }

    /// Redoes the newest undo not redone yet.
    pub fn redo(&mut self) {
        let p = self
            .activity
            .iter()
            .find(|e| e.redo)
            .map_or_else(|| json!({}), |e| json!({"entry": e.seq}));
        self.send(req("redo", p), Why::Write("Redo"));
    }

    pub fn run_check(&mut self) {
        self.checked = Some(false);
        self.send_aside(req("check", json!({"offline": true})), Why::Check);
    }

    /// Follows `who` (or stops): their sheet and camera as they move.
    pub fn follow(&mut self, who: Option<String>) {
        self.following = who;
        if let Some(p) = self
            .following
            .clone()
            .and_then(|id| self.people.get(&id).map(|p| p.view.clone()))
        {
            self.adopt(&p, false);
        }
    }

    /// The window's theme; the scene is asked again in it.
    pub fn set_dark(&mut self, dark: bool) {
        if self.dark != dark {
            self.dark = dark;
            self.flash.clear();
            self.ask_scene();
            self.ask_other_sheets();
        }
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

    /// A label's or port flag's link: the next sheet (in canvas order,
    /// after this one) that draws its net, framing the net there.
    pub fn follow_link(&mut self, net: &str) {
        let (Some(block), Some(here)) = (self.shown.block.clone(), self.shown.sheet.clone()) else {
            return;
        };
        let reach = self.nav.reach(&block, net);
        let sheets: Vec<String> = self
            .nav
            .sheets(&block)
            .iter()
            .map(|(s, _)| s.clone())
            .collect();
        let at = sheets.iter().position(|s| *s == here).unwrap_or(0);
        let next = (1..=sheets.len())
            .map(|k| &sheets[(at + k) % sheets.len()])
            .find(|s| **s != here && reach.contains(s));
        if let Some(s) = next.cloned() {
            self.frame_ref = Some(net.to_owned());
            let layer = self.shown.layer.clone();
            self.show(Some(block), Some(s), layer);
        }
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
        self.pending
            .values()
            .all(|w| matches!(w, Why::OtherSheet | Why::Check))
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
            Event::Lost if !self.lost => {
                self.lost = true;
                self.pending.clear();
                self.toasts.lost(Instant::now());
                self.retry = Some(Retry {
                    at: Instant::now() + RETRY_FIRST,
                    delay: RETRY_FIRST,
                    trying: None,
                });
            }
            Event::Lost => {}
        }
    }

    fn answered(&mut self, why: Why, a: Result<Value>) {
        let now = Instant::now();
        let v = match a {
            Ok(v) => v,
            Err(e) => {
                match why {
                    Why::Scene => self.scene_error = Some(e),
                    Why::Write(title) => self.toasts.error(title, &e, 0, now),
                    Why::Check => self.checked = None,
                    _ => {}
                }
                return;
            }
        };
        match why {
            Why::Scene => {
                let Some(sc) = Scene::from_json(&v) else {
                    return;
                };
                self.scene_error = None;
                let moved = self.scene.as_ref().is_none_or(|o| o.block != sc.block);
                if self.flash_on {
                    self.flash.arrived(self.scene.as_ref(), &sc, now);
                }
                self.nav.scene(&sc);
                self.findings.sheet(&sc.findings);
                if let Some(r) = self.frame_ref.take() {
                    let boxes = sc
                        .owners
                        .iter()
                        .filter(|o| o.reference == r || o.net.as_deref() == Some(r.as_str()))
                        .filter_map(|o| o.boxed);
                    self.frame = boxes.reduce(|a, b| {
                        [
                            [a[0][0].min(b[0][0]), a[0][1].min(b[0][1])],
                            [a[1][0].max(b[1][0]), a[1][1].max(b[1][1])],
                        ]
                    });
                }
                if self.shown.design.is_none() {
                    self.shown.design = sc.design.clone();
                }
                self.shown.block = Some(sc.block.clone());
                self.shown.sheet = Some(sc.sheet.clone());
                let block = sc.block.clone();
                self.scene = Some(sc);
                self.view.due();
                self.ask_other_sheets();
                if moved {
                    self.send(
                        req("layout.instructions", json!({"block": block})),
                        Why::Instructions(block.clone()),
                    );
                    self.send(req("block.show", json!({"block": block})), Why::Block);
                }
            }
            Why::OtherSheet => {
                if let Some(sc) = Scene::from_json(&v) {
                    self.nav.scene(&sc);
                }
            }
            Why::Designs => {
                self.nav.designs(&v);
                let ids: Vec<String> = self.nav.design_ids().map(str::to_owned).collect();
                for d in ids {
                    self.send(
                        req("netlist", json!({"design": d})),
                        Why::Netlist(d.clone()),
                    );
                }
            }
            Why::Netlist(d) => {
                self.nav.netlist(&d, &v);
                self.ask_blocks();
            }
            Why::Libcomps => self.nav.libcomps(&v),
            Why::Block => {
                self.nav.block(&v);
                self.ask_blocks();
            }
            Why::Rules => self.findings.rules(&v),
            Why::Erc => self.findings.erc(&v),
            Why::Check => {
                self.findings.check(&v);
                self.checked = Some(true);
            }
            Why::Instructions(b) => {
                if self.shown.block.as_deref() == Some(b.as_str()) {
                    self.instructions = lists::instructions(&v);
                }
            }
            Why::Activity => self.activity = lists::activity(&v),
            Why::Component(r) => {
                if let Some(p) = self.props.as_mut().filter(|p| p.reference == r) {
                    p.component(&v);
                    if let Some(l) = lists::libcomp_of(&v) {
                        self.send(req("libcomp.show", json!({"libcomp": l})), Why::Libcomp(r));
                    }
                }
            }
            Why::Libcomp(r) => {
                if let Some(p) = self.props.as_mut().filter(|p| p.reference == r) {
                    p.libcomp(&v);
                }
            }
            Why::Net(r) => {
                if let Some(p) = self.props.as_mut().filter(|p| p.reference == r) {
                    p.net(&v);
                }
            }
            Why::PropsInstructions(r) => {
                if let Some(p) = self.props.as_mut().filter(|p| p.reference == r) {
                    p.instructions(&v);
                }
            }
            Why::People => self.people.reset(&v["participants"]),
            Why::Write(_) | Why::Quiet => {}
        }
    }

    fn ask_blocks(&mut self) {
        if self.pending.values().any(|w| matches!(w, Why::Block)) {
            return;
        }
        for b in self.nav.wanted_blocks() {
            self.send(req("block.show", json!({"block": b})), Why::Block);
        }
    }

    fn noted(&mut self, method: &str, params: &Value) {
        let now = Instant::now();
        match method {
            "project.changed" => {
                let changed = &params["changed"];
                if self.scene.as_ref().is_some_and(|s| s.moved_by(changed)) {
                    self.ask_scene();
                }
                self.send(req("design.list", json!({})), Why::Designs);
                self.send(req("erc", json!({})), Why::Erc);
                if let Some(b) = self.shown.block.clone() {
                    self.send(
                        req("layout.instructions", json!({"block": b})),
                        Why::Instructions(b),
                    );
                }
                if changed
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|k| k.as_str().is_some_and(|k| k.starts_with("lib/")))
                {
                    self.send(req("libcomp.list", json!({})), Why::Libcomps);
                }
            }
            "participant.changed" => {
                self.people.changed(params);
                let me = self.me().to_owned();
                if let Some(mine) = self.people.get(&me).map(|p| p.view.clone())
                    && adopts(&mine, self.sent_view.as_ref(), &me, self.follow_shows)
                {
                    let selection = self
                        .people
                        .get(&me)
                        .map(|p| p.selection.clone())
                        .unwrap_or_default();
                    self.adopt(&mine, true);
                    self.selection = selection;
                    self.ask_props();
                }
                if let Some(f) = self.following.clone() {
                    match self.people.get(&f).map(|p| p.view.clone()) {
                        Some(v) => self.adopt(&v, false),
                        None => self.following = None,
                    }
                }
            }
            "activity.appended" => {
                let me = self.me().to_owned();
                self.toasts.activity(params, &me, now);
                self.send(
                    req("activity.log", json!({"limit": ACTIVITY})),
                    Why::Activity,
                );
            }
            "resync" => {
                self.send(req("participant.list", json!({})), Why::People);
                self.ask_all();
            }
            _ => {}
        }
    }

    /// Takes `v`'s sheet and camera; `frame` frames its camera's box as
    /// the show framed it.
    fn adopt(&mut self, v: &SheetView, mine: bool) {
        if v.block.is_none() {
            return;
        }
        let place_moved = self.shown.block != v.block
            || self.shown.sheet != v.sheet
            || self.shown.layer != v.layer;
        self.shown.design = v.design.clone().or(self.shown.design.take());
        self.shown.block = v.block.clone();
        self.shown.sheet = v.sheet.clone();
        self.shown.layer = v.layer.clone();
        self.camera = v.camera.map(from_nm);
        if mine {
            self.sent_view = Some(v.clone());
        }
        if place_moved {
            self.ask_scene();
        }
    }

    /// Sends what is due, expires toasts and flashes, and reconnects a
    /// lost link. Returns how soon it needs calling again, if it does.
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
        if self.select.ready(now) {
            let refs = self.selection.clone();
            self.send(req("selection.set", json!({"refs": refs})), Why::Quiet);
        }
        if self.view.ready(now) && self.following.is_none() {
            let v = self.view_now();
            let mut p = json!({});
            for (k, x) in [
                ("design", &v.design),
                ("block", &v.block),
                ("sheet", &v.sheet),
                ("layer", &v.layer),
            ] {
                if let Some(x) = x {
                    p[k] = json!(x);
                }
            }
            if let Some(c) = &self.camera {
                p["camera"] = to_nm(c);
            }
            self.sent_view = Some(v);
            self.send(req("view.set", p), Why::Quiet);
        }
        let flash = self.flash.due().then_some(Duration::from_millis(33));
        [self.select.wait(now), self.view.wait(now), flash]
            .into_iter()
            .flatten()
            .min()
    }

    fn view_now(&self) -> SheetView {
        SheetView {
            design: self.shown.design.clone(),
            block: self.shown.block.clone(),
            sheet: self.shown.sheet.clone(),
            layer: self.shown.layer.clone(),
            camera: self.camera.as_ref().map(|c| {
                let v = to_nm(c);
                (
                    [
                        v["centre_nm"][0].as_i64().unwrap_or(0),
                        v["centre_nm"][1].as_i64().unwrap_or(0),
                    ],
                    v["zoom_nm_per_pt"].as_i64().unwrap_or(1),
                )
            }),
            shown_by: Some(self.me().to_owned()),
        }
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
            retry.delay = (retry.delay * 2).min(RETRY_MAX);
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
    use std::time::Duration;

    #[test]
    fn every_request_the_window_sends_parses() {
        for (m, p) in [
            ("design.list", json!({})),
            ("netlist", json!({"design": "d"})),
            ("libcomp.list", json!({})),
            ("lint.get", json!({})),
            ("erc", json!({})),
            ("check", json!({"offline": true})),
            ("activity.log", json!({"limit": ACTIVITY})),
            ("layout.instructions", json!({"block": "top"})),
            ("block.show", json!({"block": "top"})),
            ("component.show", json!({"component": "top/r1"})),
            ("libcomp.show", json!({"libcomp": "r_100k"})),
            ("net.show", json!({"net": "top/sda"})),
            (
                "scene",
                json!({"block": "top", "sheet": "s1", "theme": "dark"}),
            ),
            ("selection.set", json!({"refs": ["top/r1"]})),
            (
                "view.set",
                json!({"block": "top", "sheet": "s1", "camera": {"centre_nm": [0, 0], "zoom_nm_per_pt": 10000}}),
            ),
            ("view.show", json!({"ref": "top/r1", "participant": "p-1"})),
            ("undo", json!({})),
            ("redo", json!({})),
            ("participant.list", json!({})),
        ] {
            assert!(Request::from_rpc(m, Some(p.clone())).is_ok(), "{m} {p}");
        }
    }

    #[test]
    fn cameras_turn_between_millimetres_and_nanometres() {
        let c = Camera {
            centre: [12.5, -3.0],
            zoom: 4.0,
        };
        let n = to_nm(&c);
        assert_eq!(
            n,
            json!({"centre_nm": [12_500_000, -3_000_000], "zoom_nm_per_pt": 250_000})
        );
        assert_eq!(from_nm(([12_500_000, -3_000_000], 250_000)), c);
    }

    #[test]
    fn a_throttle_sends_the_last_value_at_most_every_150_ms() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        assert!(!t.ready(t0), "nothing due");
        t.due();
        assert!(t.ready(t0));
        t.due();
        assert!(!t.ready(t0 + Duration::from_millis(100)));
        assert!(t.ready(t0 + SELECT_EVERY));
        assert_eq!(t.wait(t0 + SELECT_EVERY), None);
        t.due();
        assert_eq!(
            t.wait(t0 + SELECT_EVERY + Duration::from_millis(50)),
            Some(Duration::from_millis(100))
        );
    }

    #[test]
    fn a_window_adopts_shows_it_didn_t_send() {
        let sent = SheetView {
            block: Some("top".into()),
            sheet: Some("s1".into()),
            ..Default::default()
        };
        let mut shown = sent.clone();
        assert!(
            !adopts(&shown, Some(&sent), "me", true),
            "its own view comes back"
        );
        shown.sheet = Some("s2".into());
        shown.shown_by = Some("me".into());
        assert!(
            adopts(&shown, Some(&sent), "me", false),
            "its own Go to, always"
        );
        shown.shown_by = Some("agent".into());
        assert!(adopts(&shown, Some(&sent), "me", true));
        assert!(
            !adopts(&shown, Some(&sent), "me", false),
            "ui.follow_shows off"
        );
        let empty = SheetView::default();
        assert!(
            !adopts(&empty, Some(&sent), "me", true),
            "a view that shows nothing"
        );
    }
}
