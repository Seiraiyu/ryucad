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
    let _timed = timed();
    let start = std::time::Instant::now();
    let data = [47, 38, 26, 0, 49, 26, 0, 49, 26, 0, 0, 31, 31, 31, 31, 31];
    assert!(ryukei_testkit::along_bytes::carried(&data).is_none());
    eprintln!("{:?}", start.elapsed());
    assert!(
        !bars() || start.elapsed().as_secs() < 10,
        "{:?}",
        start.elapsed()
    );
}

/// A slot swept askew along three collinear legs a tenth of a metre each
/// (issue #160): each straight side lies on one plane with the next leg's
/// across their joint. Faces on one surface can't be certified apart
/// beside the edge they share, and trying it exhausted the search's
/// budget both ways, about five seconds a pair in release, four pairs; in
/// the fuzzer, under its sanitizer, that churn took the process past its
/// 2 GB limit. They are now intersected as coincident faces at once.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in a debug build: run it in release")]
fn a_slot_swept_along_three_collinear_legs() {
    let data = [
        0x2e, 0x31, 0x43, 0x1a, 0x2e, 0x31, 0x43, 0x43, 0x60, 0x1a, 0x00, 0x43, 0x43, 0x23,
    ];
    let (body, _, along) = ryukei_testkit::along_bytes::carried(&data).expect("a body");
    assert!(along);
    assert_eq!(body.counts().faces, 14);
    assert_eq!(check(&body), []);
    assert_eq!(check_intersections(&body), []);
}

/// The `boolean` target's general case (plan 8.F's campaign): spline,
/// swept and lofted operands whose trace once ran past libFuzzer's 1,200 s
/// (under its sanitizer), pairing thousands of singular leaves two by
/// two (now swept in order instead). It is refused, typed, in about half
/// a minute in release.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in a debug build: run it in release")]
fn a_general_boolean_with_thousands_of_singular_leaves() {
    let _timed = timed();
    use ryukei::{BooleanError, Error};
    use ryukei_testkit::boolean_bytes::general_case;
    let start = std::time::Instant::now();
    let case = general_case(&bytes(concat!(
        "000000000000090a00000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffff0000000000000000000000000000000000000500000000",
    )))
    .expect("operands");
    match boolean(OpKey(9), case.op, &case.target, &case.tools()) {
        Err(Error::InvalidBoolean(BooleanError::Uncertain { .. })) => {}
        Ok((body, _)) => {
            assert_eq!(check(&body), []);
            assert_eq!(check_intersections(&body), []);
        }
        Err(e) => panic!("unexpected {e:?}"),
    }
    eprintln!("{:?}", start.elapsed());
    assert!(
        !bars() || start.elapsed().as_secs() < 300,
        "{:?}",
        start.elapsed()
    );
}

/// The `blend` target's spline case (plan 8.F's campaign): fillets on
/// spline faces whose rails' pcurves took 34 minutes to certify under the
/// fuzzer's sanitizer and debug assertions, past libFuzzer's 1,200 s. It
/// builds and checks clean in seconds in release. Its pipe face stalled
/// the mesher at 1 cm and 0.3 rad (#255): its plane was stretched round
/// the tube, and refinement split the longest plane edge of triangles
/// that turned too far round it. Now it meshes at every chord and angle
/// tried, within the chord.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in a debug build: run it in release")]
fn a_spline_fillet_whose_rails_took_half_an_hour() {
    let _timed = timed();
    use ryukei::blend::blend;
    use ryukei_testkit::blend_bytes::spline_case;
    let start = std::time::Instant::now();
    let case = spline_case(&bytes(concat!(
        "4e4f4f4f4f4f4f4f4f4f4f4f4f4f4ffc4f4f4f4f204f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f",
        "4f4f4f4f0000004f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f",
        "4f4f4f4f4f0000004f584f4f200000004f4f4f4f4f4f4f0a",
    )))
    .expect("a case");
    let (body, _) = blend(OpKey(9), &case.body, &case.blends).unwrap();
    assert_eq!(check(&body), []);
    assert_eq!(check_intersections(&body), []);
    for chord in [1e-3, 1e-2, 3e-2] {
        for angle in [0.3, 0.5, 1.0] {
            let p = MeshParams::new(chord, angle);
            let m = tessellate(&body, &p).unwrap();
            assert_eq!(m.faults, [], "{chord} m, {angle} rad");
            check_mesh(&body, &m).unwrap();
            let d = ryukei_testkit::mesh::deviation(&body, &m, 4, 40);
            assert!(d <= chord, "{chord} m, {angle} rad: {d}");
        }
    }
    eprintln!("{:?}", start.elapsed());
    assert!(
        !bars() || start.elapsed().as_secs() < 300,
        "{:?}",
        start.elapsed()
    );
}

