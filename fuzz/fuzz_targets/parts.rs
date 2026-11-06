//! ryueda's part data from bytes: as a parts cache entry (a typed miss, or
//! a record and its terms that encode and decode to themselves), and as
//! the text of every vocabulary parameter and of a search condition, and
//! as a catalogue's files (jlcparts' manifest, lookup table, shards and
//! attributes, raw and gzipped). Any panic is a bug. 6.C adds its
//! adapters' answers.

#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../crates/ryueda-parts/tests/bytes.rs"]
#[allow(dead_code)]
mod bytes;

fuzz_target!(|data: &[u8]| bytes::check(data));
