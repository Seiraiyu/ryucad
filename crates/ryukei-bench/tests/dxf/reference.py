"""What ezdxf reads of each DXF ryuformats writes (plan 16.B): the
audit's errors and fixes, the layers, each layer's closed loops' areas
(entities chained end to end, flattened within a micrometre's
thousandth), the bend lines' ends and the notes, and (plan 9.D, for
drawings) how many entities of each type the model space holds. Reads a manifest
{"files": [...]} and prints a JSON list, one entry per file."""

import json
import sys

import ezdxf
from ezdxf import path as dxfpath

TOL = 1e-6


def points(e):
    return [(v.x, v.y) for v in dxfpath.make_path(e).flattening(1e-6)]


def loops(entities):
    """Chain entities' polylines end to end into closed loops; each loop's
    absolute shoelace area."""
    pieces = [points(e) for e in entities]
    pieces = [p for p in pieces if len(p) >= 2]
    out = []
    while pieces:
        chain = pieces.pop(0)
        closed = abs(chain[0][0] - chain[-1][0]) < TOL and abs(chain[0][1] - chain[-1][1]) < TOL
        while not closed:
            end = chain[-1]
            for i, p in enumerate(pieces):
                if abs(p[0][0] - end[0]) < TOL and abs(p[0][1] - end[1]) < TOL:
                    chain += p[1:]
                    break
                if abs(p[-1][0] - end[0]) < TOL and abs(p[-1][1] - end[1]) < TOL:
                    chain += p[::-1][1:]
                    break
            else:
                raise SystemExit(f"a loop doesn't close at {end}")
            pieces.pop(i)
            closed = abs(chain[0][0] - chain[-1][0]) < TOL and abs(chain[0][1] - chain[-1][1]) < TOL
        a = 0.0
        for (x0, y0), (x1, y1) in zip(chain, chain[1:]):
            a += x0 * y1 - x1 * y0
        out.append(abs(a) / 2.0)
    return sorted(out)


def counts(msp):
    out = {}
    for e in msp:
        out[e.dxftype()] = out.get(e.dxftype(), 0) + 1
    return out


def main():
    files = json.load(open(sys.argv[1]))["files"]
    result = []
    for f in files:
        doc = ezdxf.readfile(f)
        auditor = doc.audit()
        msp = doc.modelspace()
        lines = []
        for layer in ("BEND_UP", "BEND_DOWN"):
            for e in msp.query(f'LINE[layer=="{layer}"]'):
                s, t = e.dxf.start, e.dxf.end
                lines.append({"layer": layer, "from": [s.x, s.y], "to": [t.x, t.y]})
        result.append({
            "file": f,
            "version": doc.dxfversion,
            "errors": [str(x.message) for x in auditor.errors],
            "fixes": len(auditor.fixes),
            "layers": sorted(l.dxf.name for l in doc.layers),
            "outline": loops(list(msp.query('*[layer=="OUTLINE"]'))),
            "inner": loops(list(msp.query('*[layer=="INNER"]'))),
            "bend_lines": lines,
            "notes": [e.dxf.text for e in msp.query('TEXT[layer=="BEND_NOTES"]')],
            "counts": counts(msp),
        })
    json.dump(result, sys.stdout)


main()
