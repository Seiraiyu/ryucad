//! How names and keys were derived, recorded while a caller asks
//! ([`recording`]), so a caller can derive them again under another key
//! (ryucad's flattened imports, healed once per shape and named per
//! feature, #813). Recording changes no result: [`super::Name::derive`]
//! and [`super::OpKey::from_bytes`] return what they always do, and only
//! note how while a recording runs on their thread.
//!
//! A recording is the calling thread's alone: a thread-local stack, so
//! nothing locks and other threads pay one thread-local read a derive.
//! Work the recorded call hands to other threads (rayon) isn't seen; a
//! caller checks that what it needs was recorded ([`Derivations::has`]).

use super::{Name, OpKey, Role};
use std::cell::RefCell;
use std::collections::BTreeMap;

/// How a name was made: [`Name::derive`]'s arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derived {
    pub key: OpKey,
    pub role: Role,
    pub parents: Vec<Name>,
}

/// What a recording saw: each name derived and how, and each key made
/// from bytes with its bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Derivations {
    pub names: BTreeMap<Name, Derived>,
    pub keys: BTreeMap<OpKey, Vec<u8>>,
}

thread_local! {
    /// This thread's recordings, innermost last: a derive goes to the
    /// innermost only.
    static RECORDING: RefCell<Vec<Derivations>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn name(name: Name, key: OpKey, role: Role, parents: &[Name]) {
    RECORDING.with(|r| {
        if let Ok(mut r) = r.try_borrow_mut()
            && let Some(t) = r.last_mut()
        {
            t.names.entry(name).or_insert_with(|| Derived {
                key,
                role,
                parents: parents.to_vec(),
            });
        }
    });
}

pub(super) fn key(key: OpKey, bytes: &[u8]) {
    RECORDING.with(|r| {
        if let Ok(mut r) = r.try_borrow_mut()
            && let Some(t) = r.last_mut()
        {
            t.keys.entry(key).or_insert_with(|| bytes.to_vec());
        }
    });
}

/// Takes the innermost recording off this thread's stack even if `f`
/// unwinds.
struct Pop;

impl Drop for Pop {
    fn drop(&mut self) {
        RECORDING.with(|r| {
            r.borrow_mut().pop();
        });
    }
}

/// `f`'s result and every derivation made on this thread while it ran
/// (and perhaps others' a rayon worker ran here meanwhile, harmless to a
/// caller that looks names up by value). Derivations on other threads
/// aren't seen.
pub fn recording<T>(f: impl FnOnce() -> T) -> (T, Derivations) {
    RECORDING.with(|r| r.borrow_mut().push(Derivations::default()));
    let pop = Pop;
    let out = f();
    let seen = RECORDING.with(|r| r.borrow_mut().last_mut().map(std::mem::take));
    drop(pop);
    (out, seen.unwrap_or_default())
}

impl Derivations {
    /// Whether `n` was recorded.
    pub fn has(&self, n: Name) -> bool {
        self.names.contains_key(&n)
    }

    /// `n` as it would have been derived had every key `from` led to been
    /// `to`'s: each recorded name derived again from its parents so
    /// derived, under its key so mapped; a key `from` is `to`, a key that
    /// is a recorded name is that name so derived, a key made from bytes
    /// holding `from`'s is made from them holding `to`'s. Anything not
    /// recorded (a geometry hash, an ordinal) is as it is. `memo` holds
    /// what is done.
    pub fn rekey(&self, from: OpKey, to: OpKey, n: Name, memo: &mut BTreeMap<Name, Name>) -> Name {
        if let Some(m) = memo.get(&n) {
            return *m;
        }
        let out = match self.names.get(&n) {
            Some(d) => {
                let key = self.rekey_key(from, to, d.key, memo);
                let parents: Vec<Name> = d
                    .parents
                    .iter()
                    .map(|p| self.rekey(from, to, *p, memo))
                    .collect();
                Name::derive(key, d.role, &parents)
            }
            None => n,
        };
        memo.insert(n, out);
        out
    }

    /// A key as [`Derivations::rekey`] maps it.
    pub fn rekey_key(
        &self,
        from: OpKey,
        to: OpKey,
        k: OpKey,
        memo: &mut BTreeMap<Name, Name>,
    ) -> OpKey {
        if k == from {
            return to;
        }
        let as_name = Name::from_u128(k.0);
        if self.names.contains_key(&as_name) {
            return OpKey(self.rekey(from, to, as_name, memo).to_u128());
        }
        if let Some(bytes) = self.keys.get(&k) {
            let (f, t) = (from.0.to_le_bytes(), to.0.to_le_bytes());
            let mut b = bytes.clone();
            let mut i = 0;
            let mut hit = false;
            while i + 16 <= b.len() {
                if b[i..i + 16] == f {
                    b[i..i + 16].copy_from_slice(&t);
                    hit = true;
                    i += 16;
                } else {
                    i += 1;
                }
            }
            if hit {
                return OpKey::from_bytes(&b);
            }
        }
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chain through a name used as a key and a key made from bytes,
    /// derived again under another key, is the chain made under it.
    #[test]
    fn a_chain_derived_again_is_the_chain_under_the_other_key() {
        let leaf = Name::from_u128(5);
        let chain = move |k: OpKey| {
            let a = Name::derive(k, Role::NewFace, &[leaf]);
            let sub = OpKey(Name::derive(k, Role::Merged, &[]).to_u128());
            let b = Name::derive(sub, Role::NewEdge, &[a, leaf]);
            let bytes = [b"tag\0".as_slice(), &k.0.to_le_bytes(), &[9]].concat();
            let c = Name::derive(OpKey::from_bytes(&bytes), Role::Vertex, &[b]);
            [a, b, c]
        };
        let (from, to) = (OpKey(11), OpKey(12));
        let (got, d) = recording(|| chain(from));
        let want = chain(to);
        let mut memo = BTreeMap::new();
        for (g, w) in got.iter().zip(&want) {
            assert_eq!(d.rekey(from, to, *g, &mut memo), *w);
        }
        assert_eq!(d.rekey(from, to, leaf, &mut memo), leaf);
        // Nothing records outside a recording, nor on another thread.
        let (_, empty) = recording(|| ());
        assert!(empty.names.is_empty());
        let (_, apart) = recording(|| std::thread::spawn(move || chain(from)).join().unwrap());
        assert!(apart.names.is_empty() && apart.keys.is_empty());
        // Nested: the inner sees its own, the outer the rest.
        let (inner, outer) = recording(|| {
            let a = Name::derive(from, Role::NewFace, &[]);
            let (_, inner) = recording(|| Name::derive(from, Role::NewEdge, &[]));
            assert!(!inner.has(a));
            inner
        });
        assert_eq!(inner.names.len(), 1);
        assert_eq!(outer.names.len(), 1);
    }
}
