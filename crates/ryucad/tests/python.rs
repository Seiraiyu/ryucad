//! The Python client under `python/` (ryucad plan 32.A): its generated
//! codes held in step with `ryu_doc::error::Code`, and its tests run
//! against the ryucad this workspace just built.

use ryucad::error::Code;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const HEADER: &str = "\
# Generated from ryu_doc::error::Code by crates/ryucad/tests/python.rs:
# never edit it by hand. After adding a code, run
#   UPDATE_PYTHON=1 cargo test -p ryucad --test python
\"\"\"Every stable error code ryucad's products answer, and each one's exit
class: 1 the agent can fix it, 2 usage, 3 not a usable project, 4 not the
agent's fault.\"\"\"

from enum import Enum


class Code(str, Enum):
    \"\"\"A stable error code: never renamed or reused.\"\"\"

";

fn name(c: Code) -> String {
    serde_json::to_value(c)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

/// `python/src/ryucad/_codes.py` as `Code::ALL` makes it.
fn codes_py() -> String {
    let mut out = HEADER.to_owned();
    for &c in Code::ALL {
        let n = name(c);
        out.push_str(&format!("    {} = \"{n}\"\n", n.to_uppercase()));
    }
    out.push_str("\n\n#: Each code's exit class.\nEXIT: dict[Code, int] = {\n");
    for &c in Code::ALL {
        out.push_str(&format!(
            "    Code.{}: {},\n",
            name(c).to_uppercase(),
            c.exit_code()
        ));
    }
    out.push_str("}\n");
    out
}

/// The committed `_codes.py` is what `Code::ALL` makes: a new code
/// without the file made again fails PR CI. `UPDATE_PYTHON=1` writes it.
#[test]
fn the_codes_are_ryu_docs() {
    let path = root().join("python/src/ryucad/_codes.py");
    let want = codes_py();
    if std::env::var_os("UPDATE_PYTHON").is_some() {
        std::fs::write(&path, &want).unwrap();
        return;
    }
    let got = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        got == want,
        "python/src/ryucad/_codes.py is out of step with ryu_doc::error::Code: \
         run UPDATE_PYTHON=1 cargo test -p ryucad --test python"
    );
}

/// The interpreter: `python3`, else `python` (Windows), if it is 3.10 or
/// newer.
fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|p| {
        Command::new(p)
            .args([
                "-c",
                "import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)",
            ])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// The package's unit and integration tests, against the ryucad this
/// workspace built: in PR CI on Linux x64, in full.yml on every platform
/// (with `RYUCAD_REQUIRE_PYTHON` set, a missing python3 fails there).
/// Each project is made in a temporary folder outside the repository.
#[test]
fn the_package_passes_its_tests_against_this_build() {
    let Some(python) = python() else {
        assert!(
            std::env::var_os("RYUCAD_REQUIRE_PYTHON").is_none(),
            "no python3 (3.10 or newer)"
        );
        eprintln!("skipped: no python3 (3.10 or newer)");
        return;
    };
    let scratch = tempfile::tempdir().unwrap();
    let root = root();
    let run = Command::new(python)
        .args(["-m", "unittest", "discover", "-v", "-s"])
        .arg(root.join("python/tests"))
        .current_dir(scratch.path())
        .env("PYTHONPATH", root.join("python/src"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("RYUCAD_EXE", env!("CARGO_BIN_EXE_ryucad"))
        .env("TMPDIR", scratch.path())
        .env("TEMP", scratch.path())
        .env("TMP", scratch.path())
        // No repository above the scratch folder is found (#1004).
        .env("GIT_CEILING_DIRECTORIES", scratch.path())
        .env_remove("RYUCAD_NO_DAEMON")
        .env_remove("RYUCAD_PARTICIPANT")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "{}\n{err}",
        String::from_utf8_lossy(&run.stdout)
    );
    // The integration tests ran: RYUCAD_EXE was set.
    assert!(!err.contains("RYUCAD_EXE names no ryucad"), "{err}");
}
