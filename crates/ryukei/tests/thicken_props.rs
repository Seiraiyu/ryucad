//! Thickened sheets against closed forms (design 295, plan 12.E).
//!
//! - **Parallel sets.** A seeded patch of a plane, a cylinder, a sphere or
//!   a torus over a seeded box of its parameters, thickened by seeded
//!   `front` and `back`: its walls lie along the normals, so the solid is
//!   `{S(u, v) + s n(u, v) : s ∈ [-back, front]}`, whose volume has a
//!   closed form (`∫ (1 - 2 H s + K s²) ds dA`: on a cylinder of radius
//!   `R`, `Δu h ((R + f)² - (R - b)²) / 2`; on a sphere,
//!   `Δu (sin v₁ - sin v₀) ((R + f)³ - (R - b)³) / 3`; on a torus,
//!   `Δu [Δv R ρ² / 2 + (sin v₁ - sin v₀) ρ³ / 3]` between `ρ = r - b`
//!   and `r + f`).
//! - **Sides add up.** Seeded sheets of several faces (box faces extracted
//!   as an L or an open box): the volume thickened both ways is the sum
//!   of the two one-sided volumes, within their bounds.
//!
//! Each result is sound (checks, crosses nothing, meshes) and its volume
//! certified. A typed refusal is counted, never a failure. Release only.

use core::f64::consts::{PI, TAU};
use ryukei::Error;
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Cylinder, Frame, Plane, Rigid, Sphere, Surface, Torus};
use ryukei::measure::{Entity, Properties, Value, properties};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::offset::{ThickenSpec, thicken};
use ryukei::primitive;
use ryukei::sheet::{extract, from_surface, remove};
use ryukei::topo::Body;
use ryukei_testkit::Lcg;

/// Seeds tried for each test.
const SEEDS: u64 = 64;

fn volume(b: &Body) -> Value {
    match properties(b, Entity::Body).unwrap() {
        Properties::Body { volume, .. } => volume,
        p => panic!("not a solid: {p:?}"),
    }
}

fn sound(b: &Body) -> bool {
    check(b).is_empty()
        && check_intersections(b).is_empty()
        && tessellate(b, &MeshParams::new(1e-3, 0.5))
            .is_ok_and(|m| m.faults.is_empty() && check_mesh(b, &m).is_ok())
}

