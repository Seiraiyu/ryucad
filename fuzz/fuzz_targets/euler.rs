//! Euler operator sequences played from bytes, starting from a box. The
//! first byte picks the mode: valid steps must keep the full check clean,
//! wild steps must fail without changing the body or succeed with sound
//! topology. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::check::{check, check_topology};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::euler_script::{Bytes, Pool, start, valid_step, wild_step};

fn exact(b: &Body) -> String {
    let mut b = b.clone();
    b.compact();
    b.dump()
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let mut b = start();
    let mut bytes = Bytes::new(rest);
    let mut pool = Pool::default();
    let mut n = 0u128;
    while !bytes.is_empty() {
        n += 1;
        let key = OpKey(1000 + n);
        if mode % 2 == 0 {
            if let Some(step) = valid_step(&mut b, key, &mut bytes) {
                ryukei::pcurve::fill(&mut b).unwrap();
                let faults = check(&b);
                assert!(faults.is_empty(), "after {}: {faults:?}", step.op);
            }
        } else {
            let before = exact(&b);
            let (op, result) = wild_step(&mut b, key, &mut bytes, &mut pool);
            match result {
                Err(_) => assert_eq!(exact(&b), before, "{op} failed but changed the body"),
                Ok(()) => assert_eq!(check_topology(&b), [], "after {op}"),
            }
        }
    }
});
