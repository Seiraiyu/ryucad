//! The built-in `jlc-2l` preset (design 732): JLCPCB's published
//! capabilities for its 1.6 mm two-layer process, facts about a process,
//! not copied text. Stage 5.A uses the openings' numbers; 5.B adds the
//! rules. Not a file and not overridable until ryueda 7.
//!
//! A fabricator's process, not a parts connector: this is the one file
//! outside them that names it (`tests/connectors.rs`).

/// A preset's numbers, in nanometres.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    /// Solder mask openings grow beyond a pad by this on every side,
    /// where neither the pad nor its footprint says otherwise.
    pub mask_expansion_nm: i64,
    /// Paste openings change by this on every side (negative shrinks),
    /// where neither the pad nor its footprint says otherwise.
    pub paste_margin_nm: i64,
    /// The width a track takes when its command gives none.
    pub default_track_width_nm: i64,
    /// The drill and diameter a via takes when its command gives none.
    pub default_via_drill_nm: i64,
    pub default_via_diameter_nm: i64,
    /// The board and its copper (1 oz), for exports and readers.
    pub board_thickness_nm: i64,
    pub copper_thickness_nm: i64,
    /// A track's least width.
    pub track_width_min_nm: i64,
    /// The least distance between copper of different islands.
    pub clearance_nm: i64,
    /// The drill range of plated holes, vias' and through pads' alike.
    pub drill_min_nm: i64,
    pub drill_max_nm: i64,
    /// An unplated hole's least drill (its most is `drill_max_nm`).
    pub npth_drill_min_nm: i64,
    /// A via's least diameter.
    pub via_diameter_min_nm: i64,
    /// The least copper round a through pad's hole.
    pub annular_ring_min_nm: i64,
    /// The least copper round a via's hole.
    pub via_annular_ring_min_nm: i64,
    /// The least distance between two holes' walls, unless both are vias.
    pub hole_spacing_nm: i64,
    /// The least distance between two vias' hole walls.
    pub via_hole_spacing_nm: i64,
    /// The least distance from copper or a hole to the outline or a cutout.
    pub edge_clearance_nm: i64,
    /// The least distance from a track to another island's mask opening.
    pub mask_trace_clearance_nm: i64,
}

impl Preset {
    /// The rules as `board drc` answers them.
    pub fn rules_json(&self) -> serde_json::Value {
        serde_json::json!({
            "preset": self.name,
            "board_thickness_nm": self.board_thickness_nm,
            "copper_thickness_nm": self.copper_thickness_nm,
            "track_width_min_nm": self.track_width_min_nm,
            "clearance_nm": self.clearance_nm,
            "drill_min_nm": self.drill_min_nm,
            "drill_max_nm": self.drill_max_nm,
            "npth_drill_min_nm": self.npth_drill_min_nm,
            "via_diameter_min_nm": self.via_diameter_min_nm,
            "annular_ring_min_nm": self.annular_ring_min_nm,
            "via_annular_ring_min_nm": self.via_annular_ring_min_nm,
            "hole_spacing_nm": self.hole_spacing_nm,
            "via_hole_spacing_nm": self.via_hole_spacing_nm,
            "edge_clearance_nm": self.edge_clearance_nm,
            "mask_trace_clearance_nm": self.mask_trace_clearance_nm,
            "mask_expansion_nm": self.mask_expansion_nm,
            "default_track_width_nm": self.default_track_width_nm,
            "default_via_drill_nm": self.default_via_drill_nm,
            "default_via_diameter_nm": self.default_via_diameter_nm,
        })
    }
}

