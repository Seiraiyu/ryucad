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

/// The feature format that holds `mode` (design 364). ryucad 16.A takes
/// 11 and 18.A takes 12; a stage taking 13 first moves this one on.
pub const MODE_FORMAT: u64 = 13;

/// The feature format that holds `place` (ryucad 21.D, `import
/// --flatten`): 15 and 16 are ryucad 18.B's and 17.A's, in flight when
/// it was planned.
pub const PLACE_FORMAT: u64 = 17;

/// Where a flattened import's body stands in its part: the file's
/// placements of the usages from a root down to its product, composed
/// (`pose`, written as an instance's pose is), and those usages (`via`),
/// which say which occurrence of the product it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub pose: super::assembly::Pose,
    pub via: Vec<Via>,
}

/// One usage of a flattened body's walk: the
/// `NEXT_ASSEMBLY_USAGE_OCCURRENCE`'s id, the child product it places and
/// its place among its parent's usages of that child with that id. The
/// child is named because writers give usages ids that aren't unique (KiCad
/// and ryustep number them per parent, some files give every one `1`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Via {
    pub usage: String,
    pub product: ProductRef,
    pub ordinal: u32,
}

impl Place {
    pub fn to_json(&self) -> Value {
        let via: Vec<Value> = self
            .via
            .iter()
            .map(|u| {
                let mut v = json!({"usage": u.usage, "product": u.product.to_json()});
                if u.ordinal != 0 {
                    v["ordinal"] = json!(u.ordinal);
                }
                v
            })
            .collect();
        json!({"pose": self.pose.to_json(), "via": via})
    }

    pub fn from_json(v: &Value, at: &str) -> Result<Place> {
        let mut o = Obj::new(v, at)?;
        let pose = super::assembly::Pose::from_json(&o.take("pose")?, &format!("{at}/pose"))?;
        let list = o.take("via")?;
        let items = list.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
            invalid(
                &format!("{at}/via"),
                "place.via must list the usages from a root down",
            )
        })?;
        let mut via = Vec::new();
        for (k, u) in items.iter().enumerate() {
            let here = format!("{at}/via/{k}");
            let mut uo = Obj::new(u, &here)?;
            let usage = uo
                .take("usage")?
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| {
                    invalid(&format!("{here}/usage"), "a usage's id must be a string")
                })?;
            let product = ProductRef::from_json(&uo.take("product")?, &format!("{here}/product"))?;
            let ordinal = match uo.take_opt("ordinal") {
                None => 0,
                Some(n) => count(&n, &format!("{here}/ordinal"))?,
            };
            uo.finish()?;
            via.push(Via {
                usage,
                product,
                ordinal,
            });
        }
        o.finish()?;
        Ok(Place { pose, via })
    }
}

/// How an import builds its body: a reference (the file's topology, never
/// healed, never changed) or a healed solid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Reference,
    Solid,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Reference => "reference",
            Mode::Solid => "solid",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "reference" => Some(Mode::Reference),
            "solid" => Some(Mode::Solid),
            _ => None,
        }
    }
}

/// An import feature: which body of which file.
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub file: FileRef,
    /// The product definition whose body it is; `None` for a body no
    /// product reaches.
    pub product: Option<ProductRef>,
    /// Which of the product's bodies (or of the bodies no product
    /// reaches), from 0 in instance order.
    pub body: u32,
    /// `Some(Reference)` for a reference; `None` (no field, as every file
    /// before design 364) is a healed solid, written with no field.
    pub mode: Option<Mode>,
    /// Where a flattened import's body stands (`import --flatten`); none
    /// for a body at its part's origin.
    pub place: Option<Place>,
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

impl ProductRef {
    /// `{id, name}`, with `ordinal` when it isn't 0.
    pub fn to_json(&self) -> Value {
        let mut v = json!({"id": self.id, "name": self.name});
        if self.ordinal != 0 {
            v["ordinal"] = json!(self.ordinal);
        }
        v
    }

    /// `{id, name, ordinal?}`; `at` is its JSON pointer.
    pub fn from_json(p: &Value, at: &str) -> Result<ProductRef> {
        let mut po = Obj::new(p, at)?;
        let text = |po: &mut Obj, key: &str| -> Result<String> {
            po.take(key)?.as_str().map(str::to_owned).ok_or_else(|| {
                invalid(
                    &format!("{at}/{key}"),
                    &format!("product.{key} must be a string"),
                )
            })
        };
        let id = text(&mut po, "id")?;
        let name = text(&mut po, "name")?;
        let ordinal = match po.take_opt("ordinal") {
            None => 0,
            Some(n) => count(&n, &format!("{at}/ordinal"))?,
        };
        po.finish()?;
        Ok(ProductRef { id, name, ordinal })
    }
}

/// An imported file's path as a feature names it, checked; `at` is its
/// JSON pointer.
pub fn import_path(v: &Value, at: &str) -> Result<String> {
    v.as_str()
        .filter(|p| is_import_path(p))
        .map(str::to_owned)
        .ok_or_else(|| {
            invalid(
                at,
                "file.path must be imports/<id>.step, the ID 1 to 63 of a-z, 0-9, _ and -",
            )
        })
}

impl Import {
    /// Its mode: a solid unless it says reference.
    pub fn mode(&self) -> Mode {
        self.mode.unwrap_or(Mode::Solid)
    }

    /// The format it needs: [`PLACE_FORMAT`] for a placed one,
    /// [`MODE_FORMAT`] for a reference, else 5.
    pub fn format(&self) -> u64 {
        if self.place.is_some() {
            PLACE_FORMAT
        } else if self.mode() == Mode::Reference {
            MODE_FORMAT
        } else {
            5
        }
    }

