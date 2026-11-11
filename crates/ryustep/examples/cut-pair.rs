//! Cuts a pair of faces out of a STEP file's body, with every face that
//! shares an edge with either, as a sheet, and writes it as
//! `ryukei::codec` bytes: a public-domain body's pair kept as a test's
//! input (#699). The body is read as ryustep's tests read it (`OpKey(7)`).
//!
//! ```text
//! cargo run --release -p ryustep --example cut-pair -- <file.stp> <representation> <face> <face> <out.body>
//! ```
//!
//! Faces are named in hex, as `check_intersections`' faults print them.

use ryukei::name::{Name, OpKey};
use ryukei::topo::FaceId;
use ryustep::ReadOptions;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, path, rep, a, b, out] = &args[..] else {
        eprintln!("usage: cut-pair <file.stp> <representation> <face> <face> <out.body>");
        std::process::exit(2);
    };
    let rep: u64 = rep.parse().expect("a representation number");
    let opts = ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    };
    let model = ryustep::read(&std::fs::read(path).expect("the file reads"), &opts).expect("a model");
    let body = model
        .bodies
        .into_iter()
        .find(|x| x.representation == rep)
        .expect("the representation")
        .result
        .expect("a healed body")
        .0;
    let named = |s: &str| {
        let n = Name::from_u128(u128::from_str_radix(s, 16).expect("a hex name"));
        body.faces()
            .find(|&f| body.face(f).expect("live").name == n)
            .expect("the face")
    };
    let mut faces: Vec<FaceId> = vec![named(a), named(b)];
    for f in faces.clone() {
        for l in body.face_loops(f) {
            for fin in body.loop_fins(l) {
                let e = body.fin(fin).expect("live").edge;
                for g in body.edge_fins(e) {
                    let face = body.loop_(body.fin(g).expect("live").loop_).expect("live").face;
                    if !faces.contains(&face) {
                        faces.push(face);
                    }
                }
            }
        }
    }
    let names: Vec<Name> = faces.iter().map(|&f| body.face(f).expect("live").name).collect();
    let (sheet, _) = ryukei::sheet::extract(OpKey(7), &body, &names).expect("a sheet");
    std::fs::write(out, ryukei::codec::encode_body(&sheet)).expect("written");
    println!("{out}: {} faces", sheet.faces().count());
}
