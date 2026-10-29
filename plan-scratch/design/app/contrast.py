#!/usr/bin/env python3
"""WCAG 2.1 contrast of the mockups' palettes (design 199's Accessibility).

Reads every `[data-look=...][data-theme=...][data-contrast=...]` block of
app.css and checks, for each: text and weak text at least 4.5:1 on the
app, raised and frame surfaces; state colours used as text (danger, info,
ok, warn as row text and badges' labels) at least 4.5:1 on app and raised;
the accent and the focus ring at least 3:1 on app and raised; selected
text at least 4.5:1 on its selection. Prints each failure and exits 1 on
any; prints `contrast: N palettes pass` otherwise.

    python3 design/app/contrast.py
"""
import pathlib
import re
import sys

CSS = pathlib.Path(__file__).with_name("app.css").read_text()


def palettes():
    for m in re.finditer(r"(\[data-look[^{]*)\{([^}]*)\}", CSS):
        sel, body = m.group(1).strip(), m.group(2)
        tokens = dict(re.findall(r"--([a-z-]+):\s*(#[0-9a-f]{6})", body))
        if tokens:
            yield sel, tokens


def lum(hex6):
    def ch(c):
        c = c / 255
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4

    r, g, b = (int(hex6[i : i + 2], 16) for i in (1, 3, 5))
    return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)


def ratio(a, b):
    la, lb = sorted((lum(a), lum(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


RULES = [
    (("text", "text-weak"), ("app", "raised", "frame"), 4.5),
    (("danger", "info", "ok", "warn"), ("app", "raised"), 4.5),
    (("accent", "focus"), ("app", "raised"), 3.0),
    (("select-fg",), ("select-bg",), 4.5),
]

fails = 0
count = 0
for sel, t in palettes():
    count += 1
    for fgs, bgs, need in RULES:
        for fg in fgs:
            for bg in bgs:
                r = ratio(t[fg], t[bg])
                if r < need:
                    fails += 1
                    print(f"FAIL {sel}: --{fg} {t[fg]} on --{bg} {t[bg]} is {r:.2f}:1, needs {need}:1")
if count == 0:
    sys.exit("contrast: no palettes found in app.css")
if fails:
    sys.exit(1)
print(f"contrast: {count} palettes pass")
