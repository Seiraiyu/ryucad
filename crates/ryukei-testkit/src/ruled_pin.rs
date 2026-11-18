//! ryukei phase 22's boolean twin for the ruled closed form (design 724,
//! plan 22.D), after FTC 07 AP242 #13541's pin and bore (#658): a block
//! [`SIZE`] bored along z through its middle by a frustum of half-angle
//! [`TAPER`], and a pin of radius [`PIN`] along x at height [`HEIGHT`],
//! its axis [`OFFSET`] from the bore's. The bore's radius at that height
//! is `OFFSET + PIN − bite`, so the pin's outer ruling sinks `bite` into
//! the wall, and its ends pass through the wall into the block. At
//! [`BITE`] (FTC 07 AP242's least gap) the pin grazes the wall; at
//! [`CROSSING`] it crosses it plainly. Both build and check clean, on
//! main as on the branch.
//!
//! Plan 22.D's search, over bites from 1e-9 to 1e-4 m, heights of 10, 15
//! and 20 mm, and the pin's inner ruling grazing the bore from the block
//! side as well, found no shape where main refused: a cylinder and cone
//! cut by a boolean trace and fit at every one, so the ruled form never
//! fires here. FTC 07 AP242's cut (`intersect::face`'s
//! `ftc_07_ap242s_cylinder_and_cone_meet_in_closed_form`, whose trace
//! certifies chains no fit does) drives the form's real path; this twin
//! pins a graze's answer and its bits, so a change that moves it shows.

use ryukei::Error;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Frame, Point3, Rigid, Vector3};
use ryukei::name::{Journal, OpKey};
use ryukei::primitive;
use ryukei::topo::Body;

const MM: f64 = 1e-3;
/// The block: x, y and z.
pub const SIZE: [f64; 3] = [40.0 * MM, 40.0 * MM, 30.0 * MM];
/// The bore's half-angle, radians.
pub const TAPER: f64 = 0.017_453_292_519_943_295;
/// The pin's radius and length.
pub const PIN: f64 = 1.524 * MM;
pub const LENGTH: f64 = 30.0 * MM;
/// The pin's axis: its height, and its offset from the bore's axis (in y).
pub const HEIGHT: f64 = 15.0 * MM;
pub const OFFSET: f64 = 11.2 * MM;
/// How far the pin sinks into the wall: grazing, and crossing.
pub const BITE: f64 = 3.1e-9;
pub const CROSSING: f64 = 1e-4;

fn tan() -> f64 {
    ryukei::math::tan(TAPER)
}

/// The bore's radius at height `z`.
fn bore_radius(bite: f64, z: f64) -> f64 {
    OFFSET + PIN - bite + (z - HEIGHT) * tan()
}

/// The block bored by the frustum for `bite`.
pub fn bored(bite: f64) -> Body {
    let block = primitive::block(OpKey(0x22d0), SIZE[0], SIZE[1], SIZE[2], &Rigid::IDENTITY)
        .unwrap()
        .0;
    let at = Rigid::translation(Vector3::new(SIZE[0] / 2.0, SIZE[1] / 2.0, 0.0));
    let bore = primitive::frustum(
        OpKey(0x22d1),
        bore_radius(bite, 0.0),
        bore_radius(bite, SIZE[2]),
        SIZE[2],
        &at,
    )
    .unwrap()
    .0;
    let tools = [Tool {
        key: ToolKey(1),
        body: &bore,
        place: Rigid::IDENTITY,
    }];
    boolean(OpKey(0x22d2), BooleanOp::Subtract, &block, &tools)
        .unwrap()
        .0
}

/// The pin, along x from `x = (SIZE − LENGTH) / 2`.
pub fn pin() -> Body {
    let along_x = Rigid::from_frame(
        &Frame::new(
            Point3::new((SIZE[0] - LENGTH) / 2.0, SIZE[1] / 2.0 + OFFSET, HEIGHT),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    );
    primitive::cylinder(OpKey(0x22d3), PIN, LENGTH, &along_x)
        .unwrap()
        .0
}

/// The pin subtracted from the bored block.
pub fn cut(bored: &Body, pin: &Body) -> Result<(Body, Journal), Error> {
    let tools = [Tool {
        key: ToolKey(2),
        body: pin,
        place: Rigid::IDENTITY,
    }];
    boolean(OpKey(0x22d4), BooleanOp::Subtract, bored, &tools)
}

/// Whether `p` should be inside the cut for `bite`, or `None` within
/// `margin` of any of the block's, the bore's or the pin's surfaces.
pub fn expected(bite: f64, p: Point3, margin: f64) -> Option<bool> {
    let in_block = (0..3).all(|k| {
        let x = [p.x, p.y, p.z][k];
        x > margin && x < SIZE[k] - margin
    });
    let near_block = (0..3).any(|k| {
        let x = [p.x, p.y, p.z][k];
        x.abs() <= margin || (x - SIZE[k]).abs() <= margin
    });
    let (dx, dy) = (p.x - SIZE[0] / 2.0, p.y - SIZE[1] / 2.0);
    let rho = ryukei::math::sqrt(dx * dx + dy * dy);
    let wall = rho - bore_radius(bite, p.z);
    let (py, pz) = (p.y - (SIZE[1] / 2.0 + OFFSET), p.z - HEIGHT);
    let x0 = (SIZE[0] - LENGTH) / 2.0;
    let side = ryukei::math::sqrt(py * py + pz * pz) - PIN;
    let in_pin = side < 0.0 && p.x > x0 && p.x < x0 + LENGTH;
    let near_pin = side.abs() <= margin && p.x > x0 - margin && p.x < x0 + LENGTH + margin
        || side < margin && ((p.x - x0).abs() <= margin || (p.x - x0 - LENGTH).abs() <= margin);
    if near_block || wall.abs() <= margin * 2.0 || near_pin {
        return None;
    }
    Some(in_block && wall > 0.0 && !in_pin)
}
