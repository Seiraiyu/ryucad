//! The window's connection to a catalogue's daemon: `ryu_app::link`'s,
//! as ryuplm's client ([`crate::core::product::Plm`]).

use crate::core::product::Plm;
use ryu_doc::error::Result;

pub use ryu_app::link::{Event, Wake};

pub type Link = ryu_app::link::Link<Plm>;
pub type Daemon = ryu_session::client::Daemon<ryuplm::Plm>;

/// The `ryuplm` binary the app starts daemons from: beside the app, else
/// on `PATH`, and of the app's own build.
pub fn find_ryuplm() -> Result<Daemon> {
    ryu_app::link::find::<Plm>()
}

/// The catalogue at or above `dir` (`ryuplm.json`), or
/// `project_not_found`.
pub fn find_root(dir: &std::path::Path) -> Result<std::path::PathBuf> {
    use ryu_doc::error::{Code, Error};
    let start = ryuplm::source::plain(dir.canonicalize().map_err(|e| Error::io(dir, &e))?);
    start
        .ancestors()
        .find(|d| d.join(ryuplm::catalogue::MANIFEST).is_file())
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| {
            Error::new(
                Code::ProjectNotFound,
                format!(
                    "no ryuplm.json in {} or any folder above it",
                    start.display()
                ),
            )
            .with("path", &start)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_found_at_or_above_the_folder() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        ryuplm::server::init(&root, None).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        assert_eq!(find_root(&root.join("a/b")).unwrap(), root);
        let other = tempfile::tempdir().unwrap();
        assert_eq!(
            find_root(other.path()).unwrap_err().code,
            ryu_doc::error::Code::ProjectNotFound
        );
    }
}
