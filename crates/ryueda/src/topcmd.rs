//! Requests on what holds connectivity: blocks (their names, ports and
//! buses), rails, net classes and designs. Each write is one logged,
//! undoable change: a block's guarded by its revision, the rest by the
//! project's.

use crate::conn::{
    self, BLOCKS, Block, Bus, CLASSES, Class, DESIGNS, Design, Direction, Node, Port, RAILS, Rail,
};
use crate::design::{BlockFiles, Part, block_path, part_path, top_path};
use crate::error::{Code, Error, Result};
use crate::fault;
use crate::id::Id;
use crate::library::Loaded;
use crate::project::Project;
use crate::server::check_revision;
use crate::session::Session;
use crate::write::{self, Writes, commit, has_dependents, new_id, set_opt, users};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A net's or a rail's intent, as a request gives it: quantities as
/// text, a class by ID or name. An empty text clears a field.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<String>,
}

/// The class a request names, by ID or name; empty for none.
pub fn class_ref(p: &Project, text: &str) -> Result<Option<Id>> {
    if text.is_empty() {
        return Ok(None);
    }
    p.conn.class(text).cloned().map(Some).ok_or_else(|| {
        Error::new(Code::NotFound, format!("there is no class {text}")).with("class", text)
    })
}

/// Writes `args` over a net's or rail's intent fields.
pub fn fill_intent(
    p: &Project,
    args: &IntentArgs,
    class: &mut Option<Id>,
    fields: [&mut Option<String>; 3],
) -> Result<()> {
    if let Some(c) = &args.class {
        *class = class_ref(p, c).map_err(|e| e.with("param", "class"))?;
    }
    let [v, c, z] = fields;
    set_opt(v, &args.voltage);
    set_opt(c, &args.current);
    set_opt(z, &args.impedance);
    Ok(())
}

fn loaded_json<T: Serialize>(l: &Loaded<T>, key: &str) -> Value {
    let mut v = json!({"faults": fault::list(&l.faults)});
    match &l.item {
        Ok(t) => v[key] = json!(t),
        Err(e) => v["error"] = json!(e),
    }
    v
}

/// A project-level object's answer: its reference, file, contents,
/// faults and the project's revision.
fn top_json<T: Serialize>(
    p: &Project,
    folder: &str,
    key: &str,
    map: &BTreeMap<Id, Loaded<T>>,
    id: &Id,
) -> Value {
    let mut v = map
        .get(id)
        .map_or_else(|| json!({}), |l| loaded_json(l, key));
    v["reference"] = json!(format!("{key}/{id}"));
    v["path"] = json!(top_path(folder, id));
    v["revision"] = json!(p.revision);
    v
}

fn top_list<T>(
    p: &Project,
    key: &str,
    plural: &str,
    map: &BTreeMap<Id, Loaded<T>>,
    name: impl Fn(&T) -> &str,
) -> Value {
    let list: Vec<Value> = map
        .iter()
        .map(|(id, l)| {
            let mut v =
                json!({"id": id, "reference": format!("{key}/{id}"), "faults": l.faults.len()});
            match &l.item {
                Ok(t) => v["name"] = json!(name(t)),
                Err(e) => v["error"] = json!(e),
            }
            v
        })
        .collect();
    json!({plural: list, "revision": p.revision})
}

/// The object a request names among `map`, by ID or name, as read.
fn top_get<'a, T: Clone>(
    map: &'a BTreeMap<Id, Loaded<T>>,
    found: Option<&'a Id>,
    what: &str,
    text: &str,
) -> Result<(Id, T)> {
    let id = found.ok_or_else(|| {
        Error::new(Code::NotFound, format!("there is no {what} {text}")).with(what, text)
    })?;
    let item = map[id].item.clone().map_err(|e| e.with(what, text))?;
    Ok((id.clone(), item))
}

fn named<'a, T>(
    map: &'a BTreeMap<Id, Loaded<T>>,
    text: &str,
    name: impl Fn(&T) -> &str,
) -> Option<&'a Id> {
    map.keys().find(|id| id.as_str() == text).or_else(|| {
        let mut hits = map
            .iter()
            .filter(|(_, l)| l.item.as_ref().is_ok_and(|t| name(t) == text));
        hits.next()
            .map(|(id, _)| id)
            .filter(|_| hits.next().is_none())
    })
}

