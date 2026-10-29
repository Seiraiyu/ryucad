//! A project's connectivity as read: its designs, blocks (each with its
//! components, nets and instances), rails and classes, every file with
//! what its checks find. A fault fails only what reads its file: a broken
//! component fails its block's designs' netlists, not the project's load.
//!
//! The checks here are those a file can't make alone: what pins and
//! instances name is in the block (or the project), names are unique, a
//! component's library component resolves, and no block contains itself.
//! Electrical judgement is ERC's.

use crate::conn::{
    self, BLOCK_FILE, BLOCKS, Block, CLASSES, COMPONENTS, Class, Component, DESIGNS, Design,
    INSTANCES, Instance, NC, NETS, Net, Node, RAILS, Rail,
};
use crate::doc;
use crate::error::{Code, Error, Result};
use crate::fault::Fault;
use crate::id::Id;
use crate::libcomp::LibRef;
use crate::library::Loaded;
use crate::source::{Kind, Source, join};
use crate::symbol::pointer_escape;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// One block's folder.
#[derive(Clone, Debug)]
pub struct BlockFiles {
    pub id: Id,
    /// A hash of every file in the block's folder: writes are guarded by
    /// it.
    pub revision: String,
    /// `block.json`, or why it doesn't read (or isn't there).
    pub block: Loaded<Block>,
    pub components: BTreeMap<Id, Loaded<Component>>,
    pub nets: BTreeMap<Id, Loaded<Net>>,
    pub instances: BTreeMap<Id, Loaded<Instance>>,
}

/// Which folder of a block an object's file is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Component,
    Net,
    Instance,
}

impl Part {
    pub fn folder(self) -> &'static str {
        match self {
            Part::Component => COMPONENTS,
            Part::Net => NETS,
            Part::Instance => INSTANCES,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Part::Component => "component",
            Part::Net => "net",
            Part::Instance => "instance",
        }
    }
}

/// The path of a block's file, relative to the project.
pub fn block_path(block: &Id) -> String {
    join(&join(BLOCKS, block.as_str()), BLOCK_FILE)
}

/// The path of an object's file in a block.
pub fn part_path(block: &Id, part: Part, id: &Id) -> String {
    join(
        &join(&join(BLOCKS, block.as_str()), part.folder()),
        &format!("{id}.json"),
    )
}

/// The path of a design's, rail's or class's file.
pub fn top_path(folder: &str, id: &Id) -> String {
    join(folder, &format!("{id}.json"))
}

impl BlockFiles {
    /// Whether an object of any part of the block has the ID `id`:
    /// components, nets and instances share one, so `<block>/<id>` names
    /// one object.
    pub fn has(&self, id: &str) -> bool {
        let Ok(id) = Id::parse(id, "object") else {
            return false;
        };
        self.components.contains_key(&id)
            || self.nets.contains_key(&id)
            || self.instances.contains_key(&id)
    }

    /// Every fault of the block's files, `block.json` first.
    pub fn faults(&self) -> Vec<Fault> {
        let mut out = loaded_faults(&self.block, &block_path(&self.id));
        for (part, list) in [
            (
                Part::Component,
                self.components.iter().map(dyn_faults).collect::<Vec<_>>(),
            ),
            (Part::Net, self.nets.iter().map(dyn_faults).collect()),
            (
                Part::Instance,
                self.instances.iter().map(dyn_faults).collect(),
            ),
        ] {
            for (id, item, faults) in list {
                let path = part_path(&self.id, part, id);
                if let Some(e) = item {
                    out.push(unreadable(e, &path));
                }
                out.extend(faults.iter().cloned());
            }
        }
        out
    }

    /// The block's net called `name`, by ID or by name.
    pub fn net(&self, name: &str) -> Option<&Id> {
        self.nets.keys().find(|id| id.as_str() == name).or_else(|| {
            self.nets
                .iter()
                .find(|(_, n)| n.item.as_ref().is_ok_and(|n| n.name == name))
                .map(|(id, _)| id)
        })
    }
}

type Faulted<'a> = (&'a Id, Option<&'a Error>, &'a Vec<Fault>);

fn dyn_faults<'a, T>((id, l): (&'a Id, &'a Loaded<T>)) -> Faulted<'a> {
    (id, l.item.as_ref().err(), &l.faults)
}

fn unreadable(e: &Error, path: &str) -> Fault {
    Fault {
        severity: crate::fault::Severity::Error,
        error: e.clone().with("path", path),
    }
}

