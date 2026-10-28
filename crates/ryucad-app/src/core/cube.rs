//! The navigation cube in the viewport's top-right corner, as FreeCAD has
//! one: a cube turning with the camera whose 6 faces, 12 edges and 8
//! corners give the 26 views of `ryucad::view::StandardView` (each face
//! cut 3 × 3, its middle the face, its sides the edges' bevels, its
//! corners the corners'), two arrows that roll the view a quarter turn
//! about the line of sight, four that turn it a quarter turn about the
//! screen's axes, and a menu button. Geometry only: `ui::view` draws it
//! and asks it what the pointer is over.

use ryucad::view::{Camera, StandardView};

/// How far from a face's middle its own tile reaches, of the cube's half
/// size: the rest is its edges' and corners' bevels.
pub const INNER: f64 = 0.6;

/// The cube's half size, in points.
pub const HALF: f32 = 28.0;

/// How far the cube's middle sits from the viewport's top-right corner,
/// in points: room for the arrows round it.
pub const INSET: f32 = 62.0;

/// What a point of the cube's widget is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// A face, edge or corner: look from there.
    View(StandardView),
    /// A roll arrow: a quarter turn about the line of sight, the model
    /// turning counterclockwise for 1 and clockwise for -1.
    Roll(i8),
    /// A turn arrow: a quarter turn about the screen's up axis (`right`
    /// -1 or 1: the eye moves that way) or right axis (`up`).
    Turn { right: i8, up: i8 },
    /// The menu button below the cube.
    Menu,
}

/// One tile of the cube's surface: the view it gives, the face it lies
/// on (the outward direction), and its corners on the cube `[-1, 1]³`,
/// counterclockwise from outside.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub view: [i8; 3],
    pub face: [i8; 3],
    pub corners: [[f64; 3]; 4],
}

/// The 54 tiles: each face's 9, faces in the order x, -x, y, -y, z, -z.
pub fn tiles() -> Vec<Tile> {
    let mut out = Vec::new();
    for axis in 0..3 {
        for sign in [1i8, -1] {
            let mut face = [0i8; 3];
            face[axis] = sign;
            // Two directions across the face, making a right-handed frame
            // with its outward normal.
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            let (a, b) = if sign > 0 { (a, b) } else { (b, a) };
            let spans = [(-1.0, -INNER), (-INNER, INNER), (INNER, 1.0)];
            for (i, (a0, a1)) in spans.iter().enumerate() {
                for (j, (b0, b1)) in spans.iter().enumerate() {
                    let at = |u: f64, v: f64| {
                        let mut p = [0.0; 3];
                        p[axis] = f64::from(sign);
                        p[a] = u;
                        p[b] = v;
                        p
                    };
                    let mut view = face;
                    view[a] = i as i8 - 1;
                    view[b] = j as i8 - 1;
                    out.push(Tile {
                        view,
                        face,
                        corners: [at(*a0, *b0), at(*a1, *b0), at(*a1, *b1), at(*a0, *b1)],
                    });
                }
            }
        }
    }
    out
}

