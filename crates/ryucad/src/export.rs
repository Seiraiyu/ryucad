//! Export: a part or an assembly, at the working files, a Git revision or
//! a release label, as a STEP file.
//!
//! A part is one product of its solids as shown, in model order (consumed
//! solids aren't); an assembly is a product placing each instance's
//! product at its solved pose, and each instance pattern's copies at
//! theirs, subassemblies products of their own; a part placed at two
//! revisions is two products. A product's identifier is its PDM number,
//! else the one product its import features (or, for an assembly, its
//! imported instances) all name, else its part ID; its name the `name`
//! property, else that product's name, else its ID; its description the
//! `description` property; its revision the release label it is exported
//! at. An imported assembly's instance of its own bodies (an import with
//! no usage, at the origin) writes them as the assembly's own bodies, and
//! a usage's identifier is the file's where an import made the instance,
//! else the instance's ID: a file exported as it was imported comes back
//! with its products and usages, so the import matches the export.
//!
//! The bytes are the same for the same model and options: the header's
//! file name and time stamp are the caller's, the time stamp else the
//! commit's time when exporting a commit (a Git revision, or a release
//! where it became official), else now (UTC, seconds). Exports are built
//! from the session's results each time and never cached.

use crate::assembly::{Model, Pin};
use crate::doc::assembly::{Pose, Rev};
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::import::ProductRef;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::proto::Answer;
use crate::rebuild::{Output, PartResult};
use crate::session::Session;
use ryukei::topo::Body;
use ryupdm::props::Properties;
use ryustep::schema::Schema;
use ryustep::{Child, LengthUnit, Product, WriteError, WriteModel, WriteOptions};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// What `export` asks for: the part, the revision (`working`, a Git
/// revision, or `label:<L>`), `ap242` or `ap214`, `mm` or `m`, and the
/// header's time stamp and file name.
#[derive(Clone, Debug, Default)]
pub struct Ask {
    pub part: String,
    pub rev: Option<String>,
    pub schema: Option<String>,
    pub unit: Option<String>,
    pub time_stamp: Option<String>,
    pub file_name: Option<String>,
}

/// Writes `ask.part` as a STEP file: the answer, with the file's bytes.
/// `export_failed` when a feature of anything it places fails,
/// `export_unrepresentable` for a face STEP can't describe.
pub fn export(session: &mut Session, ask: Ask) -> Result<Answer> {
    let (schema, schema_name) = match ask.schema.as_deref().unwrap_or("ap242") {
        "ap242" => (Schema::Ap242, "ap242"),
        "ap214" => (Schema::Ap214, "ap214"),
        other => return Err(invalid("schema", other, "ap242 or ap214")),
    };
    let (unit, unit_name) = match ask.unit.as_deref().unwrap_or("mm") {
        "mm" => (LengthUnit::Millimeter, "mm"),
        "m" => (LengthUnit::Meter, "m"),
        other => return Err(invalid("unit", other, "mm or m")),
    };
    let part = crate::server::part_id(&ask.part)?;
    let (label, git) = match ask.rev.as_deref() {
        Some(r) if r.starts_with("label:") => {
            let l = &r["label:".len()..];
            let rev = Rev::from_json(&json!({"label": l}), "/rev")?;
            (Some(rev), None)
        }
        r => (None, r),
    };
    let (commit, s) = session.at(git)?;
    let rev = label.unwrap_or(Rev::Working);
    if rev == Rev::Working {
        s.project().part(&part)?;
    }
    let mut walk = Walk {
        built: s.nodes(&part, &rev)?,
        ..Walk::default()
    };
    walk.product(s, &part, &rev)?;
    if !walk.failing.is_empty() {
        let failing: Vec<&Value> = walk.failing.values().collect();
        return Err(Error::new(
            Code::ExportFailed,
            format!(
                "{part} can't be exported: {} feature{} of what it places fail{}",
                failing.len(),
                if failing.len() == 1 { "" } else { "s" },
                if failing.len() == 1 { "s" } else { "" },
            ),
        )
        .with("part", &part)
        .with("failing", failing));
    }
    // The commit the model is read from: the revision's, or where the
    // release became official.
    let at = commit
        .clone()
        .or_else(|| walk.pin.as_ref().and_then(|p| p.commit.clone()));
    let time_stamp = match (ask.time_stamp, &at) {
        (Some(t), _) => t,
        (None, Some(c)) => utc(crate::git::repo(s.root())?.commit_time(c)?),
        (None, None) => utc(now()),
    };
    let opts = WriteOptions {
        schema,
        unit,
        file_name: ask.file_name.unwrap_or_default(),
        time_stamp,
        ..WriteOptions::default()
    };
    let bytes = ryustep::write(&walk.model, &opts).map_err(|e| walk.unwritable(e))?;
    let products: Vec<Value> = walk
        .model
        .products
        .iter()
        .zip(&walk.nodes)
        .map(|(p, (part, rev))| {
            let mut v = json!({
                "product": p.id,
                "name": p.name,
                "part": part,
                "rev": rev.to_json(),
                "bodies": p.bodies.len(),
                "children": p.children.len(),
            });
            if !p.revision.is_empty() {
                v["revision"] = json!(p.revision);
            }
            v
        })
        .collect();
    let mut result = json!({
        "part": part,
        "rev": ask.rev.as_deref().unwrap_or("working"),
        "schema": schema_name,
        "unit": unit_name,
        "file_name": opts.file_name,
        "time_stamp": opts.time_stamp,
        "products": products,
    });
    if let Some(c) = at {
        result["commit"] = json!(c);
    }
    Ok(Answer::bytes(result, "step", bytes))
}

