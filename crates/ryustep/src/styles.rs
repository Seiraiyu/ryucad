//! Styles (design 448, plan 24.B): the colours and transparencies a
//! file gives its faces, bodies and occurrences, as ISO 10303-46 and
//! CAx-IF's *Model Styling and Organization* give them.
//!
//! [`read`] follows every `STYLED_ITEM` (over-riding and context
//! dependent ones too) through its `PRESENTATION_STYLE_ASSIGNMENT`s to
//! the surface style a face can take: a `SURFACE_STYLE_USAGE` on both
//! sides or the positive one, its `SURFACE_SIDE_STYLE`, a
//! `SURFACE_STYLE_FILL_AREA`'s `FILL_AREA_STYLE_COLOUR` (a `COLOUR_RGB`
//! or one of CAx-IF's eight `DRAUGHTING_PRE_DEFINED_COLOUR`s), and a
//! `SURFACE_STYLE_RENDERING_WITH_PROPERTIES`' `SURFACE_STYLE_TRANSPARENT`
//! for its transparency alone. A styled item with nothing a face can take
//! (curve and point styles, back sides, `NULL_STYLE`) is `retained`; one
//! whose chain or context doesn't read is `failed`, and fails nothing
//! else. Colours round to the nearest 8-bit channel and opacities
//! (1 − transparency) to six decimals, so what ryucad writes reads back
//! as it was.

use crate::ImportError;
use crate::schema::{Entities, Entity, Id, Side, wrong};
use std::collections::BTreeMap;

/// A surface style: an sRGB colour and an opacity, either unset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    pub colour: Option<[u8; 3]>,
    pub opacity: Option<f64>,
}

