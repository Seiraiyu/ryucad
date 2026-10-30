use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::name::OpKey;
use ryukei_testkit::blend_bytes::spline_case;
use ryukei_testkit::boolean_bytes::general_case;

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn scratch_boolean() {
    let case = general_case(&bytes("000000000000090a00000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0000000000000000000000000000000000000500000000")).unwrap();
    let t = std::time::Instant::now();
    let r = boolean(OpKey(9), case.op, &case.target, &case.tools());
    eprintln!("SCRATCH boolean {:?} in {:?}", r.as_ref().map(|b| b.0.counts()), t.elapsed());
    if let Ok((b, _)) = r {
        let t = std::time::Instant::now();
        let f = ryukei::check::check_intersections(&b);
        eprintln!("SCRATCH boolean check {:?} in {:?}", f, t.elapsed());
    }
}

#[test]
fn scratch_blend() {
    let case = spline_case(&bytes("4e4f4f4f4f4f4f4f4f4f4f4f4f4f4ffc4f4f4f4f204f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f0000004f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f0000004f584f4f200000004f4f4f4f4f4f4f0a")).unwrap();
    let t = std::time::Instant::now();
    let r = blend(OpKey(9), &case.body, &case.blends);
    eprintln!("SCRATCH blend {:?} in {:?}", r.as_ref().map(|b| b.0.counts()), t.elapsed());
    if let Ok((b, _)) = r {
        let m = ryukei::mesh::tessellate(&b, &ryukei::mesh::MeshParams::new(1e-2, 0.5)).unwrap();
        eprintln!("SCRATCH blend mesh faults {:?}", m.faults);
        for f in b.faces() {
            let x = b.face(f).unwrap();
            eprintln!("SCRATCH face {f:?} {} box {:?}", format!("{:?}", b.surface(x.surface).unwrap()).chars().take(120).collect::<String>(), ryukei::intersect::face_box(&b, f));
        }
        for (chord, angle) in [(1e-3, 0.5), (1e-2, 0.3), (3e-2, 0.5), (1e-2, 1.0)] {
            let m = ryukei::mesh::tessellate(&b, &ryukei::mesh::MeshParams::new(chord, angle)).unwrap();
            eprintln!("SCRATCH mesh {chord} {angle}: {:?}", m.faults);
        }
    }
}
