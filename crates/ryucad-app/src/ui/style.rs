//! Colours the window paints entities in. The render theme's highlight
//! colours come from `ryucad render`, so a highlight looks the same in the
//! window and in an agent's render.

use eframe::egui::Color32;
use ryucad::live::Style;

pub type Rgb = [f32; 3];

/// This window's selection.
pub const SELECTED: Rgb = [0.10, 0.75, 0.90];
/// What the mouse is over.
pub const HOVER: Rgb = [0.60, 0.88, 1.0];
/// The feature chosen in the tree.
pub const CHOSEN: Rgb = [0.70, 0.80, 0.95];

/// Colours for other participants, in turn.
const PALETTE: [Rgb; 6] = [
    [0.85, 0.37, 0.80],
    [0.95, 0.55, 0.25],
    [0.45, 0.70, 0.30],
    [0.60, 0.50, 0.95],
    [0.90, 0.35, 0.40],
    [0.30, 0.65, 0.65],
];

/// A participant's colour: the same in every window, from their ID.
pub fn participant(id: &str) -> Rgb {
    let n: usize = id.bytes().map(usize::from).sum();
    PALETTE[n % PALETTE.len()]
}

/// A highlight's colour by style.
pub fn highlight(style: Style) -> Rgb {
    match style {
        Style::Pending => ryucad::render::PENDING,
        Style::Changed => ryucad::render::CHANGED,
        Style::Note => ryucad::render::NOTE,
    }
}

pub fn color32(c: Rgb) -> Color32 {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}
