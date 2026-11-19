//! A board's stackup (design 924): its copper layers and the dielectrics
//! between them, top to bottom. Copper layers are named by position:
//! `top`, `in1` … `inN`, `bottom`. A board without one has its preset's.

use super::preset::Preset;
use crate::error::{Code, Error, Result};
use crate::fault::Fault;
use serde::{Deserialize, Serialize};

/// The most copper layers a stackup holds.
pub const MAX_COPPER: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StackLayer {
    Copper {
        thickness_nm: i64,
    },
    Dielectric {
        thickness_nm: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        material: Option<String>,
        /// Relative permittivity and loss tangent in millionths: data for
        /// stage 8's impedance; nothing here computes with them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        er_ppm: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        loss_ppm: Option<i64>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stackup {
    pub layers: Vec<StackLayer>,
}

impl Stackup {
    /// How many copper layers it has.
    pub fn copper(&self) -> usize {
        self.layers
            .iter()
            .filter(|l| matches!(l, StackLayer::Copper { .. }))
            .count()
    }

    /// The sum of its layers' thicknesses.
    pub fn thickness(&self) -> i64 {
        self.layers
            .iter()
            .map(|l| match l {
                StackLayer::Copper { thickness_nm }
                | StackLayer::Dielectric { thickness_nm, .. } => *thickness_nm,
            })
            .sum()
    }
}

/// What is wrong with a stackup: at most one fault, at `/stackup`.
pub fn judge(s: &Stackup) -> Vec<Fault> {
    let copper = |l: &StackLayer| matches!(l, StackLayer::Copper { .. });
    let why = if s.layers.first().is_none_or(|l| !copper(l))
        || s.layers.last().is_none_or(|l| !copper(l))
    {
        Some("starts and ends with copper")
    } else if s.layers.windows(2).any(|w| copper(&w[0]) == copper(&w[1])) {
        Some("alternates copper and dielectric")
    } else if !(2..=MAX_COPPER).contains(&s.copper()) {
        Some("has 2 to 32 copper layers")
    } else if s.layers.iter().any(|l| match l {
        StackLayer::Copper { thickness_nm } | StackLayer::Dielectric { thickness_nm, .. } => {
            *thickness_nm <= 0
        }
    }) {
        Some("has positive thicknesses")
    } else {
        None
    };
    why.map(|w| {
        Fault::error(Code::BoardStackupInvalid, format!("a stackup {w}"))
            .with("pointer", "/stackup")
    })
    .into_iter()
    .collect()
}

/// The shapes' names of `n` copper layers, in stack order.
pub fn names(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            if i == 0 {
                "copper.top".to_owned()
            } else if i + 1 == n {
                "copper.bottom".to_owned()
            } else {
                format!("copper.in{i}")
            }
        })
        .collect()
}

/// `n` copper layers of the preset's copper with equal FR-4 dielectrics
/// filling its board thickness (what `--layers N` writes).
pub fn even(p: &Preset, n: usize) -> Result<Stackup> {
    if !(2..=MAX_COPPER).contains(&n) {
        return Err(Error::new(Code::InvalidParam, "--layers is 2 to 32").with("param", "layers"));
    }
    let gaps = (n - 1) as i64;
    let each = (p.board_thickness_nm - n as i64 * p.copper_thickness_nm) / gaps;
    if each <= 0 {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{n} copper layers don't fit preset {}'s board thickness",
                p.name
            ),
        )
        .with("param", "layers"));
    }
    let mut layers = Vec::with_capacity(2 * n - 1);
    for i in 0..n {
        if i > 0 {
            layers.push(StackLayer::Dielectric {
                thickness_nm: each,
                material: Some("FR-4".into()),
                er_ppm: None,
                loss_ppm: None,
            });
        }
        layers.push(StackLayer::Copper {
            thickness_nm: p.copper_thickness_nm,
        });
    }
    Ok(Stackup { layers })
}

/// A board's stackup: its own, else its preset's.
pub fn of(b: &super::file::Board) -> Stackup {
    b.stackup.clone().unwrap_or_else(|| {
        super::preset::by_name(&b.preset)
            .unwrap_or(super::preset::DEFAULT)
            .stackup()
    })
}

/// How many copper layers a board has (2 when its own stackup is broken:
/// its fault says so, and nothing may index past what exists).
pub fn copper_count(b: &super::file::Board) -> usize {
    let s = of(b);
    if judge(&s).is_empty() { s.copper() } else { 2 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::preset::JLC_2L;
    use crate::error::Code;

    fn cu() -> StackLayer {
        StackLayer::Copper {
            thickness_nm: 35_000,
        }
    }
    fn fr4(t: i64) -> StackLayer {
        StackLayer::Dielectric {
            thickness_nm: t,
            material: Some("FR-4".into()),
            er_ppm: None,
            loss_ppm: None,
        }
    }

    #[test]
    fn the_presets_stackup_is_two_layers_filling_the_board() {
        let s = JLC_2L.stackup();
        assert_eq!(s.copper(), 2);
        assert_eq!(s.thickness(), JLC_2L.board_thickness_nm);
        assert!(judge(&s).is_empty());
        assert_eq!(names(2), ["copper.top", "copper.bottom"]);
        assert_eq!(
            names(4),
            ["copper.top", "copper.in1", "copper.in2", "copper.bottom"]
        );
    }

    #[test]
    fn even_stackups_fill_the_presets_thickness() {
        let s = even(&JLC_2L, 4).unwrap();
        assert_eq!(s.copper(), 4);
        assert_eq!(s.layers.len(), 7);
        // Three equal dielectrics; the remainder of the division is left out.
        assert!(JLC_2L.board_thickness_nm - s.thickness() < 3);
        assert!(judge(&s).is_empty());
        assert!(even(&JLC_2L, 1).is_err());
        assert!(even(&JLC_2L, 33).is_err());
    }

    #[test]
    fn bad_stackups_are_faults() {
        for layers in [
            vec![cu()],
            vec![cu(), cu()],
            vec![fr4(1), cu(), fr4(1), cu()],
            vec![cu(), fr4(1), fr4(1), cu()],
            vec![cu(), fr4(0), cu()],
            vec![StackLayer::Copper { thickness_nm: -1 }, fr4(1), cu()],
        ] {
            let f = judge(&Stackup { layers });
            assert_eq!(f.len(), 1, "{f:?}");
            assert_eq!(f[0].error.code, Code::BoardStackupInvalid);
            assert_eq!(f[0].error.details["pointer"], "/stackup");
        }
    }

    #[test]
    fn a_stackup_reads_and_writes_canonically() {
        let v = serde_json::json!({"layers": [
            {"kind": "copper", "thickness_nm": 35000},
            {"kind": "dielectric", "thickness_nm": 1530000, "material": "FR-4", "er_ppm": 4500000},
            {"kind": "copper", "thickness_nm": 35000}]});
        let s: Stackup = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap(), v);
        let bad =
            serde_json::json!({"layers": [{"kind": "copper", "thickness_nm": 1, "name": "x"}]});
        assert!(serde_json::from_value::<Stackup>(bad).is_err());
    }
}
