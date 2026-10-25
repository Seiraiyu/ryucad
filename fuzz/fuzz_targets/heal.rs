//! Shapes with a file's faults from bytes
//! ([`ryukei_testkit::heal_bytes`]): bodies written as a file describes
//! them, then faces turned over, edges copied, moved or split, loops
//! reversed and cavities forgotten. Healing must give a typed refusal
//! ([`Error::InvalidHeal`]) or a body that passes the checker; faults
//! healing must undo must give the body's faces, edges and vertices back.
//! Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::check;
use ryukei::heal::{HealOptions, heal};
use ryukei::name::OpKey;
use ryukei_testkit::heal_bytes::heal_case;

fuzz_target!(|data: &[u8]| {
    let Some(case) = heal_case(data) else {
        return;
    };
    match heal(OpKey(0x4ea1), &case.shape, &HealOptions::default()) {
        Ok((body, _, _)) => {
            assert_eq!(check(&body), []);
            if case.mild {
                assert_eq!(body.counts(), case.body.counts());
            }
        }
        Err(Error::InvalidHeal(e)) => assert!(!case.mild, "a mild case refused: {e:?}"),
        Err(e) => panic!("unexpected {e:?}"),
    }
});
