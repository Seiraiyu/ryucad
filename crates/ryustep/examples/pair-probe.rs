//! Dumps the face pairs the self-intersection check can't settle, for
//! diagnosing them: each body healed as `check-split` heals it (ryucad's
//! import key, or `CHECK_SPLIT_KEY`), checked on one thread, and for each
//! fault both faces' surfaces and edges (index, tolerance, the faces
//! using it, ends and middle in millimetres, curve kind). A refused body
//! names the refusal's faces by STEP instance and dumps them as the
//! reference body has them.
//!
//! ```text
//! cargo run --release -p ryustep --example pair-probe -- <file.step> [body index]
//! ONLY_REP=11774 …   one representation by instance number
//! PAIR=4,10 …        `face_face` on two faces of the healed body
//! ```

use ryukei::check::Fault;
use ryukei::name::OpKey;
use ryukei::topo::{Body, FaceId};
use ryustep::ReadOptions;
use std::time::Instant;

fn short<T: std::fmt::Debug>(t: &T, n: usize) -> String {
    format!("{t:?}").chars().take(n).collect()
}

fn face_line(body: &Body, f: FaceId) {
    let face = body.face(f).unwrap();
    println!(
        "  face {} {:?} surface {}",
        f.index(),
        face.sense,
        short(body.surface(face.surface).unwrap(), 90)
    );
    for l in body.face_loops(f) {
        for fin in body.loop_fins(l) {
            let e = body.fin(fin).unwrap().edge;
            let edge = body.edge(e).unwrap();
            let c = body.curve(edge.curve).unwrap();
            let [t0, t1] = edge.interval;
            let mm = |t: f64| {
                let p = c.point(t);
                format!("[{:.6},{:.6},{:.6}]", p.x * 1e3, p.y * 1e3, p.z * 1e3)
            };
            let users: Vec<usize> = body
                .edge_fins(e)
                .map(|g| body.loop_(body.fin(g).unwrap().loop_).unwrap().face.index())
                .collect();
            println!(
                "    edge {} tol {:.2e} faces {users:?} {}→{} mid {} {}",
                e.index(),
                edge.tolerance.get(),
                mm(t0),
                mm(t1),
                mm(0.5 * (t0 + t1)),
                short(c, 40)
            );
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let only: Option<usize> = args.get(2).map(|s| s.parse().expect("a body index"));
    let rep: Option<u64> = std::env::var("ONLY_REP")
        .ok()
        .map(|s| s.parse().expect("an instance"));
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("a pool");
    let bytes = std::fs::read(path).expect("the file reads");
    let opened = ryustep::open(&bytes).expect("the file opens");
    let opts = ReadOptions {
        key: match std::env::var("CHECK_SPLIT_KEY") {
            Ok(k) => OpKey(k.parse().expect("a key")),
            Err(_) => OpKey::from_bytes(b"ryucad.feature\0body\0body"),
        },
        heal: Default::default(),
    };
    for k in 0..opened.held.len() {
        if only.is_some_and(|o| o != k) || rep.is_some_and(|r| r != opened.held[k].representation) {
            continue;
        }
        let t = Instant::now();
        let read = one.install(|| opened.body(k, &opts));
        let Ok((body, _, _)) = &read.result else {
            let err = read.result.as_ref().err();
            println!(
                "#{k}: refused {err:?} in {:.1} s",
                t.elapsed().as_secs_f64()
            );
            let text = format!("{err:?}");
            let names: Vec<ryukei::name::Name> = read
                .instances
                .keys()
                .copied()
                .filter(|n| text.contains(&format!("{n:?}")))
                .collect();
            for n in &names {
                println!("  {n:?} is #{}", read.instances[n]);
            }
            if let Ok((rb, _)) = &opened.reference(k, &opts).result {
                let faces: Vec<FaceId> = names
                    .iter()
                    .filter_map(|n| match rb.by_name(*n) {
                        Some(ryukei::topo::EntityId::Face(f)) => Some(f),
                        _ => None,
                    })
                    .collect();
                for &f in &faces {
                    face_line(rb, f);
                }
            }
            continue;
        };
        println!(
            "#{k}: healed in {:.1} s, {} faces",
            t.elapsed().as_secs_f64(),
            body.counts().faces
        );
        if let Ok(pair) = std::env::var("PAIR") {
            let ids: Vec<usize> = pair
                .split(',')
                .map(|x| x.parse().expect("a face index"))
                .collect();
            // By `FaceId` index, as the faults name them.
            let face = |i: usize| body.faces().find(|f| f.index() == i).expect("a face");
            let (fa, fb) = (face(ids[0]), face(ids[1]));
            for f in [fa, fb] {
                face_line(body, f);
            }
            let r = ryukei::intersect::face_face((body, fa), (body, fb));
            println!("face_face {pair}: {}", short(&r, 900));
            continue;
        }
        let t = Instant::now();
        let work = ryukei::work::Work::unlimited();
        let faults = one
            .install(|| ryukei::check::check_intersections_with(body, &work))
            .expect("no budget");
        println!(
            "#{k}: check {:.1} s, {} units, {} faults",
            t.elapsed().as_secs_f64(),
            work.spent(),
            faults.len()
        );
        for fault in &faults {
            println!("{fault:?}");
            if let Fault::CannotCheck { a, b }
            | Fault::FacesCross { a, b }
            | Fault::FacesOverlap { a, b } = *fault
            {
                face_line(body, a);
                face_line(body, b);
            }
        }
    }
}
