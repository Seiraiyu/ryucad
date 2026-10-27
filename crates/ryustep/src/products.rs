//! The product tree: which product definitions a file holds, the shapes
//! (B-rep items) each has, and where an assembly uses each child.
//!
//! A definition's shape is the representation its
//! `SHAPE_DEFINITION_REPRESENTATION` names, together with every
//! representation related to that one by a relationship without a
//! transformation (AP203 files put the B-rep in a second representation
//! that way). An assembly usage (`NEXT_ASSEMBLY_USAGE_OCCURRENCE`) is
//! placed by the `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` that names it:
//! its relationship's transformation, oriented by which side holds the
//! child's shape.
//!
//! Everything stays as the file wrote it: instance numbers and the file's
//! units. A fault in one usage or definition is collected in
//! [`Products::faults`] and leaves the rest of the tree.

use std::collections::{BTreeMap, BTreeSet};

use crate::ImportError;
use crate::schema::{Entities, Entity, Id};

/// A file's product tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Products {
    /// Every `PRODUCT_DEFINITION`, by instance number.
    pub definitions: Vec<Definition>,
    /// The definitions no usage places, as indices into `definitions`.
    pub roots: Vec<usize>,
    /// Representations holding B-rep or mapped items that no definition's
    /// shape reaches (a file without products, say).
    pub unplaced: Vec<Shape>,
    /// What couldn't be read: definitions, then usages, in instance order,
    /// then usages that closed a cycle, then unplaced representations.
    pub faults: Vec<ImportError>,
}

/// A product definition: a product as one view of one version of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    /// The `PRODUCT_DEFINITION`'s instance number.
    pub instance: Id,
    /// The `PRODUCT`'s identifier, name and description.
    pub product_id: Box<str>,
    pub name: Box<str>,
    pub description: Box<str>,
    /// Its `PRODUCT_DEFINITION_FORMATION`'s identifier: the revision,
    /// where the writer gave one (empty when unset).
    pub revision: Box<str>,
    /// Its representations that hold B-rep or mapped items.
    pub shapes: Vec<Shape>,
    /// Where it uses other definitions.
    pub children: Vec<Occurrence>,
}

/// A representation holding B-rep items, and representations mapped into
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub representation: Id,
    /// Its context, which gives its units.
    pub context: Id,
    /// Its `MANIFOLD_SOLID_BREP`s, `BREP_WITH_VOIDS`, `FACETED_BREP`s and
    /// `SHELL_BASED_SURFACE_MODEL`s.
    pub breps: Vec<Id>,
    pub mapped: Vec<Mapped>,
}

/// A `MAPPED_ITEM`: `representation`, its placement `origin` moved onto
/// `target` in the representation holding the item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mapped {
    pub item: Id,
    pub representation: Id,
    pub origin: Id,
    pub target: Id,
}

/// One use of a child definition in its parent.
#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    /// The `NEXT_ASSEMBLY_USAGE_OCCURRENCE`'s instance number, identifier
    /// and name.
    pub instance: Id,
    pub id: Box<str>,
    pub name: Box<str>,
    /// The child, an index into [`Products::definitions`].
    pub definition: usize,
    /// Where the child goes; `None` if the file says nothing (in place).
    pub transform: Option<Transform>,
    /// The representations `transform` relates, the child's first: their
    /// contexts give its units. `None` with no transform.
    pub representations: Option<[Id; 2]>,
}

/// How a child is placed in its parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transform {
    /// The child's axis placement `child` (in the child's shape) goes onto
    /// `parent` (in the parent's): an `ITEM_DEFINED_TRANSFORMATION`, with
    /// its items swapped if the file related the representations the
    /// other way round.
    Items { child: Id, parent: Id },
    /// A `CARTESIAN_TRANSFORMATION_OPERATOR_3D` taking the child into the
    /// parent, or the parent into the child if `inverse`.
    Operator { operator: Id, inverse: bool },
}

