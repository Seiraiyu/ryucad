//! Hole specs: no input panics the parser; anything that parses writes a
//! canonical text that parses to a spec with the same text; and a seeded
//! spec's text parses back to it.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::hole::spec::{Spec, arbitrary};

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data)
        && let Ok(s) = Spec::parse(text)
    {
        let t = s.text();
        let again = Spec::parse(&t).unwrap_or_else(|e| panic!("{t}: {e}"));
        assert_eq!(again.text(), t);
    }
    let mut seed = data.iter().fold(0x9e37_79b9_7f4a_7c15u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3)
    });
    if seed == 0 {
        seed = 1;
    }
    let s = arbitrary(&mut seed);
    assert_eq!(Spec::parse(&s.text()).unwrap(), s);
});
