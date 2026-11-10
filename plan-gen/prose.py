"""The plan's words; gen.py adds the code."""

BRANCH = "cox/ryupdm-7a-plan-K468HCGN"
PROTO = "PROTO_COMMIT"

HEAD = f"""# ryupdm 7.A: Release packages in ryucad -- Implementation Plan

**Goal:** `ryucad release package` builds any official release's package offline, from its official commit alone, as a deterministic zip with a hashed manifest; `release package verify` checks one someone was sent against the clone and its signed `.package` tag; `ryucad review package` is the service's client.
**Architecture:** `ryupdm::package` (public, no kernel) defines the package: the manifest and its key and hash, the zip writer and bounded reader, `packages.formats` in `ryupdm.json`, the package attestation kind beside design 261's, and the offline verifier. `ryucad::package` builds the files through the methods agents call (`export`, `drawing.export`, `bom.get`, `render`) from a frozen session at the official commit, and serves `release.package` and `release.package.verify`; `release list` and `release check` read the package tag. The service (7.B) only runs this command.
**Tech stack:** Rust 2024 (toolchain 1.99), `miniz_oxide` 0.8 (in the workspace already), `ed25519-dalek` 2 and `blake3` (ryupdm has them), gix, clap, the fake review service in `crates/ryucad/tests/fake_review`.
**Epic:** work item 6106 (ryupdm 7.A). Stories 6150 (design phase 1) and 6151 (phase 2).

Design 436 (approved). 7.A ships in the next ryucad release after v0.9.0 (question 384); it starts from `main`.

## What is there, and what this plan adds

- **Releases** (`ryupdm::history`, `record`, `state`): records, official commits, `Releases::official` (path → commit and blob), `objects` (each `ryupdm/` tag's own object) and `tags`. Nothing new is read from Git but the `.package` tag's object.
- **Attestations** (`ryupdm::attest`, design 261): message, signature block, `HOSTED_KEYS`, `trusted`, `sign` under `testing`. This plan splits `parse_message` into `split_message` (any kind) and `check_signature`, and adds `package::attest` beside it, over its own domain.
- **The files' methods**: `export::export` with `rev: label:L` (STEP's time stamp is the official commit's, ryucad 6.C), `drawing::request::export` (PDF of every sheet, DXF of one; `{{revision}}` and `{{date}}` are the release's in a frozen session at its commit, design 417), `bom::get` with `rev: label:L`, and `server::render` at a commit. Each answers its bytes when given no `out`, which is how the builder collects them.
- **The zip**: `ryuformats::zip` writes 3MF's zips, but ryupdm can't link `ryuformats` (it depends on ryukei, and ryupdm never calls the kernel), so `ryupdm::package::zip` is its own, as design 436 says, and reads zips too (question 411).
- **`release list`/`release check`**: a `.package` tag would be `release_tag_orphaned` today (`Releases::orphaned_tags` looks for `releases/<part>/<label>.package.json`); Task 7 fixes that.

## Decisions this plan makes (questions 405 to 411, each answered with its recommended option)

1. **Codes.** A proposed record is the existing `label_not_official`, a missing one `label_not_found` (both already mean exactly that, for instances); new codes `release_obsolete`, `package_failed`, `package_invalid` (exit 1, the verdict `invalid` with its `clause`) and `packages_format_unknown`. Design 436 wrote `release_not_official` (question 405: reuse).
2. **`built_with.commit`** is what `ryucad version` gives: a release stamps `RYUCAD_COMMIT="${{GITHUB_SHA::12}}"` (release.yml), so 12 hex, `unknown` from a source archive. The design's example shows 40 (question 406: 12 hex, 7 to 40 accepted).
3. **`ryupdm.json`'s format.** ryupdm 6.A's plan (438, question 399) takes format 3 for teams and revoked keys and is executing; by design 261's rule ("whichever lands first takes the next"), packages take 4 if 6.A has merged when Task 4 runs, else 3. The prototype wrote 3 (main had no 6.A); the executor checks `config::FORMAT` on `main` first (question 407: the next format when Task 4 runs).
4. **Paths.** `<number>` in file names is the PDM number with anything but ASCII letters, digits, `-`, `_` and `.` made `_` (`file_safe`), so paths stay ASCII as the design requires.
5. **Writing.** `--out FILE.zip` writes the zip, `--out DIR/` (or an existing folder) the files unzipped, and no `--out` writes `<number>-<label>.zip` into the current folder (`into`, so the daemon names it). `out` is never a project file (`export::out_path`'s rule). A cancel stops it before it writes (`job.writing()`), as `export`'s does (#487).
6. **Children.** An assembly's `children` lists each part it places directly (sorted by project, part, label, each once), `package: null` always in 7.A: nothing here knows another package's key, and the service runs this same command (question 409: null in 7.A, left out of the hash so a later stage can fill it).
7. **Warnings.** A drawing finding of severity `error` (`drawing_view_failed`, unresolved references) fails the package; `drawing_view_uncertain` is a warning in the answer's and the manifest's `warnings`; lint findings are neither (they don't change what the drawing is).
8. **The golden** pins every file's bytes but the record's, and the manifest with `commit`, `content`, `record` and the STEP's and record's hashes left out: a record names the build that made it (`built_with`), so commits differ between a development and a stamped build, which `deps`' goldens already avoid hashing. The zip's own bytes are pinned by `package::zip`'s unit test and checked alike on one thread and four (question 410).
9. **The render** is design 436's command as written, `render P --view iso` at 1600×1200 with `render`'s other defaults (sketches and datums drawn, appearance on, no participant's view) (question 408).

## Measured (the prototype)

On cox-worker-4 (AMD Opteron 6380, 8 threads, 16 GB, shared with ryukei 13.D's executor; `CARGO_BUILD_JOBS=4`), debug builds, 2026-11-10, on `{BRANCH}` at `{PROTO}`:

MEASURED

## Tasks

| Task | Work item | Description |
|------|-----------|-------------|
TASK_TABLE
"""

