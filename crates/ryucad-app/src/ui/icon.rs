//! Icons (design 199, Fonts and icons): Phosphor Regular's glyphs by code
//! point, and ryucad's own 15 CAD glyphs as stroke tables
//! ([`glyphs`], ported stroke for stroke from `design/app/glyphs.js`),
//! drawn alike at any size and tinted by state. An icon never stands
//! alone: [`button`] gives every icon button its command's title as its
//! accesskit name and tooltip.

use eframe::egui::{self, Color32, FontFamily, FontId, Pos2, Rect, Vec2};

/// Every icon the window draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// A Phosphor Regular glyph: its name and code point.
    Ph(&'static str, char),
    /// One of ryucad's glyphs.
    Glyph(glyphs::Glyph),
}

macro_rules! phosphor {
    ($($konst:ident = $name:literal $cp:literal,)*) => {
        impl Icon {
            $(pub const $konst: Icon = Icon::Ph($name, $cp);)*
            /// Every Phosphor icon the window uses.
            pub const PHOSPHOR: &'static [Icon] = &[$(Icon::$konst,)*];
        }
    };
}

// Code points from Phosphor 2.1.2's style sheet (design/app/vendor).
phosphor! {
    CUBE = "cube" '\u{e1da}',
    CUBE_FOCUS = "cube-focus" '\u{ed0a}',
    CUBE_TRANSPARENT = "cube-transparent" '\u{ec7c}',
    CYLINDER = "cylinder" '\u{e8fc}',
    SPHERE = "sphere" '\u{ee66}',
    PENCIL_SIMPLE_LINE = "pencil-simple-line" '\u{ebc6}',
    LINK_SIMPLE = "link-simple" '\u{e2e6}',
    UNITE = "unite" '\u{e87e}',
    SUBTRACT = "subtract" '\u{ebd6}',
    INTERSECT = "intersect" '\u{e2d2}',
    FLIP_HORIZONTAL = "flip-horizontal" '\u{ed6a}',
    FILE_ARROW_DOWN = "file-arrow-down" '\u{e232}',
    PAPERCLIP = "paperclip" '\u{e39a}',
    LOCK_SIMPLE = "lock-simple" '\u{e308}',
    ARROW_COUNTER_CLOCKWISE = "arrow-counter-clockwise" '\u{e038}',
    ARROWS_LEFT_RIGHT = "arrows-left-right" '\u{e0a0}',
    X_CIRCLE = "x-circle" '\u{e4f8}',
    WARNING = "warning" '\u{e4e0}',
    ARROWS_CLOCKWISE = "arrows-clockwise" '\u{e094}',
    EYE = "eye" '\u{e220}',
    EYE_SLASH = "eye-slash" '\u{e224}',
    ARROW_ELBOW_DOWN_RIGHT = "arrow-elbow-down-right" '\u{e046}',
    PUSH_PIN = "push-pin" '\u{e3e2}',
    WARNING_CIRCLE = "warning-circle" '\u{e4e2}',
    GIT_MERGE = "git-merge" '\u{e280}',
    SEAL_CHECK = "seal-check" '\u{e606}',
    SEAL_WARNING = "seal-warning" '\u{e60c}',
    SEAL_QUESTION = "seal-question" '\u{e608}',
    PROHIBIT = "prohibit" '\u{e3de}',
    PLUS_CIRCLE = "plus-circle" '\u{e3d6}',
    NOTE_PENCIL = "note-pencil" '\u{e34c}',
    ARROWS_DOWN_UP = "arrows-down-up" '\u{e098}',
    MINUS_CIRCLE = "minus-circle" '\u{e32c}',
    ARROW_U_UP_LEFT = "arrow-u-up-left" '\u{e08a}',
    ARROW_U_UP_RIGHT = "arrow-u-up-right" '\u{e08c}',
    ARROWS_OUT = "arrows-out" '\u{e0a2}',
    SELECTION = "selection" '\u{e69a}',
    GIT_DIFF = "git-diff" '\u{e27c}',
    INTERSECT_SQUARE = "intersect-square" '\u{e87a}',
    COMMAND = "command" '\u{e1c4}',
    MAGNIFYING_GLASS = "magnifying-glass" '\u{e30c}',
    CARET_DOWN = "caret-down" '\u{e136}',
    CARET_RIGHT = "caret-right" '\u{e13a}',
    X = "x" '\u{e4f6}',
    CHECK_CIRCLE = "check-circle" '\u{e184}',
    PLUGS_CONNECTED = "plugs-connected" '\u{eb5a}',
    COPY = "copy" '\u{e1ca}',
    TREE_STRUCTURE = "tree-structure" '\u{e67c}',
    SLIDERS_HORIZONTAL = "sliders-horizontal" '\u{e434}',
    LIST_BULLETS = "list-bullets" '\u{e2f2}',
    CLOCK_COUNTER_CLOCKWISE = "clock-counter-clockwise" '\u{e1a0}',
    KEYBOARD = "keyboard" '\u{e2d8}',
    INFO = "info" '\u{e2ce}',
    FOLDER_OPEN = "folder-open" '\u{e256}',
    RULER = "ruler" '\u{e6b8}',
    SCALES = "scales" '\u{e750}',
    PALETTE = "palette" '\u{e6c8}',
    ROBOT = "robot" '\u{e762}',
    USER = "user" '\u{e4c2}',
    SPINNER_GAP = "spinner-gap" '\u{e66c}',
    CLOCK = "clock" '\u{e19a}',
    TERMINAL_WINDOW = "terminal-window" '\u{eae8}',
    PLUGS = "plugs" '\u{eb56}',
    GEAR = "gear" '\u{e270}',
    STETHOSCOPE = "stethoscope" '\u{e7ea}',
    FUNCTION = "function" '\u{ebe4}',
    PACKAGE = "package" '\u{e390}',
    QUESTION = "question" '\u{e3e8}',
    EXPORT = "export" '\u{eaf0}',
    TABLE = "table" '\u{e476}',
    CROSSHAIR = "crosshair" '\u{e1d6}',
    LAYOUT = "layout" '\u{e6d6}',
    SIGN_OUT = "sign-out" '\u{e42a}',
    KNIFE = "knife" '\u{e636}',
    ARROWS_OUT_CARDINAL = "arrows-out-cardinal" '\u{e0a4}',
}

