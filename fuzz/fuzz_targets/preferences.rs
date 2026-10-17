//! Preferences files from bytes: a hand edit, a newer ryucad, a torn
//! write. Any text must give only known keys with valid values, or a
//! reason it can't be used. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::config::{Scope, parse, validate};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(values) = parse(text) else {
        return;
    };
    for (k, v) in &values {
        assert_eq!(validate(k, v, Some(Scope::User)).unwrap(), Scope::User);
    }
});