/// What one scan of the entities finds.
#[derive(Default)]
struct Scan {
    definitions: Vec<Id>,
    /// Product definition shape → what it is the shape of.
    shape_of: BTreeMap<Id, Id>,
    /// Shape definition representations: (shape, representation).
    shape_reps: Vec<(Id, Id)>,
    usages: Vec<Id>,
    /// Context dependent shape representations: (product shape,
    /// relationship).
    placements: Vec<(Id, Id)>,
    /// Relationships without a transformation: (relationship, rep_1,
    /// rep_2).
    plain: Vec<(Id, Id, Id)>,
    representations: Vec<Id>,
}

impl Products {
    /// The product tree of `entities`.
    pub fn build(e: &Entities) -> Products {
        let mut s = Scan::default();
        for &id in e.ids() {
            match e.get(id, id) {
                Ok(Entity::Definition { .. }) => s.definitions.push(id),
                Ok(Entity::DefinitionShape { definition }) => {
                    s.shape_of.insert(id, *definition);
                }
                Ok(Entity::ShapeDefinition {
                    definition,
                    representation,
                }) => s.shape_reps.push((*definition, *representation)),
                Ok(Entity::Usage { .. }) => s.usages.push(id),
                Ok(Entity::ContextDependentShape {
                    relationship,
                    product_shape,
                }) => s.placements.push((*product_shape, *relationship)),
                Ok(Entity::RepresentationRelationship {
                    rep_1,
                    rep_2,
                    transformation: None,
                }) => s.plain.push((id, *rep_1, *rep_2)),
                Ok(Entity::Representation { .. }) => s.representations.push(id),
                _ => {}
            }
        }
        let mut p = Products::default();
        let used: BTreeSet<Id> = s.placements.iter().map(|&(_, r)| r).collect();
        let mut linked: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        for &(rel, a, b) in &s.plain {
            if !used.contains(&rel) {
                linked.entry(a).or_default().push(b);
                linked.entry(b).or_default().push(a);
            }
        }
        let index: BTreeMap<Id, usize> = s
            .definitions
            .iter()
            .enumerate()
            .map(|(k, &d)| (d, k))
            .collect();
        let mut groups: Vec<BTreeSet<Id>> = vec![BTreeSet::new(); s.definitions.len()];
        for &(shape, rep) in &s.shape_reps {
            if let Some(k) = s.shape_of.get(&shape).and_then(|d| index.get(d)) {
                let mut stack = vec![rep];
                while let Some(r) = stack.pop() {
                    if groups[*k].insert(r) {
                        stack.extend(linked.get(&r).into_iter().flatten());
                    }
                }
            }
        }
        for (k, &d) in s.definitions.iter().enumerate() {
            let (product_id, name, description, revision) = match product_of(e, d) {
                Ok(names) => names,
                Err(f) => {
                    p.faults.push(f);
                    ("".into(), "".into(), "".into(), "".into())
                }
            };
            let mut shapes = Vec::new();
            for &rep in &groups[k] {
                match shape(e, d, rep, &mut p.faults) {
                    Ok(Some(sh)) => shapes.push(sh),
                    Ok(None) => {}
                    Err(f) => p.faults.push(f),
                }
            }
            p.definitions.push(Definition {
                instance: d,
                product_id,
                name,
                description,
                revision,
                shapes,
                children: Vec::new(),
            });
        }
        let placed_by: BTreeMap<Id, Id> = s
            .placements
            .iter()
            .rev()
            .filter_map(|&(shape, rel)| s.shape_of.get(&shape).map(|&u| (u, rel)))
            .collect();
        for &u in &s.usages {
            match occurrence(e, u, &index, &groups, placed_by.get(&u).copied()) {
                Ok((parent, o)) => p.definitions[parent].children.push(o),
                Err(f) => p.faults.push(f),
            }
        }
        p.break_cycles();
        let mut placed = vec![false; p.definitions.len()];
        for d in &p.definitions {
            for o in &d.children {
                placed[o.definition] = true;
            }
        }
        p.roots = (0..p.definitions.len()).filter(|&k| !placed[k]).collect();
        let mut faults = std::mem::take(&mut p.faults);
        p.unplaced = unplaced(e, &p, &s.representations, &mut faults);
        p.faults = faults;
        p
    }

