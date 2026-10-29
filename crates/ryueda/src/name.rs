//! IDs. Library objects take readable IDs made from their names
//! (`R_0402_1005Metric` is `r_0402_1005metric`), so importing a library
//! again finds the same objects. Design objects (blocks, components,
//! nets, instances, ports, buses, rails, classes, designs) take random
//! ones: 10 lowercase base32 characters, 50 bits of the OS's randomness,
//! so two agents adding parts on two branches never pick the same ID.
//! `RYUEDA_ID_SEED` makes them a fixed sequence, for tests.

use crate::error::{Code, Error, Result};
use crate::id::Id;

/// The longest readable ID before a collision's `-<n>`, so the whole
/// stays within an ID's 63 characters.
const MAX: usize = 56;

/// Names Windows reserves for devices: an ID equal to one gets a `_`.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The readable form of `name`: ASCII letters lowercased, digits, `_` and
/// `-` kept, every run of anything else one `_`, trimmed to start and end
/// with a letter or digit, at most 56 characters; `item` when nothing is
/// left.
pub fn readable(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let edge = |c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit());
    let mut s = out.trim_matches(edge).to_owned();
    if s.len() > MAX {
        s.truncate(MAX);
        s = s.trim_end_matches(edge).to_owned();
    }
    if s.is_empty() {
        s = "item".to_owned();
    }
    if RESERVED.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

/// The ID for an object called `name`: its readable form, or with `-2`,
/// `-3`, … when `taken` refuses that.
pub fn free(name: &str, taken: impl Fn(&str) -> bool) -> Id {
    let base = readable(name);
    let text = std::iter::once(base.clone())
        .chain((2u64..).map(|n| format!("{base}-{n}")))
        .find(|s| !taken(s))
        .expect("some n is free");
    Id::parse(&text, "library object").expect("readable IDs are IDs")
}

/// RFC 4648's base32 alphabet, lowercased: every character may start
/// an ID.
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// The variable that makes random IDs a fixed sequence.
pub const SEED_VAR: &str = "RYUEDA_ID_SEED";

/// 50 bits as 10 base32 characters, the first five bits first.
fn base32(bits: u64) -> String {
    (0..10)
        .map(|i| BASE32[((bits >> (45 - 5 * i)) & 31) as usize] as char)
        .collect()
}

/// A random ID for a design object that `taken` doesn't refuse. With
/// `RYUEDA_ID_SEED` set, the IDs are the seed's sequence instead, each
/// the first of it `taken` doesn't refuse: the same files give the same
/// IDs in any process, in a daemon or not.
pub fn random(taken: impl Fn(&str) -> bool) -> Result<Id> {
    random_from(std::env::var(SEED_VAR).ok().as_deref(), taken)
}

/// [`random`], with the seed given.
pub fn random_from(seed: Option<&str>, taken: impl Fn(&str) -> bool) -> Result<Id> {
    for k in 0u64.. {
        let bits = match seed {
            Some(seed) => {
                let mut h = blake3::Hasher::new();
                h.update(b"ryueda.id.1\0");
                h.update(seed.as_bytes());
                h.update(&k.to_le_bytes());
                let b = h.finalize();
                u64::from_le_bytes(b.as_bytes()[..8].try_into().expect("8 bytes"))
            }
            None => {
                let mut b = [0u8; 8];
                getrandom::fill(&mut b)
                    .map_err(|e| Error::new(Code::Internal, format!("no random source: {e}")))?;
                u64::from_le_bytes(b)
            }
        };
        let text = base32(bits & ((1 << 50) - 1));
        if !taken(&text) {
            return Id::parse(&text, "design object");
        }
    }
    unreachable!("some ID is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_ids() {
        assert_eq!(readable("R_0402_1005Metric"), "r_0402_1005metric");
        assert_eq!(readable("Conn_01x02_Pin"), "conn_01x02_pin");
        assert_eq!(readable("LED 3mm (red)"), "led_3mm_red");
        assert_eq!(readable("  +3V3 "), "3v3");
        assert_eq!(readable("Ω µ"), "item");
        assert_eq!(readable("--a--"), "a");
        assert_eq!(readable("CON"), "con_");
        let long = readable(&"ab".repeat(40));
        assert_eq!(long.len(), 56);
        assert_eq!(readable(&format!("{}__x", "a".repeat(55))), "a".repeat(55));
    }

    #[test]
    fn collisions_take_the_next_number() {
        let taken = ["r", "r-2"];
        assert_eq!(free("R", |s| taken.contains(&s)).as_str(), "r-3");
        assert_eq!(free("C", |s| taken.contains(&s)).as_str(), "c");
        let long = free(&"x".repeat(80), |s| s.len() == 56);
        assert_eq!(long.as_str().len(), 58);
    }

    #[test]
    fn random_ids_are_ten_base32_characters() {
        let id = random_from(None, |_| false).unwrap();
        assert_eq!(id.as_str().len(), 10);
        assert!(id.as_str().bytes().all(|c| BASE32.contains(&c)));
        assert_ne!(random_from(None, |_| false).unwrap(), id);
        assert_eq!(base32(0), "aaaaaaaaaa");
        assert_eq!(base32((1 << 50) - 1), "7777777777");
        assert_eq!(base32(1 << 45), "baaaaaaaaa");
    }

    #[test]
    fn a_seed_gives_a_fixed_sequence_past_what_is_taken() {
        let first = random_from(Some("t"), |_| false).unwrap();
        assert_eq!(random_from(Some("t"), |_| false).unwrap(), first);
        let second = random_from(Some("t"), |s| s == first.as_str()).unwrap();
        assert_ne!(second, first);
        let other = random_from(Some("u"), |_| false).unwrap();
        assert_ne!(other, first);
        assert_eq!(first.as_str().len(), 10);
    }
}