TASKS = []
def task(**k):
    TASKS.append(k)

task(title="Error codes and the package job phase", item=6152,
 intro="Question 405 answered: reuse `label_not_found` and `label_not_official`; the four new codes are these. The codes every later task returns, and the progress phase `release.package` reports.",
 files="modify `crates/ryu-doc/src/error.rs` (the enum's end and its tests); modify `crates/ryu-session/src/job.rs` (`Phase`)",
 test="the `package_codes_are_the_agents_to_fix` test in the diff below (its snake_case names and exit 1).",
 fail="`cargo test -p ryu-doc package_codes` fails to compile: `no variant named PackagesFormatUnknown found for enum Code`.",
 ok="`cargo test -p ryu-doc package_codes` prints `test error::tests::package_codes_are_the_agents_to_fix ... ok`; `cargo test -p ryu-session` passes (59 tests in the prototype, 0.30 s).",
 commit='git commit -m "ryu-doc: release package codes; ryu-session: the package phase"')

task(title="The package zip writer and reader", item=6153,
 intro="Question 411 answered: the writer and its bounded reader live here, in ryupdm. A zip that is the same bytes for the same files everywhere, and a reader that bounds every size, since a package is something a person was sent.",
 files="create `crates/ryupdm/src/package/zip.rs`; modify `crates/ryupdm/Cargo.toml` (`miniz_oxide`)",
 test="the module's two tests: the bytes of a two-file zip pinned (`7f4a7436…`) and read back; sizes, a named limit, a flipped CRC, garbage and every truncation of the zip (`for cut in 0..z.len()`) give typed `Unread`s, never a panic.",
 fail="with only `pub mod package;` in `lib.rs` and an empty `package/mod.rs` holding `pub mod zip;`, `cargo test -p ryupdm package::zip` fails to compile until the file exists.",
 impl="create `zip.rs` as below. The made-by version is Unix's (3 << 8 | 20) so external attributes carry mode 0644 (`zipinfo` shows `-rw-r--r-- 2.0 unx … defN 80-Jan-01 00:00`).",
 ok="`cargo test -p ryupdm package::zip` prints `2 passed`. Run `unzip -l` and `python3 -I -c \"import zipfile;print(zipfile.ZipFile(...).testzip())\"` on a package after Task 9: `None` (no bad entry).",
 commit='git commit -m "ryupdm: package::zip, deterministic and bounded"')