    /// Drops every usage that closes a cycle, found by a depth-first walk
    /// from each definition in turn, reporting it as `ReferenceCycle`.
    fn break_cycles(&mut self) {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            New,
            Open,
            Done,
        }
        let n = self.definitions.len();
        let mut mark = vec![Mark::New; n];
        for start in 0..n {
            if mark[start] != Mark::New {
                continue;
            }
            // (definition, next child to look at)
            let mut stack = vec![(start, 0)];
            mark[start] = Mark::Open;
            while let Some(&mut (d, ref mut next)) = stack.last_mut() {
                let children = &self.definitions[d].children;
                if *next == children.len() {
                    mark[d] = Mark::Done;
                    stack.pop();
                    continue;
                }
                let c = children[*next].definition;
                match mark[c] {
                    Mark::New => {
                        *next += 1;
                        mark[c] = Mark::Open;
                        stack.push((c, 0));
                    }
                    Mark::Open => {
                        let o = self.definitions[d].children.remove(*next);
                        self.faults.push(ImportError::ReferenceCycle {
                            instance: o.instance,
                        });
                    }
                    Mark::Done => *next += 1,
                }
            }
        }
    }
}

/// A product's identifier, name and description, and its version's
/// identifier.
type ProductNames = (Box<str>, Box<str>, Box<str>, Box<str>);

/// A definition's product identifier, name and description, and its
/// formation's identifier.
fn product_of(e: &Entities, d: Id) -> Result<ProductNames, ImportError> {
    let Entity::Definition { formation, .. } = e.get(d, d)? else {
        unreachable!("a definition")
    };
    let (revision, product) = match e.get(d, *formation)? {
        Entity::Formation { id, product } => (id.clone(), *product),
        _ => {
            return Err(ImportError::WrongEntity {
                instance: *formation,
                expected: "a PRODUCT_DEFINITION_FORMATION",
            });
        }
    };
    match e.get(*formation, product)? {
        Entity::Product {
            id,
            name,
            description,
        } => Ok((id.clone(), name.clone(), description.clone(), revision)),
        _ => Err(ImportError::WrongEntity {
            instance: product,
            expected: "a PRODUCT",
        }),
    }
}

/// Representation `rep`'s B-rep and mapped items, or `None` if it has
/// neither. An item that can't be read is a fault, and the others are
/// still found.
fn shape(
    e: &Entities,
    from: Id,
    rep: Id,
    faults: &mut Vec<ImportError>,
) -> Result<Option<Shape>, ImportError> {
    let (items, context) = match e.get(from, rep)? {
        Entity::Representation { items, context, .. } => (items, *context),
        _ => {
            return Err(ImportError::WrongEntity {
                instance: rep,
                expected: "a representation",
            });
        }
    };
    let mut breps = Vec::new();
    let mut mapped = Vec::new();
    for &item in items {
        match e.get(rep, item) {
            Ok(Entity::Brep(_)) => breps.push(item),
            Ok(Entity::MappedItem { source, target }) => match e.get(item, *source) {
                Ok(Entity::RepresentationMap {
                    origin,
                    representation,
                }) => mapped.push(Mapped {
                    item,
                    representation: *representation,
                    origin: *origin,
                    target: *target,
                }),
                Ok(_) => faults.push(ImportError::WrongEntity {
                    instance: *source,
                    expected: "a REPRESENTATION_MAP",
                }),
                Err(f) => faults.push(f),
            },
            Ok(_) => {}
            Err(f) => faults.push(f),
        }
    }
    if breps.is_empty() && mapped.is_empty() {
        return Ok(None);
    }
    Ok(Some(Shape {
        representation: rep,
        context,
        breps,
        mapped,
    }))
}

