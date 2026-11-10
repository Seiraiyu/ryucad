//! Release packages (design 436): what a supplier, a shop or an auditor
//! is sent of one official release. A package is a zip ([`zip`]) holding
//! `package.json` (a [`Manifest`], kind `ryupdm.package`), `record.json`
//! (the record's bytes at its official commit) and the files the
//! project's `packages.formats` name, built by `ryucad release package`.
//! Its key ([`key`]) finds a build of the same inputs; its hash
//! ([`Manifest::hash`]) is what a package attestation ([`attest`]) signs.
//! [`verify`] judges a package someone was sent.

pub mod zip;

use ryucad_doc::doc::canonical;
use ryucad_doc::doc::files::{Obj, header, invalid};
use ryucad_doc::doc::id::{Id, ProjectId};
use ryucad_doc::error::{Code, Error, Result};
use serde_json::{Value, json};

pub const KIND: &str = "ryupdm.package";
pub const FORMAT: u64 = 1;
/// The manifest's path in the zip: always its first entry.
pub const MANIFEST: &str = "package.json";
/// The record's path in the zip.
pub const RECORD: &str = "record.json";
/// What the input key hashes first.
pub const KEY_DOMAIN: &str = "ryupdm.package.1";
/// The largest manifest read.
pub const MAX_MANIFEST: u64 = 1 << 20;
/// The largest file a package holds.
pub const MAX_FILE: u64 = 2 << 30;

/// A file kind a package may hold (`packages.formats` in `ryupdm.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Format {
    Bom,
    Dxf,
    Pdf,
    Render,
    Step,
}

impl Format {
    pub const ALL: [Format; 5] = [
        Format::Bom,
        Format::Dxf,
        Format::Pdf,
        Format::Render,
        Format::Step,
    ];
    /// What a project that names none packages: every format but `dxf`.
    pub const DEFAULT: [Format; 4] = [Format::Bom, Format::Pdf, Format::Render, Format::Step];

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Bom => "bom",
            Format::Dxf => "dxf",
            Format::Pdf => "pdf",
            Format::Render => "render",
            Format::Step => "step",
        }
    }

    pub fn parse(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.as_str() == s)
    }
}

/// Reads a list of format names, at `at` (a JSON pointer): sorted and
/// each once; an unknown name is `packages_format_unknown`.
pub fn formats_from(v: &Value, at: &str) -> Result<Vec<Format>> {
    let list = v
        .as_array()
        .ok_or_else(|| invalid(at, "formats must be an array of names"))?;
    let mut out = Vec::new();
    for (i, f) in list.iter().enumerate() {
        let name = f.as_str().unwrap_or("");
        let f = Format::parse(name).ok_or_else(|| {
            Error::new(
                Code::PackagesFormatUnknown,
                format!("{at}/{i}: {name:?} isn't a package format (bom, dxf, pdf, render, step)"),
            )
            .with("pointer", format!("{at}/{i}"))
            .with("format", name)
        })?;
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out.sort();
    Ok(out)
}

/// The ryucad that built a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltWith {
    /// Its version (`0.10.0`).
    pub ryucad: String,
    /// The commit it was built from, as `ryucad version` gives it (12 hex
    /// in a release), `unknown` for a build from a source archive.
    pub commit: String,
}

/// A part an assembly's package places directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub part: Id,
    /// The label it is pinned at; `None` for a part placed in work.
    pub label: Option<String>,
    /// The project it comes from: this one's, or a dependency's.
    pub project: ProjectId,
    /// That release's package key, when the builder knows it. A
    /// reference, left out of the package's hash.
    pub package: Option<String>,
}

/// One file of a package, beside the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub path: String,
    /// BLAKE3, 64 hex.
    pub blake3: String,
    pub size: u64,
    pub media_type: String,
}

impl File {
    /// The entry for `data` at `path`.
    pub fn of(path: &str, data: &[u8]) -> File {
        File {
            path: path.to_owned(),
            blake3: blake3::hash(data).to_hex().to_string(),
            size: data.len() as u64,
            media_type: media_type(path).to_owned(),
        }
    }
}

/// A file's media type, by its extension.
pub fn media_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, e)| e) {
        Some("step") => "model/step",
        Some("pdf") => "application/pdf",
        Some("dxf") => "image/vnd.dxf",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}