task(title="The manifest, formats, key and package hash", item=6154,
 intro="Question 406 answered: `built_with.commit` is what `ryucad version` gives (`commit_ok`: 7 to 40 hex, or `unknown`). `ryupdm.package`, format 1, design 436's manifest; `Format`; the input key and the package hash; `archive` (manifest first, then path order).",
 files="create `crates/ryupdm/src/package/mod.rs`; modify `crates/ryupdm/src/lib.rs`",
 test="the module's tests: a manifest round-trips through its canonical bytes (and a newer format is `format_too_new`, a path `../etc/passwd` refused at `/files/1/path`); a child's `package` changes the bytes but not the hash, a format or the ryucad version changes key and hash; formats read sorted, `stl` is `packages_format_unknown` at `/packages/formats/1`; paths are plain ASCII and relative.",
 fail="`cargo test -p ryupdm package::tests` fails to compile until `mod.rs` holds `Manifest`.",
 ok="`cargo test -p ryupdm package::tests` prints `4 passed`.",
 commit='git commit -m "ryupdm: the package manifest, its key and hash"')

task(title="packages.formats in ryupdm.json", item=6155,
 intro="Question 407 answered: the next format when this task runs. `packages: {formats: [...]}`, at `ryupdm.config`'s next format, with the default when absent. **Before starting:** read `config::FORMAT` on `main`. If ryupdm 6.A has made it 3 (teams and revoked keys, plan 438), packages need 4: change `FORMAT` to 4, `format()`'s `3` for packages to 4 (after 6.A's own branch for 3), the refusal's text and `declared < 3` to `declared < 4`, and the test's `json!(3)` to `json!(4)`.",
 files="modify `crates/ryupdm/src/config.rs`",
 test="`packages_are_the_default_unless_written_and_need_format_3` in the diff; and `faults_point_at_their_field` now names `FORMAT + 1` as too new.",
 fail="`cargo test -p ryupdm config::tests::packages` fails to compile: `no field packages on type Config`.",
 ok="`cargo test -p ryupdm config::` passes (the prototype's whole `ryupdm` lib: 130 passed in 8.87 s).",
 commit='git commit -m "ryupdm: packages.formats in ryupdm.json"')

task(title="Package attestations", item=6156,
 intro="`ryupdm.package_attestation`, format 1, signed over its own domain, and the verdict on a release's `.package` tag. `attest.rs` gains `split_message` and `check_signature`, which `parse_message` and `verify_message` now use (the release attestation's behaviour and bytes are unchanged; its tests pass as they were).",
 files="modify `crates/ryupdm/src/attest.rs`; create `crates/ryupdm/src/package/attest.rs`; add `pub mod attest;` to `package/mod.rs`",
 test="`a_signed_attestation_verifies_and_round_trips` (the same message twice, a wrong hash `Mismatch(\"hash\")`, no keys `UnknownKey`) and `neither_kind_of_signature_verifies_as_the_other` (a package attestation signed over the release domain is `BadSignature`, and so is its signature checked over the release domain).",
 fail="`cargo test -p ryupdm package::attest` fails to compile until the file exists.",
 ok="`cargo test -p ryupdm attest` passes: `package::attest`'s 2 tests and `attest`'s own as before.",
 commit='git commit -m "ryupdm: package attestations over their own domain"')

task(title="Verifying a package's files and its release", item=6157,
 intro="`verify::files` (the zip against its manifest) and `verify::release` (the manifest against the release this clone holds), with stable clause names.",
 files="create `crates/ryupdm/src/package/verify.rs`; add `pub mod verify;` to `package/mod.rs`",
 test="`a_package_verifies_and_each_tampering_has_its_clause`: a file changed, an extra file, a missing file, a wrong record hash, a manifest listing another record, the manifest not first, garbage.",
 fail="`cargo test -p ryupdm package::verify` fails to compile until the file exists.",
 ok="`cargo test -p ryupdm package::` prints `7 passed` with Tasks 2, 3 and 5's.",
 commit='git commit -m "ryupdm: verifying a package offline"')

