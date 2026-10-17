//! The live session: who is attached to a daemon, and what each of them is
//! looking at and doing. It lives only in the daemon's memory; the files
//! never hold it, and a `--no-daemon` run has none.

use crate::entity::EntityRef;
use crate::error::{Code, Error, Result};
use crate::notify::{Hub, Topic};
use crate::view::Camera;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
pub struct Highlight {
    pub style: Style,
    pub label: Option<String>,
    pub entities: Vec<EntityRef>,
}

/// What a participant says it is doing, and the entities it is working on.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Status {
    pub text: String,
    pub entities: Vec<EntityRef>,
}

/// One human (an app window) or agent attached to the session.
#[derive(Clone, Debug)]
pub struct Participant {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub status: Option<Status>,
    /// Selected entities, in the order they were selected.
    pub selection: Vec<EntityRef>,
    pub camera: Option<Camera>,
    pub highlights: Vec<Highlight>,
    connections: usize,
    last_seen: Instant,
}

impl Participant {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "name": self.name,
            "status": self.status,
            "selection": self.selection,
            "camera": self.camera,
            "highlights": self.highlights,
        })
    }
}

/// Everything the daemon knows about who is attached, and the
/// subscriptions it tells about changes.
#[derive(Debug, Default)]
pub struct Live {
    participants: BTreeMap<String, Participant>,
    next: u32,
    pub hub: Hub,
    /// What subscribers were last told about the model.
    pub seen: Seen,
    pub log: crate::activity::Log,
}

/// The project and part revisions subscribers were last told about.
#[derive(Debug, Default)]
pub struct Seen {
    pub project: Value,
    pub parts: BTreeMap<crate::doc::id::Id, String>,
}

/// Every participant's state, to compare before and after a change.
pub type Snapshot = BTreeMap<String, Value>;

impl Live {
    pub fn new() -> Live {
        Live::default()
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
                camera: None,
                highlights: Vec::new(),
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

    pub fn get(&self, id: &str) -> Result<&Participant> {
        self.participants.get(id).ok_or_else(|| no_participant(id))
    }

    pub fn get_mut(&mut self, id: &str) -> Result<&mut Participant> {
        self.participants
            .get_mut(id)
            .ok_or_else(|| no_participant(id))
    }

    /// The participant a request is about: `id` if given, otherwise the
    /// one human attached. `None` when no human is attached.
    pub fn target(&self, id: Option<&str>) -> Result<Option<&Participant>> {
        if let Some(id) = id {
            return self.get(id).map(Some);
        }
        let humans: Vec<&Participant> = self
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
        refs: Vec<EntityRef>,
        mode: SelectMode,
    ) -> Result<&[EntityRef]> {
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
        entities: Vec<EntityRef>,
    ) -> Result<&[Highlight]> {
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
    pub fn clear_highlights(&mut self, id: &str, style: Option<Style>) -> Result<&[Highlight]> {
        let p = self.get_mut(id)?;
        p.highlights.retain(|h| style.is_some_and(|s| s != h.style));
        Ok(&p.highlights)
    }

    /// Sets participant `id`'s status; an empty text clears it.
    pub fn set_status(&mut self, id: &str, text: &str, entities: Vec<EntityRef>) -> Result<()> {
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
    pub fn prune(&mut self, mut keep: impl FnMut(&EntityRef) -> bool) -> Vec<String> {
        let mut changed = Vec::new();
        for p in self.participants.values_mut() {
            let mut gone = 0;
            let mut retain = |v: &mut Vec<EntityRef>| {
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

    pub fn participants(&self) -> impl Iterator<Item = &Participant> {
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
                    Topic::Participants,
                    "participant.changed",
                    &json!({ "participant": v }),
                );
            }
        }
        for id in before.keys().filter(|id| !after.contains_key(*id)) {
            self.hub.send(
                Topic::Participants,
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

    fn r(name: char) -> EntityRef {
        EntityRef::parse(&format!("p/f/{}", name.to_string().repeat(32))).unwrap()
    }

    #[test]
    fn selections_replace_add_and_remove() {
        let mut live = Live::new();
        let a = live.caller(None);
        assert_eq!(live.get(&a).unwrap().name, DEFAULT_AGENT);
        let sel = |l: &mut Live, refs: Vec<EntityRef>, m| l.select(&a, refs, m).unwrap().to_vec();
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
        assert_eq!(sel(&mut live, vec![], SelectMode::Replace), []);
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