impl Icon {
    pub const CONE: Icon = Icon::Glyph(glyphs::Glyph::Cone);
    pub const TORUS: Icon = Icon::Glyph(glyphs::Glyph::Torus);
    pub const EXTRUDE: Icon = Icon::Glyph(glyphs::Glyph::Extrude);
    pub const EXTRUDE_CUT: Icon = Icon::Glyph(glyphs::Glyph::ExtrudeCut);
    pub const REVOLVE: Icon = Icon::Glyph(glyphs::Glyph::Revolve);
    pub const REVOLVE_CUT: Icon = Icon::Glyph(glyphs::Glyph::RevolveCut);
    pub const FILLET: Icon = Icon::Glyph(glyphs::Glyph::Fillet);
    pub const CHAMFER: Icon = Icon::Glyph(glyphs::Glyph::Chamfer);
    pub const HOLE: Icon = Icon::Glyph(glyphs::Glyph::Hole);
    pub const PATTERN_LINEAR: Icon = Icon::Glyph(glyphs::Glyph::PatternLinear);
    pub const PATTERN_CIRCULAR: Icon = Icon::Glyph(glyphs::Glyph::PatternCircular);
    pub const ASSEMBLY: Icon = Icon::Glyph(glyphs::Glyph::Assembly);
    pub const MATE_CYLINDRICAL: Icon = Icon::Glyph(glyphs::Glyph::MateCylindrical);
    pub const MATE_PLANAR: Icon = Icon::Glyph(glyphs::Glyph::MatePlanar);
    pub const MATE_BALL: Icon = Icon::Glyph(glyphs::Glyph::MateBall);

    /// The icon's name: Phosphor's, or the glyph's in `glyphs.js`.
    pub fn name(self) -> &'static str {
        match self {
            Icon::Ph(name, _) => name,
            Icon::Glyph(g) => g.name(),
        }
    }

    /// Every icon by name, for tests and the icon sheet.
    pub fn all() -> impl Iterator<Item = Icon> {
        Icon::PHOSPHOR
            .iter()
            .copied()
            .chain(glyphs::Glyph::ALL.iter().map(|g| Icon::Glyph(*g)))
    }
}

/// Paints `icon` filling the square `rect` in `colour`.
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, colour: Color32) {
    match icon {
        Icon::Ph(_, cp) => {
            let font = FontId::new(
                rect.height(),
                FontFamily::Name(crate::ui::fonts::ICONS.into()),
            );
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, cp, font, colour);
        }
        Icon::Glyph(g) => glyphs::paint(painter, rect, g, colour),
    }
}

