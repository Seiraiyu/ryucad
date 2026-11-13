//! The colours every ryu product paints participants and highlights in,
//! in its windows and its renders alike (moved from `ryucad::render` by
//! ryueda 3.C, unchanged).

use crate::live::Style;

/// sRGB, 0 to 1.
pub type Rgb = [f32; 3];

/// Highlight colours: about to change, just changed, pointed out.
pub const PENDING: Rgb = [1.0, 0.70, 0.22];
pub const CHANGED: Rgb = [0.33, 0.78, 0.42];
pub const NOTE: Rgb = [0.30, 0.58, 1.0];

/// Participants' colours, in turn: apart from each other for colour-blind
/// eyes too. A participant's marks and pins are drawn in theirs, in the
/// window and in renders alike.
pub const PARTICIPANTS: [Rgb; 6] = [
    [0.79, 0.34, 0.73],
    [0.97, 0.59, 0.34],
    [0.35, 0.57, 0.07],
    [0.54, 0.47, 0.99],
    [0.90, 0.35, 0.40],
    [0.28, 0.63, 0.61],
];

/// A participant's colour: the same everywhere, from their ID or name.
pub fn participant(id: &str) -> Rgb {
    let n: usize = id.bytes().map(usize::from).sum();
    PARTICIPANTS[n % PARTICIPANTS.len()]
}

/// A highlight's colour by style.
pub fn highlight(style: Style) -> Rgb {
    match style {
        Style::Pending => PENDING,
        Style::Changed => CHANGED,
        Style::Note => NOTE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A participant's colour is a function of their id alone.
    #[test]
    fn a_participant_s_colour_is_one_of_six_by_id() {
        assert_eq!(participant("p-1"), participant("p-1"));
        assert!(PARTICIPANTS.contains(&participant("agent")));
        assert_eq!(highlight(Style::Changed), CHANGED);
    }
}
