//! The `ryucad review` client against a stand-in service.

mod fake_review;

use fake_review::{Fake, json};
use ryucad::error::Code;
use ryucad::review::Client;
use serde_json::json as j;

#[test]
fn gets_and_posts_with_the_token() {
    let fake = Fake::start(vec![
        json(
            "GET",
            "/api/v1/repos/acme/widget/pulls/7",
            200,
            j!({"number": 7}),
        ),
        json("POST", "/api/v1/threads/t1/comments", 201, j!({"id": "c1"})),
    ]);
    let c = Client::new(&fake.url, "tok");
    assert_eq!(
        c.get("/repos/acme/widget/pulls/7").unwrap(),
        j!({"number": 7})
    );
    assert_eq!(
        c.post("/threads/t1/comments", &j!({"body": "hi"})).unwrap(),
        j!({"id": "c1"})
    );
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(seen[0].auth.as_deref(), Some("Bearer tok"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&seen[1].body).unwrap(),
        j!({"body": "hi"})
    );
}

#[test]
fn refusals_keep_their_codes() {
    let fake = Fake::start(vec![
        json(
            "POST",
            "/api/v1/repos/a/b/pulls/1/merge-in",
            409,
            j!({"error": {"code": "head_moved", "message": "the head moved", "details": {"head": "abc"}}}),
        ),
        json(
            "GET",
            "/api/v1/x",
            401,
            j!({"error": {"code": "unauthorized", "message": "bad token"}}),
        ),
        json(
            "GET",
            "/api/v1/y",
            500,
            j!({"error": {"code": "internal", "message": "oops"}}),
        ),
        json("GET", "/api/v1/z", 202, j!({"state": "queued"})),
    ]);
    let c = Client::new(&fake.url, "tok");
    let e = c.post("/repos/a/b/pulls/1/merge-in", &j!({})).unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (Code::ReviewHeadMoved, "the head moved")
    );
    assert_eq!(e.details["service"], j!({"head": "abc"}));
    assert_eq!(e.details["status"], 409);
    assert_eq!(c.get("/x").unwrap_err().code, Code::ReviewUnauthenticated);
    let e = c.get("/y").unwrap_err();
    assert_eq!(
        (e.code, &e.details["service_code"]),
        (Code::ReviewFailed, &j!("internal"))
    );
    assert_eq!(c.get("/z").unwrap_err().code, Code::ReviewPending);
    assert_eq!(c.get("/nowhere").unwrap_err().code, Code::ReviewNotFound);
}

#[test]
fn an_unreachable_service() {
    // A port nothing listens on: bind, note it, close.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let c = Client::new(&format!("http://127.0.0.1:{port}"), "tok");
    let e = c.get("/repos/a/b/pulls/1").unwrap_err();
    assert_eq!(e.code, Code::ReviewUnreachable);
    assert_eq!(e.code.exit_code(), 4);
}

#[test]
fn downloads_bytes_by_relative_or_absolute_url() {
    let fake = Fake::start(vec![(
        "GET",
        "/api/v1/artifacts/k1".to_owned(),
        200,
        "image/png",
        vec![1, 2, 3],
    )]);
    let c = Client::new(&fake.url, "tok");
    assert_eq!(c.download("/api/v1/artifacts/k1").unwrap(), vec![1, 2, 3]);
    assert_eq!(
        c.download(&format!("{}/api/v1/artifacts/k1", fake.url))
            .unwrap(),
        vec![1, 2, 3]
    );
}