/// Whether `path` is one a package may hold: ASCII letters, digits, `-`,
/// `_`, `.` and `/`, no empty, `.` or `..` part, not absolute.
pub fn path_ok(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 255
        && path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_./".contains(&c))
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
}

/// `ryupdm.package`, format 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub built_with: BuiltWith,
    /// Assemblies only, by part ID.
    pub children: Vec<Child>,
    /// The record's official commit.
    pub commit: String,
    /// The record's content hash.
    pub content: String,
    /// In path order.
    pub files: Vec<File>,
    pub label: String,
    /// The part's PDM number, else its ID.
    pub number: String,
    /// What it was built with: `formats`, and the fixed `render` and `step`.
    pub formats: Vec<Format>,
    pub part: Id,
    pub project: ProjectId,
    /// BLAKE3 of `record.json`.
    pub record: String,
    /// Warnings the build gave (uncertain views), as their JSON.
    pub warnings: Vec<Value>,
}

/// The fixed parameters every package names beside its formats.
pub const RENDER: &str = "iso 1600x1200";
pub const STEP: &str = "ap242 mm";

/// A `built_with` commit: 7 to 40 lower-case hex digits, or `unknown`.
pub fn commit_ok(s: &str) -> bool {
    s == "unknown"
        || ((7..=40).contains(&s.len())
            && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')))
}

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn text(o: &mut Obj, key: &str, at: &str, ok: impl Fn(&str) -> bool, what: &str) -> Result<String> {
    o.take(key)?
        .as_str()
        .filter(|s| ok(s))
        .map(str::to_owned)
        .ok_or_else(|| invalid(&format!("{at}/{key}"), &format!("{key} must be {what}")))
}

fn label_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && s.bytes().all(|c| c.is_ascii_alphanumeric())
}

