//! How names and keys were derived, recorded while a caller asks
//! ([`recording`]), so a caller can derive them again under another key
//! (ryucad's flattened imports, healed once per shape and named per
//! feature, #813). Recording changes no result: [`super::Name::derive`]
//! and [`super::OpKey::from_bytes`] return what they always do, and only
//! note how while a recording runs.

use super::{Name, OpKey, Role};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// How many recordings run; the table is shared by all of them.
static RUNNING: AtomicUsize = AtomicUsize::new(0);
static TABLE: Mutex<Option<Derivations>> = Mutex::new(None);

fn table() -> std::sync::MutexGuard<'static, Option<Derivations>> {
    TABLE.lock().unwrap_or_else(|p| p.into_inner())
}

pub(super) fn name(name: Name, key: OpKey, role: Role, parents: &[Name]) {
    if RUNNING.load(Ordering::Acquire) == 0 {
        return;
    }
    if let Some(t) = table().as_mut() {
        t.names.entry(name).or_insert_with(|| Derived {
            key,
            role,
            parents: parents.to_vec(),
        });
    }
}

pub(super) fn key(key: OpKey, bytes: &[u8]) {
    if RUNNING.load(Ordering::Acquire) == 0 {
        return;
    }
    if let Some(t) = table().as_mut() {
        t.keys.entry(key).or_insert_with(|| bytes.to_vec());
    }
}

/// `f`'s result and every derivation made while it ran, on any thread:
/// at least `f`'s own, and possibly others' made at the same time, which
/// a caller that looks names up by value never confuses with its own.
pub fn recording<T>(f: impl FnOnce() -> T) -> (T, Derivations) {
    {
        let mut t = table();
        if RUNNING.fetch_add(1, Ordering::AcqRel) == 0 || t.is_none() {
            *t = Some(Derivations::default());
        }
    }
    let out = f();
    let mut t = table();
    let seen = t.clone().unwrap_or_default();
    if RUNNING.fetch_sub(1, Ordering::AcqRel) == 1 {
        *t = None;
    }
    (out, seen)
}

impl Derivations {
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
        let chain = |k: OpKey| {
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
        // Nothing records outside a recording.
        let (_, empty) = recording(|| ());
        assert!(empty.names.is_empty() || !empty.names.contains_key(&want[0]));
    }
}
