//! A part's solids. A feature that founds a solid (a body-making feature
//! whose operation is `new`, or a split keeping both sides) names it by
//! its own ID; every feature listed after it that names it as a
//! target changes it, in list order: the solid's chain. A feature's view
//! of a solid is the last of its chain listed before the feature: what it
//! reads the solid's faces, edges and vertices from. The solid's tip, the
//! last of its chain, is what the part's model shows. A feature that
//! consumes bodies (a `boolean` its tools, a `sew` its sheets, a `thicken`
//! its sheet) ends their chains: they are no longer part of the model. A split joins its target's chain
//! and founds the side in front of its plane, whose chain starts at the
//! split.

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
    /// Each consumed body, by founder, with the feature that consumed it.
    consumed: BTreeMap<Id, Id>,
    /// The solids each feature of a chain belongs to, in list order.
    member: BTreeMap<Id, Vec<Id>>,
    /// The inserts in list order: each founds every key below it.
    families: Vec<Id>,
    /// Each insert's implicit chain, `[insert]`, by insert.
    family_chain: BTreeMap<Id, Vec<Id>>,
    /// Each deleted solid (a key, or an insert's ID for its whole family)
    /// with the `delete_body` that deleted it.
    deleted: BTreeMap<Id, Id>,
}

impl Solids {
    /// The solids of the part's list.
    pub fn of(part: &Part) -> Solids {
        Solids::of_order(part, part.order())
    }

