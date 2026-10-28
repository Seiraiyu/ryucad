//! The board model: one model whatever the source, holding what a
//! fabricator sees, in integer nanometres.

mod feature;
mod refs;

pub use feature::{
    AttrValue, Attributes, Barcode, Feature, FeatureKind, Features, Font, FontStroke, Library,
    Polarity, Symbol, Text, UserSymbol,
};
pub use refs::{Ref, RefError};

use std::collections::BTreeMap;

use rayon::prelude::*;

use crate::geom::{Mdeg, Nm, Point, Region};
use crate::shape::{Shape, ShapeError, feature_shape};

/// Where a board came from, as its files say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    /// `odb++`, and later `ipc-2581`, `gerber`, `ryueda`.
    pub format: String,
    pub version: String,
    pub application: Option<String>,
    /// The units the files were written in, as they name them.
    pub units: Option<String>,
    pub created: Option<String>,
    pub saved: Option<String>,
}

/// The data classes a source filled, which checks declare they need.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Capabilities {
    pub geometry: bool,
    pub nets: bool,
    pub components: bool,
    pub stackup: bool,
    pub drill: bool,
    pub flex: bool,
    pub feature_ids: bool,
}

/// Where in a source something was found.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Location {
    /// The file inside the job, by its path there.
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// A fault or a warning the producer of a board recorded: a code of the
/// producer's own (ryudfm's error codes), what it is about and why.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Notice {
    pub code: String,
    /// A reference (`<step>/<layer>/<index>` and the like) or a layer,
    /// symbol or file name, when there is one.
    pub about: Option<String>,
    pub message: String,
    pub location: Option<Location>,
}

/// A layer's role.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LayerKind {
    Signal,
    PowerGround,
    Mixed,
    SolderMask,
    SolderPaste,
    SilkScreen,
    Drill,
    Rout,
    Component,
    Dielectric,
    Document,
    ConductivePaste,
    Mask,
    /// A kind the source names that isn't one of these, as written.
    Other(String),
}

impl LayerKind {
    pub fn is_copper(&self) -> bool {
        matches!(
            self,
            LayerKind::Signal | LayerKind::PowerGround | LayerKind::Mixed
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Top,
    Bottom,
    Inner,
}

/// Whether a layer is part of the board or only travels with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    Board,
    Misc,
}

/// The layers a drill or rout layer runs between, by index in
/// [`Board::layers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A layer from the matrix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub name: String,
    pub row: u32,
    pub context: Context,
    pub kind: LayerKind,
    pub polarity: Polarity,
    pub side: Option<Side>,
    pub span: Option<Span>,
    pub attributes: Attributes,
}

/// A step repeated into another (a panel's boards).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repeat {
    pub step: String,
    pub at: Point,
    pub dx: Nm,
    pub dy: Nm,
    pub nx: u32,
    pub ny: u32,
    pub rot: Mdeg,
    pub mirror: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Plated,
    NonPlated,
    Via,
}

/// A drill tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    pub number: u32,
    pub kind: ToolKind,
    pub finished: Nm,
    pub drilled: Option<Nm>,
}

/// What a drill layer's span makes of its holes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoleClass {
    Through,
    Blind,
    Buried,
}

/// A drill layer's tools; its holes are the layer's features.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drill {
    /// The drill layer, by index in [`Board::layers`].
    pub layer: usize,
    pub tools: Vec<Tool>,
    /// Back-drilled, where the attributes say so.
    pub back_drill: bool,
}

/// A component pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub name: String,
    pub at: Point,
    pub rot: Mdeg,
    /// An index into the step's nets.
    pub net: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub refdes: String,
    /// An index into the step's packages.
    pub package: u32,
    pub part: String,
    pub at: Point,
    pub rot: Mdeg,
    pub mirror: bool,
    pub side: Side,
    pub properties: BTreeMap<String, String>,
    pub pins: Vec<Pin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackagePin {
    pub name: String,
    pub at: Point,
    pub shape: Vec<Region>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub outline: Vec<Region>,
    pub pins: Vec<PackagePin>,
}

/// A feature by layer and index, as a net's member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeatureAt {
    pub layer: usize,
    pub index: usize,
}

/// A component pin by component and pin index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PinAt {
    pub component: usize,
    pub pin: usize,
}

/// A point of the CAD netlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CadPoint {
    pub at: Point,
    pub side: Option<Side>,
    pub via: bool,
}

/// A net and its members: features, pins and the CAD netlist's points,
/// kept apart so stage 1 can compare them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Net {
    pub name: String,
    pub features: Vec<FeatureAt>,
    pub pins: Vec<PinAt>,
    pub cad: Vec<CadPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StackKind {
    Copper,
    Dielectric,
}

/// One layer of the stackup, in stack order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackLayer {
    pub name: String,
    pub kind: StackKind,
    pub material: Option<String>,
    pub thickness: Option<Nm>,
    /// Dielectric constant, loss tangent, copper weight and the like, as
    /// the source gives them.
    pub properties: Attributes,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stackup {
    pub layers: Vec<StackLayer>,
}

/// A step: a board, a panel or a coupon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub datum: Point,
    /// The outline, its cutouts as holes.
    pub profile: Option<Region>,
    pub repeats: Vec<Repeat>,
    /// Each layer's features, in [`Board::layers`] order.
    pub layers: Vec<Features>,
    pub components: Vec<Component>,
    pub packages: Vec<Package>,
    pub nets: Vec<Net>,
    pub attributes: Attributes,
}

