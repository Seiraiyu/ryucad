## Numerics (decided in phase 0)

The design is document 12 in the Coxswain planning project `ryucad-ryukei`.

- **Units and tolerances.** Model space is SI meters. `ryukei::tol` fixes `LINEAR_RESOLUTION` = 1e-8 m, `ANGULAR_RESOLUTION` = 1e-11 rad and a size box of ±500 m for every body. Per-entity `Tolerance` values are never below the resolution. Unit conversion and larger models are ryucad's job.
- **Precision policy.** Decisions that change topology or classification (side of a plane, inside a circle or sphere, ordering) use exact or certified predicates (`ryukei::predicates`, `ryukei::interval`), never a raw float sign. Geometric values (coordinates, parameters, distances, constructed points) are f64 compared through `tol`, never with `==`. Predicates take their f64 inputs as exact. An uncertain interval sign must be escalated or resolved by an explicit tolerance rule, never guessed.
- **Determinism.** Kernel code gets transcendental functions only from `ryukei::math`; `clippy.toml` bans the `f64` versions and `mul_add`. `tests/golden/determinism.txt` holds the bit-exact hashes that every CI platform and CPU level must reproduce. When you change results on purpose, regenerate it and review the diff.
