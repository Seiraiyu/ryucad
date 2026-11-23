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
    shard_in(ITEMS, path)
}

/// The folder of documents, below the catalogue (design 937).
pub const DOCUMENTS: &str = "documents";

/// The name of a JSON file one folder below `top`, at its shard or not.
fn shard_in<'a>(top: &str, path: &'a str) -> Option<&'a str> {
    let rest = path.strip_prefix(top)?.strip_prefix('/')?;
    let (dir, file) = rest.split_once('/')?;
    (dir.len() == 2 && !file.contains('/')).then_some(())?;
    file.strip_suffix(".json").filter(|n| n.len() >= 2)
}

/// [`shard_of`] for document records.
pub fn doc_shard_of(path: &str) -> Option<&str> {
    shard_in(DOCUMENTS, path)
}

/// Where document `id`'s record lives: `documents/<first two>/<id>.json`.
pub fn document_path(id: &Id) -> String {
    format!("{DOCUMENTS}/{}/{id}.json", &id.as_str()[..2])
}

/// The ID of the document record at `path`, when it is one at its shard.
pub fn document_of(path: &str) -> Option<Id> {
    let id = Id::parse(doc_shard_of(path)?, "document").ok()?;
    (document_path(&id) == path).then_some(id)
}

/// The folder holding document `id`'s bytes.
pub fn bytes_dir(id: &Id) -> String {
    format!("{DOCUMENTS}/{}/{id}", &id.as_str()[..2])
}

/// Where document `id`'s bytes named `name` live.
pub fn bytes_path(id: &Id, name: &str) -> String {
    format!("{}/{name}", bytes_dir(id))
}

/// The document whose folder holds the file at `path`, and the file's
/// name: `documents/<first two>/<id>/<name>`, at its shard.
pub fn bytes_of(path: &str) -> Option<(Id, &str)> {
    let rest = path.strip_prefix(DOCUMENTS)?.strip_prefix('/')?;
    let mut it = rest.splitn(3, '/');
    let (shard, id, name) = (it.next()?, it.next()?, it.next()?);
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let id = Id::parse(id, "document").ok()?;
    (id.as_str().len() >= 2 && &id.as_str()[..2] == shard).then_some((id, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_and_their_bytes_live_at_their_shard() {
        let id = Id::parse("k3v9q2m8x4c1p", "document").unwrap();
        assert_eq!(document_path(&id), "documents/k3/k3v9q2m8x4c1p.json");
        assert_eq!(
            document_of("documents/k3/k3v9q2m8x4c1p.json"),
            Some(id.clone())
        );
        assert_eq!(document_of("documents/k4/k3v9q2m8x4c1p.json"), None);
        assert_eq!(document_of("items/k3/k3v9q2m8x4c1p.json"), None);
        assert_eq!(bytes_dir(&id), "documents/k3/k3v9q2m8x4c1p");
        assert_eq!(
            bytes_path(&id, "Spec A.pdf"),
            "documents/k3/k3v9q2m8x4c1p/Spec A.pdf"
        );
        assert_eq!(
            bytes_of("documents/k3/k3v9q2m8x4c1p/Spec A.pdf"),
            Some((id.clone(), "Spec A.pdf"))
        );
        assert_eq!(bytes_of("documents/k4/k3v9q2m8x4c1p/a.pdf"), None);
        assert_eq!(bytes_of("documents/k3/k3v9q2m8x4c1p/sub/a.pdf"), None);
        assert_eq!(bytes_of("documents/k3/k3v9q2m8x4c1p.json"), None);
        assert_eq!(
            doc_shard_of("documents/k4/k3v9q2m8x4c1p.json"),
            Some("k3v9q2m8x4c1p")
        );
    }

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
