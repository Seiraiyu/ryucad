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
}

/// Read from https://jlcpcb.com/capabilities/pcb-capabilities on
/// 2026-11-17. Its "Soldermask Expansion" row says "1:1": "LDI equipment
/// upgraded in June 2025. Pad size:soldermask opening can be 1:1", so the
/// mask opening is the pad, an expansion of 0. The page gives a 0.09 mm
/// minimum clearance between soldermask openings and neighbouring traces
/// (stage 5.B's rule) and no paste margin, so paste is the pad too.
pub const JLC_2L: Preset = Preset {
    name: "jlc-2l",
    mask_expansion_nm: 0,
    paste_margin_nm: 0,
    default_track_width_nm: 250_000,
    default_via_drill_nm: 300_000,
    default_via_diameter_nm: 600_000,
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
}