impl Style {
    /// Its fields where set, `under`'s where not.
    pub fn over(self, under: Option<Style>) -> Style {
        let u = under.unwrap_or_default();
        Style {
            colour: self.colour.or(u.colour),
            opacity: self.opacity.or(u.opacity),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.colour.is_none() && self.opacity.is_none()
    }
}

/// The smallest opacity a transparency reads as: a face ryucad draws is
/// never wholly invisible.
pub const MIN_OPACITY: f64 = 0.001;

/// A colour component as the nearest 8-bit channel (`[0, 1]`, clamped).
pub fn channel(x: f64) -> u8 {
    let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
    (x * 255.0 + 0.5).floor() as u8
}

/// The opacity a transparency gives: `1 − t`, to six decimals, in
/// `[MIN_OPACITY, 1]`.
pub fn opacity_of(t: f64) -> f64 {
    let o = 1.0 - if t.is_nan() { 0.0 } else { t };
    ((o * 1e6 + 0.5).floor() / 1e6).clamp(MIN_OPACITY, 1.0)
}

/// The transparency written for an opacity.
pub fn transparency_of(opacity: f64) -> f64 {
    1.0 - opacity
}

/// CAx-IF's eight predefined colours, by name in any case.
pub fn predefined(name: &str) -> Option<[u8; 3]> {
    Some(match name.to_ascii_lowercase().as_str() {
        "red" => [255, 0, 0],
        "green" => [0, 255, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "magenta" => [255, 0, 255],
        "cyan" => [0, 255, 255],
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        _ => return None,
    })
}

/// One styled item read.
#[derive(Clone, Debug, PartialEq)]
pub struct Assigned {
    /// The styled item.
    pub instance: Id,
    /// What it styles: a face, a B-rep item or shell, a mapped item or a
    /// representation.
    pub item: Id,
    pub style: Style,
    pub over_riding: bool,
    /// A context dependent one's usage path:
    /// `NEXT_ASSEMBLY_USAGE_OCCURRENCE`s from the outermost assembly
    /// down. Empty for any other.
    pub path: Vec<Id>,
    /// Every instance its style was read from: the styled item, then the
    /// rest ascending.
    pub instances: Vec<Id>,
}

/// Every styled item of a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Styles {
    /// Those with a style a face takes, by instance.
    pub assigned: Vec<Assigned>,
    /// For each item a style that isn't context dependent styles, the
    /// one it takes (an index into `assigned`): an over-riding one before
    /// a plain one, then the lowest instance.
    pub plain: BTreeMap<Id, usize>,
    /// For each representation a `MAPPED_ITEM` maps, the mapped items, by
    /// instance.
    pub mapped: BTreeMap<Id, Vec<Id>>,
    /// Styled items with nothing a face takes, ascending.
    pub retained: Vec<Id>,
    /// Styled items whose style or context doesn't read, with why.
    pub failed: Vec<(Id, ImportError)>,
}

/// Whether `name` (a record's entity name, a complex instance's joined)
/// is a styled item's.
fn is_styled(name: &str) -> bool {
    name.split(' ').any(|n| {
        matches!(
            n,
            "STYLED_ITEM" | "OVER_RIDING_STYLED_ITEM" | "CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM"
        )
    })
}

/// Every styled item of `e`; `name_of` names each instance as its record
/// does (the census's names), so a styled item that doesn't decode is
/// `failed` too.
pub fn read<'n>(e: &Entities, name_of: &(dyn Fn(Id) -> Option<&'n str> + Sync)) -> Styles {
    let mut out = Styles::default();
    // The few instances read here, found in one parallel pass: styled
    // items, mapped items, and what failed to decode that names itself a
    // styled item.
    let found = e.select(|id, x| match x {
        Ok(Entity::StyledItem { .. } | Entity::MappedItem { .. }) => true,
        Ok(_) => false,
        Err(_) => name_of(id).is_some_and(is_styled),
    });
    // Each shape representation's shape definitions, read once and only
    // for a file whose styles hold in a context.
    let mut shapes: Option<BTreeMap<Id, Vec<Id>>> = None;
    for id in found {
        let x = match e.get(id, id) {
            Ok(x) => x,
            Err(err) => {
                if name_of(id).is_some_and(is_styled) {
                    out.failed.push((id, err));
                }
                continue;
            }
        };
        if let Entity::MappedItem { source, .. } = x
            && let Ok(Entity::RepresentationMap { representation, .. }) = e.get(id, *source)
        {
            out.mapped.entry(*representation).or_default().push(id);
            continue;
        }
        let Entity::StyledItem {
            styles,
            item,
            over_riding,
            context,
        } = x
        else {
            continue;
        };
        let mut by_context = Vec::new();
        let read = assigned(e, id, styles, &mut by_context).and_then(|style| {
            let path = match context {
                Some(c) => usage_path(e, id, c)?,
                None if !by_context.is_empty() => {
                    let shapes = shapes.get_or_insert_with(|| shape_definitions(e));
                    by_context_path(e, id, &by_context, shapes)?
                }
                None => Vec::new(),
            };
            Ok((style, path))
        });
        // A style by context replaces its item's style whole, as OCCT's
        // XDE means it: opaque where it gives no transparency.
        let read = read.map(|(style, path)| {
            let whole = context.is_none() && !path.is_empty();
            let style = style.map(|(mut s, used)| {
                if whole {
                    s.opacity = s.opacity.or(Some(1.0));
                }
                (s, used)
            });
            (style, path)
        });
        match read {
            Ok((Some((style, mut instances)), path)) => {
                instances.sort_unstable();
                instances.dedup();
                instances.retain(|&i| i != id);
                instances.insert(0, id);
                if path.is_empty() {
                    let k = out.assigned.len();
                    match out.plain.get(item) {
                        Some(&j) if out.assigned[j].over_riding || !*over_riding => {}
                        _ => drop(out.plain.insert(*item, k)),
                    }
                }
                out.assigned.push(Assigned {
                    instance: id,
                    item: *item,
                    style,
                    over_riding: *over_riding,
                    path,
                    instances,
                });
            }
            Ok((None, _)) => out.retained.push(id),
            Err(err) => out.failed.push((id, err)),
        }
    }
    out
}

/// The surface style a styled item's assignments give a face, and the
/// instances it came from; `None` for one with nothing a face takes.
/// A `PRESENTATION_STYLE_BY_CONTEXT`'s context is added to `contexts`.
fn assigned(
    e: &Entities,
    id: Id,
    styles: &[Id],
    contexts: &mut Vec<Id>,
) -> Result<Option<(Style, Vec<Id>)>, ImportError> {
    let mut used = Vec::new();
    let mut style = Style::default();
    for &psa in styles {
        let styles = match e.get(id, psa)? {
            Entity::StyleAssignment { styles } => styles,
            Entity::StyleByContext { styles, context } => {
                contexts.push(*context);
                styles
            }
            _ => return Err(wrong(psa, "a PRESENTATION_STYLE_ASSIGNMENT")),
        };
        for &s in styles {
            let Entity::SurfaceStyleUsage { side, style: sss } = e.get(psa, s)? else {
                continue;
            };
            // A back side's style: ryucad draws no back sides.
            if *side == Side::Negative {
                continue;
            }
            let got = side_style(e, s, *sss, &mut used)?;
            if !got.is_empty() {
                style = style.over(Some(got));
                used.extend([psa, s]);
            }
        }
    }
    Ok((!style.is_empty()).then_some((style, used)))
}

