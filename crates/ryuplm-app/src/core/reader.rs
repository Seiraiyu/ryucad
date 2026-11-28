//! One request in flight per reader (design 938, as ryucad-app's
//! workspace reads, design 698): asking again while one is out only
//! marks it, and its answer asks once more, so a burst of notifications
//! (a 10,000-item import arriving as one change, or many) costs one round
//! however long it is.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Reader {
    out: bool,
    again: bool,
}

/// Readers by name: `item`, `bom`, `count:class:fastener`…
#[derive(Debug, Default)]
pub struct Readers(BTreeMap<String, Reader>);

impl Readers {
    /// Whether to send `key`'s request now; if not, it goes again when
    /// the one out is answered.
    pub fn ask(&mut self, key: &str) -> bool {
        let r = self.0.entry(key.to_owned()).or_default();
        if r.out {
            r.again = true;
            false
        } else {
            r.out = true;
            true
        }
    }

    /// `key`'s request was answered or refused: whether to send it again
    /// now, for what was asked meanwhile.
    pub fn answered(&mut self, key: &str) -> bool {
        let Some(r) = self.0.get_mut(key) else {
            return false;
        };
        r.out = r.again;
        std::mem::take(&mut r.again)
    }

    /// Whether `key` was asked again while its request was out: it is
    /// sent again once that is answered.
    pub fn again(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(|r| r.again)
    }

    pub fn busy(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(|r| r.out)
    }

    /// Whether nothing is out.
    pub fn idle(&self) -> bool {
        self.0.values().all(|r| !r.out)
    }

    /// The link was lost: nothing is out any more.
    pub fn reset(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_asks_is_one_round() {
        let mut r = Readers::default();
        assert!(r.ask("bom"), "the first goes");
        for _ in 0..10_000 {
            assert!(!r.ask("bom"), "held while one is out");
        }
        assert!(r.ask("findings"), "another reader isn't held by it");
        assert!(r.again("bom") && !r.again("findings"));
        assert!(
            r.answered("bom"),
            "once more for everything asked meanwhile"
        );
        assert!(!r.answered("bom"), "and no more");
        assert!(!r.busy("bom"));
        assert!(!r.answered("findings") && r.idle());
        r.ask("x");
        r.reset();
        assert!(r.idle() && r.ask("x"), "a reconnect starts afresh");
    }
}
