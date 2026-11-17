use super::{Name, ToolKey};

/// One change to a named entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Created(Name),
    Deleted(Name),
    /// The geometry changed but the identity did not.
    Modified(Name),
    /// The parent's name retired. An Euler operator's children are in its
    /// documented order; a boolean's are sorted.
    Split {
        parent: Name,
        children: Vec<Name>,
    },
    /// Every parent's name retired. The parents are sorted.
    Merged {
        parents: Vec<Name>,
        child: Name,
    },
    /// A boolean's tool entity: its own name `from` became `to`, derived
    /// from the boolean's key and the tool's.
    Placed {
        tool: ToolKey,
        from: Name,
        to: Name,
    },
    /// A blended edge or a vertex a blend took away: its name retired,
    /// and `new` are the blend faces that took its place, sorted. ryucad
    /// carries a reference to the edge over to its fillet this way.
    Replaced {
        old: Name,
        new: Vec<Name>,
    },
}

impl Event {
    /// A merge of two, with its parents sorted.
    pub fn merged(a: Name, b: Name, child: Name) -> Self {
        Self::merged_all(vec![a, b], child)
    }

    /// A merge of any number, with its parents sorted.
    pub fn merged_all(mut parents: Vec<Name>, child: Name) -> Self {
        parents.sort();
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

    /// These events with every name renamed by `f`, a merge's parents and
    /// a replacement's new names sorted again: what goes with
    /// [`crate::topo::Body::renamed`] (#813). Nothing else changes.
    pub fn renamed(&self, f: impl Fn(Name) -> Name) -> Journal {
        let events = self
            .events
            .iter()
            .map(|e| match e {
                Event::Created(n) => Event::Created(f(*n)),
                Event::Deleted(n) => Event::Deleted(f(*n)),
                Event::Modified(n) => Event::Modified(f(*n)),
                Event::Split { parent, children } => Event::Split {
                    parent: f(*parent),
                    children: children.iter().map(|c| f(*c)).collect(),
                },
                Event::Merged { parents, child } => {
                    Event::merged_all(parents.iter().map(|p| f(*p)).collect(), f(*child))
                }
                Event::Placed { tool, from, to } => Event::Placed {
                    tool: *tool,
                    from: f(*from),
                    to: f(*to),
                },
                Event::Replaced { old, new } => {
                    let mut new: Vec<Name> = new.iter().map(|n| f(*n)).collect();
                    new.sort();
                    Event::Replaced { old: f(*old), new }
                }
            })
            .collect();
        Journal { events }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::{OpKey, Role};

    #[test]
    fn renamed_maps_every_name_and_sorts_merges_again() {
        let n = |k: u128| Name::derive(OpKey(k), Role::NewFace, &[]);
        let f = |x: Name| Name::derive(OpKey(5), Role::Merged, &[x]);
        let mut j = Journal::new();
        j.push(Event::Created(n(1)));
        j.push(Event::merged(n(2), n(3), n(4)));
        j.push(Event::Split {
            parent: n(4),
            children: vec![n(6), n(5)],
        });
        let r = j.renamed(f);
        assert_eq!(r.events().len(), 3);
        assert_eq!(r.events()[0], Event::Created(f(n(1))));
        assert_eq!(r.events()[1], Event::merged(f(n(2)), f(n(3)), f(n(4))));
        assert_eq!(
            r.events()[2],
            Event::Split {
                parent: f(n(4)),
                children: vec![f(n(6)), f(n(5))],
            }
        );
    }

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
                    parents: vec![lo, hi],
                    child: c
                },
                Event::Deleted(c),
            ]
        );
    }

    #[test]
    fn merges_of_many_are_sorted_and_tools_are_recorded() {
        let names: Vec<Name> = (1..=4)
            .map(|k| Name::derive(OpKey(k), Role::NewEdge, &[]))
            .collect();
        let child = Name::derive(OpKey(9), Role::Merged, &names);
        let mut shuffled = names.clone();
        shuffled.reverse();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            Event::merged_all(shuffled, child),
            Event::Merged {
                parents: sorted,
                child
            }
        );
        let placed = Event::Placed {
            tool: ToolKey(7),
            from: names[0],
            to: names[1],
        };
        assert_ne!(placed, Event::Created(names[1]));
        let replaced = Event::Replaced {
            old: names[0],
            new: vec![names[1]],
        };
        assert_ne!(replaced, Event::Deleted(names[0]));
    }
}
