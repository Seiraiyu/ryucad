//! A part's solids. A feature whose operation is `new` founds a solid,
//! named by its own ID; every feature listed after it that names it as a
//! target changes it, in list order: the solid's chain. A feature's view
//! of a solid is the last of its chain listed before the feature: what it
//! reads the solid's faces, edges and vertices from. The solid's tip, the
//! last of its chain, is what the part's model shows.

use crate::doc::id::Id;
use crate::doc::project::Part;
use std::collections::BTreeMap;

/// The solids of a part's list, as it is or as a write would leave it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Solids {
    /// Each solid's chain, founder first, by founder.
    chains: BTreeMap<Id, Vec<Id>>,
    /// Each listed feature's place in the list.
    at: BTreeMap<Id, usize>,
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
        for id in order {
            let Some(Ok(f)) = part.features.get(id) else {
                continue;
            };
            let Some(c) = f.combine() else { continue };
            if c.is_new() {
                chains.insert(id.clone(), vec![id.clone()]);
                continue;
            }
            for t in &c.targets {
                if let Some(chain) = chains.get_mut(t) {
                    chain.push(id.clone());
                }
            }
        }
        Solids { chains, at }
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
    /// those it acts on, by founder.
    pub fn solids_of(&self, feature: &Id) -> Vec<&Id> {
        self.chains
            .iter()
            .filter(|(_, c)| c.contains(feature))
            .map(|(s, _)| s)
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

    /// Every solid with its tip, the last feature of its chain, by
    /// founder.
    pub fn tips(&self) -> impl Iterator<Item = (&Id, &Id)> {
        self.chains
            .iter()
            .map(|(s, c)| (s, c.last().expect("a chain starts at its founder")))
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
            file: Ok(crate::doc::files::PartFile { features: order }),
            parameters: None,
            env: crate::expr::Env::empty(),
            features: features.into_iter().map(|(i, f)| (id(i), Ok(f))).collect(),
            uses: BTreeMap::<Id, BTreeSet<String>>::new(),
            revision: String::new(),
        }
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
