//! The live session: who is attached to a daemon, and what each of them is
//! looking at and doing. It lives only in the daemon's memory; the files
//! never hold it, and a `--no-daemon` run has none. What a participant
//! points at (a product's entity references) and what else its window
//! shows (a camera, a compare) are the product's ([`Product::Target`],
//! [`Product::View`]).

use crate::Product;
use crate::notify::{Hub, Topics};
use ryu_doc::error::{Code, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How long an agent participant with no open connection is kept: CLI
/// calls are separate connections, so an agent's selection and highlights
/// must outlive each one.
pub const AGENT_EXPIRY: Duration = Duration::from_secs(10 * 60);

/// The participant a CLI call acts as when it doesn't name one.
pub const DEFAULT_AGENT: &str = "agent";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Human,
    Agent,
}

/// How `selection.set` combines the given entities with the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectMode {
    #[default]
    Replace,
    Add,
    Remove,
}

/// The longest status line.
pub const STATUS_MAX: usize = 200;

/// How a highlight is drawn: entities about to change, entities just
/// changed, or entities pointed out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    Pending,
    Changed,
    #[default]
    Note,
}

/// A group of highlighted entities with one style and label.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Highlight<T> {
    pub style: Style,
    pub label: Option<String>,
    pub entities: Vec<T>,
}

/// What a participant says it is doing, and the entities it is working on.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Status<T> {
    pub text: String,
    pub entities: Vec<T>,
}

/// One human (an app window) or agent attached to the session.
pub struct Participant<P: Product> {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub status: Option<Status<P::Target>>,
    /// Selected entities, in the order they were selected.
    pub selection: Vec<P::Target>,
    pub highlights: Vec<Highlight<P::Target>>,
    /// What else the participant's window shows, the product's.
    pub view: P::View,
    connections: usize,
    last_seen: Instant,
}

impl<P: Product> Clone for Participant<P> {
    fn clone(&self) -> Self {
        Participant {
            id: self.id.clone(),
            kind: self.kind,
            name: self.name.clone(),
            status: self.status.clone(),
            selection: self.selection.clone(),
            highlights: self.highlights.clone(),
            view: self.view.clone(),
            connections: self.connections,
            last_seen: self.last_seen,
        }
    }
}

impl<P: Product> std::fmt::Debug for Participant<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Participant")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("name", &self.name)
            .field("status", &self.status)
            .field("selection", &self.selection)
            .field("highlights", &self.highlights)
            .field("view", &self.view)
            .finish()
    }
}

impl<P: Product> Participant<P> {
    pub fn to_json(&self) -> Value {
        let mut v = Map::new();
        v.insert("id".into(), json!(self.id));
        v.insert("kind".into(), json!(self.kind));
        v.insert("name".into(), json!(self.name));
        v.insert("status".into(), json!(self.status));
        v.insert("selection".into(), json!(self.selection));
        v.insert("highlights".into(), json!(self.highlights));
        P::view_json(&self.view, &mut v);
        Value::Object(v)
    }
}

/// Everything the daemon knows about who is attached, and the
/// subscriptions it tells about changes.
pub struct Live<P: Product> {
    participants: BTreeMap<String, Participant<P>>,
    next: u32,
    pub hub: Hub<P::Topic>,
    /// What subscribers were last told about, the product's.
    pub seen: P::Seen,
    pub log: crate::activity::Log,
    /// How often a watched daemon checks the files, in milliseconds.
    pub poll_ms: u64,
}

impl<P: Product> Default for Live<P> {
    fn default() -> Self {
        Live {
            participants: BTreeMap::new(),
            next: 0,
            hub: Hub::default(),
            seen: P::Seen::default(),
            log: crate::activity::Log::with_budgets(
                crate::activity::MAX_BYTES,
                crate::activity::MAX_BULKY_BYTES,
                P::BULKY,
            ),
            poll_ms: 0,
        }
    }
}

impl<P: Product> std::fmt::Debug for Live<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live")
            .field("participants", &self.participants)
            .field("next", &self.next)
            .field("hub", &self.hub)
            .field("seen", &self.seen)
            .field("log", &self.log)
            .field("poll_ms", &self.poll_ms)
            .finish()
    }
}

/// Every participant's state, to compare before and after a change.
pub type Snapshot = BTreeMap<String, Value>;

impl<P: Product> Live<P> {
    pub fn new() -> Live<P> {
        Live::with_poll(250)
    }

    /// A live session whose daemon checks the files every `poll_ms`.
    pub fn with_poll(poll_ms: u64) -> Live<P> {
        Live {
            poll_ms,
            ..Live::default()
        }
    }

