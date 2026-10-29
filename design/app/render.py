#!/usr/bin/env python3
"""Renders the mockups (design 199, plan 11.A) to PNGs with a headless
Chromium: every page in every variant, the 2x detail crops, and the looks
sheet. Standard library only.

    python3 design/app/render.py setup   # once: Chromium and its libraries
    python3 design/app/render.py         # every PNG under design/app/png/
    python3 design/app/render.py page layout icons   # those pages, Drafting light

`setup` installs Playwright's chrome-headless-shell (`npx -y
playwright@1.56.1 install chromium-headless-shell`) unless one is in
~/.cache/ms-playwright already, and, where the host lacks the libraries it
needs (Debian 12 without a desktop lacks libnspr4, libnss3 and
libasound2), downloads their packages with `apt-get download` and unpacks
them into target/mockup-libs (no root needed). CHROME names another
binary.

A page takes its variant from its query: `?look=drafting|graphite|studio
&theme=light|dark&contrast=normal|high`; `crop=x,y,w,h` shows that part
of the window only, for the 2x crops.
"""
import concurrent.futures
import glob
import os
import pathlib
import shutil
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
LIBS = ROOT / "target" / "mockup-libs"
OUT = HERE / "png"

# (page, width, height) at 1x.
PAGES = [
    ("layout", 1600, 1000),
    ("palette", 1600, 1000),
    ("feedback", 1600, 1000),
    ("progress", 1600, 1000),
    ("start", 1600, 1000),
    ("form", 1600, 1000),
    ("tree", 1600, 1000),
    ("shortcuts", 1600, 1000),
    ("icons", 1600, 1500),
]

# (name, look, theme, contrast)
VARIANTS = [
    ("drafting-light", "drafting", "light", "normal"),
    ("drafting-dark", "drafting", "dark", "normal"),
    ("graphite-light", "graphite", "light", "normal"),
    ("graphite-dark", "graphite", "dark", "normal"),
    ("studio-light", "studio", "light", "normal"),
    ("studio-dark", "studio", "dark", "normal"),
    ("high-light", "drafting", "light", "high"),
    ("high-dark", "drafting", "dark", "high"),
]

# (crop name, page, x, y, w, h): details at 2x, Drafting light.
CROPS = [
    ("toolbar", "layout", 0, 0, 800, 56),
    ("tree", "layout", 0, 56, 250, 560),
    ("palette", "palette", 480, 50, 640, 340),
    ("error-card", "feedback", 0, 560, 560, 440),
    ("toasts", "feedback", 1240, 700, 360, 300),
    ("progress", "progress", 0, 940, 900, 60),
    ("form", "form", 1330, 56, 270, 600),
    ("tree-search", "tree", 0, 56, 250, 420),
    ("icons-16", "icons", 0, 0, 800, 900),
]


def chrome():
    if os.environ.get("CHROME"):
        return os.environ["CHROME"]
    found = sorted(glob.glob(os.path.expanduser(
        "~/.cache/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-linux64/chrome-headless-shell")))
    if not found:
        sys.exit("no chrome-headless-shell: run `python3 design/app/render.py setup`, or set CHROME")
    return found[-1]


def env():
    e = dict(os.environ)
    libdirs = [str(p) for p in (LIBS / "usr/lib/x86_64-linux-gnu", LIBS / "usr/lib/x86_64-linux-gnu/nss") if p.is_dir()]
    if libdirs:
        e["LD_LIBRARY_PATH"] = ":".join(libdirs + [e.get("LD_LIBRARY_PATH", "")]).rstrip(":")
    return e


def missing_libs(binary):
    out = subprocess.run(["ldd", binary], capture_output=True, text=True, env=env()).stdout
    return [line.split()[0] for line in out.splitlines() if "not found" in line]


