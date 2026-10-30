//! Notifications to subscribed connections. Each subscriber has its own
//! queue, drained by its connection's writer thread, so a slow client never
//! holds up the daemon. Notifications carry state, not deltas: a client
//! that falls too far behind gets `resync` instead and fetches the state
//! again.

use ryu_doc::error::{Code, Error, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// The most notifications a subscriber's queue holds before it is dropped
/// in favour of one `resync`.
pub const QUEUE_LIMIT: usize = 1000;

/// What a subscription can ask for: a product's topics, by their names
/// in `subscribe` (serde's).
pub trait Topics: Copy + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Every topic, in the order `subscribe` reports them.
    const ALL: &'static [Self];
    /// `participant.changed`: who is attached and what they do.
    const PARTICIPANTS: Self;
    /// `activity.appended`: the activity log.
    const ACTIVITY: Self;
    /// `progress`: long requests' progress, any participant's; `None`
    /// for a product with no long requests.
    const PROGRESS: Option<Self> = None;
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// One JSON-RPC notification line, without the newline.
pub fn line(method: &str, params: &Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// A subscribed connection's queue.
pub struct Subscriber<T> {
    pub participant: String,
    topics: Vec<T>,
    queue: Mutex<VecDeque<String>>,
    ready: Condvar,
    closed: AtomicBool,
}

impl<T> Subscriber<T> {
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

    /// Queues `line` for this subscriber: a request's progress, sent
    /// from outside the hub while the request holds the daemon's lock.
    pub fn send(&self, line: String) {
        if !self.is_closed() {
            self.push(line);
        }
    }

    /// Whether the subscription asked for `topic`.
    pub fn wants(&self, topic: T) -> bool
    where
        T: PartialEq,
    {
        self.topics.contains(&topic)
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

    /// Every line queued now, without waiting: for tests.
    #[doc(hidden)]
    pub fn drain(&self) -> Vec<String> {
        lock(&self.queue).drain(..).collect()
    }
}

/// Every subscription of a daemon.
pub struct Hub<T> {
    subscribers: Vec<Arc<Subscriber<T>>>,
}

impl<T> Default for Hub<T> {
    fn default() -> Self {
        Hub {
            subscribers: Vec::new(),
        }
    }
}

impl<T> std::fmt::Debug for Hub<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hub({} subscribers)", self.subscribers.len())
    }
}

impl<T: Topics> Hub<T> {
    /// A new subscription for `participant` to `topics` (all when empty).
    pub fn subscribe(&mut self, participant: &str, topics: &[T]) -> Arc<Subscriber<T>> {
        let topics = if topics.is_empty() {
            T::ALL.to_vec()
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
    pub fn send(&mut self, topic: T, method: &str, params: &Value) {
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
pub fn topics<T: Topics>(names: &[String]) -> Result<Vec<T>> {
    names
        .iter()
        .map(|n| {
            serde_json::from_value(json!(n)).map_err(|_| {
                Error::new(
                    Code::InvalidParam,
                    format!("unknown topic {n:?}; use {}", known::<T>()),
                )
                .with("topic", n)
            })
        })
        .collect()
}

/// Every topic's name: `model, project, participants, activity, merge or
/// pdm`.
fn known<T: Topics>() -> String {
    let names: Vec<String> = T::ALL
        .iter()
        .map(|t| match json!(t) {
            Value::String(s) => s,
            v => v.to_string(),
        })
        .collect();
    match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Topic;

    #[test]
    fn subscribers_get_their_topics_in_order() {
        let mut hub = Hub::<Topic>::default();
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
        let mut hub = Hub::<Topic>::default();
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
        let mut hub = Hub::<Topic>::default();
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
        let mut hub = Hub::<Topic>::default();
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
            topics::<Topic>(&["model".into(), "activity".into()]).unwrap(),
            [Topic::Model, Topic::Activity]
        );
        let e = topics::<Topic>(&["models".into()]).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(
            e.message,
            "unknown topic \"models\"; use model, project, participants, activity, merge, pdm or progress"
        );
    }
}
