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

use crate::{Nm, Source, Valued, iso_metric, iso_metric_limits, pipe, unified};

/// A thread's pitch: millimetres for ISO metric, threads per inch for
/// unified threads, tenths of a thread per inch for pipe (11.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pitch {
    Mm(Nm),
    Tpi(u16),
    Tpi10(u32),
}

impl Pitch {
    /// The pitch as an exact fraction of nanometres, `(num, den)`.
    pub fn nm(self) -> (i128, i128) {
        match self {
            Pitch::Mm(n) => (i128::from(n), 1),
            Pitch::Tpi(t) => (i128::from(crate::NM_PER_IN), i128::from(t)),
            Pitch::Tpi10(t) => (i128::from(crate::NM_PER_IN) * 10, i128::from(t)),
        }
    }

    /// As specs write it: `1.0`, `1.25`; `20`; `11.5`.
    pub fn text(self) -> String {
        match self {
            Pitch::Mm(n) => {
                let t = crate::mm_decimal(n);
                if t.contains('.') { t } else { format!("{t}.0") }
            }
            Pitch::Tpi(t) => t.to_string(),
            Pitch::Tpi10(t) if t % 10 == 0 => (t / 10).to_string(),
            Pitch::Tpi10(t) => format!("{}.{}", t / 10, t % 10),
        }
    }
}

/// A built-in thread, resolved: what it is and every value with its
/// source. Internal threads only in 28.A.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    pub standard: Standard,
    /// The size as the standard names it: `M6`, `1/4`, `#10`, `G1/4`.
    pub size: &'static str,
    /// The basic major diameter; none for pipe threads.
    pub major: Option<Valued<Nm>>,
    pub pitch: Valued<Pitch>,
    /// Whether `pitch` is the size's own (coarse for ISO, the series' TPI).
    pub coarse: bool,
    /// The class's text; none for pipe threads.
    pub class: Option<String>,
    pub default_class: bool,
    /// The minor diameter's least and greatest; none for pipe threads.
    pub minor: Option<(Valued<Nm>, Valued<Nm>)>,
    pub major_min: Option<Valued<Nm>>,
    /// The taper on the diameter, `(1, 16)`, for tapered pipe threads.
    pub taper: Option<(u32, u32)>,
}

/// Why a lookup failed, with what would be valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unknown {
    /// The nearest sizes of the standard, by size order.
    Size {
        known: Vec<&'static str>,
    },
    /// The pitches (or TPI) the size lists, as specs write them.
    Pitch {
        listed: Vec<String>,
    },
    Class {
        known: Vec<String>,
    },
    /// A pipe thread takes no class.
    ClassNotTaken,
}

/// Up to two sizes either side of where `name` would sort among `names`
/// (by the number in it), else the first four.
fn nearest(
    names: &[&'static str],
    value: impl Fn(&str) -> Option<i128>,
    name: &str,
) -> Vec<&'static str> {
    let at = value(name)
        .map(|v| {
            names
                .iter()
                .take_while(|n| value(n).is_some_and(|w| w < v))
                .count()
        })
        .unwrap_or(0);
    let lo = at.saturating_sub(2);
    names[lo..(lo + 4).min(names.len())].to_vec()
}

/// A metric size's major diameter in µm, from its name (`M6.5` → 6500).
fn metric_value(n: &str) -> Option<i128> {
    let d = n.strip_prefix('M')?;
    let (w, f) = d.split_once('.').unwrap_or((d, ""));
    let f = format!("{f:0<3}");
    Some(w.parse::<i128>().ok()? * 1000 + f.get(..3)?.parse::<i128>().ok()?)
}