def setup():
    if not glob.glob(os.path.expanduser("~/.cache/ms-playwright/chromium_headless_shell-*")) and not os.environ.get("CHROME"):
        subprocess.run(["npx", "-y", "playwright@1.56.1", "install", "chromium-headless-shell"], check=True)
    binary = chrome()
    if missing_libs(binary):
        LIBS.mkdir(parents=True, exist_ok=True)
        subprocess.run(["apt-get", "download", "libnspr4", "libnss3", "libasound2"], cwd=LIBS, check=True)
        for deb in LIBS.glob("*.deb"):
            subprocess.run(["dpkg-deb", "-x", str(deb), str(LIBS)], check=True)
    still = missing_libs(binary)
    if still:
        sys.exit(f"{binary} still lacks {', '.join(still)}")
    print(f"ready: {binary}")


def shot(url, out, w, h, scale=1):
    out.parent.mkdir(parents=True, exist_ok=True)
    cmd = [chrome(), "--no-sandbox", "--hide-scrollbars", "--allow-file-access-from-files",
           "--force-color-profile=srgb", f"--force-device-scale-factor={scale}",
           f"--window-size={w},{h}", "--virtual-time-budget=3000", f"--screenshot={out}", url]
    r = subprocess.run(cmd, capture_output=True, text=True, env=env(), timeout=120)
    if r.returncode != 0 or not out.exists():
        raise RuntimeError(f"{url}: {r.stderr[-500:]}")
    return out


def url(page, look, theme, contrast, crop=None):
    q = f"look={look}&theme={theme}&contrast={contrast}"
    if crop:
        q += "&crop=" + ",".join(map(str, crop))
    return (HERE / f"{page}.html").as_uri() + "?" + q


def looks_sheet():
    """One page of thumbnails: a row per surface, a column per variant."""
    cells = []
    for page, _, _ in PAGES:
        row = "".join(f'<td><img src="{page}/{v}.png"></td>' for v, *_ in VARIANTS)
        cells.append(f"<tr><th>{page}</th>{row}</tr>")
    head = "".join(f"<th>{v}</th>" for v, *_ in VARIANTS)
    html = f"""<!doctype html><meta charset="utf-8"><style>
      body {{ margin: 0; padding: 16px; background: #fff; font: 13px sans-serif; color: #222; }}
      td, th {{ padding: 4px; vertical-align: top; text-align: left; }}
      img {{ width: 360px; height: 225px; object-fit: cover; object-position: top; border: 1px solid #ccc; display: block; }}
    </style><table><tr><th></th>{head}</tr>{''.join(cells)}</table>"""
    sheet = OUT / "looks.html"
    sheet.write_text(html)
    w, h = 16 * 2 + 100 + 8 * 370, 16 * 2 + 30 + len(PAGES) * 235
    shot(sheet.as_uri(), OUT / "looks.png", w, h)
    sheet.unlink()


def render():
    if OUT.exists():
        shutil.rmtree(OUT)
    jobs = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        for page, w, h in PAGES:
            for name, look, theme, contrast in VARIANTS:
                jobs.append(pool.submit(shot, url(page, look, theme, contrast), OUT / page / f"{name}.png", w, h))
        for name, page, x, y, w, h in CROPS:
            jobs.append(pool.submit(shot, url(page, "drafting", "light", "normal", (x, y, w, h)),
                                    OUT / "crops" / f"{name}@2x.png", w, h, 2))
        for j in jobs:
            print("wrote", j.result().relative_to(ROOT))
    looks_sheet()
    print("wrote", (OUT / "looks.png").relative_to(ROOT))


if __name__ == "__main__":
    if sys.argv[1:] == ["setup"]:
        setup()
    elif not sys.argv[1:]:
        render()
    elif sys.argv[1] == "page" and len(sys.argv) > 2:
        sizes = {p: (w, h) for p, w, h in PAGES}
        for page in sys.argv[2:]:
            if page not in sizes:
                sys.exit(f"no page {page}: {', '.join(sizes)}")
            out = shot(url(page, "drafting", "light", "normal"), OUT / page / "drafting-light.png", *sizes[page])
            print("wrote", out.relative_to(ROOT))
    else:
        sys.exit(__doc__)
