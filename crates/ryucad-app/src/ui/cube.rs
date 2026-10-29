//! Drawing the navigation cube, its arrows and the axis triad with
//! egui's painter over the viewport (`core::cube` says where things are
//! and what a point is over).

use crate::core::cube::{Cube, Hit, label};
use crate::ui::style;
use eframe::egui::{self, Color32, Painter, Pos2, Shape, Stroke};
use ryucad::view::Camera;

fn pos(p: [f32; 2]) -> Pos2 {
    egui::pos2(p[0], p[1])
}

/// The cube as `camera` turns it, its tiles shaded by how squarely they
/// face the eye, the views under `hover` (a face, edge or corner's
/// tiles, on every face they touch) and a hovered arrow in the selection
/// colour, the faces seen squarely enough labelled.
pub fn draw(painter: &Painter, cube: &Cube, camera: &Camera, hover: Option<Hit>, dark: bool) {
    let accent = style::color32(style::SELECTED);
    let outline = if dark {
        Color32::from_gray(30)
    } else {
        Color32::from_gray(90)
    };
    let text = if dark {
        Color32::from_gray(235)
    } else {
        Color32::from_gray(25)
    };
    let seen = cube.visible(camera);
    for (tile, pts, facing) in &seen {
        let lit = (0.55 + 0.45 * facing).clamp(0.0, 1.0);
        let fill = if hover == ryucad::view::StandardView::cube(tile.view).map(Hit::View) {
            accent
        } else if dark {
            Color32::from_gray((50.0 + 60.0 * lit) as u8)
        } else {
            Color32::from_gray((150.0 + 95.0 * lit) as u8)
        };
        painter.add(Shape::convex_polygon(
            pts.iter().map(|p| pos(*p)).collect(),
            fill,
            Stroke::new(0.5, outline),
        ));
    }
    for (tile, pts, facing) in &seen {
        if tile.view != tile.face || *facing < 0.35 {
            continue;
        }
        let centre = [(pts[0][0] + pts[2][0]) / 2.0, (pts[0][1] + pts[2][1]) / 2.0];
        painter.text(
            pos(centre),
            egui::Align2::CENTER_CENTER,
            label(tile.face),
            egui::FontId::proportional(8.0 + 3.0 * facing),
            text,
        );
    }
    for (hit, tri) in cube.arrows() {
        let fill = if hover == Some(hit) {
            accent
        } else if dark {
            Color32::from_gray(150)
        } else {
            Color32::from_gray(110)
        };
        painter.add(Shape::convex_polygon(
            tri.iter().map(|p| pos(*p)).collect(),
            fill,
            Stroke::NONE,
        ));
    }
}

/// The model's x, y and z directions in red, green and blue from `at`,
/// as `camera` turns them, nearest the eye drawn last.
pub fn triad(painter: &Painter, at: Pos2, camera: &Camera, dark: bool) {
    let (right, up, forward) = camera.basis();
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let mut axes = [
        ([1.0, 0.0, 0.0], ryucad::render::AXIS_X, "X"),
        ([0.0, 1.0, 0.0], ryucad::render::AXIS_Y, "Y"),
        ([0.0, 0.0, 1.0], ryucad::render::AXIS_Z, "Z"),
    ];
    // Farthest first: forward points away from the eye.
    axes.sort_by(|a, b| dot(b.0, forward).total_cmp(&dot(a.0, forward)));
    let halo = if dark {
        Color32::from_black_alpha(160)
    } else {
        Color32::from_white_alpha(200)
    };
    for (d, c, name) in axes {
        let tip = at + egui::vec2(24.0 * dot(d, right) as f32, -24.0 * dot(d, up) as f32);
        let colour = style::color32(c);
        painter.line_segment([at, tip], Stroke::new(4.0, halo));
        painter.line_segment([at, tip], Stroke::new(2.0, colour));
        // An axis along the line of sight shows as a dot, unlabelled.
        if (tip - at).length() < 2.0 {
            continue;
        }
        let away = (tip - at).normalized() * 8.0;
        painter.text(
            tip + away,
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(11.0),
            colour,
        );
    }
}