task(title="Package tags in history and the release check", item=6158,
 intro="A `.package` tag answers to its release, so it is no orphan while the release is official; `release check` reports `release_package_invalid` (an error) for one that doesn't verify, never a missing one.",
 files="modify `crates/ryupdm/src/history.rs` (`orphaned_tags`); modify `crates/ryupdm/src/check.rs` (the table, `CODES`, `run`)",
 test="ryucad's `a_package_tag_verifies_under_a_trusted_key_only` (Task 11) holds both from the CLI's side: after a `.package` tag under an untrusted key, `release check` fails naming `release_package_invalid` and never `release_tag_orphaned`. Here, the existing `check::tests` and `history::tests` keep passing (14 codes).",
 fail="before the change, Task 11's test fails with `release_tag_orphaned` in the findings.",
 ok="`cargo test -p ryupdm` passes (lib 130, api 6 with 1 ignored, structure_props 4 with 1 ignored in the prototype).",
 commit='git commit -m "ryupdm: package tags in history and release check"')

task(title="Building a package from its official commit", item=6159,
 intro="Questions 408 and 409 answered: the render is design 436's command with `render`'s defaults; `children[].package` is always `null`. `ryucad::package::build`: resolve the label in the clone's history, read the formats, number, drawings and children at the official commit, build each file through the agents' methods, fail whole on any failure, and make the manifest. `server::render_for_package` is the one render it makes.",
 files="create `crates/ryucad/src/package.rs` (the module up to `described`, and its tests module); modify `crates/ryucad/src/lib.rs`; modify `crates/ryucad/src/server.rs` (`render_for_package`)",
 test="in `package::tests`: `each_release_is_packaged_from_its_own_commit` (A at A's commit, B at B's, the paths, A's title block `REV` A and B's B, two builds the same zip and key); `proposed_obsolete_and_missing_releases_have_no_package` (`label_not_found`, `label_not_official`, `release_obsolete`); `an_assemblys_package_lists_its_children_and_its_bom` (an assembly placing `p` at A: its child, its BOM files, `,100,,p,A,1,each,` in the flat CSV).",
 fail="`cargo test -p ryucad --lib package::tests` fails to compile until `build` exists.",
 ok="`cargo test -p ryucad --lib package::tests` prints these three `ok` (with Tasks 10 and 11's, all six passed in 19.6 s on the prototype).",
 commit='git commit -m "ryucad: building a release package"')

task(title="The release.package request", item=6160,
 intro="`release.package` (a long request): build, then write the zip to `out`, the files to a folder, the zip into `into` under its own name, or answer the bytes. `package_stamped` names a fixed ryucad for the goldens.",
 files="modify `crates/ryucad/src/package.rs` (`described` to `package_stamped`); modify `crates/ryucad/src/proto.rs` (the two requests and the round-trip list); modify `crates/ryucad/src/server.rs` (`answer_live`'s arm, `dispatch`'s refusal); modify `crates/ryucad/src/daemon.rs` (`LONG`)",
 test="`proto::tests`' round trip of every request, which now lists `ReleasePackage` and `ReleasePackageVerify`; Task 14's CLI scenario writes through it both ways.",
 fail="`cargo test -p ryucad --lib proto::` fails to compile (`no variant ReleasePackage`).",
 ok="`cargo test -p ryucad --lib proto::` passes; `daemon::LONG` has 22 methods.",
 commit='git commit -m "ryucad: the release.package request"')

