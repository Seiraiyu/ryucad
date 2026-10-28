//! The board model: one model whatever the source, holding what a
//! fabricator sees, in integer nanometres. This task holds its features,
//! symbol tables, user symbols and fonts; the board itself follows.

mod feature;

pub use feature::{
    AttrValue, Attributes, Barcode, Feature, FeatureKind, Features, Font, FontStroke, Library,
    Polarity, Symbol, Text, UserSymbol,
};
