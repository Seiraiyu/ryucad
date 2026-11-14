//! The `ryueda.content.1` hash of a design or a library component
//! (design 629): what a release record says was released.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::libcomp::{LibComp, LibRef};
use crate::project::{Manifest, ProjectKind};
use crate::source::Source;

/// The tag every ryueda content hash starts with.
pub const TAG: &str = "ryueda.content.1";

/// BLAKE3 over length-prefixed pieces, as design 44's hash is built.
pub(crate) struct Pieces(blake3::Hasher);

impl Pieces {
    pub(crate) fn new() -> Pieces {
        let mut p = Pieces(blake3::Hasher::new());
        p.piece(TAG.as_bytes());
        p
    }

    pub(crate) fn piece(&mut self, b: &[u8]) {
        self.0.update(&(b.len() as u64).to_le_bytes());
        self.0.update(b);
    }

    /// The file at `path` as two pieces, its path and its bytes; `false`
    /// when there is none (and nothing hashed).
    pub(crate) fn file(&mut self, src: &dyn Source, path: &str) -> Result<bool> {
        match src.read(path)? {
            Some(b) => {
                self.piece(path.as_bytes());
                self.piece(&b);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The file at `path`, which must be there.
    pub(crate) fn needed(&mut self, src: &dyn Source, path: &str) -> Result<()> {
        if self.file(src, path)? {
            Ok(())
        } else {
            Err(Error::new(Code::NotFound, format!("no {path}")).with("path", path))
        }
    }

    pub(crate) fn finish(self) -> String {
        self.0.finalize().to_hex().to_string()
    }
}

/// Where a library object of this project lives.
pub(crate) fn object_path(folder: &str, id: &Id) -> String {
    format!("{}/{folder}/{id}.json", crate::library::DIR)
}

/// The library component `id` of this project, read.
pub(crate) fn libcomp(src: &dyn Source, id: &Id) -> Result<LibComp> {
    let path = object_path(crate::library::COMPONENTS, id);
    let bytes = src
        .read(&path)?
        .ok_or_else(|| Error::new(Code::NotFound, format!("no {path}")).with("path", &path))?;
    crate::doc::read(&bytes, &path, crate::libcomp::KIND, crate::libcomp::FORMAT)
}

/// A reference to an object of this library, or `component_not_released`
/// for one in another project (pins are plan 10.C's).
fn local<'a>(r: &'a LibRef, what: &str) -> Result<&'a Id> {
    match &r.library {
        None => Ok(&r.id),
        Some(alias) => Err(not_released(alias, &r.id, what)),
    }
}

/// `component_not_released` for `alias/id`.
pub(crate) fn not_released(alias: &Id, id: &Id, what: &str) -> Error {
    Error::new(
        Code::ComponentNotReleased,
        format!(
            "{what} places {alias}/{id} as it is (working): a release can only pin another project's components at a release label"
        ),
    )
    .with("alias", alias)
    .with("component", id)
}

/// A library component's symbol and footprints, in that order.
pub(crate) fn objects(h: &mut Pieces, src: &dyn Source, id: &Id, c: &LibComp) -> Result<()> {
    let what = format!("component {id}");
    h.needed(
        src,
        &object_path(crate::library::SYMBOLS, local(&c.symbol, &what)?),
    )?;
    for u in &c.footprints {
        h.needed(
            src,
            &object_path(crate::library::FOOTPRINTS, local(&u.footprint, &what)?),
        )?;
    }
    Ok(())
}

/// The content hash of `part` in the project `src` holds: a design's in
/// a design project, a library component's in a library project. Its
/// pieces are the tag, the default unit, then the part's files, each as
/// its path and its bytes.
pub fn hash(src: &dyn Source, part: &Id) -> Result<String> {
    let m = crate::pdm::manifest(src)?;
    let mut h = Pieces::new();
    h.piece(
        serde_json::to_string(&m.default_unit)
            .expect("a unit")
            .as_bytes(),
    );
    match m.project_kind {
        ProjectKind::Library => {
            let c = libcomp(src, part)?;
            h.needed(src, &object_path(crate::library::COMPONENTS, part))?;
            objects(&mut h, src, part, &c)?;
        }
        ProjectKind::Design => design(&mut h, src, &m, part)?,
    }
    Ok(h.finish())
}

/// A design's pieces: Task 10.
fn design(h: &mut Pieces, src: &dyn Source, m: &Manifest, part: &Id) -> Result<()> {
    let _ = (h, src, m);
    Err(Error::new(
        Code::Internal,
        format!("no content hash for design {part} yet"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdm::tests::{board, id};

    #[test]
    fn a_components_hash_covers_it_its_symbol_and_footprints_only() {
        let l = board(ProjectKind::Library);
        let h = hash(&l, &id("r_100k")).unwrap();
        assert_eq!(h.len(), 64);
        assert_eq!(hash(&l, &id("r_100k")).unwrap(), h, "deterministic");
        let moved = |path: &str, key: &str| {
            let mut f = l.clone();
            let mut v: serde_json::Value = serde_json::from_slice(&f[path]).unwrap();
            v[key] = serde_json::json!("99k");
            f.insert(path.into(), crate::canonical::to_string(&v).into_bytes());
            hash(&f, &id("r_100k")).unwrap() != h
        };
        assert!(moved("library/components/r_100k.json", "value"));
        assert!(moved("library/symbols/r.json", "zz_test"));
        assert!(moved(
            "library/footprints/r_0402_1005metric.json",
            "zz_test"
        ));
        // Standards, style and sourcing judge or draw it; they don't change it.
        let mut f = l.clone();
        f.insert("style.json".into(), b"{}\n".to_vec());
        f.insert("parts.json".into(), b"{}\n".to_vec());
        f.insert("library/standards.json".into(), b"{}\n".to_vec());
        assert_eq!(hash(&f, &id("r_100k")).unwrap(), h);
        // The unit is a piece.
        let mut inch = l.clone();
        let m = Manifest::new(
            crate::project::tests::pid(1),
            crate::units::Unit::In,
            ProjectKind::Library,
        );
        inch.insert("ryueda.json".into(), m.to_text().into_bytes());
        assert_ne!(hash(&inch, &id("r_100k")).unwrap(), h);
        assert_eq!(hash(&l, &id("nope")).unwrap_err().code, Code::NotFound);
    }
}
