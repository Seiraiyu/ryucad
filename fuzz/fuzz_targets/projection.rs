//! Views and sections from bytes (design 326, plan 10.C): the first byte
//! a view direction on a coarse grid
//! ([`ryukei_testkit::projection::grid_direction`]), the second a section
//! plane's, the rest a body as the blend target makes them
//! ([`ryukei_testkit::blend_bytes::body`]). Every view is a typed
//! refusal or a projection that is the same twice, whose pieces the
//! mesh oracle doesn't contradict and whose silhouettes are square to
//! the view; every section is a typed refusal or a body that passes the
//! checker with caps on the plane. Calls share one
//! [`ryukei_testkit::FUZZ_BUDGET`] and are cancelled at
//! [`ryukei_testkit::FUZZ_DEADLINE`]. Any panic, and any other error, is
//! a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ryukei_testkit::projection::check_bytes(data));