/// A seeded patch and its parallel set's volume for `(front, back)`.
fn patch(g: &mut Lcg) -> (Body, Box<dyn Fn(f64, f64) -> f64>) {
    let kind = g.next_u64() % 4;
    let du = g.next_f64(0.3, 2.5);
    let u0 = g.next_f64(0.0, TAU);
    let rect_u = [u0, u0 + du];
    match kind {
        0 => {
            let (w, h) = (g.next_f64(0.2, 2.0), g.next_f64(0.2, 2.0));
            let s = Surface::Plane(Plane::new(Frame::WORLD));
            let b = from_surface(OpKey(1), &s, [[0.0, w], [0.0, h]]).unwrap().0;
            (b, Box::new(move |f, k| w * h * (f + k)))
        }
        1 => {
            let r = g.next_f64(0.5, 2.0);
            let h = g.next_f64(0.2, 2.0);
            let s = Surface::Cylinder(Cylinder::new(Frame::WORLD, r).unwrap());
            let b = from_surface(OpKey(1), &s, [rect_u, [0.0, h]]).unwrap().0;
            (
                b,
                Box::new(move |f, k| du * h * ((r + f).powi(2) - (r - k).powi(2)) / 2.0),
            )
        }
        2 => {
            let r = g.next_f64(0.5, 2.0);
            let v0 = g.next_f64(-1.2, 0.8);
            let v1 = (v0 + g.next_f64(0.2, 0.7)).min(1.3);
            let s = Surface::Sphere(Sphere::new(Frame::WORLD, r).unwrap());
            let b = from_surface(OpKey(1), &s, [rect_u, [v0, v1]]).unwrap().0;
            let lat = ryukei::math::sin(v1) - ryukei::math::sin(v0);
            (
                b,
                Box::new(move |f, k| du * lat * ((r + f).powi(3) - (r - k).powi(3)) / 3.0),
            )
        }
        _ => {
            let big = g.next_f64(1.5, 3.0);
            let r = g.next_f64(0.4, 1.0);
            let v0 = g.next_f64(-PI, PI);
            let dv = g.next_f64(0.3, 2.0);
            let s = Surface::Torus(Torus::new(Frame::WORLD, big, r).unwrap());
            let b = from_surface(OpKey(1), &s, [rect_u, [v0, v0 + dv]])
                .unwrap()
                .0;
            let lat = ryukei::math::sin(v0 + dv) - ryukei::math::sin(v0);
            let at = move |rho: f64| dv * big * rho * rho / 2.0 + lat * rho.powi(3) / 3.0;
            (b, Box::new(move |f, k| du * (at(r + f) - at(r - k))))
        }
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only: minutes in a debug build")]
fn patches_thicken_to_their_parallel_sets() {
    let (mut judged, mut refused) = (0, 0);
    for seed in 0..SEEDS {
        let mut g = Lcg::new(seed);
        let (sheet, exact) = patch(&mut g);
        let f = if g.next_u64() % 4 == 0 {
            0.0
        } else {
            g.next_f64(0.01, 0.2)
        };
        let b = if f > 0.0 && g.next_u64() % 4 == 0 {
            0.0
        } else {
            g.next_f64(0.01, 0.2)
        };
        let spec = ThickenSpec {
            front: f,
            back: b,
            faces: Vec::new(),
        };
        match thicken(OpKey(2), &sheet, &spec) {
            Ok((solid, _)) => {
                assert!(sound(&solid), "seed {seed}: unsound");
                let v = volume(&solid);
                let x = exact(f, b);
                assert!(
                    (v.value() - x).abs() <= v.bound() + 1e-9 * x.abs().max(1.0),
                    "seed {seed}: {v:?} against {x}"
                );
                judged += 1;
            }
            Err(e @ Error::InvalidOffset(_)) => {
                eprintln!("seed {seed}: refused {e:?}");
                refused += 1;
            }
            Err(e) => panic!("seed {seed}: {e:?}"),
        }
    }
    eprintln!("{judged} judged, {refused} refused");
    assert!(judged >= SEEDS * 9 / 10, "{judged} judged of {SEEDS}");
}

/// A seeded sheet of box faces: an L of two, or a box less its top.
fn boxed(g: &mut Lcg) -> Body {
    let (a, b, c) = (
        g.next_f64(0.3, 2.0),
        g.next_f64(0.3, 2.0),
        g.next_f64(0.3, 2.0),
    );
    let (block, _) = primitive::block(OpKey(3), a, b, c, &Rigid::IDENTITY).unwrap();
    let face = |r| Name::derive(OpKey(3), Role::Face(r), &[]);
    if g.next_u64() % 2 == 0 {
        extract(
            OpKey(4),
            &block,
            &[face(FaceRole::PlusZ), face(FaceRole::MinusY)],
        )
        .unwrap()
        .0
    } else {
        remove(OpKey(4), &block, &[face(FaceRole::PlusZ)])
            .unwrap()
            .0
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only: minutes in a debug build")]
fn front_and_back_add_up() {
    let mut judged = 0;
    for seed in 0..SEEDS {
        let mut g = Lcg::new(seed);
        let sheet = boxed(&mut g);
        let (f, b) = (g.next_f64(0.01, 0.1), g.next_f64(0.01, 0.1));
        let mut vols = Vec::new();
        for (x, y) in [(f, b), (f, 0.0), (0.0, b)] {
            let spec = ThickenSpec {
                front: x,
                back: y,
                faces: Vec::new(),
            };
            match thicken(OpKey(2), &sheet, &spec) {
                Ok((solid, _)) => {
                    assert!(sound(&solid), "seed {seed} ({x}, {y}): unsound");
                    vols.push(volume(&solid));
                }
                Err(e @ Error::InvalidOffset(_)) => {
                    eprintln!("seed {seed} ({x}, {y}): refused {e:?}");
                }
                Err(e) => panic!("seed {seed}: {e:?}"),
            }
        }
        if let [both, front, back] = vols[..] {
            let gap = (both.value() - front.value() - back.value()).abs();
            let bound = both.bound() + front.bound() + back.bound() + 1e-9;
            assert!(gap <= bound, "seed {seed}: {both:?} {front:?} {back:?}");
            judged += 1;
        }
    }
    eprintln!("{judged} judged");
    assert!(judged >= SEEDS * 9 / 10, "{judged} judged of {SEEDS}");
}
