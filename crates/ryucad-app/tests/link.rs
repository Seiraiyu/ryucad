//! The app's link to a real daemon.

mod common;

use common::{Project, ryucad};
use ryucad::proto::Request;
use ryucad_app::core::link::{Event, Link, check_build};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(20);

fn open(p: &Project) -> (Link, Arc<AtomicUsize>) {
    let woken = Arc::new(AtomicUsize::new(0));
    let w = Arc::clone(&woken);
    let wake = Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    });
    (Link::open(&p.root, ryucad(), "Dana", wake).unwrap(), woken)
}

/// Events until `f` picks one out.
fn until<T>(link: &Link, mut f: impl FnMut(Event) -> Option<T>) -> T {
    let t = Instant::now();
    loop {
        let left = WAIT.saturating_sub(t.elapsed());
        let e = link
            .wait_event(left)
            .expect("timed out waiting for an event");
        if let Some(v) = f(e) {
            return v;
        }
    }
}

#[test]
fn the_window_hears_what_agents_do() {
    let p = Project::new().with_part();
    let (mut link, woken) = open(&p);
    assert!(link.participant.starts_with("p-"));
    let names: Vec<&str> = link.session["participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Dana"), "{names:?}");
    p.ok(&["feature", "edit", "p", "boss", "--param", "radius=4"]);
    let changed = until(&link, |e| match e {
        Event::Note { method, params } if method == "model.changed" => Some(params),
        _ => None,
    });
    assert_eq!(changed["parts"][0]["part"], "p");
    let ticket = link.send(Request::PartList {});
    let parts = until(&link, |e| match e {
        Event::Answer { ticket: t, answer } if t == ticket => Some(answer.unwrap().result),
        _ => None,
    });
    assert_eq!(parts["parts"][0]["part"], "p");
    assert!(woken.load(Ordering::SeqCst) >= 2);
}

#[test]
fn losing_the_daemon_is_an_event() {
    let p = Project::new();
    let (link, _) = open(&p);
    p.ok(&["daemon", "stop"]);
    until(&link, |e| (e == Event::Lost).then_some(()));
}

#[test]
fn a_ryucad_of_another_build_is_refused() {
    let mut other = ryucad().clone();
    assert!(check_build(&other).is_ok());
    other.version.commit = "0123456789ab".into();
    let e = check_build(&other).unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::Daemon);
    assert_eq!(e.details["ryucad"]["commit"], "0123456789ab");
}

/// A request sent aside is answered while one on the request connection
/// still waits, as the same participant.
#[test]
fn a_request_sent_aside_doesnt_wait_behind_the_other() {
    let p = Project::new().with_part();
    let (mut link, _) = open(&p);
    let check: Request =
        serde_json::from_value(serde_json::json!({"method": "check", "params": {}})).unwrap();
    let slow = link.send(check);
    let quick = link.send_aside(Request::ParticipantList {}, Default::default());
    let mut order = Vec::new();
    while order.len() < 2 {
        let (ticket, answer) = until(&link, |e| match e {
            Event::Answer { ticket, answer } => Some((ticket, answer)),
            Event::Lost => panic!("lost"),
            _ => None,
        });
        if ticket == quick {
            // Said hello as the window's own participant: one human, not
            // a second one.
            let who = answer.unwrap().result;
            let humans: Vec<_> = who["participants"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| p["kind"] == "human")
                .collect();
            assert_eq!(humans.len(), 1, "{who}");
            assert_eq!(humans[0]["id"], link.participant.as_str(), "{who}");
        }
        order.push(ticket);
    }
    assert_ne!(slow, quick);
    assert!(order.contains(&slow) && order.contains(&quick));
}
