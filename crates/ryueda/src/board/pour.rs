//! A board's pours (design 924): `pours/<id>.json`, copper of one net on
//! one layer over a boundary (a contour, or the whole outline), filled by
//! `ryu_copper::fill` on every build. The fill is derived, never stored.

use super::file::{Contour, CopperLayer};
use crate::error::Code;
use crate::fault::Fault;
use serde::{Deserialize, Serialize};

pub use super::file::{POUR_KIND, POURS};

/// How a pour meets its net's pads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connect {
    #[default]
    Thermal,
    Solid,
}

impl Connect {
    fn is_thermal(&self) -> bool {
        *self == Connect::Thermal
    }
}

/// What a pour does with copper its net doesn't reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Islands {
    #[default]
    Remove,
    Keep,
}

impl Islands {
    fn is_remove(&self) -> bool {
        *self == Islands::Remove
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThermalRule {
    pub gap_nm: i64,
    pub spoke_nm: i64,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pour {
    pub net: String,
    pub layer: CopperLayer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contour: Option<Contour>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cutouts: Vec<Contour>,
    /// The board's outline, following its edits (a plane).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub whole: bool,
    /// Higher fills first; a lower pour clears it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Connect::is_thermal")]
    pub connect: Connect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal: Option<ThermalRule>,
    #[serde(default, skip_serializing_if = "Islands::is_remove")]
    pub islands: Islands,
}

impl Pour {
    /// A plane: `net` over the whole outline on `layer`, defaults else.
    pub fn whole(net: &str, layer: CopperLayer) -> Pour {
        Pour {
            net: net.to_owned(),
            layer,
            contour: None,
            cutouts: Vec::new(),
            whole: true,
            priority: 0,
            clearance_nm: None,
            min_width_nm: None,
            connect: Connect::Thermal,
            thermal: None,
            islands: Islands::Remove,
        }
    }
}

fn contour_fault(c: &Contour, pointer: &str) -> Option<Fault> {
    let bad = |why: &str| {
        Some(
            Fault::error(Code::BoardContourInvalid, format!("a pour's contour {why}"))
                .with("pointer", pointer),
        )
    };
    match super::outline::contour(c) {
        Err(super::outline::Bad::OutOfRange) => bad("reaches past ±2^40 nm"),
        Err(super::outline::Bad::Invalid(why)) => bad(why),
        Ok(k) => {
            let r = super::outline::ring(&k);
            if r.len() < 3 || !super::outline::simple(&r) {
                bad("crosses or touches itself")
            } else {
                None
            }
        }
    }
}

/// A pour's faults on a board of `n` copper layers.
pub fn judge(p: &Pour, n: usize) -> Vec<Fault> {
    let mut out = Vec::new();
    let invalid = |msg: &str, ptr: &str| {
        Fault::error(Code::InvalidDocument, msg.to_owned()).with("pointer", ptr)
    };
    match (&p.contour, p.whole) {
        (Some(_), true) => out.push(invalid(
            "a pour is a contour or the whole board, not both",
            "/whole",
        )),
        (None, false) => out.push(invalid(
            "a pour needs a contour, or whole: true",
            "/contour",
        )),
        (Some(c), false) => out.extend(contour_fault(c, "/contour")),
        (None, true) => {}
    }
    for (i, c) in p.cutouts.iter().enumerate() {
        out.extend(contour_fault(c, &format!("/cutouts/{i}")));
    }
    if p.layer.index(n).is_none() {
        out.push(
            Fault::error(
                Code::BoardLayerNotFound,
                format!(
                    "the board has {n} copper layers and no {}; they are {}",
                    p.layer.short(),
                    super::load::layer_list(n)
                ),
            )
            .with("pointer", "/layer")
            .with("layer", p.layer.short()),
        );
    }
    for (v, ptr) in [
        (p.clearance_nm, "/clearance_nm"),
        (p.min_width_nm, "/min_width_nm"),
        (p.thermal.map(|t| t.gap_nm), "/thermal/gap_nm"),
        (p.thermal.map(|t| t.spoke_nm), "/thermal/spoke_nm"),
    ] {
        if v.is_some_and(|v| v <= 0) {
            out.push(invalid("a pour's lengths are positive", ptr));
        }
    }
    out
}

/// A pour's values under its preset's floor.
pub fn floors(p: &Pour, preset: &super::preset::Preset) -> Vec<Fault> {
    let mut out = Vec::new();
    let below = |what: &str, v: i64, floor: i64, ptr: &str| {
        Fault::error(
            Code::RuleBelowPreset,
            format!(
                "a pour's {what} of {v} nm is under {}'s {floor} nm",
                preset.name
            ),
        )
        .with("pointer", ptr)
    };
    if let Some(c) = p.clearance_nm.filter(|c| *c < preset.clearance_nm) {
        out.push(below("clearance", c, preset.clearance_nm, "/clearance_nm"));
    }
    if let Some(w) = p.min_width_nm.filter(|w| *w < preset.track_width_min_nm) {
        out.push(below(
            "least width",
            w,
            preset.track_width_min_nm,
            "/min_width_nm",
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc;
    use serde_json::json;

    #[test]
    fn a_pour_reads_writes_and_is_judged() {
        let v = json!({"kind": POUR_KIND, "format": 1, "net": "rail:gnd", "layer": "in1", "whole": true,
                       "priority": 2, "connect": "solid", "islands": "keep",
                       "thermal": {"gap_nm": 300_000, "spoke_nm": 400_000}});
        let p: Pour = doc::from_value(v.clone(), "p.json", POUR_KIND, 1).unwrap();
        assert_eq!(doc::to_value(&p, POUR_KIND, 1), v);
        assert!(judge(&p, 4).is_empty());
        assert_eq!(judge(&p, 2)[0].error.code, Code::BoardLayerNotFound);
        let both = Pour {
            contour: Some(crate::board::file::Contour::rect(10, 10)),
            ..p.clone()
        };
        assert_eq!(judge(&both, 4)[0].error.code, Code::InvalidDocument);
        let neither = Pour {
            whole: false,
            ..p.clone()
        };
        assert_eq!(judge(&neither, 4)[0].error.code, Code::InvalidDocument);
        let open = Pour {
            whole: false,
            contour: Some(crate::board::file::Contour {
                start: [0, 0],
                edges: vec![crate::board::file::Op::Line { to: [5, 0] }],
            }),
            ..p.clone()
        };
        assert_eq!(judge(&open, 4)[0].error.code, Code::BoardContourInvalid);
        // Defaults are left out of the file.
        let bare: Pour = doc::from_value(
            json!({"kind": POUR_KIND, "format": 1, "net": "a", "layer": "top", "whole": true}),
            "p.json",
            POUR_KIND,
            1,
        )
        .unwrap();
        assert_eq!(bare.connect, Connect::Thermal);
        assert_eq!(bare.islands, Islands::Remove);
        assert_eq!(
            doc::to_value(&bare, POUR_KIND, 1)
                .as_object()
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn floors_are_the_presets() {
        let p = Pour {
            clearance_nm: Some(10),
            min_width_nm: Some(10),
            ..Pour::whole("a", crate::board::file::CopperLayer::Top)
        };
        let f = floors(&p, &crate::board::preset::JLC_2L);
        assert_eq!(
            f.iter()
                .filter(|x| x.error.code == Code::RuleBelowPreset)
                .count(),
            2
        );
    }
}
