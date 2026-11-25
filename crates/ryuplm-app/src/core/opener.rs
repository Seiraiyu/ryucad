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

/// Opens `path` with `program`, else the OS's opener, detached. The path
/// is one argument, never a command line: on Windows the shell opens it
/// (ShellExecuteW), as no `cmd /C start` would, since cmd reads `&`, `^`
/// and `%` in a file name as its own.
pub fn open_with(program: Option<&OsStr>, path: &Path) -> Result<()> {
    let failed = |why: String| {
        Error::new(
            Code::Internal,
            format!("nothing opened {}: {why}", path.display()),
        )
        .with("path", path)
    };
    let mut cmd = match program {
        Some(p) => Command::new(p),
        None if cfg!(windows) => {
            refuse_programs(path)?;
            return shell_open(path).map_err(failed);
        }
        None if cfg!(target_os = "macos") => Command::new("open"),
        None => Command::new("xdg-open"),
    };
    cmd.arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_detached(&mut cmd)
        .map(drop)
        .map_err(|e| failed(e.to_string()))
}

/// What Windows' shell runs rather than opens, beside `PATHEXT`'s.
const PROGRAMS: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "msc",
    "msi",
    "msp",
    "scr",
    "lnk",
    "url",
    "hta",
    "cpl",
    "pif",
    "reg",
    "appref-ms",
    "application",
    "jar",
    "inf",
    "scf",
];

/// Whether Windows' shell would run a file named `name` rather than
/// open it: its extension a program's, `PATHEXT`'s or [`PROGRAMS`].
pub fn runs_on_windows(name: &str, pathext: &str) -> bool {
    // Windows drops a name's trailing dots and spaces.
    let Some((_, ext)) = name.trim_end_matches([' ', '.']).rsplit_once('.') else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    PROGRAMS.contains(&ext.as_str())
        || pathext
            .split(';')
            .any(|p| p.trim().trim_start_matches('.').eq_ignore_ascii_case(&ext))
}

/// A document a committer named `x.bat` is left where it was written:
/// the shell's Open would run it.
fn refuse_programs(path: &Path) -> Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let pathext = std::env::var("PATHEXT").unwrap_or_default();
    if name
        .as_deref()
        .is_some_and(|n| runs_on_windows(n, &pathext))
    {
        let folder = path.parent().unwrap_or(path);
        return Err(Error::new(
            Code::Internal,
            format!(
                "{} is a program's file: Windows would run it rather than open it, so it is left in {}",
                path.display(),
                folder.display()
            ),
        )
        .with("path", path));
    }
    Ok(())
}

/// `path` as ShellExecuteW's file: its UTF-16, NUL-ended, as it is.
pub fn wide(path: &Path) -> Vec<u16> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(windows)]
fn shell_open(path: &Path) -> std::result::Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let file = wide(path);
    // SAFETY: `file` is NUL-ended and outlives the call; every other
    // pointer is null, which ShellExecuteW takes as the default verb, no
    // parameters and no folder.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Above 32 is success; anything else is one of its error numbers.
    if r as usize > 32 {
        Ok(())
    } else {
        Err(format!("the shell refused it (error {})", r as usize))
    }
}

#[cfg(not(windows))]
fn shell_open(_: &Path) -> std::result::Result<(), String> {
    unreachable!("the shell opens documents on Windows only")
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

    #[test]
    fn a_name_cmd_would_read_goes_to_the_shell_as_it_is() {
        // `&` ends cmd's command, `^` escapes, `%PATH%` expands and the
        // parentheses group: each allowed in a document's file name.
        let name = "spec&calc ^x %PATH% (1).pdf";
        assert!(ryuplm::document::name_ok(name), "a name ryuplm allows");
        let p = temp_path("d000000000001", name);
        let w = wide(&p);
        assert_eq!(w.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&w[..w.len() - 1]).unwrap(),
            p.to_string_lossy(),
            "the file, with nothing quoted or added"
        );
    }

    #[test]
    fn a_program_s_file_is_never_run_on_windows() {
        let pathext = ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.WS;.MSC";
        for n in [
            "evil.exe", "a.BAT", "x.cmd", "y.ps1", "z.lnk", "w.ws", "v.js. ",
        ] {
            assert!(runs_on_windows(n, pathext), "{n}");
        }
        for n in [
            "spec.pdf",
            "drawing.dxf",
            "notes",
            "spec&calc.pdf",
            "a.exe.pdf",
        ] {
            assert!(!runs_on_windows(n, pathext), "{n}");
        }
        assert!(runs_on_windows("m.foo", ".FOO"), "PATHEXT's own");
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