fn loaded_faults<T>(l: &Loaded<T>, path: &str) -> Vec<Fault> {
    let mut out: Vec<Fault> = l
        .item
        .as_ref()
        .err()
        .map(|e| unreadable(e, path))
        .into_iter()
        .collect();
    out.extend(l.faults.iter().cloned());
    out
}

/// A project's connectivity.
#[derive(Clone, Debug, Default)]
pub struct Connectivity {
    pub designs: BTreeMap<Id, Loaded<Design>>,
    pub blocks: BTreeMap<Id, BlockFiles>,
    pub rails: BTreeMap<Id, Loaded<Rail>>,
    pub classes: BTreeMap<Id, Loaded<Class>>,
    /// Files that are none of these, and folders with no `block.json`.
    pub faults: Vec<Fault>,
    /// A hash of the designs', rails' and classes' files: part of the
    /// project's revision, which guards writes to them.
    pub top: [u8; 32],
}

/// A file of one folder: its name and bytes.
type Named = (String, Arc<[u8]>);

/// Feeds one length-prefixed piece to a revision hash.
pub(crate) fn piece(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

fn unknown(path: &str) -> Fault {
    Fault::warning(
        Code::InvalidDocument,
        format!("{path} isn't a file ryueda's designs hold"),
    )
    .with("path", path)
}

/// The files of folder `dir`, each hashed (by its path within `within`)
/// into `h`; folders in it are faults.
fn files_in(
    src: &dyn Source,
    dir: &str,
    within: &str,
    h: &mut blake3::Hasher,
    faults: &mut Vec<Fault>,
) -> Result<Vec<Named>> {
    let mut out = Vec::new();
    for (name, kind) in src.list(dir)? {
        let path = join(dir, &name);
        if kind != Kind::File {
            faults.push(unknown(&path));
            continue;
        }
        let bytes = src
            .read_shared(&path)?
            .ok_or_else(|| Error::new(Code::NotFound, format!("{path} vanished")))?;
        piece(h, join(within, &name).as_bytes());
        piece(h, &bytes);
        out.push((name, bytes));
    }
    Ok(out)
}

/// Reads every `<id>.json` of one folder in parallel as a `kind`, each
/// with what its own checks find and its path in their details.
fn read_all<T: serde::de::DeserializeOwned + Send>(
    files: &[Named],
    dir: &str,
    kind: &str,
    faults: &mut Vec<Fault>,
    check: impl Fn(&T) -> Vec<Fault> + Sync,
) -> BTreeMap<Id, Loaded<T>> {
    let mut named = Vec::new();
    for (file, bytes) in files {
        let id = file
            .strip_suffix(".json")
            .and_then(|s| Id::parse(s, "design object").ok());
        match id {
            Some(id) => named.push((id, join(dir, file), bytes)),
            None => faults.push(unknown(&join(dir, file))),
        }
    }
    named
        .into_par_iter()
        .map(|(id, path, bytes)| {
            let loaded = match doc::read::<T>(bytes, &path, kind, conn::FORMAT) {
                Ok(t) => Loaded {
                    faults: check(&t)
                        .into_iter()
                        .map(|f| f.with("path", &path))
                        .collect(),
                    item: Ok(t),
                },
                Err(e) => Loaded {
                    item: Err(e),
                    faults: Vec::new(),
                },
            };
            (id, loaded)
        })
        .collect()
}

/// Reads one block's folder; `None` for a folder holding no files at all
/// (what undoing a block's creation leaves).
fn load_block(src: &dyn Source, id: Id, faults: &mut Vec<Fault>) -> Result<Option<BlockFiles>> {
    let dir = join(BLOCKS, id.as_str());
    let mut h = blake3::Hasher::new();
    h.update(b"ryueda.block.1");
    let mut file = None;
    let mut parts: BTreeMap<String, Vec<Named>> = BTreeMap::new();
    let mut any = false;
    for (name, kind) in src.list(&dir)? {
        let path = join(&dir, &name);
        match (name.as_str(), kind) {
            (BLOCK_FILE, Kind::File) => {
                let bytes = src
                    .read_shared(&path)?
                    .ok_or_else(|| Error::new(Code::NotFound, format!("{path} vanished")))?;
                piece(&mut h, BLOCK_FILE.as_bytes());
                piece(&mut h, &bytes);
                file = Some(bytes);
                any = true;
            }
            (folder @ (COMPONENTS | NETS | INSTANCES), Kind::Dir) => {
                let found = files_in(src, &path, folder, &mut h, faults)?;
                any |= !found.is_empty();
                parts.insert(folder.to_owned(), found);
            }
            _ => {
                faults.push(unknown(&path));
                any = true;
            }
        }
    }
    if !any {
        return Ok(None);
    }
    let path = block_path(&id);
    let block = match file {
        Some(bytes) => match doc::read::<Block>(&bytes, &path, conn::BLOCK_KIND, conn::FORMAT) {
            Ok(b) => Loaded {
                faults: b
                    .check()
                    .into_iter()
                    .map(|f| f.with("path", &path))
                    .collect(),
                item: Ok(b),
            },
            Err(e) => Loaded {
                item: Err(e),
                faults: Vec::new(),
            },
        },
        None => Loaded {
            item: Err(Error::new(
                Code::NotFound,
                format!("{dir} holds files but no {BLOCK_FILE}"),
            )),
            faults: Vec::new(),
        },
    };
    let take = |f: &str| parts.get(f).cloned().unwrap_or_default();
    let components = read_all(
        &take(COMPONENTS),
        &join(&dir, COMPONENTS),
        conn::COMPONENT_KIND,
        faults,
        Component::check,
    );
    let nets = read_all(
        &take(NETS),
        &join(&dir, NETS),
        conn::NET_KIND,
        faults,
        Net::check,
    );
    let instances = read_all(
        &take(INSTANCES),
        &join(&dir, INSTANCES),
        conn::INSTANCE_KIND,
        faults,
        Instance::check,
    );
    Ok(Some(BlockFiles {
        id,
        revision: h.finalize().to_hex()[..32].to_owned(),
        block,
        components,
        nets,
        instances,
    }))
}

/// Reads a project's connectivity from `src`. References between files
/// are judged by [`Connectivity::judge`], once the libraries are read.
pub fn load(src: &dyn Source) -> Result<Connectivity> {
    let mut faults = Vec::new();
    let mut top = blake3::Hasher::new();
    top.update(b"ryueda.top.1");
    let mut tops = BTreeMap::new();
    for folder in [DESIGNS, RAILS, CLASSES] {
        tops.insert(
            folder,
            files_in(src, folder, folder, &mut top, &mut faults)?,
        );
    }
    let designs = read_all(
        &tops[DESIGNS],
        DESIGNS,
        conn::DESIGN_KIND,
        &mut faults,
        Design::check,
    );
    let rails = read_all(
        &tops[RAILS],
        RAILS,
        conn::RAIL_KIND,
        &mut faults,
        Rail::check,
    );
    let classes = read_all(
        &tops[CLASSES],
        CLASSES,
        conn::CLASS_KIND,
        &mut faults,
        Class::check,
    );
    let mut blocks = BTreeMap::new();
    for (name, kind) in src.list(BLOCKS)? {
        let id = Id::parse(&name, "block").ok().filter(|_| kind == Kind::Dir);
        let Some(id) = id else {
            faults.push(unknown(&join(BLOCKS, &name)));
            continue;
        };
        if let Some(b) = load_block(src, id.clone(), &mut faults)? {
            blocks.insert(id, b);
        }
    }
    Ok(Connectivity {
        designs,
        blocks,
        rails,
        classes,
        faults,
        top: *top.finalize().as_bytes(),
    })
}

fn fault(code: Code, ptr: String, msg: String) -> Fault {
    Fault::error(code, msg).with("pointer", ptr)
}

impl Connectivity {
    /// The block `id`, or `not_found`.
    pub fn block(&self, id: &Id) -> Result<&BlockFiles> {
        self.blocks.get(id).ok_or_else(|| {
            Error::new(Code::NotFound, format!("there is no block {id}")).with("block", id)
        })
    }

    /// Whether a rail has the ID `id`.
    pub fn has_rail(&self, id: &str) -> bool {
        self.rails.keys().any(|k| k.as_str() == id)
    }

    /// The rail called `name`, by ID or by name.
    pub fn rail(&self, name: &str) -> Option<&Id> {
        named(&self.rails, name, |r| &r.name)
    }

    /// The class called `name`, by ID or by name.
    pub fn class(&self, name: &str) -> Option<&Id> {
        named(&self.classes, name, |c| &c.name)
    }

    /// Blocks in cycles of instances, with the blocks of each cycle.
    fn cycles(&self) -> BTreeMap<Id, Vec<Id>> {
        let edges: BTreeMap<&Id, BTreeSet<&Id>> = self
            .blocks
            .iter()
            .map(|(id, b)| {
                let to = b
                    .instances
                    .values()
                    .filter_map(|i| i.item.as_ref().ok())
                    .map(|i| &i.of)
                    .filter(|of| self.blocks.contains_key(*of))
                    .collect();
                (id, to)
            })
            .collect();
        // Each block reaches itself, or not, by a walk of its successors.
        let mut out = BTreeMap::new();
        for start in edges.keys() {
            let mut seen = BTreeSet::new();
            let mut stack: Vec<&Id> = edges[start].iter().copied().collect();
            while let Some(b) = stack.pop() {
                if seen.insert(b) {
                    stack.extend(edges.get(b).into_iter().flatten().copied());
                }
            }
            if seen.contains(start) {
                // The cycle's blocks: those it reaches that reach it back.
                let chain: Vec<Id> = seen
                    .iter()
                    .filter(|b| {
                        let mut seen = BTreeSet::new();
                        let mut stack = vec![**b];
                        while let Some(x) = stack.pop() {
                            if seen.insert(x) {
                                stack.extend(edges.get(x).into_iter().flatten().copied());
                            }
                        }
                        seen.contains(start)
                    })
                    .map(|b| (*b).clone())
                    .collect();
                out.insert((*start).clone(), chain);
            }
        }
        out
    }

    /// Adds what each file's references find: designs' roots; pins',
    /// ports' and buses' nets, ports and rails; nets' classes and pairs;
    /// instances' blocks, ports and overrides; names unique among a
    /// block's nets and ports and the rails; one ID per object of a block;
    /// each component's library component, by `libcomp`; and cycles.
    pub fn judge(&mut self, libcomp: &(dyn Fn(&LibRef) -> Result<()> + Sync)) {
        let rail_names = self.rail_names();
        let cycles = self.cycles();
        let judged: Vec<(Id, BlockFiles)> = self
            .blocks
            .par_iter()
            .map(|(id, b)| {
                let mut b = b.clone();
                self.judge_block(&mut b, &rail_names, &cycles, libcomp);
                (id.clone(), b)
            })
            .collect();
        self.blocks = judged.into_iter().collect();
        for (id, d) in &mut self.designs {
            if let Ok(design) = &d.item
                && !self.blocks.contains_key(&design.root)
            {
                d.faults.push(
                    fault(
                        Code::NotFound,
                        "/root".into(),
                        format!("design {id}'s root block {} isn't there", design.root),
                    )
                    .with("path", top_path(DESIGNS, id)),
                );
            }
        }
        let names: Vec<(Id, String)> = self
            .rails
            .iter()
            .filter_map(|(id, r)| Some((id.clone(), r.item.as_ref().ok()?.name.clone())))
            .collect();
        for (id, name) in &names {
            let others: Vec<&Id> = names
                .iter()
                .filter(|(o, n)| n == name && o != id)
                .map(|(o, _)| o)
                .collect();
            let rail = self.rails.get_mut(id).expect("named");
            if !others.is_empty() {
                rail.faults.push(
                    fault(
                        Code::NetNameDuplicate,
                        "/name".into(),
                        format!("rail {name} shares its name with {}", ids(&others)),
                    )
                    .with("path", top_path(RAILS, id)),
                );
            }
            if let Ok(r) = &rail.item
                && let Some(c) = &r.class
                && !self.classes.contains_key(c)
            {
                rail.faults.push(
                    fault(
                        Code::NotFound,
                        "/class".into(),
                        format!("there is no class {c}"),
                    )
                    .with("path", top_path(RAILS, id)),
                );
            }
        }
    }

    fn rail_names(&self) -> BTreeMap<String, Id> {
        self.rails
            .iter()
            .filter_map(|(id, r)| Some((r.item.as_ref().ok()?.name.clone(), id.clone())))
            .collect()
    }

    /// What a node a file of block `b` names finds: a missing net, rail or
    /// port.
    fn node_fault(&self, b: &BlockFiles, n: &Node, ptr: &str) -> Option<Fault> {
        let (code, what) = match n {
            Node::Net(id) if !b.nets.contains_key(id) => (Code::NetNotFound, format!("net {id}")),
            Node::Rail(id) if !self.rails.contains_key(id) => {
                (Code::NotFound, format!("rail {id}"))
            }
            Node::Port(id)
                if !b
                    .block
                    .item
                    .as_ref()
                    .is_ok_and(|x| x.ports.contains_key(id)) =>
            {
                (Code::PortNotFound, format!("port {id}"))
            }
            _ => return None,
        };
        Some(fault(
            code,
            ptr.to_owned(),
            format!("block {} has no {what}", b.id),
        ))
    }

    fn judge_block(
        &self,
        b: &mut BlockFiles,
        rail_names: &BTreeMap<String, Id>,
        cycles: &BTreeMap<Id, Vec<Id>>,
        libcomp: &(dyn Fn(&LibRef) -> Result<()> + Sync),
    ) {
        let bid = b.id.clone();
        let block = b.block.item.as_ref().ok().cloned();
        // Names: a block's nets and ports share them with each other and
        // with the rails.
        let mut taken: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Some(block) = &block {
            for (id, p) in &block.ports {
                taken
                    .entry(p.name.clone())
                    .or_default()
                    .push(format!("port {id}"));
            }
        }
        for (id, n) in &b.nets {
            if let Ok(n) = &n.item {
                taken
                    .entry(n.name.clone())
                    .or_default()
                    .push(format!("net {id}"));
            }
        }
        if let Some(block) = &block {
            let path = block_path(&bid);
            for (id, p) in &block.ports {
                let mut others: Vec<String> = taken[&p.name]
                    .iter()
                    .filter(|o| **o != format!("port {id}") && o.starts_with("port"))
                    .cloned()
                    .collect();
                others.extend(rail_names.get(&p.name).map(|r| format!("rail {r}")));
                if !others.is_empty() {
                    b.block.faults.push(
                        fault(
                            Code::NetNameDuplicate,
                            format!("/ports/{id}/name"),
                            format!("port {} shares its name with {}", p.name, others.join(", ")),
                        )
                        .with("path", &path),
                    );
                }
            }
            for (bus, x) in &block.buses {
                for (i, n) in x.nets.iter().enumerate() {
                    if !b.nets.contains_key(n) {
                        b.block.faults.push(
                            fault(
                                Code::NetNotFound,
                                format!("/buses/{bus}/nets/{i}"),
                                format!("block {bid} has no net {n}"),
                            )
                            .with("path", &path),
                        );
                    }
                }
            }
        }
        let nets: BTreeSet<Id> = b.nets.keys().cloned().collect();
        let components: BTreeSet<Id> = b.components.keys().cloned().collect();
        let mut found: Vec<(Part, Id, Fault)> = Vec::new();
        for (id, n) in &b.nets {
            let Ok(net) = &n.item else { continue };
            let mut others: Vec<String> = taken[&net.name]
                .iter()
                .filter(|o| **o != format!("net {id}"))
                .cloned()
                .collect();
            others.extend(rail_names.get(&net.name).map(|r| format!("rail {r}")));
            if !others.is_empty() {
                found.push((
                    Part::Net,
                    id.clone(),
                    fault(
                        Code::NetNameDuplicate,
                        "/name".into(),
                        format!(
                            "net {} shares its name with {}",
                            net.name,
                            others.join(", ")
                        ),
                    ),
                ));
            }
            if id.as_str() == NC {
                found.push((
                    Part::Net,
                    id.clone(),
                    fault(
                        Code::InvalidDocument,
                        String::new(),
                        format!("no net may take the ID {NC}: it marks a pin not connected"),
                    ),
                ));
            }
            if components.contains(id) {
                found.push((
                    Part::Net,
                    id.clone(),
                    fault(
                        Code::DuplicateId,
                        String::new(),
                        format!("block {bid} has a component {id} too"),
                    ),
                ));
            }
            if let Some(c) = &net.class
                && !self.classes.contains_key(c)
            {
                found.push((
                    Part::Net,
                    id.clone(),
                    fault(
                        Code::NotFound,
                        "/class".into(),
                        format!("there is no class {c}"),
                    ),
                ));
            }
            if let Some(p) = &net.pair
                && !nets.contains(&p.with)
            {
                found.push((
                    Part::Net,
                    id.clone(),
                    fault(
                        Code::NetNotFound,
                        "/pair/with".into(),
                        format!("block {bid} has no net {}", p.with),
                    ),
                ));
            }
        }
        for (id, c) in &b.components {
            let Ok(comp) = &c.item else { continue };
            for (pin, n) in &comp.pins {
                let ptr = format!("/pins/{}", pointer_escape(pin));
                if let Some(f) = self.node_fault(b, n, &ptr) {
                    found.push((Part::Component, id.clone(), f));
                }
            }
            if let Err(e) = libcomp(&comp.of) {
                found.push((
                    Part::Component,
                    id.clone(),
                    Fault {
                        severity: crate::fault::Severity::Error,
                        error: e.with("pointer", "/of"),
                    },
                ));
            }
        }
        for (id, i) in &b.instances {
            let Ok(inst) = &i.item else { continue };
            if components.contains(id) || nets.contains(id) {
                found.push((
                    Part::Instance,
                    id.clone(),
                    fault(
                        Code::DuplicateId,
                        String::new(),
                        format!("block {bid} has a component or net {id} too"),
                    ),
                ));
            }
            let child = self.blocks.get(&inst.of);
            match child {
                None => found.push((
                    Part::Instance,
                    id.clone(),
                    fault(
                        Code::NotFound,
                        "/of".into(),
                        format!("there is no block {}", inst.of),
                    ),
                )),
                Some(child) => {
                    if let Some(chain) = cycles.get(&bid).filter(|c| c.contains(&inst.of)) {
                        found.push((
                            Part::Instance,
                            id.clone(),
                            fault(
                                Code::BlockCycle,
                                "/of".into(),
                                format!(
                                    "block {} contains block {bid} again, through {}",
                                    inst.of,
                                    ids(&chain.iter().collect::<Vec<_>>())
                                ),
                            )
                            .with("chain", chain),
                        ));
                    }
                    let ports = child.block.item.as_ref().ok().map(|x| &x.ports);
                    for port in inst.ports.keys() {
                        if !ports.is_some_and(|p| p.contains_key(port)) {
                            found.push((
                                Part::Instance,
                                id.clone(),
                                fault(
                                    Code::PortNotFound,
                                    format!("/ports/{port}"),
                                    format!("block {} has no port {port}", inst.of),
                                ),
                            ));
                        }
                    }
                    for path in inst.overrides.keys() {
                        if !self.reaches(child, path) {
                            found.push((
                                Part::Instance,
                                id.clone(),
                                fault(
                                    Code::NotFound,
                                    format!("/overrides/{}", pointer_escape(path)),
                                    format!("block {} has no occurrence {path}", inst.of),
                                ),
                            ));
                        }
                    }
                }
            }
            for (port, n) in &inst.ports {
                if let Some(f) = self.node_fault(b, n, &format!("/ports/{port}")) {
                    found.push((Part::Instance, id.clone(), f));
                }
            }
        }
        for (part, id, f) in found {
            let f = f.with("path", part_path(&bid, part, &id));
            match part {
                Part::Component => b.components.get_mut(&id).expect("judged").faults.push(f),
                Part::Net => b.nets.get_mut(&id).expect("judged").faults.push(f),
                Part::Instance => b.instances.get_mut(&id).expect("judged").faults.push(f),
            }
        }
    }

    /// Whether `path` (`<instance>/…/<component>`) names a component below
    /// `block`.
    fn reaches(&self, block: &BlockFiles, path: &str) -> bool {
        let segments: Vec<&str> = path.split('/').collect();
        let (last, through) = segments.split_last().expect("split gives one");
        let mut at = block;
        for s in through {
            let next = Id::parse(s, "instance")
                .ok()
                .and_then(|i| at.instances.get(&i))
                .and_then(|i| i.item.as_ref().ok())
                .and_then(|i| self.blocks.get(&i.of));
            match next {
                Some(b) => at = b,
                None => return false,
            }
        }
        Id::parse(last, "component").is_ok_and(|c| at.components.contains_key(&c))
    }

    /// Every fault of every file, in path order.
    pub fn faults(&self) -> Vec<Fault> {
        let mut out = self.faults.clone();
        for (folder, list) in [
            (
                DESIGNS,
                self.designs.iter().map(dyn_faults).collect::<Vec<_>>(),
            ),
            (RAILS, self.rails.iter().map(dyn_faults).collect()),
            (CLASSES, self.classes.iter().map(dyn_faults).collect()),
        ] {
            for (id, item, faults) in list {
                if let Some(e) = item {
                    out.push(unreadable(e, &top_path(folder, id)));
                }
                out.extend(faults.iter().cloned());
            }
        }
        for b in self.blocks.values() {
            out.extend(b.faults());
        }
        out
    }

    /// The blocks a design reaches from its root, the root first, then in
    /// ID order; cycles stop at a block already reached.
    pub fn reached<'a>(&'a self, root: &'a Id) -> Vec<&'a Id> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(b) = stack.pop() {
            if !seen.insert(b) {
                continue;
            }
            if let Some(files) = self.blocks.get(b) {
                stack.extend(
                    files
                        .instances
                        .values()
                        .filter_map(|i| i.item.as_ref().ok())
                        .map(|i| &i.of),
                );
            }
        }
        let mut out = vec![root];
        out.extend(seen.into_iter().filter(|b| *b != root));
        out
    }
}

