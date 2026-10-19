//! Random sequences of Euler operators from a box: valid ones keep the
//! body valid, inverses undo exactly, and wild ones fail cleanly.

use proptest::collection::vec;
use proptest::prelude::*;
use ryukei::check::{check, check_topology};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::euler_script::{Bytes, Pool, start, valid_step, wild_step};

/// The body after `compact`, without names.
fn shape(b: &Body) -> String {
    let mut b = b.clone();
    b.compact();
    b.dump_unnamed()
}

/// The body after `compact`, names included.
fn exact(b: &Body) -> String {
    let mut b = b.clone();
    b.compact();
    b.dump()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn valid_sequences_stay_valid(data in vec(any::<u8>(), 0..2000)) {
        let mut b = start();
        let mut bytes = Bytes::new(&data);
        let mut n = 0u128;
        while !bytes.is_empty() {
            n += 1;
            if let Some(step) = valid_step(&mut b, OpKey(1000 + n), &mut bytes) {
                // New fins have no pcurves until someone gives them theirs.
                ryukei::pcurve::fill(&mut b).unwrap();
                let faults = check(&b);
                prop_assert!(faults.is_empty(), "after {} (step {n}): {faults:?}", step.op);
            }
        }
    }

    #[test]
    fn inverses_undo_exactly(data in vec(any::<u8>(), 0..2000)) {
        let mut b = start();
        let mut bytes = Bytes::new(&data);
        let mut n = 0u128;
        while !bytes.is_empty() {
            n += 1;
            let before = shape(&b);
            if let Some(step) = valid_step(&mut b, OpKey(1000 + n), &mut bytes)
                && let Some(undo) = step.undo
            {
                let mut undone = b.clone();
                undo.apply(&mut undone, OpKey(1)).unwrap();
                prop_assert_eq!(shape(&undone), before, "{} then {:?}", step.op, undo);
            }
        }
    }

    #[test]
    fn wild_sequences_fail_cleanly(data in vec(any::<u8>(), 0..2000)) {
        let mut b = start();
        let mut bytes = Bytes::new(&data);
        let mut pool = Pool::default();
        let mut n = 0u128;
        while !bytes.is_empty() {
            n += 1;
            let before = exact(&b);
            let (op, result) = wild_step(&mut b, OpKey(1000 + n), &mut bytes, &mut pool);
            match result {
                Err(_) => prop_assert_eq!(exact(&b), before, "{} failed but changed the body", op),
                Ok(()) => prop_assert_eq!(check_topology(&b), [], "after {}", op),
            }
        }
    }
}

#[test]
fn every_operator_runs() {
    // A fixed long sequence reaches every operator at least once.
    let data: Vec<u8> = {
        let mut g = ryukei_testkit::Lcg::new(0x6575_6c72);
        (0..20_000).map(|_| (g.next_u64() >> 56) as u8).collect()
    };
    let mut b = start();
    let mut bytes = Bytes::new(&data);
    let mut seen = std::collections::BTreeSet::new();
    let mut n = 0u128;
    while !bytes.is_empty() {
        n += 1;
        if let Some(step) = valid_step(&mut b, OpKey(1000 + n), &mut bytes) {
            seen.insert(step.op);
        }
    }
    for op in ryukei_testkit::euler_script::OPERATORS {
        assert!(seen.contains(op), "{op} never ran: {seen:?}");
    }
    ryukei::pcurve::fill(&mut b).unwrap();
    assert_eq!(check(&b), []);
}