pub fn lookup(
    standard: Standard,
    size: &str,
    pitch: Option<Pitch>,
    class: Option<&str>,
) -> Result<Thread, Unknown> {
    let fact = Source::Fact;
    match standard {
        Standard::IsoMetric => {
            let names: Vec<&'static str> = iso_metric::SIZES.iter().map(|s| s.name).collect();
            let s = iso_metric::size(size).ok_or_else(|| Unknown::Size {
                known: nearest(&names, metric_value, size),
            })?;
            let p = match pitch {
                None => s.pitches[0],
                Some(Pitch::Mm(p)) if s.pitches.contains(&p) => p,
                Some(_) => {
                    return Err(Unknown::Pitch {
                        listed: s.pitches.iter().map(|&p| Pitch::Mm(p).text()).collect(),
                    });
                }
            };
            let c = match class {
                None => iso_metric_limits::Class::DEFAULT,
                Some(t) => iso_metric_limits::Class::parse(t).ok_or_else(|| Unknown::Class {
                    known: iso_metric_limits::Class::all()
                        .iter()
                        .map(|c| c.text())
                        .collect(),
                })?,
            };
            let l = iso_metric_limits::internal_limits(s, p, c);
            Ok(Thread {
                standard,
                size: s.name,
                major: Some(Valued::new(s.major, fact)),
                pitch: Valued::new(Pitch::Mm(p), fact),
                coarse: p == s.pitches[0],
                class: Some(c.text()),
                default_class: c == iso_metric_limits::Class::DEFAULT,
                minor: Some(l.minor),
                major_min: Some(l.major_min),
                taper: None,
            })
        }
        Standard::Unc | Standard::Unf | Standard::Unef | Standard::Un => {
            let in_series = |s: &unified::Size| match standard {
                Standard::Unc => s.unc,
                Standard::Unf => s.unf,
                Standard::Unef => s.unef,
                _ => Some(0),
            };
            let names: Vec<&'static str> = unified::SIZES
                .iter()
                .filter(|s| in_series(s).is_some())
                .map(|s| s.name)
                .collect();
            let order = |n: &str| unified::size(n).map(|s| i128::from(s.major));
            let s = unified::size(size)
                .filter(|s| in_series(s).is_some())
                .ok_or_else(|| Unknown::Size {
                    known: nearest(&names, order, size),
                })?;
            let own = in_series(s).filter(|&t| t != 0);
            let t = match (pitch, own) {
                (None, Some(t)) => t,
                (Some(Pitch::Tpi(t)), Some(o)) if t == o => t,
                (Some(Pitch::Tpi(t)), None) if unified::UN_TPI.contains(&t) => t,
                _ => {
                    let listed = match own {
                        Some(o) => vec![o.to_string()],
                        None => unified::UN_TPI.iter().map(u16::to_string).collect(),
                    };
                    return Err(Unknown::Pitch { listed });
                }
            };
            let c = match class {
                None => unified::Class::DEFAULT,
                Some(x) => unified::Class::parse(x).ok_or_else(|| Unknown::Class {
                    known: unified::Class::ALL
                        .iter()
                        .map(|c| c.text().to_owned())
                        .collect(),
                })?,
            };
            let l = unified::internal_limits(s, t, c);
            Ok(Thread {
                standard,
                size: s.name,
                major: Some(Valued::new(s.major, fact)),
                pitch: Valued::new(Pitch::Tpi(t), Source::Public("FED-STD-H28/2B")),
                coarse: own.is_some(),
                class: Some(c.text().to_owned()),
                default_class: c == unified::Class::DEFAULT,
                minor: Some(l.minor),
                major_min: Some(l.major_min),
                taper: None,
            })
        }
        Standard::Npt | Standard::Nptf | Standard::Nps | Standard::Bspt | Standard::Bspp => {
            if class.is_some() {
                return Err(Unknown::ClassNotTaken);
            }
            let (table, nominal, taper) = match standard {
                Standard::Bspp => (pipe::BRITISH, size.strip_prefix('G'), None),
                Standard::Bspt => match size.strip_prefix("Rc").or(size.strip_prefix("Rp")) {
                    Some(n) if pipe::british_tapered(n) => {
                        let taper = size.starts_with("Rc").then_some((1, 16));
                        (pipe::BRITISH, Some(n), taper)
                    }
                    _ => (pipe::BRITISH, None, None),
                },
                Standard::Nps => (pipe::NATIONAL, Some(size), None),
                _ => (pipe::NATIONAL, Some(size), Some((1, 16))),
            };
            let found = nominal.and_then(|n| table.iter().find(|s| s.name == n));
            let Some(p) = found else {
                // The first four, as the standard writes them (`G1/16`).
                let prefix = match standard {
                    Standard::Bspp => "G",
                    Standard::Bspt => "Rc",
                    _ => "",
                };
                let known = table
                    .iter()
                    .take(4)
                    .map(|s| table_name(&format!("{prefix}{}", s.name)))
                    .collect();
                return Err(Unknown::Size { known });
            };
            if let Some(Pitch::Tpi10(t)) = pitch
                && t != p.tpi10
            {
                return Err(Unknown::Pitch {
                    listed: vec![Pitch::Tpi10(p.tpi10).text()],
                });
            }
            Ok(Thread {
                standard,
                size: table_name(size),
                major: None,
                pitch: Valued::new(Pitch::Tpi10(p.tpi10), fact),
                coarse: true,
                class: None,
                default_class: true,
                minor: None,
                major_min: None,
                taper,
            })
        }
    }
}

/// The static name a pipe size is known by, with its prefix (`G1/4`).
fn table_name(size: &str) -> &'static str {
    // The prefixed names are few: keep them in one static list.
    PIPE_NAMES
        .iter()
        .copied()
        .find(|n| *n == size)
        .unwrap_or("")
}

