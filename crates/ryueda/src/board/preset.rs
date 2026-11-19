//! The built-in `jlc-2l` preset (design 732): JLCPCB's published
//! capabilities for its 1.6 mm two-layer process, facts about a process,
//! not copied text. Stage 5.A uses the openings' numbers; 5.B adds the
//! rules. Not a file and not overridable until ryueda 7.
//!
//! A fabricator's process, not a parts connector: this is the one file
//! outside them that names it (`tests/connectors.rs`).
//!
//! Since ryueda 7.B a preset is a document (kind `ryueda.fab-preset`): the
//! built-ins stay constants here, with their pages quoted beside their
//! numbers, and their document is their canonical serialisation; a
//! project may hold its own in `presets/<id>.json`.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// A preset document's kind and format.
pub const KIND: &str = "ryueda.fab-preset";
pub const FORMAT: u64 = 1;

/// How a fabricator's assembly service reads a BOM and a pick and place
/// file. `board export` writes both in this form beside its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssemblyForm {
    /// The files' suffix: `<board>-bom-<name>.csv`, `<board>-pos-<name>.csv`.
    pub name: Cow<'static, str>,
    /// The supplier whose part numbers its BOM carries (an offer's
    /// supplier, compared ignoring case and spaces).
    pub supplier: Cow<'static, str>,
    /// The BOM's columns: the value, the designators, the footprint, the
    /// supplier's part number.
    pub bom: [Cow<'static, str>; 4],
    /// The pick and place file's columns: the designator, x, y, the side,
    /// the rotation.
    pub pos: [Cow<'static, str>; 5],
    /// What follows a length in the pick and place file (`mm`), and the
    /// top and bottom sides' names there.
    pub length_suffix: Cow<'static, str>,
    pub top: Cow<'static, str>,
    pub bottom: Cow<'static, str>,
}

/// A preset's numbers, in nanometres.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub name: Cow<'static, str>,
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
    /// The least distance from copper to a via's hole wall, a through
    /// pad's and an unplated hole's, where the hole doesn't join it.
    pub via_hole_clearance_nm: i64,
    pub pth_hole_clearance_nm: i64,
    pub npth_hole_clearance_nm: i64,
    /// Whether the process makes blind and buried vias.
    pub blind_vias: bool,
    /// Inner layers' least distance from copper to a via's and a through
    /// pad's hole wall, where it differs from the outer layers'.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_via_hole_clearance_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_pth_hole_clearance_nm: Option<i64>,
    /// The stackup a board of this preset has when it gives none; the
    /// two-layer one of [`Preset::stackup`] when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stackup: Option<super::stackup::Stackup>,
    /// The assembly service's file form, if the fabricator has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly: Option<AssemblyForm>,
}

/// A drilled hole's kind, as the hole clearances tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoleKind {
    Via,
    Pth,
    Npth,
}

impl Preset {
    /// ryueda 7.A's rules (via spans and hole clearances), as `board drc`
    /// answers them for a board with a stackup of its own.
    pub fn layer_rules_json(&self) -> serde_json::Value {
        serde_json::json!({
            "blind_vias": self.blind_vias,
            "via_hole_clearance_nm": self.via_hole_clearance_nm,
            "pth_hole_clearance_nm": self.pth_hole_clearance_nm,
            "npth_hole_clearance_nm": self.npth_hole_clearance_nm,
        })
    }

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

impl Preset {
    /// The preset as its document: canonical JSON with its kind and
    /// format. A built-in's own stackup (which a constant can't hold) is
    /// written into it.
    pub fn document(&self) -> serde_json::Value {
        let full = Preset {
            stackup: self.stackup.clone().or_else(|| self.builtin_stackup()),
            ..self.clone()
        };
        crate::doc::to_value(&full, KIND, FORMAT)
    }

    /// A built-in's stackup, which its constant can't hold (a `Vec`).
    fn builtin_stackup(&self) -> Option<super::stackup::Stackup> {
        (self.stackup.is_none() && self.name == "jlc-4l").then(jlc_4l_stackup)
    }

    /// A preset from its document (a project's `presets/<id>.json`).
    pub fn from_document(v: &serde_json::Value, path: &str) -> crate::error::Result<Preset> {
        crate::doc::from_value(v.clone(), path, KIND, FORMAT)
    }