    /// Registers a connection. With `participant`, the connection joins that
    /// participant (an app's second connection). Otherwise an agent joins
    /// the agent of the same name, created if needed, and a human is always
    /// a new participant. Returns the participant's ID.
    pub fn hello(
        &mut self,
        kind: Kind,
        name: Option<&str>,
        participant: Option<&str>,
    ) -> Result<String> {
        if let Some(id) = participant {
            let p = self.get_mut(id)?;
            if p.kind != kind {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("participant {id} is not a {kind:?} participant"),
                )
                .with("participant", id));
            }
            p.connections += 1;
            p.last_seen = Instant::now();
            return Ok(id.to_owned());
        }
        if let Some(n) = name
            && (n.trim().is_empty() || n.chars().count() > 64)
        {
            return Err(Error::new(
                Code::InvalidParam,
                "a participant name is 1 to 64 characters",
            )
            .with("name", n));
        }
        let id = match kind {
            Kind::Agent => self.agent(name.unwrap_or(DEFAULT_AGENT)),
            Kind::Human => {
                let name = match name {
                    Some(n) => n.to_owned(),
                    None => format!("human {}", self.count(Kind::Human) + 1),
                };
                self.create(Kind::Human, name)
            }
        };
        let p = self.participants.get_mut(&id).expect("just made");
        p.connections += 1;
        Ok(id)
    }

    /// The agent participant called `name`, created if needed, without
    /// attaching a connection.
    pub fn agent(&mut self, name: &str) -> String {
        let found = self
            .participants
            .values()
            .find(|p| p.kind == Kind::Agent && p.name == name)
            .map(|p| p.id.clone());
        let id = found.unwrap_or_else(|| self.create(Kind::Agent, name.to_owned()));
        self.participants.get_mut(&id).expect("exists").last_seen = Instant::now();
        id
    }

    fn create(&mut self, kind: Kind, name: String) -> String {
        self.next += 1;
        let id = format!("p-{:08x}", self.next);
        self.participants.insert(
            id.clone(),
            Participant {
                id: id.clone(),
                kind,
                name,
                status: None,
                selection: Vec::new(),
                highlights: Vec::new(),
                view: P::View::default(),
                connections: 0,
                last_seen: Instant::now(),
            },
        );
        id
    }

    fn count(&self, kind: Kind) -> usize {
        self.participants
            .values()
            .filter(|p| p.kind == kind)
            .count()
    }

    /// A connection of participant `id` closed. A human leaves with its last
    /// connection; an agent stays until it expires. Returns whether the
    /// participant left.
    pub fn goodbye(&mut self, id: &str) -> bool {
        let Some(p) = self.participants.get_mut(id) else {
            return false;
        };
        p.connections = p.connections.saturating_sub(1);
        p.last_seen = Instant::now();
        if p.kind == Kind::Human && p.connections == 0 {
            self.participants.remove(id);
            return true;
        }
        false
    }

    /// Notes that participant `id` just made a request.
    pub fn touch(&mut self, id: &str) {
        if let Some(p) = self.participants.get_mut(id) {
            p.last_seen = Instant::now();
        }
    }

    /// Removes agents idle longer than `AGENT_EXPIRY` with no connection
    /// open. Returns the IDs removed.
    pub fn expire(&mut self, now: Instant) -> Vec<String> {
        let gone: Vec<String> = self
            .participants
            .values()
            .filter(|p| {
                p.kind == Kind::Agent
                    && p.connections == 0
                    && now.saturating_duration_since(p.last_seen) >= AGENT_EXPIRY
            })
            .map(|p| p.id.clone())
            .collect();
        for id in &gone {
            self.participants.remove(id);
        }
        gone
    }

    /// Participant `id`, or the one named `id` (`--of agent`, `--of "dana
    /// (window 1)"`): an ID first, then a name only one participant has.
    pub fn get(&self, id: &str) -> Result<&Participant<P>> {
        let id = self.resolve(id)?;
        Ok(&self.participants[&id])
    }

    pub fn get_mut(&mut self, id: &str) -> Result<&mut Participant<P>> {
        let id = self.resolve(id)?;
        Ok(self.participants.get_mut(&id).expect("just resolved"))
    }

    fn resolve(&self, id: &str) -> Result<String> {
        if self.participants.contains_key(id) {
            return Ok(id.to_owned());
        }
        let named: Vec<&Participant<P>> = self
            .participants
            .values()
            .filter(|p| p.name == id)
            .collect();
        match named.as_slice() {
            [] => Err(no_participant(id)),
            [one] => Ok(one.id.clone()),
            many => Err(Error::new(
                Code::AmbiguousParticipant,
                format!(
                    "{} participants are named {id}; name one by its ID",
                    many.len()
                ),
            )
            .with(
                "participants",
                many.iter().map(|p| p.to_json()).collect::<Vec<_>>(),
            )),
        }
    }

    /// The participant a request is about: `id` if given, otherwise the
    /// one human attached. `None` when no human is attached.
    pub fn target(&self, id: Option<&str>) -> Result<Option<&Participant<P>>> {
        if let Some(id) = id {
            return self.get(id).map(Some);
        }
        let humans: Vec<&Participant<P>> = self
            .participants
            .values()
            .filter(|p| p.kind == Kind::Human)
            .collect();
        match humans.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some(one)),
            many => Err(Error::new(
                Code::AmbiguousParticipant,
                format!(
                    "{} humans are attached; name one with --of or `participant`",
                    many.len()
                ),
            )
            .with(
                "participants",
                many.iter().map(|p| p.to_json()).collect::<Vec<_>>(),
            )),
        }
    }

    /// The humans attached: the app windows.
    pub fn humans(&self) -> impl Iterator<Item = &Participant<P>> {
        self.participants.values().filter(|p| p.kind == Kind::Human)
    }

    /// The participant a request acts as: `me` if its connection said
    /// hello, otherwise the default agent.
    pub fn caller(&mut self, me: Option<&str>) -> String {
        match me {
            Some(id) if self.participants.contains_key(id) => id.to_owned(),
            _ => self.agent(DEFAULT_AGENT),
        }
    }

    /// Changes participant `id`'s selection; returns the new one.
    pub fn select(
        &mut self,
        id: &str,
        refs: Vec<P::Target>,
        mode: SelectMode,
    ) -> Result<&[P::Target]> {
        let p = self.get_mut(id)?;
        match mode {
            SelectMode::Replace => p.selection.clear(),
            SelectMode::Add => {}
            SelectMode::Remove => {
                p.selection.retain(|r| !refs.contains(r));
                return Ok(&p.selection);
            }
        }
        for r in refs {
            if !p.selection.contains(&r) {
                p.selection.push(r);
            }
        }
        Ok(&p.selection)
    }

    /// Replaces participant `id`'s highlight with the same style and label.
    /// No entities removes it.
    pub fn highlight(
        &mut self,
        id: &str,
        style: Style,
        label: Option<String>,
        entities: Vec<P::Target>,
    ) -> Result<&[Highlight<P::Target>]> {
        let p = self.get_mut(id)?;
        p.highlights
            .retain(|h| !(h.style == style && h.label == label));
        if !entities.is_empty() {
            p.highlights.push(Highlight {
                style,
                label,
                entities,
            });
        }
        Ok(&p.highlights)
    }

    /// Removes participant `id`'s highlights of `style`, or all of them.
    pub fn clear_highlights(
        &mut self,
        id: &str,
        style: Option<Style>,
    ) -> Result<&[Highlight<P::Target>]> {
        let p = self.get_mut(id)?;
        p.highlights.retain(|h| style.is_some_and(|s| s != h.style));
        Ok(&p.highlights)
    }

    /// The participant whose view a request that shows something changes
    /// (ryucad's `compare.set`): `id` if given, else the one human (an
    /// agent shows a person what it did), else, with no human attached,
    /// the caller `me`.
    pub fn compare_target(&mut self, id: Option<&str>, me: Option<&str>) -> Result<String> {
        if let Some(id) = id {
            return Ok(self.get(id)?.id.clone());
        }
        match self.target(None)? {
            Some(h) => Ok(h.id.clone()),
            None => Ok(self.caller(me)),
        }
    }

    /// The participant whose view a render follows by default (ryucad's
    /// hidden set): `id` if given, else the one human (whose view an
    /// agent's request changes; issue #137), else the caller `me`, as also
    /// when several humans are attached.
    pub fn view_target(&mut self, id: Option<&str>, me: Option<&str>) -> Result<String> {
        if let Some(id) = id {
            return Ok(self.get(id)?.id.clone());
        }
        match self.target(None) {
            Ok(Some(h)) => Ok(h.id.clone()),
            _ => Ok(self.caller(me)),
        }
    }

    /// Sets participant `id`'s status; an empty text clears it.
    pub fn set_status(&mut self, id: &str, text: &str, entities: Vec<P::Target>) -> Result<()> {
        if text.chars().count() > STATUS_MAX {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a status is at most {STATUS_MAX} characters"),
            ));
        }
        let p = self.get_mut(id)?;
        p.status = (!text.is_empty()).then(|| Status {
            text: text.to_owned(),
            entities,
        });
        Ok(())
    }

    /// Drops every reference `keep` refuses from selections, highlights and
    /// statuses. Returns the IDs of participants whose state changed.
    pub fn prune(&mut self, mut keep: impl FnMut(&P::Target) -> bool) -> Vec<String> {
        let mut changed = Vec::new();
        for p in self.participants.values_mut() {
            let mut gone = 0;
            let mut retain = |v: &mut Vec<P::Target>| {
                let before = v.len();
                v.retain(&mut keep);
                gone += before - v.len();
            };
            retain(&mut p.selection);
            for h in &mut p.highlights {
                retain(&mut h.entities);
            }
            if let Some(s) = &mut p.status {
                retain(&mut s.entities);
            }
            p.highlights.retain(|h| !h.entities.is_empty());
            if gone > 0 {
                changed.push(p.id.clone());
            }
        }
        changed
    }

    pub fn participants(&self) -> impl Iterator<Item = &Participant<P>> {
        self.participants.values()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.participants
            .iter()
            .map(|(id, p)| (id.clone(), p.to_json()))
            .collect()
    }

    /// Notifies subscribers of every participant that changed, arrived or
    /// left since `before`.
    pub fn announce(&mut self, before: &Snapshot) {
        let after = self.snapshot();
        for (id, v) in &after {
            if before.get(id) != Some(v) {
                self.hub.send(
                    P::Topic::PARTICIPANTS,
                    "participant.changed",
                    &json!({ "participant": v }),
                );
            }
        }
        for id in before.keys().filter(|id| !after.contains_key(*id)) {
            self.hub.send(
                P::Topic::PARTICIPANTS,
                "participant.changed",
                &json!({"participant": {"id": id}, "left": true}),
            );
        }
    }

    pub fn list(&self) -> Value {
        json!({"participants": self.participants().map(Participant::to_json).collect::<Vec<_>>()})
    }
}

