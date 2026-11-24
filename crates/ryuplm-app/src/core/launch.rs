//! Programs the window starts, detached: ryucad's or ryueda's window on a
//! linked representation's folder (`ryucad open`, `ryueda open`), when
//! that program is installed (design 938, the Item tile).

use ryu_doc::error::{Code, Error, Result};
use ryu_session::client::{find_program, spawn_detached};
use std::process::{Command, Stdio};

fn program(product: &str) -> Option<std::path::PathBuf> {
    matches!(product, "ryucad" | "ryueda")
        .then(|| find_program(product))
        .flatten()
}

/// Whether `product`'s program is beside the app or on `PATH`.
pub fn installed(product: &str) -> bool {
    program(product).is_some()
}

/// Runs `<product> open <folder>`: the product's own window on it.
pub fn representation(product: &str, folder: &str) -> Result<()> {
    let exe = program(product).ok_or_else(|| {
        Error::new(
            Code::AppNotInstalled,
            format!("{product} isn't installed here"),
        )
        .with("product", product)
    })?;
    let mut cmd = Command::new(&exe);
    cmd.arg("open")
        .arg(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_detached(&mut cmd).map(drop).map_err(|e| {
        Error::new(
            Code::AppNotInstalled,
            format!("{product} open didn't start: {e}"),
        )
        .with("path", &exe)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ryucad_and_ryueda_open_representations() {
        assert!(!installed("ryudfm"));
        assert_eq!(
            representation("ryudfm", "/nowhere").unwrap_err().code,
            Code::AppNotInstalled
        );
    }
}
