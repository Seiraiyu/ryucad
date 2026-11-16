//! The Python client under `python/` (ryucad plan 32.A): its generated
//! codes held in step with `ryu_doc::error::Code`, and its tests run
//! against the ryucad this workspace just built.

use ryucad::error::Code;
use std::path::{Path, PathBuf};

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
