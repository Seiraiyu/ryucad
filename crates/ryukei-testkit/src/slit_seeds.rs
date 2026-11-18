//! ryukei phase 22's twin of #652's slit ball (design 724, plan 22.C): a
//! block [`LENGTH`] × [`WIDTH`] × [`HEIGHT`] whose three edges at corner
//! [`CORNER`] are filleted [`ROUND`] and meet in a ball. The fillets
//! along the two axes other than [`KEPT`] are deleted, healed by growth;
//! the ball and the fillet along [`KEPT`] are kept. On main the delete
//! refuses `HealAmbiguous` at the ball: its seed came by rule 3's shortcut
//! and holds its old face, so nothing yields in the seed retry. Its piece
//! is a slit piece, so its seed yields (plan 22.C): the walk closes
//! without it through `attempt_with`'s seed retry (#941), the kept fillet
//! runs out to the corner along its whole edge, and the ball is deleted.
//!
//! Chosen by plan 22.C's search over the eight corners, three kept
//! fillets, two blocks and three radii: the first shape main refuses at
//! the ball and the branch builds in seven faces.

use core::f64::consts::FRAC_PI_4;
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::geom::{Rigid, Surface};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::{Body, EntityId, FaceId};
use ryukei::tweak::{DeleteSpec, Heal};

/// The block and its rounded corner.
pub const LENGTH: f64 = 0.1;
pub const WIDTH: f64 = 0.08;
pub const HEIGHT: f64 = 0.06;
pub const ROUND: f64 = 0.01;

/// The corner rounded: its face along x, along y and along z.
pub const CORNER: [FaceRole; 3] = [FaceRole::MinusX, FaceRole::MinusY, FaceRole::PlusZ];

/// The axis the kept fillet runs along: 0 for x, 1 for y, 2 for z.
pub const KEPT: usize = 2;

const BLOCK: OpKey = OpKey(0x22c0);

/// The block's face `role`.
fn face(b: &Body, role: FaceRole) -> FaceId {
    match b.by_name(Name::derive(BLOCK, Role::Face(role), &[])) {
        Some(EntityId::Face(f)) => f,
        _ => panic!("no face {role:?}"),
    }
}

/// The block's edge between faces `a` and `c`.
fn edge(b: &Body, a: FaceRole, c: FaceRole) -> Name {
    let (fa, fc) = (face(b, a), face(b, c));
    let e = b
        .edges()
        .find(|&e| {
            let fs: Vec<FaceId> = b
                .edge_fins(e)
                .map(|fin| b.loop_(b.fin(fin).unwrap().loop_).unwrap().face)
                .collect();
            fs.contains(&fa) && fs.contains(&fc)
        })
        .expect("the corner's edge");
    b.edge(e).unwrap().name
}

fn on(b: &Body, f: FaceId) -> &Surface {
    b.surface(b.face(f).unwrap().surface).unwrap()
}

/// The block with the three edges at [`CORNER`] filleted [`ROUND`]: ten
/// faces, the three fillets meeting in a ball.
pub fn rounded() -> Body {
    let (b, _) = primitive::block(BLOCK, LENGTH, WIDTH, HEIGHT, &Rigid::IDENTITY).unwrap();
    let [x, y, z] = CORNER;
    let fillet = |a, c| EdgeBlend {
        edge: edge(&b, a, c),
        kind: BlendKind::Fillet { radius: ROUND },
    };
    blend(
        OpKey(0x22c1),
        &b,
        &[fillet(y, z), fillet(x, z), fillet(x, y)],
    )
    .unwrap()
    .0
}

/// The fillet running along axis `k` (0 x, 1 y, 2 z): the cylinder face
/// whose box is longest that way.
pub fn fillet(b: &Body, k: usize) -> FaceId {
    let along = |f: FaceId| {
        let bx = ryukei::intersect::face_box(b, f);
        let (lo, hi) = (bx.min(), bx.max());
        [hi.x - lo.x, hi.y - lo.y, hi.z - lo.z][k]
    };
    b.faces()
        .filter(|&f| matches!(on(b, f), Surface::Cylinder(_)))
        .max_by(|&f, &g| along(f).total_cmp(&along(g)))
        .expect("a fillet")
}

/// The ball where the three fillets meet.
pub fn ball(b: &Body) -> FaceId {
    b.faces()
        .find(|&f| matches!(on(b, f), Surface::Sphere(_)))
        .expect("the ball")
}

/// The delete of the two fillets other than the one along [`KEPT`],
/// healed by growth.
pub fn spec(b: &Body) -> DeleteSpec {
    DeleteSpec {
        faces: (0..3)
            .filter(|&k| k != KEPT)
            .map(|k| b.face(fillet(b, k)).unwrap().name)
            .collect(),
        heal: Heal::Grow,
    }
}

/// The volume once the kept fillet runs out to the corner: the block
/// less the fillet's corner strip, `(1 - π/4) r²`, along its whole edge.
pub fn run_out_volume() -> f64 {
    let along = [LENGTH, WIDTH, HEIGHT][KEPT];
    LENGTH * WIDTH * HEIGHT - (1.0 - FRAC_PI_4) * ROUND * ROUND * along
}
