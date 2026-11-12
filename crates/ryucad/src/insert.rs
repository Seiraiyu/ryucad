//! Inserts (design 521): a part takes the solids another part shows, at
//! the revision it pins, into its own history, moved by its placement.
//! What an insert reads is the source's model from outside the part, as
//! an instance's is ([`crate::assembly::Children`]); its result is never
//! on disk, since it is a move of bodies the source part keeps.

use crate::assembly::{Children, Model, Pin};
use crate::doc::assembly::Rev;
use crate::doc::id::Id;
use crate::doc::insert::Insert;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Made, Moved};
use crate::reference::Lineage;
use rayon::prelude::*;
use ryukei::geom::Rigid;
use std::collections::BTreeMap;
use std::sync::Arc;

/// What an insert inserted: its source and pin, its placement, each
/// solid's lineage in this part's frame, and the warnings building it
/// gave (`insert_source_failed`, `insert_obsolete`).
#[derive(Clone, Debug)]
pub struct Inserted {
    pub of: Id,
    pub rev: Rev,
    pub pin: Option<Pin>,
    pub pose: Rigid,
    /// The source part's model hash it was built from.
    pub model: String,
    pub lineage: BTreeMap<Id, Arc<Lineage>>,
    /// Each solid's source part (itself, or the part a nested insert
    /// reads) and source key: what `import convert` names.
    pub sources: BTreeMap<Id, (Id, Id)>,
    pub warnings: Vec<serde_json::Value>,
}

/// The hash of `source` moved by `pose`: equal hashes, equal bodies.
pub fn inserted_hash(source: &str, pose: &Rigid) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"ryucad.inserted\0");
    h.update(source.as_bytes());
    h.update(&crate::assembly::pose_bits(pose));
    h.finalize().to_hex()[..32].to_owned()
}

/// Whether `r` is the identity, bit for bit.
fn is_identity(r: &Rigid) -> bool {
    crate::assembly::pose_bits(r) == crate::assembly::pose_bits(&Rigid::IDENTITY)
}

/// Builds insert `id` of `part` from the source's model in `children`.
pub fn build(part: &Part, id: &Id, ins: &Insert, children: &Children) -> Result<Made> {
    let pose = match &ins.placement {
        None => Rigid::IDENTITY,
        Some(p) => crate::rebuild::placement(p).map_err(|e| e.with("feature", id))?,
    };
    let model = match children.get(&(ins.part.clone(), ins.rev.clone())) {
        Some(Ok(m)) => Arc::clone(m),
        Some(Err(e)) => return Err(source_error(id, ins, e)),
        None => {
            return Err(Error::new(
                Code::Internal,
                format!(
                    "{id}: {} wasn't rebuilt before the part inserting it",
                    ins.part
                ),
            )
            .with("feature", id));
        }
    };
    // A part's model is one occurrence at its origin; an assembly's is
    // refused before it is read (see `source_error`).
    let Some(occ) = model.occurrences.first() else {
        return Err(empty(id, &ins.part));
    };
    if occ.solids.is_empty() {
        return Err(empty(id, &ins.part));
    }
    let identity = is_identity(&pose);
    let moved: Vec<Result<(Id, Arc<Built>, Arc<Lineage>, (Id, Id))>> = occ
        .solids
        .par_iter()
        .map(|s| {
            let key = Id::solid_key(&format!("{id}/{}", s.founder), "feature")
                .expect("an ID and a key make a key");
            // What `import convert` names for a reference solid: the
            // source part and its key. A nested insert's reference is
            // converted where it was imported; its hint names the first
            // part down, which says where to look.
            let source = (ins.part.clone(), s.founder.clone());
            if identity {
                return Ok((key, Arc::clone(&s.built), Arc::clone(&s.lineage), source));
            }
            let body = s.built.body.transformed(&pose).map_err(|e| {
                Error::new(
                    Code::InvalidParam,
                    format!("{id}: {} leaves the size box when placed: {e}", s.founder),
                )
                .with("feature", id)
                .with("pointer", "/placement")
                .with("reason", "outside_size_box")
            })?;
            let built = Built {
                body,
                journal: Default::default(),
                origins: Arc::new(moved_origins(id, &s.built)),
                hash: inserted_hash(&s.built.hash, &pose),
                reference: s.built.reference,
                moved: Some(Arc::new(Moved {
                    source: Arc::clone(&s.built),
                    feature: Arc::clone(&s.feature),
                    pose,
                })),
            };
            Ok((
                key,
                Arc::new(built),
                Arc::new(s.lineage.moved(&pose)),
                source,
            ))
        })
        .collect();
    let mut solids = Vec::with_capacity(moved.len());
    let mut lineage = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for r in moved {
        let (key, b, l, src) = r?;
        lineage.insert(key.clone(), l);
        sources.insert(key.clone(), src);
        solids.push((key, b));
    }
    let mut warnings = Vec::new();
    if let Some(w) = failing_warning(part, id, ins, &model) {
        warnings.push(w);
    }
    if model.pin.as_ref().is_some_and(|p| p.obsolete) {
        warnings.push(obsolete_warning(id, ins));
    }
    let inserted = Inserted {
        of: ins.part.clone(),
        rev: ins.rev.clone(),
        pin: model.pin.clone(),
        pose,
        model: model.hash.clone(),
        lineage,
        sources,
        warnings,
    };
    Ok(Made {
        inserted: Some(Arc::new(inserted)),
        ..Made::new(solids)
    })
}