/// An icon `size` points square, tinted `colour`, as a label.
pub fn show(ui: &mut egui::Ui, icon: Icon, size: f32, colour: Color32) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect, icon, colour);
    }
    r
}

/// An icon button: `size` points of icon in a frame, named `title` for
/// screen readers and the window tests, with `title` (and its keys) as
/// its tooltip; `on` draws it chosen. Disabled, it says `reason` on hover.
pub fn button(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    title: &str,
    tip: &str,
    on: bool,
    enabled: Result<(), &str>,
) -> egui::Response {
    let pad = Vec2::new(4.0, 3.0);
    let (rect, r) = ui.allocate_exact_size(
        Vec2::splat(size) + 2.0 * pad,
        if enabled.is_ok() {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let ok = enabled.is_ok();
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ok, on, title));
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let (fill, colour) = if on {
            (v.selection.bg_fill, v.selection.stroke.color)
        } else if !ok {
            (Color32::TRANSPARENT, v.weak_text_color())
        } else if r.hovered() {
            (v.widgets.hovered.bg_fill, v.text_color())
        } else {
            (Color32::TRANSPARENT, v.text_color())
        };
        ui.painter()
            .rect_filled(rect, v.widgets.inactive.corner_radius, fill);
        paint(ui.painter(), rect.shrink2(pad), icon, colour);
    }
    match enabled {
        Ok(()) => r.on_hover_text(tip),
        Err(why) => r.on_disabled_hover_text(format!("{tip}\n{why}")),
    }
}

pub mod glyphs {
    //! ryucad's 15 CAD glyphs on Phosphor's 256-unit grid with its
    //! 16-unit stroke, round caps and joins: `design/app/glyphs.js`'s
    //! table, stroke for stroke.

    use super::*;

    /// Phosphor's stroke, in grid units.
    pub const STROKE: f32 = 16.0;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Glyph {
        Cone,
        Torus,
        Extrude,
        ExtrudeCut,
        Revolve,
        RevolveCut,
        Fillet,
        Chamfer,
        Hole,
        PatternLinear,
        PatternCircular,
        Assembly,
        MateCylindrical,
        MatePlanar,
        MateBall,
    }

    impl Glyph {
        pub const ALL: [Glyph; 15] = [
            Glyph::Cone,
            Glyph::Torus,
            Glyph::Extrude,
            Glyph::ExtrudeCut,
            Glyph::Revolve,
            Glyph::RevolveCut,
            Glyph::Fillet,
            Glyph::Chamfer,
            Glyph::Hole,
            Glyph::PatternLinear,
            Glyph::PatternCircular,
            Glyph::Assembly,
            Glyph::MateCylindrical,
            Glyph::MatePlanar,
            Glyph::MateBall,
        ];