task(title="The release.package.verify request", item=6161,
 intro="`release.package.verify`: the file's verdict against the clone, its tag, and with `rebuild` its bytes built again.",
 files="modify `crates/ryucad/src/package.rs` (`invalid`, `verify`, and one test); modify `crates/ryucad/src/server.rs` (`dispatch`)",
 test="`a_package_verifies_offline_and_tampering_is_named`: written by `package_stamped` with this build, it verifies `unattested` and `rebuilt: true`; a byte added to its STEP is `package_invalid` with clause `file_changed`.",
 fail="`cargo test -p ryucad --lib package::tests::a_package_verifies` fails to compile until `verify` exists.",
 ok="the test passes.",
 commit='git commit -m "ryucad: release.package.verify"')

task(title="release list's package and pdm configure --package-formats", item=6162,
 intro="`release list` gives each official release record `package: {tag, hash?, key?, built_with?, clause?}`; `pdm.configure` takes `package_formats` (`[]` for the default) and `pdm.config` answers `packages`.",
 files="modify `crates/ryucad/src/release.rs`; modify `crates/ryucad/src/pdm.rs`; modify `crates/ryucad/src/proto.rs` (`PdmConfigure`); modify `crates/ryucad/src/server.rs` (`dispatch`); add the two tag tests to `package.rs`",
 test="`a_package_tag_verifies_under_a_trusted_key_only` (a tag signed by the test key, which nothing trusts: verify `tag_unknown_key`, list `{\"tag\": \"invalid\", \"clause\": \"unknown_key\"}`, check `release_package_invalid` and no orphan) and `a_package_tag_under_a_trusted_key_verifies` (the key trusted through `approvals.keys` with a rule for no part of the project: `verified`, list `attested` with the hash; a package of other formats under that tag is `tag_hash`).",
 fail="the two tests fail: `listed[\"records\"][0][\"package\"]` is `null`.",
 ok="`cargo test -p ryucad --lib package::tests` prints `6 passed`.",
 commit='git commit -m "ryucad: release list reads package tags; pdm configure sets package formats"')

task(title="The release package CLI", item=6163,
 intro="`ryucad release package <part> [--label] [--out FILE.zip|DIR/] [--formats a,b]` and `ryucad release package verify FILE [--rebuild]`, beside `pdm configure --package-formats a,b | --default-package-formats`. Paths are made absolute (and a trailing `/` kept), as `export`'s are; no `--out` sends `into: <cwd>`.",
 files="modify `crates/ryucad/src/cli.rs` (`PdmCmd::Configure`, `ReleaseCmd::Package`, `PackageArgs`, `PackageCmd`, and `request`)",
 test="`every_protocol_method_has_a_command` in `tests/cli.rs` (it fails without the commands); Task 14's scenario.",
 fail="`cargo test -p ryucad --test cli every_protocol_method_has_a_command` fails naming `release.package`.",
 ok="it passes; `ryucad release package --help` shows `Usage: ryucad release package [OPTIONS] <PART>` and `ryucad release package <COMMAND>`.",
 commit='git commit -m "ryucad: release package and verify in the CLI"')

task(title="The review package clients", item=6164,
 intro="`ryucad review package get|build|share` against 7.B's API: `get` downloads through the signed link (up to 4 GiB), writes the file, then verifies it against the clone in process; `build` and `share` post.",
 files="modify `crates/ryucad/src/review.rs` (`MAX_PACKAGE`, `download_up_to`, the three actions); modify `crates/ryucad/src/cli.rs` (`ReviewCmd::Package`, `ReviewPackageCmd`, `run`)",
 test="Task 14's additions to `review_against_a_fake_service`.",
 fail="`cargo test -p ryucad --test cli review_against_a_fake_service` fails: `unrecognized subcommand 'package'` in the envelope.",
 ok="it passes.",
 commit='git commit -m "ryucad: review package get, build and share"')

task(title="packages_from_the_cli and the review scenario", item=6165,
 intro="The CLI scenario both ways (in process and through the daemon) with one project ID and fixed commit dates, so both build the same bytes; the fake service's package routes.",
 files="modify `crates/ryucad/tests/cli.rs`",
 test="the diff below is the test.",
 fail="before Tasks 9 to 13 it fails at `release package` (`unrecognized subcommand`).",
 ok="`cargo test -p ryucad --test cli -- packages_from_the_cli review_against_a_fake_service releases_from_the_cli every_protocol_method_has_a_command` passes (TIME_CLI).",
 commit='git commit -m "ryucad: packages from the CLI, both ways"')

