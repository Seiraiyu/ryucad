//! A part's solids. A feature whose operation is `new` founds a solid,
//! named by its own ID; every feature listed after it that names it as a
//! target changes it, in list order: the solid's chain. A feature's view
//! of a solid is the last of its chain listed before the feature: what it
//! reads the solid's faces, edges and vertices from. The solid's tip, the
//! last of its chain, is what the part's model shows. A `boolean` feature
//! joins its target's chain and consumes its tools: their chains end, and
//! they are no longer part of the model.

use crate::doc::id::Id;
use crate::doc::project::Part;
use std::collections::BTreeMap;

/// The solids of a part's list, as it is or as a write would leave it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Solids {
    /// Each solid's chain, founder first, by founder.
    chains: BTreeMap<Id, Vec<Id>>,
    /// The founders in list order.
    founders: Vec<Id>,
    /// Each listed feature's place in the list.
    at: BTreeMap<Id, usize>,
    /// Each consumed solid, by founder, with the boolean that consumed it.
    consumed: BTreeMap<Id, Id>,
}

impl Solids {
    /// The solids of the part's list.
    pub fn of(part: &Part) -> Solids {
        Solids::of_order(part, part.order())
    }

    /// The solids of `order`, a list of the part's features. A feature
    /// whose file doesn't load acts on nothing; one naming a solid founded
    /// after it doesn't join that chain (building it says why).
    pub fn of_order(part: &Part, order: &[Id]) -> Solids {
        let at: BTreeMap<Id, usize> = order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        let mut chains: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        let mut founders: Vec<Id> = Vec::new();
        let mut consumed: BTreeMap<Id, Id> = BTreeMap::new();
        for id in order {
            let Some(Ok(f)) = part.features.get(id) else {
                continue;
            };
            if f.combine().is_some_and(|c| c.is_new()) {
                chains.insert(id.clone(), vec![id.clone()]);
                founders.push(id.clone());
                continue;
            }
            // A solid joins the chain of what acts on it, until consumed.
            let (acts_on, consumes) = match f.boolean() {
                Some(b) => (vec![&b.target], b.tools.iter().collect()),
                None => (f.targets(), Vec::new()),
            };
            for t in acts_on {
                if !consumed.contains_key(t)
                    && let Some(chain) = chains.get_mut(t)
                {
                    chain.push(id.clone());
                }
            }
            for t in consumes {
                if chains.contains_key(t) && !consumed.contains_key(t) {
                    consumed.insert(t.clone(), id.clone());
                }
            }
        }
        Solids {
            chains,
            founders,
            at,
            consumed,
        }
    }

    /// The boolean that consumed `solid` before `reader`, if one did.
    pub fn consumed_by(&self, solid: &Id, reader: &Id) -> Option<&Id> {
        let by = self.consumed.get(solid)?;
        (self.at.get(by)? < self.at.get(reader)?).then_some(by)
    }

    /// The boolean that consumed `solid`, if one did.
    pub fn consumer(&self, solid: &Id) -> Option<&Id> {
        self.consumed.get(solid)
    }

    /// Whether `id` founds a solid.
    pub fn is_solid(&self, id: &Id) -> bool {
        self.chains.contains_key(id)
    }

    /// The solid founded by `founder`: its chain, founder first.
    pub fn chain(&self, founder: &Id) -> Option<&[Id]> {
        self.chains.get(founder).map(Vec::as_slice)
    }

    /// The solids `feature` belongs to: its own, if it founds one, else
    /// those it acts on, in list order.
    pub fn solids_of(&self, feature: &Id) -> Vec<&Id> {
        self.founders
            .iter()
            .filter(|s| self.chains[*s].contains(feature))
            .collect()
    }

    /// The feature whose result holds `solid` as `reader` sees it: the
    /// last of the solid's chain listed before `reader`. `None` when
    /// `solid` founds no solid, or `reader` isn't listed after its
    /// founder.
    pub fn view(&self, solid: &Id, reader: &Id) -> Option<&Id> {
        let me = *self.at.get(reader)?;
        self.chains
            .get(solid)?
            .iter()
            .take_while(|f| self.at[*f] < me)
            .last()
    }

    /// Every solid of the model, not consumed, with its tip, the last
    /// feature of its chain, in list order.
    pub fn tips(&self) -> impl Iterator<Item = (&Id, &Id)> {
        self.founders
            .iter()
            .filter(|s| !self.consumed.contains_key(*s))
            .map(|s| {
                (
                    s,
                    self.chains[s]
                        .last()
                        .expect("a chain starts at its founder"),
                )
            })
    }

