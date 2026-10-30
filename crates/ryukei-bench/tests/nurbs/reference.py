# SPDX-License-Identifier: Apache-2.0
"""SMLib's side of ryukei-bench's NURBS comparisons (design 94, plan 8.F).

Reads a manifest (JSON: {"jobs": [job, ...]}) and prints, as JSON, one
result per job, each run in a process of its own so that an SMLib crash
(or the hang a corrupted heap can leave) is that job's "error", not the
end of the run. Bodies come as USD files ryuformats wrote (BrepArrays, in
meters); curves as exact clamped NURBS ({"degree", "knots", "mults",
"points", "weights"}). Faces are picked by a point: the face nearest it.
Jobs:

- {"kind": "volume", "file"}: the body's volume.
- {"kind": "loft", "sections": [[curve, ...], ...]}: a skin through the
  sections, capped (create_skin_primitive, cubic): its volume.
- {"kind": "pipe", "profile": [curve, ...], "path": [curve, ...]}: the
  profile along a planar path (sweep_along_planar_path): its volume.
- {"kind": "offset", "file", "distance"}: offset_brep: its volume.
- {"kind": "shell", "file", "thickness", "open": [point, ...]}: shell_brep
  inwards with those faces open: its volume.
- {"kind": "ssi", "a", "pa", "b", "pb", "samples"}: intersect_surfaces of
  the two faces' surfaces, each curve sampled at samples + 1 parameters.
- {"kind": "race", "block", "pin", "places": [[x, y, z], ...]}: the pins
  united, then taken from the block on one thread, timed: seconds, faces,
  volume.

Run with the Python the devcontainer installs:
/opt/usd/bin/python reference.py manifest.json
"""

import json
import subprocess
import sys
import time

import usd_brep as ub


def body(path):
    breps = ub.usd.import_breps(path)
    if len(breps) != 1:
        raise RuntimeError(f"{path}: {len(breps)} breps")
    return breps[0]


def curve(c):
    return ub.create_canonical_curve(
        [tuple(p) for p in c["points"]],
        c["knots"],
        c["mults"],
        c["weights"] if c["weights"] else None,
        c["degree"],
    )


def nearest_face(b, point):
    return min(b.faces(), key=lambda f: f.closest_point(tuple(point))[1])


def volume(b):
    if not b.is_manifold_solid():
        raise RuntimeError("not a manifold solid")
    return b.volume(1e-9)


def sampled(c, samples):
    t0, t1 = c.parameter_range()
    return [list(c.evaluate(t0 + (t1 - t0) * k / samples)) for k in range(samples + 1)]


def run(job):
    kind = job["kind"]
    if kind == "volume":
        return {"volume": volume(body(job["file"]))}
    if kind == "loft":
        counts = [len(s) for s in job["sections"]]
        curves = [curve(c) for s in job["sections"] for c in s]
        return {"volume": volume(ub.create_skin_primitive(counts, curves, 3, True))}
    if kind == "pipe":
        swept = ub.sweep_along_planar_path(
            [curve(c) for c in job["profile"]], [curve(c) for c in job["path"]]
        )
        return {"volume": volume(swept)}
    if kind == "offset":
        grown = ub.offset_brep(body(job["file"]), job["distance"], extended_offset=False)
        return {"volume": volume(grown)}
    if kind == "shell":
        b = body(job["file"])
        faces = [nearest_face(b, p) for p in job["open"]]
        shelled = ub.shell_brep(b, -job["thickness"], faces_to_shell=faces)
        return {"volume": volume(shelled)}
    if kind == "ssi":
        sa = nearest_face(body(job["a"]), job["pa"]).surface()
        sb = nearest_face(body(job["b"]), job["pb"]).surface()
        curves = ub.intersect_surfaces(sa, sb)
        return {"curves": [sampled(c, job["samples"]) for c in curves]}
    if kind == "race":
        block = body(job["block"])
        pin = body(job["pin"])
        pins = []
        for x, y, z in job["places"]:
            at = ub.Axis2Placement((x, y, z), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0))
            pins.append(ub.transform(pin.copy(), at))
        t = time.perf_counter()
        tools = ub.merge_breps(pins, operation=ub.BooleanOp.UNION) if len(pins) > 1 else pins[0]
        cut = ub.boolean_difference(block, tools)
        seconds = time.perf_counter() - t
        return {"seconds": seconds, "faces": cut.face_count(), "volume": volume(cut)}
    raise RuntimeError(f"no such job: {kind}")


def isolated(job):
    try:
        out = subprocess.run(
            [sys.executable, "-I", __file__, "--job", json.dumps(job)],
            capture_output=True,
            text=True,
            timeout=3600,
        )
    except subprocess.TimeoutExpired:
        return {"error": "SMLib took over an hour"}
    if out.returncode != 0:
        why = out.stderr.strip()[-2000:]
        return {"error": f"exited {out.returncode}: {why}"}
    return json.loads(out.stdout)


def here(job):
    try:
        return run(job)
    except Exception as e:  # SMLib's refusal, reported, not raised
        return {"error": str(e)}


if __name__ == "__main__":
    if sys.argv[1] == "--job":
        json.dump(here(json.loads(sys.argv[2])), sys.stdout)
    else:
        with open(sys.argv[1]) as f:
            jobs = json.load(f)["jobs"]
        json.dump([isolated(j) for j in jobs], sys.stdout)