    /// Its fields in a feature file, besides the header.
    pub fn fields(&self) -> Value {
        let mut v = json!({
            "body": self.body,
            "file": {"blake3": self.file.blake3, "path": self.file.path},
        });
        if let Some(p) = &self.product {
            v["product"] = p.to_json();
        }
        if self.mode() == Mode::Reference {
            v["mode"] = json!("reference");
        }
        if let Some(p) = &self.place {
            v["place"] = p.to_json();
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj) -> Result<Import> {
        let file = o.take("file")?;
        let mut f = Obj::new(&file, "/file")?;
        let path = import_path(&f.take("path")?, "/file/path")?;
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
            Some(p) => Some(ProductRef::from_json(&p, "/product")?),
        };
        let body = count(&o.take("body")?, "/body")?;
        let mode = match o.take_opt("mode") {
            None => None,
            Some(m) => match m.as_str().and_then(Mode::parse) {
                // `solid` reads as no field: a solid is written with none.
                Some(Mode::Solid) => None,
                Some(Mode::Reference) => Some(Mode::Reference),
                None => return Err(invalid("/mode", "mode must be reference or solid")),
            },
        };
        let place = match o.take_opt("place") {
            None => None,
            Some(p) => Some(Place::from_json(&p, "/place")?),
        };
        Ok(Import {
            file: FileRef { path, blake3 },
            product,
            body,
            mode,
            place,
        })
    }
}

/// A whole number from 0 to 2³² − 1.
pub(crate) fn count(v: &Value, at: &str) -> Result<u32> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| invalid(at, "must be a whole number from 0"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{FEATURE_FORMAT, Feature};
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
    fn mode_reads_writes_and_needs_its_format() {
        // No mode: a solid, format 5, byte for byte as before.
        let f = read(file(json!({}))).unwrap();
        let Feature::Import(i) = &f else {
            unreachable!()
        };
        assert_eq!(i.mode, None);
        assert_eq!(i.mode(), Mode::Solid);
        assert_eq!(f.format(), 5);
        assert!(f.to_json().get("mode").is_none());
        // A reference: format 13, written back as read.
        let v = file(json!({"mode": "reference", "format": MODE_FORMAT}));
        let f = read(v.clone()).unwrap();
        let Feature::Import(i) = &f else {
            unreachable!()
        };
        assert_eq!(i.mode(), Mode::Reference);
        assert_eq!(f.format(), MODE_FORMAT);
        assert_eq!(f.to_json(), v);
        assert_eq!(crate::doc::files::feature_format(&v), MODE_FORMAT);
        // `solid` spelled out reads as a solid and writes as none.
        let f = read(file(json!({"mode": "solid"}))).unwrap();
        assert!(f.to_json().get("mode").is_none());
        assert_eq!(f.format(), 5);
        // Anything else is refused at its pointer.
        for bad in [json!("mesh"), json!(1), json!(null)] {
            let e = read(file(json!({"mode": bad}))).unwrap_err();
            assert_eq!(e.code, Code::InvalidDocument);
            assert_eq!(e.details["pointer"], "/mode");
        }
        const { assert!(FEATURE_FORMAT >= MODE_FORMAT) };
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
        // format is the newest a reader needs), but one claiming a format
        // past the newest is too new.
        let e = read(file(json!({"format": FEATURE_FORMAT + 1}))).unwrap_err();
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

    #[test]
    fn place_reads_writes_and_needs_its_format() {
        let place = json!({
            "pose": {"origin_m": [0.01, 0, 0.002], "x": [1, 0, 0], "z": [0, 0, 1]},
            "via": [
                {"product": {"id": "S", "name": "Sub"}, "usage": "U1"},
                {"ordinal": 2, "product": {"id": "R", "name": "R", "ordinal": 1}, "usage": "R7"},
            ],
        });
        let v = file(json!({"place": place, "mode": "reference", "format": PLACE_FORMAT}));
        let f = read(v.clone()).unwrap();
        let Feature::Import(i) = &f else {
            unreachable!()
        };
        let p = i.place.as_ref().unwrap();
        assert_eq!(
            p.via,
            vec![
                Via {
                    usage: "U1".into(),
                    product: ProductRef {
                        id: "S".into(),
                        name: "Sub".into(),
                        ordinal: 0
                    },
                    ordinal: 0
                },
                Via {
                    usage: "R7".into(),
                    product: ProductRef {
                        id: "R".into(),
                        name: "R".into(),
                        ordinal: 1
                    },
                    ordinal: 2
                },
            ]
        );
        assert_eq!(f.format(), PLACE_FORMAT);
        assert_eq!(f.to_json(), v);
        assert_eq!(crate::doc::files::feature_format(&v), PLACE_FORMAT);
        // A healed solid placed takes the same format.
        let v = file(json!({"place": place, "format": PLACE_FORMAT}));
        assert_eq!(read(v.clone()).unwrap().format(), PLACE_FORMAT);
        for (bad, at) in [
            (json!({"pose": place["pose"]}), "/place/via"),
            (json!({"pose": place["pose"], "via": []}), "/place/via"),
            (
                json!({"pose": place["pose"], "via": [{"usage": 3}]}),
                "/place/via/0/usage",
            ),
            (
                json!({"pose": place["pose"], "via": [{"usage": "U1"}]}),
                "/place/via/0/product",
            ),
            (json!({"via": place["via"]}), "/place/pose"),
        ] {
            let e = read(file(json!({"place": bad}))).unwrap_err();
            assert_eq!(e.details["pointer"], at, "{bad}");
        }
        const { assert!(FEATURE_FORMAT >= PLACE_FORMAT) };
    }
}