/// An insert's report: `{of, rev, pin?, solids: [{key, kind, reference,
/// hash}], count}`, the pin `{official, commit}` or `{proposed}` as an
/// instance's (design 521).
pub fn report(ins: &Insert, out: Option<&Made>) -> serde_json::Value {
    let of = match ins.rev.project() {
        Some(p) => format!("{p}:{}", ins.part),
        None => ins.part.to_string(),
    };
    let mut v = serde_json::json!({"of": of, "rev": ins.rev.to_json()});
    if let Some(p) = &ins.placement {
        v["placement"] = p.to_json();
    }
    if let Some(m) = out {
        let solids: Vec<serde_json::Value> = m.solids.iter().map(|(k, b)| serde_json::json!({
            "key": k, "kind": crate::kind::name(&b.body), "reference": b.reference, "hash": b.hash,
        })).collect();
        v["count"] = serde_json::json!(solids.len());
        v["solids"] = serde_json::json!(solids);
        if let Some(pin) = m.inserted.as_ref().and_then(|i| i.pin.as_ref()) {
            v["pin"] = pin.to_json();
        }
    }
    v
}

/// An inserted entity's origin: `{feature: <insert>, solid, from: <the
/// source origin>}` (design 521).
fn moved_origins(id: &Id, source: &Built) -> crate::rebuild::Origins {
    source
        .origins
        .iter()
        .map(|(name, from)| {
            (
                name.clone(),
                serde_json::json!({"feature": id, "from": from}),
            )
        })
        .collect()
}

fn empty(id: &Id, of: &Id) -> Error {
    Error::new(
        Code::InsertSourceEmpty,
        format!("{id} inserts {of}, which shows no solid"),
    )
    .with("feature", id)
    .with("part", of)
}

