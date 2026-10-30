//! Where a project's daemon listens. One per user and project, outside the
//! project: a Unix domain socket in a private folder, or a named pipe that
//! only its owner may open.

use crate::Product;
use interprocess::local_socket::Name;
use ryu_doc::error::{Code, Error, Result};
use std::io;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

/// Where product `P`'s daemon for one project listens.
pub struct Endpoint<P> {
    /// The socket file (Unix) or pipe name (Windows).
    pub address: String,
    /// The daemon's log file.
    pub log: PathBuf,
    pub(crate) product: PhantomData<fn() -> P>,
}

impl<P> Clone for Endpoint<P> {
    fn clone(&self) -> Self {
        Endpoint {
            address: self.address.clone(),
            log: self.log.clone(),
            product: PhantomData,
        }
    }
}

impl<P> std::fmt::Debug for Endpoint<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("address", &self.address)
            .field("log", &self.log)
            .finish()
    }
}

impl<P> PartialEq for Endpoint<P> {
    fn eq(&self, o: &Self) -> bool {
        self.address == o.address && self.log == o.log
    }
}

/// Product `P`'s per-user folder for sockets and logs, created if needed.
/// On Unix it must belong to this user and be closed to everyone else.
pub fn runtime_dir<P: Product>() -> Result<PathBuf> {
    let dir = match std::env::var_os(P::RUNTIME_DIR_VAR) {
        Some(d) => PathBuf::from(d),
        None => default_runtime_dir(P::NAME),
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
fn default_runtime_dir(name: &str) -> PathBuf {
    // SAFETY: getuid has no preconditions and can't fail.
    let uid = unsafe { libc::getuid() };
    unix_runtime_dir(name, std::env::var_os("XDG_RUNTIME_DIR"), uid, |d| {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        std::fs::symlink_metadata(d)
            .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.permissions().mode() & 0o077 == 0)
    })
}

/// `$XDG_RUNTIME_DIR/<name>` (`ryucad`); without it (cron, `sudo -u`,
/// some ssh shells) the folder logind makes for the user's desktop
/// session, if it is there and theirs alone, so a CLI started there finds
/// the app's daemon; else `/tmp/<name>-<uid>`.
#[cfg(all(unix, not(target_os = "macos")))]
fn unix_runtime_dir(
    name: &str,
    xdg: Option<std::ffi::OsString>,
    uid: u32,
    private: impl Fn(&Path) -> bool,
) -> PathBuf {
    match xdg {
        Some(d) if !d.is_empty() => PathBuf::from(d).join(name),
        _ => {
            let session = PathBuf::from(format!("/run/user/{uid}"));
            if private(&session) {
                session.join(name)
            } else {
                PathBuf::from("/tmp").join(format!("{name}-{uid}"))
            }
        }
    }
}

/// The user's own temporary folder, as the system has it (`TMPDIR` is
/// usually set to it, but not always: see above).
#[cfg(target_os = "macos")]
fn default_runtime_dir(name: &str) -> PathBuf {
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
        return PathBuf::from("/tmp").join(format!("{name}-{uid}"));
    }
    buf.truncate(n - 1);
    PathBuf::from(std::ffi::OsStr::from_bytes(&buf)).join(name)
}

#[cfg(windows)]
fn default_runtime_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(name).join("run")
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
pub fn user_prefix<P: Product>(dir: &Path) -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let key = format!("{domain}\\{user}\0{}", dir.to_string_lossy());
    format!("{}-{}-", P::NAME, short_hash(&key))
}

impl<P: Product> Endpoint<P> {
    /// The endpoint for the project at `root`. Every path to one folder
    /// names the same endpoint: `root` is made canonical where it exists
    /// (the daemon finds its root canonical, and a client handed a path
    /// through a link, as macOS's `/var` is to `/private/var`, would
    /// otherwise wait on a socket no daemon binds; #229), and a verbatim
    /// Windows path (`\\?\C:\…`) and its ordinary form are one.
    pub fn for_project(root: &Path) -> Result<Endpoint<P>> {
        let dir = runtime_dir::<P>()?;
        let root =
            ryu_doc::source::plain(root.canonicalize().unwrap_or_else(|_| root.to_path_buf()));
        let hash = short_hash(&root.to_string_lossy());
        #[cfg(unix)]
        let address = dir
            .join(format!("{hash}.sock"))
            .to_string_lossy()
            .into_owned();
        #[cfg(windows)]
        let address = format!("{}{hash}", user_prefix::<P>(&dir));
        Ok(Endpoint {
            address,
            log: dir.join(format!("{hash}.log")),
            product: PhantomData,
        })
    }

    /// Every endpoint this user has, live or stale: for `daemon stop --all`.
    pub fn all() -> Result<Vec<Endpoint<P>>> {
        let dir = runtime_dir::<P>()?;
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
            let hash = name
                .strip_prefix(&user_prefix::<P>(&dir))
                .map(str::to_owned);
            if let Some(hash) = hash {
                #[cfg(unix)]
                let address = entry.path().to_string_lossy().into_owned();
                #[cfg(windows)]
                let address = name.clone();
                out.push(Endpoint {
                    address,
                    log: dir.join(format!("{hash}.log")),
                    product: PhantomData,
                });
            }
        }
        out.sort_by(|a, b| a.address.cmp(&b.address));
        Ok(out)
    }
}

impl<P> Endpoint<P> {
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

    use crate::testing::Test;

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn without_xdg_runtime_dir_the_session_folder_is_found() {
        let yes = |_: &Path| true;
        let no = |_: &Path| false;
        assert_eq!(
            unix_runtime_dir("ryucad", Some("/run/user/7".into()), 7, no),
            Path::new("/run/user/7/ryucad")
        );
        // Unset or empty: logind's folder when it is there and private.
        for xdg in [None, Some("".into())] {
            assert_eq!(
                unix_runtime_dir("ryucad", xdg.clone(), 7, yes),
                Path::new("/run/user/7/ryucad")
            );
            assert_eq!(
                unix_runtime_dir("ryucad", xdg, 7, no),
                Path::new("/tmp/ryucad-7")
            );
        }
    }

    #[test]
    fn one_endpoint_per_project() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the only test in this binary that sets the variable.
        unsafe { std::env::set_var(Test::RUNTIME_DIR_VAR, dir.path().join("run")) };
        let a = Endpoint::<Test>::for_project(Path::new("/projects/a")).unwrap();
        let b = Endpoint::<Test>::for_project(Path::new("/projects/b")).unwrap();
        assert_ne!(a.address, b.address);
        assert_eq!(
            a,
            Endpoint::<Test>::for_project(Path::new("/projects/a")).unwrap()
        );
        #[cfg(windows)]
        assert_eq!(
            Endpoint::<Test>::for_project(Path::new(r"\\?\C:\projects\a")).unwrap(),
            Endpoint::<Test>::for_project(Path::new(r"C:\projects\a")).unwrap()
        );
        // A folder named through a link, or with `..`, is the folder: the
        // daemon binds where its canonical root says (#229).
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let canonical = Endpoint::<Test>::for_project(&real.canonicalize().unwrap()).unwrap();
        assert_eq!(
            canonical,
            Endpoint::<Test>::for_project(&real.join("..").join("real")).unwrap()
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert_eq!(canonical, Endpoint::<Test>::for_project(&link).unwrap());
        }
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
            runtime_dir::<Test>().unwrap();
            let mode = std::fs::metadata(dir.path().join("run"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }
}
