//! Notifications to subscribed connections. Each subscriber has its own
//! queue, drained by its connection's writer thread, so a slow client never
//! holds up the daemon. Notifications carry state, not deltas: a client
//! that falls too far behind gets `resync` instead and fetches the state
//! again.

use crate::error::{Code, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// The most notifications a subscriber's queue holds before it is dropped
/// in favour of one `resync`.
pub const QUEUE_LIMIT: usize = 1000;

/// What a subscription can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// `model.changed`: rebuilt parts.
    Model,
    /// `project.changed`: the manifest, the parts, settings.
    Project,
    /// `participant.changed`: who is attached and what they do.
    Participants,
    /// `activity.appended`: the activity log.
    Activity,
    /// `merge.changed`: a merge in progress started, ended, or what it
    /// leaves to settle changed.
    Merge,
}

impl Topic {
    pub const ALL: [Topic; 5] = [
        Topic::Model,
        Topic::Project,
        Topic::Participants,
        Topic::Activity,
        Topic::Merge,
    ];
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// One JSON-RPC notification line, without the newline.
pub fn line(method: &str, params: &Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// A subscribed connection's queue.
pub struct Subscriber {
    pub participant: String,
    topics: Vec<Topic>,
    queue: Mutex<VecDeque<String>>,
    ready: Condvar,
    closed: AtomicBool,
}

impl Subscriber {
    fn push(&self, line: String) {
        let mut q = lock(&self.queue);
        if q.len() >= QUEUE_LIMIT {
            q.clear();
            q.push_back(self::line("resync", &json!({})));
        } else {
            q.push_back(line);
        }
        self.ready.notify_one();
    }

    /// The next line to send, waiting for one; `None` once closed.
    pub fn next(&self) -> Option<String> {
        let mut q = lock(&self.queue);
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            if let Some(l) = q.pop_front() {
                return Some(l);
            }
            q = self.ready.wait(q).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// Stops the subscription: the writer's `next` returns `None`.
    pub fn close(&self) {
        let _q = lock(&self.queue);
        self.closed.store(true, Ordering::SeqCst);
        self.ready.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Every line queued now, without waiting.
    #[cfg(test)]
    pub(crate) fn drain(&self) -> Vec<String> {
        lock(&self.queue).drain(..).collect()
    }
}

/// Every subscription of a daemon.
#[derive(Default)]
pub struct Hub {
    subscribers: Vec<Arc<Subscriber>>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hub({} subscribers)", self.subscribers.len())
    }
}

impl Hub {
    /// A new subscription for `participant` to `topics` (all when empty).
    pub fn subscribe(&mut self, participant: &str, topics: &[Topic]) -> Arc<Subscriber> {
        let topics = if topics.is_empty() {
            Topic::ALL.to_vec()
        } else {
            topics.to_vec()
        };
        let s = Arc::new(Subscriber {
            participant: participant.to_owned(),
            topics,
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            closed: AtomicBool::new(false),
        });
        self.subscribers.push(Arc::clone(&s));
        s
    }

    /// Queues a notification for every open subscription to `topic`.
    pub fn send(&mut self, topic: Topic, method: &str, params: &Value) {
        self.subscribers.retain(|s| !s.is_closed());
        let line = line(method, params);
        for s in &self.subscribers {
            if s.topics.contains(&topic) {
                s.push(line.clone());
            }
        }
    }

    /// Queues a notification for `participant`'s subscriptions, whatever
    /// their topics: a request meant for one app window.
    pub fn send_to(&mut self, participant: &str, method: &str, params: &Value) {
        self.subscribers.retain(|s| !s.is_closed());
        let line = line(method, params);
        for s in &self.subscribers {
            if s.participant == participant {
                s.push(line.clone());
            }
        }
    }

    /// Whether anyone is subscribed.
    pub fn is_watched(&mut self) -> bool {
        self.subscribers.retain(|s| !s.is_closed());
        !self.subscribers.is_empty()
    }
}

/// Parses `subscribe`'s topic names.
pub fn topics(names: &[String]) -> Result<Vec<Topic>> {
    names
        .iter()
        .map(|n| {
            serde_json::from_value(json!(n)).map_err(|_| {
                Error::new(
                    Code::InvalidParam,
                    format!(
                        "unknown topic {n:?}; use model, project, participants, activity or merge"
                    ),
                )
                .with("topic", n)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribers_get_their_topics_in_order() {
        let mut hub = Hub::default();
        let all = hub.subscribe("p-1", &[]);
        let some = hub.subscribe("p-2", &[Topic::Participants]);
        hub.send(Topic::Model, "model.changed", &json!({"n": 1}));
        hub.send(Topic::Participants, "participant.changed", &json!({"n": 2}));
        assert!(all.next().unwrap().contains("model.changed"));
        assert!(all.next().unwrap().contains("participant.changed"));
        assert!(some.next().unwrap().contains(r#""n":2"#));
        some.close();
        assert_eq!(some.next(), None);
        assert!(hub.is_watched());
        all.close();
        assert!(!hub.is_watched());
    }

    #[test]
    fn a_full_queue_becomes_one_resync() {
        let mut hub = Hub::default();
        let s = hub.subscribe("p-1", &[]);
        for i in 0..=QUEUE_LIMIT {
            hub.send(Topic::Model, "model.changed", &json!({"n": i}));
        }
        let first: Value = serde_json::from_str(&s.next().unwrap()).unwrap();
        assert_eq!(first["method"], "resync");
        hub.send(Topic::Model, "model.changed", &json!({"n": "after"}));
        assert!(s.next().unwrap().contains("after"));
    }

    #[test]
    fn a_waiting_writer_wakes_for_a_notification_or_a_close() {
        let mut hub = Hub::default();
        let s = hub.subscribe("p-1", &[]);
        let waiter = {
            let s = Arc::clone(&s);
            std::thread::spawn(move || (s.next(), s.next()))
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        hub.send(Topic::Activity, "activity.appended", &json!({}));
        std::thread::sleep(std::time::Duration::from_millis(50));
        s.close();
        let (a, b) = waiter.join().unwrap();
        assert!(a.unwrap().contains("activity.appended"));
        assert_eq!(b, None);
    }

    #[test]
    fn a_notification_for_one_participant() {
        let mut hub = Hub::default();
        let a = hub.subscribe("p-1", &[Topic::Model]);
        let b = hub.subscribe("p-2", &[]);
        hub.send_to("p-1", "app.open", &json!({}));
        hub.send(Topic::Participants, "participant.changed", &json!({}));
        assert!(a.next().unwrap().contains("app.open"));
        assert!(b.next().unwrap().contains("participant.changed"));
    }

    #[test]
    fn topic_names() {
        assert_eq!(
            topics(&["model".into(), "activity".into()]).unwrap(),
            [Topic::Model, Topic::Activity]
        );
        assert_eq!(
            topics(&["models".into()]).unwrap_err().code,
            Code::InvalidParam
        );
    }
}
