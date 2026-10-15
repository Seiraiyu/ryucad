use super::Name;

/// One change to a named entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Created(Name),
    Deleted(Name),
    /// The geometry changed but the identity did not.
    Modified(Name),
    /// The parent's name retired; the children are in the operator's
    /// documented order.
    Split {
        parent: Name,
        children: [Name; 2],
    },
    /// Both parents' names retired. The parents are sorted.
    Merged {
        parents: [Name; 2],
        child: Name,
    },
}

impl Event {
    /// A merge, with its parents sorted.
    pub fn merged(a: Name, b: Name, child: Name) -> Self {
        let parents = if a <= b { [a, b] } else { [b, a] };
        Self::Merged { parents, child }
    }
}

/// What an operation did to names, in order. ryucad follows an agent's
/// reference through it; the kernel only reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Journal {
    events: Vec<Event>,
}

impl Journal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Appends another operation's events after these.
    pub fn append(&mut self, mut other: Journal) {
        self.events.append(&mut other.events);
    }

    pub fn push(&mut self, event: Event) {
        self.events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::{OpKey, Role};

    #[test]
    fn records_in_order_and_sorts_merges() {
        let a = Name::derive(OpKey(1), Role::NewFace, &[]);
        let b = Name::derive(OpKey(2), Role::NewFace, &[]);
        let c = Name::derive(OpKey(3), Role::Merged, &[a, b]);
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let mut j = Journal::new();
        assert!(j.is_empty());
        j.push(Event::Created(a));
        j.push(Event::merged(hi, lo, c));
        let mut k = Journal::new();
        k.push(Event::Deleted(c));
        j.append(k);
        assert_eq!(
            j.events(),
            &[
                Event::Created(a),
                Event::Merged {
                    parents: [lo, hi],
                    child: c
                },
                Event::Deleted(c),
            ]
        );
    }
}
