"""Builds the 12.G plan from template.md, tasks.py and the task commits."""
import subprocess, sys
sys.path.insert(0, '/tmp/ryukei12g')
from tasks import TASKS

REPO = '/tmp/ryukei12g/tasks'
BASE = subprocess.check_output(['git', '-C', REPO, 'merge-base', 'HEAD', 'origin/main']).decode().strip()


def git(*a):
    return subprocess.check_output(['git', '-C', REPO, *a]).decode()


commits = git('log', '--reverse', '--format=%h %s', f'{BASE}..HEAD').strip().splitlines()
by_title = {}
for line in commits:
    h, s = line.split(' ', 1)
    by_title[s] = h
order = {
    1: 'ryukei 12.G: compatible curves keep their parameter maps',
    2: 'ryukei 12.G: fill errors and names',
    3: 'ryukei 12.G: fill',
    4: 'ryukei 12.G: the fill golden section',
    5: 'ryukei 12.G: filling is bounded',
    6: 'ryukei 12.G: fill_props',
    7: 'ryukei 12.G: the sheet fuzz target fills',
    8: "ryukei 12.G: OCCT's BRepFill_Filling as reference",
    10: 'ryukei 12.G: docs/claude: fill',
}
FILL = dict(l.split('=', 1) for l in open('/tmp/ryukei12g/fill.txt').read().split('\n@@\n') if '=' in l)

tip = git('rev-parse', '--short=8', 'HEAD').strip()
out = open('/tmp/ryukei12g/template.md').read()
out = out.replace('{{BASE}}', BASE[:8]).replace('{{TIP}}', tip)
out = out.replace('{{BASE_SHORT}}', BASE[:8]).replace('{{TIP_SHORT}}', tip)
rows, sections = [], []
for n, wi, title, body in TASKS:
    h = by_title.get(order.get(n, ''), None)
    rows.append(f"| {n} | #{wi} | {'`' + h + '`' if h else '(none)'} | {title} |")
    if h:
        diff = git('show', '--format=', h)
        body = body.replace('{{DIFF}}', f"The change, exactly (`git show {h}`):\n\n````diff\n{diff}````")
    for k, v in FILL.items():
        body = body.replace('{{' + k.strip() + '}}', v.strip())
    sections.append(f"## Task {n}: {title} (#{wi})\n\n{body}")
out = out.replace('{{TABLE}}', '\n'.join(rows)).replace('{{TASKS}}', '\n\n'.join(sections))
assert '{{' not in out, [l for l in out.splitlines() if '{{' in l][:5]
open('/tmp/ryukei12g/plan.md', 'w').write(out)
print(len(out.splitlines()), 'lines')
