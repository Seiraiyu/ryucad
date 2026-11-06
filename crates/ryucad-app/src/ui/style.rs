//! The window's look: the `ui.style` preference (a predefined palette,
//! corners and spacing for light and dark), and the colours the window
//! paints entities in. The render theme's highlight colours come from
//! `ryucad render`, so a highlight looks the same in the window and in an
//! agent's render.

use eframe::egui::{self, Color32, CornerRadius, Margin, Shadow, Stroke};
use ryucad::live::Style;

pub type Rgb = [f32; 3];

/// This window's selection. It, the hover and the chosen feature's
/// faces stay apart from each other, from every colour below and from the
/// model and the viewport, for colour-blind eyes too (the
/// colour-blindness test).
pub const SELECTED: Rgb = [0.50, 0.67, 1.0];
/// What the mouse is over.
pub const HOVER: Rgb = [0.52, 0.86, 1.0];
/// The feature chosen in the tree.
pub const CHOSEN: Rgb = [0.62, 0.68, 0.79];
/// A split chosen in the tree: the side behind its plane, which its
/// target keeps, and the side in front, the solid it founds.
pub const SPLIT_BEHIND: Rgb = [0.86, 0.62, 0.30];
pub const SPLIT_FRONT: Rgb = [0.55, 0.40, 0.80];
/// A frame's x, y and z axes, as CAD draws them: red, green, blue.
pub const AXES: [Rgb; 3] = [[0.85, 0.20, 0.20], [0.20, 0.65, 0.25], [0.20, 0.40, 0.90]];

/// A compare's colours, the same as `ryucad render --compare`'s: faces
/// only the working files have, faces on both sides whose geometry
/// differs, faces only the other side has (in its ghost), and the rest of
/// the ghost.
pub use ryucad::render::{
    COMPARE_CHANGED as CHANGED, COMPARE_GHOST as GHOST, COMPARE_NEW as NEW,
    COMPARE_REMOVED as REMOVED,
};

/// Colours for other participants, in turn: apart from each other for
/// colour-blind eyes too.
pub const PALETTE: [Rgb; 6] = [
    [0.79, 0.34, 0.73],
    [0.97, 0.59, 0.34],
    [0.35, 0.57, 0.07],
    [0.54, 0.47, 0.99],
    [0.90, 0.35, 0.40],
    [0.28, 0.63, 0.61],
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

/// The `ui.contrast` preference: the look's own colours, or one
/// high-contrast palette per theme over the look's metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Contrast {
    #[default]
    Normal,
    High,
}

impl Contrast {
    pub fn parse(s: &str) -> Contrast {
        match s {
            "high" => Contrast::High,
            _ => Contrast::Normal,
        }
    }
}

/// High contrast in a light theme: black on white, a dark blue accent
/// (`design/app/app.css`).
const HIGH_LIGHT: Palette = Palette {
    app: 0xffffff,
    viewport: 0xf2f2f2,
    raised: 0xffffff,
    frame: 0xffffff,
    frame_hover: 0xe6e6e6,
    frame_active: 0xd0d0d0,
    border: 0x000000,
    border_strong: 0x000000,
    text: 0x000000,
    text_weak: 0x333333,
    accent: 0x0037a6,
    select_bg: 0x0037a6,
    select_fg: 0xffffff,
    warning: 0x7a4d00,
    danger: 0xb00000,
    ok: 0x006b21,
    info: 0x0037a6,
    focus: 0x0037a6,
};

/// High contrast in a dark theme: white on black, a yellow accent.
const HIGH_DARK: Palette = Palette {
    app: 0x000000,
    viewport: 0x2b2b2b,
    raised: 0x000000,
    frame: 0x000000,
    frame_hover: 0x262626,
    frame_active: 0x404040,
    border: 0xffffff,
    border_strong: 0xffffff,
    text: 0xffffff,
    text_weak: 0xd6d6d6,
    accent: 0xffd400,
    select_bg: 0xffd400,
    select_fg: 0x000000,
    warning: 0xffb000,
    danger: 0xff6b6b,
    ok: 0x4cff7a,
    info: 0x7ab8ff,
    focus: 0xffd400,
};

/// A style's colours in one mode, as `0xRRGGBB`: the mockups' palettes
/// (`design/app/app.css`, plan 11.A), which pass WCAG AA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub app: u32,
    /// The 3D view's; only light ones are used (see `viewport_background`).
    pub viewport: u32,
    pub raised: u32,
    pub frame: u32,
    pub frame_hover: u32,
    pub frame_active: u32,
    pub border: u32,
    pub border_strong: u32,
    pub text: u32,
    pub text_weak: u32,
    pub accent: u32,
    pub select_bg: u32,
    pub select_fg: u32,
    /// Stale, redundant, modified: amber.
    pub warning: u32,
    /// Failed, conflicting: red.
    pub danger: u32,
    /// Released, added: green.
    pub ok: u32,
    /// Under-defined sketches, proposals: blue.
    pub info: u32,
    /// The keyboard focus ring.
    pub focus: u32,
}

