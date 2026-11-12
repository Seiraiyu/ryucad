//! Where a built-in value came from. `ryucad standards` and every
//! derived size report it, so an agent can tell a formula's result from a
//! public-domain document's value from a plain fact.

/// The source of one built-in value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// Worked out here by a published formula: the standard and the
    /// formula's name (`"ISO 965-1 T_D1"`).
    Formula(&'static str),
    /// Printed in a public-domain document: its name and section
    /// (`"FED-STD-H28/2B"`).
    Public(&'static str),
    /// Common knowledge: a size's name and pitch, a drill of a series.
    Fact,
}

impl Source {
    /// As `ryucad standards` and derived sizes give it:
    /// `formula: ISO 965-1 T_D1`, `FED-STD-H28/2B`, `fact`.
    pub fn text(self) -> String {
        match self {
            Source::Formula(f) => format!("formula: {f}"),
            Source::Public(d) => d.to_owned(),
            Source::Fact => "fact".to_owned(),
        }
    }

    /// The source's kind, as a derived size's `source`: `formula`,
    /// `public` or `fact`.
    pub fn kind(self) -> &'static str {
        match self {
            Source::Formula(_) => "formula",
            Source::Public(_) => "public",
            Source::Fact => "fact",
        }
    }
}

/// A value with its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Valued<T> {
    pub value: T,
    pub source: Source,
}

impl<T> Valued<T> {
    pub const fn new(value: T, source: Source) -> Valued<T> {
        Valued { value, source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_read_as_the_design_names_them() {
        assert_eq!(
            Source::Formula("ISO 965-1 T_D1").text(),
            "formula: ISO 965-1 T_D1"
        );
        assert_eq!(Source::Public("FED-STD-H28/2B").text(), "FED-STD-H28/2B");
        assert_eq!(Source::Fact.text(), "fact");
        assert_eq!(Source::Public("x").kind(), "public");
    }
}