/// Usage `u` as an occurrence of its child, with its parent's index.
fn occurrence(
    e: &Entities,
    u: Id,
    index: &BTreeMap<Id, usize>,
    groups: &[BTreeSet<Id>],
    relationship: Option<Id>,
) -> Result<(usize, Occurrence), ImportError> {
    let Entity::Usage {
        id,
        name,
        parent,
        child,
    } = e.get(u, u)?
    else {
        unreachable!("a usage")
    };
    let definition = |d: Id| {
        e.get(u, d)?;
        index.get(&d).copied().ok_or(ImportError::WrongEntity {
            instance: d,
            expected: "a PRODUCT_DEFINITION",
        })
    };
    let (p, c) = (definition(*parent)?, definition(*child)?);
    let (transform, representations) = match relationship {
        None => (None, None),
        Some(rel) => match transform(e, rel, &groups[p], &groups[c])? {
            Some((t, reps)) => (Some(t), Some(reps)),
            None => (None, None),
        },
    };
    Ok((
        p,
        Occurrence {
            instance: u,
            id: id.clone(),
            name: name.clone(),
            definition: c,
            transform,
            representations,
        },
    ))
}

/// The transformation relationship `rel` gives, oriented child to parent,
/// with the representations it relates, the child's first.
fn transform(
    e: &Entities,
    rel: Id,
    parent: &BTreeSet<Id>,
    child: &BTreeSet<Id>,
) -> Result<Option<(Transform, [Id; 2])>, ImportError> {
    let (rep_1, rep_2, t) = match e.get(rel, rel)? {
        Entity::RepresentationRelationship {
            rep_1,
            rep_2,
            transformation,
        } => (*rep_1, *rep_2, *transformation),
        _ => {
            return Err(ImportError::WrongEntity {
                instance: rel,
                expected: "a representation relationship",
            });
        }
    };
    let Some(t) = t else {
        return Ok(None);
    };
    // rep_1 is the child's by AP214's recommended practice; some writers
    // relate them the other way.
    let inverse = parent.contains(&rep_1) && !child.contains(&rep_1);
    let reps = if inverse {
        [rep_2, rep_1]
    } else {
        [rep_1, rep_2]
    };
    let t = match e.get(rel, t)? {
        Entity::ItemTransformation { item_1, item_2 } => {
            if inverse {
                Transform::Items {
                    child: *item_2,
                    parent: *item_1,
                }
            } else {
                Transform::Items {
                    child: *item_1,
                    parent: *item_2,
                }
            }
        }
        Entity::Operator { .. } => Transform::Operator {
            operator: t,
            inverse,
        },
        _ => {
            return Err(ImportError::WrongEntity {
                instance: t,
                expected: "a transformation",
            });
        }
    };
    Ok(Some((t, reps)))
}

