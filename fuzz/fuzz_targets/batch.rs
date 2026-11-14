//! `batch` from bytes (design 603, plan 31.C): see
//! `crates/ryucad/tests/batch_bytes/mod.rs`. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../crates/ryucad/tests/batch_bytes/mod.rs"]
#[allow(dead_code)]
mod batch_bytes;

fuzz_target!(|data: &[u8]| batch_bytes::check(data));