        /// The glyph's name in `glyphs.js`.
        pub fn name(self) -> &'static str {
            match self {
                Glyph::Cone => "cone",
                Glyph::Torus => "torus",
                Glyph::Extrude => "extrude",
                Glyph::ExtrudeCut => "extrude-cut",
                Glyph::Revolve => "revolve",
                Glyph::RevolveCut => "revolve-cut",
                Glyph::Fillet => "fillet",
                Glyph::Chamfer => "chamfer",
                Glyph::Hole => "hole",
                Glyph::PatternLinear => "pattern-linear",
                Glyph::PatternCircular => "pattern-circular",
                Glyph::Assembly => "assembly",
                Glyph::MateCylindrical => "mate-cylindrical",
                Glyph::MatePlanar => "mate-planar",
                Glyph::MateBall => "mate-ball",
            }
        }
    }

    /// One stroke, in grid units (y down, as the screen).
    #[derive(Clone, Debug, PartialEq)]
    pub enum Stroke {
        /// A polyline and its width.
        Line(Vec<[f32; 2]>, f32),
        /// An ellipse `(cx, cy, rx, ry)`, stroked at a width or filled.
        Ellipse([f32; 4], f32, bool),
        /// An arc of an ellipse `(cx, cy, rx, ry)` from `from` to `to`
        /// degrees, clockwise from +x as the screen has it.
        Arc([f32; 4], f32, f32, f32),
    }

    fn line(points: &[[f32; 2]]) -> Stroke {
        Stroke::Line(points.to_vec(), STROKE)
    }

    fn thin(points: &[[f32; 2]], w: f32) -> Stroke {
        Stroke::Line(points.to_vec(), w)
    }

    fn ellipse(e: [f32; 4]) -> Stroke {
        Stroke::Ellipse(e, STROKE, false)
    }

    fn arc(e: [f32; 4], from: f32, to: f32) -> Stroke {
        Stroke::Arc(e, from, to, STROKE)
    }

    /// A block in cabinet projection, as `glyphs.js`'s `box`.
    fn cabinet(x: f32, y: f32, w: f32, h: f32, dx: f32, dy: f32) -> Vec<Stroke> {
        vec![
            line(&[[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]]),
            line(&[[x, y], [x + dx, y + dy], [x + w + dx, y + dy], [x + w, y]]),
            line(&[
                [x + w + dx, y + dy],
                [x + w + dx, y + h + dy],
                [x + w, y + h],
            ]),
        ]
    }

    fn block(x: f32, y: f32, w: f32, h: f32) -> Vec<Stroke> {
        cabinet(x, y, w, h, 36.0, -28.0)
    }

    /// A line with Phosphor's open head, as `glyphs.js`'s `arrow`.
    fn arrow(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Stroke> {
        let a = (y1 - y0).atan2(x1 - x0);
        let (l, s) = (40.0, 0.55);
        vec![
            line(&[[x0, y0], [x1, y1]]),
            line(&[
                [x1 - l * (a + s).cos(), y1 - l * (a + s).sin()],
                [x1, y1],
                [x1 - l * (a - s).cos(), y1 - l * (a - s).sin()],
            ]),
        ]
    }

    /// Dashes down `x`, as `glyphs.js`'s `dashes`.
    fn dashes(x: f32, from: f32, to: f32, step: f32, len: f32, w: f32) -> Vec<Stroke> {
        let mut out = Vec::new();
        let mut y = from;
        while y < to {
            out.push(thin(&[[x, y], [x, y + len]], w));
            y += step;
        }
        out
    }

    /// A glyph's strokes.
    pub fn strokes(g: Glyph) -> Vec<Stroke> {
        let mut s = Vec::new();
        match g {
            Glyph::Cone => {
                s.push(ellipse([128.0, 198.0, 88.0, 26.0]));
                s.push(line(&[[44.0, 196.0], [128.0, 32.0], [212.0, 196.0]]));
            }
            Glyph::Torus => {
                s.push(ellipse([128.0, 132.0, 108.0, 60.0]));
                s.push(arc([128.0, 136.0, 44.0, 24.0], 20.0, 160.0));
                s.push(arc([128.0, 135.0, 32.0, 15.0], 200.0, 340.0));
            }
            Glyph::Extrude => {
                s.extend(block(48.0, 140.0, 128.0, 72.0));
                s.extend(arrow(128.0, 128.0, 128.0, 24.0));
            }
            Glyph::ExtrudeCut => {
                s.extend(block(40.0, 112.0, 152.0, 96.0));
                s.push(ellipse([116.0, 116.0, 36.0, 12.0]));
                s.extend(arrow(116.0, 20.0, 116.0, 92.0));
            }
            Glyph::Revolve => {
                s.extend(dashes(128.0, 24.0, 236.0, 40.0, 20.0, 12.0));
                s.push(line(&[
                    [128.0, 64.0],
                    [176.0, 64.0],
                    [176.0, 176.0],
                    [128.0, 176.0],
                ]));
                s.push(arc([128.0, 164.0, 88.0, 68.0], 200.0, 380.0));
                s.push(line(&[[178.0, 168.0], [206.0, 176.0], [222.0, 150.0]]));
            }
            Glyph::RevolveCut => {
                s.extend(dashes(128.0, 24.0, 236.0, 40.0, 20.0, 12.0));
                s.push(line(&[
                    [128.0, 48.0],
                    [192.0, 48.0],
                    [192.0, 96.0],
                    [160.0, 112.0],
                    [192.0, 128.0],
                    [192.0, 192.0],
                    [128.0, 192.0],
                ]));
                s.push(arc([128.0, 180.0, 96.0, 60.0], 200.0, 330.0));
            }
            Glyph::Fillet => {
                s.push(line(&[
                    [136.0, 208.0],
                    [24.0, 208.0],
                    [24.0, 64.0],
                    [136.0, 64.0],
                ]));
                s.push(arc([136.0, 160.0, 96.0, 96.0], 270.0, 360.0));
                s.push(line(&[[232.0, 160.0], [232.0, 208.0], [136.0, 208.0]]));
                s.push(thin(&[[136.0, 160.0], [204.0, 92.0]], 10.0));
                s.push(thin(&[[126.0, 150.0], [146.0, 170.0]], 10.0));
            }
            Glyph::Chamfer => {
                s.push(line(&[
                    [24.0, 208.0],
                    [24.0, 64.0],
                    [152.0, 64.0],
                    [232.0, 144.0],
                    [232.0, 208.0],
                    [24.0, 208.0],
                ]));
                s.push(thin(&[[172.0, 52.0], [240.0, 120.0]], 10.0));
                s.push(thin(&[[164.0, 44.0], [180.0, 60.0]], 10.0));
                s.push(thin(&[[232.0, 112.0], [248.0, 128.0]], 10.0));
            }
            Glyph::Hole => {
                s.extend(block(32.0, 112.0, 160.0, 96.0));
                s.push(ellipse([116.0, 94.0, 32.0, 12.0]));
                s.extend(dashes(88.0, 112.0, 200.0, 28.0, 12.0, 10.0));
                s.extend(dashes(144.0, 112.0, 200.0, 28.0, 12.0, 10.0));
            }
            Glyph::PatternLinear => {
                for x in [32.0, 104.0, 176.0] {
                    s.push(line(&[
                        [x, 88.0],
                        [x + 48.0, 88.0],
                        [x + 48.0, 136.0],
                        [x, 136.0],
                        [x, 88.0],
                    ]));
                }
                s.extend(arrow(32.0, 196.0, 224.0, 196.0));
            }
            Glyph::PatternCircular => {
                for i in 0..6 {
                    let a = i as f32 * std::f32::consts::PI / 3.0 - std::f32::consts::FRAC_PI_2;
                    let c = [128.0 + 84.0 * a.cos(), 128.0 + 84.0 * a.sin(), 22.0, 22.0];
                    s.push(if i == 0 {
                        Stroke::Ellipse(c, STROKE, true)
                    } else {
                        Stroke::Ellipse(c, 12.0, false)
                    });
                }
                s.push(Stroke::Ellipse([128.0, 128.0, 12.0, 12.0], 10.0, false));
            }
            Glyph::Assembly => {
                s.extend(cabinet(24.0, 124.0, 88.0, 88.0, 28.0, -22.0));
                s.extend(cabinet(124.0, 64.0, 88.0, 88.0, 28.0, -22.0));
            }
            Glyph::MateCylindrical => {
                s.push(ellipse([128.0, 40.0, 32.0, 16.0]));
                s.push(line(&[[96.0, 40.0], [96.0, 216.0]]));
                s.push(line(&[[160.0, 40.0], [160.0, 216.0]]));
                s.push(arc([128.0, 216.0, 32.0, 16.0], 0.0, 180.0));
                s.push(arc([128.0, 128.0, 88.0, 32.0], 20.0, 160.0));
                s.push(line(&[[48.0, 140.0], [60.0, 116.0]]));
                s.extend(arrow(224.0, 200.0, 224.0, 64.0));
            }
            Glyph::MatePlanar => {
                s.push(line(&[
                    [24.0, 184.0],
                    [88.0, 120.0],
                    [232.0, 120.0],
                    [168.0, 184.0],
                    [24.0, 184.0],
                ]));
                s.extend(arrow(128.0, 152.0, 128.0, 40.0));
                s.extend(arrow(128.0, 152.0, 204.0, 152.0));
            }
            Glyph::MateBall => {
                s.push(ellipse([128.0, 88.0, 48.0, 48.0]));
                s.push(arc([128.0, 152.0, 88.0, 80.0], 20.0, 160.0));
                s.push(line(&[[128.0, 136.0], [128.0, 176.0]]));
            }
        }
        s
    }

    fn arc_point(e: [f32; 4], deg: f32) -> [f32; 2] {
        let t = deg.to_radians();
        [e[0] + e[2] * t.cos(), e[1] + e[3] * t.sin()]
    }

    /// The points a stroke passes through, arcs and ellipses every 5°.
    pub fn points(s: &Stroke) -> Vec<[f32; 2]> {
        match s {
            Stroke::Line(p, _) => p.clone(),
            Stroke::Ellipse(e, _, _) => (0..=72).map(|i| arc_point(*e, i as f32 * 5.0)).collect(),
            Stroke::Arc(e, from, to, _) => {
                let n = ((to - from) / 5.0).ceil().max(1.0) as usize;
                (0..=n)
                    .map(|i| arc_point(*e, from + (to - from) * i as f32 / n as f32))
                    .collect()
            }
        }
    }

    fn width(s: &Stroke) -> f32 {
        match s {
            Stroke::Line(_, w) | Stroke::Ellipse(_, w, _) | Stroke::Arc(_, _, _, w) => *w,
        }
    }

    /// Paints `g` filling the square `rect`: each stroke a path with round
    /// caps and joins (a dot at every point), at Phosphor's weight scaled.
    pub fn paint(painter: &egui::Painter, rect: Rect, g: Glyph, colour: Color32) {
        let k = rect.width() / 256.0;
        let at = |p: [f32; 2]| Pos2::new(rect.left() + p[0] * k, rect.top() + p[1] * k);
        for s in strokes(g) {
            let pts: Vec<Pos2> = points(&s).into_iter().map(at).collect();
            let w = (width(&s) * k).max(1.0);
            if let Stroke::Ellipse(_, _, true) = s {
                painter.add(egui::Shape::convex_polygon(pts, colour, egui::Stroke::NONE));
                continue;
            }
            painter.add(egui::Shape::line(pts.clone(), egui::Stroke::new(w, colour)));
            if w > 1.5 {
                for p in pts {
                    painter.circle_filled(p, w / 2.0, colour);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// `glyphs.js`'s `checkGrid`: every point plus half its stroke
        /// inside 0..256.
        #[test]
        fn glyphs_stay_inside_phosphor_s_grid() {
            for g in Glyph::ALL {
                for s in strokes(g) {
                    let half = width(&s) / 2.0;
                    for [x, y] in points(&s) {
                        assert!(
                            x - half >= -0.01
                                && y - half >= -0.01
                                && x + half <= 256.01
                                && y + half <= 256.01,
                            "{} {x},{y}",
                            g.name()
                        );
                    }
                }
            }
        }

        /// The glyphs are `glyphs.js`'s, by name, and no other.
        #[test]
        fn every_glyph_of_the_mockups_is_ported() {
            let js = include_str!("../../../../design/app/glyphs.js");
            let table = &js[js.find("const GLYPHS = {").unwrap()..];
            let table = &table[..table.find("\n};").unwrap()];
            let named: Vec<&str> = table
                .lines()
                .filter_map(|l| {
                    let l = l.trim_start();
                    let end = l.find(": [")?;
                    let name = l[..end].trim_matches('"');
                    (!name.is_empty() && !name.contains(' ')).then_some(name)
                })
                .collect();
            let ours: Vec<&str> = Glyph::ALL.iter().map(|g| g.name()).collect();
            assert_eq!(named, ours);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_have_unique_names_and_code_points() {
        let all: Vec<Icon> = Icon::all().collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.name(), b.name());
                if let (Icon::Ph(_, x), Icon::Ph(_, y)) = (a, b) {
                    assert_ne!(x, y, "{} and {}", a.name(), b.name());
                }
            }
        }
    }

    /// Each Phosphor icon's code point is the one the vendored style
    /// sheet gives its name, and the embedded font has it.
    #[test]
    fn phosphor_code_points_are_the_style_sheet_s() {
        let css = include_str!("../../../../design/app/vendor/phosphor.css");
        let ctx = egui::Context::default();
        crate::ui::fonts::install(&ctx);
        let mut out = ctx.run_pass(egui::RawInput::default(), |ctx| {
            for icon in Icon::PHOSPHOR {
                let Icon::Ph(name, cp) = *icon else {
                    unreachable!()
                };
                let head = format!(".ph.ph-{name}:before {{");
                let at = css.find(&head).unwrap_or_else(|| panic!("{name}"));
                let rest = &css[at + head.len()..];
                let q = rest.find("\"\\").unwrap() + 2;
                let hex = &rest[q..q + rest[q..].find('"').unwrap()];
                assert_eq!(
                    u32::from(cp),
                    u32::from_str_radix(hex, 16).unwrap(),
                    "{name}"
                );
                let font = FontId::new(16.0, FontFamily::Name(crate::ui::fonts::ICONS.into()));
                assert!(ctx.fonts_mut(|f| f.has_glyph(&font, cp)), "{name}");
            }
        });
        out.textures_delta.clear();
    }
}