    /// The hole clearance of a hole of `kind` on an inner or outer layer.
    pub fn hole_clearance(&self, kind: HoleKind, inner: bool) -> i64 {
        match (kind, inner) {
            (HoleKind::Via, true) => self
                .inner_via_hole_clearance_nm
                .unwrap_or(self.via_hole_clearance_nm),
            (HoleKind::Via, false) => self.via_hole_clearance_nm,
            (HoleKind::Pth, true) => self
                .inner_pth_hole_clearance_nm
                .unwrap_or(self.pth_hole_clearance_nm),
            (HoleKind::Pth, false) => self.pth_hole_clearance_nm,
            (HoleKind::Npth, _) => self.npth_hole_clearance_nm,
        }
    }

    /// The stackup a board has when it gives none: the preset's own, else
    /// two copper layers round one FR-4 core filling the board's thickness.
    pub fn stackup(&self) -> super::stackup::Stackup {
        use super::stackup::{StackLayer, Stackup};
        if let Some(s) = self.stackup.clone().or_else(|| self.builtin_stackup()) {
            return s;
        }
        let cu = StackLayer::Copper {
            thickness_nm: self.copper_thickness_nm,
        };
        Stackup {
            layers: vec![
                cu.clone(),
                StackLayer::Dielectric {
                    thickness_nm: self.board_thickness_nm - 2 * self.copper_thickness_nm,
                    material: Some("FR-4".into()),
                    er_ppm: None,
                    loss_ppm: None,
                },
                cu,
            ],
        }
    }
}

/// JLCPCB's assembly files, read by the coxswain from JLC's help pages
/// on 2026-11-17:
///
/// - BOM, https://jlcpcb.com/help/article/bill-of-materials-for-pcb-assembly:
///   the required columns "Comment", "Designator", "Footprint", and the
///   part number as JLC's own toolkit writes it, "LCSC Part #" (its KiCad
///   guide spells it "JLCPCB Part #"; both appear). Several designators
///   share a row comma-separated ("R5,R6"), never as ranges.
/// - CPL, https://jlcpcb.com/help/article/pick-place-file-for-pcb-assembly:
///   "Designator", "Mid X", "Mid Y" ("Required units: Metric(mm)", written
///   `12.5mm` as JLC's sample does), "Layer" ("Top / Bottom") and
///   "Rotation" ("Positive values are counter clockwise"). The coxswain
///   read the page again on 2026-11-18 (ryueda 7.A): Layer is "the board
///   side where the component should be placed", so a bottom part's is
///   `Bottom`. No JLC page says whether a bottom part's rotation is
///   mirrored; ryueda writes the placement's, as seen from the top
///   (unconfirmed: see `docs/claude/ryueda-board.md`).
pub const JLC_ASSEMBLY: AssemblyForm = AssemblyForm {
    name: Cow::Borrowed("jlc"),
    supplier: Cow::Borrowed("LCSC"),
    bom: [
        Cow::Borrowed("Comment"),
        Cow::Borrowed("Designator"),
        Cow::Borrowed("Footprint"),
        Cow::Borrowed("LCSC Part #"),
    ],
    pos: [
        Cow::Borrowed("Designator"),
        Cow::Borrowed("Mid X"),
        Cow::Borrowed("Mid Y"),
        Cow::Borrowed("Layer"),
        Cow::Borrowed("Rotation"),
    ],
    length_suffix: Cow::Borrowed("mm"),
    top: Cow::Borrowed("Top"),
    bottom: Cow::Borrowed("Bottom"),
};

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
/// The hole clearances were read by the coxswain from the same page on
/// 2026-11-18 (ryueda 7.A): "Via hole to Track": "0.2mm"; "PTH to
/// Track": "0.35mm is recommended, minimum 0.28mm" (the preset takes the
/// minimum); "NPTH to Track": "0.2mm". Blind and buried vias are HDI
/// only: https://jlcpcb.com/pcb-fabrication/hdi-pcb gives HDI's layer
/// count as "4–24 layers", and the capabilities page lists blind (laser)
/// and buried vias under HDI, so this 1-2 layer process makes none.
///
/// ryueda's defaults (a 0.25 mm track, a 0.30 mm drill in a 0.60 mm via)
/// are its own choices and meet every rule above.
pub const JLC_2L: Preset = Preset {
    name: Cow::Borrowed("jlc-2l"),
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
    via_hole_clearance_nm: 200_000,
    pth_hole_clearance_nm: 280_000,
    npth_hole_clearance_nm: 200_000,
    blind_vias: false,
    inner_via_hole_clearance_nm: None,
    inner_pth_hole_clearance_nm: None,
    stackup: None,
    assembly: Some(JLC_ASSEMBLY),
};

/// JLCPCB's multilayer process at 1.6 mm with its standard 4-layer
/// stackup, read by the coxswain from
/// https://jlcpcb.com/capabilities/pcb-capabilities on 2026-11-19 (ryueda
/// 7.B):
///
/// - "Min. track width and spacing (1 oz)": "Multilayer: 0.09 / 0.09 mm
///   (3.5 / 3.5 mil)"; the page has no inner-layer row (inner copper is
///   0.5 oz by default). One clearance rule, 0.09 mm.
/// - Via: "Multilayer: 0.15 mm hole size / 0.25 mm via diameter" (0.1/0.2
///   mm is only for boards 1 mm or thinner with ENIG/OSP). PTH drill:
///   "Multilayer: 0.15 – 6.3 mm". "Min. Non-plated holes": "0.50mm".
/// - PTH annular ring, multilayer 1 oz: "absolute minimum 0.15 mm", "0.20
///   mm or above" recommended: the floor is 0.15 mm. A via's ring is its
///   least geometry's, (0.25 - 0.15) / 2 = 0.05 mm, as `jlc-2l`'s.
/// - "Via Hole-to-Hole Spacing": "0.2mm"; "Pad Hole-to-Hole Spacing":
///   "0.45mm". "Copper clearance from routed board edges: ≧0.2 mm".
/// - "Soldermask Expansion": "1:1"; "Keep at least 0.09 mm clearance
///   between soldermask openings and neighboring traces."
/// - Hole to track, every layer count: "Via hole to Track": "0.2mm";
///   "PTH to Track": "0.35mm is recommended, minimum 0.28mm"; "NPTH to
///   Track": "0.2mm". Inner layers: "Inner layer via hole to copper
///   clearance": "0.2mm"; "Inner layer PTH pad hole to copper clearance":
///   "0.3mm".
/// - Blind and buried vias are HDI's
///   (https://jlcpcb.com/pcb-fabrication/hdi-pcb, "4–24 layers"): this
///   standard process makes none.
///
/// The stackup is [`jlc_4l_stackup`]. ryueda's defaults (a 0.2 mm track, a
/// 0.30 mm drill in a 0.60 mm via) are its own and meet every rule above.
pub const JLC_4L: Preset = Preset {
    name: Cow::Borrowed("jlc-4l"),
    mask_expansion_nm: 0,
    paste_margin_nm: 0,
    default_track_width_nm: 200_000,
    default_via_drill_nm: 300_000,
    default_via_diameter_nm: 600_000,
    board_thickness_nm: 1_600_000,
    copper_thickness_nm: 35_000,
    track_width_min_nm: 90_000,
    clearance_nm: 90_000,
    drill_min_nm: 150_000,
    drill_max_nm: 6_300_000,
    npth_drill_min_nm: 500_000,
    via_diameter_min_nm: 250_000,
    annular_ring_min_nm: 150_000,
    via_annular_ring_min_nm: 50_000,
    hole_spacing_nm: 450_000,
    via_hole_spacing_nm: 200_000,
    edge_clearance_nm: 200_000,
    mask_trace_clearance_nm: 90_000,
    via_hole_clearance_nm: 200_000,
    pth_hole_clearance_nm: 280_000,
    npth_hole_clearance_nm: 200_000,
    blind_vias: false,
    inner_via_hole_clearance_nm: Some(200_000),
    inner_pth_hole_clearance_nm: Some(300_000),
    stackup: None,
    assembly: Some(JLC_ASSEMBLY),
};

/// JLC04161H-7628, read by the coxswain from https://jlcpcb.com/impedance
/// on 2026-11-19: top copper 0.035 mm, "Prepreg 7628" 0.21040 mm, inner
/// copper 0.0152 mm, core 1.065 mm ("1.1 mm H/HOZ with copper"), inner
/// copper 0.0152 mm, prepreg 0.21040 mm, bottom copper 0.035 mm (about 1.59
/// mm). 7628's εr is 4.4 by the page's prepreg table; the core's 4.6.
pub fn jlc_4l_stackup() -> super::stackup::Stackup {
    use super::stackup::{StackLayer, Stackup};
    let cu = |t: i64| StackLayer::Copper { thickness_nm: t };
    let di = |t: i64, m: &str, er: i64| StackLayer::Dielectric {
        thickness_nm: t,
        material: Some(m.to_owned()),
        er_ppm: Some(er),
        loss_ppm: None,
    };
    Stackup {
        layers: vec![
            cu(35_000),
            di(210_400, "7628", 4_400_000),
            cu(15_200),
            di(1_065_000, "core", 4_600_000),
            cu(15_200),
            di(210_400, "7628", 4_400_000),
            cu(35_000),
        ],
    }
}

/// The preset a new board takes.
pub const DEFAULT: &Preset = &JLC_2L;

/// Every preset this ryueda knows.
pub const ALL: [&Preset; 2] = [&JLC_2L, &JLC_4L];

/// The preset a board names; stage 5 knows one.
pub fn by_name(name: &str) -> Option<&'static Preset> {
    ALL.into_iter().find(|p| p.name == name)
}