/// A profile swept along a path (issue #222, from the `sweep` target): it
/// lies wholly beyond the path's centre of curvature, so its section runs
/// backwards all along. It was built inside out (its loops laid out for a
/// section moving forwards, its faces' senses taken from the geometry),
/// which `check` doesn't see on curved faces, and four faces stalled the
/// mesher. It is refused as `PathTooCurved`, as a fold is.
#[test]
fn a_sweep_carried_backwards_whole_is_refused() {
    let data = bytes("3d01ff00e5000e00000000000000002b");
    let (made, _, along) = ryukei_testkit::along_bytes::outcome(&data).expect("a sweep");
    assert!(along);
    assert!(
        matches!(
            made,
            Err(ryukei::Error::InvalidSweep(
                ryukei::SweepError::PathTooCurved { .. }
            ))
        ),
        "{made:?}"
    );
}

/// The sweep fuzz target's timeout, in seconds: a find past it ran away.
const RUNAWAY: f64 = 600.0;

/// #260's two sweep finds, as the sweep fuzz target read them.
const RUNAWAY_SWEEPS: [&str; 2] = [
    // sweep/timeout-1c7ea944cd423ea6d97ae931e51c0305782a55ed
    concat!(
        "0adfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf0000000000000008dfdfdfdfdfdfdfdfdfdfdf",
        "dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf",
        "dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf",
    ),
    // sweep/timeout-a73041963ed854e33b70395d83be635a32f8982c
    concat!(
        "8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8caf8c8c8c8c8c8c8c8c8c8c8c8c",
        "8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c",
        "8c8c8c8c8c8c8c8c8c8c0a0a",
    ),
];

/// Whether this file's time limits are asserted: only with
/// `RYUKEI_BARS=1`, on the machine they were recorded on, since one case's
/// time varies 7 to 160 times between CPUs and with load (plan 15.C's
/// review: a Xeon E5-2650 and an Opteron 6380 passed the 300 s limits
/// below). Times are always printed; what bounds a case on every machine
/// is the kernel's caps, in units (`runaway_finds_end_under_the_caps`).
fn bars() -> bool {
    std::env::var("RYUKEI_BARS").as_deref() == Ok("1")
}

/// Held by the tests that time themselves, so in a bars run none shares
/// the machine with a sibling.
static TIMED: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn timed() -> std::sync::MutexGuard<'static, ()> {
    TIMED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// #260's two sweep finds build and check clean, each under 30 s on one
/// thread (cox-worker-2: about 6 and 2 s), not refused at a cap: the
/// second's cone legs part at their straight section (plan 15.C).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn the_runaway_sweeps_build_and_check_clean() {
    let _timed = timed();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    for hex in RUNAWAY_SWEEPS {
        let s = pool.install(|| {
            let t = std::time::Instant::now();
            let (made, _, _) = ryukei_testkit::along_bytes::outcome(&bytes(hex)).expect("a sweep");
            let body = made.expect("builds");
            assert_eq!(check(&body), []);
            assert_eq!(check_intersections(&body), []);
            t.elapsed().as_secs_f64()
        });
        eprintln!("{}…: {s:.1} s", &hex[..8]);
        assert!(!bars() || s < 30.0, "{}: {s} s", &hex[..8]);
    }
}

/// #260's two sweep finds, whose self-intersection check ran past 600 s,
/// and the boolean whose trace's clusters did: each ends under the
/// kernel's caps, built (and sound) or refused as uncertain. What ends
/// them is the caps' units, the same on every machine; the seconds are
/// printed, and held only under the sweep target's timeout (`RUNAWAY`),
/// so a slow or loaded machine doesn't fail it. The bytes are the fuzz
/// targets' own (branch `cox/ryukei-8f-fuzz-finds-CABN934E`,
/// `fuzz/finds`), read as the targets read them. Release only: in a debug build a cap takes minutes to spend.
#[cfg_attr(debug_assertions, ignore = "release only")]
#[test]
fn runaway_finds_end_under_the_caps() {
    use ryukei::{BooleanError, Error};
    use std::time::Instant;
    for hex in RUNAWAY_SWEEPS {
        let t = Instant::now();
        let data = bytes(hex);
        if let Some((body, _)) = sweep(&data) {
            assert_eq!(check(&body), []);
        }
        if let Some((made, _, _)) = ryukei_testkit::along_bytes::outcome(&data) {
            match made {
                Ok(body) => {
                    assert_eq!(check(&body), []);
                    for f in check_intersections(&body) {
                        assert!(
                            matches!(f, ryukei::check::Fault::CannotCheck { .. }),
                            "{f:?}"
                        );
                    }
                }
                Err(e) => assert!(
                    matches!(
                        e,
                        Error::InvalidSweep(_)
                            | Error::InvalidLoft(_)
                            | Error::Degenerate
                            | Error::NonFinite
                    ),
                    "{e:?}"
                ),
            }
        }
        let s = t.elapsed().as_secs_f64();
        eprintln!("{}…: {s:.1} s", &hex[..8]);
        assert!(s < RUNAWAY, "{s} s");
    }
    // boolean/timeout-62012e0e3b836318f71aaacd91893e13589b15cc: its first
    // byte, odd, takes the rest as a general case.
    let t = Instant::now();
    let data = bytes(concat!(
        "01000000000000090a00000000ffffffffffffffffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffffffffffffffff0000000000000000000000000000",
        "000000000500000000",
    ));
    let case = ryukei_testkit::boolean_bytes::general_case(&data[1..]).expect("operands");
    match boolean(OpKey(9), case.op, &case.target, &case.tools()) {
        Ok((body, _)) => assert_eq!(check(&body), []),
        Err(e) => assert!(
            matches!(
                e,
                Error::InvalidBoolean(
                    BooleanError::Uncertain { .. }
                        | BooleanError::Unsupported { .. }
                        | BooleanError::NonManifold { .. }
                )
            ),
            "{e:?}"
        ),
    }
    let s = t.elapsed().as_secs_f64();
    eprintln!("boolean: {s:.1} s");
    assert!(s < RUNAWAY, "{s} s");
}

