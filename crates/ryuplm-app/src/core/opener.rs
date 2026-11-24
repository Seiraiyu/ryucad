//! Opening a document's bytes (story 14555): where they go, the CLI that
//! fetches a service-stored document's (ryuplm 3.B), and the OS's opener.

use ryu_doc::error::{Code, Error, Result};
use ryu_session::client::spawn_detached;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Names a program to open documents with instead of the OS's (tests).
pub const OPENER_VAR: &str = "RYUPLM_OPENER";

/// Where document `id`'s bytes are written: a folder of this window's
/// own under the temporary folder, by the document's ID, under its file
/// name (already safe on every OS, design 937).
pub fn temp_path(id: &str, name: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!("ryuplm-app-{}", std::process::id()))
        .join(id)
        .join(name)
}

/// `ryuplm -C <root> --no-daemon document get <id> --out <out>`: the CLI
/// fetches a service-stored document's bytes before it writes them.
pub fn fetch_cli(exe: &Path, root: &Path, id: &str, out: &Path) -> Result<()> {
    let run = Command::new(exe)
        .arg("-C")
        .arg(root)
        .args(["--no-daemon", "document", "get", id, "--out"])
        .arg(out)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            Error::new(Code::AppNotInstalled, format!("ryuplm didn't start: {e}")).with("path", exe)
        })?;
    let v: serde_json::Value = serde_json::from_slice(&run.stdout).map_err(|_| {
        Error::new(
            Code::Internal,
            format!(
                "ryuplm said: {}",
                String::from_utf8_lossy(&run.stderr).trim()
            ),
        )
    })?;
    if v["ok"] == true {
        return Ok(());
    }
    Err(serde_json::from_value(v["error"].clone())
        .unwrap_or_else(|_| Error::new(Code::Internal, v["error"].to_string())))
}

/// Opens `path` with `program`, else the OS's opener, detached.
pub fn open_with(program: Option<&OsStr>, path: &Path) -> Result<()> {
    let mut cmd = match program {
        Some(p) => Command::new(p),
        None if cfg!(target_os = "macos") => Command::new("open"),
        None if cfg!(windows) => {
            let mut c = Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        }
        None => Command::new("xdg-open"),
    };
    cmd.arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_detached(&mut cmd).map(drop).map_err(|e| {
        Error::new(
            Code::Internal,
            format!("nothing opened {}: {e}", path.display()),
        )
        .with("path", path)
    })
}

/// [`open_with`] `RYUPLM_OPENER`, else the OS's opener.
pub fn open(path: &Path) -> Result<()> {
    let program = std::env::var_os(OPENER_VAR);
    open_with(program.as_deref(), path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_go_to_a_folder_of_their_own() {
        let p = temp_path("d000000000001", "spec.pdf");
        assert!(p.ends_with("d000000000001/spec.pdf"), "{}", p.display());
        assert!(p.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn the_cli_s_refusal_is_its_error() {
        let d = tempfile::tempdir().unwrap();
        // No ryuplm there: the CLI can't start.
        let e = fetch_cli(
            &d.path().join("no-ryuplm"),
            d.path(),
            "d000000000001",
            &d.path().join("x"),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::AppNotInstalled);
    }

    #[cfg(unix)]
    #[test]
    fn the_opener_runs_with_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let said = d.path().join("said");
        let opener = d.path().join("opener");
        std::fs::write(
            &opener,
            format!("#!/bin/sh\necho \"$1\" > '{}'\n", said.display()),
        )
        .unwrap();
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = d.path().join("spec.pdf");
        std::fs::write(&file, b"%PDF").unwrap();
        open_with(Some(opener.as_os_str()), &file).unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !said.exists() {
            assert!(std::time::Instant::now() < end, "the opener never ran");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(&said).unwrap().trim(),
            file.display().to_string()
        );
    }
}
