//! Sheet-metal parts from bytes ([`ryukei_testkit::sheet_metal_bytes`],
//! design 298, plan 11.D): a plate flanged on its walls, drilled, unfolded,
//! cut across its bend lines in the flat, refolded and unfolded again.
//! Every step is a typed refusal, or every body checks clean and meshes,
//! the refolded part is the part where nothing was cut, and the second
//! flat is the cut one. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei_testkit::sheet_metal_bytes::{case, judge, trip, typed};

fuzz_target!(|data: &[u8]| {
    let Some(made) = case(data) else {
        return;
    };
    let c = match made {
        Ok(c) => c,
        Err(e) => return assert!(typed(&e), "{e:?}"),
    };
    match trip(&c) {
        Ok(r) => judge(&c, &r).unwrap(),
        Err(e) => assert!(typed(&e), "{e:?}"),
    }
});
