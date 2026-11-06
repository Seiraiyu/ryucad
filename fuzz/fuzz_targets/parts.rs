//! ryueda's part data from bytes: as a parts cache entry (a typed miss, or
//! a record and its terms that encode and decode to themselves), and as
//! the text of every vocabulary parameter and of a search condition. Any
//! panic is a bug. 6.B and 6.C add each adapter's answers.

#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../crates/ryueda-parts/tests/bytes.rs"]
#[allow(dead_code)]
mod bytes;

fuzz_target!(|data: &[u8]| bytes::check(data));