/// Read from https://jlcpcb.com/capabilities/pcb-capabilities on
/// 2026-11-17. Its "Soldermask Expansion" row says "1:1": "LDI equipment
/// upgraded in June 2025. Pad size:soldermask opening can be 1:1", so the
/// mask opening is the pad, an expansion of 0. The page gives a 0.09 mm
/// minimum clearance between soldermask openings and neighbouring traces
/// ("Keep at least 0.09 mm clearance between soldermask openings and
/// neighboring traces") and no paste margin, so paste is the pad too.
///
/// The copper rules were read by the coxswain from the same page on
/// 2026-11-17, for its 1-2 layer, 1.6 mm, 1 oz process. Its rows:
///
/// - Track width and clearance: "Min. track width and spacing (1 oz)":
///   "1- and 2-layer: 0.10 / 0.10 mm (4 / 4 mil)", and "Pad to track
///   clearance (1oz)": "0.1mm". One clearance rule, 0.10 mm.
/// - Drills: PTH "2-layer: 0.15 – 6.3 mm"; via "2-layer: 0.15mm hole
///   size / 0.25mm via diameter"; "Min. Non-plated holes": "0.50mm". The
///   page also allows 0.1 mm "(more costly!)"; the preset takes the
///   standard 0.15 mm.
/// - Via diameter: 0.25 mm, from the via row above.
/// - Annular ring: "PTH annular ring": "≧0.20mm" (the 2-layer 1 oz row
///   reads "Recommended 0.25 mm or above; absolute minimum 0.18 mm"), so
///   0.20 mm round a through pad's hole. The page has no via row: a via's
///   ring is taken from its minimum geometry, (0.25 mm - 0.15 mm) / 2 =
///   0.05 mm. Two rules, because one 0.20 mm rule would refuse the page's
///   own smallest via.
/// - Hole to hole: "Via Hole-to-Hole Spacing": "0.2mm"; "Pad Hole-to-Hole
///   Spacing": "0.45mm". Both kept: 0.2 mm between two vias' holes,
///   0.45 mm between any other two.
/// - Edge: "Copper clearance from routed board edges: ≧0.2 mm". The page
///   gives no general hole-to-edge figure, so holes take the same 0.2 mm.
/// - Thickness: FR4 options 0.4/0.6/0.8/1.0/1.2/1.6/2.0 mm, this process
///   1.6 mm ±10%; outer copper 1 oz. The page gives no µm figure for 1 oz:
///   35 µm is 1 oz/ft² converted (about 34.8 µm), not the page's.
///
/// ryueda's defaults (a 0.25 mm track, a 0.30 mm drill in a 0.60 mm via)
/// are its own choices and meet every rule above.
pub const JLC_2L: Preset = Preset {
    name: "jlc-2l",
    mask_expansion_nm: 0,
    paste_margin_nm: 0,
    default_track_width_nm: 250_000,
    default_via_drill_nm: 300_000,
    default_via_diameter_nm: 600_000,
    board_thickness_nm: 1_600_000,
    copper_thickness_nm: 35_000,
    track_width_min_nm: 100_000,
    clearance_nm: 100_000,
    drill_min_nm: 150_000,
    drill_max_nm: 6_300_000,
    npth_drill_min_nm: 500_000,
    via_diameter_min_nm: 250_000,
    annular_ring_min_nm: 200_000,
    via_annular_ring_min_nm: 50_000,
    hole_spacing_nm: 450_000,
    via_hole_spacing_nm: 200_000,
    edge_clearance_nm: 200_000,
    mask_trace_clearance_nm: 90_000,
};

/// The preset a new board takes.
pub const DEFAULT: &Preset = &JLC_2L;

/// Every preset this ryueda knows.
pub const ALL: [&Preset; 1] = [&JLC_2L];

/// The preset a board names; stage 5 knows one.
pub fn by_name(name: &str) -> Option<&'static Preset> {
    ALL.into_iter().find(|p| p.name == name)
}

/// The known presets' names, for messages.
pub fn names() -> String {
    ALL.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_one_preset_is_found_by_name() {
        assert_eq!(by_name("jlc-2l"), Some(&JLC_2L));
        assert_eq!(by_name("acme-6l"), None);
        assert_eq!(DEFAULT.name, "jlc-2l");
    }

    #[test]
    fn the_defaults_meet_the_rules() {
        let p = JLC_2L;
        for (name, v) in [
            ("board_thickness_nm", p.board_thickness_nm),
            ("copper_thickness_nm", p.copper_thickness_nm),
            ("track_width_min_nm", p.track_width_min_nm),
            ("clearance_nm", p.clearance_nm),
            ("drill_min_nm", p.drill_min_nm),
            ("npth_drill_min_nm", p.npth_drill_min_nm),
            ("via_diameter_min_nm", p.via_diameter_min_nm),
            ("annular_ring_min_nm", p.annular_ring_min_nm),
            ("via_annular_ring_min_nm", p.via_annular_ring_min_nm),
            ("hole_spacing_nm", p.hole_spacing_nm),
            ("via_hole_spacing_nm", p.via_hole_spacing_nm),
            ("edge_clearance_nm", p.edge_clearance_nm),
        ] {
            assert!(v > 0, "{name}");
        }
        assert!(p.drill_max_nm > p.drill_min_nm);
        assert!(p.drill_max_nm > p.npth_drill_min_nm);
        assert!(p.hole_spacing_nm >= p.via_hole_spacing_nm);
        assert_eq!(p.mask_trace_clearance_nm, 90_000, "the page's 0.09 mm");
        assert!(p.default_track_width_nm >= p.track_width_min_nm);
        assert!((p.drill_min_nm..=p.drill_max_nm).contains(&p.default_via_drill_nm));
        assert!(p.default_via_diameter_nm >= p.via_diameter_min_nm);
        assert!(
            p.default_via_diameter_nm - p.default_via_drill_nm >= 2 * p.via_annular_ring_min_nm
        );
        // The page's smallest via meets its own rules.
        assert!(p.via_diameter_min_nm - p.drill_min_nm >= 2 * p.via_annular_ring_min_nm);
        let v = p.rules_json();
        assert_eq!(v["clearance_nm"], serde_json::json!(p.clearance_nm));
        assert_eq!(v["preset"], "jlc-2l");
    }
}
