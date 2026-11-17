#!/bin/sh
# Builds the Python client's wheel and sdist into python/dist:
#   sh scripts/python-dist.sh            (PYTHON=python where python3 isn't)
# Hatchling can't name files above python/, so the repository's LICENSE
# and NOTICE are copied in first; python/.gitignore keeps the copies out
# of Git. Needs `build` (python3 -m pip install build). A release stamps
# the version first (scripts/stamp-version.sh v<version>); a checkout
# builds 0.0.0.
set -eu
cd "$(dirname "$0")/.."
py=${PYTHON:-python3}
cp LICENSE NOTICE python/
rm -rf python/dist python/build
"$py" -m build python
# The wheel must carry both licences (design 697).
"$py" - python/dist/*.whl <<'PY'
import sys, zipfile
names = zipfile.ZipFile(sys.argv[1]).namelist()
for want in ("LICENSE", "NOTICE"):
    if not any(n.endswith(".dist-info/licenses/" + want) for n in names):
        sys.exit(f"python-dist: the wheel carries no {want}")
PY
ls python/dist
