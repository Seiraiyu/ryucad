//! BLAKE3 of documents' bytes as the session shares them: each allocation
//! hashed once per process, so judging a catalogue again after a write
//! hashes only the bytes that write brought (design 937). The cache is
//! keyed by the allocation and holds it weakly: a dropped file leaves no
//! entry that a new allocation at the same address could answer for.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

type Entry = (Weak<[u8]>, String);

static CACHE: Mutex<Option<HashMap<usize, Entry>>> = Mutex::new(None);
static HASHED: AtomicUsize = AtomicUsize::new(0);

fn key(b: &Arc<[u8]>) -> usize {
    Arc::as_ptr(b) as *const u8 as usize
}

/// The BLAKE3 hex of `b`.
pub fn of(b: &Arc<[u8]>) -> String {
    {
        let cache = CACHE.lock().expect("the digest cache isn't poisoned");
        if let Some((w, h)) = cache.as_ref().and_then(|c| c.get(&key(b)))
            && w.upgrade().is_some_and(|x| Arc::ptr_eq(&x, b))
        {
            return h.clone();
        }
    }
    let h = blake3::hash(b).to_hex().to_string();
    HASHED.fetch_add(1, Ordering::Relaxed);
    let mut cache = CACHE.lock().expect("the digest cache isn't poisoned");
    let c = cache.get_or_insert_with(HashMap::new);
    if c.len() >= 65_536 {
        c.retain(|_, (w, _)| w.strong_count() > 0);
    }
    c.insert(key(b), (Arc::downgrade(b), h.clone()));
    h
}

/// [`of`] for each of `bytes`, in order, on up to eight threads once
/// there are 64 or more.
pub fn all(bytes: &[Arc<[u8]>]) -> Vec<String> {
    if bytes.len() < 64 {
        return bytes.iter().map(of).collect();
    }
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    let per = bytes.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = bytes
            .chunks(per)
            .map(|c| s.spawn(move || c.iter().map(of).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a hashing thread doesn't panic"))
            .collect()
    })
}

/// Drops entries whose bytes are gone.
pub fn prune() {
    if let Some(c) = CACHE
        .lock()
        .expect("the digest cache isn't poisoned")
        .as_mut()
    {
        c.retain(|_, (w, _)| w.strong_count() > 0);
    }
}

/// How many hashes have been computed (not answered from the cache): tests.
pub fn hashed() -> usize {
    HASHED.load(Ordering::Relaxed)
}

#[cfg(test)]
fn cached(h: &str) -> bool {
    CACHE
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|c| c.values().any(|(_, x)| x == h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_bytes_are_hashed_once_and_forgotten_when_dropped() {
        let a: Arc<[u8]> = Arc::from(b"some bytes".as_slice());
        let want = blake3::hash(b"some bytes").to_hex().to_string();
        assert_eq!(of(&a), want);
        assert!(cached(&want), "kept for the next ask");
        assert_eq!(of(&a.clone()), want, "a clone is the same allocation");
        // Equal bytes in another allocation are hashed again, never mixed up.
        let b: Arc<[u8]> = Arc::from(b"other bytes".as_slice());
        assert_eq!(of(&b), blake3::hash(b"other bytes").to_hex().to_string());
        drop(a);
        prune();
        assert!(!cached(&want), "a dropped allocation leaves the cache");
        let many: Vec<Arc<[u8]>> = (0..100u8).map(|k| Arc::from(vec![k; 1000])).collect();
        let got = all(&many);
        for (b, h) in many.iter().zip(&got) {
            assert_eq!(*h, blake3::hash(b).to_hex().to_string());
        }
    }
}
