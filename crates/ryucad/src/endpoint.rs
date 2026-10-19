//! Where a project's daemon listens. One per user and project, outside the
//! project: a Unix domain socket in a private folder, or a named pipe that
//! only its owner may open.

use crate::error::{Code, Error, Result};
use interprocess::local_socket::Name;
use std::io;
use std::path::{Path, PathBuf};

/// Overrides the runtime folder; tests use it to keep their daemons apart.
pub const RUNTIME_DIR_VAR: &str = "RYUCAD_RUNTIME_DIR";

#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    /// The socket file (Unix) or pipe name (Windows).
    pub address: String,
    /// The daemon's log file.
    pub log: PathBuf,
}

/// The per-user folder for sockets and logs, created if needed. On Unix it
/// must belong to this user and be closed to everyone else.
pub fn runtime_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os(RUNTIME_DIR_VAR) {
        Some(d) => PathBuf::from(d),
        None => default_runtime_dir(),
    };
    create_private(&dir).map_err(|e| {
        Error::new(
            Code::Daemon,
            format!("runtime folder {}: {e}", dir.display()),
        )
        .with("path", &dir)
    })?;
    Ok(dir)
}

/// Never from `$TMPDIR`: agent sessions, sudo, IDE terminals and launchd
/// jobs set their own, and a CLI that looked elsewhere than the app would
/// start a second daemon the app never sees.
#[cfg(all(unix, not(target_os = "macos")))]
fn default_runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("ryucad"),
        // SAFETY: getuid has no preconditions and can't fail.
        _ => PathBuf::from("/tmp").join(format!("ryucad-{}", unsafe { libc::getuid() })),
    }
}

/// The user's own temporary folder, as the system has it (`TMPDIR` is
/// usually set to it, but not always: see above).
#[cfg(target_os = "macos")]
fn default_runtime_dir() -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; 1024];
    // SAFETY: the buffer is as long as we say; confstr writes at most that,
    // NUL-terminated, and returns the length it needed (0 on failure).
    let n = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    // SAFETY: getuid has no preconditions and can't fail.
    let uid = unsafe { libc::getuid() };
    if n == 0 || n > buf.len() {
        return PathBuf::from("/tmp").join(format!("ryucad-{uid}"));
    }
    buf.truncate(n - 1);
    PathBuf::from(std::ffi::OsStr::from_bytes(&buf)).join("ryucad")
}

#[cfg(windows)]
fn default_runtime_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ryucad").join("run")
}

#[cfg(unix)]
fn create_private(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = std::fs::metadata(dir)?;
    // SAFETY: getuid has no preconditions and can't fail.
    if meta.uid() != unsafe { libc::getuid() } {
        return Err(io::Error::other("must belong to this user"));
    }
    // Ours but open to others (made by hand, say): close it.
    if meta.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(windows)]
fn create_private(dir: &Path) -> io::Result<()> {
    // Under %LOCALAPPDATA%, which only this user can read.
    std::fs::create_dir_all(dir)
}

/// The first 16 hex digits of the BLAKE3 hash of `s`.
fn short_hash(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex()[..16].to_owned()
}

/// The pipe prefix that marks this user's daemons on Windows, where pipe
/// names are machine-wide. It covers the runtime folder too, so a folder's
/// daemons are its own, as the sockets in it are on Unix.
#[cfg(windows)]
pub fn user_prefix(dir: &Path) -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let key = format!("{domain}\\{user}\0{}", dir.to_string_lossy());
    format!("ryucad-{}-", short_hash(&key))
}

impl Endpoint {
    /// The endpoint for the project at `root` (already canonical). A
    /// verbatim Windows path (`\\?\C:\…`) and its ordinary form name the
    /// same project, so they get the same endpoint.
    pub fn for_project(root: &Path) -> Result<Endpoint> {
        let dir = runtime_dir()?;
        let root = crate::doc::project::plain(root.to_path_buf());
        let hash = short_hash(&root.to_string_lossy());
        #[cfg(unix)]
        let address = dir
            .join(format!("{hash}.sock"))
            .to_string_lossy()
            .into_owned();
        #[cfg(windows)]
        let address = format!("{}{hash}", user_prefix(&dir));
        Ok(Endpoint {
            address,
            log: dir.join(format!("{hash}.log")),
        })
    }

    /// Every endpoint this user has, live or stale: for `daemon stop --all`.
    pub fn all() -> Result<Vec<Endpoint>> {
        let dir = runtime_dir()?;
        let mut out = Vec::new();
        #[cfg(unix)]
        let entries = std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, &e))?;
        #[cfg(windows)]
        let entries =
            std::fs::read_dir(r"\\.\pipe\").map_err(|e| Error::io(Path::new(r"\\.\pipe\"), &e))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            #[cfg(unix)]
            let hash = name.strip_suffix(".sock").map(str::to_owned);
            #[cfg(windows)]
            let hash = name.strip_prefix(&user_prefix(&dir)).map(str::to_owned);
            if let Some(hash) = hash {
                #[cfg(unix)]
                let address = entry.path().to_string_lossy().into_owned();
                #[cfg(windows)]
                let address = name.clone();
                out.push(Endpoint {
                    address,
                    log: dir.join(format!("{hash}.log")),
                });
            }
        }
        out.sort_by(|a, b| a.address.cmp(&b.address));
        Ok(out)
    }

    /// The name to bind or connect to.
    pub fn name(&self) -> io::Result<Name<'_>> {
        use interprocess::local_socket::prelude::*;
        #[cfg(unix)]
        return self
            .address
            .as_str()
            .to_fs_name::<interprocess::local_socket::GenericFilePath>();
        #[cfg(windows)]
        return self
            .address
            .as_str()
            .to_ns_name::<interprocess::local_socket::GenericNamespaced>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_endpoint_per_project() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the only test in this binary that sets the variable.
        unsafe { std::env::set_var(RUNTIME_DIR_VAR, dir.path().join("run")) };
        let a = Endpoint::for_project(Path::new("/projects/a")).unwrap();
        let b = Endpoint::for_project(Path::new("/projects/b")).unwrap();
        assert_ne!(a.address, b.address);
        assert_eq!(a, Endpoint::for_project(Path::new("/projects/a")).unwrap());
        #[cfg(windows)]
        assert_eq!(
            Endpoint::for_project(Path::new(r"\\?\C:\projects\a")).unwrap(),
            Endpoint::for_project(Path::new(r"C:\projects\a")).unwrap()
        );
        assert!(a.log.starts_with(dir.path().join("run")));
        assert!(a.name().is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("run"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
            // A folder of ours that others can read is closed.
            std::fs::set_permissions(
                dir.path().join("run"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            runtime_dir().unwrap();
            let mode = std::fs::metadata(dir.path().join("run"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }
}