/// A face's label, by its outward direction: front looks along +y, so
/// the front face is the one at -y.
pub fn label(face: [i8; 3]) -> &'static str {
    match face {
        [0, -1, 0] => "Front",
        [0, 1, 0] => "Back",
        [-1, 0, 0] => "Left",
        [1, 0, 0] => "Right",
        [0, 0, 1] => "Top",
        _ => "Bottom",
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The cube where a widget puts it: its middle on screen, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cube {
    pub centre: [f32; 2],
    pub half: f32,
}

impl Cube {
    /// The cube in the top-right corner of a viewport whose top right is
    /// at `corner`.
    pub fn at(corner: [f32; 2]) -> Cube {
        Cube {
            centre: [corner[0] - INSET, corner[1] + INSET],
            half: HALF,
        }
    }

    /// Where a point of the cube shows, seen as `camera` sees the model
    /// (orthographically, whatever the camera's projection).
    pub fn project(&self, camera: &Camera, p: [f64; 3]) -> [f32; 2] {
        let (right, up, _) = camera.basis();
        [
            self.centre[0] + self.half * dot(p, right) as f32,
            self.centre[1] - self.half * dot(p, up) as f32,
        ]
    }

    /// The tiles facing the eye, with their corners on screen and how
    /// squarely their face meets the eye (0 edge on, 1 head on).
    pub fn visible(&self, camera: &Camera) -> Vec<(Tile, [[f32; 2]; 4], f32)> {
        let (_, _, forward) = camera.basis();
        tiles()
            .into_iter()
            .filter_map(|t| {
                let facing = -dot(t.face.map(f64::from), forward);
                (facing > 1e-6).then(|| {
                    let pts = t.corners.map(|c| self.project(camera, c));
                    (t, pts, facing as f32)
                })
            })
            .collect()
    }

    /// The arrows round the cube: each a triangle on screen.
    pub fn arrows(&self) -> Vec<(Hit, [[f32; 2]; 3])> {
        let [cx, cy] = self.centre;
        let (r, w, l) = (self.half * 1.75, 7.0, 9.0);
        let tri = |dx: f32, dy: f32| {
            // Pointing out along (dx, dy) from the cube.
            let tip = [cx + dx * (r + l), cy + dy * (r + l)];
            let base = [cx + dx * r, cy + dy * r];
            [
                tip,
                [base[0] - dy * w, base[1] + dx * w],
                [base[0] + dy * w, base[1] - dx * w],
            ]
        };
        let mut out = vec![
            (Hit::Turn { right: -1, up: 0 }, tri(-1.0, 0.0)),
            (Hit::Turn { right: 1, up: 0 }, tri(1.0, 0.0)),
            (Hit::Turn { right: 0, up: 1 }, tri(0.0, -1.0)),
            (Hit::Turn { right: 0, up: -1 }, tri(0.0, 1.0)),
        ];
        // The roll arrows beside the up arrow, clear of the cube however
        // it turns (its outline reaches 1.63 half sizes from its middle),
        // pointing round it.
        let top = cy - self.half * 1.6;
        for (k, x) in [(1i8, cx - self.half * 1.35), (-1, cx + self.half * 1.35)] {
            let d = f32::from(-k);
            out.push((
                Hit::Roll(k),
                [
                    [x + d * l, top + 2.0],
                    [x, top - w + 2.0],
                    [x, top + w + 2.0],
                ],
            ));
        }
        out
    }

    /// The menu button: a square below the cube, min and max corners.
    pub fn menu(&self) -> ([f32; 2], [f32; 2]) {
        let [cx, cy] = self.centre;
        let y = cy + self.half * 1.75 + 12.0;
        ([cx + self.half, y - 7.0], [cx + self.half + 14.0, y + 7.0])
    }

    /// The box the whole widget takes, min and max corners: picking in
    /// the viewport stays out of it.
    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let reach = self.half * 1.75 + 20.0;
        let [cx, cy] = self.centre;
        ([cx - reach, cy - reach], [cx + reach, cy + reach])
    }

    /// What `at` (in points) is over, if anything of the widget.
    pub fn hit(&self, camera: &Camera, at: [f32; 2]) -> Option<Hit> {
        let (lo, hi) = self.menu();
        if (lo[0]..=hi[0]).contains(&at[0]) && (lo[1]..=hi[1]).contains(&at[1]) {
            return Some(Hit::Menu);
        }
        for (hit, tri) in self.arrows() {
            if inside(&tri, at) {
                return Some(hit);
            }
        }
        // The faces facing the eye don't overlap: the first tile holding
        // the point is the one seen there.
        self.visible(camera)
            .into_iter()
            .find(|(_, pts, _)| inside(pts, at))
            .and_then(|(t, _, _)| StandardView::cube(t.view))
            .map(Hit::View)
    }
}

/// Whether `p` lies inside the convex polygon `pts` (either winding).
fn inside(pts: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut sign = 0.0f32;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        let c = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        if c != 0.0 {
            if sign != 0.0 && c.signum() != sign {
                return false;
            }
            sign = c.signum();
        }
    }
    true
}

