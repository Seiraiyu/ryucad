
## The material library in Git (decided in ryucad phase 10)

The design is document 198 in the Coxswain planning project `ryucad-ryukei`; this is its phase 2, plan 10.A.

- **Attributes.** `drivers::LINES` gains `materials.json merge=ryucad diff=ryucad text eol=lf`; existing projects report `gitattributes_missing` until doctor's repair adds it. The line moved the `diff` golden section's hash (it textconvs `.gitattributes`). Appearance, views and explodes (plans 10.C and 10.D) live under `parts/**/*.json`, which the existing line covers; a root `parameters.json` (plan 10.E) will need a line of its own.
- **Textconv, merge, diff.** Textconv gives each material one line (`materials.steel = density 7850 kg/m^3, colour #888888`) and the default its own; the merge driver merges the library by material and field (`merge::newest` knows its format, so a newer one merges as lines); `diff` lists library changes by pointer at the top level (`materials.changes`), never under `--model`, since no result changes.
- **Testing.** `diff.rs` and `merge.rs` unit tests, `drivers.rs`'s line counts, the `merge` fuzz target's `0xfd` mode (densities set, materials removed, the default moved on either side).
