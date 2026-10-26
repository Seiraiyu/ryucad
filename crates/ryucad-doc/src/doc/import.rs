//! The `import` feature: one body of a STEP file the project holds under
//! `imports/`, named by its path and the BLAKE3 of its bytes, founding a
//! solid of its own.
//!
//! The hash makes the feature's bytes follow the file's: a new version of
//! the file is a new feature file, so part revisions, PDM content hashes,
//! cache keys, diffs and merges all see it.

use super::files::{Obj, invalid};
use super::id::Id;
use super::project::IMPORTS;
use crate::error::Result;
use serde_json::{Value, json};

/// An import feature: which body of which file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub file: FileRef,
    /// The product definition whose body it is; `None` for a body no
    /// product reaches.
    pub product: Option<ProductRef>,
    /// Which of the product's bodies (or of the bodies no product
    /// reaches), from 0 in instance order.
    pub body: u32,
}

/// A file under `imports/` and the BLAKE3 of the bytes the feature was
/// written against.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileRef {
    /// Relative to the project root: `imports/<id>.step`.
    pub path: String,
    /// 64 lowercase hex digits.
    pub blake3: String,
}

/// A STEP product definition, by its `PRODUCT`'s id and name, and its
/// place among definitions with both equal, in instance order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProductRef {
    pub id: String,
    pub name: String,
    pub ordinal: u32,
}

/// Whether `path` is where an imported file may live: `imports/<id>.step`.
pub fn is_import_path(path: &str) -> bool {
    path.strip_prefix(IMPORTS)
        .and_then(|p| p.strip_prefix('/'))
        .and_then(|p| p.strip_suffix(".step"))
        .is_some_and(|stem| Id::parse(stem, "file").is_ok())
}

/// Whether `s` is a BLAKE3 as files write it.
fn is_blake3(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Import {
    /// Its fields in a feature file, besides the header.
    pub fn fields(&self) -> Value {
        let mut v = json!({
            "body": self.body,
            "file": {"blake3": self.file.blake3, "path": self.file.path},
        });
        if let Some(p) = &self.product {
            v["product"] = json!({"id": p.id, "name": p.name});
            if p.ordinal != 0 {
                v["product"]["ordinal"] = json!(p.ordinal);
            }
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj) -> Result<Import> {
        let file = o.take("file")?;
        let mut f = Obj::new(&file, "/file")?;
        let path = f.take("path")?;
        let path = path
            .as_str()
            .filter(|p| is_import_path(p))
            .ok_or_else(|| {
                invalid(
                    "/file/path",
                    "file.path must be imports/<id>.step, the ID 1 to 63 of a-z, 0-9, _ and -",
                )
            })?
            .to_owned();
        let blake3 = f.take("blake3")?;
        let blake3 = blake3
            .as_str()
            .filter(|h| is_blake3(h))
            .ok_or_else(|| {
                invalid(
                    "/file/blake3",
                    "file.blake3 must be 64 lowercase hex digits",
                )
            })?
            .to_owned();
        f.finish()?;
        let product = match o.take_opt("product") {
            None => None,
            Some(p) => {
                let mut po = Obj::new(&p, "/product")?;
                let text = |po: &mut Obj, key: &str| -> Result<String> {
                    po.take(key)?.as_str().map(str::to_owned).ok_or_else(|| {
                        invalid(
                            &format!("/product/{key}"),
                            &format!("product.{key} must be a string"),
                        )
                    })
                };
                let id = text(&mut po, "id")?;
                let name = text(&mut po, "name")?;
                let ordinal = match po.take_opt("ordinal") {
                    None => 0,
                    Some(n) => count(&n, "/product/ordinal")?,
                };
                po.finish()?;
                Some(ProductRef { id, name, ordinal })
            }
        };
        let body = count(&o.take("body")?, "/body")?;
        Ok(Import {
            file: FileRef { path, blake3 },
            product,
            body,
        })
    }
}

/// A whole number from 0 to 2³² − 1.
fn count(v: &Value, at: &str) -> Result<u32> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| invalid(at, "must be a whole number from 0"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::Feature;
    use crate::error::Code;
    use crate::expr::Env;
    use crate::units::Unit;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn read(v: Value) -> Result<Feature> {
        Feature::from_json(&v, Unit::Mm, &Env::empty())
    }

    fn file(extra: Value) -> Value {
        let mut v = json!({
            "body": 0,
            "file": {"blake3": HASH, "path": "imports/bracket.step"},
            "format": 5,
            "kind": "ryucad.feature",
            "type": "import",
        });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    #[test]
    fn reads_and_writes_itself() {
        for v in [
            file(json!({})),
            file(json!({"body": 2, "product": {"id": "BRK-100", "name": "Bracket"}})),
            file(json!({"product": {"id": "", "name": "", "ordinal": 3}})),
        ] {
            let f = read(v.clone()).unwrap();
            assert_eq!(f.to_json(), v);
            assert_eq!(f.type_name(), "import");
            assert_eq!(f.format(), 5);
            assert_eq!(crate::doc::files::feature_format(&v), 5);
            assert!(f.combine().unwrap().is_new() && f.targets().is_empty());
            assert!(crate::doc::files::feature_acts(&v).founds);
        }
        let f = read(file(
            json!({"product": {"id": "a", "name": "b", "ordinal": 0}}),
        ))
        .unwrap();
        assert_eq!(f.to_json()["product"], json!({"id": "a", "name": "b"}));
    }

    #[test]
    fn refuses_what_isnt_an_import() {
        for (v, at) in [
            (
                file(json!({"file": {"blake3": HASH, "path": "parts/x.step"}})),
                "/file/path",
            ),
            (
                file(json!({"file": {"blake3": HASH, "path": "imports/X.step"}})),
                "/file/path",
            ),
            (
                file(json!({"file": {"blake3": HASH, "path": "imports/a/b.step"}})),
                "/file/path",
            ),
            (
                file(json!({"file": {"blake3": HASH, "path": "imports/a.stp"}})),
                "/file/path",
            ),
            (
                file(json!({"file": {"blake3": "abc", "path": "imports/a.step"}})),
                "/file/blake3",
            ),
            (
                file(json!({"file": {"blake3": HASH.to_uppercase(), "path": "imports/a.step"}})),
                "/file/blake3",
            ),
            (file(json!({"body": -1})), "/body"),
            (file(json!({"body": 1.5})), "/body"),
            (file(json!({"product": {"id": "a"}})), "/product/name"),
            (
                file(json!({"product": {"id": 1, "name": "b"}})),
                "/product/id",
            ),
            (
                file(json!({"product": {"id": "a", "name": "b", "ordinal": -2}})),
                "/product/ordinal",
            ),
            (file(json!({"operation": "join", "targets": ["b"]})), ""),
        ] {
            let e = read(v.clone()).unwrap_err();
            assert_eq!(e.code, Code::InvalidDocument, "{v}");
            if !at.is_empty() {
                assert_eq!(e.details["pointer"], at, "{v}");
            }
        }
        // Format 4 doesn't hold it, so a file claiming 4 still reads (the
        // format is the newest a reader needs), but one claiming 6 is too
        // new.
        let e = read(file(json!({"format": 6}))).unwrap_err();
        assert_eq!(e.code, Code::FormatTooNew);
    }

    #[test]
    fn import_paths() {
        assert!(is_import_path("imports/bracket.step"));
        assert!(is_import_path("imports/a-2.step"));
        for p in [
            "imports/.step",
            "imports/con.step",
            "import/a.step",
            "imports/a.step/b",
        ] {
            assert!(!is_import_path(p), "{p}");
        }
    }
}
