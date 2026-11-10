"""Writes the ryupdm 7.A plan from the prototype on cox/ryupdm-7a-plan-K468HCGN:
every code block is the prototype's own text (new files whole or in
parts, changed files as their diff hunks against BASE), so the plan quotes
what ran. Run: python3 gen.py > plan.md"""
import re, subprocess, sys, os
sys.path.insert(0, os.path.dirname(__file__))
from hunks import hunks, ROOT, BASE
sys.path.pop(0)
import prose

def read(path):
    return open(os.path.join(ROOT, path)).read()

def lang(path):
    return {"rs": "rust", "toml": "toml", "md": "markdown", "sh": "sh"}.get(path.rsplit(".", 1)[-1], "")

def block(path, text, note=None):
    out = [f"`{path}`" + (f" ({note})" if note else "") + ":", "", f"```{lang(path)}", text.rstrip("\n"), "```", ""]
    return "\n".join(out)

def diff(path, idx):
    hs = hunks(path)
    text = "".join(hs[i] for i in idx)
    return "\n".join([f"`{path}`:", "", "```diff", text.rstrip("\n"), "```", ""])

def between(path, start, end=None):
    s = read(path)
    i = s.index(start)
    j = s.index(end, i + len(start)) if end else len(s)
    return s[i:j]

PKG = "crates/ryucad/src/package.rs"
T = "#[cfg(test)]\npub(crate) mod tests {"

def pkg_tests_head():
    return between(PKG, T, "    #[test]\n    fn each_release_is_packaged")

def test_fn(name, path=PKG):
    s = read(path)
    i = s.index(f"    fn {name}(")
    i = s.rindex("    #[test]", 0, i)
    j = s.find("\n    #[test]", i + 10)
    if j < 0:
        j = s.rindex("\n}")
    return s[i:j].rstrip() + "\n"

def mod_rs_for_task3():
    s = read("crates/ryupdm/src/package/mod.rs")
    return s.replace("pub mod attest;\npub mod verify;\n", "")

CODE = {
    1: lambda: [diff("crates/ryu-doc/src/error.rs", [0, 1]), diff("crates/ryu-session/src/job.rs", [0])],
    2: lambda: [diff("crates/ryupdm/Cargo.toml", [0]), block("crates/ryupdm/src/package/zip.rs", read("crates/ryupdm/src/package/zip.rs"), "new")],
    3: lambda: [block("crates/ryupdm/src/package/mod.rs", mod_rs_for_task3(), "new; Tasks 5 and 6 add `pub mod attest;` and `pub mod verify;` beside `pub mod zip;`"), diff("crates/ryupdm/src/lib.rs", [0])],
    4: lambda: [diff("crates/ryupdm/src/config.rs", [0, 1, 2, 3, 4, 5, 6, 7])],
    5: lambda: [diff("crates/ryupdm/src/attest.rs", [0, 1, 2, 3]), block("crates/ryupdm/src/package/attest.rs", read("crates/ryupdm/src/package/attest.rs"), "new; and `pub mod attest;` in `package/mod.rs`")],
    6: lambda: [block("crates/ryupdm/src/package/verify.rs", read("crates/ryupdm/src/package/verify.rs"), "new; and `pub mod verify;` in `package/mod.rs`")],
    7: lambda: [diff("crates/ryupdm/src/history.rs", [0]), diff("crates/ryupdm/src/check.rs", [0, 1, 2, 3])],
    8: lambda: [block(PKG, between(PKG, "//! `release.package`", "/// The answer describing"), "new: the module, up to `described`"),
                 block(PKG, pkg_tests_head() + test_fn("each_release_is_packaged_from_its_own_commit") + "\n" + test_fn("proposed_obsolete_and_missing_releases_have_no_package") + "\n" + test_fn("an_assemblys_package_lists_its_children_and_its_bom") + "}\n", "its tests module, at the end of the file"),
                 diff("crates/ryucad/src/lib.rs", [0]), diff("crates/ryucad/src/server.rs", [5])],
    9: lambda: [block(PKG, between(PKG, "/// The answer describing", "/// How a verify ends"), "after `build`"),
                diff("crates/ryucad/src/proto.rs", [1, 2]), diff("crates/ryucad/src/server.rs", [0, 4]), diff("crates/ryucad/src/daemon.rs", [0, 1])],
    10: lambda: [block(PKG, between(PKG, "/// How a verify ends", T), "after `package_stamped`"),
                 block(PKG, test_fn("a_package_verifies_offline_and_tampering_is_named"), "in its tests module"),
                 diff("crates/ryucad/src/server.rs", [3])],
    11: lambda: [diff("crates/ryucad/src/release.rs", [0, 1, 2]), diff("crates/ryucad/src/pdm.rs", [0, 1, 2, 3]), diff("crates/ryucad/src/proto.rs", [0]), diff("crates/ryucad/src/server.rs", [1, 2]),
                 block(PKG, test_fn("a_package_tag_verifies_under_a_trusted_key_only") + "\n" + test_fn("a_package_tag_under_a_trusted_key_verifies"), "in its tests module")],
    12: lambda: [diff("crates/ryucad/src/cli.rs", [2, 3, 4, 5, 6, 7]), diff("crates/ryucad/tests/cli.rs", [0])],
    13: lambda: [diff("crates/ryucad/src/review.rs", [0, 1, 2, 3, 4]), diff("crates/ryucad/src/cli.rs", [0, 1, 8])],
    14: lambda: [diff("crates/ryucad/tests/cli.rs", [1, 2, 3])],
    15: lambda: [diff("crates/ryucad/tests/determinism.rs", [0, 1])],
    16: lambda: [diff("fuzz/fuzz_targets/pdm.rs", [0, 1, 2])],
    17: lambda: [diff("docs/claude/ryupdm.md", [0])],
}

table = "\n".join(f"| {n} | #{t['item']} | {t['title']} |" for n, t in enumerate(prose.TASKS, 1))
measured = open(os.path.join(os.path.dirname(__file__), "measured.md")).read().strip()
proto = subprocess.run(["git", "rev-parse", "--short=12", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
head = prose.HEAD.replace("TASK_TABLE", table).replace("MEASURED", measured).replace("PROTO_COMMIT", proto)
for k, v in prose.FILL.items():
    head = head.replace(k, v)
out = [head]
for n, t in enumerate(prose.TASKS, 1):
    out.append(f"## Task {n}: {t['title']} (#{t['item']})\n")
    out.append(t["intro"].strip() + "\n")
    out.append(f"**Files:** {t['files']}\n")
    out.append("**Step 1 -- failing test:** " + t["test"].strip() + "\n")
    out.append("**Step 2 -- see it fail:** " + t["fail"].strip() + "\n")
    out.append("**Step 3 -- implement:** " + t.get("impl", "the code below, as the prototype has it.").strip() + "\n")
    out.extend(CODE[n]())
    out.append("**Step 4 -- see it pass:** " + t["ok"].strip() + "\n")
    out.append(f"**Step 5 -- commit:** `{t['commit']}`\n")
out.append(prose.TAIL)
text = "\n".join(out)
for k, v in prose.FILL.items():
    text = text.replace(k, v)
print(text)
sys.exit(0)
print("\n".join(out))
