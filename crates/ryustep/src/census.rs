//! The census (design 448, ryucad 24): every instance of a file in one
//! category by its entity name, so an import or an export can say what
//! it interpreted, kept, left unreferenced, failed and wrote (#593).
//!
//! The table is fixed and sorted ([`category_of`]); a complex instance
//! takes the most specific category among its parts' ([`Category::rank`]),
//! and a `PROPERTY_DEFINITION` the category its name says
//! ([`property_category`]). Category names are stable, as error codes
//! are: agents branch on them.

use crate::part21::{Param, Part, Record};

/// What an instance is about. [`Category::as_str`] is its stable name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Geometry,
    Topology,
    ProductStructure,
    Units,
    /// Styles, colours, fonts, `INVISIBILITY`.
    Appearance,
    Layers,
    Properties,
    Materials,
    /// Tolerances, datums, dimensions, draughting models, annotation
    /// occurrences.
    Pmi,
    /// CAx-IF's geometric validation properties.
    Validation,
    ExternalReferences,
    /// People, organisations, approvals, dates, security classification,
    /// product categories.
    Management,
    Tessellated,
    Kinematics,
    Unknown,
}

impl Category {
    /// Every category, in the order reports list them.
    pub const ALL: [Category; 15] = [
        Category::Geometry,
        Category::Topology,
        Category::ProductStructure,
        Category::Units,
        Category::Appearance,
        Category::Layers,
        Category::Properties,
        Category::Materials,
        Category::Pmi,
        Category::Validation,
        Category::ExternalReferences,
        Category::Management,
        Category::Tessellated,
        Category::Kinematics,
        Category::Unknown,
    ];

    /// Its stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Geometry => "geometry",
            Category::Topology => "topology",
            Category::ProductStructure => "product_structure",
            Category::Units => "units",
            Category::Appearance => "appearance",
            Category::Layers => "layers",
            Category::Properties => "properties",
            Category::Materials => "materials",
            Category::Pmi => "pmi",
            Category::Validation => "validation",
            Category::ExternalReferences => "external_references",
            Category::Management => "management",
            Category::Tessellated => "tessellated",
            Category::Kinematics => "kinematics",
            Category::Unknown => "unknown",
        }
    }

    /// How specific it is, smallest first: what a complex instance of
    /// several categories' parts is.
    fn rank(self) -> u8 {
        match self {
            Category::Pmi => 0,
            Category::Validation => 1,
            Category::Materials => 2,
            Category::Properties => 3,
            Category::Appearance => 4,
            Category::Layers => 5,
            Category::Kinematics => 6,
            Category::Tessellated => 7,
            Category::ExternalReferences => 8,
            Category::Management => 9,
            Category::ProductStructure => 10,
            Category::Topology => 11,
            Category::Geometry => 12,
            Category::Units => 13,
            Category::Unknown => 14,
        }
    }
}

/// The category of the entity named `name` (upper case): `Unknown` for a
/// name the table doesn't hold.
pub fn category_of(name: &str) -> Category {
    TABLE
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .map_or(Category::Unknown, |k| TABLE[k].1)
}

/// The category of a property named `name`, compared without case:
/// CAx-IF's validation, material and document properties, else a
/// property.
pub fn property_category(name: &str) -> Category {
    match name.trim().to_ascii_lowercase().as_str() {
        "geometric validation property" => Category::Validation,
        "material property" | "density" => Category::Materials,
        "external definition" | "document property" => Category::ExternalReferences,
        _ => Category::Properties,
    }
}

/// An instance's category: a `PROPERTY_DEFINITION`'s by its name, else
/// its parts' most specific.
pub fn category_of_record(record: &Record) -> Category {
    match record {
        Record::Simple(p) if &*p.name == "PROPERTY_DEFINITION" => match p.params.first() {
            Some(Param::Str(s)) => property_category(s),
            _ => Category::Properties,
        },
        r => r
            .parts()
            .iter()
            .map(|p| category_of(&p.name))
            .min_by_key(|c| c.rank())
            .unwrap_or(Category::Unknown),
    }
}

/// An instance's entity name as reports give it: a simple record's name,
/// or a complex instance's part names sorted and joined by spaces.
pub fn joined(parts: &[Part]) -> String {
    let mut names: Vec<&str> = parts.iter().map(|p| &*p.name).collect();
    names.sort_unstable();
    names.join(" ")
}

use crate::part21::{File, Instance};
use std::borrow::Cow;
use std::collections::HashMap;

/// Every instance of a file: its entity name, its category, and the
/// instances it refers to, kept as a graph by index (dangling references
/// left out), with whether its weakly connected component holds an
/// anchor (product structure, a style, or a representation): an
/// instance outside every such component is unreferenced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Census {
    ids: Vec<u64>,
    names: Vec<Box<str>>,
    name: Vec<u32>,
    category: Vec<Category>,
    offsets: Vec<u32>,
    targets: Vec<u32>,
    anchored: Vec<bool>,
}

/// One instance as the parallel decode sees it.
pub(crate) struct Seen<'f> {
    name: Cow<'f, str>,
    category: Category,
    refs: Vec<u32>,
}

fn refs(p: &Param, out: &mut Vec<u64>) {
    match p {
        Param::Ref(r) => out.push(*r),
        Param::List(l) => l.iter().for_each(|x| refs(x, out)),
        Param::Typed(_, v) => refs(v, out),
        _ => {}
    }
}

/// What the census keeps of `inst`, whose file's instance numbers are
/// `ids` (ascending).
pub(crate) fn seen<'f>(inst: &'f Instance, ids: &[u64]) -> Seen<'f> {
    let parts = inst.record.parts();
    let name = match parts {
        [one] => Cow::Borrowed(&*one.name),
        _ => Cow::Owned(joined(parts)),
    };
    let mut raw = Vec::new();
    for p in parts {
        p.params.iter().for_each(|x| refs(x, &mut raw));
    }
    let mut out: Vec<u32> = raw
        .into_iter()
        .filter_map(|r| ids.binary_search(&r).ok().map(|k| k as u32))
        .collect();
    out.sort_unstable();
    out.dedup();
    Seen {
        name,
        category: category_of_record(&inst.record),
        refs: out,
    }
}

/// Whether an instance anchors its component: product structure, a
/// style, or a representation (not a relationship between them).
fn is_anchor(c: Category, name: &str) -> bool {
    matches!(c, Category::ProductStructure | Category::Appearance)
        || name == "REPRESENTATION"
        || (name.ends_with("_REPRESENTATION") && !name.contains("RELATIONSHIP"))
}

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let up = parent[parent[x as usize] as usize];
        parent[x as usize] = up;
        x = up;
    }
    x
}

impl Census {
    /// The census of `file`, decoded one instance at a time (in
    /// parallel, as [`crate::schema::Entities::decode_counted`] does).
    pub fn of(file: &File) -> Census {
        use rayon::prelude::*;
        let ids: Vec<u64> = file.instances.iter().map(|i| i.id).collect();
        let seen: Vec<Seen<'_>> = file
            .instances
            .par_iter()
            .with_min_len(256)
            .map(|i| seen(i, &ids))
            .collect();
        Census::from_seen(ids, seen)
    }

