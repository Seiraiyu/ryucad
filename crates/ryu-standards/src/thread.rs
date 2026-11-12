//! Thread standards by name, and (Task 9) the lookup every caller uses:
//! a standard, a size, a pitch or TPI and a class to its diameters and
//! limits, each with its source.
//!
//! Sources: the names are ryucad's own keys for each standard's name.
//! The values come from `iso_metric`, `unified` and `pipe`, whose module
//! docs name theirs.

/// A built-in thread standard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Standard {
    IsoMetric,
    Unc,
    Unf,
    Unef,
    /// Unified constant-pitch series (8-UN, 12-UN, ...).
    Un,
    Npt,
    Nptf,
    /// NPSM, straight pipe threads for mechanical joints.
    Nps,
    /// R, Rc and Rp (ISO 7-1 designations).
    Bspt,
    /// G (ISO 228-1 designations).
    Bspp,
}

impl Standard {
    pub const ALL: [Standard; 10] = [
        Standard::IsoMetric,
        Standard::Unc,
        Standard::Unf,
        Standard::Unef,
        Standard::Un,
        Standard::Npt,
        Standard::Nptf,
        Standard::Nps,
        Standard::Bspt,
        Standard::Bspp,
    ];

    /// Its key in hole files and project tables.
    pub fn key(self) -> &'static str {
        match self {
            Standard::IsoMetric => "iso_metric",
            Standard::Unc => "unc",
            Standard::Unf => "unf",
            Standard::Unef => "unef",
            Standard::Un => "un",
            Standard::Npt => "npt",
            Standard::Nptf => "nptf",
            Standard::Nps => "nps",
            Standard::Bspt => "bspt",
            Standard::Bspp => "bspp",
        }
    }

    pub fn parse(key: &str) -> Option<Standard> {
        Standard::ALL.into_iter().find(|s| s.key() == key)
    }

    pub fn is_pipe(self) -> bool {
        matches!(
            self,
            Standard::Npt | Standard::Nptf | Standard::Nps | Standard::Bspt | Standard::Bspp
        )
    }

    /// Whether its sizes and pitches are in inches (unified and pipe).
    pub fn inch(self) -> bool {
        self != Standard::IsoMetric
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standards_read_and_write_their_keys() {
        for s in Standard::ALL {
            assert_eq!(Standard::parse(s.key()), Some(s));
        }
        assert_eq!(Standard::parse("iso_metric"), Some(Standard::IsoMetric));
        assert_eq!(Standard::parse("ISO_METRIC"), None);
        assert!(Standard::Npt.is_pipe() && !Standard::Unc.is_pipe());
        assert!(Standard::Unc.inch() && !Standard::IsoMetric.inch());
    }
}