/// The `sheet_metal` target's slow input (cox-worker-4, plan 11.D's fuzz
/// hour: 1,225 s under the fuzzer's instrumentation at a load of 43):
/// `0x0f` and 95 bytes of `0xfa`, a flanged plate cut across two bend
/// lines. Its round trip holds; what took the time is the judge's full
/// self-intersection check of the refolded parts (about 10 s on one
/// thread in release). Under a fuzz case's watchdog it ends at the
/// deadline with nothing found, and in release the whole judge passes.
#[test]
fn the_slow_sheet_metal_input_is_bounded_and_holds() {
    let _timed = timed();
    use ryukei_testkit::sheet_metal_bytes::{case, judge, judge_within, trip};
    use std::time::{Duration, Instant};
    let mut data = vec![0x0f];
    data.extend([0xfa; 95]);
    let c = case(&data).expect("enough bytes").expect("a part");
    let r = trip(&c).expect("a round trip");
    let t = Instant::now();
    let deadline = Duration::from_millis(500);
    let within = ryukei_testkit::fuzz_case_within(deadline, |w| judge_within(&c, &r, w));
    assert_eq!(within, Ok(()));
    let s = t.elapsed().as_secs_f64();
    eprintln!("sheet_metal within {deadline:?}: {s:.1} s");
    assert!(!bars() || s < 60.0, "{s} s");
    if !cfg!(debug_assertions) {
        assert_eq!(judge(&c, &r), Ok(()));
    }
}

/// The `sweep` target's committed seed `9e6a35a6` (issue #359): a lone
/// periodic spline carried along an arc. Placing the profile cut its whole
/// period to a clamped spline whose ends meet, so the side was a tube
/// closing in no direction, which the mesher refuses (`BadDomain`). The
/// side's profile is periodic again, and the body meshes as the target
/// meshes it.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in a debug build: run it in release")]
fn a_lone_periodic_spline_carried_round_an_arc_meshes() {
    let data = [
        0x3d, 0x01, 0xff, 0x00, 0xa7, 0xe5, 0x00, 0x0e, 0, 0, 0, 0, 0, 0, 0, 0, 0x2b,
    ];
    let (body, small, along) = ryukei_testkit::along_bytes::carried(&data).expect("a body");
    assert!(along);
    assert_eq!(check(&body), []);
    assert_eq!(check_intersections(&body), []);
    let params = MeshParams {
        max_triangles: 20_000,
        ..MeshParams::new((small / 8.0).clamp(1e-3, 0.05), 0.5)
    };
    let m = tessellate(&body, &params).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(&body, &m), Ok(()));
}

/// #260's second sweep find: neighbouring legs' cone sides tangent along a
/// straight section. Neither keeps off the other's surface, and the
/// section plane was only a circle's, an ellipse's or a planar spline's;
/// the plane through the line square to their tangent plane parts them.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn neighbouring_legs_part_at_their_straight_section() {
    let data = bytes(concat!(
        "8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8caf8c8c8c8c8c8c8c8c8c8c8c8c",
        "8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c",
        "8c8c8c8c8c8c8c8c8c8c0a0a",
    ));
    let (made, _, _) = ryukei_testkit::along_bytes::outcome(&data).expect("a sweep");
    let body = made.expect("builds: its cone legs part at their section");
    assert_eq!(check(&body), []);
    assert_eq!(check_intersections(&body), []);
}
