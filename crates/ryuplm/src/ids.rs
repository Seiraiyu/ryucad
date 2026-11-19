//! Item and line IDs (design 734): 64 random bits as 13 characters of
//! Crockford's base32, lowercased, so two branches never give two items
//! one ID, and items are filed by their ID's first two characters, a
//! shard that never moves. `RYUPLM_ID_SEED` makes them a fixed sequence,
//! for tests.

use crate::error::{Code, Error, Result};
use crate::id::Id;

/// Crockford's alphabet, lowercased: no i, l, o or u.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// The variable that makes random IDs a fixed sequence.
pub const SEED_VAR: &str = "RYUPLM_ID_SEED";

/// The folder of items, below the catalogue.
pub const ITEMS: &str = "items";

/// `bits` as 13 characters: bits 63..4 five at a time, then the low four.
pub fn text(bits: u64) -> String {
    let mut s: String = (0..12)
        .map(|i| ALPHABET[((bits >> (59 - 5 * i)) & 31) as usize] as char)
        .collect();
    s.push(ALPHABET[(bits & 15) as usize] as char);
    s
}

/// Whether `s` has the form of an ID [`random`] makes: 13 characters of
/// the alphabet. [`random`]'s own end in `0`-`f` (the 13th character holds
/// the low four bits), but the form takes any of the 32 there, so an ID
/// written by hand like the design's example `k3v9q2m8x4c1p` is one too.
pub fn generated(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 13 && b.iter().all(|c| ALPHABET.contains(c))
}

/// A random ID that `taken` doesn't refuse; the seed's sequence with
/// `RYUPLM_ID_SEED` set, the same in any process, daemon or not.
pub fn random(taken: impl Fn(&str) -> bool) -> Result<Id> {
    random_from(std::env::var(SEED_VAR).ok().as_deref(), taken)
}

/// The `k`th draw of seed `seed`: what [`random_from`] tries `k`th.
pub fn seeded_bits(seed: &str, k: u64) -> u64 {
    let mut h = blake3::Hasher::new();
    h.update(b"ryuplm.id.1\0");
    h.update(seed.as_bytes());
    h.update(&k.to_le_bytes());
    u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().expect("8 bytes"))
}

/// The IDs [`random_from`] gives seed `seed` when each is taken before the
/// next is asked for, in order, in one pass: it tries draws from 0 every
/// time, so asking it for `n` costs `n²/2` hashes, too slow for the bars'
/// 100,000 items. The same IDs, so seeded catalogues don't change.
pub struct Seeded<'a> {
    seed: &'a str,
    k: u64,
    taken: std::collections::HashSet<String>,
}

impl<'a> Seeded<'a> {
    pub fn new(seed: &'a str) -> Seeded<'a> {
        Seeded {
            seed,
            k: 0,
            taken: Default::default(),
        }
    }
}

impl Iterator for Seeded<'_> {
    type Item = Id;
    fn next(&mut self) -> Option<Id> {
        loop {
            let t = text(seeded_bits(self.seed, self.k));
            self.k += 1;
            if self.taken.insert(t.clone()) {
                return Some(Id::parse(&t, "item").expect("a generated ID parses"));
            }
        }
    }
}

/// [`random`], with the seed given.
pub fn random_from(seed: Option<&str>, taken: impl Fn(&str) -> bool) -> Result<Id> {
    for k in 0u64.. {
        let bits = match seed {
            Some(seed) => seeded_bits(seed, k),
            None => {
                let mut b = [0u8; 8];
                getrandom::fill(&mut b)
                    .map_err(|e| Error::new(Code::Internal, format!("no random source: {e}")))?;
                u64::from_le_bytes(b)
            }
        };
        let t = text(bits);
        if !taken(&t) {
            return Id::parse(&t, "item");
        }
    }
    unreachable!("some ID is free")
}

/// Where item `id` lives: `items/<first two>/<id>.json`.
pub fn item_path(id: &Id) -> String {
    format!("{ITEMS}/{}/{id}.json", &id.as_str()[..2])
}

/// The ID of the item file at `path`, when `path` is one at its shard.
pub fn item_of(path: &str) -> Option<Id> {
    let name = shard_of(path)?;
    let id = Id::parse(name, "item").ok()?;
    (item_path(&id) == path).then_some(id)
}

/// The name of a JSON file one folder below `items/`, at its shard or not:
/// what a misplaced item file is called.
pub fn shard_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(ITEMS)?.strip_prefix('/')?;
    let (dir, file) = rest.split_once('/')?;
    (dir.len() == 2 && !file.contains('/')).then_some(())?;
    file.strip_suffix(".json").filter(|n| n.len() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_thirteen_crockford_characters() {
        assert_eq!(text(0), "0000000000000");
        assert_eq!(text(u64::MAX), "zzzzzzzzzzzzf");
        // The first two characters take every one of 1,024 values.
        assert_eq!(&text(0b11111_00001 << 54)[..2], "z1");
        let id = random_from(Some("seed"), |_| false).unwrap();
        assert_eq!(id.as_str().len(), 13);
        assert!(generated(id.as_str()));
        assert_eq!(random_from(Some("seed"), |_| false).unwrap(), id);
        let next = random_from(Some("seed"), |s| s == id.as_str()).unwrap();
        let mut made: Vec<Id> = Vec::new();
        for _ in 0..50 {
            made.push(random_from(Some("seed"), |s| made.iter().any(|m| m.as_str() == s)).unwrap());
        }
        assert_eq!(
            Seeded::new("seed").take(50).collect::<Vec<_>>(),
            made,
            "one pass, the same IDs"
        );
        assert_ne!(next, id);
        assert!(!generated("k3v9q2m8x4c1"));
        assert!(!generated("k3v9q2m8x4c1i"));
        assert!(generated("k3v9q2m8x4c1p"));
    }

    #[test]
    fn items_live_at_their_shard() {
        let id = Id::parse("k3v9q2m8x4c1p", "item").unwrap();
        assert_eq!(item_path(&id), "items/k3/k3v9q2m8x4c1p.json");
        assert_eq!(item_of("items/k3/k3v9q2m8x4c1p.json"), Some(id.clone()));
        assert_eq!(item_of("items/k4/k3v9q2m8x4c1p.json"), None);
        assert_eq!(item_of("items/k3v9q2m8x4c1p.json"), None);
        assert_eq!(item_of("items/k3/k3v9q2m8x4c1p.txt"), None);
        assert_eq!(item_of("items/k3/sub/k3v9q2m8x4c1p.json"), None);
        assert_eq!(
            shard_of("items/k4/k3v9q2m8x4c1p.json"),
            Some("k3v9q2m8x4c1p")
        );
    }
}