/// `rail.add`.
pub fn rail_add(
    session: &mut Session,
    name: &str,
    id: Option<&str>,
    intent: &IntentArgs,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let id = new_id(id, "rail", |s| p.conn.has_rail(s))?;
    let mut r = Rail {
        name: name.to_owned(),
        class: None,
        voltage: None,
        current: None,
        impedance: None,
    };
    fill_intent(
        p,
        intent,
        &mut r.class,
        [&mut r.voltage, &mut r.current, &mut r.impedance],
    )?;
    let mut w = Writes::default();
    w.put(top_path(RAILS, &id), &r, conn::RAIL_KIND);
    commit(session, &format!("rail {name}"), w)?;
    let p = session.project();
    Ok(top_json(p, RAILS, "rail", &p.conn.rails, &id))
}

/// `rail.edit`: a new name, class or intent.
pub fn rail_edit(
    session: &mut Session,
    rail: &str,
    name: Option<&str>,
    intent: &IntentArgs,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let (id, mut r) = top_get(&p.conn.rails, p.conn.rail(rail), "rail", rail)?;
    if let Some(n) = name {
        r.name = n.to_owned();
    }
    fill_intent(
        p,
        intent,
        &mut r.class,
        [&mut r.voltage, &mut r.current, &mut r.impedance],
    )?;
    let mut w = Writes::default();
    w.put(top_path(RAILS, &id), &r, conn::RAIL_KIND);
    commit(session, &format!("rail {id}"), w)?;
    let p = session.project();
    Ok(top_json(p, RAILS, "rail", &p.conn.rails, &id))
}

/// `rail.delete`: refused while a pin or an instance's port names it.
pub fn rail_delete(session: &mut Session, rail: &str, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let (id, _) = top_get(&p.conn.rails, p.conn.rail(rail), "rail", rail)?;
    let using = users(p, &Node::Rail(id.clone()), None);
    if !using.is_empty() {
        return Err(has_dependents(&format!("rail {id}"), &using));
    }
    let mut w = Writes::default();
    w.delete(top_path(RAILS, &id));
    commit(session, &format!("rail {id}"), w)?;
    Ok(json!({"deleted": format!("rail/{id}"), "revision": session.project().revision}))
}

pub fn rail_list(p: &Project) -> Value {
    top_list(p, "rail", "rails", &p.conn.rails, |r| &r.name)
}

/// `class.add`.
pub fn class_add(
    session: &mut Session,
    name: &str,
    id: Option<&str>,
    description: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    if p.conn.class(name).is_some() {
        return Err(Error::new(
            Code::DuplicateId,
            format!("there is already a class {name}"),
        )
        .with("class", name));
    }
    let id = new_id(id, "class", |s| {
        p.conn.classes.keys().any(|k| k.as_str() == s)
    })?;
    let c = Class {
        name: name.to_owned(),
        description: description.unwrap_or_default().to_owned(),
    };
    let mut w = Writes::default();
    w.put(top_path(CLASSES, &id), &c, conn::CLASS_KIND);
    commit(session, &format!("class {name}"), w)?;
    let p = session.project();
    Ok(top_json(p, CLASSES, "class", &p.conn.classes, &id))
}

/// `class.edit`.
pub fn class_edit(
    session: &mut Session,
    class: &str,
    name: Option<&str>,
    description: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let (id, mut c) = top_get(&p.conn.classes, p.conn.class(class), "class", class)?;
    if let Some(n) = name {
        c.name = n.to_owned();
    }
    if let Some(d) = description {
        c.description = d.to_owned();
    }
    let mut w = Writes::default();
    w.put(top_path(CLASSES, &id), &c, conn::CLASS_KIND);
    commit(session, &format!("class {id}"), w)?;
    let p = session.project();
    Ok(top_json(p, CLASSES, "class", &p.conn.classes, &id))
}

