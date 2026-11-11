//! Each body of a STEP file with blends under 2 mm, its small blends
//! deleted, chain by chain or whole (`--whole`), under a cancel: what
//! each refusal names (the face's surface kind and loop count), and how
//! long it took. `R16C_BODY=<n>` and `R16C_CHAIN=<n>` pick one.
//!
//!   cargo run --release -p ryustep --example blend-probe -- <file> [seconds] [--whole]

use ryukei::Error;
use ryukei::blend::{BlendKind, Chamfer};
use ryukei::name::{Name, OpKey};
use ryukei::topo::{Body, EntityId};
use ryukei::tweak::{
    DeleteBlends, RecogniseOptions, Recognised, delete_blends_with, recognise_blends,
};
use ryukei::work::Work;
use ryustep::{ReadOptions, read};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn size(k: &BlendKind) -> f64 {
    match *k {
        BlendKind::Fillet { radius } => radius,
        BlendKind::Chamfer(Chamfer::Equal { distance }) => distance,
        BlendKind::Chamfer(Chamfer::Distances { first, second, .. }) => first.max(second),
        BlendKind::Chamfer(Chamfer::DistanceAngle {
            distance, angle, ..
        }) => distance.max(distance * ryukei::math::tan(angle)),
    }
}

/// An entity named `n` of `b`, in a few words.
fn what(b: &Body, n: Name) -> String {
    match b.by_name(n) {
        Some(EntityId::Face(f)) => {
            let s = format!("{:?}", b.surface(b.face(f).unwrap().surface).unwrap());
            let kind: String = s.chars().take_while(|c| c.is_alphanumeric()).collect();
            format!("face {n:?} {kind}, {} loops", b.face_loops(f).count())
        }
        Some(EntityId::Edge(_)) => format!("edge {n:?}"),
        Some(EntityId::Vertex(v)) => format!("vertex {n:?} at {:?}", b.vertex(v).unwrap().point),
        _ => format!("{n:?}, not in the body"),
    }
}

/// `f` under a cancel set `secs` seconds in.
fn deadline<T>(secs: u64, f: impl FnOnce(&Work) -> Result<T, Error>) -> Result<T, Error> {
    let flag = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    let start = Instant::now();
    std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(secs) {
                std::thread::sleep(Duration::from_millis(100));
            }
            flag.store(true, Ordering::Relaxed);
        });
        let r = f(&Work::unlimited().with_cancel(&flag));
        done.store(true, Ordering::Relaxed);
        r
    })
}

/// The refusal, with what its name is in `b`.
fn refusal(b: &Body, e: &Error) -> String {
    let named = match e {
        Error::InvalidTweak(t) => {
            let s = format!("{t:?}");
            s.find("Name(")
                .and_then(|i| u128::from_str_radix(&s[i + 5..i + 37], 16).ok())
                .map(|v| what(b, Name::from_u128(v)))
        }
        _ => None,
    };
    format!("{e:?} [{}]", named.unwrap_or_default())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(120);
    let whole = args.iter().any(|a| a == "--whole");
    let pick = |v: &str| std::env::var(v).ok().and_then(|s| s.parse::<usize>().ok());
    let (only_body, only_chain) = (pick("R16C_BODY"), pick("R16C_CHAIN"));
    let options = ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    };
    let m = read(&std::fs::read(path).unwrap(), &options).unwrap();
    for (i, imported) in m.bodies.iter().enumerate() {
        if only_body.is_some_and(|o| o != i) {
            continue;
        }
        let Ok((body, _, _)) = &imported.result else {
            continue;
        };
        let r = match recognise_blends(body, &RecogniseOptions::default()) {
            Ok(r) => r,
            Err(e) => {
                println!("#{i}: recognition {e:?}");
                continue;
            }
        };
        let small: Vec<&Recognised> = r.blends.iter().filter(|x| size(&x.kind) <= 2e-3).collect();
        if small.is_empty() {
            continue;
        }
        println!(
            "#{i}: {} faces, {} blends under 2 mm of {}",
            body.faces().count(),
            small.len(),
            r.blends.len()
        );
        let mut chains: BTreeMap<usize, Vec<&Recognised>> = BTreeMap::new();
        for b in &small {
            chains.entry(b.chain).or_default().push(b);
        }
        let mut runs: Vec<(String, Vec<Name>)> = Vec::new();
        if whole {
            runs.push(("whole".into(), Vec::new()));
        } else {
            for (c, list) in &chains {
                if only_chain.is_some_and(|o| o != *c) {
                    continue;
                }
                runs.push((
                    format!("chain {c}"),
                    list.iter().flat_map(|b| b.faces.clone()).collect(),
                ));
            }
        }
        for (label, faces) in runs {
            let spec = DeleteBlends {
                max_size: faces.is_empty().then_some(2e-3),
                faces,
                options: RecogniseOptions::default(),
            };
            let t = Instant::now();
            let w = deadline(secs, |w| delete_blends_with(OpKey(0x13d), body, &spec, w));
            let s = t.elapsed().as_secs_f64();
            match w {
                Ok((o, _)) => println!(
                    "  {label}: {} faces, check {} faults, {} pairs, in {s:.2} s",
                    o.faces().count(),
                    ryukei::check::check(&o).len(),
                    ryukei::check::check_intersections(&o).len()
                ),
                Err(e) => println!("  {label}: {} in {s:.2} s", refusal(body, &e)),
            }
        }
    }
}