impl Manifest {
    /// The parameters object, as the key and the manifest hash them.
    pub fn parameters(&self) -> Value {
        json!({
            "formats": self.formats.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
            "render": RENDER,
            "step": STEP,
        })
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "built_with": {"ryucad": self.built_with.ryucad, "commit": self.built_with.commit},
            "children": self.children.iter().map(|c| json!({
                "part": c.part, "label": c.label, "project": c.project, "package": c.package,
            })).collect::<Vec<_>>(),
            "commit": self.commit,
            "content": self.content,
            "files": self.files.iter().map(|f| json!({
                "path": f.path, "blake3": f.blake3, "size": f.size, "media_type": f.media_type,
            })).collect::<Vec<_>>(),
            "format": FORMAT,
            "kind": KIND,
            "label": self.label,
            "number": self.number,
            "parameters": self.parameters(),
            "part": self.part,
            "project": self.project,
            "record": self.record,
        });
        if !self.warnings.is_empty() {
            v["warnings"] = json!(self.warnings);
        }
        v
    }

    /// The canonical bytes of `package.json`.
    pub fn canonical(&self) -> String {
        canonical::to_string(&self.to_json())
    }

    /// The package hash: BLAKE3 of the canonical manifest with each
    /// child's `package` left out (a reference, which a later build may
    /// fill in). What a package attestation signs.
    pub fn hash(&self) -> String {
        let mut v = self.to_json();
        for c in v["children"].as_array_mut().into_iter().flatten() {
            c.as_object_mut().expect("an object").remove("package");
        }
        blake3::hash(canonical::to_string(&v).as_bytes())
            .to_hex()
            .to_string()
    }

    /// The input key of this package: [`key`] of its inputs.
    pub fn key(&self) -> String {
        key(
            &self.content,
            &self.record,
            &self.built_with,
            &self.parameters(),
        )
    }

    /// The name a package of this release is given: `<number>-<label>.zip`.
    pub fn file_name(&self) -> String {
        format!("{}-{}.zip", self.number, self.label)
    }

    pub fn from_json(v: &Value) -> Result<Manifest> {
        let mut o = header(v, KIND, FORMAT)?;
        let bv = o.take("built_with")?;
        let mut b = Obj::new(&bv, "/built_with")?;
        let built_with = BuiltWith {
            ryucad: text(
                &mut b,
                "ryucad",
                "/built_with",
                |s| !s.is_empty(),
                "a version",
            )?,
            commit: text(
                &mut b,
                "commit",
                "/built_with",
                commit_ok,
                "a commit's hex, or unknown",
            )?,
        };
        b.finish()?;
        let children = o
            .take("children")?
            .as_array()
            .ok_or_else(|| invalid("/children", "children must be an array"))?
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let at = format!("/children/{i}");
                let mut c = Obj::new(c, &at)?;
                let part = Id::parse(c.take("part")?.as_str().unwrap_or(""), "part")
                    .map_err(|_| invalid(&format!("{at}/part"), "part must be a part ID"))?;
                let label = match c.take("label")? {
                    Value::Null => None,
                    Value::String(s) if label_ok(&s) => Some(s),
                    _ => {
                        return Err(invalid(
                            &format!("{at}/label"),
                            "label must be a label or null",
                        ));
                    }
                };
                let project =
                    ProjectId::parse(c.take("project")?.as_str().unwrap_or("")).map_err(|_| {
                        invalid(&format!("{at}/project"), "project must be a project ID")
                    })?;
                let package = match c.take("package")? {
                    Value::Null => None,
                    Value::String(s) if is_hex(&s, 64) => Some(s),
                    _ => {
                        return Err(invalid(
                            &format!("{at}/package"),
                            "package must be 64 hex or null",
                        ));
                    }
                };
                c.finish()?;
                Ok(Child {
                    part,
                    label,
                    project,
                    package,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let commit = text(&mut o, "commit", "", |s| is_hex(s, 40), "40 hex")?;
        let content = text(&mut o, "content", "", |s| is_hex(s, 64), "64 hex")?;
        let files = o
            .take("files")?
            .as_array()
            .ok_or_else(|| invalid("/files", "files must be an array"))?
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let at = format!("/files/{i}");
                let mut f = Obj::new(f, &at)?;
                let out = File {
                    path: text(&mut f, "path", &at, path_ok, "a package path")?,
                    blake3: text(&mut f, "blake3", &at, |s| is_hex(s, 64), "64 hex")?,
                    size: f
                        .take("size")?
                        .as_u64()
                        .filter(|n| *n <= MAX_FILE)
                        .ok_or_else(|| {
                            invalid(
                                &format!("{at}/size"),
                                "size must be a whole number of bytes",
                            )
                        })?,
                    media_type: text(&mut f, "media_type", &at, |s| !s.is_empty(), "a media type")?,
                };
                f.finish()?;
                Ok(out)
            })
            .collect::<Result<Vec<_>>>()?;
        if files.windows(2).any(|w| w[0].path >= w[1].path) {
            return Err(invalid("/files", "files must be in path order, each once"));
        }
        let label = text(&mut o, "label", "", label_ok, "a label")?;
        let number = text(&mut o, "number", "", |s| !s.is_empty(), "a part number")?;
        let pv = o.take("parameters")?;
        let mut p = Obj::new(&pv, "/parameters")?;
        let formats = formats_from(&p.take("formats")?, "/parameters/formats")?;
        if p.take("render")? != json!(RENDER) || p.take("step")? != json!(STEP) {
            return Err(invalid(
                "/parameters",
                "render and step must be this build's",
            ));
        }
        p.finish()?;
        let part = Id::parse(o.take("part")?.as_str().unwrap_or(""), "part")
            .map_err(|_| invalid("/part", "part must be a part ID"))?;
        let project = ProjectId::parse(o.take("project")?.as_str().unwrap_or(""))
            .map_err(|_| invalid("/project", "project must be a project ID"))?;
        let record = text(&mut o, "record", "", |s| is_hex(s, 64), "64 hex")?;
        let warnings = match o.take_opt("warnings") {
            None => Vec::new(),
            Some(Value::Array(w)) if !w.is_empty() => w,
            Some(_) => return Err(invalid("/warnings", "warnings must be a non-empty array")),
        };
        o.finish()?;
        Ok(Manifest {
            built_with,
            children,
            commit,
            content,
            files,
            label,
            number,
            formats,
            part,
            project,
            record,
            warnings,
        })
    }
}