/// A palette colour as egui's.
pub fn rgb(x: u32) -> Color32 {
    Color32::from_rgb((x >> 16) as u8, (x >> 8) as u8, x as u8)
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
    /// Its name as the status line says it: `Drafting`.
    pub fn title(self) -> &'static str {
        match self {
            Look::Drafting => "Drafting",
            Look::Graphite => "Graphite",
            Look::Studio => "Studio",
        }
    }

    pub fn parse(s: &str) -> Look {
        match s {
            "graphite" => Look::Graphite,
            "studio" => Look::Studio,
            _ => Look::Drafting,
        }
    }

    /// Its colours in one mode and contrast: high contrast is one palette
    /// per theme, whatever the look.
    pub fn colours(self, dark: bool, contrast: Contrast) -> Palette {
        match (contrast, dark) {
            (Contrast::Normal, _) => self.palette(dark),
            (Contrast::High, false) => HIGH_LIGHT,
            (Contrast::High, true) => HIGH_DARK,
        }
    }

    /// Its own colours in one mode (normal contrast).
    pub fn palette(self, dark: bool) -> Palette {
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
                text_weak: 0x6a675f,
                accent: 0xd4530c,
                select_bg: 0xfae2d2,
                select_fg: 0x8c3504,
                warning: 0x97650a,
                danger: 0xc93a3a,
                ok: 0x1f7a3d,
                info: 0x1f5fc8,
                focus: 0xd4530c,
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
                text_weak: 0x908d83,
                accent: 0xff7a2f,
                select_bg: 0x3d2414,
                select_fg: 0xffc7a3,
                warning: 0xe3a63a,
                danger: 0xf0706b,
                ok: 0x5cc77f,
                info: 0x6ea0ff,
                focus: 0xff7a2f,
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
                text_weak: 0x656b73,
                accent: 0x0c82bf,
                select_bg: 0xdcedf8,
                select_fg: 0x075a84,
                warning: 0x9b680a,
                danger: 0xcf3f3f,
                ok: 0x1f7a3d,
                info: 0x1f5fc8,
                focus: 0x0c82bf,
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
                text_weak: 0x91959c,
                accent: 0x38a3dc,
                select_bg: 0x133a50,
                select_fg: 0xbfe3ff,
                warning: 0xe3a63a,
                danger: 0xf0706b,
                ok: 0x5cc77f,
                info: 0x6ea0ff,
                focus: 0x38a3dc,
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
                text_weak: 0x686d74,
                accent: 0x3557f0,
                select_bg: 0x3557f0,
                select_fg: 0xffffff,
                warning: 0x9f6a09,
                danger: 0xcf3f3f,
                ok: 0x1f7a3d,
                info: 0x1f5fc8,
                focus: 0x3557f0,
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
                text_weak: 0x8c9096,
                accent: 0x6f88ff,
                select_bg: 0x6f88ff,
                select_fg: 0x0a0f2a,
                warning: 0xe3a63a,
                danger: 0xf0706b,
                ok: 0x5cc77f,
                info: 0x6ea0ff,
                focus: 0x6f88ff,
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
        self.style_with(dark, Contrast::Normal)
    }

    /// [`Look::style`] in a contrast: high contrast keeps the look's
    /// sizes, and outlines every control, since its frames are the
    /// background's colour.
    pub fn style_with(self, dark: bool, contrast: Contrast) -> egui::Style {
        let p = self.colours(dark, contrast);
        let m = self.metrics();
        let c = rgb;
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
        if contrast == Contrast::High {
            w.inactive.bg_stroke = Stroke::new(1.0, c(p.border_strong));
            w.hovered.bg_stroke = Stroke::new(1.0, c(p.focus));
            w.open.bg_stroke = Stroke::new(1.0, c(p.focus));
        }
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
    /// High contrast's light viewport is its palette's; its dark one is
    /// the shared grey, which the mockup's darker one would leave a well
    /// defined sketch's near-black lines too close to (see the
    /// colour-blindness test).
    pub fn viewport_background(self, dark: bool, contrast: Contrast) -> Rgb {
        if dark {
            return DARK_VIEWPORT;
        }
        let x = self.colours(false, contrast).viewport;
        [x >> 16, x >> 8, x].map(|b| (b & 0xff) as f32 / 255.0)
    }
}

/// The 3D view's background in every dark theme.
pub const DARK_VIEWPORT: Rgb = [0.22, 0.23, 0.26];