/// `class.delete`: refused while a net or rail names it.
pub fn class_delete(session: &mut Session, class: &str, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let (id, _) = top_get(&p.conn.classes, p.conn.class(class), "class", class)?;
    let mut using: Vec<String> = Vec::new();
    for (bid, b) in &p.conn.blocks {
        for (nid, n) in &b.nets {
            if n.item.as_ref().is_ok_and(|n| n.class.as_ref() == Some(&id)) {
                using.push(format!("{bid}/{nid}"));
            }
        }
    }
    for (rid, r) in &p.conn.rails {
        if r.item.as_ref().is_ok_and(|r| r.class.as_ref() == Some(&id)) {
            using.push(format!("rail/{rid}"));
        }
    }
    if !using.is_empty() {
        return Err(has_dependents(&format!("class {id}"), &using));
    }
    let mut w = Writes::default();
    w.delete(top_path(CLASSES, &id));
    commit(session, &format!("class {id}"), w)?;
    Ok(json!({"deleted": format!("class/{id}"), "revision": session.project().revision}))
}

pub fn class_list(p: &Project) -> Value {
    top_list(p, "class", "classes", &p.conn.classes, |c| &c.name)
}

/// `design.create`: a design of a root block.
pub fn design_create(
    session: &mut Session,
    name: &str,
    root: &str,
    id: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let root = write::block(p, root)?.id.clone();
    let id = new_id(id, "design", |s| {
        p.conn.designs.keys().any(|k| k.as_str() == s)
    })?;
    let d = Design {
        name: name.to_owned(),
        root,
    };
    let mut w = Writes::default();
    w.put(top_path(DESIGNS, &id), &d, conn::DESIGN_KIND);
    commit(session, &format!("design {name}"), w)?;
    let p = session.project();
    Ok(top_json(p, DESIGNS, "design", &p.conn.designs, &id))
}

/// `design.edit`: a new name or root.
pub fn design_edit(
    session: &mut Session,
    design: &str,
    name: Option<&str>,
    root: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let found = named(&p.conn.designs, design, |d: &Design| &d.name);
    let (id, mut d) = top_get(&p.conn.designs, found, "design", design)?;
    if let Some(n) = name {
        d.name = n.to_owned();
    }
    if let Some(r) = root {
        d.root = write::block(p, r)?.id.clone();
    }
    let mut w = Writes::default();
    w.put(top_path(DESIGNS, &id), &d, conn::DESIGN_KIND);
    commit(session, &format!("design {id}"), w)?;
    let p = session.project();
    Ok(top_json(p, DESIGNS, "design", &p.conn.designs, &id))
}

/// `design.delete`: its blocks stay.
pub fn design_delete(session: &mut Session, design: &str, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let found = named(&p.conn.designs, design, |d: &Design| &d.name);
    let (id, _) = top_get(&p.conn.designs, found, "design", design)?;
    let mut w = Writes::default();
    w.delete(top_path(DESIGNS, &id));
    commit(session, &format!("design {id}"), w)?;
    Ok(json!({"deleted": format!("design/{id}"), "revision": session.project().revision}))
}

pub fn design_list(p: &Project) -> Value {
    top_list(p, "design", "designs", &p.conn.designs, |d| &d.name)
}

/// A port op of `block.edit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortOp {
    Add {
        name: String,
        direction: Direction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voltage: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// An empty voltage clears it.
    Set {
        port: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<Direction>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voltage: Option<String>,
    },
    Remove {
        port: String,
    },
}

/// A bus op of `block.edit`: nets by ID or name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum BusOp {
    Add {
        name: String,
        #[serde(default)]
        nets: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Set {
        bus: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nets: Option<Vec<String>>,
    },
    Remove {
        bus: String,
    },
}

/// A block's answer: its file, what it holds, its faults and revision.
pub fn block_json(b: &BlockFiles) -> Value {
    let mut v = loaded_json(&b.block, "block");
    v["reference"] = json!(b.id);
    v["path"] = json!(block_path(&b.id));
    v["revision"] = json!(b.revision);
    v["faults"] = fault::list(&b.faults());
    let names = |m: &dyn Fn() -> Vec<Value>| Value::Array(m());
    v["components"] = names(&|| {
        b.components
            .iter()
            .map(|(id, c)| {
                let mut v = json!({"id": id});
                if let Ok(c) = &c.item {
                    v["refdes"] = json!(c.refdes);
                }
                v
            })
            .collect()
    });
    v["nets"] = names(&|| {
        b.nets
            .iter()
            .map(|(id, n)| {
                let mut v = json!({"id": id});
                if let Ok(n) = &n.item {
                    v["name"] = json!(n.name);
                }
                v
            })
            .collect()
    });
    v["instances"] = names(&|| {
        b.instances
            .iter()
            .map(|(id, i)| {
                let mut v = json!({"id": id});
                if let Ok(i) = &i.item {
                    v["of"] = json!(i.of);
                }
                v
            })
            .collect()
    });
    v
}

