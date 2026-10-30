# SPDX-License-Identifier: Apache-2.0
"""USD's references for ryukei-bench's `usd` test (design 193, plans 8.B
and 8.C).

Reads a manifest (JSON: {"files": [path, ...]}) and prints, as JSON, for
each file: what OpenUSD (usd-core) opens, the SHA-256 of its root layer as
OpenUSD writes it as text (so usda, usdc and usdz of one stage compare),
what usd-brep's BrepArray validator says (the schema rules alone), for a
usdz what OpenUSD's package validators say, and for a usda what SMLib
imports: each brep's volume and whether it is a manifold solid (a usdc or
usdz of the same layer imports the same). Run with the Python the
devcontainer installs: /opt/usd/bin/python reference.py manifest.json
"""

import hashlib
import json
import subprocess
import sys

# First: it registers the omniSolid schema, which OpenUSD reads only once.
import usd_brep
from brep_validator import validate_file
from pxr import Sdf, Usd, UsdGeom, UsdValidation

# OpenUSD's checks of a usdz package: stored and aligned entries, the
# root layer first, known file kinds, nothing referred to outside it.
PACKAGE_VALIDATORS = [
    "usdUtilsValidators:UsdzPackageValidator",
    "usdUtilsValidators:RootPackageValidator",
    "usdUtilsValidators:FileExtensionValidator",
    "usdUtilsValidators:PackageEncapsulationValidator",
]


def opened(path):
    stage = Usd.Stage.Open(path)
    if stage is None:
        return {"opened": False}
    every = list(stage.Traverse(Usd.TraverseInstanceProxies()))
    return {
        "opened": True,
        "default_prim": str(stage.GetDefaultPrim().GetPath()),
        "meters_per_unit": UsdGeom.GetStageMetersPerUnit(stage),
        "up_axis": str(UsdGeom.GetStageUpAxis(stage)),
        "breparrays": sum(1 for p in every if p.GetTypeName() == "BrepArray"),
        "meshes": sum(1 for p in every if p.IsA(UsdGeom.Mesh)),
        "instances": sum(1 for p in stage.Traverse() if p.IsInstance()),
        "prototypes": len(stage.GetPrototypes()),
        "flattened": len(stage.Flatten().ExportToString()) > 0,
    }


def layer_text(path):
    layer = Sdf.Layer.FindOrOpen(path)
    if layer is None:
        return {"layer": None}
    text = layer.ExportToString().encode()
    return {"layer": hashlib.sha256(text).hexdigest()}


def packaged(path):
    registry = UsdValidation.ValidationRegistry()
    validators = [registry.GetOrLoadValidatorByName(n) for n in PACKAGE_VALIDATORS]
    errors = UsdValidation.ValidationContext(validators).Validate(Usd.Stage.Open(path))
    return {"package": [f"{e.GetName()}: {e.GetMessage()}" for e in errors]}


def validated(path):
    v = validate_file(path, workers=1, brep_only=True)
    return {"valid": bool(v.passed), "messages": [str(m) for m in v.messages][:40]}


def imported(path):
    # In a process of its own: an SMLib crash (or the hang a corrupted heap
    # can leave) is this file's import_error, not the end of the run.
    try:
        out = subprocess.run(
            [sys.executable, "-I", __file__, "--import", path],
            capture_output=True,
            text=True,
            timeout=600,
        )
    except subprocess.TimeoutExpired:
        return {"import_error": "SMLib's import took over 600 s"}
    if out.returncode != 0:
        why = out.stderr.strip()[-2000:]
        return {"import_error": f"SMLib's import exited {out.returncode}: {why}"}
    return json.loads(out.stdout)


def imported_here(path):
    try:
        breps = usd_brep.usd.import_breps(path)
    except Exception as e:  # SMLib's refusal, reported, not raised
        return {"import_error": str(e)}
    out = []
    for b in breps:
        manifold = bool(usd_brep.is_manifold_solid(b))
        volume = usd_brep.compute_volume(b, 1e-6) if manifold else None
        out.append({"manifold": manifold, "volume": volume})
    return {"breps": out}


def main(manifest):
    with open(manifest) as f:
        files = json.load(f)["files"]
    report = []
    for path in files:
        r = {"path": path}
        r.update(opened(path))
        r.update(layer_text(path))
        r.update(validated(path))
        if path.endswith(".usdz"):
            r.update(packaged(path))
        if path.endswith(".usda"):
            r.update(imported(path))
        report.append(r)
    json.dump(report, sys.stdout)


if __name__ == "__main__":
    if sys.argv[1] == "--import":
        json.dump(imported_here(sys.argv[2]), sys.stdout)
    else:
        main(sys.argv[1])