fn side_style(e: &Entities, from: Id, sss: Id, used: &mut Vec<Id>) -> Result<Style, ImportError> {
    let Entity::SurfaceSideStyle { styles } = e.get(from, sss)? else {
        return Err(wrong(sss, "a SURFACE_SIDE_STYLE"));
    };
    let mut out = Style::default();
    for &el in styles {
        match e.get(sss, el)? {
            Entity::SurfaceFill { fill } => {
                if let Some(c) = fill_colour(e, el, *fill, used)? {
                    out.colour = out.colour.or(Some(c));
                    used.extend([el, *fill]);
                }
            }
            Entity::Rendering { properties } => {
                for &p in properties {
                    if let Entity::Transparent { transparency } = e.get(el, p)? {
                        out.opacity = out.opacity.or(Some(opacity_of(*transparency)));
                        used.extend([el, p]);
                    }
                }
            }
            _ => {}
        }
    }
    if !out.is_empty() {
        used.push(sss);
    }
    Ok(out)
}

fn fill_colour(e: &Entities, from: Id, fas: Id, used: &mut Vec<Id>) -> Result<Option<[u8; 3]>, ImportError> {
    let Entity::FillAreaStyle { styles } = e.get(from, fas)? else {
        return Err(wrong(fas, "a FILL_AREA_STYLE"));
    };
    for &s in styles {
        let Entity::FillColour { colour } = e.get(fas, s)? else {
            continue;
        };
        let c = match e.get(s, *colour)? {
            Entity::ColourRgb { rgb } => Some(rgb.map(channel)),
            Entity::PredefinedColour { name } => predefined(name),
            _ => None,
        };
        if let Some(c) = c {
            used.extend([s, *colour]);
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// The usage path an over-riding style's context names, outermost
/// first: each `NEXT_ASSEMBLY_USAGE_OCCURRENCE`, a
/// `SPECIFIED_HIGHER_USAGE_OCCURRENCE`'s chain, or a
/// `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION`'s usage, joined in order (and
/// reversed when the file lists them innermost first). Anything else in
/// a context isn't a usage. A path whose usages don't run parent to
/// child is `Malformed`.
fn usage_path(e: &Entities, id: Id, context: &[Id]) -> Result<Vec<Id>, ImportError> {
    let mut path = Vec::new();
    for &c in context {
        usages(e, id, c, &mut path, 0)?;
    }
    let chains = |p: &[Id]| -> Result<bool, ImportError> {
        for w in p.windows(2) {
            let (Entity::Usage { child, .. }, Entity::Usage { parent, .. }) =
                (e.get(id, w[0])?, e.get(id, w[1])?)
            else {
                return Ok(false);
            };
            if child != parent {
                return Ok(false);
            }
        }
        Ok(true)
    };
    if chains(&path)? {
        return Ok(path);
    }
    path.reverse();
    if chains(&path)? {
        return Ok(path);
    }
    Err(ImportError::Malformed {
        instance: id,
        attribute: 4,
        what: "a style's context isn't one usage path",
    })
}

/// For each representation a `SHAPE_DEFINITION_REPRESENTATION` names,
/// the shapes (`PRODUCT_DEFINITION_SHAPE`s) it represents.
fn shape_definitions(e: &Entities) -> BTreeMap<Id, Vec<Id>> {
    let mut out: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for id in e.select(|_, x| matches!(x, Ok(Entity::ShapeDefinition { .. }))) {
        if let Ok(Entity::ShapeDefinition {
            definition,
            representation,
        }) = e.get(id, id)
        {
            out.entry(*representation).or_default().push(*definition);
        }
    }
    out
}

/// The usage path the contexts of a styled item's
/// `PRESENTATION_STYLE_BY_CONTEXT`s name, as OCCT writes a component's
/// colour: each context a usage (or what [`usage_path`] takes), or a
/// shape representation of a usage's `PRODUCT_DEFINITION_SHAPE`. Empty
/// where none names a usage: then the style holds wherever its item is.
fn by_context_path(
    e: &Entities,
    id: Id,
    contexts: &[Id],
    shapes: &BTreeMap<Id, Vec<Id>>,
) -> Result<Vec<Id>, ImportError> {
    let mut named = Vec::new();
    for &c in contexts {
        let mut found = Vec::new();
        usages(e, id, c, &mut found, 0)?;
        if found.is_empty() {
            for &pds in shapes.get(&c).into_iter().flatten() {
                if let Ok(Entity::DefinitionShape { definition }) = e.get(c, pds)
                    && let Ok(Entity::Usage { .. }) = e.get(pds, *definition)
                {
                    found.push(*definition);
                }
            }
            found.dedup();
        }
        named.extend(found);
    }
    if named.is_empty() {
        return Ok(named);
    }
    usage_path(e, id, &named)
}

fn usages(e: &Entities, from: Id, c: Id, out: &mut Vec<Id>, depth: usize) -> Result<(), ImportError> {
    if depth > 64 {
        return Err(ImportError::Malformed {
            instance: from,
            attribute: 4,
            what: "a style's context nests too deep",
        });
    }
    match e.get(from, c)? {
        Entity::Usage { .. } => out.push(c),
        Entity::HigherUsage { upper, next } => {
            usages(e, c, *upper, out, depth + 1)?;
            out.push(*next);
        }
        Entity::ContextDependentShape { product_shape, .. } => {
            if let Entity::DefinitionShape { definition } = e.get(c, *product_shape)?
                && let Entity::Usage { .. } = e.get(*product_shape, *definition)?
            {
                out.push(*definition);
            }
        }
        _ => {}
    }
    Ok(())
}

/// The styles a body takes from its file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BodyStyles {
    /// Its own: a style on its representation (or the first holding its
    /// items), else on its first B-rep item, or that item's shells, that
    /// has one, else on a mapped item mapping its representation.
    pub body: Option<Style>,
    /// Its faces', by face instance: a face's own, else its B-rep item's
    /// where that differs from the body's.
    pub faces: BTreeMap<Id, Style>,
    /// Every instance read for them, ascending.
    pub instances: Vec<Id>,
}

impl crate::Opened {
    /// Every styled item of the file, read once.
    pub fn styles(&self) -> &Styles {
        self.styles
            .get_or_init(|| read(&self.entities, &|id| self.census.name_at(id)))
    }

    /// The styles `held[k]` takes, none of them context dependent.
    pub fn body_styles(&self, k: usize) -> BodyStyles {
        let st = self.styles();
        if st.assigned.is_empty() {
            return BodyStyles::default();
        }
        let h = &self.held[k];
        let first = &self.held[self.first[k]];
        let mut used = Vec::new();
        let take = |id: Id, used: &mut Vec<Id>| -> Option<Style> {
            let a = &st.assigned[*st.plain.get(&id)?];
            used.extend(&a.instances);
            Some(a.style)
        };
        let items: Vec<(Id, Option<Style>)> = h
            .items
            .iter()
            .map(|&item| {
                let own = take(item, &mut used).or_else(|| {
                    self.shells_of(item)
                        .into_iter()
                        .find_map(|s| take(s, &mut used))
                });
                (item, own)
            })
            .collect();
        let rep = take(h.representation, &mut used)
            .or_else(|| {
                (first.representation != h.representation)
                    .then(|| take(first.representation, &mut used))
                    .flatten()
            })
            .or_else(|| {
                st.mapped
                    .get(&h.representation)
                    .into_iter()
                    .flatten()
                    .find_map(|&m| take(m, &mut used))
            });
        let body = rep.or_else(|| items.iter().find_map(|(_, s)| *s));
        let mut faces = BTreeMap::new();
        for &(item, own) in &items {
            for face in self.faces_of(item) {
                if let Some(s) = take(face, &mut used) {
                    faces.insert(face, s);
                } else if let Some(s) = own.filter(|s| Some(*s) != body) {
                    faces.insert(face, s);
                }
            }
        }
        used.sort_unstable();
        used.dedup();
        BodyStyles {
            body,
            faces,
            instances: used,
        }
    }

    /// The shells of B-rep item `item`, in its order: a solid's outer
    /// shell and voids, a surface model's shells or face sets.
    pub fn shells_of(&self, item: Id) -> Vec<Id> {
        use crate::schema::Brep;
        let e = &self.entities;
        match e.get(item, item) {
            Ok(Entity::Brep(Brep::Solid { outer, voids, .. })) => {
                let mut out = vec![*outer];
                for &v in voids {
                    match e.get(item, v) {
                        Ok(Entity::OrientedShell { shell, .. }) => out.push(*shell),
                        _ => out.push(v),
                    }
                }
                out
            }
            Ok(Entity::Brep(Brep::Shells(s))) | Ok(Entity::Brep(Brep::Faces(s))) => s.clone(),
            _ => Vec::new(),
        }
    }

    /// The faces of B-rep item `item`, ascending.
    pub fn faces_of(&self, item: Id) -> Vec<Id> {
        let e = &self.entities;
        let mut out = Vec::new();
        for shell in self.shells_of(item) {
            let faces = match e.get(item, shell) {
                Ok(Entity::Shell { faces, .. }) | Ok(Entity::FaceSet { faces }) => faces.clone(),
                Ok(Entity::OrientedShell { shell, .. }) => match e.get(item, *shell) {
                    Ok(Entity::Shell { faces, .. }) => faces.clone(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            for f in faces {
                match e.get(shell, f) {
                    Ok(Entity::OrientedFace { face, .. }) => out.push(*face),
                    _ => out.push(f),
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// What an occurrence style styles, in the product its usage path ends
/// at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A body of `held` (by index) whose definition is that product's.
    Body(usize),
    /// A face of such a body.
    Face { held: usize, face: Id },
    /// The product's shape representation: everything it shows.
    Product,
    /// Nothing that product has, or a path the file's tree doesn't hold.
    Unplaced,
}

/// A context dependent style.
#[derive(Clone, Debug, PartialEq)]
pub struct OccurrenceStyle {
    /// Its usages as (parent definition, index into its `children`),
    /// from the outermost; empty when the tree doesn't hold them.
    pub path: Vec<(usize, usize)>,
    /// Its usages' instances, as the file names them.
    pub usages: Vec<Id>,
    pub target: Target,
    pub style: Style,
    /// The styled item, and every instance its style was read from.
    pub instance: Id,
    pub instances: Vec<Id>,
}

impl crate::Opened {
    /// Every context dependent style of the file, by instance.
    pub fn occurrence_styles(&self) -> Vec<OccurrenceStyle> {
        let st = self.styles();
        let mut at: BTreeMap<Id, (usize, usize)> = BTreeMap::new();
        for (k, d) in self.products.definitions.iter().enumerate() {
            for (n, c) in d.children.iter().enumerate() {
                at.insert(c.instance, (k, n));
            }
        }
        st.assigned
            .iter()
            .filter(|a| !a.path.is_empty())
            .map(|a| {
                let path: Option<Vec<(usize, usize)>> =
                    a.path.iter().map(|u| at.get(u).copied()).collect();
                let target = match &path {
                    Some(p) => {
                        let (k, n) = *p.last().expect("not empty");
                        self.target_in(self.products.definitions[k].children[n].definition, a.item)
                    }
                    None => Target::Unplaced,
                };
                OccurrenceStyle {
                    path: path.unwrap_or_default(),
                    usages: a.path.clone(),
                    target,
                    style: a.style,
                    instance: a.instance,
                    instances: a.instances.clone(),
                }
            })
            .collect()
    }

    /// What `item` is in definition `leaf`.
    fn target_in(&self, leaf: usize, item: Id) -> Target {
        for (k, h) in self
            .held
            .iter()
            .enumerate()
            .filter(|(_, h)| h.definition == Some(leaf))
        {
            if h.representation == item {
                return Target::Body(k);
            }
            for &i in &h.items {
                if i == item || self.shells_of(i).contains(&item) {
                    return Target::Body(k);
                }
                if self.faces_of(i).binary_search(&item).is_ok() {
                    return Target::Face {
                        held: k,
                        face: item,
                    };
                }
            }
        }
        if self.products.definitions[leaf]
            .shapes
            .iter()
            .any(|s| s.representation == item)
        {
            return Target::Product;
        }
        Target::Unplaced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::part21::parse;
    use crate::schema::Entities;

    /// The styles of a file whose data section is `data`, every instance
    /// named as its record names it.
    pub(crate) fn styles_of(data: &str) -> Styles {
        let text = format!(
            "ISO-10303-21;HEADER;FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;\
             DATA;{data}ENDSEC;END-ISO-10303-21;"
        );
        let file = parse(text.as_bytes()).unwrap();
        let names: BTreeMap<Id, String> = file
            .instances
            .iter()
            .map(|i| (i.id, crate::census::joined(i.record.parts())))
            .collect();
        read(&Entities::decode(&file), &|id| names.get(&id).map(String::as_str))
    }

    /// One fill colour `#1` and its chain to assignment `#9`; `extra`
    /// adds the rest.
    fn chain(rgb: &str, extra: &str) -> String {
        format!(
            "#1=COLOUR_RGB('',{rgb});#2=FILL_AREA_STYLE_COLOUR('',#1);\
             #3=FILL_AREA_STYLE('',(#2));#4=SURFACE_STYLE_FILL_AREA(#3);\
             #7=SURFACE_SIDE_STYLE('',(#4));#8=SURFACE_STYLE_USAGE(.BOTH.,#7);\
             #9=PRESENTATION_STYLE_ASSIGNMENT((#8));{extra}"
        )
    }

    #[test]
    fn colours_round_to_the_nearest_channel_and_back() {
        for k in 0..=255u8 {
            assert_eq!(channel(f64::from(k) / 255.0), k);
        }
        assert_eq!(channel(0.5), 128);
        assert_eq!(channel(-1.0), 0);
        assert_eq!(channel(2.0), 255);
        assert_eq!(channel(f64::NAN), 0);
        for o in [1.0, 0.5, 0.3, 0.123456, 0.001, 0.999] {
            assert_eq!(opacity_of(transparency_of(o)), o, "{o}");
        }
        assert_eq!(opacity_of(1.0), MIN_OPACITY);
        assert_eq!(opacity_of(-0.5), 1.0);
        assert_eq!(predefined("RED"), Some([255, 0, 0]));
        assert_eq!(predefined("white"), Some([255, 255, 255]));
        assert_eq!(predefined("mauve"), None);
    }

    #[test]
    fn a_styled_item_reads_its_colour_and_transparency() {
        let s = styles_of(&chain(
            "0.8,0.4,0.",
            "#5=SURFACE_STYLE_TRANSPARENT(0.25);\
             #6=SURFACE_STYLE_RENDERING_WITH_PROPERTIES(.NORMAL_SHADING.,#1,(#5));\
             #17=SURFACE_SIDE_STYLE('',(#4,#6));#18=SURFACE_STYLE_USAGE(.POSITIVE.,#17);\
             #19=PRESENTATION_STYLE_ASSIGNMENT((#18));\
             #20=STYLED_ITEM('',(#9),#100);#21=STYLED_ITEM('',(#19),#101);\
             #22=OVER_RIDING_STYLED_ITEM('',(#9),#101,#21);",
        ));
        assert_eq!(s.failed, []);
        let at = |item: Id| &s.assigned[s.plain[&item]];
        assert_eq!(
            at(100).style,
            Style {
                colour: Some([204, 102, 0]),
                opacity: None
            }
        );
        assert_eq!(at(100).instances, [20, 1, 2, 3, 4, 7, 8, 9]);
        // An over-riding style beats a plain one on the same item.
        assert_eq!(at(101).instance, 22);
        assert_eq!(s.assigned[1].style.opacity, Some(0.75));
        assert_eq!(s.assigned[1].style.colour, Some([204, 102, 0]));
    }

    #[test]
    fn styles_a_face_cannot_take_are_retained_and_broken_ones_failed() {
        let s = styles_of(&chain(
            "1.,0.,0.",
            "#10=SURFACE_STYLE_USAGE(.NEGATIVE.,#7);#11=PRESENTATION_STYLE_ASSIGNMENT((#10));\
             #12=CURVE_STYLE('',$,$,$);#13=PRESENTATION_STYLE_ASSIGNMENT((#12));\
             #14=PRESENTATION_STYLE_ASSIGNMENT((NULL_STYLE(.NULL.)));\
             #20=STYLED_ITEM('',(#11),#100);#21=STYLED_ITEM('',(#13),#100);\
             #22=STYLED_ITEM('',(#14),#100);#23=STYLED_ITEM('',(#99),#100);\
             #24=STYLED_ITEM('',(#9));#99=CARTESIAN_POINT('',(0.,0.,0.));",
        ));
        assert!(s.assigned.is_empty());
        assert_eq!(s.retained, [20, 21, 22]);
        let failed: Vec<Id> = s.failed.iter().map(|f| f.0).collect();
        assert_eq!(failed, [23, 24]);
    }

    #[test]
    fn predefined_colours_read_by_name() {
        let s = styles_of(
            "#1=DRAUGHTING_PRE_DEFINED_COLOUR('green');#2=FILL_AREA_STYLE_COLOUR('',#1);\
             #3=FILL_AREA_STYLE('',(#2));#4=SURFACE_STYLE_FILL_AREA(#3);\
             #7=SURFACE_SIDE_STYLE('',(#4));#8=SURFACE_STYLE_USAGE(.BOTH.,#7);\
             #9=PRESENTATION_STYLE_ASSIGNMENT((#8));#20=STYLED_ITEM('',(#9),#100);",
        );
        assert_eq!(s.assigned[0].style.colour, Some([0, 255, 0]));
    }

    /// A root `#50` using `#51` (`#40`), which uses `#52` (`#41`): a
    /// context of usages outermost first, innermost first, as a higher
    /// usage, and as shape relationships all read as `[40, 41]`; usages
    /// that don't chain fail.
    /// OCCT's component colour: a `STYLED_ITEM` on the part's body whose
    /// assignment is a `PRESENTATION_STYLE_BY_CONTEXT` in a shape
    /// representation of the usage's `PRODUCT_DEFINITION_SHAPE` (OCCT
    /// 7.8's XDE writer, `tests/data/occt/colour_assembly.stp`), taken
    /// whole. One whose context is no usage's holds wherever its item is.
    #[test]
    fn a_style_by_context_in_a_usages_shape_reads_as_that_usage() {
        let s = styles_of(&chain(
            "1.,0.,0.",
            "#40=NEXT_ASSEMBLY_USAGE_OCCURRENCE('4','','',#50,#51,$);\
             #44=PRODUCT_DEFINITION_SHAPE('Placement','',#40);\
             #45=SHAPE_DEFINITION_REPRESENTATION(#44,#46);\
             #46=SHAPE_REPRESENTATION('',(#47),#48);\
             #10=PRESENTATION_STYLE_BY_CONTEXT((#8),#46);\
             #11=PRESENTATION_STYLE_BY_CONTEXT((#8),#60);\
             #20=STYLED_ITEM('color',(#10),#100);\
             #21=STYLED_ITEM('color',(#11),#101);\
             #60=SHAPE_REPRESENTATION('',(#47),#48);",
        ));
        assert_eq!(s.failed, []);
        let a = s.assigned.iter().find(|a| a.instance == 20).unwrap();
        assert_eq!(a.path, [40]);
        assert_eq!(a.style.colour, Some([255, 0, 0]));
        // Taken whole, as OCCT means it: opaque, giving no transparency.
        assert_eq!(a.style.opacity, Some(1.0));
        assert!(!s.plain.contains_key(&100));
        let b = s.assigned.iter().find(|a| a.instance == 21).unwrap();
        assert!(b.path.is_empty());
        assert_eq!(s.plain[&101], 1);
    }

    #[test]
    fn contexts_read_as_usage_paths_outermost_first() {
        let tree = "#40=NEXT_ASSEMBLY_USAGE_OCCURRENCE('1','','',#50,#51,$);\
                    #41=NEXT_ASSEMBLY_USAGE_OCCURRENCE('2','','',#51,#52,$);\
                    #42=NEXT_ASSEMBLY_USAGE_OCCURRENCE('3','','',#50,#52,$);\
                    #43=SPECIFIED_HIGHER_USAGE_OCCURRENCE('','','',#50,#52,$,#40,#41);\
                    #44=PRODUCT_DEFINITION_SHAPE('','',#40);#45=PRODUCT_DEFINITION_SHAPE('','',#41);\
                    #46=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#60,#44);\
                    #47=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#61,#45);";
        let with = |context: &str| {
            styles_of(&chain(
                "1.,0.,0.",
                &format!(
                    "{tree}#20=STYLED_ITEM('',(#9),#100);\
                     #21=CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM('',(#9),#100,#20,({context}));"
                ),
            ))
        };
        for context in ["#40,#41", "#41,#40", "#43", "#46,#47"] {
            let s = with(context);
            let a = s.assigned.iter().find(|a| a.instance == 21).unwrap();
            assert_eq!(a.path, [40, 41], "{context}");
        }
        let s = with("#40,#42");
        assert_eq!(s.failed.iter().map(|f| f.0).collect::<Vec<_>>(), [21]);
    }

    /// OCCT's AP214 box (`tests/data/occt/box_ap214.stp`) with `extra`
    /// added to its data section: what a test styles it with.
    pub(crate) fn styled_box(extra: impl Fn(&crate::Opened) -> String) -> crate::Opened {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/occt/box_ap214.stp"
        ))
        .unwrap();
        let plain = crate::open(&bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let end = text.rfind("ENDSEC;").unwrap();
        let more = extra(&plain);
        crate::open(format!("{}{more}{}", &text[..end], &text[end..]).as_bytes()).unwrap()
    }

    /// A red style `#9001`…`#9009` and a blue one `#9011`…`#9019`.
    fn two_styles() -> String {
        let one = |base: u64, rgb: &str| {
            format!(
                "#{a}=COLOUR_RGB('',{rgb});#{b}=FILL_AREA_STYLE_COLOUR('',#{a});\
                 #{c}=FILL_AREA_STYLE('',(#{b}));#{d}=SURFACE_STYLE_FILL_AREA(#{c});\
                 #{e}=SURFACE_SIDE_STYLE('',(#{d}));#{f}=SURFACE_STYLE_USAGE(.BOTH.,#{e});\
                 #{g}=PRESENTATION_STYLE_ASSIGNMENT((#{f}));",
                a = base + 1,
                b = base + 2,
                c = base + 3,
                d = base + 4,
                e = base + 5,
                f = base + 6,
                g = base + 9,
            )
        };
        one(9000, "1.,0.,0.") + &one(9010, "0.,0.,1.")
    }

    #[test]
    fn a_body_takes_its_items_style_and_a_face_its_own() {
        let o = styled_box(|o| {
            let item = o.held[0].items[0];
            let face = o.faces_of(item)[0];
            format!(
                "{}#9100=STYLED_ITEM('',(#9009),#{item});#9101=STYLED_ITEM('',(#9019),#{face});",
                two_styles()
            )
        });
        let s = o.body_styles(0);
        assert_eq!(s.body.unwrap().colour, Some([255, 0, 0]));
        let face = o.faces_of(o.held[0].items[0])[0];
        assert_eq!(s.faces.len(), 1);
        assert_eq!(s.faces[&face].colour, Some([0, 0, 255]));
        assert!(s.instances.contains(&9100) && s.instances.contains(&9101));
        assert!(s.instances.contains(&9001) && s.instances.contains(&9011));
    }

    #[test]
    fn a_representations_style_is_the_bodys_and_a_shells_its_items() {
        let o = styled_box(|o| {
            let rep = o.held[0].representation;
            format!("{}#9100=STYLED_ITEM('',(#9019),#{rep});", two_styles())
        });
        assert_eq!(o.body_styles(0).body.unwrap().colour, Some([0, 0, 255]));
        let o = styled_box(|o| {
            let shell = o.shells_of(o.held[0].items[0])[0];
            format!("{}#9100=STYLED_ITEM('',(#9009),#{shell});", two_styles())
        });
        assert_eq!(o.body_styles(0).body.unwrap().colour, Some([255, 0, 0]));
        // An unstyled file reads no styles, and costs nothing more.
        let plain = styled_box(|_| String::new());
        assert_eq!(plain.body_styles(0), BodyStyles::default());
        assert!(plain.styles().assigned.is_empty());
    }

    #[test]
    fn an_occurrence_style_names_its_usages_and_its_body() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/occt/assembly.stp");
        let bytes = std::fs::read(path).unwrap();
        let plain = crate::open(&bytes).unwrap();
        let defs = &plain.products.definitions;
        let root = plain.products.roots[0];
        // The second usage of the block, and the block's body.
        let (n, usage) = defs[root]
            .children
            .iter()
            .enumerate()
            .filter(|(_, c)| &*defs[c.definition].name == "block")
            .nth(1)
            .map(|(n, c)| (n, c.instance))
            .unwrap();
        let block = defs[root].children[n].definition;
        let k = plain
            .held
            .iter()
            .position(|h| h.definition == Some(block))
            .unwrap();
        let item = plain.held[k].items[0];
        // The pin's usage: the block's body isn't in what it places.
        let pin = defs[root]
            .children
            .iter()
            .find(|c| &*defs[c.definition].name == "pin")
            .unwrap()
            .instance;
        let text = String::from_utf8(bytes).unwrap();
        let end = text.rfind("ENDSEC;").unwrap();
        let extra = format!(
            "{}#9100=STYLED_ITEM('',(#9009),#{item});\
             #9101=CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM('',(#9019),#{item},#9100,(#{usage}));\
             #9102=CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM('',(#9019),#{item},#9100,(#{pin}));\
             #9103=CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM('',(#9019),#{item},#9100,(#999999));",
            two_styles()
        );
        let o = crate::open(format!("{}{extra}{}", &text[..end], &text[end..]).as_bytes()).unwrap();
        let occ = o.occurrence_styles();
        assert_eq!(occ.len(), 2, "{occ:?}");
        assert_eq!(occ[0].path, [(root, n)]);
        assert_eq!(occ[0].usages, [usage]);
        assert_eq!(occ[0].target, Target::Body(k));
        assert_eq!(occ[0].style.colour, Some([0, 0, 255]));
        assert_eq!(occ[0].instance, 9101);
        // In a usage whose product doesn't hold the item: unplaced, its
        // usages kept.
        assert_eq!(occ[1].target, Target::Unplaced);
        assert_eq!(occ[1].usages, [pin]);
        // A context naming an instance the file doesn't have fails.
        assert!(o.styles().failed.iter().any(|f| f.0 == 9103));
        // The plain style is the block's everywhere else.
        assert_eq!(o.body_styles(k).body.unwrap().colour, Some([255, 0, 0]));
    }
}