fn invalid(param: &str, value: &str, expected: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("{param} {value:?} isn't one STEP export knows: expected {expected}"),
    )
    .with("param", param)
}

/// The model being written, walked from the part exported.
#[derive(Default)]
struct Walk {
    model: WriteModel,
    /// Each product's part and revision, by index.
    nodes: Vec<(Id, Rev)>,
    /// Each product's index, by part and revision.
    index: BTreeMap<(Id, Rev), usize>,
    /// Each product's bodies' parts and solids, for refusals.
    owners: Vec<Vec<(Id, Id)>>,
    /// Failing features, by part, revision and feature.
    failing: BTreeMap<(Id, String, Id), Value>,
    /// How the part exported resolved, at a release label.
    pin: Option<Pin>,
    /// Every part and revision the export reaches, rebuilt at once
    /// ([`Session::nodes`]).
    built: BTreeMap<(Id, Rev), (Part, PartResult, Option<Pin>)>,
}

impl Walk {
    /// `part` at `rev` rebuilt, its failing features noted.
    fn node(&mut self, s: &mut Session, part: &Id, rev: &Rev) -> Result<(Part, PartResult)> {
        let at = (part.clone(), rev.clone());
        let (p, result, pin) = match self.built.get(&at) {
            Some(n) => n.clone(),
            None => s.node(part, rev)?,
        };
        if self.nodes.is_empty() && self.pin.is_none() {
            self.pin = pin;
        }
        for (fid, r) in &result.features {
            if let Err(e) = r {
                self.failing
                    .entry((part.clone(), rev.text(), fid.clone()))
                    .or_insert_with(|| {
                        json!({
                            "part": part,
                            "rev": rev.to_json(),
                            "feature": fid,
                            "code": e.code,
                            "message": e.message,
                        })
                    });
            }
        }
        Ok((p, result))
    }

