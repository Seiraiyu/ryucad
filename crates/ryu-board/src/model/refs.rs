//! References inside a board: `<step>/<layer>/<index>` for a feature,
//! `<step>/comp/<refdes>` for a component, `<step>/net/<name>` for a net.
//! The second segments `comp` and `net` are read as those kinds, so a
//! layer named `comp` or `net` has no feature references; a net's name
//! may hold `/`. In a project they are prefixed by the board's ID. They
//! aren't identities across revisions.

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ref {
    Feature {
        step: String,
        layer: String,
        index: usize,
    },
    Component {
        step: String,
        refdes: String,
    },
    Net {
        step: String,
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefError(pub String);

impl fmt::Display for RefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a board reference: {}", self.0)
    }
}

impl std::error::Error for RefError {}

impl fmt::Display for Ref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ref::Feature { step, layer, index } => write!(f, "{step}/{layer}/{index}"),
            Ref::Component { step, refdes } => write!(f, "{step}/comp/{refdes}"),
            Ref::Net { step, name } => write!(f, "{step}/net/{name}"),
        }
    }
}

impl FromStr for Ref {
    type Err = RefError;

    fn from_str(s: &str) -> Result<Ref, RefError> {
        let err = || RefError(s.to_string());
        let (step, rest) = s.split_once('/').ok_or_else(err)?;
        let (second, rest) = rest.split_once('/').ok_or_else(err)?;
        if step.is_empty() || second.is_empty() || rest.is_empty() {
            return Err(err());
        }
        Ok(match second {
            "comp" if !rest.contains('/') => Ref::Component {
                step: step.into(),
                refdes: rest.into(),
            },
            "net" => Ref::Net {
                step: step.into(),
                name: rest.into(),
            },
            _ => {
                if rest.is_empty()
                    || !rest.bytes().all(|b| b.is_ascii_digit())
                    || (rest.len() > 1 && rest.starts_with('0'))
                {
                    return Err(err());
                }
                Ref::Feature {
                    step: step.into(),
                    layer: second.into(),
                    index: rest.parse().map_err(|_| err())?,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for s in [
            "pcb/top/12",
            "pcb/comp/U1",
            "pcb/net/GND",
            "pcb/net/A/B",
            "panel/drill/0",
        ] {
            assert_eq!(s.parse::<Ref>().unwrap().to_string(), s);
        }
        assert_eq!(
            "pcb/top/12".parse::<Ref>().unwrap(),
            Ref::Feature {
                step: "pcb".into(),
                layer: "top".into(),
                index: 12
            }
        );
        assert_eq!(
            "pcb/net/A/B".parse::<Ref>().unwrap(),
            Ref::Net {
                step: "pcb".into(),
                name: "A/B".into()
            }
        );
    }

    #[test]
    fn faults() {
        for s in [
            "",
            "pcb",
            "pcb/top",
            "pcb/top/x",
            "pcb/top/01",
            "/top/1",
            "pcb//1",
            "pcb/comp/U1/2",
            "pcb/top/-1",
        ] {
            assert!(s.parse::<Ref>().is_err(), "{s}");
        }
    }
}