/// Gives `ctx` a look in a contrast, in both modes: `ui.theme` picks
/// between them.
pub fn apply(ctx: &egui::Context, look: Look, contrast: Contrast) {
    ctx.set_style_of(egui::Theme::Light, look.style_with(false, contrast));
    ctx.set_style_of(egui::Theme::Dark, look.style_with(true, contrast));
}

#[cfg(test)]
mod vision;

#[cfg(test)]
mod tests {
    use super::*;

    const LOOKS: [Look; 3] = [Look::Drafting, Look::Graphite, Look::Studio];
    const CONTRASTS: [Contrast; 2] = [Contrast::Normal, Contrast::High];

    #[test]
    fn backgrounds_keep_silhouettes_visible() {
        let lum = |c: Rgb| (c[0] + c[1] + c[2]) / 3.0;
        for look in LOOKS {
            let (l, d) = (
                look.viewport_background(false, Contrast::Normal),
                look.viewport_background(true, Contrast::Normal),
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

    /// Each look's palette, and high contrast's, as the approved mockups'
    /// stylesheet has them.
    #[test]
    fn palettes_are_the_mockups() {
        let css = include_str!("../../../../design/app/app.css");
        let blocks = LOOKS
            .iter()
            .flat_map(|l| [(*l, false, Contrast::Normal), (*l, true, Contrast::Normal)])
            .chain([
                (Look::Drafting, false, Contrast::High),
                (Look::Drafting, true, Contrast::High),
            ]);
        for (look, dark, contrast) in blocks {
            {
                let name = format!("{look:?}").to_lowercase();
                let theme = if dark { "dark" } else { "light" };
                let head = match contrast {
                    Contrast::Normal => format!(
                        "[data-look=\"{name}\"][data-theme=\"{theme}\"][data-contrast=\"normal\"] {{"
                    ),
                    // One block per theme, whatever the look.
                    Contrast::High => {
                        format!("[data-look][data-theme=\"{theme}\"][data-contrast=\"high\"] {{")
                    }
                };
                let at = css.find(&head).expect("a block per look and theme");
                let block = &css[at..at + css[at..].find('}').unwrap()];
                let token = |t: &str| {
                    let i = block.find(&format!("--{t}: #")).expect(t) + t.len() + 5;
                    u32::from_str_radix(&block[i..i + 6], 16).unwrap()
                };
                let p = look.colours(dark, contrast);
                for (t, v) in [
                    ("app", p.app),
                    ("raised", p.raised),
                    ("frame", p.frame),
                    ("frame-hover", p.frame_hover),
                    ("border", p.border),
                    ("border-strong", p.border_strong),
                    ("text", p.text),
                    ("text-weak", p.text_weak),
                    ("accent", p.accent),
                    ("select-bg", p.select_bg),
                    ("select-fg", p.select_fg),
                    ("focus", p.focus),
                    ("ok", p.ok),
                    ("warn", p.warning),
                    ("danger", p.danger),
                    ("info", p.info),
                ] {
                    assert_eq!(v, token(t), "{name} {theme} {contrast:?} --{t}");
                }
                // Only light viewports are used (see `viewport_background`).
                if !dark {
                    assert_eq!(
                        p.viewport,
                        token("viewport"),
                        "{name} {contrast:?} --viewport"
                    );
                }
            }
        }
    }

    /// WCAG 2.1's contrast ratio of two colours.
    fn contrast(a: u32, b: u32) -> f32 {
        let lum = |x: u32| {
            let ch = |c: u32| {
                let c = (c & 0xff) as f32 / 255.0;
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(x >> 16) + 0.7152 * ch(x >> 8) + 0.0722 * ch(x)
        };
        let (la, lb) = (lum(a), lum(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// `design/app/contrast.py`'s rules, on the palettes the window uses,
    /// high contrast's too; and the keyboard's ring at least 3:1 against
    /// what it borders: a tile's surfaces and the viewport.
    #[test]
    fn palettes_pass_wcag_aa() {
        for (look, level) in LOOKS.iter().flat_map(|l| CONTRASTS.map(|c| (*l, c))) {
            for dark in [false, true] {
                let p = look.colours(dark, level);
                let v = look.viewport_background(dark, level);
                let viewport = v
                    .iter()
                    .fold(0, |x, c| (x << 8) | (c * 255.0).round() as u32);
                let rules: [(&[u32], &[u32], f32); 5] = [
                    (&[p.text, p.text_weak], &[p.app, p.raised, p.frame], 4.5),
                    (
                        &[p.danger, p.info, p.ok, p.warning],
                        &[p.app, p.raised],
                        4.5,
                    ),
                    (&[p.accent, p.focus], &[p.app, p.raised], 3.0),
                    (&[p.select_fg], &[p.select_bg], 4.5),
                    (&[p.focus], &[p.app, p.raised, viewport], 3.0),
                ];
                for (fgs, bgs, need) in rules {
                    for &fg in fgs {
                        for &bg in bgs {
                            let r = contrast(fg, bg);
                            assert!(
                                r >= need,
                                "{look:?} {level:?} dark={dark}: {fg:06x} on {bg:06x} is {r:.2}:1"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Design 199's colours that must stay apart, for normal vision and
    /// simulated protanopia, deuteranopia and tritanopia: the selection,
    /// hover and chosen colours against each other and every colour
    /// below; each group's colours against each other (compare's, the
    /// sketch states', a split's two sides', the participants'); and each against every
    /// viewport background and the model's own colour, which they paint
    /// over. CIEDE2000 at least 10 for every pair.
    #[test]
    fn colours_stay_apart_for_colour_blind_eyes() {
        use ryucad::render as r;
        let groups: [(&str, Vec<(&str, Rgb)>); 5] = [
            (
                "selection",
                vec![("selected", SELECTED), ("hover", HOVER), ("chosen", CHOSEN)],
            ),
            (
                "compare",
                vec![
                    ("new", r::COMPARE_NEW),
                    ("changed", r::COMPARE_CHANGED),
                    ("removed", r::COMPARE_REMOVED),
                    ("ghost", r::COMPARE_GHOST),
                ],
            ),
            (
                "sketch",
                vec![
                    ("well", r::SKETCH_WELL),
                    ("under", r::SKETCH_UNDER),
                    ("over", r::SKETCH_OVER),
                    ("failing", r::SKETCH_FAILING),
                ],
            ),
            (
                "split",
                vec![("behind", SPLIT_BEHIND), ("front", SPLIT_FRONT)],
            ),
            (
                "participants",
                PALETTE
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (["p0", "p1", "p2", "p3", "p4", "p5"][i], *c))
                    .collect(),
            ),
        ];
        let mut grounds: Vec<(String, Rgb)> = LOOKS
            .iter()
            .flat_map(|l| {
                CONTRASTS.iter().flat_map(move |c| {
                    [false, true].map(|d| {
                        (
                            format!("{l:?} {c:?} dark={d}"),
                            l.viewport_background(d, *c),
                        )
                    })
                })
            })
            .collect();
        grounds.push(("the model".into(), r::SURFACE));
        let mut fails = Vec::new();
        for (vision, m) in vision::VISIONS {
            let mut check = |a: &str, x: Rgb, b: &str, y: Rgb| {
                let d = vision::apart(x, y, &m);
                if d < 10.0 {
                    fails.push(format!("{vision}: {a} against {b} is {d:.1}"));
                }
            };
            for (g, colours) in &groups {
                for (i, (a, x)) in colours.iter().enumerate() {
                    for (b, y) in &colours[i + 1..] {
                        check(a, *x, b, *y);
                    }
                    for (ground, y) in &grounds {
                        check(a, *x, ground, *y);
                    }
                    // The selection paints over everything.
                    if *g == "selection" {
                        for (h, others) in &groups {
                            if h != g {
                                for (b, y) in others {
                                    check(a, *x, b, *y);
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }

    #[test]
    fn high_contrast_keeps_the_look_s_sizes_and_outlines_controls() {
        for look in LOOKS {
            for dark in [false, true] {
                let (n, h) = (
                    look.style_with(dark, Contrast::Normal),
                    look.style_with(dark, Contrast::High),
                );
                assert_eq!(n.spacing.item_spacing, h.spacing.item_spacing);
                assert_eq!(n.spacing.interact_size, h.spacing.interact_size);
                assert_eq!(
                    n.visuals.widgets.inactive.corner_radius,
                    h.visuals.widgets.inactive.corner_radius
                );
                // Controls are outlined: their fill is the background's.
                let stroke = h.visuals.widgets.inactive.bg_stroke;
                assert!(stroke.width >= 1.0);
                let border = if dark { 0xffffff } else { 0x000000 };
                assert_eq!(stroke.color, rgb(border));
                // The same palette whatever the look.
                assert_eq!(
                    look.colours(dark, Contrast::High),
                    Look::Drafting.colours(dark, Contrast::High)
                );
            }
        }
        assert_eq!(Contrast::parse("high"), Contrast::High);
        assert_eq!(Contrast::parse(""), Contrast::Normal);
        // High contrast's light viewport is its own; every dark one shares
        // the grey a well defined sketch shows against.
        assert_eq!(
            Look::Studio.viewport_background(false, Contrast::High),
            [0xf2 as f32 / 255.0; 3]
        );
        assert_eq!(
            Look::Studio.viewport_background(true, Contrast::High),
            DARK_VIEWPORT
        );
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
