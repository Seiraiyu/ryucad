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
    for (id, w) in &r.waivers {
        if NOT_WAIVABLE.contains(&w.code) {
            out.push(
                Fault::error(
                    Code::FindingNotWaivable,
                    format!(
                        "waiver {id}: {} can't be waived",
                        serde_json::json!(w.code).as_str().unwrap_or("")
                    ),
                )
                .with("pointer", format!("/waivers/{id}")),
            );
        }
        if id.len() != 16
            || !id.bytes().all(|b| b.is_ascii_hexdigit())
            || w.reason.trim().is_empty()
        {
            out.push(
                Fault::error(
                    Code::InvalidDocument,
                    format!("waiver {id} needs a finding's ID (16 hex digits) and a reason"),
                )
                .with("pointer", format!("/waivers/{id}")),
            );
        }
    }
    out
}

/// A resolved value and the layer that set it: `class:<id>`, `board`,
/// `preset:<name>`, or `ryueda` (a built-in default).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Resolved {
    pub value: i64,
    pub source: String,
}

/// A board's rules resolved against its preset and its design's classes.
#[derive(Clone, Debug)]
pub struct Rules {
    pub preset: std::sync::Arc<Preset>,
    pub file: RulesFile,
    /// Each board net (as the board names it: `sda`, `rail:gnd`) that has a
    /// class, with it.
    pub class_of: BTreeMap<String, Id>,
}

/// The placement grid when the rules give none: 0.1 mm.
pub const PLACEMENT_GRID_NM: i64 = 100_000;

impl Rules {
    /// A preset's rules alone: no rules.json, no classes.
    pub fn plain(preset: &Preset) -> Rules {
        Rules::from_parts(std::sync::Arc::new(preset.clone()), None, BTreeMap::new())
    }

    pub fn from_parts(
        preset: std::sync::Arc<Preset>,
        file: Option<RulesFile>,
        class_of: BTreeMap<String, Id>,
    ) -> Rules {
        Rules {
            preset,
            file: file.unwrap_or_default(),
            class_of,
        }
    }

    /// The rules of a board whose design flattened to `n`.
    pub fn new(
        preset: std::sync::Arc<Preset>,
        file: Option<&RulesFile>,
        n: &crate::netlist::Netlist,
    ) -> Rules {
        let class_of = n
            .nets
            .iter()
            .filter_map(|x| Some((super::occ::local(&n.design, &x.id), x.class.clone()?)))
            .collect();
        Rules::from_parts(preset, file.cloned(), class_of)
    }

    fn class_set(&self, net: Option<&str>) -> Option<(&Id, &RuleSet)> {
        let id = self.class_of.get(net?)?;
        Some((id, self.file.classes.get(id)?))
    }

    /// The class's value, else the board's, else `fallback` from the preset;
    /// never under `floor`.
    fn pick(
        &self,
        net: Option<&str>,
        get: impl Fn(&RuleSet) -> Option<i64>,
        fallback: i64,
        floor: i64,
    ) -> Resolved {
        let found = self
            .class_set(net)
            .and_then(|(id, s)| get(s).map(|v| (v, format!("class:{id}"))))
            .or_else(|| get(&self.file.defaults).map(|v| (v, "board".to_owned())));
        match found {
            Some((v, source)) if v >= floor => Resolved { value: v, source },
            // Under the floor: a fault of the file; DRC applies the floor.
            _ => Resolved {
                value: fallback.max(floor),
                source: format!("preset:{}", self.preset.name),
            },
        }
    }

    pub fn clearance(&self, net: Option<&str>) -> Resolved {
        let p = &self.preset;
        self.pick(net, |s| s.clearance_nm, p.clearance_nm, p.clearance_nm)
    }

    /// The least width DRC applies to a track of `net`.
    pub fn track_width_min(&self, net: Option<&str>) -> Resolved {
        let p = &self.preset;
        self.pick(
            net,
            |s| s.track_width_nm,
            p.track_width_min_nm,
            p.track_width_min_nm,
        )
    }

