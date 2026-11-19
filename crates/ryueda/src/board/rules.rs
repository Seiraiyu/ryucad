//! A board's rules (design 924): `boards/<board>/rules.json`, kind
//! `ryueda.board-rules`: defaults, rules per net class, and waivers. Every
//! value is judged against the board's preset's floor; resolution is
//! [`Rules`].

use super::preset::Preset;
use crate::error::Code;
use crate::fault::Fault;
use crate::id::Id;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const FORMAT: u64 = 1;

/// A via's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViaRule {
    pub drill_nm: i64,
    pub diameter_nm: i64,
}

/// A pour's settings (read by 7.D).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PourRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal_gap_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal_spoke_nm: Option<i64>,
}

/// One layer of rules: the board's defaults, or a class's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSet {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance_nm: Option<i64>,
    /// The least width, and the width a track takes when drawn with none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_width_nm: Option<i64>,
    /// The size a via takes when drawn with none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<ViaRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hole_clearance_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_clearance_nm: Option<i64>,
    /// Board-wide only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub courtyard_clearance_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pour: Option<PourRule>,
    /// Board-wide only (read by 7.C's legaliser).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_grid_nm: Option<i64>,
}

impl RuleSet {
    pub fn is_empty(&self) -> bool {
        *self == RuleSet::default()
    }
}

/// A waived finding (Task 13 reads these).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiver {
    pub code: Code,
    pub refs: Vec<String>,
    pub reason: String,
    pub by: String,
    /// The finding's measurement when it was waived, for codes that have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

/// `rules.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesFile {
    #[serde(default, skip_serializing_if = "RuleSet::is_empty")]
    pub defaults: RuleSet,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub classes: BTreeMap<Id, RuleSet>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub waivers: BTreeMap<String, Waiver>,
}

fn below(pointer: String, what: &str, value: i64, floor: i64, preset: &str) -> Fault {
    Fault::error(
        Code::RuleBelowPreset,
        format!("{what} {value} nm is under preset {preset}'s floor of {floor} nm"),
    )
    .with("pointer", pointer)
    .with("floor_nm", floor)
}

/// One rule set's faults, its pointer prefix `at` (`/defaults`,
/// `/classes/power`).
fn judge_set(s: &RuleSet, p: &Preset, at: &str, class: bool, out: &mut Vec<Fault>) {
    let name = &*p.name;
    let mut floor = |field: &str, v: Option<i64>, f: i64| {
        if let Some(v) = v
            && v < f
        {
            out.push(below(format!("{at}/{field}"), field, v, f, name));
        }
    };
    floor("clearance_nm", s.clearance_nm, p.clearance_nm);
    floor("track_width_nm", s.track_width_nm, p.track_width_min_nm);
    floor(
        "hole_clearance_nm",
        s.hole_clearance_nm,
        p.via_hole_clearance_nm
            .min(p.pth_hole_clearance_nm)
            .min(p.npth_hole_clearance_nm),
    );
    floor(
        "edge_clearance_nm",
        s.edge_clearance_nm,
        p.edge_clearance_nm,
    );
    floor("courtyard_clearance_nm", s.courtyard_clearance_nm, 0);
    floor("placement_grid_nm", s.placement_grid_nm, 1);
    if let Some(v) = s.via {
        if v.drill_nm < p.drill_min_nm {
            out.push(below(
                format!("{at}/via/drill_nm"),
                "a via's drill",
                v.drill_nm,
                p.drill_min_nm,
                name,
            ));
        } else if v.drill_nm > p.drill_max_nm {
            out.push(
                Fault::error(
                    Code::RuleBelowPreset,
                    format!(
                        "a via's drill {} nm is over preset {name}'s {} nm",
                        v.drill_nm, p.drill_max_nm
                    ),
                )
                .with("pointer", format!("{at}/via/drill_nm"))
                .with("max_nm", p.drill_max_nm),
            );
        }
        if v.diameter_nm < p.via_diameter_min_nm {
            out.push(below(
                format!("{at}/via/diameter_nm"),
                "a via's diameter",
                v.diameter_nm,
                p.via_diameter_min_nm,
                name,
            ));
        } else if v.diameter_nm - v.drill_nm < 2 * p.via_annular_ring_min_nm {
            out.push(below(
                format!("{at}/via/diameter_nm"),
                "a via's ring",
                (v.diameter_nm - v.drill_nm) / 2,
                p.via_annular_ring_min_nm,
                name,
            ));
        }
    }
    if let Some(r) = s.pour {
        let mut pf = |field: &str, v: Option<i64>, f: i64| {
            if let Some(v) = v
                && v < f
            {
                out.push(below(format!("{at}/pour/{field}"), field, v, f, name));
            }
        };
        pf("clearance_nm", r.clearance_nm, p.clearance_nm);
        pf("min_width_nm", r.min_width_nm, p.track_width_min_nm);
        pf("thermal_gap_nm", r.thermal_gap_nm, p.clearance_nm);
        pf("thermal_spoke_nm", r.thermal_spoke_nm, p.track_width_min_nm);
    }
    if class {
        for (field, set) in [
            ("courtyard_clearance_nm", s.courtyard_clearance_nm.is_some()),
            ("placement_grid_nm", s.placement_grid_nm.is_some()),
        ] {
            if set {
                out.push(
                    Fault::error(
                        Code::InvalidDocument,
                        format!("{field} is board-wide: set it in defaults"),
                    )
                    .with("pointer", format!("{at}/{field}")),
                );
            }
        }
    }
}