    /// The solids of `order`, a list of the part's features. A feature
    /// whose file doesn't load acts as its text says (nothing, if it
    /// isn't JSON); one naming a solid founded
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
        let mut families: Vec<Id> = Vec::new();
        let mut family_chain: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        let mut deleted: BTreeMap<Id, Id> = BTreeMap::new();
        for id in order {
            // A feature that doesn't load acts as its text says, so what
            // follows it in a chain reads it and fails through it.
            type Facts<'a> = (bool, bool, Vec<&'a Id>, Vec<&'a Id>, Vec<&'a Id>);
            let (founds, inserts, acts_on, consumes, deletes): Facts = match part.features.get(id) {
                Some(Ok(f)) => {
                    // What it consumes isn't in a chain it joins.
                    let consumes = f.consumes();
                    let acts_on = f
                        .targets()
                        .into_iter()
                        .filter(|t| !consumes.contains(t))
                        .collect();
                    (
                        f.founds(),
                        f.inserts(),
                        acts_on,
                        consumes,
                        f.deletes().iter().collect(),
                    )
                }
                Some(Err(_)) => match part.acts.get(id) {
                    Some(a) => (
                        a.founds,
                        a.inserts,
                        a.acts_on.iter().collect(),
                        a.consumes.iter().collect(),
                        a.deletes.iter().collect(),
                    ),
                    None => continue,
                },
                None => continue,
            };
            // A key below an insert listed before this feature joins the
            // family: its chain starts at the insert (design 521).
            for t in acts_on.iter().chain(&consumes) {
                if t.is_path()
                    && !chains.contains_key(*t)
                    && families.iter().any(|f| f.as_str() == t.founder_str())
                {
                    chains.insert((*t).clone(), vec![t.founder()]);
                }
            }
            // Consumed or deleted: the key, or its insert's whole family.
            let gone = |t: &Id, consumed: &BTreeMap<Id, Id>, deleted: &BTreeMap<Id, Id>| {
                consumed.contains_key(t)
                    || deleted.contains_key(t)
                    || (t.is_path() && deleted.contains_key(t.founder_str()))
            };
            // A solid joins the chain of what acts on it, until consumed.
            for t in acts_on {
                if !gone(t, &consumed, &deleted)
                    && let Some(chain) = chains.get_mut(t)
                {
                    chain.push(id.clone());
                }
            }
            for t in consumes {
                if chains.contains_key(t) && !gone(t, &consumed, &deleted) {
                    consumed.insert(t.clone(), id.clone());
                }
            }
            for t in deletes {
                if !deleted.contains_key(t) {
                    deleted.insert(t.clone(), id.clone());
                }
            }
            if inserts {
                families.push(id.clone());
                family_chain.insert(id.clone(), vec![id.clone()]);
            }
            // A split acts on its target and founds the side in front: the
            // only feature that does both. Every other founder acts on
            // nothing, so its chain is as before.
            if founds {
                chains.insert(id.clone(), vec![id.clone()]);
                founders.push(id.clone());
            }
        }
        // Each feature's solids, so asking for them doesn't walk every
        // chain (issue #25: a list of thousands asked for each feature).
        // An insert belongs to its family, and the features acting on a
        // key below it to that key.
        let mut member: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        let paths = chains.keys().filter(|k| k.is_path());
        for f in founders.iter().chain(paths) {
            for id in &chains[f] {
                let list = member.entry(id.clone()).or_default();
                if list.last() != Some(f) {
                    list.push(f.clone());
                }
            }
        }
        for f in &families {
            let list = member.entry(f.clone()).or_default();
            if !list.contains(f) {
                list.insert(0, f.clone());
            }
        }
        Solids {
            chains,
            founders,
            at,
            consumed,
            member,
            families,
            family_chain,
            deleted,
        }
    }

    /// The feature that consumed `solid` before `reader`, if one did.
    pub fn consumed_by(&self, solid: &Id, reader: &Id) -> Option<&Id> {
        let by = self.consumed.get(solid)?;
        (self.at.get(by)? < self.at.get(reader)?).then_some(by)
    }

    /// The feature that consumed `solid`, if one did.
    pub fn consumer(&self, solid: &Id) -> Option<&Id> {
        self.consumed.get(solid)
    }

    /// The `delete_body` that deleted `solid` before `reader`, if one
    /// did: the key itself, or its insert's whole family.
    pub fn deleted_by(&self, solid: &Id, reader: &Id) -> Option<&Id> {
        let by = self.deleted.get(solid).or_else(|| {
            solid
                .is_path()
                .then(|| self.deleted.get(solid.founder_str()))
                .flatten()
        })?;
        (self.at.get(by)? < self.at.get(reader)?).then_some(by)
    }

    /// The `delete_body` that deleted `solid`, if one did.
    pub fn deleter(&self, solid: &Id) -> Option<&Id> {
        self.deleted.get(solid).or_else(|| {
            solid
                .is_path()
                .then(|| self.deleted.get(solid.founder_str()))
                .flatten()
        })
    }

    /// The insert whose family holds `key`, if `key` is below one.
    pub fn founder_of(&self, key: &Id) -> Option<&Id> {
        self.family_of(key)
    }

    /// The insert that founds `key`'s family, if `key` is below one.
    fn family_of(&self, key: &Id) -> Option<&Id> {
        if !key.is_path() {
            return None;
        }
        self.families
            .iter()
            .find(|f| f.as_str() == key.founder_str())
    }

    /// Whether `id` is a solid's key: a founder's, or one below an
    /// insert.
    pub fn is_solid(&self, id: &Id) -> bool {
        self.chains.contains_key(id) || self.family_of(id).is_some()
    }

    /// The solid keyed `founder`: its chain, founder first (an insert
    /// first for a key below it).
    pub fn chain(&self, founder: &Id) -> Option<&[Id]> {
        match self.chains.get(founder) {
            Some(c) => Some(c),
            None => self
                .family_chain
                .get(self.family_of(founder)?)
                .map(Vec::as_slice),
        }
    }

    /// The solids `feature` belongs to: its own, if it founds one, else
    /// those it acts on, in list order.
    pub fn solids_of(&self, feature: &Id) -> Vec<&Id> {
        self.member
            .get(feature)
            .map_or_else(Vec::new, |l| l.iter().collect())
    }

    /// The feature whose result holds `solid` as `reader` sees it: the
    /// last of the solid's chain listed before `reader`. `None` when
    /// `solid` founds no solid, or `reader` isn't listed after its
    /// founder.
    pub fn view(&self, solid: &Id, reader: &Id) -> Option<&Id> {
        let me = *self.at.get(reader)?;
        self.chain(solid)?
            .iter()
            .take_while(|f| self.at[*f] < me)
            .last()
    }

    /// Every solid of the model, not consumed or deleted, with its tip, the last
    /// feature of its chain, in list order.
    pub fn tips(&self) -> impl Iterator<Item = (&Id, &Id)> {
        self.founders
            .iter()
            .filter(|s| !self.consumed.contains_key(*s) && self.deleter(s).is_none())
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
        self.chain(founder).and_then(|c| c.last())
    }

    /// The inserts whose families the model shows, in list order: not
    /// deleted whole.
    pub fn families(&self) -> impl Iterator<Item = &Id> {
        self.families
            .iter()
            .filter(|f| !self.deleted.contains_key(*f))
    }

    /// The keys of `insert`'s family a feature acted on, each with its
    /// chain (insert first), in key order.
    pub fn family_chains<'a>(&'a self, insert: &'a Id) -> impl Iterator<Item = (&'a Id, &'a [Id])> {
        self.chains
            .iter()
            .filter(move |(k, _)| k.is_path() && k.founder_str() == insert.as_str())
            .map(|(k, c)| (k, c.as_slice()))
    }

    /// Whether `key` is deleted, whole or with its family.
    pub fn is_deleted(&self, key: &Id) -> bool {
        self.deleter(key).is_some()
    }
}