    /// The width a track of `net` takes when drawn with none.
    pub fn track_width_default(&self, net: Option<&str>) -> Resolved {
        let p = &self.preset;
        self.pick(
            net,
            |s| s.track_width_nm,
            p.default_track_width_nm,
            p.track_width_min_nm,
        )
    }

    /// The size a via of `net` takes when drawn with none.
    pub fn via_default(&self, net: Option<&str>) -> (ViaRule, String) {
        let found = self
            .class_set(net)
            .and_then(|(id, s)| s.via.map(|v| (v, format!("class:{id}"))))
            .or_else(|| self.file.defaults.via.map(|v| (v, "board".to_owned())));
        found.unwrap_or_else(|| {
            (
                ViaRule {
                    drill_nm: self.preset.default_via_drill_nm,
                    diameter_nm: self.preset.default_via_diameter_nm,
                },
                format!("preset:{}", self.preset.name),
            )
        })
    }

    /// A net's hole clearance rule, if the rules set one (the preset's
    /// per-kind values apply under it).
    pub fn hole_clearance(&self, net: Option<&str>) -> Option<Resolved> {
        let r = self.pick(net, |s| s.hole_clearance_nm, 0, 0);
        (!r.source.starts_with("preset:")).then_some(r)
    }

    pub fn edge_clearance(&self, net: Option<&str>) -> Resolved {
        let p = &self.preset;
        self.pick(
            net,
            |s| s.edge_clearance_nm,
            p.edge_clearance_nm,
            p.edge_clearance_nm,
        )
    }

    pub fn courtyard_clearance(&self) -> Resolved {
        match self.file.defaults.courtyard_clearance_nm {
            Some(v) if v >= 0 => Resolved {
                value: v,
                source: "board".into(),
            },
            _ => Resolved {
                value: 0,
                source: "ryueda".into(),
            },
        }
    }

    pub fn placement_grid(&self) -> Resolved {
        match self.file.defaults.placement_grid_nm {
            Some(v) if v > 0 => Resolved {
                value: v,
                source: "board".into(),
            },
            _ => Resolved {
                value: PLACEMENT_GRID_NM,
                source: "ryueda".into(),
            },
        }
    }

    /// The clearance two islands need: the largest over both's nets (an
    /// island with no net takes the board's).
    pub fn pair_clearance(&self, a: &[&str], b: &[&str]) -> Resolved {
        let side = |nets: &[&str]| -> Resolved {
            if nets.is_empty() {
                return self.clearance(None);
            }
            nets.iter()
                .map(|n| self.clearance(Some(n)))
                .max_by_key(|r| r.value)
                .unwrap()
        };
        let (x, y) = (side(a), side(b));
        if y.value > x.value { y } else { x }
    }

    /// The largest clearance any pair can need: what the engine is asked.
    pub fn widest_clearance(&self) -> i64 {
        std::iter::once(&self.file.defaults)
            .chain(self.file.classes.values())
            .filter_map(|s| s.clearance_nm)
            .chain(std::iter::once(self.preset.clearance_nm))
            .max()
            .unwrap_or(self.preset.clearance_nm)
            .max(self.preset.clearance_nm)
    }

    /// Whether any clearance is the board's own (else DRC runs exactly as
    /// stage 5's).
    pub fn has_clearance_rules(&self) -> bool {
        std::iter::once(&self.file.defaults)
            .chain(self.file.classes.values())
            .any(|s| s.clearance_nm.is_some())
    }

    pub fn has_width_rules(&self) -> bool {
        std::iter::once(&self.file.defaults)
            .chain(self.file.classes.values())
            .any(|s| s.track_width_nm.is_some())
    }

    pub fn has_hole_rules(&self) -> bool {
        std::iter::once(&self.file.defaults)
            .chain(self.file.classes.values())
            .any(|s| s.hole_clearance_nm.is_some())
    }

    pub fn has_edge_rules(&self) -> bool {
        std::iter::once(&self.file.defaults)
            .chain(self.file.classes.values())
            .any(|s| s.edge_clearance_nm.is_some())
    }

