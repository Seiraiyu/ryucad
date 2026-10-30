//! ryusch from bytes: `ryusch::testing::check_bytes`, a seeded block and
//! a batch of instructions and moves laid out, judged by the hard rules,
//! twice alike, and stable over one edit. Any panic is a bug; finds go
//! in ryusch's `tests/fuzz_regressions.rs`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ryusch::testing::check_bytes(data));