/// Every pipe size name `lookup` accepts: the national table's, and the
/// British table's with `G`, and `Rc` and `Rp` where tapered.
const PIPE_NAMES: [&str; 63] = [
    "1/16", "1/8", "1/4", "3/8", "1/2", "3/4", "1", "1-1/4", "1-1/2", "2", "2-1/2", "3", "3-1/2",
    "4", "5", "6", "G1/16", "G1/8", "G1/4", "G3/8", "G1/2", "G5/8", "G3/4", "G7/8", "G1", "G1-1/4",
    "G1-1/2", "G2", "G2-1/2", "G3", "G4", "G5", "G6", "Rc1/16", "Rc1/8", "Rc1/4", "Rc3/8", "Rc1/2",
    "Rc3/4", "Rc1", "Rc1-1/4", "Rc1-1/2", "Rc2", "Rc2-1/2", "Rc3", "Rc4", "Rc5", "Rc6", "Rp1/16",
    "Rp1/8", "Rp1/4", "Rp3/8", "Rp1/2", "Rp3/4", "Rp1", "Rp1-1/4", "Rp1-1/2", "Rp2", "Rp2-1/2",
    "Rp3", "Rp4", "Rp5", "Rp6",
];

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
    #[test]
    fn the_lookup() {
        let m6 = lookup(Standard::IsoMetric, "M6", None, None).unwrap();
        assert_eq!(m6.pitch.value, Pitch::Mm(1_000_000));
        assert!(m6.coarse && m6.default_class);
        assert_eq!(m6.class.as_deref(), Some("6H"));
        assert_eq!(m6.minor.unwrap().1.value, 5_153_000);
        let fine = lookup(
            Standard::IsoMetric,
            "M8",
            Some(Pitch::Mm(1_000_000)),
            Some("5H"),
        )
        .unwrap();
        assert!(!fine.coarse && !fine.default_class);
        let q = lookup(Standard::Unc, "1/4", None, None).unwrap();
        assert_eq!(
            (q.pitch.value, q.class.as_deref()),
            (Pitch::Tpi(20), Some("2B"))
        );
        let npt = lookup(Standard::Npt, "1/8", None, None).unwrap();
        assert_eq!(
            (npt.pitch.value, npt.taper, npt.minor),
            (Pitch::Tpi10(270), Some((1, 16)), None)
        );
        assert_eq!(
            lookup(Standard::Bspp, "G1/4", None, None).unwrap().taper,
            None
        );
        assert_eq!(
            lookup(Standard::Bspt, "Rc1/4", None, None).unwrap().taper,
            Some((1, 16))
        );
        assert_eq!(
            lookup(Standard::Bspt, "Rp1/4", None, None).unwrap().taper,
            None
        );
        // What's wrong, and what's valid.
        let Err(Unknown::Size { known }) = lookup(Standard::IsoMetric, "M6.5", None, None) else {
            panic!()
        };
        assert_eq!(known, ["M5", "M6", "M7", "M8"]);
        let Err(Unknown::Pitch { listed }) =
            lookup(Standard::IsoMetric, "M6", Some(Pitch::Mm(1_250_000)), None)
        else {
            panic!()
        };
        assert_eq!(listed, ["1.0", "0.75"]);
        let Err(Unknown::Class { known }) = lookup(Standard::IsoMetric, "M6", None, Some("6g"))
        else {
            panic!()
        };
        assert!(known.contains(&"6H".to_owned()));
        assert_eq!(
            lookup(Standard::Npt, "1/8", None, Some("2B")).unwrap_err(),
            Unknown::ClassNotTaken
        );
        assert!(matches!(
            lookup(Standard::Un, "1/2", Some(Pitch::Tpi(12)), None),
            Ok(t) if t.pitch.value == Pitch::Tpi(12)
        ));
    }

    #[test]
    fn every_pipe_name_is_known() {
        for standard in [
            Standard::Npt,
            Standard::Nptf,
            Standard::Nps,
            Standard::Bspt,
            Standard::Bspp,
        ] {
            for name in PIPE_NAMES {
                if let Ok(t) = lookup(standard, name, None, None) {
                    assert_eq!(t.size, name, "{standard:?} {name}");
                }
            }
        }
        for s in pipe::NATIONAL {
            assert_eq!(
                lookup(Standard::Npt, s.name, None, None).unwrap().size,
                s.name
            );
        }
        for s in pipe::BRITISH {
            let g = format!("G{}", s.name);
            assert_eq!(lookup(Standard::Bspp, &g, None, None).unwrap().size, g);
            for p in ["Rc", "Rp"] {
                let r = format!("{p}{}", s.name);
                match lookup(Standard::Bspt, &r, None, None) {
                    Ok(t) => assert_eq!(t.size, r),
                    Err(_) => assert!(!pipe::british_tapered(s.name), "{r}"),
                }
            }
        }
        let Err(Unknown::Size { known }) = lookup(Standard::Bspp, "1/4", None, None) else {
            panic!()
        };
        assert_eq!(known, ["G1/16", "G1/8", "G1/4", "G3/8"]);
        let Err(Unknown::Size { known }) = lookup(Standard::Bspt, "R1/4", None, None) else {
            panic!()
        };
        assert_eq!(known, ["Rc1/16", "Rc1/8", "Rc1/4", "Rc3/8"]);
    }
}