/// A board as a fabricator sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    pub name: String,
    pub source: Source,
    pub layers: Vec<Layer>,
    pub steps: Vec<Step>,
    /// The step that is the board, by index.
    pub board_step: usize,
    pub library: Library,
    pub drills: Vec<Drill>,
    pub stackup: Option<Stackup>,
    pub attributes: Attributes,
    pub capabilities: Capabilities,
    /// What failed without failing the board.
    pub faults: Vec<Notice>,
    pub warnings: Vec<Notice>,
}

impl Board {
    pub fn step(&self, name: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.name == name)
    }

    pub fn layer(&self, name: &str) -> Option<usize> {
        self.layers.iter().position(|l| l.name == name)
    }

    /// The class of a drill layer's holes, from its span among the copper
    /// layers in matrix order.
    pub fn hole_class(&self, drill: &Drill) -> Option<HoleClass> {
        let span = self.layers.get(drill.layer)?.span?;
        let copper: Vec<usize> = (0..self.layers.len())
            .filter(|&i| self.layers[i].kind.is_copper())
            .collect();
        let (first, last) = (*copper.first()?, *copper.last()?);
        let (a, b) = (span.start.min(span.end), span.start.max(span.end));
        Some(if a == first && b == last {
            HoleClass::Through
        } else if a == first || b == last {
            HoleClass::Blind
        } else {
            HoleClass::Buried
        })
    }

    /// Feature `index`'s shape on `layer` of `step`, built on first use.
    pub fn feature_shape(
        &self,
        step: usize,
        layer: usize,
        index: usize,
    ) -> &Result<Shape, ShapeError> {
        feature_shape(&self.steps[step].layers[layer], index, &self.library)
    }

    /// Every feature's shape on `layer` of `step`, in drawing order, built
    /// in parallel where they aren't yet.
    pub fn layer_shapes(&self, step: usize, layer: usize) -> Vec<&Result<Shape, ShapeError>> {
        let features = &self.steps[step].layers[layer];
        (0..features.features().len())
            .into_par_iter()
            .map(|i| feature_shape(features, i, &self.library))
            .collect()
    }

    /// Builds the shapes of several layers at once, side by side.
    pub fn build_shapes(&self, step: usize, layers: &[usize]) {
        layers.par_iter().for_each(|&l| {
            self.layer_shapes(step, l);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Place;
    use crate::symbol::StandardSymbol;

    fn layer(name: &str, kind: LayerKind, span: Option<Span>) -> Layer {
        Layer {
            name: name.into(),
            row: 0,
            context: Context::Board,
            kind,
            polarity: Polarity::Positive,
            side: None,
            span,
            attributes: Attributes::new(),
        }
    }

    fn pads(n: usize) -> Features {
        Features::new(
            vec![Symbol::Standard(StandardSymbol::Round { d: 100 })],
            (0..n)
                .map(|i| {
                    Feature::new(
                        FeatureKind::Pad {
                            symbol: 0,
                            place: Place::at(Point::new(i as i64 * 1000, 0)),
                        },
                        Polarity::Positive,
                    )
                })
                .collect(),
        )
    }

    fn board() -> Board {
        let layers = vec![
            layer("top", LayerKind::Signal, None),
            layer("in1", LayerKind::Signal, None),
            layer("in2", LayerKind::PowerGround, None),
            layer("bot", LayerKind::Signal, None),
            layer("drill", LayerKind::Drill, Some(Span { start: 0, end: 3 })),
            layer("via12", LayerKind::Drill, Some(Span { start: 0, end: 1 })),
            layer("via23", LayerKind::Drill, Some(Span { start: 1, end: 2 })),
        ];
        Board {
            name: "b".into(),
            source: Source::default(),
            steps: vec![Step {
                name: "pcb".into(),
                datum: Point::ORIGIN,
                profile: None,
                repeats: vec![],
                layers: (0..layers.len()).map(|_| pads(3)).collect(),
                components: vec![],
                packages: vec![],
                nets: vec![],
                attributes: Attributes::new(),
            }],
            layers,
            board_step: 0,
            library: Library::default(),
            drills: vec![],
            stackup: None,
            attributes: Attributes::new(),
            capabilities: Capabilities::default(),
            faults: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn holes_are_classed_by_their_span() {
        let b = board();
        let d = |layer| Drill {
            layer,
            tools: vec![],
            back_drill: false,
        };
        assert_eq!(b.hole_class(&d(4)), Some(HoleClass::Through));
        assert_eq!(b.hole_class(&d(5)), Some(HoleClass::Blind));
        assert_eq!(b.hole_class(&d(6)), Some(HoleClass::Buried));
        assert_eq!(b.hole_class(&d(0)), None);
        assert_eq!((b.step("pcb"), b.layer("bot")), (Some(0), Some(3)));
    }

    #[test]
    fn shapes_are_built_only_where_asked() {
        let b = board();
        assert!(b.feature_shape(0, 1, 2).is_ok());
        let built = |l: usize| {
            b.steps[0].layers[l]
                .shapes
                .iter()
                .filter(|s| s.get().is_some())
                .count()
        };
        assert_eq!((built(0), built(1)), (0, 1));
        b.build_shapes(0, &[0, 2]);
        assert_eq!((built(0), built(1), built(2), built(3)), (3, 1, 3, 0));
        assert_eq!(b.layer_shapes(0, 3).len(), 3);
    }
}
