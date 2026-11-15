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
/// for one in another project: a library component's objects are its own
/// library's (a design's placements of other projects are their pins,
/// read in [`design`]).
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

/// The blocks a design reaches from its root block through instances,
/// in ID order: what its content hash and readiness read.
pub(crate) fn reached(
    conn: &crate::design::Connectivity,
    root: &Id,
) -> std::collections::BTreeSet<Id> {
    let mut reached = std::collections::BTreeSet::new();
    let mut todo = vec![root.clone()];
    while let Some(b) = todo.pop() {
        if !reached.insert(b.clone()) {
            continue;
        }
        if let Some(files) = conn.blocks.get(&b) {
            todo.extend(
                files
                    .instances
                    .values()
                    .filter_map(|i| i.item.as_ref().ok())
                    .map(|i| i.of.clone()),
            );
        }
    }
    reached
}

/// The design `part`'s file, read.
pub(crate) fn design_of(src: &dyn Source, part: &Id) -> Result<(String, crate::conn::Design)> {
    use crate::conn::{DESIGN_KIND, DESIGNS};
    let at = crate::design::top_path(DESIGNS, part);
    let bytes = src
        .read(&at)?
        .ok_or_else(|| Error::new(Code::NotFound, format!("no design {part}")).with("path", &at))?;
    let d = crate::doc::read(&bytes, &at, DESIGN_KIND, crate::conn::newest(DESIGN_KIND))?;
    Ok((at, d))
}

/// A design's pieces (design 629's "Content hashes"): the manifest's
/// properties and schematic settings, the design file, each block it
/// reaches from its root in ID order (`block.json`, its components,
/// nets and instances by name, `layout.json`), the rails and classes
/// those name, `erc.json`, then each `self` library component placed,
/// in the order first placed, with its symbol and footprints.
fn design(h: &mut Pieces, src: &dyn Source, m: &Manifest, part: &Id) -> Result<()> {
    use crate::conn::{BLOCK_FILE, BLOCKS, CLASSES, COMPONENTS, INSTANCES, NETS, Node, RAILS};
    use std::collections::BTreeSet;
    h.piece(
        crate::canonical::to_string(&serde_json::json!({
            "properties": m.properties, "schematic": m.schematic,
        }))
        .as_bytes(),
    );
    let (at, d) = design_of(src, part)?;
    h.needed(src, &at)?;
    let conn = crate::design::load(src)?;
    let reached = reached(&conn, &d.root);
    let (mut rails, mut classes) = (BTreeSet::new(), BTreeSet::new());
    let mut placed: Vec<Id> = Vec::new();
    let mut others: Vec<(Id, Id)> = Vec::new();
    let what = format!("design {part}");
    let mut rail = |n: &Node| {
        if let Node::Rail(r) = n {
            rails.insert(r.clone());
        }
    };
    for b in &reached {
        let dir = format!("{BLOCKS}/{b}");
        h.needed(src, &format!("{dir}/{BLOCK_FILE}"))?;
        for sub in [COMPONENTS, NETS, INSTANCES] {
            let at = format!("{dir}/{sub}");
            let mut names: Vec<String> = src
                .list(&at)?
                .into_iter()
                .filter(|(n, k)| *k == crate::source::Kind::File && n.ends_with(".json"))
                .map(|(n, _)| n)
                .collect();
            names.sort();
            for n in names {
                h.needed(src, &format!("{at}/{n}"))?;
            }
        }
        h.file(src, &format!("{dir}/{}", crate::schem::LAYOUT))?;
        let Some(files) = conn.blocks.get(b) else {
            continue;
        };
        for c in files
            .components
            .values()
            .filter_map(|c| c.item.as_ref().ok())
        {
            c.pins.values().for_each(&mut rail);
            match &c.of.library {
                None if !placed.contains(&c.of.id) => placed.push(c.of.id.clone()),
                None => {}
                Some(alias) => {
                    let k = (alias.clone(), c.of.id.clone());
                    if !others.contains(&k) {
                        others.push(k);
                    }
                }
            }
        }
        for i in files
            .instances
            .values()
            .filter_map(|i| i.item.as_ref().ok())
        {
            i.ports.values().for_each(&mut rail);
        }
        for n in files.nets.values().filter_map(|n| n.item.as_ref().ok()) {
            if let Some(c) = &n.class {
                classes.insert(c.clone());
            }
        }
    }
    for r in &rails {
        if let Some(Ok(rail)) = conn.rails.get(r).map(|l| l.item.as_ref())
            && let Some(c) = &rail.class
        {
            classes.insert(c.clone());
        }
    }
    // A missing rail or class is no piece: a dangling name is ERC's fault,
    // and the name is in the bytes of what names it.
    for r in &rails {
        h.file(src, &crate::design::top_path(RAILS, r))?;
    }
    for c in &classes {
        h.file(src, &crate::design::top_path(CLASSES, c))?;
    }
    h.file(src, crate::project::ERC)?;
    for id in &placed {
        let c = libcomp(src, id)?;
        h.needed(src, &object_path(crate::library::COMPONENTS, id))?;
        objects(h, src, id, &c)?;
    }
    // Each other project's component placed, in the order first placed:
    // its pin (design 629). A same-repository pin names one immutable
    // record by (project, component, label); another repository's adds
    // the content hash `ryueda.lock` holds for it.
    for (alias, id) in &others {
        let pin = m
            .libraries
            .get(alias)
            .and_then(|a| Some((a, a.labels.get(id)?, a.project.as_ref()?)));
        let Some((a, label, project)) = pin else {
            return Err(not_released(alias, id, &what));
        };
        h.piece(b"pin");
        h.piece(project.to_string().as_bytes());
        h.piece(id.as_str().as_bytes());
        h.piece(label.as_bytes());
        if a.url.is_some() {
            h.piece(foreign_content(src, project, id, label)?.as_bytes());
        }
    }
    Ok(())
}