/// The insert's error for a source that couldn't be read at all.
fn source_error(id: &Id, ins: &Insert, e: &Error) -> Error {
    match e.code {
        Code::InsertCycle
        | Code::AssemblyCycle
        | Code::LabelNotFound
        | Code::LabelNotOfficial
        | Code::InsertNotReleased
        | Code::DependencyNotFetched
        | Code::DependencyRewritten
        | Code::DependencyProjectMismatch
        | Code::DependencyMissing => {
            let mut e = e.clone().with("feature", id);
            if e.code == Code::AssemblyCycle {
                e.code = Code::InsertCycle;
            }
            e
        }
        Code::NotFound => Error::new(
            Code::NotFound,
            format!(
                "{id} inserts {}, which isn't a part of the project",
                ins.part
            ),
        )
        .with("feature", id)
        .with("part", &ins.part),
        _ => Error::new(
            Code::DependencyFailed,
            format!("{id} inserts {}, which failed to rebuild", ins.part),
        )
        .with("feature", id)
        .with("dependency", &ins.part)
        .with("cause", e.code),
    }
}

/// `wrong_part_kind` for an insert of an assembly (question 625).
pub fn assembly_error(id: &Id, of: &Id) -> Error {
    Error::new(
        Code::WrongPartKind,
        format!("{id} inserts {of}, an assembly: only a part's solids are inserted"),
    )
    .with("feature", id)
    .with("part", of)
    .with("kind", "assembly")
    .with(
        "hint",
        "flatten a STEP assembly into one part (ryucad import <file> --flatten), or insert its parts one by one",
    )
}

/// `insert_source_failed` when the source's own rebuild failed: it
/// inserts each solid as the source shows it, naming the failed features.
fn failing_warning(part: &Part, id: &Id, ins: &Insert, model: &Model) -> Option<serde_json::Value> {
    let failing = model.failing.as_ref()?;
    (!failing.is_empty()).then(|| {
        serde_json::json!({"code": "insert_source_failed", "part": part.id, "feature": id,
            "of": ins.part, "rev": ins.rev.to_json(), "failing": failing,
            "message": format!("{id} inserts {}, which failed to rebuild ({}): each solid as it shows", ins.part, failing.join(", "))})
    })
}

/// `insert_obsolete`: pinned to a label an obsolete record names.
fn obsolete_warning(id: &Id, ins: &Insert) -> serde_json::Value {
    serde_json::json!({"code": "insert_obsolete", "feature": id, "part": ins.part,
        "rev": ins.rev.to_json(),
        "message": format!("{id} inserts {} at {}, which is obsolete", ins.part, ins.rev.text())})
}

/// [`build`], refusing an assembly as the source before it is read
/// (question 625).
pub fn build_checked(part: &Part, id: &Id, ins: &Insert, children: &Children) -> Result<Made> {
    if let Some(Ok(m)) = children.get(&(ins.part.clone(), ins.rev.clone()))
        && m.assembly
    {
        return Err(assembly_error(id, &ins.part));
    }
    build(part, id, ins, children)
}

/// What `insert.update` is asked.
pub struct Update<'a> {
    pub part: &'a str,
    pub feature: &'a str,
    pub to: Option<&'a str>,
    pub dry_run: bool,
}

