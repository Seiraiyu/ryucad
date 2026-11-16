//! Self-union from bytes (design 299, plan 14.C): crossing shells of
//! placed primitives ([`ryukei_testkit::crossing`]) healed with repair,
//! given to `self_union` where healing left them crossing, and the result
//! given to `self_union` again. The case is
//! [`ryukei_testkit::crossing::self_union_run`], which the release
//! regression tests run too: every result must be a typed refusal
//! (healing's, a repair's, or the fuzz case's budget or deadline) or a
//! body that passes the checker, doesn't cross or overlap itself, meshes
//! with no faults, and comes back from `self_union` as it was. Any panic,
//! and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    ryukei_testkit::crossing::self_union_run(data);
});
