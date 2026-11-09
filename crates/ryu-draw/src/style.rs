//! Colours, line kinds and their widths and dash patterns by standard.
//!
//! ISO 128-2 draws in line group 0.5: thick 0.5 mm, thin 0.25 mm, each
//! pattern's lengths a multiple of its width `d` (dashed 12d on, 3d off;
//! chain 24d, 3d, 0.5d, 3d; phantom 24d, 3d, 0.5d, 3d, 0.5d, 3d). ASME
//! Y14.2 draws thick 0.6 mm and thin 0.3 mm, its patterns in millimetres
//! (hidden 3 on, 1.5 off; centre 20, 1.5, 3, 1.5; phantom 20, 1.5, 3,
//! 1.5, 3, 1.5). Lengths are millimetres on the sheet.

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Colour(pub u8, pub u8, pub u8);

impl Colour {
    pub const BLACK: Colour = Colour(0, 0, 0);
    pub const WHITE: Colour = Colour(255, 255, 255);

    /// From `#rrggbb` text; a missing or bad pair reads 0, as ryueda's
    /// writer always read them.
    pub fn hex(text: &str) -> Colour {
        let c = |i: usize| {
            text.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .unwrap_or(0)
        };
        Colour(c(1), c(3), c(5))
    }

    /// As `#rrggbb`.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// Each channel over 255, as PDF reads colours.
    pub fn unit(self) -> (f32, f32, f32) {
        let f = |v: u8| f32::from(v) / 255.0;
        (f(self.0), f(self.1), f(self.2))
    }
}

/// Which drafting standard a drawing follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Standard {
    Asme,
    Iso,
}

/// What a line shows, which decides its width and pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    /// Visible outlines and edges: thick, continuous.
    Visible,
    /// Hidden outlines and edges: thin, dashed.
    Hidden,
    /// Thin and continuous: tangent edges, dimension and extension lines,
    /// leaders, hatching.
    Thin,
    /// Thin long dash and two dots: tangent edges drawn as phantoms.
    Phantom,
    /// Thin long dash and dot: centre lines.
    Centre,
    /// The sheet's border and the title block's frame: thick.
    Border,
}

/// How a path is stroked.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub colour: Colour,
    /// In the drawing's unit (the tables give millimetres).
    pub width: f64,
    /// On and off lengths in the drawing's unit, from the path's start;
    /// empty for continuous.
    pub dash: Vec<f64>,
}

impl Standard {
    /// The thick and thin line widths, in millimetres.
    pub fn widths(self) -> (f64, f64) {
        match self {
            Standard::Asme => (0.6, 0.3),
            Standard::Iso => (0.5, 0.25),
        }
    }

    /// How a line of `kind` is stroked in black.
    pub fn stroke(self, kind: LineKind) -> Stroke {
        let (thick, thin) = self.widths();
        let (width, dash) = match (self, kind) {
            (_, LineKind::Visible | LineKind::Border) => (thick, vec![]),
            (_, LineKind::Thin) => (thin, vec![]),
            (Standard::Asme, LineKind::Hidden) => (thin, vec![3.0, 1.5]),
            (Standard::Asme, LineKind::Centre) => (thin, vec![20.0, 1.5, 3.0, 1.5]),
            (Standard::Asme, LineKind::Phantom) => (thin, vec![20.0, 1.5, 3.0, 1.5, 3.0, 1.5]),
            (Standard::Iso, LineKind::Hidden) => (thin, vec![12.0 * thin, 3.0 * thin]),
            (Standard::Iso, LineKind::Centre) => {
                (thin, vec![24.0 * thin, 3.0 * thin, 0.5 * thin, 3.0 * thin])
            }
            (Standard::Iso, LineKind::Phantom) => (
                thin,
                vec![
                    24.0 * thin,
                    3.0 * thin,
                    0.5 * thin,
                    3.0 * thin,
                    0.5 * thin,
                    3.0 * thin,
                ],
            ),
        };
        Stroke {
            colour: Colour::BLACK,
            width,
            dash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_read_and_write_hex() {
        assert_eq!(Colour::hex("#1b6e3a"), Colour(0x1b, 0x6e, 0x3a));
        assert_eq!(Colour::hex("#1b6e3a").to_hex(), "#1b6e3a");
        assert_eq!(Colour::hex("#zz"), Colour(0, 0, 0));
    }

    #[test]
    fn iso_patterns_follow_their_width_and_asme_its_millimetres() {
        let h = Standard::Iso.stroke(LineKind::Hidden);
        assert_eq!((h.width, h.dash.clone()), (0.25, vec![3.0, 0.75]));
        let v = Standard::Asme.stroke(LineKind::Visible);
        assert_eq!((v.width, v.dash.len()), (0.6, 0));
        assert_eq!(Standard::Asme.stroke(LineKind::Hidden).dash, vec![3.0, 1.5]);
        assert_eq!(Standard::Iso.stroke(LineKind::Phantom).dash.len(), 6);
    }
}
