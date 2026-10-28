//! Features as written, the symbol tables they name, and user symbols.

use std::collections::{BTreeMap, BTreeSet};

use crate::geom::{Arc, Nm, Pen, Place, Point, Region};
use crate::symbol::StandardSymbol;

/// Whether a feature adds material or clears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Polarity {
    Positive,
    Negative,
}

impl Polarity {
    /// The polarity of something negative-in-negative: two negatives add.
    pub fn times(self, o: Polarity) -> Polarity {
        if self == o {
            Polarity::Positive
        } else {
            Polarity::Negative
        }
    }
}

/// An attribute's value, as the attribute list declares it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AttrValue {
    /// A boolean attribute that is set.
    Flag,
    /// A number, as its decimal text.
    Number(String),
    /// Text, or an option's name.
    Text(String),
}

pub type Attributes = BTreeMap<String, AttrValue>;

/// One entry of a symbol table: what a feature's symbol index names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Symbol {
    Standard(StandardSymbol),
    /// A user symbol, by name in the board's library.
    User(String),
    /// A name that is neither: features using it have no shape.
    Unknown(String),
}

/// A text feature: its string drawn in a stroke font.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Text {
    pub font: String,
    /// The text's origin (the bottom left of its first character), its
    /// rotation and mirror.
    pub place: Place,
    /// Each character's width and height.
    pub width: Nm,
    pub height: Nm,
    /// The width of the strokes.
    pub line_width: Nm,
    /// As written; dynamic text (`$$DATE`) is kept, never substituted.
    pub string: String,
}

/// A barcode, kept as written and left out of shapes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Barcode {
    pub place: Place,
    pub kind: String,
    pub string: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FeatureKind {
    /// A line from `a` to `b` drawn with symbol `symbol` (an index into
    /// the table) as its pen.
    Line {
        a: Point,
        b: Point,
        symbol: u32,
    },
    Arc {
        arc: Arc,
        symbol: u32,
    },
    Pad {
        symbol: u32,
        place: Place,
    },
    Surface {
        regions: Vec<Region>,
    },
    Text(Text),
    Barcode(Barcode),
}

/// A feature as written.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Feature {
    pub kind: FeatureKind,
    pub polarity: Polarity,
    pub dcode: Option<i32>,
    pub attributes: Attributes,
    /// The writer's feature ID (ODB++ 8's `ID=`), when it gave one.
    pub id: Option<u64>,
    /// The net, an index into the step's nets, once nets are read.
    pub net: Option<u32>,
}

impl Feature {
    pub fn new(kind: FeatureKind, polarity: Polarity) -> Feature {
        Feature {
            kind,
            polarity,
            dcode: None,
            attributes: Attributes::new(),
            id: None,
            net: None,
        }
    }
}

/// A symbol table and the features that use it: a layer's features file,
/// or a user symbol's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Features {
    symbols: Vec<Symbol>,
    features: Vec<Feature>,
}

impl Features {
    pub fn new(symbols: Vec<Symbol>, features: Vec<Feature>) -> Features {
        Features { symbols, features }
    }

    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    pub fn features(&self) -> &[Feature] {
        &self.features
    }

    /// The user symbols this table names.
    pub fn uses(&self) -> impl Iterator<Item = &str> {
        self.symbols.iter().filter_map(|s| match s {
            Symbol::User(n) => Some(n.as_str()),
            _ => None,
        })
    }
}

/// A named list of features in its own coordinates, used as a pad.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserSymbol {
    pub name: String,
    pub features: Features,
}

/// One stroke of a font character, in the font's own coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontStroke {
    pub a: Point,
    pub b: Point,
    pub pen: Pen,
}

/// A stroke font: each character's strokes in a box `width` by `height`
/// (its origin at the box's bottom left), and `offset` between characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Font {
    pub name: String,
    pub width: Nm,
    pub height: Nm,
    pub offset: Nm,
    pub chars: BTreeMap<char, Vec<FontStroke>>,
}

/// The user symbols and fonts a board's features draw with, and the
/// symbols that use themselves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Library {
    symbols: BTreeMap<String, UserSymbol>,
    fonts: BTreeMap<String, Font>,
    /// Each symbol on a cycle, with every symbol of its cycle, sorted.
    cyclic: BTreeMap<String, Vec<String>>,
}