    /// The solid's tip, if `founder` founds one.
    pub fn tip(&self, founder: &Id) -> Option<&Id> {
        self.chains.get(founder).and_then(|c| c.last())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::files::{Feature, Placement, Primitive, PrimitiveType};
    use crate::doc::project::Part;
    use crate::doc::solid::{Combine, Operation};
    use crate::units::{Quantity, Unit};
    use std::collections::BTreeSet;

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// A unit box at `x` mm, `operation`ing `targets`.
    pub(crate) fn boxed(x: &str, operation: Operation, targets: &[&str]) -> Feature {
        let q = |s: &str| Quantity::parse(s, Unit::Mm).unwrap();
        let mut placement = Placement::identity(Unit::Mm);
        placement.origin[0] = q(x);
        Feature::Primitive(Primitive {
            ty: PrimitiveType::Box,
            params: [("x", "1"), ("y", "1"), ("z", "1")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), q(v)))
                .collect(),
            placement,
            combine: Combine {
                operation,
                targets: targets.iter().map(|t| id(t)).collect(),
            },
        })
    }

    /// A part listing `features` in order.
    pub(crate) fn part(features: Vec<(&str, Feature)>) -> Part {
        let order: Vec<Id> = features.iter().map(|(i, _)| id(i)).collect();
        Part {
            id: id("p"),
            file: Ok(crate::doc::files::PartFile::new(order)),
            parameters: None,
            env: crate::expr::Env::empty(),
            features: features.into_iter().map(|(i, f)| (id(i), Ok(f))).collect(),
            uses: BTreeMap::<Id, BTreeSet<String>>::new(),
            revision: String::new(),
        }
    }

    /// A boolean of `op` on `target` with `tools`.
    pub(crate) fn boolean(
        op: crate::doc::solid::BooleanOp,
        target: &str,
        tools: &[&str],
    ) -> Feature {
        Feature::Boolean(crate::doc::solid::Boolean {
            op,
            target: id(target),
            tools: tools.iter().map(|t| id(t)).collect(),
        })
    }

    #[test]
    fn a_boolean_consumes_its_tools() {
        use crate::doc::solid::BooleanOp::Subtract;
        use Operation::*;
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("t", boxed("0.5", New, &[])),
            ("u", boxed("0.2", New, &[])),
            ("cut", boolean(Subtract, "a", &["t", "u"])),
            ("after", boxed("0", Join, &["t"])),
            ("more", boxed("2", Join, &["a"])),
        ]);
        let s = Solids::of(&p);
        assert_eq!(s.chain(&id("a")).unwrap(), [id("a"), id("cut"), id("more")]);
        // A consumed solid's chain ends; nothing joins it after.
        assert_eq!(s.chain(&id("t")).unwrap(), [id("t")]);
        assert_eq!(s.consumed_by(&id("t"), &id("after")), Some(&id("cut")));
        assert_eq!(s.consumed_by(&id("t"), &id("cut")), None);
        assert_eq!(s.consumer(&id("u")), Some(&id("cut")));
        assert_eq!(s.view(&id("t"), &id("cut")), Some(&id("t")));
        // Only what remains is the model.
        assert_eq!(s.tips().collect::<Vec<_>>(), [(&id("a"), &id("more"))]);
    }

    #[test]
    fn chains_views_and_tips() {
        use Operation::*;
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("b", boxed("5", New, &[])),
            ("j", boxed("0.5", Join, &["a"])),
            ("c", boxed("0.5", Cut, &["a", "b"])),
            ("early", boxed("0", Cut, &["late"])),
            ("late", boxed("9", New, &[])),
        ]);
        let s = Solids::of(&p);
        assert_eq!(s.chain(&id("a")).unwrap(), [id("a"), id("j"), id("c")]);
        assert_eq!(s.chain(&id("b")).unwrap(), [id("b"), id("c")]);
        // A target founded later isn't joined.
        assert_eq!(s.chain(&id("late")).unwrap(), [id("late")]);
        assert!(s.is_solid(&id("a")) && !s.is_solid(&id("j")));
        assert_eq!(s.solids_of(&id("c")), [&id("a"), &id("b")]);
        assert_eq!(s.solids_of(&id("a")), [&id("a")]);
        // Each reader sees the solid as the features before it left it.
        assert_eq!(s.view(&id("a"), &id("j")), Some(&id("a")));
        assert_eq!(s.view(&id("a"), &id("c")), Some(&id("j")));
        assert_eq!(s.view(&id("a"), &id("late")), Some(&id("c")));
        assert_eq!(s.view(&id("b"), &id("c")), Some(&id("b")));
        // Not a solid, or not after its founder.
        assert_eq!(s.view(&id("j"), &id("c")), None);
        assert_eq!(s.view(&id("late"), &id("early")), None);
        assert_eq!(s.view(&id("a"), &id("a")), None);
        assert_eq!(
            s.tips().collect::<Vec<_>>(),
            [
                (&id("a"), &id("c")),
                (&id("b"), &id("c")),
                (&id("late"), &id("late"))
            ]
        );
        // As a write would leave the list.
        let moved = [id("a"), id("c"), id("b"), id("j")];
        let s = Solids::of_order(&p, &moved);
        assert_eq!(s.chain(&id("a")).unwrap(), [id("a"), id("c"), id("j")]);
        assert_eq!(s.chain(&id("b")).unwrap(), [id("b")]);
    }
}