fn no_participant(id: &str) -> Error {
    Error::new(Code::NotFound, format!("no participant {id}")).with("participant", id)
}

/// The error for session requests in a run with no daemon.
pub fn no_live_session(method: &str) -> Error {
    Error::new(
        Code::NoLiveSession,
        format!("{method} needs the project's daemon; drop --no-daemon"),
    )
    .with("method", method)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Test, Topic};

    type Live = super::Live<Test>;

    #[test]
    fn agents_are_reused_by_name_and_humans_are_not() {
        let mut live = Live::new();
        let a = live.hello(Kind::Agent, None, None).unwrap();
        let b = live.hello(Kind::Agent, Some("agent"), None).unwrap();
        let c = live.hello(Kind::Agent, Some("reviewer"), None).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        let h1 = live.hello(Kind::Human, None, None).unwrap();
        let h2 = live.hello(Kind::Human, None, None).unwrap();
        assert_ne!(h1, h2);
        assert_eq!(live.get(&h2).unwrap().name, "human 2");
        assert_eq!(live.participants().count(), 4);
        let e = live.hello(Kind::Agent, Some(" "), None).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn participants_are_named_by_id_or_name() {
        let mut live = Live::new();
        let a = live.hello(Kind::Agent, None, None).unwrap();
        let r = live.hello(Kind::Agent, Some("reviewer"), None).unwrap();
        assert_eq!(live.get("agent").unwrap().id, a);
        assert_eq!(live.get_mut("reviewer").unwrap().id, r);
        assert_eq!(live.target(Some("reviewer")).unwrap().unwrap().id, r);
        assert_eq!(live.get("nobody").unwrap_err().code, Code::NotFound);
        let h1 = live.hello(Kind::Human, Some("dana"), None).unwrap();
        let h2 = live.hello(Kind::Human, Some("dana"), None).unwrap();
        assert_eq!(live.get(&h2).unwrap().id, h2);
        assert_ne!(h1, h2);
        assert_eq!(
            live.get("dana").unwrap_err().code,
            Code::AmbiguousParticipant
        );
    }

    #[test]
    fn a_second_connection_joins_its_participant() {
        let mut live = Live::new();
        let h = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        assert_eq!(live.hello(Kind::Human, None, Some(&h)).unwrap(), h);
        let e = live.hello(Kind::Agent, None, Some(&h)).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(
            live.hello(Kind::Human, None, Some("p-ffffffff"))
                .unwrap_err()
                .code,
            Code::NotFound
        );
        // Two connections: the human stays until both close.
        assert!(!live.goodbye(&h));
        assert!(live.goodbye(&h));
        assert!(live.get(&h).is_err());
    }

    #[test]
    fn agents_outlive_their_connections_until_they_expire() {
        let mut live = Live::new();
        let a = live.hello(Kind::Agent, None, None).unwrap();
        assert!(!live.goodbye(&a));
        let now = Instant::now();
        assert!(live.expire(now).is_empty());
        assert_eq!(live.expire(now + AGENT_EXPIRY), vec![a.clone()]);
        assert!(live.get(&a).is_err());
    }

    fn r(name: char) -> String {
        format!("p/f/{}", name.to_string().repeat(32))
    }

    #[test]
    fn selections_replace_add_and_remove() {
        let mut live = Live::new();
        let a = live.caller(None);
        assert_eq!(live.get(&a).unwrap().name, DEFAULT_AGENT);
        let sel = |l: &mut Live, refs: Vec<String>, m| l.select(&a, refs, m).unwrap().to_vec();
        assert_eq!(
            sel(&mut live, vec![r('a'), r('b')], SelectMode::Replace),
            [r('a'), r('b')]
        );
        assert_eq!(
            sel(&mut live, vec![r('b'), r('c')], SelectMode::Add),
            [r('a'), r('b'), r('c')]
        );
        assert_eq!(
            sel(&mut live, vec![r('a')], SelectMode::Remove),
            [r('b'), r('c')]
        );
        assert!(sel(&mut live, vec![], SelectMode::Replace).is_empty());
        sel(&mut live, vec![r('a'), r('b')], SelectMode::Replace);
        assert_eq!(live.prune(|x| *x != r('a')), vec![a.clone()]);
        assert_eq!(live.get(&a).unwrap().selection, [r('b')]);
        assert!(live.prune(|_| true).is_empty());
    }

    #[test]
    fn highlights_are_replaced_by_style_and_label() {
        let mut live = Live::new();
        let a = live.caller(None);
        live.highlight(&a, Style::Note, None, vec![r('a')]).unwrap();
        live.highlight(&a, Style::Note, Some("hole".into()), vec![r('b')])
            .unwrap();
        let h = live.highlight(&a, Style::Note, None, vec![r('c')]).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[1].entities, [r('c')]);
        live.highlight(&a, Style::Pending, None, vec![r('a')])
            .unwrap();
        let h = live.clear_highlights(&a, Some(Style::Note)).unwrap();
        assert_eq!(
            h.iter().map(|h| h.style).collect::<Vec<_>>(),
            [Style::Pending]
        );
        // Pruning empties the group, which then goes.
        live.set_status(&a, "filleting", vec![r('a'), r('b')])
            .unwrap();
        assert_eq!(live.prune(|x| *x != r('a')), vec![a.clone()]);
        let p = live.get(&a).unwrap();
        assert!(p.highlights.is_empty());
        assert_eq!(p.status.as_ref().unwrap().entities, [r('b')]);
        live.set_status(&a, "", vec![]).unwrap();
        assert!(live.get(&a).unwrap().status.is_none());
        let long = "x".repeat(STATUS_MAX + 1);
        assert_eq!(
            live.set_status(&a, &long, vec![]).unwrap_err().code,
            Code::InvalidParam
        );
        assert!(live.clear_highlights(&a, None).unwrap().is_empty());
    }

    #[test]
    fn changes_are_announced() {
        let mut live = Live::new();
        let sub = live.hub.subscribe("p-x", &[Topic::Participants]);
        let before = live.snapshot();
        let h = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        live.announce(&before);
        let v: Value = serde_json::from_str(&sub.next().unwrap()).unwrap();
        assert_eq!(v["params"]["participant"]["name"], "Dana");
        // Nothing visible changed: nothing is sent.
        let before = live.snapshot();
        live.touch(&h);
        live.announce(&before);
        live.goodbye(&h);
        live.announce(&before);
        let v: Value = serde_json::from_str(&sub.next().unwrap()).unwrap();
        assert_eq!(v["params"], json!({"participant": {"id": h}, "left": true}));
    }

    #[test]
    fn the_target_is_the_one_human() {
        let mut live = Live::new();
        assert!(live.target(None).unwrap().is_none());
        live.hello(Kind::Agent, None, None).unwrap();
        assert!(live.target(None).unwrap().is_none());
        let h = live.hello(Kind::Human, None, None).unwrap();
        assert_eq!(live.target(None).unwrap().unwrap().id, h);
        live.hello(Kind::Human, None, None).unwrap();
        let e = live.target(None).unwrap_err();
        assert_eq!(e.code, Code::AmbiguousParticipant);
        assert_eq!(e.details["participants"].as_array().unwrap().len(), 2);
        assert_eq!(live.target(Some(&h)).unwrap().unwrap().id, h);
    }
}