/// Moves an insert's pin as one logged change, answering before it
/// writes what moving it changes: the inserted solids `added`, `removed`
/// and `changed` (by key), and every feature of the part that fails after
/// the move and didn't before, with its error (its `candidates` where it
/// has them). A dry run rebuilds over the proposed files and writes
/// nothing.
pub fn update(
    session: &mut crate::session::Session,
    ask: Update<'_>,
    expected: Option<&str>,
) -> Result<serde_json::Value> {
    use crate::doc::files::Feature;
    use serde_json::json;
    use std::collections::BTreeSet;
    session.check_project_revision(expected)?;
    let pid = crate::server::part_id(ask.part)?;
    let fid = Id::parse(ask.feature, "feature").map_err(|e| e.with("param", "feature"))?;
    let p = session.project().part(&pid)?.clone();
    let Some(Ok(Feature::Insert(ins))) = p.features.get(&fid) else {
        return Err(
            Error::new(Code::NotFound, format!("{pid} has no insert {fid}"))
                .with("part", &pid)
                .with("feature", &fid),
        );
    };
    let to = match ask.to.unwrap_or("newest") {
        "working" if ins.rev.project().is_none() => Rev::Working,
        "working" => {
            return Err(Error::new(
                Code::InstanceForeignWorking,
                format!(
                    "{} is another project's: insert one of its official release labels",
                    ins.part
                ),
            )
            .with("param", "to"));
        }
        "newest" => match crate::release::newest_official(session, &ins.part, ins.rev.project())? {
            Some(l) => relabel(&ins.rev, l),
            None => {
                return Err(Error::new(
                    Code::LabelNotFound,
                    format!("{} has no official release to update {fid} to", ins.part),
                )
                .with("part", &ins.part)
                .with("param", "to"));
            }
        },
        l => relabel(
            &ins.rev,
            crate::doc::assembly::label_text(l.strip_prefix("label:").unwrap_or(l))
                .map_err(|e| e.with("param", "to"))?,
        ),
    };
    let rel = format!("parts/{pid}/features/{fid}.json");
    let bytes = pinned_bytes(ins, &to);
    let writes = BTreeMap::from([(rel.clone(), Some(bytes.clone()))]);
    let before = session.rebuilt(&pid)?.clone();
    let failed_before: BTreeSet<Id> = before
        .features
        .iter()
        .filter(|(_, r)| r.is_err())
        .map(|(f, _)| f.clone())
        .collect();
    let mut proposed = session.proposed(&writes)?;
    let after = proposed.rebuilt(&pid)?.clone();
    // A label that doesn't resolve fails the insert itself: that is the
    // answer's error, not a reader's.
    if let Some((_, Err(e))) = after.features.iter().find(|(f, _)| *f == fid) {
        return Err(e.clone());
    }
    let keys = |r: &crate::rebuild::PartResult| -> BTreeMap<Id, String> {
        r.output(&fid)
            .map(|o| {
                o.solids()
                    .iter()
                    .map(|(k, b)| (k.clone(), b.hash.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (a, b) = (keys(&before), keys(&after));
    let added: Vec<&Id> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    let removed: Vec<&Id> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let changed: Vec<&Id> = b
        .iter()
        .filter(|(k, h)| a.get(*k).is_some_and(|x| x != *h))
        .map(|(k, _)| k)
        .collect();
    let failing: Vec<serde_json::Value> = after
        .features
        .iter()
        .filter(|(f, r)| r.is_err() && !failed_before.contains(f))
        .map(|(f, r)| json!({"feature": f, "error": r.as_ref().err()}))
        .collect();
    let mut v = json!({"part": pid, "feature": fid, "from": ins.rev.to_json(),
        "to": to.to_json(),
        "solids": {"added": added, "removed": removed, "changed": changed},
        "failing": failing, "written": false});
    if ask.dry_run {
        return Ok(v);
    }
    let current = crate::mate::current(session, &pid)?;
    let at = session.root().join(&rel);
    session.write_bytes(&at, &bytes)?;
    session.reload()?;
    crate::deps::keep_lock(session)?;
    let assemblies = crate::mate::keep_current(session, &current)?;
    v["written"] = json!(true);
    v["project_revision"] = json!(session.project_revision()?);
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    Ok(v)
}

/// Insert `ins`'s file pinned to `to`, canonical: what an update writes.
fn pinned_bytes(ins: &Insert, to: &Rev) -> Vec<u8> {
    let mut moved = ins.clone();
    moved.rev = to.clone();
    let f = crate::doc::files::Feature::Insert(Box::new(moved));
    crate::doc::canonical::to_string(&f.to_json()).into_bytes()
}

/// Insert `fid` of `pid`'s file pinned to `to` as an update answers it
/// (`"working"`, `{label}` or `{label, project}`): what doctor's
/// `insert_update` repair writes.
pub(crate) fn moved_bytes(
    session: &crate::session::Session,
    pid: &str,
    fid: &str,
    to: &serde_json::Value,
) -> Option<Vec<u8>> {
    let p = session.project().parts.get(&Id::parse(pid, "part").ok()?)?;
    let ins = p
        .features
        .get(&Id::parse(fid, "feature").ok()?)?
        .as_ref()
        .ok()?
        .insert()?;
    let to = match (to, ins.rev.project()) {
        (serde_json::Value::String(w), _) if w == "working" => Rev::Working,
        (v, Some(project)) => Rev::Foreign {
            project: project.clone(),
            label: v["label"].as_str()?.to_owned(),
        },
        (v, None) => Rev::Label(v["label"].as_str()?.to_owned()),
    };
    Some(pinned_bytes(ins, &to))
}

/// `rev` at label `l`, keeping its project.
fn relabel(rev: &Rev, l: String) -> Rev {
    match rev.project() {
        Some(p) => Rev::Foreign {
            project: p.clone(),
            label: l,
        },
        None => Rev::Label(l),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::assembly::tests::{id, insert_in};
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    /// pcb (a 10 × 10 × 1.6 mm board and a 2 mm cube on it), enclosure
    /// inserting pcb at working, raised 5 mm.
    pub(crate) fn boarded() -> (tempfile::TempDir, crate::session::Session) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "pcb"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "pcb", "type": "box", "id": "board",
            "params": {"x": "10 mm", "y": "10 mm", "z": "1.6 mm"}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "pcb", "type": "box", "id": "u1",
            "params": {"x": "2 mm", "y": "2 mm", "z": "2 mm"},
            "placement": {"origin": ["4 mm", "4 mm", "1.6 mm"]}}),
        )
        .unwrap();
        call(&mut s, "part.create", json!({"part": "enclosure"})).unwrap();
        insert_in(dir.path(), "enclosure", "pcb", "pcb", json!("working"));
        s.reload().unwrap();
        (dir, s)
    }

    #[test]
    fn an_insert_founds_each_solid_its_source_shows() {
        let (_dir, mut s) = boarded();
        let r = s.rebuilt(&id("enclosure")).unwrap().clone();
        let o = r.output(&id("pcb")).unwrap();
        let keys: Vec<&str> = o.solids().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["pcb/board", "pcb/u1"]);
        let ins = o.inserted().unwrap();
        assert_eq!(ins.of.as_str(), "pcb");
        // At the identity the source's own bodies are shared.
        let src = s
            .rebuilt(&id("pcb"))
            .unwrap()
            .output(&id("board"))
            .unwrap()
            .solids()[0]
            .1
            .clone();
        assert!(Arc::ptr_eq(&o.solids()[0].1, &src));
        // Names are the source's, byte for byte.
        assert_eq!(
            crate::rebuild::names(&o.solids()[0].1.body),
            crate::rebuild::names(&src.body)
        );
    }

    #[test]
    fn a_placement_moves_the_bodies_and_keeps_the_source_beside_them() {
        let (_dir, mut s) = boarded();
        call(
            &mut s,
            "feature.edit",
            json!({"part": "enclosure", "id": "pcb",
            "placement": {"origin": ["0 mm", "0 mm", "5 mm"]}}),
        )
        .unwrap();
        let r = s.rebuilt(&id("enclosure")).unwrap().clone();
        let (_, b) = &r.output(&id("pcb")).unwrap().solids()[0];
        let m = b.moved.as_ref().unwrap();
        assert_eq!(
            m.source.hash,
            s.rebuilt(&id("pcb"))
                .unwrap()
                .output(&id("board"))
                .unwrap()
                .solids()[0]
                .1
                .hash
        );
        let lo = crate::inspect::loose_extent(&b.body).unwrap().0;
        assert!((lo[2] - 0.005).abs() < 1e-12);
        // The hash is the source's and the pose's: equal inputs, equal bodies.
        assert_eq!(b.hash, inserted_hash(&m.source.hash, &m.pose));
    }

    #[test]
    fn an_insert_refuses_an_assembly_an_empty_source_and_a_missing_one() {
        let (dir, mut s) = boarded();
        call(
            &mut s,
            "part.create",
            json!({"part": "asm", "kind": "assembly"}),
        )
        .unwrap();
        call(&mut s, "part.create", json!({"part": "empty"})).unwrap();
        insert_in(dir.path(), "enclosure", "a", "asm", json!("working"));
        insert_in(dir.path(), "enclosure", "e", "empty", json!("working"));
        insert_in(dir.path(), "enclosure", "n", "nothing", json!("working"));
        s.reload().unwrap();
        let r = s.rebuilt(&id("enclosure")).unwrap().clone();
        let err = |f: &str| {
            r.features
                .iter()
                .find(|(i, _)| i.as_str() == f)
                .unwrap()
                .1
                .clone()
                .unwrap_err()
        };
        assert_eq!(err("a").code, Code::WrongPartKind);
        assert!(
            err("a").details["hint"]
                .as_str()
                .unwrap()
                .contains("--flatten")
        );
        assert_eq!(err("e").code, Code::InsertSourceEmpty);
        assert_eq!(err("n").code, Code::NotFound);
        let _ = dir;
    }

    #[test]
    fn the_model_shows_inserted_solids_and_what_acted_on_them() {
        let (_dir, mut s) = boarded();
        call(
            &mut s,
            "feature.add",
            json!({"part": "enclosure", "type": "box", "id": "lid",
            "params": {"x": "12 mm", "y": "12 mm", "z": "1 mm"},
            "placement": {"origin": ["-1 mm", "-1 mm", "4 mm"]}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "enclosure", "type": "boolean", "id": "b",
            "op": "subtract", "target": "lid", "tools": ["pcb/u1"]}),
        )
        .unwrap();
        let p = s.project().part(&id("enclosure")).unwrap().clone();
        let r = s.rebuilt(&id("enclosure")).unwrap().clone();
        let solids = crate::solid::Solids::of(&p);
        let shown: Vec<(String, String)> = r
            .shown(&solids)
            .iter()
            .map(|x| (x.solid.to_string(), x.tip.to_string()))
            .collect();
        // pcb/u1 was consumed; pcb/board shows as the insert left it.
        assert_eq!(
            shown,
            [
                ("pcb/board".into(), "pcb".into()),
                ("lid".into(), "b".into())
            ]
        );
        let m = crate::assembly::Model::of_part(&p, &crate::doc::assembly::Rev::Working, &r);
        let founders: Vec<&str> = m.occurrences[0]
            .solids
            .iter()
            .map(|x| x.founder.as_str())
            .collect();
        assert_eq!(founders, ["pcb/board", "lid"]);
    }

    #[test]
    fn inserts_are_written_checked_and_default_to_the_newest_label() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "pcb"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "pcb", "type": "box", "id": "board",
            "params": {"x": "10 mm", "y": "10 mm", "z": "1.6 mm"}}),
        )
        .unwrap();
        call(&mut s, "part.create", json!({"part": "enclosure"})).unwrap();
        // No release: working, and the answer says which it took.
        let v = call(
            &mut s,
            "feature.add",
            json!({"part": "enclosure", "type": "insert",
            "id": "board", "of": "pcb", "placement": {"origin": ["0 mm", "0 mm", "2 mm"]}}),
        )
        .unwrap();
        assert_eq!(v["feature"]["rev"], "working");
        assert_eq!(
            v["took"],
            json!({"rev": "working", "why": "no_official_label"})
        );
        let f = s.project().part(&id("enclosure")).unwrap().features[&id("board")]
            .clone()
            .unwrap();
        assert_eq!(f.format(), crate::doc::insert::FORMAT);
        // Refusals at write.
        for (params, code) in [
            (json!({"of": "nothing"}), Code::NotFound),
            (json!({"of": "enclosure"}), Code::InsertCycle),
            (json!({"of": "pcb", "rev": "Z"}), Code::LabelNotFound),
            (json!({"of": "pcb", "solid": "x"}), Code::InvalidParam),
        ] {
            let mut p = params.clone();
            p["part"] = json!("enclosure");
            p["type"] = json!("insert");
            let e = call(&mut s, "feature.add", p).unwrap_err();
            assert_eq!(e.code, code, "{params}");
        }
        call(
            &mut s,
            "part.create",
            json!({"part": "asm", "kind": "assembly"}),
        )
        .unwrap();
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "enclosure", "type": "insert", "of": "asm"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::WrongPartKind);
        // An insert in an assembly is wrong_part_kind too.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "asm", "type": "insert", "of": "pcb"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::WrongPartKind);
        // Edits move the placement and keep the rest.
        let v = call(
            &mut s,
            "feature.edit",
            json!({"part": "enclosure", "id": "board",
            "placement": {"origin": ["0 mm", "0 mm", "3 mm"]}}),
        )
        .unwrap();
        assert_eq!(
            v["feature"]["placement"]["origin"],
            json!(["0 mm", "0 mm", "3 mm"])
        );
        assert_eq!(v["feature"]["rev"], "working");
    }

    #[test]
    fn an_insert_reports_its_source_pin_and_solids_and_inspect_counts_them() {
        let (_dir, mut s) = boarded();
        let r = call(&mut s, "rebuild", json!({"part": "enclosure"})).unwrap();
        let f = &r["parts"][0]["features"][0];
        assert_eq!(f["type"], "insert");
        assert_eq!(f["insert"]["of"], "pcb");
        assert_eq!(f["insert"]["rev"], "working");
        assert!(f["insert"].get("pin").is_none());
        assert_eq!(f["insert"]["count"], 2);
        assert_eq!(f["insert"]["solids"][0]["key"], "pcb/board");
        assert_eq!(f["insert"]["solids"][0]["kind"], "solid");
        assert_eq!(f["insert"]["solids"][0]["reference"], false);
        let list = call(&mut s, "feature.list", json!({"part": "enclosure"})).unwrap();
        assert_eq!(list["features"][0]["of"], "pcb");
        assert_eq!(list["features"][0]["rev"], "working");
        let one = call(
            &mut s,
            "inspect",
            json!({"part": "enclosure", "feature": "pcb"}),
        )
        .unwrap();
        assert_eq!(one["insert"]["solids"].as_array().unwrap().len(), 2);
        // The part's inspect counts them under the insert, not one by one.
        let all = call(&mut s, "inspect", json!({"part": "enclosure"})).unwrap();
        assert_eq!(
            all["inserts"],
            json!([{"feature": "pcb", "of": "pcb", "rev": "working", "solids": 2}])
        );
        assert!(
            all["bodies"]
                .as_array()
                .unwrap()
                .iter()
                .all(|b| !b["feature"].as_str().unwrap().contains('/'))
        );
    }

    /// [`boarded`] in a Git repository: pcb released at A, then changed by
    /// `change` and released at B, both committed; enclosure's insert
    /// pinned to A.
    pub(crate) fn released_with(
        change: impl FnOnce(&mut crate::session::Session),
    ) -> (tempfile::TempDir, crate::session::Session) {
        use crate::git::tests::git;
        let (dir, mut s) = boarded();
        let r = dir.path().to_owned();
        git(&r, &["init", "-q", "-b", "main"]);
        call(
            &mut s,
            "props.set",
            json!({"part": "pcb", "properties": {"number": "100-0001"}}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "pcb"})).unwrap();
        git(&r, &["add", "."]);
        git(&r, &["commit", "-q", "-m", "pcb A"]);
        change(&mut s);
        call(&mut s, "release.propose", json!({"part": "pcb"})).unwrap();
        git(&r, &["add", "."]);
        git(&r, &["commit", "-q", "-m", "pcb B"]);
        s.reload().unwrap();
        call(
            &mut s,
            "feature.edit",
            json!({"part": "enclosure", "id": "pcb", "rev": "A"}),
        )
        .unwrap();
        (dir, s)
    }

    /// The board widened at B.
    pub(crate) fn released_board() -> (tempfile::TempDir, crate::session::Session) {
        released_with(|s| {
            call(
                s,
                "feature.edit",
                json!({"part": "pcb", "id": "board", "params": {"x": "12 mm"}}),
            )
            .unwrap();
        })
    }

    /// The reference text of some edge of shown solid `key` of `part`.
    fn an_edge_of(s: &mut crate::session::Session, part: &str, key: &str) -> String {
        let (insert, _) = key.split_once('/').unwrap();
        let v = call(s, "inspect", json!({"part": part, "feature": insert})).unwrap();
        let body = v["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["solid"] == key)
            .unwrap();
        format!(
            "{part}/{key}/{}",
            body["edges"][0]["name"].as_str().unwrap()
        )
    }

    #[test]
    fn an_update_says_what_moving_the_pin_changes_before_it_writes() {
        let (_dir, mut s) = released_board();
        let rev = |s: &crate::session::Session| {
            s.project().part(&id("enclosure")).unwrap().features[&id("pcb")]
                .as_ref()
                .unwrap()
                .insert()
                .unwrap()
                .rev
                .clone()
        };
        // A dry run writes nothing and answers the change.
        let v = call(
            &mut s,
            "insert.update",
            json!({"part": "enclosure", "feature": "pcb", "dry_run": true}),
        )
        .unwrap();
        assert_eq!(v["from"], json!({"label": "A"}), "{v}");
        assert_eq!(v["to"], json!({"label": "B"}));
        assert_eq!(v["solids"]["changed"], json!(["pcb/board"]));
        assert_eq!(v["solids"]["added"], json!([]));
        assert_eq!(v["failing"], json!([]));
        assert_eq!(v["written"], false);
        assert_eq!(rev(&s), Rev::Label("A".into()));
        // Writing it.
        let v = call(
            &mut s,
            "insert.update",
            json!({"part": "enclosure", "feature": "pcb"}),
        )
        .unwrap();
        assert_eq!(v["written"], true);
        assert_eq!(rev(&s), Rev::Label("B".into()));
        // To working, and to a label that isn't there.
        let v = call(
            &mut s,
            "insert.update",
            json!({"part": "enclosure", "feature": "pcb", "to": "working", "dry_run": true}),
        )
        .unwrap();
        assert_eq!(v["to"], "working");
        let e = call(
            &mut s,
            "insert.update",
            json!({"part": "enclosure", "feature": "pcb", "to": "label:Z"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound, "{e:?}");
        let e = call(
            &mut s,
            "insert.update",
            json!({"part": "enclosure", "feature": "nope"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    #[test]
    fn an_update_that_breaks_a_reader_names_it_with_candidates() {
        let (_dir, mut s) = released_with(|s| {
            call(s, "feature.delete", json!({"part": "pcb", "id": "u1"})).unwrap();
        });
        let edge = an_edge_of(&mut s, "enclosure", "pcb/u1");
        call(
            &mut s,
            "feature.add",
            json!({"part": "enclosure", "type": "fillet", "id": "f",
            "groups": [{"radius": "0.2 mm", "edges": [edge]}]}),
        )
        .unwrap();
        let v = call(
            &mut s,
            "insert.update",
            json!({"part": "enclosure", "feature": "pcb", "dry_run": true}),
        )
        .unwrap();
        assert_eq!(v["solids"]["removed"], json!(["pcb/u1"]), "{v}");
        assert_eq!(v["failing"][0]["feature"], "f", "{v}");
        assert_eq!(v["failing"][0]["error"]["code"], "not_found", "{v}");
        assert!(
            v["failing"][0]["error"]["details"].get("solids").is_some(),
            "{v}"
        );
    }
}
