//! The window's look: the `ui.style` preference (a predefined palette,
//! corners and spacing for light and dark), and the colours the window
//! paints entities in. The render theme's highlight colours come from
//! `ryucad render`, so a highlight looks the same in the window and in an
//! agent's render.

use eframe::egui::{self, Color32, CornerRadius, Margin, Shadow, Stroke};
use ryucad::live::Style;

pub type Rgb = [f32; 3];

/// This window's selection.
pub const SELECTED: Rgb = [0.10, 0.75, 0.90];
/// What the mouse is over.
pub const HOVER: Rgb = [0.60, 0.88, 1.0];
/// The feature chosen in the tree.
pub const CHOSEN: Rgb = [0.70, 0.80, 0.95];

/// A compare's colours, the same as `ryucad render --compare`'s: faces
/// only the working files have, faces on both sides whose geometry
/// differs, faces only the other side has (in its ghost), and the rest of
/// the ghost.
pub use ryucad::render::{
    COMPARE_CHANGED as CHANGED, COMPARE_GHOST as GHOST, COMPARE_NEW as NEW,
    COMPARE_REMOVED as REMOVED,
};

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

/// The `ui.style` preference: which predefined look the window takes.
/// Every style has the same layout; only colours, corners and spacing
/// differ.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Look {
    /// Warm paper neutrals, one orange accent, square corners, dense.
    #[default]
    Drafting,
    /// Cool greys, a blue accent.
    Graphite,
    /// White and roomier, an indigo accent, round corners.
    Studio,
}

/// A style's colours in one mode, as `0xRRGGBB`.
struct Palette {
    app: u32,
    /// The 3D view's; only light ones are used (see `viewport_background`).
    viewport: u32,
    raised: u32,
    frame: u32,
    frame_hover: u32,
    frame_active: u32,
    border: u32,
    border_strong: u32,
    text: u32,
    text_weak: u32,
    accent: u32,
    select_bg: u32,
    select_fg: u32,
    warning: u32,
    danger: u32,
}

/// A style's sizes, in points.
struct Metrics {
    window_rounding: u8,
    frame_rounding: u8,
    frame_padding: [f32; 2],
    item_spacing: [f32; 2],
    row_height: f32,
}

impl Look {
    pub fn parse(s: &str) -> Look {
        match s {
            "graphite" => Look::Graphite,
            "studio" => Look::Studio,
            _ => Look::Drafting,
        }
    }

    fn palette(self, dark: bool) -> Palette {
        match (self, dark) {
            (Look::Drafting, false) => Palette {
                app: 0xf7f6f2,
                viewport: 0xefede8,
                raised: 0xfdfcfa,
                frame: 0xebe9e3,
                frame_hover: 0xe2dfd8,
                frame_active: 0xd8d4cb,
                border: 0xe2dfd7,
                border_strong: 0xcdc9bf,
                text: 0x1c1b18,
                text_weak: 0x8d897f,
                accent: 0xd4530c,
                select_bg: 0xfae2d2,
                select_fg: 0x8c3504,
                warning: 0xa86f08,
                danger: 0xc93a3a,
            },
            (Look::Drafting, true) => Palette {
                app: 0x141412,
                viewport: 0x2c2b28,
                raised: 0x1d1c1a,
                frame: 0x262522,
                frame_hover: 0x2f2e2a,
                frame_active: 0x393833,
                border: 0x292825,
                border_strong: 0x3b3a35,
                text: 0xebe8e1,
                text_weak: 0x6d695f,
                accent: 0xff7a2f,
                select_bg: 0x3d2414,
                select_fg: 0xffc7a3,
                warning: 0xe3a63a,
                danger: 0xf0706b,
            },
            (Look::Graphite, false) => Palette {
                app: 0xfafbfc,
                viewport: 0xf1f2f5,
                raised: 0xffffff,
                frame: 0xeceef1,
                frame_hover: 0xe2e5e9,
                frame_active: 0xd6dae0,
                border: 0xe3e6ea,
                border_strong: 0xc8cdd4,
                text: 0x1b1d21,
                text_weak: 0x8a919b,
                accent: 0x0c82bf,
                select_bg: 0xdcedf8,
                select_fg: 0x075a84,
                warning: 0xa86f08,
                danger: 0xcf3f3f,
            },
            (Look::Graphite, true) => Palette {
                app: 0x18191c,
                viewport: 0x2f3238,
                raised: 0x212327,
                frame: 0x2a2c31,
                frame_hover: 0x33363c,
                frame_active: 0x3d4047,
                border: 0x2b2d32,
                border_strong: 0x3b3e45,
                text: 0xe4e6e9,
                text_weak: 0x666c75,
                accent: 0x38a3dc,
                select_bg: 0x133a50,
                select_fg: 0xbfe3ff,
                warning: 0xe3a63a,
                danger: 0xf0706b,
            },
            (Look::Studio, false) => Palette {
                app: 0xffffff,
                viewport: 0xf3f4f6,
                raised: 0xffffff,
                frame: 0xf0f1f4,
                frame_hover: 0xe7e9ed,
                frame_active: 0xdde0e5,
                border: 0xe6e8ec,
                border_strong: 0xd3d7dd,
                text: 0x15171a,
                text_weak: 0x8f959e,
                accent: 0x3557f0,
                select_bg: 0x3557f0,
                select_fg: 0xffffff,
                warning: 0xa86f08,
                danger: 0xcf3f3f,
            },
            (Look::Studio, true) => Palette {
                app: 0x18191c,
                viewport: 0x25272b,
                raised: 0x1f2024,
                frame: 0x26282c,
                frame_hover: 0x2e3035,
                frame_active: 0x373a40,
                border: 0x26282c,
                border_strong: 0x34373d,
                text: 0xecedef,
                text_weak: 0x676c74,
                accent: 0x6f88ff,
                select_bg: 0x6f88ff,
                select_fg: 0x0a0f2a,
                warning: 0xe3a63a,
                danger: 0xf0706b,
            },
        }
    }