/// `camera` turned a quarter turn about its target as a roll or turn
/// arrow asks, keeping its distance and projection; `None` for a view or
/// the menu.
pub fn turned(camera: &Camera, hit: Hit) -> Option<Camera> {
    let (right, up, forward) = camera.basis();
    let back = forward.map(|x| -x);
    let (back, up) = match hit {
        Hit::Roll(k) => (back, right.map(|x| x * f64::from(k))),
        Hit::Turn { right: r, up: 0 } => (right.map(|x| x * f64::from(r)), up),
        Hit::Turn { up: u, .. } => (
            up.map(|x| x * f64::from(u)),
            back.map(|x| -x * f64::from(u)),
        ),
        Hit::View(_) | Hit::Menu => return None,
    };
    let d = camera.distance();
    Some(Camera {
        eye: [0, 1, 2].map(|k| camera.target[k] + back[k] * d),
        target: camera.target,
        up,
        projection: camera.projection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::view::{self, Projection};

    fn looking(v: StandardView) -> Camera {
        view::frame([-1.0; 3], [1.0; 3], v, view::DEFAULT_PROJECTION)
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-12)
    }

    #[test]
    fn the_tiles_give_the_26_views() {
        let t = tiles();
        assert_eq!(t.len(), 54);
        let mut views: Vec<[i8; 3]> = t.iter().map(|t| t.view).collect();
        views.sort_unstable();
        views.dedup();
        assert_eq!(views.len(), 26);
        for tile in &t {
            assert!(StandardView::cube(tile.view).is_some());
            // Counterclockwise from outside.
            let [a, b, c, _] = tile.corners;
            let (u, v) = (
                [0, 1, 2].map(|k| b[k] - a[k]),
                [0, 1, 2].map(|k| c[k] - a[k]),
            );
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            assert!(dot(n, tile.face.map(f64::from)) > 0.0, "{tile:?}");
        }
        assert_eq!(label([0, -1, 0]), "Front");
        assert_eq!(label([0, 0, -1]), "Bottom");
    }

    #[test]
    fn the_middle_of_the_cube_is_the_face_the_view_looks_at() {
        let cube = Cube::at([800.0, 0.0]);
        for (name, v) in &view::STANDARD_VIEWS[..26] {
            let c = looking(*v);
            // The cube's middle is the tile looked at straight on.
            assert_eq!(cube.hit(&c, cube.centre), Some(Hit::View(*v)), "{name}");
        }
        // From the front, the top edge's bevel is just below the cube's
        // top and the top right corner's to its right.
        let front = looking(StandardView::Front);
        let near_top = [cube.centre[0], cube.centre[1] - 0.8 * cube.half];
        assert_eq!(
            cube.hit(&front, near_top),
            StandardView::named("front_top").map(Hit::View)
        );
        let corner = [
            cube.centre[0] + 0.8 * cube.half,
            cube.centre[1] - 0.8 * cube.half,
        ];
        assert_eq!(
            cube.hit(&front, corner),
            StandardView::named("front_top_right").map(Hit::View)
        );
        // Off the cube and its arrows, nothing.
        assert_eq!(
            cube.hit(&front, [cube.centre[0] - 40.0, cube.centre[1] + 40.0]),
            None
        );
        let (lo, hi) = cube.bounds();
        assert!(lo[0] < cube.centre[0] && hi[1] > cube.centre[1]);
    }

    #[test]
    fn iso_shows_three_faces() {
        let cube = Cube::at([800.0, 0.0]);
        let seen = cube.visible(&looking(StandardView::Iso));
        let mut faces: Vec<[i8; 3]> = seen.iter().map(|(t, ..)| t.face).collect();
        faces.dedup();
        assert_eq!(faces, [[1, 0, 0], [0, -1, 0], [0, 0, 1]]);
        // The corner nearest the eye is the middle of the widget.
        assert_eq!(
            cube.hit(&looking(StandardView::Iso), cube.centre),
            StandardView::named("front_top_right").map(Hit::View)
        );
    }

    #[test]
    fn arrows_turn_a_quarter_turn() {
        let front = looking(StandardView::Front);
        let cube = Cube::at([800.0, 0.0]);
        for (hit, tri) in cube.arrows() {
            let mid = [
                (tri[0][0] + tri[1][0] + tri[2][0]) / 3.0,
                (tri[0][1] + tri[1][1] + tri[2][1]) / 3.0,
            ];
            assert_eq!(cube.hit(&front, mid), Some(hit));
        }
        let (lo, hi) = cube.menu();
        assert_eq!(
            cube.hit(&front, [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]),
            Some(Hit::Menu)
        );
        let back = |c: &Camera| {
            let (_, _, f) = c.basis();
            f.map(|x| -x)
        };
        // From the front, the eye moves right to the right view, up to the
        // top (up along +y, as the standard top view has it).
        let r = turned(&front, Hit::Turn { right: 1, up: 0 }).unwrap();
        assert!(close(back(&r), [1.0, 0.0, 0.0]) && close(r.up, [0.0, 0.0, 1.0]));
        let l = turned(&front, Hit::Turn { right: -1, up: 0 }).unwrap();
        assert!(close(back(&l), [-1.0, 0.0, 0.0]));
        let t = turned(&front, Hit::Turn { right: 0, up: 1 }).unwrap();
        assert!(close(back(&t), [0.0, 0.0, 1.0]) && close(t.up, [0.0, 1.0, 0.0]));
        let d = turned(&front, Hit::Turn { right: 0, up: -1 }).unwrap();
        assert!(close(back(&d), [0.0, 0.0, -1.0]) && close(d.up, [0.0, -1.0, 0.0]));
        // A roll keeps the direction: counterclockwise, what was on the
        // right comes to the top.
        let rolled = turned(&front, Hit::Roll(1)).unwrap();
        assert!(close(back(&rolled), back(&front)) && close(rolled.up, [1.0, 0.0, 0.0]));
        let back_again = turned(&rolled, Hit::Roll(-1)).unwrap();
        assert!(close(back_again.up, front.up));
        // Distance and projection are kept; four turns come round.
        let mut c = front.clone();
        for _ in 0..4 {
            c = turned(&c, Hit::Turn { right: 1, up: 0 }).unwrap();
        }
        assert!(close(c.eye, front.eye) && close(c.up, front.up));
        assert!((r.distance() - front.distance()).abs() < 1e-12);
        let ortho = Camera {
            projection: Projection::Orthographic { height: 2.0 },
            ..front.clone()
        };
        assert_eq!(
            turned(&ortho, Hit::Roll(1)).unwrap().projection,
            ortho.projection
        );
        assert_eq!(turned(&front, Hit::Menu), None);
    }
}