/// The content hash `ryueda.lock` (in `src`) holds for `project`'s
/// `id`@`label`: `dependency_unlocked` when it holds none.
fn foreign_content(
    src: &dyn Source,
    project: &crate::id::ProjectId,
    id: &Id,
    label: &str,
) -> Result<String> {
    use crate::project::{LOCK, LOCK_KIND};
    let lock = src
        .read(LOCK)?
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| crate::canonical::parse(&t).ok())
        .and_then(|v| ryu_doc::lock::Lock::from_json_as(&v, LOCK_KIND).ok());
    lock.as_ref()
        .and_then(|l| l.label(project, id, label))
        .map(|l| l.content.clone())
        .ok_or_else(|| {
            Error::new(
                Code::DependencyUnlocked,
                format!("{id}@{label} of {project} isn't in {LOCK}: write it again (`ryueda lib update --offline`)"),
            )
            .with("project", project)
            .with("component", id)
            .with("label", label)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdm::tests::{board, id};

    #[test]
    fn a_pin_is_a_piece_and_an_unpinned_component_has_no_hash() {
        let f = board(ProjectKind::Design);
        let plain = hash(&f, &id("d")).unwrap();
        let path = "blocks/top/components/r1.json";
        let mut v: serde_json::Value = serde_json::from_slice(&f[path]).unwrap();
        v["of"] = serde_json::json!({"library": "passives", "id": "r_100k"});
        let mut g = f.clone();
        g.insert(path.into(), crate::canonical::to_string(&v).into_bytes());
        let with_pin = |label: Option<&str>, url: bool, lock: Option<&str>| {
            let mut h = g.clone();
            let mut m = Manifest::read(&h["ryueda.json"]).unwrap();
            let mut a = crate::project::Alias::local("../passives");
            if let Some(l) = label {
                a.project = Some(crate::project::tests::pid(2));
                a.labels.insert(id("r_100k"), l.into());
            }
            if url {
                a.url = Some("file:///libs".into());
                a.path = String::new();
            }
            m.libraries.insert(id("passives"), a);
            h.insert("ryueda.json".into(), m.to_text().into_bytes());
            if let Some(content) = lock {
                let mut l = ryu_doc::lock::Lock::default();
                let mut locked = ryu_doc::lock::Locked {
                    url: "file:///libs".into(),
                    ..Default::default()
                };
                locked.labels.insert(
                    (id("r_100k"), "A".into()),
                    ryu_doc::lock::LockedLabel {
                        commit: "a".repeat(40),
                        content: content.repeat(64),
                    },
                );
                l.projects.insert(crate::project::tests::pid(2), locked);
                h.insert(
                    crate::project::LOCK.into(),
                    crate::canonical::to_string(&l.to_json_as(crate::project::LOCK_KIND))
                        .into_bytes(),
                );
            }
            hash(&h, &id("d"))
        };
        assert_eq!(
            with_pin(None, false, None).unwrap_err().code,
            Code::ComponentNotReleased
        );
        let a = with_pin(Some("A"), false, None).unwrap();
        let b = with_pin(Some("B"), false, None).unwrap();
        assert_ne!(a, b, "the label is a piece");
        assert_ne!(a, plain);
        // Another repository's: the locked content hash is a piece too.
        assert_eq!(
            with_pin(Some("A"), true, None).unwrap_err().code,
            Code::DependencyUnlocked
        );
        let c = with_pin(Some("A"), true, Some("c")).unwrap();
        let d = with_pin(Some("A"), true, Some("d")).unwrap();
        assert_ne!(c, d);
        assert_ne!(c, a);
        // A design without pins hashes as it did (its golden doesn't move).
        assert_eq!(hash(&f, &id("d")).unwrap(), plain);
    }

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

    #[test]
    fn a_designs_hash_covers_what_it_reaches_and_not_style_or_sourcing() {
        let f = board(ProjectKind::Design);
        let h = hash(&f, &id("d")).unwrap();
        assert_eq!(hash(&f, &id("d")).unwrap(), h);
        let with = |path: &str, bytes: &[u8]| {
            let mut g = f.clone();
            g.insert(path.into(), bytes.to_vec());
            hash(&g, &id("d")).unwrap()
        };
        let edit = |path: &str, key: &str, value: serde_json::Value| {
            let mut v: serde_json::Value = serde_json::from_slice(&f[path]).unwrap();
            v[key] = value;
            with(path, crate::canonical::to_string(&v).as_bytes())
        };
        // What the design is: its file, its blocks' files, the rails and
        // classes they name, the self objects placed, erc.json.
        assert_ne!(edit("designs/d.json", "name", "Other".into()), h);
        assert_ne!(
            edit("blocks/top/components/r1.json", "refdes", "R9".into()),
            h
        );
        assert_ne!(edit("rails/v3v3.json", "voltage", "5 V".into()), h);
        assert_ne!(
            edit("library/components/r_100k.json", "value", "99k".into()),
            h
        );
        assert_ne!(
            with("erc.json", b"{\"format\": 1, \"kind\": \"ryueda.erc\"}\n"),
            h
        );
        // A rail no block names, style, sourcing and lint don't move it.
        assert_eq!(
            with(
                "rails/spare.json",
                b"{\"format\": 1, \"kind\": \"ryueda.rail\", \"name\": \"SPARE\"}\n"
            ),
            h
        );
        assert_eq!(with("style.json", b"{}\n"), h);
        assert_eq!(with("parts.json", b"{}\n"), h);
        assert_eq!(with("lint.json", b"{}\n"), h);
        // Another project's component at working has no hash.
        let mut v: serde_json::Value =
            serde_json::from_slice(&f["blocks/top/components/r1.json"]).unwrap();
        v["of"] = serde_json::json!({"library": "passives", "id": "r_100k"});
        let mut g = f.clone();
        g.insert(
            "blocks/top/components/r1.json".into(),
            crate::canonical::to_string(&v).into_bytes(),
        );
        assert_eq!(
            hash(&g, &id("d")).unwrap_err().code,
            Code::ComponentNotReleased
        );
    }
}
