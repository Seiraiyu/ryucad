//! `release.package` and `release.package.verify` (design 436): an
//! official release's package built from its official commit alone, and a
//! package someone was sent judged offline. What a package holds and how
//! it is checked are `ryupdm::package`'s; this builds its files through
//! the methods agents call (`export`, `drawing.export`, `bom.get`,
//! `render`), so a package and a clone's rebuild are the same bytes.

use crate::doc::assembly::Rev;
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::proto::Answer;
use crate::session::Session;
use ryupdm::config::Config;
use ryupdm::package::{self, BuiltWith, Child, File, Format, Manifest, RECORD};
use ryupdm::record::{Kind, Record, Records};
use serde_json::{Value, json};

/// The answers' shape.
pub const SCHEMA: u64 = 1;

/// This build, as a package names it.
pub fn this_build() -> BuiltWith {
    BuiltWith {
        ryucad: env!("CARGO_PKG_VERSION").to_owned(),
        commit: env!("RYUCAD_COMMIT").to_owned(),
    }
}

/// What `release.package` was asked for.
#[derive(Clone, Debug, Default)]
pub struct Ask {
    pub part: String,
    /// The label; the part's latest official release if left out.
    pub label: Option<String>,
    /// A `.zip` to write, or a folder (an existing one, or a path ending
    /// in `/`) to write the package's files into.
    pub out: Option<String>,
    /// A folder to write `<number>-<label>.zip` into.
    pub into: Option<String>,
    /// The formats, instead of the project's `packages.formats`.
    pub formats: Option<Vec<String>>,
}

/// A package built in memory.
#[derive(Debug)]
pub struct Built {
    pub manifest: Manifest,
    /// Its files, the record among them, in path order.
    pub files: Vec<(String, Vec<u8>)>,
}

impl Built {
    pub fn zip(&self) -> Result<Vec<u8>> {
        package::archive(&self.manifest, &self.files)
    }
}

/// The record of `part`'s release `label` (else its latest official
/// one), official in this clone: its label, official commit and bytes.
/// `label_not_found`, `label_not_official` for a proposal, and
/// `release_obsolete` for an obsolete release.
fn official(
    session: &mut Session,
    part: &Id,
    label: Option<&str>,
) -> Result<(String, String, Vec<u8>)> {
    let (repo, releases) = crate::release::history(session)?;
    let label = match label {
        Some(l) => l.to_owned(),
        None => match ryupdm::state::derive(&session.files(), part, &releases).status {
            ryupdm::state::Status::Released(l) | ryupdm::state::Status::Modified(l) => l,
            ryupdm::state::Status::Obsolete(l) => l,
            _ => {
                return Err(Error::new(
                    Code::LabelNotFound,
                    format!("{part} has no official release to package"),
                )
                .with("part", part));
            }
        },
    };
    let obsolete = format!("releases/{part}/{label}.obsolete.json");
    if releases.official.contains_key(&obsolete) {
        return Err(Error::new(
            Code::ReleaseObsolete,
            format!("{part} {label} is obsolete: obsolete releases have no package"),
        )
        .with("part", part)
        .with("label", &label));
    }
    let path = format!("releases/{part}/{label}.json");
    let Some(o) = releases.official.get(&path) else {
        let records = Records::read(&*session)?;
        let known = records
            .of(part)
            .any(|e| e.label == label && e.kind == Kind::Release);
        return Err(if known {
            Error::new(
                Code::LabelNotOfficial,
                format!(
                    "{part} {label} is proposed, not official: only official releases are packaged"
                ),
            )
        } else {
            Error::new(
                Code::LabelNotFound,
                format!("{part} has no release {label}"),
            )
        }
        .with("part", part)
        .with("label", &label));
    };
    let bytes = repo
        .gix()
        .find_object(o.blob)
        .map_err(|e| Error::new(Code::Internal, format!("reading {path}: {e}")))?
        .data
        .clone();
    Ok((label, o.commit.clone(), bytes))
}

