#!/bin/sh
# The plan's questions, at the sections they touch. $1: the plan's document ID.
set -eu
D=$1
q() { cox doc question add "$D" "$@"; }
q --section task-1-error-codes-and-the-package-job-phase-6152 \
  --question "Which codes do release package's refusals use?" \
  --why "Agents branch on codes, which are never renamed: this fixes them" \
  --option "Reuse label_not_found and label_not_official (they already mean a release with no record, and one not official); add release_obsolete, package_failed, package_invalid and packages_format_unknown" \
  --option "Add release_not_official as design 436 wrote it, beside label_not_official" --recommend 0
q --section task-3-the-manifest-formats-key-and-package-hash-6154 \
  --question "What is built_with.commit in a package's manifest and attestation?" \
  --why "It is in the key and the signed bytes; design 436's example shows 40 hex, but releases stamp 12" \
  --option "What ryucad version gives: 12 hex in a release (release.yml's GITHUB_SHA::12), unknown from a source archive; 7 to 40 hex accepted" \
  --option "Stamp the full 40-hex commit in releases (a release.yml change) and require 40" --recommend 0
q --section task-4-packages-formats-in-ryupdm-json-6155 \
  --question "Which ryupdm.json format holds packages.formats, now that ryupdm 6.A (plan 438, question 399) takes 3 for teams and revoked keys?" \
  --why "Design 261's rule gives the next format to whichever change lands first; both plans are open" \
  --option "The next format when Task 4 runs: 4 if 6.A's format 3 is on main, else 3 (the executor checks config::FORMAT first)" \
  --option "Always 4, leaving 3 to 6.A even if 7.A merges first" --recommend 0
q --section task-8-building-a-package-from-its-official-commit-6159 \
  --question "What does a package's render/iso.png draw?" \
  --why "It goes to suppliers; design 436 names render P --view iso --size 1600x1200, whose defaults draw sketches and datums" \
  --option "Design 436's command as written: render's defaults, sketches and datums drawn, appearance on" \
  --option "The solids alone: no sketches or datums (a departure from the command the design names)" --recommend 0
q --section task-8-building-a-package-from-its-official-commit-6159 \
  --question "How is an assembly's children[].package filled?" \
  --why "Design 436 fills it when this machine knows the child's package key; nothing in 7.A knows one, and the service runs this same command" \
  --option "Always null in 7.A (it is left out of the package hash, so a later stage can fill it without changing what an attestation signs)" \
  --option "Add --child-package PART:LABEL=KEY, for the service to pass the keys it has" --recommend 0
q --section task-15-the-packages-golden-section-6166 \
  --question "What does the packages golden section pin?" \
  --why "Design 436 says the manifest's and zip's hashes, but a record names the build that made it, so commits (and so the manifest and zip) differ between a development and a stamped build" \
  --option "Every file's bytes but the record's, and the manifest with commit, content, record and the STEP's and record's hashes left out; the zip writer's bytes pinned by package::zip's unit test; one thread and four alike" \
  --option "Whole zip hashes, with a test hook stamping records with a fixed build so commits match everywhere" --recommend 0
q --section task-2-the-package-zip-writer-and-reader-6153 \
  --question "Where does the package's zip writer live?" \
  --why "ryuformats::zip writes 3MF's zips, but ryupdm can't link ryuformats (it depends on ryukei, and ryupdm never calls the kernel)" \
  --option "ryupdm::package::zip, its own writer and a bounded reader, as design 436 says" \
  --option "A new crate (ryu-zip) both ryuformats and ryupdm use, ryuformats' bytes unchanged" --recommend 0
