#!/usr/bin/env python3
"""Fetches the fonts design 199 bundles, checks them, and writes them where
the app will embed them (plan 11.B) and the mockups read them.

Inter 4.1 (InterVariable.ttf) and JetBrains Mono 2.304 (Regular), SIL OFL
1.1; Phosphor 2.1.2 Regular and Fill (MIT), from @phosphor-icons/web on npm,
whose two style sheets become vendor/phosphor.css and
vendor/phosphor-fill.css with their font URLs pointed at the files below.
Every archive and every file taken from it is checked by SHA-256.

    python3 design/app/fetch-fonts.py
"""
import hashlib
import io
import pathlib
import re
import sys
import tarfile
import urllib.request
import zipfile

HERE = pathlib.Path(__file__).resolve().parent
FONTS = HERE.parent.parent / "crates" / "ryucad-app" / "fonts"
VENDOR = HERE / "vendor"

ARCHIVES = {
    "inter": ("https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip",
              "9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e"),
    "mono": ("https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip",
             "6f6376c6ed2960ea8a963cd7387ec9d76e3f629125bc33d1fdcd7eb7012f7bbf"),
    "phosphor": ("https://registry.npmjs.org/@phosphor-icons/web/-/web-2.1.2.tgz",
                 "40e3096099ca818c047979cece6a3764944d5200c67b64092f7bcd8bcd1d2b08"),
}

# (archive, member, file written, its SHA-256 or None for licence texts)
FILES = [
    ("inter", "InterVariable.ttf", "InterVariable.ttf",
     "4989b125924991b90d05b2d16e0e388c48f7d5bb8b30539bbf9c755278d0ccaf"),
    ("inter", "LICENSE.txt", "Inter-LICENSE.txt", None),
    ("mono", "fonts/ttf/JetBrainsMono-Regular.ttf", "JetBrainsMono-Regular.ttf",
     "a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f"),
    ("mono", "OFL.txt", "JetBrainsMono-OFL.txt", None),
    ("phosphor", "package/src/regular/Phosphor.ttf", "Phosphor.ttf",
     "06b91e022b7ee899a63efced879392a74f0bacbda54e4467e9f663220d173a10"),
    ("phosphor", "package/src/fill/Phosphor-Fill.ttf", "Phosphor-Fill.ttf",
     "a53f5d2630cab5e3b7536ecb9d69d71519a2190298c22b1f8d770dd37bc2940a"),
    ("phosphor", "package/LICENSE", "Phosphor-LICENSE.txt", None),
]

STYLES = [("package/src/regular/style.css", "phosphor.css", "Phosphor.ttf"),
          ("package/src/fill/style.css", "phosphor-fill.css", "Phosphor-Fill.ttf")]


def sha(b):
    return hashlib.sha256(b).hexdigest()


def fetch(key):
    url, want = ARCHIVES[key]
    data = urllib.request.urlopen(url).read()
    if sha(data) != want:
        sys.exit(f"{url}: SHA-256 {sha(data)}, expected {want}")
    return data


def member(key, data, name):
    if key == "phosphor":
        return tarfile.open(fileobj=io.BytesIO(data)).extractfile(name).read()
    return zipfile.ZipFile(io.BytesIO(data)).read(name)


def main():
    FONTS.mkdir(parents=True, exist_ok=True)
    VENDOR.mkdir(exist_ok=True)
    got = {k: fetch(k) for k in ARCHIVES}
    for key, name, out, want in FILES:
        b = member(key, got[key], name)
        if want and sha(b) != want:
            sys.exit(f"{name}: SHA-256 {sha(b)}, expected {want}")
        (FONTS / out).write_bytes(b)
        print(f"wrote crates/ryucad-app/fonts/{out} ({len(b)} bytes)")
    for name, out, ttf in STYLES:
        css = member("phosphor", got["phosphor"], name).decode()
        css = re.sub(r"src:[^;]*;", f'src: url("../../../crates/ryucad-app/fonts/{ttf}") format("truetype");',
                     css, count=1)
        (VENDOR / out).write_text(css)
        print(f"wrote design/app/vendor/{out}")


main()