/// A part number as a file name: what isn't a letter, digit, `-`, `_`
/// or `.` becomes `_`.
fn file_safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Keeps a file's bytes, or its failure; a drawing's error findings fail
/// it, its uncertain views are warnings.
fn keep(
    file: String,
    r: Result<Answer>,
    files: &mut Vec<(String, Vec<u8>)>,
    failures: &mut Vec<Value>,
    warnings: &mut Vec<Value>,
) {
    match r {
        Ok(a) => {
            for f in a.result["findings"].as_array().into_iter().flatten() {
                match f["severity"].as_str() {
                    Some("error") => failures.push(json!({"file": file, "code": f["code"],
                        "message": f["message"], "details": f})),
                    _ if f["code"] == "drawing_view_uncertain" => {
                        warnings.push(json!({"file": file, "finding": f}));
                    }
                    _ => {}
                }
            }
            files.push((file, a.binary.unwrap_or_default()));
        }
        Err(e) => failures.push(failure(&file, &e)),
    }
}

/// One file that failed, as `package_failed` lists it.
fn failure(file: &str, e: &Error) -> Value {
    json!({"file": file, "code": e.code, "message": e.message, "details": e.details})
}

/// Builds the package of `part`'s release `label` (its latest if `None`)
/// in memory, `built` naming the ryucad that builds it.
pub fn build(
    session: &mut Session,
    part: &Id,
    label: Option<&str>,
    formats: Option<&[Format]>,
    built: &BuiltWith,
) -> Result<Built> {
    let (label, commit, record_bytes) = official(session, part, label)?;
    let record = std::str::from_utf8(&record_bytes)
        .ok()
        .and_then(|t| crate::doc::canonical::parse(t).ok())
        .and_then(|v| Record::from_json(&v, Kind::Release).ok())
        .ok_or_else(|| {
            Error::new(
                Code::InvalidDocument,
                format!("{part} {label}'s record doesn't read"),
            )
        })?;
    let content = record
        .release()
        .map(|r| r.content.clone())
        .expect("a release record");
    let project = record.project().clone();
    // What the official commit says: the formats, the number, the drawings
    // and, for an assembly, what it places.
    let (formats, number, kind, drawings, children) = {
        let (_, s) = session.at(Some(&commit))?;
        let formats = match formats {
            Some(f) => f.to_vec(),
            None => Config::read(&*s)?.package_formats(),
        };
        let number = ryupdm::props::read(&*s, part)
            .ok()
            .and_then(|p| {
                p.values
                    .get("number")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| part.to_string());
        let p = s.project().part(part)?.clone();
        let kind = p.file.as_ref().map_or(PartKind::Part, |f| f.kind);
        let drawings: Vec<(Id, Vec<String>)> = p
            .drawings
            .iter()
            .map(|(id, f)| {
                let sheets = f
                    .as_ref()
                    .map(|f| f.sheets.keys().map(|k| k.to_string()).collect())
                    .unwrap_or_default();
                (id.clone(), sheets)
            })
            .collect();
        let mut children: Vec<Child> = p
            .order()
            .iter()
            .filter_map(|id| match p.features.get(id) {
                Some(Ok(Feature::Instance(i))) => Some(match &i.rev {
                    Rev::Working => Child {
                        part: i.part.clone(),
                        label: None,
                        project: project.clone(),
                        package: None,
                    },
                    Rev::Label(l) => Child {
                        part: i.part.clone(),
                        label: Some(l.clone()),
                        project: project.clone(),
                        package: None,
                    },
                    Rev::Foreign {
                        project: f,
                        label: l,
                    } => Child {
                        part: i.part.clone(),
                        label: Some(l.clone()),
                        project: f.clone(),
                        package: None,
                    },
                }),
                _ => None,
            })
            .collect();
        children.sort_by(|a, b| {
            (a.project.as_str(), a.part.as_str(), &a.label).cmp(&(
                b.project.as_str(),
                b.part.as_str(),
                &b.label,
            ))
        });
        children.dedup();
        (formats, number, kind, drawings, children)
    };
    let rev = format!("label:{label}");
    let stem = format!("{}-{label}", file_safe(&number));
    let mut files: Vec<(String, Vec<u8>)> = vec![(RECORD.to_owned(), record_bytes)];
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    let job = session.job().clone();
    let stage = job.begin(ryu_session::job::Phase::Package, "files", formats.len());
    for f in &formats {
        stage.step(|| f.as_str().to_owned())?;
        match f {
            Format::Step => {
                let r = crate::export::export(
                    session,
                    crate::export::Ask {
                        part: part.to_string(),
                        rev: Some(rev.clone()),
                        format: Some("step".into()),
                        schema: Some("ap242".into()),
                        unit: Some("mm".into()),
                        ..Default::default()
                    },
                );
                keep(
                    format!("step/{stem}.step"),
                    r,
                    &mut files,
                    &mut failures,
                    &mut warnings,
                );
            }
            Format::Pdf | Format::Dxf => {
                for (d, sheets) in &drawings {
                    let (_, s) = session.at(Some(&commit))?;
                    if *f == Format::Pdf {
                        let r = crate::drawing::request::export(
                            s,
                            part.as_str(),
                            d.as_str(),
                            None,
                            Some("pdf"),
                            None,
                            None,
                        );
                        keep(
                            format!("drawings/{d}.pdf"),
                            r,
                            &mut files,
                            &mut failures,
                            &mut warnings,
                        );
                    } else {
                        for sh in sheets {
                            let (_, s) = session.at(Some(&commit))?;
                            let r = crate::drawing::request::export(
                                s,
                                part.as_str(),
                                d.as_str(),
                                Some(sh),
                                Some("dxf"),
                                None,
                                None,
                            );
                            keep(
                                format!("drawings/{d}-{sh}.dxf"),
                                r,
                                &mut files,
                                &mut failures,
                                &mut warnings,
                            );
                        }
                    }
                }
            }
            Format::Bom if kind == PartKind::Assembly => {
                let mut json_answers = serde_json::Map::new();
                for flat in [false, true] {
                    let name = if flat { "flat" } else { "indented" };
                    let csv = crate::bom::get(
                        session,
                        &crate::bom::Ask {
                            part: Some(part.as_str()),
                            rev: Some(&rev),
                            flat,
                            depth: None,
                            format: Some("csv"),
                        },
                    );
                    match csv {
                        Ok(v) => {
                            let text = v["csv"].as_str().unwrap_or("").as_bytes().to_vec();
                            files.push((format!("bom/{name}.csv"), text));
                            let mut v = v;
                            v.as_object_mut().expect("an object").remove("csv");
                            json_answers.insert(name.to_owned(), v);
                        }
                        Err(e) => failures.push(failure(&format!("bom/{name}.csv"), &e)),
                    }
                }
                if json_answers.len() == 2 {
                    let text = crate::doc::canonical::to_string(&Value::Object(json_answers));
                    files.push(("bom/bom.json".to_owned(), text.into_bytes()));
                }
            }
            Format::Bom => {}
            Format::Render => {
                let r = crate::scene::render_for_package(session, part, &commit);
                keep(
                    "render/iso.png".to_owned(),
                    r,
                    &mut files,
                    &mut failures,
                    &mut warnings,
                );
            }
        }
    }
    if !failures.is_empty() {
        return Err(Error::new(
            Code::PackageFailed,
            format!(
                "{part} {label} can't be packaged: {} file{} failed (nothing was written)",
                failures.len(),
                if failures.len() == 1 { "" } else { "s" }
            ),
        )
        .with("part", part)
        .with("label", &label)
        .with("failures", failures));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let manifest = Manifest {
        built_with: built.clone(),
        children: if kind == PartKind::Assembly {
            children
        } else {
            Vec::new()
        },
        commit,
        content,
        files: files.iter().map(|(p, d)| File::of(p, d)).collect(),
        label,
        number,
        formats,
        part: part.clone(),
        project,
        record: blake3::hash(
            &files
                .iter()
                .find(|(p, _)| p == RECORD)
                .expect("the record")
                .1,
        )
        .to_hex()
        .to_string(),
        warnings,
    };
    Ok(Built { manifest, files })
}

/// The answer describing `b`.
fn described(b: &Built) -> Value {
    let m = &b.manifest;
    let mut v = json!({
        "schema": SCHEMA,
        "part": m.part,
        "label": m.label,
        "number": m.number,
        "commit": m.commit,
        "key": m.key(),
        "hash": m.hash(),
        "built_with": {"ryucad": m.built_with.ryucad, "commit": m.built_with.commit},
        "file_name": m.file_name(),
        "files": m.files.iter().map(|f| json!({"path": f.path, "blake3": f.blake3, "size": f.size, "media_type": f.media_type})).collect::<Vec<_>>(),
        "children": m.children.iter().map(|c| json!({"part": c.part, "label": c.label, "project": c.project, "package": c.package})).collect::<Vec<_>>(),
    });
    if !m.warnings.is_empty() {
        v["warnings"] = json!(m.warnings);
    }
    v
}

fn formats_of(ask: &Option<Vec<String>>) -> Result<Option<Vec<Format>>> {
    ask.as_ref()
        .map(|f| package::formats_from(&json!(f), "/formats"))
        .transpose()
}

/// `release.package`: builds the package and writes it: to `out` (a zip,
/// or a folder), into `into` under its own name, or following the answer.
pub fn package(session: &mut Session, ask: Ask) -> Result<Answer> {
    package_stamped(session, ask, &this_build())
}

/// [`package`] naming `built` as the ryucad that built it: the goldens
/// pin a package's bytes the same in a release and a development build.
pub fn package_stamped(session: &mut Session, ask: Ask, built: &BuiltWith) -> Result<Answer> {
    let part = crate::part::part_id(&ask.part)?;
    if ask.out.is_some() && ask.into.is_some() {
        return Err(Error::new(Code::Usage, "give out or into, not both"));
    }
    let formats = formats_of(&ask.formats)?;
    let b = build(
        session,
        &part,
        ask.label.as_deref(),
        formats.as_deref(),
        built,
    )?;
    let mut result = described(&b);
    let job = session.job().clone();
    let root = session.root().to_owned();
    let target = match (&ask.out, &ask.into) {
        (Some(o), None) => Some(o.clone()),
        (None, Some(i)) => Some(format!(
            "{}/{}",
            i.trim_end_matches(['/', '\\']),
            b.manifest.file_name()
        )),
        _ => None,
    };
    let folder = ask
        .out
        .as_deref()
        .is_some_and(|o| o.ends_with('/') || o.ends_with('\\') || std::path::Path::new(o).is_dir());
    match target {
        Some(o) if folder => {
            let dir = crate::export::out_path(o.trim_end_matches(['/', '\\']), &root)?;
            refuse_inside(&dir, &root)?;
            job.check()?;
            job.writing();
            let mut all = vec![(
                package::MANIFEST.to_owned(),
                b.manifest.canonical().into_bytes(),
            )];
            all.extend(b.files.iter().cloned());
            for (p, d) in &all {
                let at = dir.join(p);
                if let Some(parent) = at.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, &e))?;
                }
                // no session: a package's --out folder, never in the project (refused above)
                crate::doc::source::write_atomic(&at, d)?;
            }
            result["out"] = json!(dir);
            Ok(Answer::json(result))
        }
        Some(o) => {
            let out = crate::export::out_path(&o, &root)?;
            let zip = b.zip()?;
            crate::export::deliver(&job, result, Some(&out), "application/zip", |w| {
                w.write_all(&zip)
            })
        }
        None => {
            let zip = b.zip()?;
            Ok(Answer::bytes(result, "application/zip", zip))
        }
    }
}