/// Representations with B-rep or mapped items that no definition's shape
/// reaches, directly or through mapped items.
fn unplaced(
    e: &Entities,
    p: &Products,
    representations: &[Id],
    faults: &mut Vec<ImportError>,
) -> Vec<Shape> {
    let mut reached = BTreeSet::new();
    let mut stack: Vec<Id> = p
        .definitions
        .iter()
        .flat_map(|d| &d.shapes)
        .map(|s| s.representation)
        .collect();
    while let Some(r) = stack.pop() {
        if !reached.insert(r) {
            continue;
        }
        // Its faults were reported with the definition that reached it.
        if let Ok(Some(sh)) = shape(e, r, r, &mut Vec::new()) {
            stack.extend(sh.mapped.iter().map(|m| m.representation));
        }
    }
    let mut out = Vec::new();
    for &r in representations {
        if reached.contains(&r) {
            continue;
        }
        match shape(e, r, r, faults) {
            Ok(Some(sh)) => out.push(sh),
            Ok(None) => {}
            Err(f) => faults.push(f),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests;

    /// Placements `#8` and `#20` to `#22`, and context `#9`, for the
    /// representations below to name.
    fn entities(data: &str) -> Entities {
        tests::entities(&format!(
            "#6=CARTESIAN_POINT('',(0.,0.,0.));#8=AXIS2_PLACEMENT_3D('',#6,$,$);\
             #9=(GEOMETRIC_REPRESENTATION_CONTEXT(3)REPRESENTATION_CONTEXT('',''));\
             #20=AXIS2_PLACEMENT_3D('',#6,$,$);#21=AXIS2_PLACEMENT_3D('',#6,$,$);\
             #22=CARTESIAN_POINT('',(1.,0.,0.));{data}"
        ))
    }

    /// Product definition `#d` (its formation, product, shape and shape
    /// definition `#d+1` to `#d+4`) whose shape is representation `rep`,
    /// holding solid `brep`, in context `#9`.
    fn part(d: Id, rep: Id, brep: Id) -> String {
        format!(
            "#{d}=PRODUCT_DEFINITION('design','',#{f},$);#{f}=PRODUCT_DEFINITION_FORMATION('','',#{p});\
             #{p}=PRODUCT('p{d}','Part {d}','about {d}',());#{s}=PRODUCT_DEFINITION_SHAPE('','',#{d});\
             #{r}=SHAPE_DEFINITION_REPRESENTATION(#{s},#{rep});\
             #{rep}=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#8,#{brep}),#9);\
             #{brep}=MANIFOLD_SOLID_BREP('',#7);",
            f = d + 1,
            p = d + 2,
            s = d + 3,
            r = d + 4,
        )
    }

    /// Usage `#u` of `child` in `parent`, placed by a relationship of
    /// `rep_1` and `rep_2` with transformation `#t`.
    fn usage(u: Id, parent: Id, child: Id, rep_1: Id, rep_2: Id, t: &str) -> String {
        format!(
            "#{u}=NEXT_ASSEMBLY_USAGE_OCCURRENCE('{u}','use {u}','',#{parent},#{child},$);\
             #{s}=PRODUCT_DEFINITION_SHAPE('','',#{u});\
             #{c}=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{rel},#{s});\
             #{rel}=(REPRESENTATION_RELATIONSHIP('','',#{rep_1},#{rep_2})\
             REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{tid})\
             SHAPE_REPRESENTATION_RELATIONSHIP());#{tid}={t};",
            s = u + 1,
            c = u + 2,
            rel = u + 3,
            tid = u + 4,
        )
    }

    #[test]
    fn a_part_alone() {
        let p = Products::build(&entities(&part(100, 110, 111)));
        assert_eq!(p.faults, []);
        assert_eq!(p.roots, [0]);
        let d = &p.definitions[0];
        assert_eq!(
            (d.instance, &*d.product_id, &*d.name, &*d.description),
            (100, "p100", "Part 100", "about 100")
        );
        assert_eq!(
            d.shapes,
            [Shape {
                representation: 110,
                context: 9,
                breps: vec![111],
                mapped: vec![]
            }]
        );
        assert_eq!(p.unplaced, []);
    }

    #[test]
    fn an_assembly() {
        // The assembly's own representation holds only placements.
        let data = format!(
            "{}{}#130=SHAPE_REPRESENTATION('',(#8),#9);\
             #300=PRODUCT_DEFINITION('design','',#301,$);\
             #301=PRODUCT_DEFINITION_FORMATION('','',#302);#302=PRODUCT('asm','Asm','',());\
             #303=PRODUCT_DEFINITION_SHAPE('','',#300);\
             #304=SHAPE_DEFINITION_REPRESENTATION(#303,#130);\
             {}{}{}",
            part(100, 110, 111),
            part(200, 210, 211),
            usage(
                400,
                300,
                100,
                110,
                130,
                "ITEM_DEFINED_TRANSFORMATION('','',#8,#20)"
            ),
            usage(
                410,
                300,
                100,
                130,
                110,
                "ITEM_DEFINED_TRANSFORMATION('','',#21,#8)"
            ),
            usage(
                420,
                300,
                200,
                210,
                130,
                "CARTESIAN_TRANSFORMATION_OPERATOR_3D('',$,$,#22,$,$)"
            ),
        );
        let p = Products::build(&entities(&data));
        assert_eq!(p.faults, []);
        let names: Vec<&str> = p.definitions.iter().map(|d| &*d.name).collect();
        assert_eq!(names, ["Part 100", "Part 200", "Asm"]);
        assert_eq!(p.roots, [2]);
        let asm = &p.definitions[2];
        assert_eq!(asm.shapes, []);
        let placed: Vec<(usize, Option<Transform>)> = asm
            .children
            .iter()
            .map(|o| (o.definition, o.transform))
            .collect();
        assert_eq!(
            placed,
            [
                (
                    0,
                    Some(Transform::Items {
                        child: 8,
                        parent: 20
                    })
                ),
                // Related the other way round: the items swap.
                (
                    0,
                    Some(Transform::Items {
                        child: 8,
                        parent: 21
                    })
                ),
                (
                    1,
                    Some(Transform::Operator {
                        operator: 424,
                        inverse: false
                    })
                ),
            ]
        );
        assert_eq!(&*asm.children[0].name, "use 400");
    }

    #[test]
    fn related_representations_and_mapped_items() {
        // AP203 style: the definition's representation is related, without
        // a transformation, to the one holding the B-rep, which also maps
        // another representation in.
        let data = format!(
            "{}#120=SHAPE_REPRESENTATION('',(#8),#9);\
             #121=SHAPE_REPRESENTATION_RELATIONSHIP('','',#120,#110);\
             #122=MAPPED_ITEM('',#123,#20);#123=REPRESENTATION_MAP(#8,#124);\
             #124=SHAPE_REPRESENTATION('',(#125),#9);#125=MANIFOLD_SOLID_BREP('',#7);\
             #126=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#127),#9);\
             #127=FACETED_BREP('',#7);",
            part(100, 110, 111)
                .replace(
                    "SHAPE_DEFINITION_REPRESENTATION(#103,#110)",
                    "SHAPE_DEFINITION_REPRESENTATION(#103,#120)"
                )
                .replace("(#8,#111)", "(#8,#111,#122)"),
        );
        let p = Products::build(&entities(&data));
        assert_eq!(p.faults, []);
        let d = &p.definitions[0];
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.shapes[0].representation, 110);
        assert_eq!(
            d.shapes[0].mapped,
            [Mapped {
                item: 122,
                representation: 124,
                origin: 8,
                target: 20
            }]
        );
        // #124 is reached through the mapped item; #126 by nothing.
        let unplaced: Vec<Id> = p.unplaced.iter().map(|s| s.representation).collect();
        assert_eq!(unplaced, [126]);
    }

    #[test]
    fn cycles_and_faults() {
        let data = format!(
            "{}{}{}{}#500=NEXT_ASSEMBLY_USAGE_OCCURRENCE('x','','',#100,#999,$);",
            part(100, 110, 111),
            part(200, 210, 211),
            usage(
                400,
                100,
                200,
                210,
                110,
                "ITEM_DEFINED_TRANSFORMATION('','',#8,#20)"
            ),
            usage(
                410,
                200,
                100,
                110,
                210,
                "ITEM_DEFINED_TRANSFORMATION('','',#8,#20)"
            ),
        );
        let p = Products::build(&entities(&data));
        assert_eq!(
            p.faults,
            [
                ImportError::DanglingReference {
                    instance: 500,
                    reference: 999
                },
                ImportError::ReferenceCycle { instance: 410 },
            ]
        );
        assert_eq!(p.roots, [0]);
        assert_eq!(p.definitions[0].children.len(), 1);
        assert_eq!(p.definitions[1].children, []);
    }
}
