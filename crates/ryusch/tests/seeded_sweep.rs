//! The `schematic` fuzz target's case over 400 seeded inputs of 128 bytes
//! (plan 2.G): every one a typed refusal or a layout keeping every hard
//! rule, stable under an edit. Plans 2.A to 2.C each grew `check_bytes`
//! (regions and papers past byte 48, networks and turns past byte 80,
//! buses and notes past byte 112), and no fuzzing ran after 2.B: on main
//! at f06230c3, 18 of these 400 failed (#664, #665). Release only (about
//! a minute on cox-worker-2); its own step in full.yml.

use ryusch::testing::check_bytes;

/// Input `seed`'s 128 bytes: xorshift64 from a fixed mix of the seed.
fn input(seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    (0..128)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// The panic's first line naming what broke.
fn why(e: &(dyn std::any::Any + Send)) -> String {
    let m = e
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default();
    m.lines()
        .find(|l| l.contains("message") || l.contains("moved") || l.contains("left"))
        .unwrap_or(m.lines().next().unwrap_or(""))
        .trim()
        .to_owned()
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn four_hundred_seeded_inputs_keep_every_hard_rule() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let failed: Vec<String> = (0..400)
        .filter_map(|seed| {
            let data = input(seed);
            std::panic::catch_unwind(|| check_bytes(&data))
                .err()
                .map(|e| format!("seed {seed}: {}", why(&*e)))
        })
        .collect();
    std::panic::set_hook(hook);
    assert!(
        failed.is_empty(),
        "{} of 400:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