impl Library {
    pub fn new(symbols: Vec<UserSymbol>, fonts: Vec<Font>) -> Library {
        let symbols: BTreeMap<String, UserSymbol> =
            symbols.into_iter().map(|s| (s.name.clone(), s)).collect();
        let fonts = fonts.into_iter().map(|f| (f.name.clone(), f)).collect();
        let cyclic = cycles(&symbols);
        Library {
            symbols,
            fonts,
            cyclic,
        }
    }

    pub fn symbol(&self, name: &str) -> Option<&UserSymbol> {
        self.symbols.get(name)
    }

    pub fn symbols(&self) -> impl Iterator<Item = &UserSymbol> {
        self.symbols.values()
    }

    pub fn font(&self, name: &str) -> Option<&Font> {
        self.fonts.get(name)
    }

    /// The cycle `name` is on, if it is on one.
    pub fn cycle(&self, name: &str) -> Option<&[String]> {
        self.cyclic.get(name).map(Vec::as_slice)
    }

    /// Every cycle among the user symbols, each once, sorted.
    pub fn cycles(&self) -> Vec<&[String]> {
        let mut seen = BTreeSet::new();
        self.cyclic
            .values()
            .filter(|c| seen.insert(c.as_slice()))
            .map(Vec::as_slice)
            .collect()
    }
}

/// Tarjan's strongly connected components over symbols by the user
/// symbols they name: a component of two or more, or one naming itself,
/// is a cycle.
fn cycles(symbols: &BTreeMap<String, UserSymbol>) -> BTreeMap<String, Vec<String>> {
    let names: Vec<&str> = symbols.keys().map(String::as_str).collect();
    let index_of: BTreeMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let edges: Vec<Vec<usize>> = names
        .iter()
        .map(|n| {
            let mut e: Vec<usize> = symbols[*n]
                .features
                .uses()
                .filter_map(|u| index_of.get(u).copied())
                .collect();
            e.sort_unstable();
            e.dedup();
            e
        })
        .collect();
    let n = names.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut next = 0;
    let mut out = BTreeMap::new();
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        // An explicit call stack of (node, next edge to look at).
        let mut calls = vec![(root, 0usize)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (v, ref mut ei)) = calls.last_mut() {
            if let Some(&w) = edges[v].get(*ei) {
                *ei += 1;
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    calls.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            calls.pop();
            if let Some(&(parent, _)) = calls.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut comp = Vec::new();
                loop {
                    let w = stack.pop().expect("v is on the stack");
                    on_stack[w] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                if comp.len() > 1 || edges[v].contains(&v) {
                    let mut chain: Vec<String> =
                        comp.iter().map(|&i| names[i].to_string()).collect();
                    chain.sort();
                    for name in &chain {
                        out.insert(name.clone(), chain.clone());
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(name: &str, uses: &[&str]) -> UserSymbol {
        UserSymbol {
            name: name.into(),
            features: Features::new(
                uses.iter().map(|u| Symbol::User((*u).into())).collect(),
                vec![],
            ),
        }
    }

    #[test]
    fn cycles_are_found_and_named() {
        let lib = Library::new(
            vec![
                user("a", &["b"]),
                user("b", &["c"]),
                user("c", &["a"]),
                user("d", &["a"]),
                user("e", &["e"]),
                user("f", &[]),
                user("g", &["missing"]),
            ],
            vec![],
        );
        let abc = ["a".to_string(), "b".into(), "c".into()];
        assert_eq!(lib.cycle("b"), Some(&abc[..]));
        assert_eq!(lib.cycle("e"), Some(&["e".to_string()][..]));
        assert_eq!(lib.cycle("d"), None);
        assert_eq!(lib.cycle("f"), None);
        assert_eq!(lib.cycle("g"), None);
        assert_eq!(lib.cycles().len(), 2);
    }

    #[test]
    fn polarity_multiplies() {
        use Polarity::*;
        assert_eq!(Negative.times(Negative), Positive);
        assert_eq!(Positive.times(Negative), Negative);
    }

    #[test]
    fn tables_compare_by_content() {
        let a = Features::new(vec![Symbol::Unknown("x".into())], vec![]);
        assert_eq!(a.clone(), a);
    }
}
