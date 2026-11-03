//! Scratch for planning 15.C (never merged): times #260's sweep finds'
//! outcomes and their checks, with per-pair lines under RYUKEI_CHECK_PROBE.
use ryukei::check::check_intersections;
use std::time::Instant;

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn finds_260() {
    let sweeps = [
        concat!(
            "0adfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf0000000000000008dfdfdfdfdfdfdfdfdfdfdf",
            "dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf",
            "dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf",
        ),
        concat!(
            "8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8caf8c8c8c8c8c8c8c8c8c8c8c8c",
            "8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c",
            "8c8c8c8c8c8c8c8c8c8c0a0a",
        ),
    ];
    for hex in sweeps {
        let data = bytes(hex);
        let t = Instant::now();
        let out = ryukei_testkit::along_bytes::outcome(&data);
        eprintln!("{}: outcome in {:.1} s", &hex[..8], t.elapsed().as_secs_f64());
        if let Some((made, _, _)) = out {
            match made {
                Ok(body) => {
                    let t = Instant::now();
                    let f = check_intersections(&body);
                    eprintln!("{}: built, {} faces, check {:.1} s, {:?}", &hex[..8], body.faces().count(), t.elapsed().as_secs_f64(), f);
                }
                Err(e) => eprintln!("{}: refused {e:?}", &hex[..8]),
            }
        }
    }
}
