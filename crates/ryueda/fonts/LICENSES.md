# Fonts in ryueda

ryueda draws schematics in one face (design 236): SVG names it, PNGs are
rasterized with it, and from plan 1.B PDFs embed a subset of it. `ryusch`
reads only its advance widths, from `crates/ryusch/src/widths.rs`, which
`tests/font_widths.rs` generates from this file and checks against it.

| File | What | Version | Licence | Text |
|---|---|---|---|---|
| `Inter-Regular.ttf` | Inter Regular, static (`extras/ttf/Inter-Regular.ttf` of the release archive) | 4.1 (rsms/inter) | SIL Open Font License 1.1 | `Inter-LICENSE.txt` |

The archive is `https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip`
(SHA-256 `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`);
`Inter-Regular.ttf` is SHA-256
`40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82` and
`Inter-LICENSE.txt` (the archive's `LICENSE.txt`) SHA-256
`262481e844521b326f5ecd053e59b98c8b2da78c8ee1bdbb6e8174305e54935a`.
Builds and CI never fetch them: they are committed.