/// Refuses a package folder in the project: its files (`package.json`,
/// `step/` and the rest) would be left there untracked, beside the
/// project's own, by a write that is neither logged nor undoable. `dir`
/// is absolute and its parent exists (`export::out_path`).
fn refuse_inside(dir: &std::path::Path, root: &std::path::Path) -> Result<()> {
    let name = dir.file_name().expect("out_path gives a file name");
    let parent = dir.parent().expect("an absolute path has a parent");
    // Folded to ASCII lower case, as `export::out_path` folds: Windows and
    // macOS find `<project>` and `<PROJECT>` the same folder.
    let fold = |p: &std::path::Path| {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect::<Vec<_>>()
    };
    let inside = match (parent.canonicalize(), root.canonicalize()) {
        (Ok(p), Ok(r)) => fold(&p.join(name)).starts_with(&fold(&r)),
        _ => fold(dir).starts_with(&fold(root)),
    };
    if inside {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "out {} is inside the project: a package folder is written outside it (or ask for a zip)",
                dir.display()
            ),
        )
        .with("param", "out")
        .with("path", dir));
    }
    Ok(())
}

/// How a verify ends: `verified` or `unattested`; `invalid` is
/// `package_invalid`.
fn invalid(c: &package::verify::Clause) -> Error {
    Error::new(
        Code::PackageInvalid,
        format!("the package doesn't verify: {}", c.text()),
    )
    .with("clause", c.name())
}