/// The known presets' names, for messages.
pub fn names() -> String {
    ALL.iter()
        .map(|p| p.name.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The preset a board names: a built-in, else the project's own.
pub fn resolve(p: &crate::project::Project, name: &str) -> Option<std::sync::Arc<Preset>> {
    if let Some(b) = by_name(name) {
        return Some(std::sync::Arc::new(b.clone()));
    }
    let _ = p; // Task 5: the project's own presets
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_in_reads_back_from_its_document() {
        for p in ALL {
            let doc = p.document();
            assert_eq!(doc["kind"], KIND);
            assert_eq!(doc["format"], FORMAT);
            let back = Preset::from_document(&doc, "presets/x.json").unwrap();
            // A built-in's stackup is written into its document.
            assert_eq!(back.stackup(), p.stackup(), "{}", p.name);
            let bare = |x: &Preset| Preset {
                stackup: None,
                ..x.clone()
            };
            assert_eq!(bare(&back), bare(p), "{}", p.name);
        }
    }

    #[test]
    fn jlc_4l_has_four_layers_and_its_inner_hole_clearances() {
        let p = &JLC_4L;
        assert_eq!(by_name("jlc-4l"), Some(p));
        let s = p.stackup();
        assert_eq!(s.copper(), 4);
        assert!(crate::board::stackup::judge(&s).is_empty());
        let t = s.thickness();
        assert!((1_500_000..=1_700_000).contains(&t), "{t}");
        assert_eq!(p.hole_clearance(HoleKind::Pth, true), 300_000);
        assert_eq!(
            p.hole_clearance(HoleKind::Pth, false),
            p.pth_hole_clearance_nm
        );
        assert_eq!(p.hole_clearance(HoleKind::Via, true), 200_000);
        assert!(!p.blind_vias);
        assert!(p.default_track_width_nm >= p.track_width_min_nm);
        assert!(p.via_diameter_min_nm - p.drill_min_nm >= 2 * p.via_annular_ring_min_nm);
        assert_eq!(
            JLC_2L.hole_clearance(HoleKind::Pth, true),
            JLC_2L.pth_hole_clearance_nm
        );
        assert_eq!(
            JLC_4L.document()["stackup"]["layers"]
                .as_array()
                .unwrap()
                .len(),
            7
        );
        assert!(JLC_2L.document().get("stackup").is_none());
    }

    #[test]
    fn a_document_with_an_unknown_field_is_refused() {
        let mut doc = JLC_2L.document();
        doc["clearence_nm"] = serde_json::json!(1);
        let e = Preset::from_document(&doc, "presets/x.json").unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidDocument);
    }

    #[test]
    fn the_one_preset_is_found_by_name() {
        assert_eq!(by_name("jlc-2l"), Some(&JLC_2L));
        assert_eq!(by_name("jlc-4l"), Some(&JLC_4L));
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

    #[test]
    fn the_preset_has_an_assembly_form() {
        let f = JLC_2L
            .assembly
            .clone()
            .expect("jlc-2l has an assembly form");
        assert!(!f.name.is_empty() && f.name.chars().all(|c| c.is_ascii_lowercase()));
        assert!(f.bom.iter().all(|c| !c.is_empty() && !c.contains(',')));
        assert!(f.pos.iter().all(|c| !c.is_empty() && !c.contains(',')));
        assert!(!f.supplier.is_empty());
        assert!(!f.top.is_empty());
    }
}