/// The input key of a package (design 436): BLAKE3 over length-prefixed
/// pieces, as the content hash takes them: `ryupdm.package.1`, the
/// record's content hash and bytes' hash, the ryucad version and commit,
/// and the parameters' canonical JSON. Equal inputs, equal key.
pub fn key(content: &str, record: &str, built: &BuiltWith, parameters: &Value) -> String {
    let mut h = blake3::Hasher::new();
    let params = canonical::to_string(parameters);
    for piece in [
        KEY_DOMAIN,
        content,
        record,
        &built.ryucad,
        &built.commit,
        &params,
    ] {
        h.update(&(piece.len() as u64).to_le_bytes());
        h.update(piece.as_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// A package's zip: the manifest first, then `files` (with the record)
/// in path order. `files` must be what the manifest lists.
pub fn archive(manifest: &Manifest, files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let text = manifest.canonical();
    let mut entries: Vec<(&str, &[u8])> = vec![(MANIFEST, text.as_bytes())];
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    entries.extend(sorted.iter().map(|(p, d)| (p.as_str(), d.as_slice())));
    zip::write(&entries).map_err(|zip::TooLarge(why)| {
        Error::new(
            Code::PackageFailed,
            format!("the package can't be written: {why}"),
        )
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn manifest() -> Manifest {
        let record = br#"{"kind":"ryupdm.release"}"#;
        Manifest {
            built_with: BuiltWith {
                ryucad: "0.10.0".into(),
                commit: "1".repeat(12),
            },
            children: vec![Child {
                part: Id::parse("bolt", "part").unwrap(),
                label: Some("C".into()),
                project: ProjectId::from_bytes([3; 16]),
                package: None,
            }],
            commit: "a".repeat(40),
            content: "c".repeat(64),
            files: vec![
                File::of(RECORD, record),
                File::of("step/100-B.step", b"ISO-10303-21;"),
            ],
            label: "B".into(),
            number: "100".into(),
            formats: vec![Format::Step],
            part: Id::parse("bracket", "part").unwrap(),
            project: ProjectId::from_bytes([1; 16]),
            record: blake3::hash(record).to_hex().to_string(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_manifest_round_trips_canonically() {
        let m = manifest();
        let v = canonical::parse(&m.canonical()).unwrap();
        assert_eq!(Manifest::from_json(&v).unwrap(), m);
        assert_eq!(m.file_name(), "100-B.zip");
        let mut late = v.clone();
        late["format"] = json!(2);
        assert_eq!(
            Manifest::from_json(&late).unwrap_err().code,
            Code::FormatTooNew
        );
        let mut bad = v.clone();
        bad["files"][1]["path"] = json!("../etc/passwd");
        assert_eq!(
            Manifest::from_json(&bad).unwrap_err().details["pointer"],
            json!("/files/1/path")
        );
    }

    #[test]
    fn a_childs_package_is_left_out_of_the_hash_not_the_key_inputs() {
        let m = manifest();
        let mut known = m.clone();
        known.children[0].package = Some("d".repeat(64));
        assert_eq!(m.hash(), known.hash());
        assert_ne!(m.canonical(), known.canonical());
        let mut other = m.clone();
        other.formats.push(Format::Pdf);
        assert_ne!(m.key(), other.key());
        assert_ne!(m.hash(), other.hash());
        let mut built = m.clone();
        built.built_with.ryucad = "0.10.1".into();
        assert_ne!(m.key(), built.key());
    }

    #[test]
    fn formats_read_sorted_and_unknown_ones_are_refused() {
        assert_eq!(
            formats_from(&json!(["step", "bom", "step"]), "/packages/formats").unwrap(),
            vec![Format::Bom, Format::Step]
        );
        let e = formats_from(&json!(["step", "stl"]), "/packages/formats").unwrap_err();
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::PackagesFormatUnknown, &json!("/packages/formats/1"))
        );
    }

    #[test]
    fn paths_are_plain_ascii_and_relative() {
        for ok in ["step/100-0042-B.step", "drawings/d1-s2.dxf", "record.json"] {
            assert!(path_ok(ok), "{ok}");
        }
        for bad in ["", "/abs", "a//b", "../x", "a/./b", "é.pdf", "a b"] {
            assert!(!path_ok(bad), "{bad}");
        }
    }
}
