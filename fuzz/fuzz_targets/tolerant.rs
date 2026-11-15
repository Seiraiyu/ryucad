//! Booleans and blends on tolerant twins from bytes
//! ([`ryukei_testkit::tolerant_bytes`], design 299): an even first byte
//! takes the rest as a boolean (`tolerant_case`), an odd one as a blend
//! (`tolerant_blend_case`). The case is
//! [`ryukei_testkit::tolerant_bytes::tolerant_run`], which the release
//! regression tests run too: every result must be a structured refusal (a
//! boolean uncertain, unsupported, non-manifold or past the tolerance cap;
//! any blend refusal; the fuzz case's budget or deadline), or a body that
//! passes the checker, doesn't cross or overlap itself, and meshes with no
//! faults. Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    ryukei_testkit::tolerant_bytes::tolerant_run(data);
});
