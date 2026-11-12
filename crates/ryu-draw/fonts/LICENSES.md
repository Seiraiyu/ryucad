# Fonts in ryu-draw

Every ryu drawing is written in Inter: Regular, and SemiBold for ryueda's
designators, titles and emphasis (plan 2.E): ryueda's schematics (design 236)
and ryucad's drawings (design 417). SVG names it, PNGs are rasterized with
it, and PDFs embed a subset of it (`ryu_draw::font::INTER`). `ryusch` reads
only its advance widths, from `crates/ryusch/src/widths.rs`, which ryueda's
`tests/font_widths.rs` generates from this file and checks against it.

| File | What | Version | Licence | Text |
|---|---|---|---|---|
| `Inter-Regular.ttf` | Inter Regular, static (`extras/ttf/Inter-Regular.ttf` of the release archive) | 4.1 (rsms/inter) | SIL Open Font License 1.1 | `Inter-LICENSE.txt` |
| `Inter-SemiBold.ttf` | Inter SemiBold, static (`extras/ttf/Inter-SemiBold.ttf` of the release archive) | 4.1 (rsms/inter) | SIL Open Font License 1.1 | `Inter-LICENSE.txt` |

The archive is `https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip`
(SHA-256 `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`);
`Inter-Regular.ttf` is SHA-256
`40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82`;
`Inter-SemiBold.ttf` is SHA-256
`78a843fade9d4612a5567302fb595b56976eb5fcebf4fea5a5912d638bafcde3`;
and
`Inter-LICENSE.txt` (the archive's `LICENSE.txt`) SHA-256
`262481e844521b326f5ecd053e59b98c8b2da78c8ee1bdbb6e8174305e54935a`.
Builds and CI never fetch them: they are committed.
