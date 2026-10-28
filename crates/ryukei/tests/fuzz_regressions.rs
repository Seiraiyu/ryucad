//! Inputs the `sweep` fuzz target found while the self-intersection check
//! was being made robust, and the `boolean` target while booleans were,
//! replayed: each sweep must come out sound, and not cross or overlap
//! itself; each boolean must also mesh without faults. The `heal` target's
//! finds must heal into a body that checks, and their mild faults back
//! into the body they were written from.

use ryukei::boolean::boolean;
use ryukei::check::{check, check_intersections};
use ryukei::heal::{HealOptions, heal};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::heal_bytes::heal_case;
use ryukei_testkit::sweep_bytes::sweep;

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// A boolean from the bytes, which must succeed, come out sound and mesh.
fn boolean_sound(hex: &str) {
    let case = boolean_case(&bytes(hex)).expect("operands");
    let (body, _) = boolean(OpKey(9), case.op, &case.target, &case.tools()).unwrap();
    assert_eq!(check(&body), []);
    assert_eq!(check_intersections(&body), []);
    let m = tessellate(&body, &MeshParams::new(1e-2, 0.5)).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(&body, &m), Ok(()));
}

/// A shape from the bytes, which must heal into a body that checks, the
/// body it was written from where its faults are mild.
fn heals(hex: &str) {
    let case = heal_case(&bytes(hex)).expect("a shape");
    let (body, _, _) = heal(OpKey(0x4ea1), &case.shape, &HealOptions::default()).unwrap();
    assert_eq!(check(&body), []);
    if case.mild {
        assert_eq!(body.counts(), case.body.counts());
    }
}

fn sound(hex: &str) {
    let (body, _) = sweep(&bytes(hex)).expect("a body");
    assert_eq!(check(&body), []);
    assert_eq!(check_intersections(&body), []);
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

/// A torus minus a coaxial cone touching its tube all round: the circle
/// where they touch is an edge alone in two loops of the torus face, which
/// merging must remove.
#[test]
fn a_torus_touched_all_round_by_a_cone() {
    boolean_sound(concat!(
        "0aafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafaf",
        "afafafafafafafafafafaf0aff03",
    ));
}

/// A torus minus two blocks: the torus face's loop passes through one
/// vertex twice, and its two visits must be one point of the mesh.
#[test]
fn a_torus_face_whose_loop_passes_a_vertex_twice() {
    boolean_sound("a319191919191919191919191919191919191919191919191919ff5b");
}

/// A block minus a cone and a block: a quarter of the cone from its apex,
/// one edge along the seam, the apex exactly at the strip's edge.
#[test]
fn a_cone_wedge_along_its_seam() {
    boolean_sound("970700001e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e20");
}

/// A cylinder minus a cone touching it near a point: out of the shared
/// vertex at the tangent wedge's tip, both faces' edges run together.
#[test]
fn a_cylinder_minus_a_cone_touching_it() {
    boolean_sound("0a0a060606060606060606060606060606d3c806060606060606060609000000adae");
}

/// Blocks united on a grid: out of a tangent wedge's tip, one face's edge
/// lies on the other face within the touch rule's reach.
#[test]
fn an_edge_lying_on_a_face_out_of_a_tangent_tip() {
    boolean_sound(concat!(
        "0108ffbfffffffffffffffffffffffffffffffffffffffff3effffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffff2408",
    ));
}

/// The same tangent tip, mirrored.
#[test]
fn the_same_tip_seen_from_the_other_side() {
    boolean_sound("29faf700000000000000047a7f7f7f7f7f7f7f7f2a");
}

/// A ball united with a tilted block whose face plane holds the ball's
/// axis: the hemisphere left is a lune exactly half a turn wide, and the
/// jumps across both poles round the same way, so its ring seemed to wind.
#[test]
fn a_hemisphere_cut_through_both_poles() {
    boolean_sound(concat!(
        "0f00000000000000000a000026000000000400007e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e",
        "7e7e7e7e7e7e7e2400000000000000000a00",
    ));
}

/// A ball united with two blocks, its centre on a face plane holding its
/// axis: the half left runs through one pole, half a turn across it, and
/// straddles the seam, which its jump along the pole crosses.
#[test]
fn a_hemisphere_through_one_pole_across_the_seam() {
    boolean_sound("0f0000005d000000000a000026000000000400002400000000000000000a00");
}

/// A cylinder, a cone's frustum and a ball written with no faults: a
/// vertex of the cylinder lies in the plane of the frustum's base, so
/// every ray from it was taken as grazing that face's triangles, and
/// which shell held which was uncertain.
#[test]
fn a_vertex_in_the_plane_of_another_shells_face() {
    heals(concat!(
        "08fc383430366131323038303166663166356133383838356238383133643137393266",
        "396438336262330000000000000000343532326530316537313032623732",
        "00f7",
    ));
}

/// A square a centimetre across carried 3 m beyond a 0.2 m bend's centre:
/// it runs back over itself round the bend, so it is refused at once,
/// where its ribbon-like sides took about 150 s a body to trace (issue
/// #133).
#[test]
fn a_profile_carried_back_over_itself_round_a_bend() {
    let start = std::time::Instant::now();
    let data = [47, 38, 26, 0, 49, 26, 0, 49, 26, 0, 0, 31, 31, 31, 31, 31];
    assert!(ryukei_testkit::along_bytes::carried(&data).is_none());
    assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
}