    /// The census of instances `ids` as `seen`, in order: names
    /// interned, the graph laid out, properties' representations given
    /// their category, and components found. Sequential, so the same on
    /// any number of threads.
    pub(crate) fn from_seen(ids: Vec<u64>, seen: Vec<Seen<'_>>) -> Census {
        let n = ids.len();
        let mut lookup: HashMap<String, u32> = HashMap::new();
        let mut names: Vec<Box<str>> = Vec::new();
        let mut name = Vec::with_capacity(n);
        let mut category = Vec::with_capacity(n);
        let mut offsets = Vec::with_capacity(n + 1);
        let mut targets = Vec::new();
        offsets.push(0u32);
        for s in &seen {
            let k = match lookup.get(s.name.as_ref()) {
                Some(&k) => k,
                None => {
                    let k = names.len() as u32;
                    names.push(s.name.as_ref().into());
                    lookup.insert(s.name.to_string(), k);
                    k
                }
            };
            name.push(k);
            category.push(s.category);
            targets.extend_from_slice(&s.refs);
            offsets.push(targets.len() as u32);
        }
        drop(seen);
        let out = |k: usize| offsets[k] as usize..offsets[k + 1] as usize;
        // A property's representation, and that representation's
        // property and geometry items, are what the property is about.
        if let (Some(&pdr), Some(&pd)) = (
            lookup.get("PROPERTY_DEFINITION_REPRESENTATION"),
            lookup.get("PROPERTY_DEFINITION"),
        ) {
            for k in 0..n {
                if name[k] != pdr {
                    continue;
                }
                let refs = &targets[out(k)];
                let Some(&def) = refs.iter().find(|&&t| name[t as usize] == pd) else {
                    continue;
                };
                let c = category[def as usize];
                if c == Category::Properties {
                    continue;
                }
                category[k] = c;
                for &rep in refs.iter().filter(|&&t| t != def) {
                    category[rep as usize] = c;
                    for &item in &targets[out(rep as usize)] {
                        if matches!(
                            category[item as usize],
                            Category::Properties | Category::Geometry
                        ) {
                            category[item as usize] = c;
                        }
                    }
                }
            }
        }
        // Weakly connected components, by union and find.
        let mut parent: Vec<u32> = (0..n as u32).collect();
        for k in 0..n {
            for &t in &targets[out(k)] {
                let (a, b) = (find(&mut parent, k as u32), find(&mut parent, t));
                if a != b {
                    parent[a.max(b) as usize] = a.min(b);
                }
            }
        }
        let mut root_anchored = vec![false; n];
        for k in 0..n {
            if is_anchor(category[k], &names[name[k] as usize]) {
                let r = find(&mut parent, k as u32);
                root_anchored[r as usize] = true;
            }
        }
        let mut anchored = Vec::with_capacity(n);
        for k in 0..n {
            let r = find(&mut parent, k as u32);
            anchored.push(root_anchored[r as usize]);
        }
        Census {
            ids,
            names,
            name,
            category,
            offsets,
            targets,
            anchored,
        }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    fn index(&self, id: u64) -> Option<usize> {
        self.ids.binary_search(&id).ok()
    }

    /// The instances instance `k` (an index) refers to, as indices.
    fn out(&self, k: usize) -> &[u32] {
        &self.targets[self.offsets[k] as usize..self.offsets[k + 1] as usize]
    }

    /// Instance `id`'s category.
    pub fn category_at(&self, id: u64) -> Option<Category> {
        self.index(id).map(|k| self.category[k])
    }

    /// Instance `id`'s entity name ([`joined`] for a complex one).
    pub fn name_at(&self, id: u64) -> Option<&str> {
        self.index(id).map(|k| &*self.names[self.name[k] as usize])
    }

    /// Whether instance `id`'s component holds an anchor.
    pub fn anchored_at(&self, id: u64) -> Option<bool> {
        self.index(id).map(|k| self.anchored[k])
    }

    /// Every instance counted by category and entity name, in category
    /// order then by name.
    pub fn counts(&self) -> Vec<(Category, &str, u64)> {
        let mut m: std::collections::BTreeMap<(Category, &str), u64> = Default::default();
        for k in 0..self.len() {
            *m.entry((self.category[k], &*self.names[self.name[k] as usize]))
                .or_default() += 1;
        }
        m.into_iter().map(|((c, n), k)| (c, n, k)).collect()
    }
}

use std::collections::BTreeMap;

/// What a caller read from a file: the instances it followed (each
/// body's representation, context and items: [`crate::Opened::roots`])
/// and whether it read the product tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Read {
    pub roots: Vec<u64>,
    pub product_tree: bool,
}

/// An instance a refusal names, with the refusal's code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    pub instance: u64,
    pub code: String,
}

/// Where a report puts an instance. Names are stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bucket {
    /// Mapped into the project.
    Interpreted = 0,
    /// Not interpreted, but kept in the committed bytes.
    Retained = 1,
    /// Reached by no product, representation or style.
    Unreferenced = 2,
    /// Named by a refusal.
    Failed = 3,
}

impl Bucket {
    pub const ALL: [Bucket; 4] = [
        Bucket::Interpreted,
        Bucket::Retained,
        Bucket::Unreferenced,
        Bucket::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Interpreted => "interpreted",
            Bucket::Retained => "retained",
            Bucket::Unreferenced => "unreferenced",
            Bucket::Failed => "failed",
        }
    }
}

/// How many instances of one entity name, the first 64 of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    pub name: String,
    pub count: u64,
    /// Ascending, at most [`FIRST`].
    pub first: Vec<u64>,
}

/// One bucket: its count and its names, the largest first (then by
/// name).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counted {
    pub count: u64,
    pub names: Vec<Named>,
}

/// One category's buckets (indexed by [`Bucket`]) and its failures'
/// codes with their counts, by code.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub category: Category,
    pub buckets: [Counted; 4],
    pub codes: Vec<(String, u64)>,
}

/// Every category a file has instances of, in [`Category::ALL`]'s order.
#[derive(Clone, Debug, PartialEq)]
pub struct Transfer {
    pub instances: u64,
    pub lines: Vec<Line>,
}

/// How many instance numbers a name keeps.
pub const FIRST: usize = 64;

/// What 24.A interprets: bodies and the product tree. Later plans add
/// appearance (24.B), properties and materials (24.C) and external
/// references (24.D).
pub const INTERPRETED: [Category; 4] = [
    Category::Geometry,
    Category::Topology,
    Category::Units,
    Category::ProductStructure,
];

impl Census {
    /// Every instance in a bucket: `failed` first (its code counted),
    /// then interpreted (an [`INTERPRETED`] category reached from
    /// `read.roots`, or anchored product structure when the tree was
    /// read), then unreferenced, else retained.
    pub fn transfer(&self, read: &Read, failed: &[Failed]) -> Transfer {
        let n = self.len();
        let mut reached = vec![false; n];
        let mut stack: Vec<u32> = read
            .roots
            .iter()
            .filter_map(|&r| self.index(r).map(|k| k as u32))
            .collect();
        while let Some(k) = stack.pop() {
            if std::mem::replace(&mut reached[k as usize], true) {
                continue;
            }
            stack.extend(
                self.out(k as usize)
                    .iter()
                    .copied()
                    .filter(|&t| !reached[t as usize]),
            );
        }
        let mut code_of: BTreeMap<usize, &str> = BTreeMap::new();
        for f in failed {
            if let Some(k) = self.index(f.instance) {
                code_of.entry(k).or_insert(&f.code);
            }
        }
        type Tally = BTreeMap<u32, (u64, Vec<u64>)>;
        let mut tally: BTreeMap<(Category, Bucket), Tally> = BTreeMap::new();
        let mut codes: BTreeMap<Category, BTreeMap<&str, u64>> = BTreeMap::new();
        for k in 0..n {
            let c = self.category[k];
            let b = if let Some(code) = code_of.get(&k) {
                *codes.entry(c).or_default().entry(code).or_default() += 1;
                Bucket::Failed
            } else if INTERPRETED.contains(&c)
                && (reached[k]
                    || (read.product_tree && c == Category::ProductStructure && self.anchored[k]))
            {
                Bucket::Interpreted
            } else if !self.anchored[k] {
                Bucket::Unreferenced
            } else {
                Bucket::Retained
            };
            let e = tally
                .entry((c, b))
                .or_default()
                .entry(self.name[k])
                .or_insert((0, Vec::new()));
            e.0 += 1;
            if e.1.len() < FIRST {
                e.1.push(self.ids[k]);
            }
        }
        let lines = Category::ALL
            .iter()
            .filter_map(|&c| {
                let buckets = Bucket::ALL.map(|b| self.counted(tally.get(&(c, b))));
                buckets.iter().any(|x| x.count > 0).then(|| Line {
                    category: c,
                    buckets,
                    codes: codes
                        .get(&c)
                        .map(|m| m.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect())
                        .unwrap_or_default(),
                })
            })
            .collect();
        Transfer {
            instances: n as u64,
            lines,
        }
    }