task(title="The packages golden section", item=6166,
 intro="Question 410 answered: every file but the record, and the manifest with what names commits left out. A part with a drawing released at A and an assembly placing it, packaged with every format on one thread and four.",
 files="modify `crates/ryucad/tests/determinism.rs`; regenerate `crates/ryucad/tests/golden/rebuild.txt` (only the new `packages` line)",
 test="`section::packages`.",
 fail="`cargo test -p ryucad --test determinism section::listed` fails: `packages` isn't in the golden.",
 impl="add the section as below, then `UPDATE_GOLDEN=1 cargo test -p ryucad --test determinism section::packages` and check that `git diff crates/ryucad/tests/golden/rebuild.txt` adds the `packages` line and changes nothing else.",
 ok="`cargo test -p ryucad --test determinism section::` passes (TIME_GOLDEN).",
 commit='git commit -m "ryucad: the packages golden section"')

task(title="Packages in the pdm fuzz target", item=6167,
 intro="The `pdm` target reads its bytes as a package's zip, a manifest and a package tag's message: a typed verdict, never a panic; a package that verifies, written again, reads back the same; a manifest round-trips.",
 files="modify `fuzz/fuzz_targets/pdm.rs`",
 test="the target itself.",
 fail="(no failing test: the target's assertions are the test).",
 ok="`cd fuzz && cargo +nightly fuzz run pdm -- -max_total_time=60` ends with no crash (FUZZ).",
 commit='git commit -m "fuzz: the pdm target reads packages"')

task(title="docs/claude/ryupdm.md: release packages", item=6168,
 intro="What this plan decided, in ryupdm's area file (CLAUDE.md is not touched: the area already has its import line).",
 files="modify `docs/claude/ryupdm.md` (a new section at its end)",
 test="none (documentation).",
 fail="none.",
 impl="append the section below, with the measured times from Tasks 14 and 15 on the machine they ran.",
 ok="`git diff docs/claude/ryupdm.md` shows only the new section.",
 commit='git commit -m "docs: release packages (ryupdm 7.A)"')

DOCS_BLOCK = None

TAIL = """## Definition of done

- `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo test -p ryu-doc -p ryu-session -p ryupdm`, `cargo test -p ryucad --lib`, `cargo test -p ryucad --test cli`, `cargo test -p ryucad --test review`, `cargo test -p ryucad --test determinism` (on a worker other sessions aren't building on, under `timeout 45m`, `CARGO_BUILD_JOBS=4` on cox-worker-4).
- The `pdm` fuzz target builds and runs a minute clean.
- One package of a real part (the showcase bracket or the gearbox corpus's `housing`) built through the CLI, opened by `unzip -l` and Python's `zipfile.testzip()`, verified with `--rebuild`, and its time recorded with the machine and load in `docs/claude/ryupdm.md`.
- The PR (`Seiraiyu/ryucad`) names the departures above. Release edges: 7.B waits on the ryucad release holding 7.A (question 384), which the coxswain wires.

## Not in this plan

The service's package job, storage, signing, `.package` tags, the Releases page, share links and the e2e (7.B); docs, design 21's row and the service's README (7.C). A `children[].package` filled in (question 409). Rebuilding with a newer ryucad (design 436: a new package is a new release).
"""

FILL = {
    "TIME_CLI": "the four passed in 47.2 s at a load of about 21; `every_protocol_method_has_a_command` and `packages_from_the_cli` again in 45.3 s at 18 to 19",
    "TIME_GOLDEN": "regenerated in 30.8 s at a load of 17 to 21, checked again with `section::listed` in 32.5 s at 15.7 to 16.4",
    "FUZZ": "`cargo +nightly fuzz run pdm -- -max_total_time=60 -rss_limit_mb=4096`: 58,091 runs in 61 s, no crash, after a nightly build of the target",
}
