use ryukei::name::OpKey;
use ryustep::{ReadOptions, read};

#[test]
fn scratch() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/nist_ctc_05_asme1_rd.stp");
    let model = read(&std::fs::read(path).unwrap(), &ReadOptions { key: OpKey(7), heal: Default::default() }).unwrap();
    let body = model.bodies.into_iter().find_map(|b| b.result.ok()).unwrap().0;
    let want = [0x2eaaf60a1bd1cc3c8c98e1c854ef373cu128, 0x57ba94795f85c200c554dc002e64a0c2];
    let faces: Vec<_> = body.faces().filter(|&f| want.contains(&body.face(f).unwrap().name.to_u128())).collect();
    for &f in &faces {
        let x = body.face(f).unwrap();
        let s = body.surface(x.surface).unwrap();
        eprintln!("SCRATCH face {f:?} {:?} box {:?}", format!("{s:?}").chars().take(300).collect::<String>(), ryukei::intersect::face_box(&body, f));
        let edges: Vec<_> = body.face_loops(f).flat_map(|l| body.loop_fins(l).map(|fin| body.fin(fin).unwrap().edge).collect::<Vec<_>>()).collect();
        eprintln!("SCRATCH   edges {edges:?}");
    }
    let t = std::time::Instant::now();
    let r = ryukei::intersect::face_face((&body, faces[0]), (&body, faces[1]));
    eprintln!("SCRATCH face_face {:?} in {:?}", r.as_ref().map(|_| "ok"), t.elapsed());
}