    fn counted(&self, tally: Option<&BTreeMap<u32, (u64, Vec<u64>)>>) -> Counted {
        let mut names: Vec<Named> = tally
            .into_iter()
            .flatten()
            .map(|(&k, (count, first))| Named {
                name: self.names[k as usize].to_string(),
                count: *count,
                first: first.clone(),
            })
            .collect();
        names.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        Counted {
            count: names.iter().map(|x| x.count).sum(),
            names,
        }
    }
}

/// Every one of `instances` counted by category and entity name, in
/// category order then by name: what a writer wrote.
pub fn count(instances: &[Instance]) -> Vec<(Category, String, u64)> {
    let mut m: BTreeMap<(Category, String), u64> = BTreeMap::new();
    for i in instances {
        *m.entry((category_of_record(&i.record), joined(i.record.parts())))
            .or_default() += 1;
    }
    m.into_iter().map(|((c, n), k)| (c, n, k)).collect()
}

/// Entity names and their categories, sorted by name. ISO 10303-42, -43,
/// -41, -46, -47 and -242's names, and CAx-IF's recommended practices'.
const TABLE: &[(&str, Category)] = &[
    ("ADVANCED_BREP_SHAPE_REPRESENTATION", Category::Geometry),
    ("ADVANCED_FACE", Category::Topology),
    ("ALL_AROUND_SHAPE_ASPECT", Category::Pmi),
    ("ANGULARITY_TOLERANCE", Category::Pmi),
    ("ANGULAR_LOCATION", Category::Pmi),
    ("ANGULAR_SIZE", Category::Pmi),
    ("ANNOTATION_CURVE_OCCURRENCE", Category::Pmi),
    ("ANNOTATION_FILL_AREA", Category::Pmi),
    ("ANNOTATION_FILL_AREA_OCCURRENCE", Category::Pmi),
    ("ANNOTATION_OCCURRENCE", Category::Pmi),
    ("ANNOTATION_PLACEHOLDER_OCCURRENCE", Category::Pmi),
    ("ANNOTATION_PLANE", Category::Pmi),
    ("ANNOTATION_TEXT_OCCURRENCE", Category::Pmi),
    ("APPLICATION_CONTEXT", Category::ProductStructure),
    (
        "APPLICATION_PROTOCOL_DEFINITION",
        Category::ProductStructure,
    ),
    ("APPLIED_APPROVAL_ASSIGNMENT", Category::Management),
    ("APPLIED_DATE_AND_TIME_ASSIGNMENT", Category::Management),
    ("APPLIED_DATE_ASSIGNMENT", Category::Management),
    ("APPLIED_DOCUMENT_REFERENCE", Category::ExternalReferences),
    (
        "APPLIED_EXTERNAL_IDENTIFICATION_ASSIGNMENT",
        Category::ExternalReferences,
    ),
    ("APPLIED_ORGANIZATION_ASSIGNMENT", Category::Management),
    (
        "APPLIED_PERSON_AND_ORGANIZATION_ASSIGNMENT",
        Category::Management,
    ),
    (
        "APPLIED_SECURITY_CLASSIFICATION_ASSIGNMENT",
        Category::Management,
    ),
    ("APPROVAL", Category::Management),
    ("APPROVAL_DATE_TIME", Category::Management),
    ("APPROVAL_PERSON_ORGANIZATION", Category::Management),
    ("APPROVAL_ROLE", Category::Management),
    ("APPROVAL_STATUS", Category::Management),
    ("AREA_MEASURE_WITH_UNIT", Category::Units),
    ("AREA_UNIT", Category::Units),
    ("ASSEMBLY_COMPONENT_USAGE", Category::ProductStructure),
    ("AXIS1_PLACEMENT", Category::Geometry),
    ("AXIS2_PLACEMENT_2D", Category::Geometry),
    ("AXIS2_PLACEMENT_3D", Category::Geometry),
    ("BEZIER_CURVE", Category::Geometry),
    ("BEZIER_SURFACE", Category::Geometry),
    ("BOOLEAN_REPRESENTATION_ITEM", Category::Properties),
    ("BOUNDARY_CURVE", Category::Geometry),
    ("BOUNDED_CURVE", Category::Geometry),
    ("BOUNDED_SURFACE", Category::Geometry),
    ("BREP_WITH_VOIDS", Category::Topology),
    ("B_SPLINE_CURVE", Category::Geometry),
    ("B_SPLINE_CURVE_WITH_KNOTS", Category::Geometry),
    ("B_SPLINE_SURFACE", Category::Geometry),
    ("B_SPLINE_SURFACE_WITH_KNOTS", Category::Geometry),
    ("CALENDAR_DATE", Category::Management),
    ("CAMERA_MODEL_D3", Category::Pmi),
    ("CAMERA_MODEL_D3_MULTI_CLIPPING", Category::Pmi),
    ("CAMERA_USAGE", Category::Pmi),
    ("CARTESIAN_POINT", Category::Geometry),
    ("CARTESIAN_TRANSFORMATION_OPERATOR", Category::Geometry),
    ("CARTESIAN_TRANSFORMATION_OPERATOR_3D", Category::Geometry),
    ("CC_DESIGN_APPROVAL", Category::Management),
    ("CC_DESIGN_CERTIFICATION", Category::Management),
    ("CC_DESIGN_CONTRACT", Category::Management),
    ("CC_DESIGN_DATE_AND_TIME_ASSIGNMENT", Category::Management),
    (
        "CC_DESIGN_PERSON_AND_ORGANIZATION_ASSIGNMENT",
        Category::Management,
    ),
    ("CC_DESIGN_SECURITY_CLASSIFICATION", Category::Management),
    ("CENTRE_OF_SYMMETRY", Category::Pmi),
    ("CERTIFICATION", Category::Management),
    ("CHARACTERIZED_OBJECT", Category::Properties),
    ("CIRCLE", Category::Geometry),
    ("CIRCULAR_RUNOUT_TOLERANCE", Category::Pmi),
    ("CLOSED_SHELL", Category::Topology),
    ("COAXIALITY_TOLERANCE", Category::Pmi),
    ("COLOUR", Category::Appearance),
    ("COLOUR_RGB", Category::Appearance),
    ("COLOUR_SPECIFICATION", Category::Appearance),
    ("COMMON_DATUM", Category::Pmi),
    ("COMPLEX_TRIANGULATED_FACE", Category::Tessellated),
    ("COMPLEX_TRIANGULATED_SURFACE_SET", Category::Tessellated),
    ("COMPOSITE_CURVE", Category::Geometry),
    ("COMPOSITE_CURVE_ON_SURFACE", Category::Geometry),
    ("COMPOSITE_CURVE_SEGMENT", Category::Geometry),
    ("COMPOSITE_GROUP_SHAPE_ASPECT", Category::Pmi),
    ("COMPOSITE_SHAPE_ASPECT", Category::Pmi),
    ("CONCENTRICITY_TOLERANCE", Category::Pmi),
    ("CONIC", Category::Geometry),
    ("CONICAL_SURFACE", Category::Geometry),
    ("CONNECTED_EDGE_SET", Category::Topology),
    ("CONNECTED_FACE_SET", Category::Topology),
    ("CONSTRUCTIVE_GEOMETRY_REPRESENTATION", Category::Geometry),
    (
        "CONSTRUCTIVE_GEOMETRY_REPRESENTATION_RELATIONSHIP",
        Category::Geometry,
    ),
    (
        "CONTEXT_DEPENDENT_KINEMATIC_LINK_REPRESENTATION",
        Category::Kinematics,
    ),
    (
        "CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM",
        Category::Appearance,
    ),
    (
        "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
        Category::ProductStructure,
    ),
    ("CONTEXT_DEPENDENT_UNIT", Category::Units),
    ("CONTRACT", Category::Management),
    ("CONVERSION_BASED_UNIT", Category::Units),
    ("COORDINATED_UNIVERSAL_TIME_OFFSET", Category::Management),
    ("COORDINATES_LIST", Category::Tessellated),
    ("CUBIC_BEZIER_TESSELLATED_EDGE", Category::Tessellated),
    ("CURVE", Category::Geometry),
    ("CURVE_STYLE", Category::Appearance),
    ("CURVE_STYLE_FONT", Category::Appearance),
    ("CURVE_STYLE_FONT_PATTERN", Category::Appearance),
    ("CYLINDRICAL_PAIR", Category::Kinematics),
    ("CYLINDRICAL_SURFACE", Category::Geometry),
    ("CYLINDRICITY_TOLERANCE", Category::Pmi),
    ("DATA_ENVIRONMENT", Category::Materials),
    ("DATE_AND_TIME", Category::Management),
    ("DATE_ROLE", Category::Management),
    ("DATE_TIME_ROLE", Category::Management),
    ("DATUM", Category::Pmi),
    ("DATUM_FEATURE", Category::Pmi),
    ("DATUM_REFERENCE", Category::Pmi),
    ("DATUM_REFERENCE_COMPARTMENT", Category::Pmi),
    ("DATUM_REFERENCE_ELEMENT", Category::Pmi),
    ("DATUM_REFERENCE_MODIFIER_WITH_VALUE", Category::Pmi),
    ("DATUM_SYSTEM", Category::Pmi),
    ("DATUM_TARGET", Category::Pmi),
    ("DEFINITIONAL_REPRESENTATION", Category::Geometry),
    ("DEGENERATE_TOROIDAL_SURFACE", Category::Geometry),
    ("DERIVED_UNIT", Category::Units),
    ("DERIVED_UNIT_ELEMENT", Category::Units),
    ("DESCRIPTIVE_REPRESENTATION_ITEM", Category::Properties),
    ("DESIGN_CONTEXT", Category::ProductStructure),
    ("DIMENSIONAL_CHARACTERISTIC_REPRESENTATION", Category::Pmi),
    ("DIMENSIONAL_EXPONENTS", Category::Units),
    ("DIMENSIONAL_LOCATION", Category::Pmi),
    ("DIMENSIONAL_LOCATION_WITH_PATH", Category::Pmi),
    ("DIMENSIONAL_SIZE", Category::Pmi),
    ("DIMENSIONAL_SIZE_WITH_PATH", Category::Pmi),
    ("DIRECTION", Category::Geometry),
    ("DOCUMENT", Category::ExternalReferences),
    ("DOCUMENT_FILE", Category::ExternalReferences),
    ("DOCUMENT_PRODUCT_EQUIVALENCE", Category::ExternalReferences),
    ("DOCUMENT_REPRESENTATION_TYPE", Category::ExternalReferences),
    ("DOCUMENT_TYPE", Category::ExternalReferences),
    ("DRAUGHTING_ANNOTATION_OCCURRENCE", Category::Pmi),
    ("DRAUGHTING_CALLOUT", Category::Pmi),
    ("DRAUGHTING_MODEL", Category::Pmi),
    ("DRAUGHTING_MODEL_ITEM_ASSOCIATION", Category::Pmi),
    (
        "DRAUGHTING_MODEL_ITEM_ASSOCIATION_WITH_PLACEHOLDER",
        Category::Pmi,
    ),
    ("DRAUGHTING_PRE_DEFINED_COLOUR", Category::Appearance),
    ("DRAUGHTING_PRE_DEFINED_CURVE_FONT", Category::Appearance),
    ("DRAUGHTING_PRE_DEFINED_TEXT_FONT", Category::Appearance),
    ("EDGE", Category::Topology),
    ("EDGE_BASED_WIREFRAME_MODEL", Category::Topology),
    (
        "EDGE_BASED_WIREFRAME_SHAPE_REPRESENTATION",
        Category::Geometry,
    ),
    ("EDGE_CURVE", Category::Topology),
    ("EDGE_LOOP", Category::Topology),
    ("ELEMENTARY_SURFACE", Category::Geometry),
    ("ELLIPSE", Category::Geometry),
    ("EXTERNALLY_DEFINED_ITEM", Category::ExternalReferences),
    ("EXTERNAL_SOURCE", Category::ExternalReferences),
    ("FACE", Category::Topology),
    ("FACETED_BREP", Category::Topology),
    ("FACETED_BREP_SHAPE_REPRESENTATION", Category::Geometry),
    ("FACE_BASED_SURFACE_MODEL", Category::Topology),
    ("FACE_BOUND", Category::Topology),
    ("FACE_OUTER_BOUND", Category::Topology),
    ("FACE_SURFACE", Category::Topology),
    ("FEATURE_FOR_DATUM_TARGET_RELATIONSHIP", Category::Pmi),
    ("FILL_AREA_STYLE", Category::Appearance),
    ("FILL_AREA_STYLE_COLOUR", Category::Appearance),
    ("FLATNESS_TOLERANCE", Category::Pmi),
    ("GENERAL_PROPERTY", Category::Properties),
    ("GENERAL_PROPERTY_ASSOCIATION", Category::Properties),
    ("GENERAL_PROPERTY_RELATIONSHIP", Category::Properties),
    (
        "GEOMETRICALLY_BOUNDED_SURFACE_SHAPE_REPRESENTATION",
        Category::Geometry,
    ),
    (
        "GEOMETRICALLY_BOUNDED_WIREFRAME_SHAPE_REPRESENTATION",
        Category::Geometry,
    ),
    ("GEOMETRIC_CURVE_SET", Category::Geometry),
    ("GEOMETRIC_ITEM_SPECIFIC_USAGE", Category::Pmi),
    ("GEOMETRIC_REPRESENTATION_CONTEXT", Category::Units),
    ("GEOMETRIC_REPRESENTATION_ITEM", Category::Geometry),
    ("GEOMETRIC_SET", Category::Geometry),
    ("GEOMETRIC_TOLERANCE", Category::Pmi),
    ("GEOMETRIC_TOLERANCE_WITH_DATUM_REFERENCE", Category::Pmi),
    ("GEOMETRIC_TOLERANCE_WITH_DEFINED_UNIT", Category::Pmi),
    ("GEOMETRIC_TOLERANCE_WITH_MODIFIERS", Category::Pmi),
    ("GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT", Category::Units),
    ("GLOBAL_UNIT_ASSIGNED_CONTEXT", Category::Units),
    ("HYPERBOLA", Category::Geometry),
    ("IDENTIFICATION_ROLE", Category::ExternalReferences),
    ("INTEGER_REPRESENTATION_ITEM", Category::Properties),
    ("INTERSECTION_CURVE", Category::Geometry),
    ("INVISIBILITY", Category::Appearance),
    ("ITEM_DEFINED_TRANSFORMATION", Category::ProductStructure),
    ("KINEMATIC_GROUND_REPRESENTATION", Category::Kinematics),
    ("KINEMATIC_JOINT", Category::Kinematics),
    ("KINEMATIC_LINK", Category::Kinematics),
    (
        "KINEMATIC_LINK_REPRESENTATION_ASSOCIATION",
        Category::Kinematics,
    ),
    ("KINEMATIC_PAIR", Category::Kinematics),
    ("KINEMATIC_PATH", Category::Kinematics),
    (
        "KINEMATIC_PROPERTY_DEFINITION_REPRESENTATION",
        Category::Kinematics,
    ),
    ("KINEMATIC_TOPOLOGY_STRUCTURE", Category::Kinematics),
    ("LENGTH_MEASURE_WITH_UNIT", Category::Units),
    ("LENGTH_UNIT", Category::Units),
    ("LIMITS_AND_FITS", Category::Pmi),
    ("LINE", Category::Geometry),
    ("LINE_PROFILE_TOLERANCE", Category::Pmi),
    (
        "LINK_MOTION_REPRESENTATION_ALONG_PATH",
        Category::Kinematics,
    ),
    ("LOCAL_TIME", Category::Management),
    ("LOOP", Category::Topology),
    ("MANIFOLD_SOLID_BREP", Category::Topology),
    ("MANIFOLD_SURFACE_SHAPE_REPRESENTATION", Category::Geometry),
    ("MAPPED_ITEM", Category::Geometry),
    ("MASS_MEASURE_WITH_UNIT", Category::Units),
    ("MASS_UNIT", Category::Units),
    ("MATERIAL_DESIGNATION", Category::Materials),
    ("MATERIAL_DESIGNATION_CHARACTERIZATION", Category::Materials),
    ("MATERIAL_PROPERTY", Category::Materials),
    ("MATERIAL_PROPERTY_REPRESENTATION", Category::Materials),
    ("MEASURE_QUALIFICATION", Category::Pmi),
    ("MEASURE_REPRESENTATION_ITEM", Category::Properties),
    ("MEASURE_WITH_UNIT", Category::Units),
    ("MECHANICAL_CONTEXT", Category::ProductStructure),
    (
        "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_AREA",
        Category::Appearance,
    ),
    (
        "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION",
        Category::Appearance,
    ),
    ("MECHANISM_REPRESENTATION", Category::Kinematics),
    ("MECHANISM_STATE_REPRESENTATION", Category::Kinematics),
    ("NAMED_UNIT", Category::Units),
    ("NEXT_ASSEMBLY_USAGE_OCCURRENCE", Category::ProductStructure),
    (
        "NON_MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
        Category::Geometry,
    ),
    ("OBJECT_ROLE", Category::ExternalReferences),
    ("OFFSET_CURVE_2D", Category::Geometry),
    ("OFFSET_CURVE_3D", Category::Geometry),
    ("OFFSET_SURFACE", Category::Geometry),
    ("OPEN_SHELL", Category::Topology),
    ("ORGANIZATION", Category::Management),
    ("ORGANIZATION_ROLE", Category::Management),
    ("ORIENTED_CLOSED_SHELL", Category::Topology),
    ("ORIENTED_EDGE", Category::Topology),
    ("ORIENTED_FACE", Category::Topology),
    ("ORIENTED_OPEN_SHELL", Category::Topology),
    ("OUTER_BOUNDARY_CURVE", Category::Geometry),
    ("OVER_RIDING_STYLED_ITEM", Category::Appearance),
    ("PAIR_REPRESENTATION_RELATIONSHIP", Category::Kinematics),
    ("PARABOLA", Category::Geometry),
    ("PARALLELISM_TOLERANCE", Category::Pmi),
    ("PARAMETRIC_REPRESENTATION_CONTEXT", Category::Units),
    ("PATH", Category::Topology),
    ("PCURVE", Category::Geometry),
    ("PERPENDICULARITY_TOLERANCE", Category::Pmi),
    ("PERSON", Category::Management),
    ("PERSON_AND_ORGANIZATION", Category::Management),
    ("PERSON_AND_ORGANIZATION_ROLE", Category::Management),
    ("PLACED_DATUM_TARGET_FEATURE", Category::Pmi),
    ("PLACEMENT", Category::Geometry),
    ("PLANAR_BOX", Category::Pmi),
    ("PLANAR_PAIR", Category::Kinematics),
    ("PLANE", Category::Geometry),
    ("PLANE_ANGLE_MEASURE_WITH_UNIT", Category::Units),
    ("PLANE_ANGLE_UNIT", Category::Units),
    ("PLUS_MINUS_TOLERANCE", Category::Pmi),
    ("POINT", Category::Geometry),
    ("POINT_ON_CURVE", Category::Geometry),
    ("POINT_ON_SURFACE", Category::Geometry),
    ("POINT_STYLE", Category::Appearance),
    ("POLYLINE", Category::Geometry),
    ("POLY_LOOP", Category::Topology),
    ("POSITION_TOLERANCE", Category::Pmi),
    ("PRECISION_QUALIFIER", Category::Pmi),
    ("PRESENTATION_LAYER_ASSIGNMENT", Category::Layers),
    ("PRESENTATION_LAYER_USAGE", Category::Layers),
    ("PRESENTATION_SET", Category::Appearance),
    ("PRESENTATION_STYLE_ASSIGNMENT", Category::Appearance),
    ("PRESENTATION_STYLE_BY_CONTEXT", Category::Appearance),
    ("PRESENTATION_VIEW", Category::Appearance),
    ("PRE_DEFINED_MARKER", Category::Appearance),
    ("PRISMATIC_PAIR", Category::Kinematics),
    ("PRODUCT", Category::ProductStructure),
    ("PRODUCT_CATEGORY", Category::Management),
    ("PRODUCT_CATEGORY_RELATIONSHIP", Category::Management),
    ("PRODUCT_CONTEXT", Category::ProductStructure),
    ("PRODUCT_DEFINITION", Category::ProductStructure),
    ("PRODUCT_DEFINITION_CONTEXT", Category::ProductStructure),
    ("PRODUCT_DEFINITION_FORMATION", Category::ProductStructure),
    (
        "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
        Category::ProductStructure,
    ),
    ("PRODUCT_DEFINITION_SHAPE", Category::ProductStructure),
    ("PRODUCT_DEFINITION_USAGE", Category::ProductStructure),
    (
        "PRODUCT_DEFINITION_WITH_ASSOCIATED_DOCUMENTS",
        Category::ProductStructure,
    ),
    ("PRODUCT_RELATED_PRODUCT_CATEGORY", Category::Management),
    ("PROJECTED_ZONE_DEFINITION", Category::Pmi),
    ("PROPERTY_DEFINITION", Category::Properties),
    ("PROPERTY_DEFINITION_RELATIONSHIP", Category::Properties),
    ("PROPERTY_DEFINITION_REPRESENTATION", Category::Properties),
    ("QUASI_UNIFORM_CURVE", Category::Geometry),
    ("QUASI_UNIFORM_SURFACE", Category::Geometry),
    ("RATIONAL_B_SPLINE_CURVE", Category::Geometry),
    ("RATIONAL_B_SPLINE_SURFACE", Category::Geometry),
    ("RATIO_MEASURE_WITH_UNIT", Category::Units),
    ("RATIO_UNIT", Category::Units),
    ("REAL_REPRESENTATION_ITEM", Category::Properties),
    ("RECTANGULAR_TRIMMED_SURFACE", Category::Geometry),
    ("REPARAMETRISED_COMPOSITE_CURVE_SEGMENT", Category::Geometry),
    ("REPRESENTATION", Category::Properties),
    ("REPRESENTATION_CONTEXT", Category::Units),
    ("REPRESENTATION_ITEM", Category::Geometry),
    ("REPRESENTATION_MAP", Category::Geometry),
    ("REPRESENTATION_RELATIONSHIP", Category::ProductStructure),
    (
        "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION",
        Category::ProductStructure,
    ),
    ("REVOLUTE_PAIR", Category::Kinematics),
    ("ROLE_ASSOCIATION", Category::ExternalReferences),
    ("ROUNDNESS_TOLERANCE", Category::Pmi),
    ("RUNOUT_ZONE_DEFINITION", Category::Pmi),
    ("SEAM_CURVE", Category::Geometry),
    ("SECURITY_CLASSIFICATION", Category::Management),
    ("SECURITY_CLASSIFICATION_LEVEL", Category::Management),
    ("SHAPE_ASPECT", Category::Pmi),
    ("SHAPE_ASPECT_RELATIONSHIP", Category::Pmi),
    (
        "SHAPE_DEFINITION_REPRESENTATION",
        Category::ProductStructure,
    ),
    ("SHAPE_DIMENSION_REPRESENTATION", Category::Pmi),
    ("SHAPE_REPRESENTATION", Category::Geometry),
    (
        "SHAPE_REPRESENTATION_RELATIONSHIP",
        Category::ProductStructure,
    ),
    ("SHAPE_REPRESENTATION_WITH_PARAMETERS", Category::Geometry),
    ("SHELL_BASED_SURFACE_MODEL", Category::Topology),
    ("SHELL_BASED_WIREFRAME_MODEL", Category::Topology),
    (
        "SHELL_BASED_WIREFRAME_SHAPE_REPRESENTATION",
        Category::Geometry,
    ),
    ("SI_UNIT", Category::Units),
    ("SOLID_ANGLE_MEASURE_WITH_UNIT", Category::Units),
    ("SOLID_ANGLE_UNIT", Category::Units),
    ("SPHERICAL_PAIR", Category::Kinematics),
    ("SPHERICAL_SURFACE", Category::Geometry),
    ("STRAIGHTNESS_TOLERANCE", Category::Pmi),
    ("STYLED_ITEM", Category::Appearance),
    ("SUBEDGE", Category::Topology),
    ("SUBFACE", Category::Topology),
    ("SURFACE", Category::Geometry),
    ("SURFACE_CURVE", Category::Geometry),
    ("SURFACE_OF_LINEAR_EXTRUSION", Category::Geometry),
    ("SURFACE_OF_REVOLUTION", Category::Geometry),
    ("SURFACE_PROFILE_TOLERANCE", Category::Pmi),
    ("SURFACE_SIDE_STYLE", Category::Appearance),
    ("SURFACE_STYLE_BOUNDARY", Category::Appearance),
    ("SURFACE_STYLE_CONTROL_GRID", Category::Appearance),
    ("SURFACE_STYLE_FILL_AREA", Category::Appearance),
    ("SURFACE_STYLE_PARAMETER_LINE", Category::Appearance),
    ("SURFACE_STYLE_REFLECTANCE_AMBIENT", Category::Appearance),
    (
        "SURFACE_STYLE_REFLECTANCE_AMBIENT_DIFFUSE",
        Category::Appearance,
    ),
    (
        "SURFACE_STYLE_REFLECTANCE_AMBIENT_DIFFUSE_SPECULAR",
        Category::Appearance,
    ),
    ("SURFACE_STYLE_RENDERING", Category::Appearance),
    (
        "SURFACE_STYLE_RENDERING_WITH_PROPERTIES",
        Category::Appearance,
    ),
    ("SURFACE_STYLE_SEGMENTATION_CURVE", Category::Appearance),
    ("SURFACE_STYLE_SILHOUETTE", Category::Appearance),
    ("SURFACE_STYLE_TRANSPARENT", Category::Appearance),
    ("SURFACE_STYLE_USAGE", Category::Appearance),
    ("SWEPT_SURFACE", Category::Geometry),
    ("SYMMETRY_TOLERANCE", Category::Pmi),
    ("TESSELLATED_ANNOTATION_OCCURRENCE", Category::Pmi),
    ("TESSELLATED_CONNECTING_EDGE", Category::Tessellated),
    ("TESSELLATED_CURVE_SET", Category::Tessellated),
    ("TESSELLATED_EDGE", Category::Tessellated),
    ("TESSELLATED_GEOMETRIC_SET", Category::Tessellated),
    ("TESSELLATED_POINT_SET", Category::Tessellated),
    ("TESSELLATED_SHAPE_REPRESENTATION", Category::Tessellated),
    ("TESSELLATED_SHELL", Category::Tessellated),
    ("TESSELLATED_SOLID", Category::Tessellated),
    ("TESSELLATED_VERTEX", Category::Tessellated),
    ("TEXT_LITERAL", Category::Pmi),
    ("TEXT_STYLE", Category::Appearance),
    ("TEXT_STYLE_FOR_DEFINED_FONT", Category::Appearance),
    ("THERMODYNAMIC_TEMPERATURE_UNIT", Category::Units),
    ("TIME_MEASURE_WITH_UNIT", Category::Units),
    ("TIME_UNIT", Category::Units),
    ("TOLERANCE_VALUE", Category::Pmi),
    ("TOLERANCE_ZONE", Category::Pmi),
    ("TOLERANCE_ZONE_FORM", Category::Pmi),
    ("TOROIDAL_SURFACE", Category::Geometry),
    ("TOTAL_RUNOUT_TOLERANCE", Category::Pmi),
    ("TRIANGULATED_FACE", Category::Tessellated),
    ("TRIANGULATED_SURFACE_SET", Category::Tessellated),
    ("TRIMMED_CURVE", Category::Geometry),
    ("TYPE_QUALIFIER", Category::Pmi),
    ("UNCERTAINTY_MEASURE_WITH_UNIT", Category::Units),
    ("UNEQUALLY_DISPOSED_GEOMETRIC_TOLERANCE", Category::Pmi),
    ("UNIFORM_CURVE", Category::Geometry),
    ("UNIFORM_SURFACE", Category::Geometry),
    ("VALUE_FORMAT_TYPE_QUALIFIER", Category::Pmi),
    ("VALUE_REPRESENTATION_ITEM", Category::Properties),
    ("VECTOR", Category::Geometry),
    ("VERTEX", Category::Topology),
    ("VERTEX_LOOP", Category::Topology),
    ("VERTEX_POINT", Category::Topology),
    ("VERTEX_SHELL", Category::Topology),
    ("VOLUME_MEASURE_WITH_UNIT", Category::Units),
    ("VOLUME_UNIT", Category::Units),
    ("WIRE_SHELL", Category::Topology),
];

