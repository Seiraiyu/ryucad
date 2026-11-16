//! The client against the stand-in service.

use ryu_review::testing::{Fake, json};
use ryu_review::{Client, Product};
use serde_json::json as j;

#[test]
fn a_product_names_itself_and_finds_its_service() {
    let fake = Fake::start(vec![json("GET", "/api/v1/x", 200, j!({"ok": 1}))]);
    // A variable only this test sets: no machine's token or preference.
    static P: Product = Product {
        name: "ryutest",
        version: "1.2.3",
        url_var: "RYU_REVIEW_TEST_URL",
        token_var: "RYU_REVIEW_TEST_TOKEN",
        url_pref: || None,
    };
    // SAFETY: this test binary sets only its own variables, before any
    // thread reads them.
    unsafe {
        std::env::set_var("RYU_REVIEW_TEST_URL", &fake.url);
        std::env::set_var("RYU_REVIEW_TEST_TOKEN", "tok");
    }
    assert_eq!(P.client().unwrap().get("/x").unwrap(), j!({"ok": 1}));
    assert_eq!(
        fake.seen.lock().unwrap()[0].auth.as_deref(),
        Some("Bearer tok")
    );
    let c = Client::new(&fake.url, "tok");
    assert_eq!(c.get("/x").unwrap(), j!({"ok": 1}));
}
