//! Annotations (design 374): a person's requests to the agent, kept in
//! `annotations/` (see `doc::annotation` for the file), with their
//! derived states, their proposed changes (see [`propose`]) and the
//! requests that list, make, answer and close them. Nothing here acts on
//! an annotation by itself: every change is a request someone sent.

use crate::doc::annotation::check_id;
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};

/// Set, annotation IDs are a fixed sequence from it: the same requests
/// give the same IDs in any process, in a daemon or not (tests).
pub const ID_SEED_VAR: &str = "RYUCAD_ID_SEED";

/// Set to a time (`YYYY-MM-DDTHH:MM:SSZ`), what the clock reads (tests).
pub const NOW_VAR: &str = "RYUCAD_NOW";

const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// A new annotation ID that `taken` doesn't refuse.
pub fn new_id(taken: impl Fn(&str) -> bool) -> Result<Id> {
    new_id_from(std::env::var(ID_SEED_VAR).ok().as_deref(), taken)
}

/// [`new_id`] with the seed given: 50 bits, random or the seed's
/// sequence (BLAKE3 of `ryucad.annotation.id.1`, the seed and a counter),
/// as 10 base32 characters, as ryueda's design IDs are.
pub fn new_id_from(seed: Option<&str>, taken: impl Fn(&str) -> bool) -> Result<Id> {
    for k in 0u64.. {
        let bits = match seed {
            Some(seed) => {
                let mut h = blake3::Hasher::new();
                h.update(b"ryucad.annotation.id.1\0");
                h.update(seed.as_bytes());
                h.update(&k.to_le_bytes());
                u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().expect("8 bytes"))
            }
            None => {
                let mut b = [0u8; 8];
                getrandom::fill(&mut b)
                    .map_err(|e| Error::new(Code::Internal, format!("no random source: {e}")))?;
                u64::from_le_bytes(b)
            }
        };
        let bits = bits & ((1 << 50) - 1);
        let text: String = (0..10)
            .map(|i| BASE32[((bits >> (45 - 5 * i)) & 31) as usize] as char)
            .collect();
        if !taken(&text) {
            return check_id(&text);
        }
    }
    unreachable!("some ID is free")
}

/// Now, UTC to the second: `YYYY-MM-DDTHH:MM:SSZ`, or `RYUCAD_NOW`.
pub fn now() -> String {
    now_from(std::env::var(NOW_VAR).ok().as_deref())
}

pub fn now_from(fixed: Option<&str>) -> String {
    if let Some(t) = fixed.filter(|t| crate::doc::annotation::is_time(t)) {
        return t.to_owned();
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    format!("{}Z", crate::export::utc(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_random_or_the_seeds_sequence() {
        let a = new_id_from(None, |_| false).unwrap();
        let b = new_id_from(None, |_| false).unwrap();
        assert_ne!(a, b);
        assert!(crate::doc::annotation::check_id(a.as_str()).is_ok());
        let s1 = new_id_from(Some("t"), |_| false).unwrap();
        assert_eq!(new_id_from(Some("t"), |_| false).unwrap(), s1);
        // A taken ID is passed over to the seed's next.
        let s2 = new_id_from(Some("t"), |id| id == s1.as_str()).unwrap();
        assert_ne!(s2, s1);
    }

    #[test]
    fn the_clock_is_utc_to_the_second_or_ryucad_now() {
        assert!(crate::doc::annotation::is_time(&now_from(None)));
        assert_eq!(
            now_from(Some("2026-11-05T14:02:11Z")),
            "2026-11-05T14:02:11Z"
        );
        // A value that isn't a time is ignored.
        assert!(crate::doc::annotation::is_time(&now_from(Some("soon"))));
    }
}
