//! Inputs the `sweep` fuzz target found while the self-intersection check
//! was being made robust, replayed: each sweep must come out sound, and
//! not cross or overlap itself.

use ryukei::check::{Fault, check, check_intersections};
use ryukei_testkit::sweep_bytes::sweep;

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn sound(hex: &str) {
    let (body, _) = sweep(&bytes(hex)).expect("a body");
    assert_eq!(check(&body), []);
    for fault in check_intersections(&body) {
        assert!(matches!(fault, Fault::CannotCheck { .. }), "{fault:?}");
    }
}

/// A slot turned part way: its lines touch the swept arcs' tori at their vertices, in contacts only rounding can place.
#[test]
fn a_slot_turned_part_way() {
    sound(concat!(
        "312f15f660bf7e39922f4a7c7784af12ac8d0ef38a968448aa4e78b7d54c62fd9a0f9c13",
        "111267c8b191ac37bf7bf837cc21e7c7008b8cb7073c9f8ae35b89d8895fdd9969f5438a",
        "acb13be55cc2487ce0b5a3997daf3d7fa5ecd7867099bf5847dcb5501469c114f3a552cf",
        "03b821a50bcb22a122912c079dac8e9537820deb158c6630adc0238251ddd336c6d26099",
        "7efd74baa0c517d40e4990d982ae7e8a79fc4de923a5844b6404ff769646a4d1097ad126",
        "d9",
    ));
}

/// A slot turned part way: a cap meets a torus face where the face's boundary line, run on, would lie, and crossings fall at the middles of brackets.
#[test]
fn a_slot_whose_cap_meets_a_torus_side() {
    sound(concat!(
        "253f9f391434a4c0502f9f35381bf99ca3acb3e774051030533230c88c793974076db26a",
        "044a7f1b13bf98d9e4cf4711c659821397ba12a62faaa5bb6c39daeaeebd00d053be8a48",
        "c38977c8a2404011af041712bb9b1a528b45deb1923383ef3ac375223b9cfb8494448982",
        "c1cc8e5e6ab562749a",
    ));
}

/// A NURBS blob turned part way: rays must not be aimed at the joints between a pcurve's pieces.
#[test]
fn a_blob_turned_part_way() {
    sound(concat!(
        "e47e0d144cb08a6401afa080b1074e79b315fd9b615e5bd798f6293c16058668608392da",
        "4de057e31a6331f76a5d390ec50fdf111c7303190338504231f717d1729ebde7a9c5f676",
        "ced8ef9f8e3ebf40a9a7f24593d6c8a015a264753980f0744a12e57ad3e92c93eefddd81",
        "4e9527193c8f5d1e660bb223ff12914c71622a96d1dcc73492a8bbebd3bdbd1ed7a2fcd9",
        "a17a861847b10614dd2deafc9f8cb6406559d4a115dd7f85c5b9fd1b08c4d12e46f49e11",
        "466fa0f45f639361062357b52760fe4186190d",
    ));
}