/// `block.create`.
pub fn block_create(
    session: &mut Session,
    name: &str,
    id: Option<&str>,
    description: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let id = new_id(id, "block", |s| {
        p.conn.blocks.keys().any(|k| k.as_str() == s)
    })?;
    let b = Block {
        name: name.to_owned(),
        description: description.unwrap_or_default().to_owned(),
        ports: BTreeMap::new(),
        buses: BTreeMap::new(),
        properties: BTreeMap::new(),
    };
    let mut w = Writes::default();
    w.put(block_path(&id), &b, conn::BLOCK_KIND);
    commit(session, &format!("block {name}"), w)?;
    Ok(block_json(session.project().conn.block(&id)?))
}

/// The port a request names in `b`, by ID or name.
fn port_of(b: &Block, block: &Id, text: &str) -> Result<Id> {
    b.port(text).cloned().ok_or_else(|| {
        Error::new(
            Code::PortNotFound,
            format!("block {block} has no port {text}"),
        )
        .with("port", text)
    })
}

/// What names port `port` of block `b`: its own pins and instance ports,
/// and every instance of `b` that maps it.
fn port_users(p: &Project, b: &Id, port: &Id) -> Vec<String> {
    let mut out = users(p, &Node::Port(port.clone()), Some(b));
    for (bid, x) in &p.conn.blocks {
        for (iid, i) in &x.instances {
            if i.item
                .as_ref()
                .is_ok_and(|i| &i.of == b && i.ports.contains_key(port))
            {
                out.push(format!("{bid}/{iid}/{port}"));
            }
        }
    }
    out
}