/// One thing the model shows: a founder's solid with its tip, or an
/// insert's family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tip<'a> {
    Solid(&'a Id, &'a Id),
    Family(&'a Id),
}

impl Solids {
    /// [`Solids::tips`] and [`Solids::families`] together, in list order.
    pub fn shown_in_order(&self) -> Vec<Tip<'_>> {
        let mut out: Vec<(usize, Tip<'_>)> = self
            .tips()
            .map(|(s, t)| {
                (
                    self.at.get(s).copied().unwrap_or(usize::MAX),
                    Tip::Solid(s, t),
                )
            })
            .collect();
        if self.families.is_empty() {
            return out.into_iter().map(|(_, t)| t).collect();
        }
        out.extend(self.families().map(|f| {
            (
                self.at.get(f).copied().unwrap_or(usize::MAX),
                Tip::Family(f),
            )
        }));
        out.sort_by_key(|(at, _)| *at);
        out.into_iter().map(|(_, t)| t).collect()
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
                targets: targets
                    .iter()
                    .map(|t| Id::solid_key(t, "feature").unwrap())
                    .collect(),
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
            project_reads: BTreeSet::new(),
            hole_standards: None,
            features: features.into_iter().map(|(i, f)| (id(i), Ok(f))).collect(),
            uses: BTreeMap::<Id, BTreeSet<String>>::new(),
            acts: BTreeMap::new(),
            imports: BTreeMap::new(),
            views: BTreeMap::new(),
            appearance: None,
            explodes: Default::default(),
            explode_uses: Default::default(),
            motions: Default::default(),
            motion_uses: Default::default(),
            drawings: Default::default(),
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

    /// An insert of `of`, as a feature that didn't load: `Solids` reads
    /// what it does from its JSON alone.
    pub(crate) fn insert_acts(p: &mut Part, id_: &str) {
        let a = crate::doc::files::Acts {
            inserts: true,
            ..Default::default()
        };
        p.features.insert(
            id(id_),
            Err(crate::error::Error::new(crate::error::Code::Internal, "x")),
        );
        p.acts.insert(id(id_), a);
    }

    pub(crate) fn delete_acts(p: &mut Part, id_: &str, solids: &[&str]) {
        let deletes = solids
            .iter()
            .map(|s| Id::solid_key(s, "feature").unwrap())
            .collect();
        let a = crate::doc::files::Acts {
            deletes,
            ..Default::default()
        };
        p.features.insert(
            id(id_),
            Err(crate::error::Error::new(crate::error::Code::Internal, "x")),
        );
        p.acts.insert(id(id_), a);
    }

    fn key(s: &str) -> Id {
        Id::solid_key(s, "feature").unwrap()
    }

    #[test]
    fn an_insert_founds_a_family_and_a_delete_ends_chains() {
        use Operation::*;
        let mut p = part(vec![
            ("base", boxed("0", New, &[])),
            ("board", boxed("9", New, &[])), // replaced by an insert below
            ("cut", boxed("0", Cut, &["base", "board/u1"])),
            ("drop", boxed("9", New, &[])), // replaced by a delete below
            ("after", boxed("0", Join, &["base"])),
        ]);
        insert_acts(&mut p, "board");
        delete_acts(&mut p, "drop", &["board"]);
        let s = Solids::of(&p);
        // Any key below the insert is its, chain [insert] until acted on.
        assert!(s.is_solid(&key("board/u2")));
        assert_eq!(s.chain(&key("board/u2")), Some(&[id("board")][..]));
        assert_eq!(s.view(&key("board/u2"), &id("cut")), Some(&id("board")));
        // The cut acts on u1: its chain is [board, cut].
        assert_eq!(
            s.chain(&key("board/u1")),
            Some(&[id("board"), id("cut")][..])
        );
        assert_eq!(s.view(&key("board/u1"), &id("after")), Some(&id("cut")));
        // A key below something that isn't an insert is no solid.
        assert!(!s.is_solid(&key("base/x")));
        // The delete of the insert deletes the whole family, after it only.
        assert_eq!(
            s.deleted_by(&key("board/u1"), &id("after")),
            Some(&id("drop"))
        );
        assert_eq!(
            s.deleted_by(&key("board/u2"), &id("after")),
            Some(&id("drop"))
        );
        assert_eq!(s.deleted_by(&key("board/u1"), &id("cut")), None);
        // The model shows base, and the family no more.
        let tips: Vec<(&str, &str)> = s.tips().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        assert_eq!(tips, [("base", "after")]);
        assert!(s.families().next().is_none());
        // Without the delete the family is shown, with its acted-on chain.
        let mut q = p.clone();
        q.file = Ok(crate::doc::files::PartFile::new(
            ["base", "board", "cut", "after"].map(id).to_vec(),
        ));
        let s = Solids::of(&q);
        assert_eq!(s.families().map(Id::as_str).collect::<Vec<_>>(), ["board"]);
        assert_eq!(
            s.family_chains(&id("board"))
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>(),
            ["board/u1"]
        );
    }

    #[test]
    fn a_delete_of_one_solid_leaves_the_rest() {
        use Operation::*;
        let mut p = part(vec![
            ("a", boxed("0", New, &[])),
            ("b", boxed("2", New, &[])),
            ("d", boxed("9", New, &[])),
            ("j", boxed("0", Join, &["a"])),
        ]);
        delete_acts(&mut p, "d", &["b"]);
        let s = Solids::of(&p);
        assert_eq!(s.deleted_by(&id("b"), &id("j")), Some(&id("d")));
        assert_eq!(s.deleter(&id("b")), Some(&id("d")));
        assert_eq!(s.chain(&id("b")), Some(&[id("b")][..]));
        let tips: Vec<&str> = s.tips().map(|(a, _)| a.as_str()).collect();
        assert_eq!(tips, ["a"]);
    }

    #[test]
    fn a_sew_and_a_thicken_from_a_sheet_consume_what_they_take() {
        let f = |v: serde_json::Value| {
            crate::doc::files::Feature::from_json(
                &v,
                crate::units::Unit::Mm,
                &crate::expr::Env::empty(),
            )
            .unwrap()
        };
        let n = crate::doc::surface::FORMAT;
        let p = part(vec![
            ("a", boxed("0", Operation::New, &[])),
            ("b", boxed("2", Operation::New, &[])),
            (
                "s",
                f(
                    serde_json::json!({"format": n, "kind": "ryucad.feature", "type": "sew",
                           "sheets": ["a", "b"], "tolerance": "0.01 mm", "solid": true}),
                ),
            ),
            (
                "t",
                f(
                    serde_json::json!({"format": n, "kind": "ryucad.feature", "type": "thicken",
                           "from": {"sheet": "s"}, "front": "1 mm", "back": "0 mm",
                           "operation": "new"}),
                ),
            ),
        ]);
        let solids = Solids::of(&p);
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        assert_eq!(solids.consumer(&id("a")), Some(&id("s")));
        assert_eq!(solids.consumer(&id("b")), Some(&id("s")));
        assert_eq!(solids.consumer(&id("s")), Some(&id("t")));
        assert_eq!(solids.chain(&id("s")), Some(&[id("s")][..]));
        assert_eq!(solids.chain(&id("t")), Some(&[id("t")][..]));
        // Each consumed body's chain ends where it was consumed.
        assert_eq!(solids.chain(&id("a")), Some(&[id("a")][..]));
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

    /// A split of `target` by the plane xy, keeping both sides or not.
    pub(crate) fn split(target: &str, keep: crate::doc::body::Keep) -> Feature {
        use crate::doc::geometry::GeomRef;
        use crate::doc::sketch::OriginPlane;
        Feature::Split(Box::new(crate::doc::body::Split {
            target: id(target),
            by: crate::doc::body::SplitBy::Plane(GeomRef::Plane(OriginPlane::Xy)),
            flip: false,
            keep,
        }))
    }

    #[test]
    fn a_split_acts_on_its_target_and_founds_the_side_in_front() {
        use crate::doc::body::Keep;
        use Operation::*;
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("s", split("a", Keep::Both)),
            ("ja", boxed("0.5", Join, &["a"])),
            ("js", boxed("0.5", Join, &["s"])),
            ("t", split("ja", Keep::Behind)),
        ]);
        let s = Solids::of(&p);
        assert_eq!(s.chain(&id("a")).unwrap(), [id("a"), id("s"), id("ja")]);
        // The side in front is a solid of its own, its chain from the split.
        assert_eq!(s.chain(&id("s")).unwrap(), [id("s"), id("js")]);
        assert!(s.is_solid(&id("s")));
        assert_eq!(s.solids_of(&id("s")), [&id("a"), &id("s")]);
        assert_eq!(s.view(&id("a"), &id("ja")), Some(&id("s")));
        assert_eq!(s.view(&id("s"), &id("js")), Some(&id("s")));
        // A split keeping one side founds nothing; `ja` isn't a solid.
        assert!(!s.is_solid(&id("t")));
        assert_eq!(
            s.tips().collect::<Vec<_>>(),
            [(&id("a"), &id("ja")), (&id("s"), &id("js"))]
        );
    }

