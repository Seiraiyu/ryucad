//! The sample project Open sample… writes (design 199, Start screen): a
//! bracket with parameters, a sketch, an extrude, holes and fillets, a
//! pin, and an assembly placing the bracket with the pin in a hole by a
//! cylindrical mate. Its files are `crates/ryucad-app/sample/project/`,
//! made through the CLI as an agent makes them (`sample/make.sh`), and
//! embedded here; `ryucad init` writes the manifest where it is opened.

use ryucad::error::{Error, Result};
use std::path::Path;

macro_rules! files {
    ($($path:literal,)*) => {
        /// Every file of the sample, by its path in the project.
        pub const FILES: &[(&str, &[u8])] = &[$(
            ($path, include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/sample/project/", $path))),
        )*];
    };
}

files! {
    "parts/bracket/features/e1.json",
    "parts/bracket/features/f1.json",
    "parts/bracket/features/h1.json",
    "parts/bracket/features/s1.json",
    "parts/bracket/features/s2.json",
    "parts/bracket/parameters.json",
    "parts/bracket/part.json",
    "parts/pin/features/shank.json",
    "parts/pin/parameters.json",
    "parts/pin/part.json",
    "parts/rig/features/m1.json",
    "parts/rig/features/pin.json",
    "parts/rig/features/plate.json",
    "parts/rig/part.json",
}

/// Writes the sample's files into `dir`.
pub fn write(dir: &Path) -> Result<()> {
    for (path, bytes) in FILES {
        let to = dir.join(path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, &e))?;
        }
        std::fs::write(&to, bytes).map_err(|e| Error::io(&to, &e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded list is the folder, file for file.
    #[test]
    fn the_embedded_sample_is_the_folder() {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else {
                    let rel = p.strip_prefix(root).unwrap();
                    let parts: Vec<_> = rel
                        .iter()
                        .map(|c| c.to_string_lossy().into_owned())
                        .collect();
                    out.push(parts.join("/"));
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample/project");
        let mut on_disk = Vec::new();
        walk(&root, &root, &mut on_disk);
        on_disk.sort();
        let mut embedded: Vec<String> = FILES.iter().map(|(p, _)| (*p).to_owned()).collect();
        embedded.sort();
        assert_eq!(embedded, on_disk);
        // No manifest: `ryucad init` writes it.
        assert!(
            !embedded
                .iter()
                .any(|p| p == "ryucad.json" || p == ".gitattributes")
        );
    }

    #[test]
    fn the_sample_is_written_file_for_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path()).unwrap();
        for (path, bytes) in FILES {
            assert_eq!(
                &std::fs::read(dir.path().join(path)).unwrap()[..],
                *bytes,
                "{path}"
            );
        }
    }
}
