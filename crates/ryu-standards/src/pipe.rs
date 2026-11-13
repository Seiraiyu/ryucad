//! Pipe threads: designations, threads per inch, and the taper.
//!
//! Sources: facts only. The designations and TPI of NPT, NPTF and NPSM
//! (ASME B1.20.1 and B1.20.3 designations), and of ISO 7-1 (R, Rc, Rp)
//! and ISO 228-1 (G), are common knowledge, and so is the 1:16 taper on
//! the diameter of NPT, NPTF, R and Rc. Their diameters, gauge lengths
//! and tap drills are not built in: a project's `hole_standards.json`
//! supplies them.

/// A nominal pipe size and its threads per inch, in tenths (11.5 is 115).
#[derive(Debug, PartialEq, Eq)]
pub struct Size {
    pub name: &'static str,
    pub tpi10: u32,
}

macro_rules! p {
    ($name:expr, $t:expr) => {
        Size {
            name: $name,
            tpi10: $t,
        }
    };
}

/// NPT, NPTF and NPSM: one table.
pub const NATIONAL: &[Size] = &[
    p!("1/16", 270),
    p!("1/8", 270),
    p!("1/4", 180),
    p!("3/8", 180),
    p!("1/2", 140),
    p!("3/4", 140),
    p!("1", 115),
    p!("1-1/4", 115),
    p!("1-1/2", 115),
    p!("2", 115),
    p!("2-1/2", 80),
    p!("3", 80),
    p!("3-1/2", 80),
    p!("4", 80),
    p!("5", 80),
    p!("6", 80),
];

/// ISO 7-1 and ISO 228-1 (British pipe threads).
pub const BRITISH: &[Size] = &[
    p!("1/16", 280),
    p!("1/8", 280),
    p!("1/4", 190),
    p!("3/8", 190),
    p!("1/2", 140),
    p!("5/8", 140),
    p!("3/4", 140),
    p!("7/8", 140),
    p!("1", 110),
    p!("1-1/4", 110),
    p!("1-1/2", 110),
    p!("2", 110),
    p!("2-1/2", 110),
    p!("3", 110),
    p!("4", 110),
    p!("5", 110),
    p!("6", 110),
];

/// Sizes 5/8 and 7/8 are G only (ISO 228-1), not R, Rc or Rp.
pub fn british_tapered(name: &str) -> bool {
    !matches!(name, "5/8" | "7/8")
}