/// `release.package.verify`: the package at `file` judged against this
/// clone: its files, its release, its package tag, and with `rebuild` its
/// bytes built again.
pub fn verify(session: &mut Session, file: &str, rebuild: bool) -> Result<Value> {
    let path = std::path::Path::new(file);
    let meta = std::fs::metadata(path).map_err(|e| Error::io(path, &e))?;
    if meta.len() > 4 << 30 {
        return Err(invalid(&package::verify::Clause::Unreadable(
            "is larger than a package".into(),
        )));
    }
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, &e))?;
    let (m, _) = package::verify::files(&bytes).map_err(|c| invalid(&c))?;
    let (repo, releases) = crate::release::history(session)?;
    package::verify::release(repo.gix(), &releases, &m).map_err(|c| invalid(&c))?;
    let hash = m.hash();
    let release = format!("releases/{}/{}.json", m.part, m.label);
    let tag =
        package::attest::of_release(repo.gix(), &releases, repo.prefix(), &release, Some(&hash));
    let mut warnings = Vec::new();
    let mut v = json!({
        "schema": SCHEMA,
        "part": m.part,
        "label": m.label,
        "commit": m.commit,
        "hash": hash,
        "key": m.key(),
        "built_with": {"ryucad": m.built_with.ryucad, "commit": m.built_with.commit},
        "files": m.files.len(),
        "tag": {"name": releases.name(&package::attest::tag_key(&m.part, &m.label)), "status": tag.status()},
    });
    let verdict = match &tag {
        package::attest::Verdict::Attested { attestation, key } => {
            v["tag"]["key"] = json!(key);
            v["tag"]["signed_at"] = json!(attestation.signed_at);
            "verified"
        }
        package::attest::Verdict::Invalid(c) => {
            return Err(invalid(&package::verify::Clause::Tag(c.clone())));
        }
        package::attest::Verdict::Revoked { key, revoked } => {
            v["tag"]["key"] = json!(key);
            v["tag"]["revoked"] = json!(revoked);
            warnings.push(
                json!({"code": "package_attestation_revoked", "message": format!(
                "{} is signed by key {key}, revoked on {revoked}: it proves nothing",
                releases.name(&package::attest::tag_key(&m.part, &m.label)))}),
            );
            "unattested"
        }
        package::attest::Verdict::Missing => {
            warnings.push(json!({"code": "package_unattested", "message": format!(
                "{} isn't here: fetch it with {}", releases.name(&package::attest::tag_key(&m.part, &m.label)),
                ryupdm::check::FETCH_TAGS)}));
            "unattested"
        }
    };
    if rebuild {
        if m.built_with != this_build() {
            warnings.push(
                json!({"code": "package_built_with_other", "message": format!(
                "built with ryucad {} ({}), not this one: rebuild with that release",
                m.built_with.ryucad, m.built_with.commit)}),
            );
            v["rebuilt"] = json!(false);
        } else {
            let again = build(
                session,
                &m.part,
                Some(&m.label),
                Some(&m.formats),
                &m.built_with,
            )?;
            let zip = again.zip()?;
            if zip != bytes {
                let differs = again
                    .manifest
                    .files
                    .iter()
                    .find(|f| !m.files.contains(f))
                    .map_or_else(|| package::MANIFEST.to_owned(), |f| f.path.clone());
                return Err(invalid(&package::verify::Clause::RebuildDiffers(differs)));
            }
            v["rebuilt"] = json!(true);
        }
    }
    v["verdict"] = json!(verdict);
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    Ok(v)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::git::tests::git;
    use crate::release::tests::released_project;
    use crate::testing::call;

    /// A fixed ryucad, so the tests' packages don't follow the build.
    pub(crate) fn stamp() -> BuiltWith {
        BuiltWith {
            ryucad: "0.10.0".into(),
            commit: "0123456789ab".into(),
        }
    }

    fn head(root: &std::path::Path) -> String {
        let out = crate::git::tests::git_command()
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// `released_project`'s part `p` with a drawing `d`, released at A,
    /// then made 12 mm long and released at B; the two official commits.
    pub(crate) fn two_releases() -> (tempfile::TempDir, Session, String, String) {
        let (dir, mut s) = released_project();
        let root = dir.path().to_owned();
        let file = json!({"format": 1, "kind": "ryucad.drawing", "name": "Box",
            "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b"}},
            "views": {"v1": {"type": "base", "of": "p", "orient": "front", "sheet": "s1"}}});
        std::fs::create_dir_all(root.join("parts/p/drawings")).unwrap();
        crate::doc::project::write_json(&root.join("parts/p/drawings/d.json"), &file).unwrap();
        s.reload().unwrap();
        call(&mut s, "release.propose", json!({"part": "p"})).unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "release A"]);
        let a = head(&root);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 12}}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "p"})).unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "release B"]);
        let b = head(&root);
        (dir, s, a, b)
    }

    fn id(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    #[test]
    fn each_release_is_packaged_from_its_own_commit() {
        let (_dir, mut s, a, b) = two_releases();
        let pa = build(&mut s, &id("p"), Some("A"), None, &stamp()).unwrap();
        let pb = build(&mut s, &id("p"), None, None, &stamp()).unwrap();
        assert_eq!(
            (pa.manifest.label.as_str(), pa.manifest.commit.as_str()),
            ("A", a.as_str())
        );
        assert_eq!(
            (pb.manifest.label.as_str(), pb.manifest.commit.as_str()),
            ("B", b.as_str())
        );
        let paths: Vec<&str> = pa.manifest.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "drawings/d.pdf",
                "record.json",
                "render/iso.png",
                "step/100-A.step"
            ]
        );
        // A's drawing is A's: its PDF is the drawing exported at A's commit.
        let texts = |s: &mut Session, rev: &str| -> Vec<String> {
            let v = call(
                s,
                "drawing.get",
                json!({"part": "p", "drawing": "d", "rev": rev, "items": true}),
            )
            .unwrap();
            let mut out = Vec::new();
            fn walk(v: &Value, out: &mut Vec<String>) {
                match v {
                    Value::Object(o) => {
                        if let Some(Value::String(t)) = o.get("text") {
                            out.push(t.clone());
                        }
                        o.values().for_each(|x| walk(x, out));
                    }
                    Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
                    _ => {}
                }
            }
            walk(&v, &mut out);
            out
        };
        let (ta, tb) = (texts(&mut s, &a), texts(&mut s, &b));
        let pdf_a = &pa
            .files
            .iter()
            .find(|(p, _)| p == "drawings/d.pdf")
            .unwrap()
            .1;
        let pdf_b = &pb
            .files
            .iter()
            .find(|(p, _)| p == "drawings/d.pdf")
            .unwrap()
            .1;
        assert_ne!(pdf_a, pdf_b);
        // A's title block says A, B's says B.
        let rev = |t: &[String]| t.windows(2).find(|w| w[0] == "REV").map(|w| w[1].clone());
        assert_eq!(
            (rev(&ta), rev(&tb)),
            (Some("A".into()), Some("B".into())),
            "{ta:?}"
        );
        // The same inputs, the same bytes.
        let again = build(&mut s, &id("p"), Some("A"), None, &stamp()).unwrap();
        assert_eq!(pa.zip().unwrap(), again.zip().unwrap());
        assert_eq!(pa.manifest.key(), again.manifest.key());
    }

    #[test]
    fn proposed_obsolete_and_missing_releases_have_no_package() {
        let (dir, mut s, _, _) = two_releases();
        let root = dir.path();
        let e = build(&mut s, &id("p"), Some("C"), None, &stamp()).unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 14}}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "p"})).unwrap();
        let e = build(&mut s, &id("p"), Some("C"), None, &stamp()).unwrap_err();
        assert_eq!(e.code, Code::LabelNotOfficial);
        std::fs::remove_file(root.join("releases/p/C.json")).unwrap();
        call(
            &mut s,
            "release.obsolete",
            json!({"part": "p", "label": "B"}),
        )
        .unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "B obsolete"]);
        let e = build(&mut s, &id("p"), Some("B"), None, &stamp()).unwrap_err();
        assert_eq!(e.code, Code::ReleaseObsolete);
    }

    #[test]
    fn a_package_verifies_offline_and_tampering_is_named() {
        let (dir, mut s, _, _) = two_releases();
        let root = dir.path().to_owned();
        let out = tempfile::tempdir().unwrap();
        let zip = out.path().join("p-A.zip");
        let ask = Ask {
            part: "p".into(),
            label: Some("A".into()),
            out: Some(zip.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let v = package_stamped(&mut s, ask, &this_build()).unwrap().result;
        assert_eq!(v["out"], json!(zip));
        let file = zip.to_string_lossy().into_owned();
        let ok = verify(&mut s, &file, true).unwrap();
        assert_eq!(
            (&ok["verdict"], &ok["rebuilt"]),
            (&json!("unattested"), &json!(true))
        );
        // A file changed inside the zip.
        let (m, mut files) = ryupdm::package::verify::files(&std::fs::read(&zip).unwrap()).unwrap();
        files
            .iter_mut()
            .find(|(p, _)| p == "step/100-A.step")
            .unwrap()
            .1
            .push(b'\n');
        let bad = out.path().join("bad.zip");
        std::fs::write(&bad, ryupdm::package::archive(&m, &files).unwrap()).unwrap();
        let e = verify(&mut s, &bad.to_string_lossy(), false).unwrap_err();
        assert_eq!(
            (e.code, &e.details["clause"]),
            (Code::PackageInvalid, &json!("file_changed"))
        );
        // A package of another release's commit.
        let mut moved = m.clone();
        moved.commit = "0".repeat(40);
        std::fs::write(&bad, ryupdm::package::archive(&moved, &files).unwrap()).unwrap();
        let e = verify(&mut s, &bad.to_string_lossy(), false).unwrap_err();
        assert_eq!(e.details["clause"], json!("file_changed"));
        let _ = root;
    }

    #[test]
    fn a_package_folder_inside_the_project_is_refused() {
        let (dir, mut s, _, _) = two_releases();
        let root = dir.path().to_owned();
        let ask = |out: String| Ask {
            part: "p".into(),
            label: Some("A".into()),
            out: Some(out),
            ..Default::default()
        };
        for out in [
            format!("{}/", root.display()),
            format!("{}/pkg/", root.display()),
            format!("{}/parts/", root.display()),
            // The project's own folder in another case: the same folder on
            // Windows and macOS, so refused on every platform.
            format!(
                "{}/{}/",
                root.parent().unwrap().display(),
                root.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_ascii_uppercase()
            ),
        ] {
            let e = package_stamped(&mut s, ask(out.clone()), &this_build()).unwrap_err();
            assert_eq!(
                (e.code, &e.details["param"]),
                (Code::InvalidParam, &json!("out")),
                "{out}"
            );
        }
        assert!(!root.join("pkg").exists());
        assert!(!root.join("package.json").exists());
        // Outside the project, the folder is written.
        let out = tempfile::tempdir().unwrap();
        let folder = out.path().join("p-A");
        let v = package_stamped(&mut s, ask(format!("{}/", folder.display())), &this_build())
            .unwrap()
            .result;
        assert_eq!(v["out"], json!(folder));
        assert!(folder.join(package::MANIFEST).is_file());
    }

    #[test]
    fn a_package_tag_verifies_under_a_trusted_key_only() {
        use ryupdm::attest::{TEST_SEED, public_key, test_key_id};
        let (dir, mut s, a, _) = two_releases();
        let root = dir.path().to_owned();
        let out = tempfile::tempdir().unwrap();
        let zip = out.path().join("p-A.zip");
        let b = build(&mut s, &id("p"), Some("A"), None, &this_build()).unwrap();
        std::fs::write(&zip, b.zip().unwrap()).unwrap();
        let at = ryupdm::package::attest::Attestation {
            built_with: b.manifest.built_with.clone(),
            commit: a.clone(),
            hash: b.manifest.hash(),
            label: "A".into(),
            part: id("p"),
            project: b.manifest.project.clone(),
            record: b.manifest.record.clone(),
            signed_at: "2026-12-01T10:02:44Z".into(),
        };
        let msg = out.path().join("msg");
        std::fs::write(&msg, ryupdm::package::attest::sign(&at, &TEST_SEED)).unwrap();
        git(
            &root,
            &[
                "tag",
                "-a",
                "-F",
                &msg.to_string_lossy(),
                "ryupdm/p/A.package",
                &a,
            ],
        );
        // The test key isn't trusted: the tag is invalid.
        let file = zip.to_string_lossy().into_owned();
        let e = verify(&mut s, &file, false).unwrap_err();
        assert_eq!(e.details["clause"], json!("tag_unknown_key"));
        let listed = call(&mut s, "release.list", json!({"part": "p"})).unwrap();
        let rec = listed["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["label"] == "A")
            .unwrap();
        assert_eq!(
            rec["package"],
            json!({"tag": "invalid", "clause": "unknown_key"})
        );
        let found = call(&mut s, "release.check", json!({})).unwrap_err();
        let report = &found.details["findings"];
        assert!(
            report.to_string().contains("release_package_invalid"),
            "{report}"
        );
        assert!(
            !report.to_string().contains("release_tag_orphaned"),
            "{report}"
        );
        let _ = (public_key(&TEST_SEED), test_key_id());
    }

    #[test]
    fn a_package_tag_under_a_trusted_key_verifies() {
        use ryupdm::attest::{TEST_SEED, hex, public_key, test_key_id};
        let (dir, mut s) = released_project();
        let root = dir.path().to_owned();
        // The test key is trusted where A arrives: a rule for no part of
        // this project, and the key beside it.
        let keys = json!({test_key_id(): hex(&public_key(&TEST_SEED))});
        call(
            &mut s,
            "pdm.configure",
            json!({"approvals": {
            "rules": [{"count": 1, "from": ["alice"], "parts": ["none-*"]}], "keys": keys}}),
        )
        .unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "keys"]);
        call(&mut s, "release.propose", json!({"part": "p"})).unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "release A"]);
        let a = head(&root);
        let b = build(&mut s, &id("p"), None, None, &this_build()).unwrap();
        let out = tempfile::tempdir().unwrap();
        let zip = out.path().join("p-A.zip");
        std::fs::write(&zip, b.zip().unwrap()).unwrap();
        let at = ryupdm::package::attest::Attestation {
            built_with: b.manifest.built_with.clone(),
            commit: a.clone(),
            hash: b.manifest.hash(),
            label: "A".into(),
            part: id("p"),
            project: b.manifest.project.clone(),
            record: b.manifest.record.clone(),
            signed_at: "2026-12-01T10:02:44Z".into(),
        };
        let msg = out.path().join("msg");
        std::fs::write(&msg, ryupdm::package::attest::sign(&at, &TEST_SEED)).unwrap();
        git(
            &root,
            &[
                "tag",
                "-a",
                "-F",
                &msg.to_string_lossy(),
                "ryupdm/p/A.package",
                &a,
            ],
        );
        let v = verify(&mut s, &zip.to_string_lossy(), false).unwrap();
        assert_eq!(
            (&v["verdict"], &v["tag"]["key"]),
            (&json!("verified"), &json!(test_key_id()))
        );
        let listed = call(&mut s, "release.list", json!({"part": "p"})).unwrap();
        assert_eq!(listed["records"][0]["package"]["tag"], "attested");
        assert_eq!(
            listed["records"][0]["package"]["hash"],
            json!(b.manifest.hash())
        );
        // A tag naming another package's hash is invalid for this one.
        let mut other = b.manifest.clone();
        other.formats = vec![Format::Step];
        let files: Vec<_> = b
            .files
            .iter()
            .filter(|(p, _)| !p.starts_with("render/"))
            .cloned()
            .collect();
        other.files = files.iter().map(|(p, d)| File::of(p, d)).collect();
        std::fs::write(&zip, package::archive(&other, &files).unwrap()).unwrap();
        let e = verify(&mut s, &zip.to_string_lossy(), false).unwrap_err();
        assert_eq!(e.details["clause"], json!("tag_hash"));
    }

    #[test]
    fn an_assemblys_package_lists_its_children_and_its_bom() {
        let (dir, mut s) = released_project();
        let root = dir.path().to_owned();
        call(&mut s, "release.propose", json!({"part": "p"})).unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "p A"]);
        call(&mut s, "part.create", json!({"part": "pin"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "pin", "type": "cylinder", "id": "c",
            "params": {"radius": 1, "height": 8}}),
        )
        .unwrap();
        call(
            &mut s,
            "part.create",
            json!({"part": "asm", "kind": "assembly"}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "asm", "type": "instance", "id": "p1",
            "of": "p", "rev": "A"}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "asm", "type": "instance", "id": "q1",
            "of": "pin"}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "asm"})).unwrap_err();
        // An assembly placing a part in work can't be released; package
        // the one placing p at A alone.
        call(&mut s, "feature.delete", json!({"part": "asm", "id": "q1"})).unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "asm", "properties": {"number": "200"}}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "asm"})).unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "asm A"]);
        let b = build(&mut s, &id("asm"), None, None, &stamp()).unwrap();
        let project = b.manifest.project.clone();
        assert_eq!(
            b.manifest.children,
            [Child {
                part: id("p"),
                label: Some("A".into()),
                project,
                package: None
            }]
        );
        let paths: Vec<&str> = b.manifest.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "bom/bom.json",
                "bom/flat.csv",
                "bom/indented.csv",
                "record.json",
                "render/iso.png",
                "step/200-A.step"
            ]
        );
        let csv = &b.files.iter().find(|(p, _)| p == "bom/flat.csv").unwrap().1;
        assert!(
            String::from_utf8_lossy(csv).contains(",100,,p,A,1,each,"),
            "{}",
            String::from_utf8_lossy(csv)
        );
    }
}