    fn metrics(self) -> Metrics {
        match self {
            Look::Drafting => Metrics {
                window_rounding: 2,
                frame_rounding: 2,
                frame_padding: [8.0, 2.0],
                item_spacing: [8.0, 2.0],
                row_height: 22.0,
            },
            Look::Graphite => Metrics {
                window_rounding: 6,
                frame_rounding: 4,
                frame_padding: [8.0, 3.0],
                item_spacing: [8.0, 4.0],
                row_height: 24.0,
            },
            Look::Studio => Metrics {
                window_rounding: 10,
                frame_rounding: 6,
                frame_padding: [10.0, 5.0],
                item_spacing: [8.0, 4.0],
                row_height: 26.0,
            },
        }
    }

    /// The egui style for one mode: egui's own, with this look's colours
    /// and sizes. Depth comes from a surface step and a border, never a
    /// shadow.
    pub fn style(self, dark: bool) -> egui::Style {
        let p = self.palette(dark);
        let m = self.metrics();
        let c = |x: u32| Color32::from_rgb((x >> 16) as u8, (x >> 8) as u8, x as u8);
        let mut style = egui::Style {
            visuals: if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            },
            ..egui::Style::default()
        };
        let v = &mut style.visuals;
        v.panel_fill = c(p.app);
        v.window_fill = c(p.raised);
        v.window_stroke = Stroke::new(1.0, c(p.border_strong));
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.window_corner_radius = CornerRadius::same(m.window_rounding);
        v.menu_corner_radius = CornerRadius::same(m.window_rounding);
        v.faint_bg_color = c(p.frame);
        v.extreme_bg_color = c(p.frame);
        v.code_bg_color = c(p.frame);
        v.weak_text_color = Some(c(p.text_weak));
        v.hyperlink_color = c(p.accent);
        v.warn_fg_color = c(p.warning);
        v.error_fg_color = c(p.danger);
        v.selection.bg_fill = c(p.select_bg);
        v.selection.stroke = Stroke::new(1.0, c(p.select_fg));
        v.text_cursor.stroke.color = c(p.accent);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = c(p.app);
        w.noninteractive.weak_bg_fill = c(p.app);
        w.noninteractive.bg_stroke = Stroke::new(1.0, c(p.border));
        w.noninteractive.fg_stroke.color = c(p.text);
        for (state, fill) in [
            (&mut w.inactive, p.frame),
            (&mut w.hovered, p.frame_hover),
            (&mut w.active, p.frame_active),
            (&mut w.open, p.frame_active),
        ] {
            state.bg_fill = c(fill);
            state.weak_bg_fill = c(fill);
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke.color = c(p.text);
            state.corner_radius = CornerRadius::same(m.frame_rounding);
            state.expansion = 0.0;
        }
        // Keyboard focus and hover still show: an accent hairline.
        w.hovered.bg_stroke = Stroke::new(1.0, c(p.border_strong));
        w.active.bg_stroke = Stroke::new(1.0, c(p.accent));
        w.noninteractive.corner_radius = CornerRadius::same(m.frame_rounding);
        let s = &mut style.spacing;
        s.item_spacing = m.item_spacing.into();
        s.button_padding = m.frame_padding.into();
        s.interact_size.y = m.row_height;
        s.window_margin = Margin::same(10);
        s.menu_margin = Margin::same(6);
        s.indent = 20.0;
        s.scroll.bar_width = 10.0;
        style
    }

    /// The 3D view's background, sRGB: the style's in a light theme; in a
    /// dark one a grey shared by every style, since the styles' own dark
    /// viewports sit too close to edges for silhouettes to show.
    pub fn viewport_background(self, dark: bool) -> Rgb {
        if dark {
            return [0.22, 0.23, 0.26];
        }
        let x = self.palette(false).viewport;
        [x >> 16, x >> 8, x].map(|b| (b & 0xff) as f32 / 255.0)
    }
}

/// Gives `ctx` a look, in both modes: `ui.theme` picks between them.
pub fn apply(ctx: &egui::Context, look: Look) {
    ctx.set_style_of(egui::Theme::Light, look.style(false));
    ctx.set_style_of(egui::Theme::Dark, look.style(true));
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOKS: [Look; 3] = [Look::Drafting, Look::Graphite, Look::Studio];

    #[test]
    fn backgrounds_keep_silhouettes_visible() {
        let lum = |c: Rgb| (c[0] + c[1] + c[2]) / 3.0;
        for look in LOOKS {
            let (l, d) = (
                look.viewport_background(false),
                look.viewport_background(true),
            );
            assert!(lum(l) > 0.85, "{look:?}");
            assert!(lum(d) < 0.3, "{look:?}");
            assert!(lum(d) > lum(ryucad::render::EDGE) + 0.05, "{look:?}");
        }
    }

    #[test]
    fn every_style_the_preference_allows_is_a_look() {
        assert_eq!(Look::parse("graphite"), Look::Graphite);
        assert_eq!(Look::parse("studio"), Look::Studio);
        assert_eq!(Look::parse("drafting"), Look::Drafting);
        // Until settings arrive, or from a newer daemon: the default.
        assert_eq!(Look::parse(""), Look::default());
    }

    #[test]
    fn modes_match_their_theme() {
        for look in LOOKS {
            assert!(look.style(true).visuals.dark_mode);
            assert!(!look.style(false).visuals.dark_mode);
            assert_eq!(look.style(false).visuals.window_shadow, Shadow::NONE);
        }
    }
}