    #[test]
    fn a_sew_that_doesnt_load_still_consumes_its_sheets() {
        use Operation::*;
        let mut p = part(vec![
            ("a", boxed("0", New, &[])),
            ("b", boxed("2", New, &[])),
            ("s", boxed("0", New, &[])),
            ("later", boxed("0.5", Cut, &["a"])),
        ]);
        let v = serde_json::json!({"format": crate::doc::surface::FORMAT, "kind": "ryucad.feature", "type": "sew",
                                   "sheets": ["a", "b"], "tolerance": "nope", "solid": true});
        p.acts.insert(id("s"), crate::doc::files::feature_acts(&v));
        p.features.insert(
            id("s"),
            Err(crate::error::Error::new(
                crate::error::Code::InvalidDocument,
                "a conflict",
            )),
        );
        let s = Solids::of(&p);
        assert_eq!(s.consumer(&id("a")), Some(&id("s")));
        assert_eq!(s.consumer(&id("b")), Some(&id("s")));
        // A cut after the sew naming a doesn't join a's chain.
        assert_eq!(s.chain(&id("a")).unwrap(), [id("a")]);
    }

    #[test]
    fn a_split_that_doesnt_load_acts_and_founds_as_its_text_says() {
        let mut p = part(vec![
            ("a", boxed("0", Operation::New, &[])),
            ("s", boxed("0", Operation::New, &[])),
        ]);
        let v = serde_json::json!({"format": 10, "kind": "ryucad.feature", "type": "split",
                                   "target": "a", "plane": {"feature": "gone"}});
        p.acts.insert(id("s"), crate::doc::files::feature_acts(&v));
        p.features.insert(
            id("s"),
            Err(crate::error::Error::new(
                crate::error::Code::InvalidDocument,
                "a conflict",
            )),
        );
        let s = Solids::of(&p);
        assert_eq!(s.chain(&id("a")).unwrap(), [id("a"), id("s")]);
        assert_eq!(s.chain(&id("s")).unwrap(), [id("s")]);
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