    fn set_json(&self, net: Option<&str>) -> serde_json::Value {
        let (via, via_source) = self.via_default(net);
        let mut v = serde_json::json!({
            "clearance_nm": self.clearance(net),
            "track_width_nm": self.track_width_min(net),
            "track_width_default_nm": self.track_width_default(net),
            "via": {"drill_nm": via.drill_nm, "diameter_nm": via.diameter_nm, "source": via_source},
            "edge_clearance_nm": self.edge_clearance(net),
        });
        if let Some(h) = self.hole_clearance(net) {
            v["hole_clearance_nm"] = serde_json::json!(h);
        }
        v
    }

    /// The resolved rules: board-wide, per class, and each net's class.
    pub fn to_json(&self) -> serde_json::Value {
        let mut board = self.set_json(None);
        board["courtyard_clearance_nm"] = serde_json::json!(self.courtyard_clearance());
        board["placement_grid_nm"] = serde_json::json!(self.placement_grid());
        if let Some(p) = self.file.defaults.pour {
            board["pour"] = serde_json::json!(p);
        }
        let classes: serde_json::Map<String, serde_json::Value> = self
            .file
            .classes
            .keys()
            .map(|id| {
                // Any net of the class resolves as the class.
                let net = self
                    .class_of
                    .iter()
                    .find(|(_, c)| *c == id)
                    .map(|(n, _)| n.as_str());
                (
                    id.to_string(),
                    match net {
                        Some(n) => self.set_json(Some(n)),
                        None => serde_json::json!({"nets": 0}),
                    },
                )
            })
            .collect();
        serde_json::json!({
            "preset": self.preset.name,
            "board": board,
            "classes": classes,
            "nets": self.class_of,
        })
    }
}

/// A board's rules from the project: its preset, its `rules.json` (when
/// it reads) and its design's classes.
pub fn of(p: &crate::project::Project, b: &super::load::BoardFiles) -> crate::error::Result<Rules> {
    let file = b.board.item.as_ref().map_err(Clone::clone)?;
    let preset = super::preset::resolve(p, &file.preset)
        .unwrap_or_else(|| std::sync::Arc::new(super::preset::DEFAULT.clone()));
    let n = crate::netlist::flatten(p, &file.design)?;
    let rules = b.rules.as_ref().and_then(|l| l.item.as_ref().ok());
    Ok(Rules::new(preset, rules, &n))
}

/// Codes a waiver can't waive: a short is a different circuit from the
/// schematic; an unbuildable footprint leaves copper out of the check.
pub const NOT_WAIVABLE: [Code; 2] = [Code::NetShort, Code::BoardFootprintUnsupported];

/// A finding's measurement, for codes that have one, and whether a larger
/// value is worse (a drill over the maximum) or a smaller one (the rest).
pub fn measurement(code: Code, details: &serde_json::Value) -> Option<(i64, bool)> {
    let get = |k: &str| details[k].as_i64();
    match code {
        Code::DrcClearance
        | Code::DrcMaskClearance
        | Code::DrcHoleClearance
        | Code::DrcEdgeClearance
        | Code::DrcHoleSpacing => Some((get("distance_nm")?, false)),
        Code::DrcTrackWidth => Some((get("width_nm")?, false)),
        Code::DrcAnnularRing => Some((get("ring_nm")?, false)),
        Code::DrcDrill => {
            let d = get("drill_nm")?;
            Some((d, get("max_nm").is_some_and(|m| d > m)))
        }
        _ => None,
    }
}

