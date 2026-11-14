//! The scripted fixture's shape (plan 31.C), and seeded batches against
//! their ops one by one (Task 15).

mod common;

#[test]
fn the_scripted_project_is_810s_size() {
    let s = common::scripted::Script::new(56, 209);
    let instances: usize = s
        .parts
        .iter()
        .filter(|p| p.kind == "assembly")
        .map(|p| p.features.len())
        .sum();
    assert_eq!(instances, 209);
    assert_eq!(s.parts.len(), 56 + 4 + 1);
    let singles = s.singles().len();
    assert!((900..1_200).contains(&singles), "{singles} single requests");
    assert_eq!(s.batch()["ops"].as_array().unwrap().len(), 3 + 61);
}
mod batch_bytes;

/// Seeded batches against their ops one by one (the fuzz target `batch`'s
/// check): 24 in a debug build, 200 in a release one.
#[test]
fn seeded_batches_write_what_their_ops_write_one_by_one() {
    let mut g = ryukei_testkit::Lcg::new(0x31c);
    let cases = if cfg!(debug_assertions) { 24 } else { 200 };
    for _ in 0..cases {
        let n = 2 + (g.next_u64() % 15) as usize;
        let data: Vec<u8> = (0..n).map(|_| g.next_u64() as u8).collect();
        batch_bytes::check(&data);
    }
}