    /// The index of `part` at `rev`'s product, written first if it isn't
    /// yet, with the products it places.
    fn product(&mut self, s: &mut Session, part: &Id, rev: &Rev) -> Result<usize> {
        if let Some(&k) = self.index.get(&(part.clone(), rev.clone())) {
            return Ok(k);
        }
        let (p, result) = self.node(s, part, rev)?;
        let k = self.model.products.len();
        self.index.insert((part.clone(), rev.clone()), k);
        self.nodes.push((part.clone(), rev.clone()));
        self.model.products.push(Product::default());
        self.owners.push(Vec::new());
        let props = properties(s, part, rev, self.pin.as_ref().filter(|_| k == 0));
        let (id, name, description) = identity(part, &p, &props);
        let mut bodies = Vec::new();
        let mut owners = Vec::new();
        let mut children = Vec::new();
        if p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly) {
            for (fid, r) in &result.features {
                match r {
                    Ok(Output::Instance(placed)) => {
                        let import = match p.features.get(fid) {
                            Some(Ok(Feature::Instance(i))) => i.import.clone(),
                            _ => None,
                        };
                        let own = import.as_ref().is_some_and(|u| u.usage.is_none())
                            && crate::assembly::rigid_pose(&placed.pose) == Pose::IDENTITY;
                        if own {
                            let (cp, cr) = self.node(s, &placed.part, &placed.rev)?;
                            if !cp.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly) {
                                let (b, o) = solids(&cp, &placed.rev, &cr);
                                bodies.extend(b);
                                owners.extend(o);
                                continue;
                            }
                        }
                        let product = self.product(s, &placed.part, &placed.rev)?;
                        let id = import
                            .and_then(|u| u.usage)
                            .unwrap_or_else(|| fid.to_string());
                        children.push(Child {
                            product,
                            place: placed.pose,
                            id,
                        });
                    }
                    Ok(Output::Copies(c)) => {
                        for copy in &c.copies {
                            let product = self.product(s, &copy.placed.part, &copy.placed.rev)?;
                            children.push(Child {
                                product,
                                place: copy.placed.pose,
                                id: Id::copy(fid, copy.index).to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
        } else {
            let (b, o) = solids(&p, rev, &result);
            bodies = b;
            owners = o;
        }
        let revision = match rev {
            Rev::Label(l) => l.clone(),
            Rev::Working => String::new(),
        };
        self.model.products[k] = Product {
            id,
            name,
            description,
            revision,
            bodies,
            children,
        };
        self.owners[k] = owners;
        Ok(k)
    }

    /// `export_unrepresentable` for a face ryustep couldn't write, naming
    /// its part and solid.
    fn unwritable(&self, e: WriteError) -> Error {
        let WriteError::NotRepresentable { face } = e else {
            return Error::new(Code::Internal, format!("writing STEP: {e}"));
        };
        let mut err = Error::new(
            Code::ExportUnrepresentable,
            format!(
                "face {face} can't be described in STEP: its loops can't be cut open along seams"
            ),
        )
        .with("face", face.to_string());
        let owner = self
            .model
            .products
            .iter()
            .zip(&self.owners)
            .flat_map(|(p, o)| p.bodies.iter().zip(o))
            .find(|(b, _)| has_face(b, face));
        if let Some((_, (part, solid))) = owner {
            err = err
                .with("part", part)
                .with("solid", solid)
                .with("entity", format!("{part}/{solid}/{face}"));
        }
        err
    }
}

fn has_face(b: &Body, name: ryukei::name::Name) -> bool {
    b.faces().any(|f| b.face(f).expect("live").name == name)
}

/// A part's solids as it shows them, in model order, with their parts and
/// founders.
fn solids(p: &Part, rev: &Rev, result: &PartResult) -> (Vec<Body>, Vec<(Id, Id)>) {
    let m = Model::of_part(p, rev, result);
    m.occurrences
        .into_iter()
        .flat_map(|o| o.solids)
        .map(|s| (s.built.body.clone(), (p.id.clone(), s.founder)))
        .unzip()
}

/// The product an imported part or assembly came from: the one product
/// every import feature of a part names, or every imported instance of
/// an assembly is used in.
fn imported(p: &Part) -> Option<ProductRef> {
    let mut named: BTreeSet<Option<ProductRef>> = BTreeSet::new();
    for fid in p.order() {
        match p.features.get(fid) {
            Some(Ok(Feature::Import(i))) => {
                named.insert(i.product.clone());
            }
            Some(Ok(Feature::Instance(i))) => {
                if let Some(u) = &i.import {
                    named.insert(Some(u.product.clone()));
                }
            }
            _ => {}
        }
    }
    match named.len() {
        1 => named.into_iter().next().flatten(),
        _ => None,
    }
}

/// A product's identifier, name and description: the PDM number, name
/// and description, else the imported product's identifier and name, else
/// the part's ID.
fn identity(part: &Id, p: &Part, props: &Properties) -> (String, String, String) {
    let text = |key: &str| {
        props
            .values
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned)
    };
    let from = imported(p);
    let id = text("number")
        .or_else(|| from.as_ref().map(|f| f.id.clone()))
        .unwrap_or_else(|| part.to_string());
    let name = text("name")
        .or_else(|| from.as_ref().map(|f| f.name.clone()))
        .unwrap_or_else(|| part.to_string());
    (id, name, text("description").unwrap_or_default())
}

/// `part`'s properties at `rev`: the working files', or for an official
/// release (`pin`, for the part exported; else looked up) its commit's.
fn properties(s: &mut Session, part: &Id, rev: &Rev, pin: Option<&Pin>) -> Properties {
    let commit = match (rev, pin) {
        (Rev::Working, _) => None,
        (Rev::Label(_), Some(p)) => p.commit.clone(),
        (Rev::Label(l), None) => crate::release::pinned(s, part, l)
            .ok()
            .and_then(|p| p.pin.commit),
    };
    let read = match commit {
        Some(c) => crate::git::repo(s.root())
            .and_then(|r| r.files(&c))
            .ok()
            .and_then(|files| ryupdm::props::read(&files, part).ok()),
        None => ryupdm::props::read(&*s, part).ok(),
    };
    read.unwrap_or_default()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `secs` since the Unix epoch as an ISO 8601 date and time in UTC, to the
/// second (Hinnant's civil-from-days).
pub fn utc(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::assembly::tests::{place_in, project, released};
    use crate::mate::tests::add_in;
    use crate::server::tests::{call, fresh};

    /// The answer to an export with `params`, and the file's bytes.
    pub(crate) fn exported(s: &mut Session, params: Value) -> Result<(Value, Vec<u8>)> {
        let req = crate::proto::Request::from_rpc("export", Some(params))?;
        let a = crate::server::answer_live(s, None, None, req)?;
        Ok((a.result, a.binary.expect("an export answers with bytes")))
    }

    fn opened(bytes: &[u8]) -> ryustep::Opened {
        ryustep::open(bytes).unwrap()
    }

    /// The header's time stamp.
    fn time_stamp(bytes: &[u8]) -> String {
        let o = opened(bytes);
        let name = o.header.entities.iter().find(|e| &*e.name == "FILE_NAME");
        match &name.unwrap().params[1] {
            ryustep::part21::Param::Str(t) => t.to_string(),
            p => panic!("{p:?}"),
        }
    }

    fn add(s: &mut Session, part: &str, f: Value) {
        let mut f = f;
        f["part"] = json!(part);
        call(s, "feature.add", f).unwrap();
    }

    #[test]
    fn times_are_utc_to_the_second() {
        assert_eq!(utc(0), "1970-01-01T00:00:00");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00");
        assert_eq!(utc(1_761_573_470), "2025-10-27T13:57:50");
        assert_eq!(utc(-1), "1969-12-31T23:59:59");
    }

    #[test]
    fn a_part_is_one_product_of_the_solids_it_shows() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        for f in [
            json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
            json!({"type": "box", "id": "lug", "params": {"x": 4, "y": 4, "z": 4},
                   "placement": {"origin": [10, 3, 0]}}),
            json!({"type": "boolean", "id": "u", "op": "unite", "target": "a", "tools": ["lug"]}),
            json!({"type": "sphere", "id": "ball", "params": {"radius": 3},
                   "placement": {"origin": [30, 0, 0]}}),
        ] {
            add(&mut s, "p", f);
        }
        let (v, bytes) = exported(
            &mut s,
            json!({"part": "p", "time_stamp": "2026-10-27T00:00:00", "file_name": "p.step"}),
        )
        .unwrap();
        assert_eq!(
            (&v["schema"], &v["unit"], &v["rev"], &v["file_name"]),
            (
                &json!("ap242"),
                &json!("mm"),
                &json!("working"),
                &json!("p.step")
            )
        );
        assert_eq!(
            v["products"],
            json!([{"product": "p", "name": "p", "part": "p", "rev": "working",
                    "bodies": 2, "children": 0}])
        );
        assert_eq!(time_stamp(&bytes), "2026-10-27T00:00:00");
        // The united block and the ball, read back as two bodies of p; the
        // lug the boolean consumed isn't one.
        let opts = ryustep::ReadOptions {
            key: ryukei::name::OpKey(1),
            heal: Default::default(),
        };
        let read = ryustep::read(&bytes, &opts).unwrap();
        assert_eq!(read.bodies.len(), 2);
        assert!(read.bodies.iter().all(|b| b.result.is_ok()));
        let d = &read.products.definitions[0];
        assert_eq!((&*d.product_id, &*d.name, &*d.revision), ("p", "p", ""));
        // The same model and options, the same bytes.
        let (_, again) = exported(
            &mut s,
            json!({"part": "p", "time_stamp": "2026-10-27T00:00:00", "file_name": "p.step"}),
        )
        .unwrap();
        assert_eq!(again, bytes);
    }

    #[test]
    fn pdm_names_the_product() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        add(
            &mut s,
            "p",
            json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
        );
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {
                "number": "PN-7", "name": "Spacer", "description": "A spacer block"}}),
        )
        .unwrap();
        let (v, bytes) = exported(&mut s, json!({"part": "p", "unit": "m"})).unwrap();
        assert_eq!(v["products"][0]["product"], "PN-7");
        let d = &opened(&bytes).products.definitions[0];
        assert_eq!(
            (&*d.product_id, &*d.name, &*d.description),
            ("PN-7", "Spacer", "A spacer block")
        );
    }

    #[test]
    fn an_assembly_places_its_parts_where_it_solved_them() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(
            r,
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.0, 0.005],
        );
        add_in(
            r,
            "frame",
            "row",
            "instance_pattern",
            json!({"form": "linear", "instances": ["b1"], "direction": "x",
                   "spacing": "20 mm", "count": 3}),
        );
        s.refresh().unwrap();
        let (v, bytes) = exported(&mut s, json!({"part": "frame", "unit": "m"})).unwrap();
        let made: Vec<(&str, u64, u64)> = v["products"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["product"].as_str().unwrap(),
                    p["bodies"].as_u64().unwrap(),
                    p["children"].as_u64().unwrap(),
                )
            })
            .collect();
        // The bolt is one product, placed three times.
        assert_eq!(made, [("frame", 0, 4), ("plate", 1, 0), ("bolt", 1, 0)]);
        let o = opened(&bytes);
        let p = &o.products;
        assert_eq!(p.roots.len(), 1);
        let root = p.roots[0];
        let ids: Vec<&str> = p.definitions[root]
            .children
            .iter()
            .map(|c| &*c.id)
            .collect();
        assert_eq!(ids, ["base", "b1", "row:1", "row:2"]);
        // Each where the rebuild put it, bit for bit in meters.
        let frame = s.model(&Id::parse("frame", "part").unwrap(), &Rev::Working);
        let frame = frame.unwrap();
        for (n, at) in [
            (1, [0.01, 0.0, 0.005]),
            (2, [0.03, 0.0, 0.005]),
            (3, [0.05, 0.0, 0.005]),
        ] {
            let t = o.placement(root, n).unwrap().translation_part();
            assert_eq!([t.x, t.y, t.z], at, "{n}");
        }
        assert_eq!(frame.occurrences.len(), 4);
    }

    #[test]
    fn releases_and_revisions_export_as_their_commits_have_them() {
        let (dir, mut s) = released();
        let r = dir.path();
        // HEAD holds release A of the bolt; the working bolt is thicker.
        let (v, at_label) = exported(&mut s, json!({"part": "bolt", "rev": "label:A"})).unwrap();
        let commit = v["commit"].as_str().unwrap().to_owned();
        assert_eq!(v["products"][0]["revision"], "A");
        let d = &opened(&at_label).products.definitions[0];
        assert_eq!((&*d.product_id, &*d.revision), ("100", "A"));
        // The time stamp is the commit's.
        let repo = crate::git::repo(r).unwrap();
        let when = utc(repo.commit_time(&commit).unwrap());
        assert_eq!(time_stamp(&at_label), when);
        // HEAD's bolt is release A's, with no revision written.
        let (v, at_head) = exported(&mut s, json!({"part": "bolt", "rev": "HEAD"})).unwrap();
        assert_eq!(v["commit"], json!(commit));
        let text = String::from_utf8(at_label.clone()).unwrap();
        let unlabelled = text.replace(
            "PRODUCT_DEFINITION_FORMATION('A'",
            "PRODUCT_DEFINITION_FORMATION(''",
        );
        assert_eq!(unlabelled.as_bytes(), at_head);
        let (_, working) = exported(&mut s, json!({"part": "bolt", "time_stamp": when})).unwrap();
        assert_ne!(working, at_head);
        // An assembly placing release A writes that product with its
        // revision, beside the working bolt: two products.
        place_in(r, "frame", "a", "bolt", json!({"label": "A"}), [0.0; 3]);
        place_in(r, "frame", "w", "bolt", json!("working"), [0.02, 0.0, 0.0]);
        s.refresh().unwrap();
        let (v, _) = exported(&mut s, json!({"part": "frame"})).unwrap();
        let revs: Vec<(&Value, &Value)> = v["products"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (&p["product"], &p["rev"]))
            .collect();
        assert_eq!(
            revs,
            [
                (&json!("frame"), &json!("working")),
                (&json!("100"), &json!({"label": "A"})),
                (&json!("100"), &json!("working")),
            ]
        );
        let e = exported(&mut s, json!({"part": "bolt", "rev": "label:B"})).unwrap_err();
        assert_eq!(e.code, Code::LabelNotFound);
    }

    #[test]
    fn what_fails_anywhere_refuses_the_export() {
        let (dir, mut s) = project();
        add(
            &mut s,
            "bolt",
            json!({"type": "box", "id": "miss", "operation": "cut", "targets": ["shank"],
                   "params": {"x": 1, "y": 1, "z": 1}, "placement": {"origin": [100, 100, 100]}}),
        );
        let e = exported(&mut s, json!({"part": "bolt"})).unwrap_err();
        assert_eq!(e.code, Code::ExportFailed);
        assert_eq!(
            e.details["failing"],
            json!([{"part": "bolt", "rev": "working", "feature": "miss",
                    "code": "operation_no_effect",
                    "message": e.details["failing"][0]["message"]}])
        );
        place_in(dir.path(), "frame", "b", "bolt", json!("working"), [0.0; 3]);
        s.refresh().unwrap();
        let e = exported(&mut s, json!({"part": "frame"})).unwrap_err();
        assert_eq!(e.code, Code::ExportFailed);
        assert_eq!(e.details["failing"][0]["feature"], "miss");
    }

    #[test]
    fn options_and_parts_are_checked() {
        let (_dir, mut s) = project();
        for (params, code, param) in [
            (
                json!({"part": "bolt", "schema": "ap203"}),
                Code::InvalidParam,
                "schema",
            ),
            (
                json!({"part": "bolt", "unit": "in"}),
                Code::InvalidParam,
                "unit",
            ),
        ] {
            let e = exported(&mut s, params).unwrap_err();
            assert_eq!((e.code, &e.details["param"]), (code, &json!(param)));
        }
        let e = exported(&mut s, json!({"part": "nut"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = exported(&mut s, json!({"part": "bolt", "rev": "HEAD"})).unwrap_err();
        assert_eq!(e.code, Code::NotAGitRepo);
        let (v, bytes) = exported(&mut s, json!({"part": "bolt", "schema": "ap214"})).unwrap();
        assert_eq!(v["schema"], "ap214");
        assert_eq!(opened(&bytes).schema, Schema::Ap214);
    }

    /// A frame (`F-1`) with a plate of its own, a sub-assembly (`S-1`) of
    /// two pins (`P-1`) and one pin more, every usage `1`: one root.
    fn frame() -> Vec<u8> {
        use ryukei::geom::{Rigid, Vector3};
        use ryukei::name::OpKey;
        use ryukei::primitive;
        let at = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
        let k = OpKey(1);
        let plate = primitive::block(k, 0.1, 0.1, 0.01, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let pin = primitive::cylinder(k, 0.002, 0.02, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let product = |id: &str, name: &str, bodies, children| Product {
            id: id.into(),
            name: name.into(),
            bodies,
            children,
            ..Default::default()
        };
        let child = |product, place| Child {
            product,
            place,
            id: "1".into(),
        };
        crate::import::tests::written(vec![
            product(
                "F-1",
                "Frame",
                vec![plate],
                vec![child(1, at(0.0, 0.0, 0.01)), child(2, at(0.05, 0.05, 0.01))],
            ),
            product(
                "S-1",
                "Sub",
                vec![],
                vec![child(2, at(0.01, 0.0, 0.0)), child(2, at(0.02, 0.0, 0.0))],
            ),
            product("P-1", "Pin", vec![pin], vec![]),
        ])
    }

    /// `bytes` written to `name` outside any project.
    fn outside(bytes: &[u8], name: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        (dir, path.to_string_lossy().into_owned())
    }

    /// Each definition's product, own bodies, and usages (id, child's
    /// product, the placement's bits), in definition order.
    #[allow(clippy::type_complexity)]
    fn tree(bytes: &[u8]) -> Vec<(String, usize, Vec<(String, String, Vec<u64>)>)> {
        let o = opened(bytes);
        let defs = &o.products.definitions;
        (0..defs.len())
            .map(|k| {
                let d = &defs[k];
                let bodies = o.held.iter().filter(|h| h.definition == Some(k)).count();
                let usages = d
                    .children
                    .iter()
                    .enumerate()
                    .map(|(n, c)| {
                        let r = o.placement(k, n).unwrap();
                        let t = r.translation_part();
                        let bits = r
                            .rotation_matrix()
                            .iter()
                            .flatten()
                            .chain([t.x, t.y, t.z].iter())
                            .map(|x| x.to_bits())
                            .collect();
                        (
                            c.id.to_string(),
                            defs[c.definition].product_id.to_string(),
                            bits,
                        )
                    })
                    .collect();
                (format!("{} {}", d.product_id, d.name), bodies, usages)
            })
            .collect()
    }

    /// Every face, edge and vertex name of every part's solids, by part.
    fn names(s: &mut Session) -> BTreeMap<String, BTreeSet<String>> {
        let parts: Vec<Id> = s.project().parts.keys().cloned().collect();
        let mut out = BTreeMap::new();
        for p in parts {
            let v = call(s, "inspect", json!({"part": p})).unwrap();
            let mut set = BTreeSet::new();
            for b in v["bodies"].as_array().into_iter().flatten() {
                for kind in ["faces", "edges", "vertices"] {
                    for e in b[kind].as_array().into_iter().flatten() {
                        set.insert(e["name"].as_str().unwrap().to_owned());
                    }
                }
            }
            out.insert(p.to_string(), set);
        }
        out
    }

    #[test]
    fn an_imported_file_exports_as_it_came() {
        let original = frame();
        let (_src, path) = outside(&original, "frame.step");
        let (dir, mut s) = fresh();
        call(&mut s, "import", json!({"path": path})).unwrap();
        // f-1 places its plate's part (f-1-body), s-1 and p-1. Exported
        // with the header the file had, it is the file, byte for byte.
        let export = |s: &mut Session, time_stamp: &str| {
            let params = json!({"part": "f-1", "unit": "m", "time_stamp": time_stamp,
                                "file_name": "frame.step"});
            exported(s, params).unwrap().1
        };
        assert_eq!(export(&mut s, "2026-10-27T00:00:00"), original);
        // At another time, the same products, bodies and usages, each
        // where it was: the plate is the frame's own body again, not a part
        // placed in it.
        let bytes = export(&mut s, "2026-10-28T00:00:00");
        assert_eq!(tree(&bytes), tree(&original));
        // Updating the import with the export changes nothing but the
        // file's hash, and renames nothing.
        let before = crate::session::files(dir.path()).unwrap();
        let read = |p: &std::path::Path| std::fs::read(p).unwrap();
        let old: BTreeMap<_, _> = before.iter().map(|p| (p.clone(), read(p))).collect();
        let names_before = names(&mut s);
        let (_out, out) = outside(&bytes, "export.step");
        let v = call(
            &mut s,
            "import",
            json!({"path": out, "update": "imports/frame.step"}),
        )
        .unwrap();
        assert_ne!(v["file"]["blake3"], v["file"]["from"]);
        for key in ["vanished", "kept", "failing"] {
            assert!(v.get(key).is_none(), "{key}: {v}");
        }
        let after = crate::session::files(dir.path()).unwrap();
        assert_eq!(after, before);
        for p in &after {
            let (a, b) = (&old[p], read(p));
            if *a == b || p.ends_with("frame.step") {
                continue;
            }
            // An import feature, only its file's hash new.
            let mut a: Value = serde_json::from_slice(a).unwrap();
            let b: Value = serde_json::from_slice(&b).unwrap();
            assert_eq!(a["type"], "import", "{}", p.display());
            a["file"]["blake3"] = b["file"]["blake3"].clone();
            assert_eq!(a, b, "{}", p.display());
        }
        assert_eq!(names(&mut s), names_before);
        // Imported into another project, it makes the same parts.
        let (_dir2, mut s2) = fresh();
        let v = call(&mut s2, "import", json!({"path": out})).unwrap();
        let made: Vec<(&Value, &Value)> = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (&p["part"], &p["kind"]))
            .collect();
        assert_eq!(
            made,
            [
                (&json!("f-1"), &json!("assembly")),
                (&json!("f-1-body"), &json!("part")),
                (&json!("s-1"), &json!("assembly")),
                (&json!("p-1"), &json!("part")),
            ]
        );
    }

    #[test]
    fn a_face_step_cant_describe_is_named() {
        let (_dir, mut s) = project();
        let bolt = Id::parse("bolt", "part").unwrap();
        let mut walk = Walk::default();
        walk.product(&mut s, &bolt, &Rev::Working).unwrap();
        let body = &walk.model.products[0].bodies[0];
        let face = body.face(body.faces().next().unwrap()).unwrap().name;
        let e = walk.unwritable(WriteError::NotRepresentable { face });
        assert_eq!(e.code, Code::ExportUnrepresentable);
        assert_eq!(e.details["entity"], format!("bolt/shank/{face}"));
    }
}