/// Marks each finding a waiver holds; a finding that got worse than its
/// waiver's limit stays an error with `details.waiver_exceeded`. Returns
/// the waivers no finding used, as `waiver_unused` warnings.
pub fn apply_waivers(
    findings: &mut [crate::board::drc::Finding],
    waivers: &BTreeMap<String, Waiver>,
    board: &str,
) -> Vec<crate::board::drc::Finding> {
    let mut used = BTreeSet::new();
    for f in findings.iter_mut() {
        let Some(w) = waivers.get(&f.id) else {
            continue;
        };
        if NOT_WAIVABLE.contains(&f.code) {
            continue;
        }
        used.insert(f.id.clone());
        let worse = match (w.limit, measurement(f.code, &f.details)) {
            (Some(limit), Some((now, larger_worse))) => {
                if larger_worse {
                    now > limit
                } else {
                    now < limit
                }
            }
            _ => false,
        };
        let about = serde_json::json!({"reason": w.reason, "by": w.by, "limit": w.limit});
        if worse {
            if let Some(d) = f.details.as_object_mut() {
                let mut x = about.clone();
                x["code"] = serde_json::json!(Code::WaiverExceeded);
                d.insert("waiver_exceeded".into(), x);
            }
        } else {
            f.waived = Some(about);
        }
    }
    waivers
        .iter()
        .filter(|(id, w)| !used.contains(*id) && !NOT_WAIVABLE.contains(&w.code))
        .map(|(id, w)| {
            crate::board::drc::finding(
                Code::WaiverUnused,
                crate::fault::Severity::Warning,
                board,
                vec![format!("{board}:waiver/{id}")],
                format!(
                    "waiver {id} ({}) waives a finding the board doesn't have",
                    w.reason
                ),
                serde_json::json!({"waiver": id, "code": w.code, "refs": w.refs}),
            )
        })
        .collect()
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
    fn rules(file: Option<RulesFile>) -> Rules {
        let class_of: BTreeMap<String, Id> =
            [("rail:vin".to_owned(), Id::parse("power", "class").unwrap())]
                .into_iter()
                .collect();
        Rules::from_parts(std::sync::Arc::new(JLC_2L), file, class_of)
    }

    #[test]
    fn a_value_is_its_class_else_the_board_else_the_preset() {
        let r = rules(None);
        let c = r.clearance(Some("rail:vin"));
        assert_eq!(
            (c.value, c.source.as_str()),
            (JLC_2L.clearance_nm, "preset:jlc-2l")
        );
        let mut f = RulesFile::default();
        f.defaults.clearance_nm = Some(150_000);
        f.defaults.track_width_nm = Some(200_000);
        f.classes.insert(
            Id::parse("power", "class").unwrap(),
            RuleSet {
                clearance_nm: Some(250_000),
                ..RuleSet::default()
            },
        );
        let r = rules(Some(f));
        assert_eq!(r.clearance(Some("rail:vin")).value, 250_000);
        assert_eq!(r.clearance(Some("rail:vin")).source, "class:power");
        assert_eq!(r.clearance(Some("sda")).value, 150_000);
        assert_eq!(r.clearance(Some("sda")).source, "board");
        assert_eq!(r.clearance(None).source, "board");
        // The class sets no width: the board's.
        assert_eq!(r.track_width_min(Some("rail:vin")).source, "board");
        assert_eq!(r.track_width_default(Some("rail:vin")).value, 200_000);
        // With nothing set, the least width is the preset's minimum and the
        // drawn width its default.
        let r0 = rules(None);
        assert_eq!(
            r0.track_width_min(Some("sda")).value,
            JLC_2L.track_width_min_nm
        );
        assert_eq!(
            r0.track_width_default(Some("sda")).value,
            JLC_2L.default_track_width_nm
        );
        assert_eq!(r0.widest_clearance(), JLC_2L.clearance_nm);
        assert_eq!(r.widest_clearance(), 250_000);
    }

    #[test]
    fn a_pair_takes_the_larger_side() {
        let mut f = RulesFile::default();
        f.classes.insert(
            Id::parse("power", "class").unwrap(),
            RuleSet {
                clearance_nm: Some(250_000),
                ..RuleSet::default()
            },
        );
        let r = rules(Some(f));
        let p = r.pair_clearance(&["sda"], &["rail:vin"]);
        assert_eq!((p.value, p.source.as_str()), (250_000, "class:power"));
        let q = r.pair_clearance(&["sda"], &["scl"]);
        assert_eq!(q.source, "preset:jlc-2l");
        assert!(r.has_clearance_rules());
        assert!(!rules(None).has_clearance_rules());
    }

    #[test]
    fn the_json_lists_each_board_value_and_each_class() {
        let mut f = RulesFile::default();
        f.defaults.edge_clearance_nm = Some(300_000);
        f.classes.insert(
            Id::parse("power", "class").unwrap(),
            RuleSet {
                clearance_nm: Some(250_000),
                ..RuleSet::default()
            },
        );
        let v = rules(Some(f)).to_json();
        assert_eq!(
            v["board"]["edge_clearance_nm"],
            serde_json::json!({"value": 300000, "source": "board"})
        );
        assert_eq!(v["board"]["clearance_nm"]["source"], "preset:jlc-2l");
        assert_eq!(v["classes"]["power"]["clearance_nm"]["value"], 250000);
        assert_eq!(v["nets"]["rail:vin"], "power");
    }
    fn finding(
        code: Code,
        refs: &[&str],
        details: serde_json::Value,
    ) -> crate::board::drc::Finding {
        crate::board::drc::finding(
            code,
            crate::fault::Severity::Error,
            "b",
            refs.iter().map(|r| (*r).to_owned()).collect(),
            "m".into(),
            details,
        )
    }

    fn waiver(f: &crate::board::drc::Finding, limit: Option<i64>) -> (String, Waiver) {
        (
            f.id.clone(),
            Waiver {
                code: f.code,
                refs: f.refs.clone(),
                reason: "measured".into(),
                by: "agent".into(),
                limit,
            },
        )
    }

    #[test]
    fn a_waiver_holds_at_and_over_its_limit_and_not_under() {
        let gap = |d: i64| {
            finding(
                Code::DrcClearance,
                &["b:R1/1", "b:R2/1"],
                serde_json::json!({"distance_nm": d, "rule_nm": 100_000}),
            )
        };
        let waivers: BTreeMap<String, Waiver> =
            [waiver(&gap(90_000), Some(90_000))].into_iter().collect();
        for (d, waived) in [(95_000, true), (90_000, true), (89_999, false)] {
            let mut f = vec![gap(d)];
            let unused = apply_waivers(&mut f, &waivers, "b");
            assert!(unused.is_empty());
            assert_eq!(f[0].waived.is_some(), waived, "{d}");
            assert_eq!(f[0].is_error(), !waived, "{d}");
            if !waived {
                assert_eq!(f[0].details["waiver_exceeded"]["limit"], 90_000);
                assert_eq!(f[0].details["waiver_exceeded"]["code"], "waiver_exceeded");
            }
        }
    }

    #[test]
    fn a_drill_over_the_maximum_is_worse_when_larger() {
        let big = |d: i64| {
            finding(
                Code::DrcDrill,
                &["b:hole/h1"],
                serde_json::json!({"drill_nm": d, "min_nm": 500_000, "max_nm": 6_300_000}),
            )
        };
        let w: BTreeMap<String, Waiver> = [waiver(&big(6_400_000), Some(6_400_000))]
            .into_iter()
            .collect();
        let mut f = vec![big(6_500_000)];
        apply_waivers(&mut f, &w, "b");
        assert!(f[0].waived.is_none());
        let mut f = vec![big(6_350_000)];
        apply_waivers(&mut f, &w, "b");
        assert!(f[0].waived.is_some());
    }

    #[test]
    fn an_unused_waiver_is_a_warning_and_shorts_are_not_waived() {
        let short = finding(Code::NetShort, &["b:R1/1"], serde_json::Value::Null);
        let w: BTreeMap<String, Waiver> = [
            waiver(&short, None),
            (
                "0123456789abcdef".into(),
                Waiver {
                    code: Code::DrcClearance,
                    refs: vec![],
                    reason: "gone".into(),
                    by: "agent".into(),
                    limit: None,
                },
            ),
        ]
        .into_iter()
        .collect();
        let mut f = vec![short.clone()];
        let unused = apply_waivers(&mut f, &w, "b");
        assert!(f[0].waived.is_none(), "a short is never waived");
        assert_eq!(unused.len(), 1);
        assert_eq!(unused[0].code, Code::WaiverUnused);
        assert!(!unused[0].is_error());
        assert_eq!(unused[0].refs, ["b:waiver/0123456789abcdef"]);
        let file = RulesFile {
            waivers: w,
            ..RulesFile::default()
        };
        let faults = judge(&file, &JLC_2L, &classes());
        assert!(
            faults
                .iter()
                .any(|x| x.error.code == Code::FindingNotWaivable),
            "{faults:?}"
        );
    }
}