/// `block.edit`: a new name or description, and ops on ports and buses.
pub fn block_edit(
    session: &mut Session,
    block: &str,
    name: Option<&str>,
    description: Option<&str>,
    ports: &[PortOp],
    buses: &[BusOp],
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let files = write::block(p, block)?;
    check_revision(&format!("block {}", files.id), &files.revision, expected)?;
    let bid = files.id.clone();
    let mut b = files
        .block
        .item
        .clone()
        .map_err(|e| e.with("block", block))?;
    if let Some(n) = name {
        b.name = n.to_owned();
    }
    if let Some(d) = description {
        b.description = d.to_owned();
    }
    for (i, op) in ports.iter().enumerate() {
        let at = |e: Error| e.with("pointer", format!("/ports/{i}"));
        match op {
            PortOp::Add {
                name,
                direction,
                voltage,
                id,
            } => {
                let id = new_id(id.as_deref(), "port", |s| {
                    b.ports.keys().any(|k| k.as_str() == s)
                })
                .map_err(at)?;
                b.ports.insert(
                    id,
                    Port {
                        name: name.clone(),
                        direction: *direction,
                        voltage: voltage.clone().filter(|v| !v.is_empty()),
                    },
                );
            }
            PortOp::Set {
                port,
                name,
                direction,
                voltage,
            } => {
                let id = port_of(&b, &bid, port).map_err(at)?;
                let x = b.ports.get_mut(&id).expect("found");
                if let Some(n) = name {
                    x.name = n.clone();
                }
                if let Some(d) = direction {
                    x.direction = *d;
                }
                set_opt(&mut x.voltage, voltage);
            }
            PortOp::Remove { port } => {
                let id = port_of(&b, &bid, port).map_err(at)?;
                let using = port_users(p, &bid, &id);
                if !using.is_empty() {
                    return Err(at(has_dependents(&format!("port {id}"), &using)));
                }
                b.ports.remove(&id);
            }
        }
    }
    let net = |text: &str| {
        files.net(text).cloned().ok_or_else(|| {
            Error::new(Code::NetNotFound, format!("block {bid} has no net {text}"))
                .with("net", text)
        })
    };
    for (i, op) in buses.iter().enumerate() {
        let at = |e: Error| e.with("pointer", format!("/buses/{i}"));
        let bus_of = |b: &Block, text: &str| {
            b.buses
                .keys()
                .find(|id| id.as_str() == text)
                .or_else(|| {
                    b.buses
                        .iter()
                        .find(|(_, x)| x.name == text)
                        .map(|(id, _)| id)
                })
                .cloned()
        };
        match op {
            BusOp::Add { name, nets, id } => {
                let id = new_id(id.as_deref(), "bus", |s| {
                    b.buses.keys().any(|k| k.as_str() == s)
                })
                .map_err(at)?;
                let nets = nets
                    .iter()
                    .map(|n| net(n))
                    .collect::<Result<_>>()
                    .map_err(at)?;
                b.buses.insert(
                    id,
                    Bus {
                        name: name.clone(),
                        nets,
                    },
                );
            }
            BusOp::Set { bus, name, nets } => {
                let id = bus_of(&b, bus).ok_or_else(|| {
                    at(Error::new(
                        Code::NotFound,
                        format!("block {bid} has no bus {bus}"),
                    ))
                })?;
                let nets: Option<Vec<Id>> = nets
                    .as_ref()
                    .map(|n| n.iter().map(|x| net(x)).collect::<Result<_>>())
                    .transpose()
                    .map_err(at)?;
                let x = b.buses.get_mut(&id).expect("found");
                if let Some(n) = name {
                    x.name = n.clone();
                }
                if let Some(n) = nets {
                    x.nets = n;
                }
            }
            BusOp::Remove { bus } => {
                let id = bus_of(&b, bus).ok_or_else(|| {
                    at(Error::new(
                        Code::NotFound,
                        format!("block {bid} has no bus {bus}"),
                    ))
                })?;
                b.buses.remove(&id);
            }
        }
    }
    let mut w = Writes::default();
    w.put(block_path(&bid), &b, conn::BLOCK_KIND);
    commit(session, &format!("block {bid}"), w)?;
    Ok(block_json(session.project().conn.block(&bid)?))
}

pub fn block_show(p: &Project, block: &str) -> Result<Value> {
    Ok(block_json(write::block(p, block)?))
}

pub fn block_list(p: &Project) -> Value {
    let list: Vec<Value> = p
        .conn
        .blocks
        .iter()
        .map(|(id, b)| {
            let mut v = json!({
                "id": id,
                "revision": b.revision,
                "components": b.components.len(),
                "nets": b.nets.len(),
                "instances": b.instances.len(),
                "faults": b.faults().len(),
            });
            match &b.block.item {
                Ok(x) => v["name"] = json!(x.name),
                Err(e) => v["error"] = json!(e),
            }
            v
        })
        .collect();
    json!({"blocks": list})
}

/// `block.delete`: the block's folder, refused while an instance places
/// it or a design is rooted on it.
pub fn block_delete(session: &mut Session, block: &str, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    let files = write::block(p, block)?;
    check_revision(&format!("block {}", files.id), &files.revision, expected)?;
    let bid = files.id.clone();
    let mut using = Vec::new();
    for (oid, o) in &p.conn.blocks {
        for (iid, i) in &o.instances {
            if i.item.as_ref().is_ok_and(|i| i.of == bid) {
                using.push(format!("{oid}/{iid}"));
            }
        }
    }
    for (did, d) in &p.conn.designs {
        if d.item.as_ref().is_ok_and(|d| d.root == bid) {
            using.push(format!("design/{did}"));
        }
    }
    if !using.is_empty() {
        return Err(has_dependents(&format!("block {bid}"), &using));
    }
    let mut w = Writes::default();
    for (part, ids) in [
        (Part::Component, files.components.keys().collect::<Vec<_>>()),
        (Part::Net, files.nets.keys().collect()),
        (Part::Instance, files.instances.keys().collect()),
    ] {
        for id in ids {
            w.delete(part_path(&bid, part, id));
        }
    }
    w.delete(block_path(&bid));
    commit(session, &format!("block {bid}"), w)?;
    Ok(json!({"deleted": bid, "path": format!("{BLOCKS}/{bid}")}))
}
