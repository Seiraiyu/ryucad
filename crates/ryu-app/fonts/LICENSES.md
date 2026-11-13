# Fonts and icons in ryucad-app

The app embeds these files (design 199, plan 11.B); the mockups in
`design/app/` read them too. `design/app/fetch-fonts.py` fetches them from
the archives below and checks every file by SHA-256.

| File | What | Version | Licence | Text |
|---|---|---|---|---|
| `InterVariable.ttf` | Inter, variable, the app's text face | 4.1 (rsms/inter) | SIL Open Font License 1.1 | `Inter-LICENSE.txt` |
| `JetBrainsMono-Regular.ttf` | JetBrains Mono, for references, revisions and values | 2.304 (JetBrains/JetBrainsMono) | SIL Open Font License 1.1 | `JetBrainsMono-OFL.txt` |
| `Phosphor.ttf`, `Phosphor-Fill.ttf` | Phosphor icons, Regular and Fill | 2.1.2 (@phosphor-icons/web) | MIT | `Phosphor-LICENSE.txt` |

ryucad's own 15 CAD glyphs (`design/app/glyphs.js`, and `ui::icon::glyphs`
from 11.B) are ryucad's work, under the repository's licence.