fn named<'a, T>(
    map: &'a BTreeMap<Id, Loaded<T>>,
    name: &str,
    of: impl Fn(&T) -> &String,
) -> Option<&'a Id> {
    map.keys().find(|id| id.as_str() == name).or_else(|| {
        let mut hits = map
            .iter()
            .filter(|(_, l)| l.item.as_ref().is_ok_and(|t| of(t) == name));
        let first = hits.next().map(|(id, _)| id);
        // A name two share names neither.
        first.filter(|_| hits.next().is_none())
    })
}

fn ids(list: &[&Id]) -> String {
    list.iter()
        .map(|i| i.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::conn::Channel;
    use serde_json::{Value, json};

    pub fn id(s: &str) -> Id {
        Id::parse(s, "test").unwrap()
    }

    fn file(kind: &str, v: Value) -> Vec<u8> {
        let mut v = v;
        v["kind"] = json!(kind);
        v["format"] = json!(1);
        crate::canonical::to_string(&v).into_bytes()
    }

    /// A design `d` whose root block `top` holds a sensor block `s` twice
    /// and a pull-up, with rails `gnd` and `v3v3`.
    pub fn files() -> BTreeMap<String, Vec<u8>> {
        let mut f = BTreeMap::new();
        let mut put = |p: &str, kind: &str, v: Value| {
            f.insert(p.to_owned(), file(kind, v));
        };
        put(
            "designs/d.json",
            conn::DESIGN_KIND,
            json!({"name": "Breakout", "root": "top"}),
        );
        put("rails/gnd.json", conn::RAIL_KIND, json!({"name": "GND"}));
        put(
            "rails/v3v3.json",
            conn::RAIL_KIND,
            json!({"name": "3V3", "voltage": "3.3 V"}),
        );
        put(
            "classes/power.json",
            conn::CLASS_KIND,
            json!({"name": "Power"}),
        );
        put(
            "blocks/top/block.json",
            conn::BLOCK_KIND,
            json!({"name": "Top"}),
        );
        put(
            "blocks/top/nets/sda.json",
            conn::NET_KIND,
            json!({"name": "SDA"}),
        );
        put(
            "blocks/top/components/r1.json",
            conn::COMPONENT_KIND,
            json!({"of": {"id": "r_100k"}, "refdes": "R1", "pins": {"1": "sda", "2": {"rail": "v3v3"}}}),
        );
        for (i, ch) in [("ch_a", "_A"), ("ch_b", "_B")] {
            put(
                &format!("blocks/top/instances/{i}.json"),
                conn::INSTANCE_KIND,
                json!({"of": "s", "ports": {"sda": "sda"}, "refdes": {"rule": "suffix", "value": ch}}),
            );
        }
        put(
            "blocks/s/block.json",
            conn::BLOCK_KIND,
            json!({"name": "Sensor", "ports": {"sda": {"name": "SDA", "direction": "bidirectional"}}}),
        );
        put(
            "blocks/s/components/u1.json",
            conn::COMPONENT_KIND,
            json!({"of": {"id": "r_100k"}, "refdes": "U1", "pins": {"1": {"port": "sda"}, "2": {"rail": "gnd"}}}),
        );
        f
    }

    fn judged(f: &BTreeMap<String, Vec<u8>>) -> Connectivity {
        let mut c = load(f).unwrap();
        c.judge(&|r: &LibRef| {
            if r.id.as_str() == "r_100k" {
                Ok(())
            } else {
                Err(Error::new(Code::NotFound, format!("no {}", r.id)))
            }
        });
        c
    }

    fn codes(c: &Connectivity) -> Vec<(Code, String, String)> {
        c.faults()
            .into_iter()
            .map(|f| {
                (
                    f.error.code,
                    f.error.details["path"].as_str().unwrap_or("").to_owned(),
                    f.error.details["pointer"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn a_sound_design_reads_without_faults() {
        let c = judged(&files());
        assert_eq!(codes(&c), []);
        assert_eq!(c.blocks.len(), 2);
        let top = c.block(&id("top")).unwrap();
        assert_eq!(top.revision.len(), 32);
        assert!(top.has("r1") && top.has("sda") && top.has("ch_a") && !top.has("x"));
        assert_eq!(top.net("SDA"), Some(&id("sda")));
        assert_eq!(c.rail("3V3"), Some(&id("v3v3")));
        assert_eq!(c.reached(&id("top")), [&id("top"), &id("s")]);
        let i = top.instances[&id("ch_a")].item.as_ref().unwrap();
        assert_eq!(i.refdes, Some(Channel::Suffix("_A".into())));
    }

    #[test]
    fn references_that_dont_resolve_are_faults_where_they_are_named() {
        let mut f = files();
        let mut put = |p: &str, kind: &str, v: Value| {
            f.insert(p.to_owned(), file(kind, v));
        };
        put(
            "blocks/top/components/r2.json",
            conn::COMPONENT_KIND,
            json!({"of": {"id": "nope"}, "refdes": "R2",
                   "pins": {"1": "gone", "2": {"rail": "none"}, "3": {"port": "p"}, "4": "nc"}}),
        );
        put(
            "blocks/top/nets/dup.json",
            conn::NET_KIND,
            json!({"name": "SDA", "class": "x", "pair": {"with": "y", "polarity": "p"}}),
        );
        put(
            "blocks/top/nets/vcc.json",
            conn::NET_KIND,
            json!({"name": "GND"}),
        );
        put(
            "blocks/top/nets/r1.json",
            conn::NET_KIND,
            json!({"name": "R1NET"}),
        );
        put(
            "blocks/top/instances/ch_c.json",
            conn::INSTANCE_KIND,
            json!({"of": "s", "ports": {"nope": "sda"}, "overrides": {"u9": "U9", "u1": "U7"}}),
        );
        put(
            "blocks/top/instances/ch_d.json",
            conn::INSTANCE_KIND,
            json!({"of": "nothing"}),
        );
        put(
            "designs/e.json",
            conn::DESIGN_KIND,
            json!({"name": "E", "root": "nothing"}),
        );
        put("blocks/top/notes.txt", "x", json!({}));
        let c = judged(&f);
        let found = codes(&c);
        let has = |code: Code, path: &str, ptr: &str| {
            assert!(
                found.contains(&(code, path.to_owned(), ptr.to_owned())),
                "{code:?} {path} {ptr}: {found:#?}"
            );
        };
        let r2 = "blocks/top/components/r2.json";
        has(Code::NetNotFound, r2, "/pins/1");
        has(Code::NotFound, r2, "/pins/2");
        has(Code::PortNotFound, r2, "/pins/3");
        has(Code::NotFound, r2, "/of");
        let dup = "blocks/top/nets/dup.json";
        has(Code::NetNameDuplicate, dup, "/name");
        has(Code::NetNameDuplicate, "blocks/top/nets/sda.json", "/name");
        has(Code::NotFound, dup, "/class");
        has(Code::NetNotFound, dup, "/pair/with");
        has(Code::NetNameDuplicate, "blocks/top/nets/vcc.json", "/name");
        has(Code::DuplicateId, "blocks/top/nets/r1.json", "");
        let ch_c = "blocks/top/instances/ch_c.json";
        has(Code::PortNotFound, ch_c, "/ports/nope");
        has(Code::NotFound, ch_c, "/overrides/u9");
        has(Code::NotFound, "blocks/top/instances/ch_d.json", "/of");
        has(Code::NotFound, "designs/e.json", "/root");
        has(Code::InvalidDocument, "blocks/top/notes.txt", "");
        assert!(!found.iter().any(|(_, _, p)| p == "/overrides/u1"));
        assert!(!found.iter().any(|(_, _, p)| p == "/pins/4"));
    }

    #[test]
    fn blocks_that_contain_themselves_are_cycles() {
        let mut f = files();
        f.insert(
            "blocks/s/instances/back.json".into(),
            file(conn::INSTANCE_KIND, json!({"of": "top"})),
        );
        let c = judged(&f);
        let cycles: Vec<Fault> = c
            .faults()
            .into_iter()
            .filter(|f| f.error.code == Code::BlockCycle)
            .collect();
        // Each instance closing the cycle: two in `top`, one in `s`.
        assert_eq!(cycles.len(), 3);
        assert_eq!(cycles[0].error.details["chain"], json!(["s", "top"]));
        // A block reached twice is listed once, and the walk ends.
        assert_eq!(c.reached(&id("top")), [&id("top"), &id("s")]);
    }

    #[test]
    fn revisions_follow_their_own_files() {
        let a = judged(&files());
        let mut f = files();
        f.insert(
            "blocks/s/nets/n.json".into(),
            file(conn::NET_KIND, json!({"name": "N"})),
        );
        let b = judged(&f);
        assert_eq!(a.blocks[&id("top")].revision, b.blocks[&id("top")].revision);
        assert_ne!(a.blocks[&id("s")].revision, b.blocks[&id("s")].revision);
        assert_eq!(a.top, b.top);
        f.insert(
            "classes/hs.json".into(),
            file(conn::CLASS_KIND, json!({"name": "HS"})),
        );
        assert_ne!(judged(&f).top, a.top);
        // A folder an undone block left behind holds no block.
        let empty: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        assert!(load(&empty).unwrap().blocks.is_empty());
    }
}