/// What is wrong with a rules file against its board's preset and its
/// design's classes; each fault without its `path` (the caller adds it).
pub fn judge(r: &RulesFile, preset: &Preset, classes: &BTreeSet<Id>) -> Vec<Fault> {
    let mut out = Vec::new();
    judge_set(&r.defaults, preset, "/defaults", false, &mut out);
    for (id, s) in &r.classes {
        let at = format!("/classes/{id}");
        if !classes.contains(id) {
            out.push(
                Fault::error(
                    Code::BoardClassNotFound,
                    format!("the design has no net class {id}"),
                )
                .with("pointer", &at)
                .with("class", id),
            );
            continue;
        }
        judge_set(s, preset, &at, true, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::preset::JLC_2L;
    use std::collections::BTreeSet;

    fn classes() -> BTreeSet<Id> {
        [Id::parse("power", "class").unwrap()].into_iter().collect()
    }

    #[test]
    fn a_rules_file_reads_and_writes_canonically() {
        let v = serde_json::json!({
            "defaults": {"clearance_nm": 150000, "via": {"drill_nm": 300000, "diameter_nm": 600000}},
            "classes": {"power": {"track_width_nm": 500000, "clearance_nm": 200000}},
            "waivers": {"0123456789abcdef": {"code": "drc_clearance", "refs": ["b:R1/1", "b:R2/1"],
                "reason": "measured", "by": "agent", "limit": 95000}}});
        let r: RulesFile = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(&r).unwrap(), v);
        assert!(
            serde_json::from_value::<RulesFile>(
                serde_json::json!({"defaults": {"clearence_nm": 1}})
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(RulesFile::default()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn values_under_the_floor_and_unknown_classes_are_faults() {
        let mut r = RulesFile::default();
        r.defaults.clearance_nm = Some(JLC_2L.clearance_nm - 1);
        r.defaults.via = Some(ViaRule {
            drill_nm: 100_000,
            diameter_nm: 600_000,
        });
        r.classes
            .insert(Id::parse("ghost", "class").unwrap(), RuleSet::default());
        r.classes.insert(
            Id::parse("power", "class").unwrap(),
            RuleSet {
                placement_grid_nm: Some(100_000),
                track_width_nm: Some(1),
                ..RuleSet::default()
            },
        );
        let f = judge(&r, &JLC_2L, &classes());
        let got: Vec<(String, String)> = f
            .iter()
            .map(|x| {
                (
                    format!("{:?}", x.error.code),
                    x.error.details["pointer"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("RuleBelowPreset".into(), "/defaults/clearance_nm".into()),
                ("RuleBelowPreset".into(), "/defaults/via/drill_nm".into()),
                ("BoardClassNotFound".into(), "/classes/ghost".into()),
                (
                    "RuleBelowPreset".into(),
                    "/classes/power/track_width_nm".into()
                ),
                (
                    "InvalidDocument".into(),
                    "/classes/power/placement_grid_nm".into()
                ),
            ]
        );
        assert!(
            f[0].error
                .message
                .contains(&JLC_2L.clearance_nm.to_string()),
            "{}",
            f[0].error.message
        );
        let ok = RulesFile {
            defaults: RuleSet {
                clearance_nm: Some(JLC_2L.clearance_nm),
                ..RuleSet::default()
            },
            ..RulesFile::default()
        };
        assert!(judge(&ok, &JLC_2L, &classes()).is_empty());
    }
}