#[cfg(test)]
mod tests {
    use super::Category::*;
    use super::*;
    use crate::part21::parse;

    #[test]
    fn the_table_is_sorted_and_the_names_are_stable() {
        assert!(
            TABLE.windows(2).all(|w| w[0].0 < w[1].0),
            "sorted, no duplicates"
        );
        let names: Vec<&str> = Category::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            names,
            [
                "geometry",
                "topology",
                "product_structure",
                "units",
                "appearance",
                "layers",
                "properties",
                "materials",
                "pmi",
                "validation",
                "external_references",
                "management",
                "tessellated",
                "kinematics",
                "unknown"
            ]
        );
    }

    /// Every entity name `schema`'s decoder types (and Task 5's) is in
    /// the table: a typed instance is never `unknown`.
    #[test]
    fn every_name_the_decoder_types_has_a_category() {
        const DECODED: &[&str] = &[
            "ADVANCED_BREP_SHAPE_REPRESENTATION",
            "ADVANCED_FACE",
            "APPLICATION_CONTEXT",
            "AXIS1_PLACEMENT",
            "AXIS2_PLACEMENT_2D",
            "AXIS2_PLACEMENT_3D",
            "BEZIER_CURVE",
            "BEZIER_SURFACE",
            "BOUNDARY_CURVE",
            "BREP_WITH_VOIDS",
            "B_SPLINE_CURVE",
            "B_SPLINE_CURVE_WITH_KNOTS",
            "B_SPLINE_SURFACE",
            "B_SPLINE_SURFACE_WITH_KNOTS",
            "CARTESIAN_POINT",
            "CARTESIAN_TRANSFORMATION_OPERATOR_3D",
            "CIRCLE",
            "CLOSED_SHELL",
            "COMPOSITE_CURVE",
            "COMPOSITE_CURVE_ON_SURFACE",
            "COMPOSITE_CURVE_SEGMENT",
            "CONICAL_SURFACE",
            "CONNECTED_FACE_SET",
            "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
            "CONVERSION_BASED_UNIT",
            "CURVE",
            "CYLINDRICAL_SURFACE",
            "DEFINITIONAL_REPRESENTATION",
            "DEGENERATE_TOROIDAL_SURFACE",
            "DIRECTION",
            "EDGE_BASED_WIREFRAME_SHAPE_REPRESENTATION",
            "EDGE_CURVE",
            "EDGE_LOOP",
            "ELLIPSE",
            "FACE",
            "FACETED_BREP",
            "FACETED_BREP_SHAPE_REPRESENTATION",
            "FACE_BASED_SURFACE_MODEL",
            "FACE_BOUND",
            "FACE_OUTER_BOUND",
            "FACE_SURFACE",
            "GEOMETRICALLY_BOUNDED_SURFACE_SHAPE_REPRESENTATION",
            "GEOMETRICALLY_BOUNDED_WIREFRAME_SHAPE_REPRESENTATION",
            "GEOMETRIC_REPRESENTATION_CONTEXT",
            "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT",
            "GLOBAL_UNIT_ASSIGNED_CONTEXT",
            "HYPERBOLA",
            "INTERSECTION_CURVE",
            "ITEM_DEFINED_TRANSFORMATION",
            "LENGTH_MEASURE_WITH_UNIT",
            "LENGTH_UNIT",
            "LINE",
            "MANIFOLD_SOLID_BREP",
            "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
            "MAPPED_ITEM",
            "MEASURE_WITH_UNIT",
            "NAMED_UNIT",
            "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
            "NON_MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
            "OFFSET_CURVE_2D",
            "OFFSET_CURVE_3D",
            "OFFSET_SURFACE",
            "OPEN_SHELL",
            "ORIENTED_CLOSED_SHELL",
            "ORIENTED_EDGE",
            "ORIENTED_FACE",
            "ORIENTED_OPEN_SHELL",
            "OUTER_BOUNDARY_CURVE",
            "PARABOLA",
            "PCURVE",
            "PLANE",
            "PLANE_ANGLE_MEASURE_WITH_UNIT",
            "PLANE_ANGLE_UNIT",
            "POLYLINE",
            "POLY_LOOP",
            "PRODUCT",
            "PRODUCT_DEFINITION",
            "PRODUCT_DEFINITION_FORMATION",
            "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
            "PRODUCT_DEFINITION_SHAPE",
            "PRODUCT_DEFINITION_WITH_ASSOCIATED_DOCUMENTS",
            "QUASI_UNIFORM_CURVE",
            "QUASI_UNIFORM_SURFACE",
            "RATIONAL_B_SPLINE_CURVE",
            "RATIONAL_B_SPLINE_SURFACE",
            "RECTANGULAR_TRIMMED_SURFACE",
            "REPARAMETRISED_COMPOSITE_CURVE_SEGMENT",
            "REPRESENTATION",
            "REPRESENTATION_CONTEXT",
            "REPRESENTATION_MAP",
            "REPRESENTATION_RELATIONSHIP",
            "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION",
            "SEAM_CURVE",
            "SHAPE_DEFINITION_REPRESENTATION",
            "SHAPE_REPRESENTATION",
            "SHAPE_REPRESENTATION_RELATIONSHIP",
            "SHAPE_REPRESENTATION_WITH_PARAMETERS",
            "SHELL_BASED_SURFACE_MODEL",
            "SHELL_BASED_WIREFRAME_SHAPE_REPRESENTATION",
            "SI_UNIT",
            "SOLID_ANGLE_UNIT",
            "SPHERICAL_SURFACE",
            "SURFACE",
            "SURFACE_CURVE",
            "SURFACE_OF_LINEAR_EXTRUSION",
            "SURFACE_OF_REVOLUTION",
            "TESSELLATED_SHAPE_REPRESENTATION",
            "TOROIDAL_SURFACE",
            "TRIMMED_CURVE",
            "UNCERTAINTY_MEASURE_WITH_UNIT",
            "UNIFORM_CURVE",
            "UNIFORM_SURFACE",
            "VECTOR",
            "VERTEX_LOOP",
            "VERTEX_POINT",
        ];
        for n in DECODED {
            assert_ne!(category_of(n), Unknown, "{n}");
        }
        for n in [
            "DOCUMENT",
            "DOCUMENT_FILE",
            "DOCUMENT_TYPE",
            "DOCUMENT_REPRESENTATION_TYPE",
            "APPLIED_DOCUMENT_REFERENCE",
            "DOCUMENT_PRODUCT_EQUIVALENCE",
            "APPLIED_EXTERNAL_IDENTIFICATION_ASSIGNMENT",
            "PROPERTY_DEFINITION",
            "PROPERTY_DEFINITION_REPRESENTATION",
            "DESCRIPTIVE_REPRESENTATION_ITEM",
        ] {
            assert_ne!(category_of(n), Unknown, "{n}");
        }
    }

    #[test]
    fn complex_instances_take_their_most_specific_part_and_properties_their_name() {
        let f = parse(
            b"ISO-10303-21;HEADER;FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;DATA;\
              #1=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\
              #2=(BOUNDED_SURFACE()B_SPLINE_SURFACE(1,1,((#9,#9),(#9,#9)),.UNSPECIFIED.,.F.,.F.,.F.)SURFACE());\
              #3=(GEOMETRIC_TOLERANCE('','',$,#9)GEOMETRIC_TOLERANCE_WITH_DATUM_REFERENCE((#9))POSITION_TOLERANCE());\
              #4=(LENGTH_MEASURE_WITH_UNIT()MEASURE_REPRESENTATION_ITEM()MEASURE_WITH_UNIT(LENGTH_MEASURE(1.),#1)REPRESENTATION_ITEM('d'));\
              #5=PROPERTY_DEFINITION('geometric validation property','volume',#9);\
              #6=PROPERTY_DEFINITION('user defined attributes','',#9);\
              #7=PROPERTY_DEFINITION('Material Property','density',#9);\
              #8=SOMETHING_NEW('x');\
              #9=CARTESIAN_POINT('',(0.,0.,0.));\
              ENDSEC;END-ISO-10303-21;",
        )
        .unwrap();
        let c: Vec<Category> = f
            .instances
            .iter()
            .map(|i| category_of_record(&i.record))
            .collect();
        assert_eq!(
            c,
            [
                Units, Geometry, Pmi, Properties, Validation, Properties, Materials, Unknown,
                Geometry
            ]
        );
        assert_eq!(
            joined(f.instances[0].record.parts()),
            "LENGTH_UNIT NAMED_UNIT SI_UNIT"
        );
    }

    /// A product with a shape, a stray point, an approval of the
    /// definition and a validation property of its shape.
    const SMALL: &[u8] = b"ISO-10303-21;HEADER;FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;DATA;\
        #1=APPLICATION_CONTEXT('');\
        #2=PRODUCT_CONTEXT('',#1,'');\
        #3=PRODUCT('P','P','',(#2));\
        #4=PRODUCT_DEFINITION_FORMATION('','',#3);\
        #5=PRODUCT_DEFINITION_CONTEXT('',#1,'');\
        #6=PRODUCT_DEFINITION('design','',#4,#5);\
        #7=PRODUCT_DEFINITION_SHAPE('','',#6);\
        #8=SHAPE_DEFINITION_REPRESENTATION(#7,#9);\
        #9=SHAPE_REPRESENTATION('',(#10),#11);\
        #10=AXIS2_PLACEMENT_3D('',#12,$,$);\
        #11=REPRESENTATION_CONTEXT('','');\
        #12=CARTESIAN_POINT('',(0.,0.,0.));\
        #13=CARTESIAN_POINT('stray',(1.,0.,0.));\
        #14=APPROVAL(#15,'');\
        #15=APPROVAL_STATUS('approved');\
        #16=CC_DESIGN_APPROVAL(#14,(#6));\
        #17=PROPERTY_DEFINITION('geometric validation property','volume',#7);\
        #18=PROPERTY_DEFINITION_REPRESENTATION(#17,#19);\
        #19=REPRESENTATION('volume',(#20),#11);\
        #20=MEASURE_REPRESENTATION_ITEM('volume measure',VOLUME_MEASURE(1.),#21);\
        #21=DIMENSIONAL_EXPONENTS(3.,0.,0.,0.,0.,0.,0.);\
        ENDSEC;END-ISO-10303-21;";

    #[test]
    fn instances_are_named_categorised_and_anchored() {
        let f = parse(SMALL).unwrap();
        let (e, c) = crate::schema::Entities::decode_counted(&f);
        assert_eq!(
            e,
            crate::schema::Entities::decode(&f),
            "decoding doesn't change"
        );
        assert_eq!(c.len(), 21);
        let cat = |id| c.category_at(id).unwrap();
        for id in 1..=8 {
            assert_eq!(cat(id), ProductStructure, "#{id}");
        }
        assert_eq!(
            [cat(9), cat(10), cat(11), cat(12), cat(13)],
            [Geometry, Geometry, Units, Geometry, Geometry]
        );
        assert_eq!([cat(14), cat(15), cat(16)], [Management; 3]);
        // The property's representation and its item take its category;
        // the item's unit and the context don't.
        assert_eq!(
            [cat(17), cat(18), cat(19), cat(20), cat(21)],
            [Validation, Validation, Validation, Validation, Units]
        );
        assert_eq!(c.name_at(13), Some("CARTESIAN_POINT"));
        for id in 1..=21 {
            assert_eq!(c.anchored_at(id), Some(id != 13), "#{id}");
        }
        assert_eq!(c.category_at(99), None);
    }

    #[test]
    fn the_census_is_the_same_on_one_thread_and_many() {
        let mut data = String::from("#1=CARTESIAN_POINT('',(0.,0.,0.));");
        for k in 2..=3000 {
            data.push_str(&format!("#{k}=VERTEX_POINT('',#{});", k - 1));
        }
        let text = format!(
            "ISO-10303-21;HEADER;FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;DATA;{data}ENDSEC;END-ISO-10303-21;"
        );
        let f = parse(text.as_bytes()).unwrap();
        let many = crate::schema::Entities::decode_counted(&f).1;
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| crate::schema::Entities::decode_counted(&f).1);
        assert_eq!(many, one);
        assert_eq!(
            many.anchored_at(1),
            Some(false),
            "no product, representation or style"
        );
    }

    fn bucket<'t>(t: &'t Transfer, c: Category, b: Bucket) -> &'t Counted {
        let line = t.lines.iter().find(|l| l.category == c).expect("listed");
        &line.buckets[b as usize]
    }

    #[test]
    fn an_import_buckets_every_instance() {
        let f = parse(SMALL).unwrap();
        let (_, c) = crate::schema::Entities::decode_counted(&f);
        let t = c.transfer(
            &Read {
                roots: vec![9],
                product_tree: true,
            },
            &[Failed {
                instance: 12,
                code: "import_failed".into(),
            }],
        );
        assert_eq!(t.instances, 21);
        let order: Vec<Category> = t.lines.iter().map(|l| l.category).collect();
        assert_eq!(
            order,
            [Geometry, ProductStructure, Units, Validation, Management]
        );
        assert_eq!(bucket(&t, ProductStructure, Bucket::Interpreted).count, 8);
        // The shape and its placement; the point it names failed; the
        // stray point is unreferenced.
        assert_eq!(bucket(&t, Geometry, Bucket::Interpreted).count, 2);
        assert_eq!(bucket(&t, Geometry, Bucket::Failed).count, 1);
        let stray = bucket(&t, Geometry, Bucket::Unreferenced);
        assert_eq!(
            (stray.count, stray.names[0].first.as_slice()),
            (1, &[13][..])
        );
        let geometry = t.lines.iter().find(|l| l.category == Geometry).unwrap();
        assert_eq!(geometry.codes, [("import_failed".to_owned(), 1)]);
        assert_eq!(
            bucket(&t, Units, Bucket::Interpreted).count,
            1,
            "the context"
        );
        assert_eq!(
            bucket(&t, Units, Bucket::Retained).count,
            1,
            "the property's unit"
        );
        assert_eq!(bucket(&t, Validation, Bucket::Retained).count, 4);
        assert_eq!(bucket(&t, Management, Bucket::Retained).count, 3);
        // Nothing read: everything anchored is retained.
        let none = c.transfer(&Read::default(), &[]);
        assert_eq!(bucket(&none, ProductStructure, Bucket::Retained).count, 8);
        assert_eq!(bucket(&none, Geometry, Bucket::Interpreted).count, 0);
    }

    #[test]
    fn names_are_largest_first_and_keep_their_first_instances() {
        let mut data = String::new();
        for k in 1..=100 {
            data.push_str(&format!("#{k}=CARTESIAN_POINT('',(0.,0.,0.));"));
        }
        data.push_str("#101=DIRECTION('',(0.,0.,1.));");
        let text = format!(
            "ISO-10303-21;HEADER;FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;DATA;{data}ENDSEC;END-ISO-10303-21;"
        );
        let (_, c) = crate::schema::Entities::decode_counted(&parse(text.as_bytes()).unwrap());
        let t = c.transfer(&Read::default(), &[]);
        let u = bucket(&t, Geometry, Bucket::Unreferenced);
        assert_eq!(u.count, 101);
        assert_eq!(u.names[0].name, "CARTESIAN_POINT");
        assert_eq!(u.names[0].count, 100);
        assert_eq!(u.names[0].first, (1..=64).collect::<Vec<u64>>());
        assert_eq!(u.names[1].name, "DIRECTION");
    }
}
