//! Requests on what a block holds: components and what their pins
//! connect to, nets and instances. Each write is one logged, undoable
//! change guarded by its block's revision.

use crate::conn::{self, Component, Node, refdes_ok, refdes_parts};
use crate::design::{BlockFiles, Part, part_path};
use crate::error::{Code, Error, Result};
use crate::fault;
use crate::id::Id;
use crate::libcmd::target;
use crate::libcomp::{LibComp, LibRef};
use crate::library::ObjKind;
use crate::project::{Project, SELF};
use crate::server::check_revision;
use crate::session::Session;
use crate::symbol::Symbol;
use crate::write::{self, Writes, commit, new_id, node_text};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// A library component reference as a request gives it (`<id>` or
/// `lib/<alias>/libcomp/<id>`), as a design's file writes it.
pub fn libcomp_ref(text: &str) -> Result<LibRef> {
    let (alias, id) = target(text, ObjKind::Libcomp)?;
    let library = (alias != SELF)
        .then(|| Id::parse(&alias, "library alias"))
        .transpose()?;
    Ok(LibRef { library, id })
}

/// The library a design's reference reaches.
fn alias_of(r: &LibRef) -> &str {
    r.library.as_ref().map_or(SELF, Id::as_str)
}

/// The library component a component places, and its symbol.
pub fn placed<'a>(p: &'a Project, of: &LibRef) -> Result<(&'a LibComp, &'a Symbol)> {
    let lc = p.libcomp(SELF, of)?;
    Ok((lc, p.symbol(alias_of(of), &lc.symbol)?))
}

/// The footprint `text` names among those `lc` (in library `alias`)
/// lists, as it lists it: by `lib/<alias>/footprint/<id>`, or by `<id>`
/// alone when one footprint has it.
fn footprint_of(lc: &LibComp, alias: &str, text: &str) -> Result<LibRef> {
    let (named, id) = target(text, ObjKind::Footprint)?;
    let bare = !text.contains('/');
    let hits: Vec<&LibRef> = lc
        .footprints
        .iter()
        .map(|u| &u.footprint)
        .filter(|f| {
            let in_alias = f.library.as_ref().map_or(alias, Id::as_str);
            f.id == id && (bare || in_alias == named)
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        _ => Err(Error::new(
            Code::NotFound,
            format!("the library component has no footprint {text}"),
        )
        .with("footprint", text)),
    }
}

/// The smallest `<prefix><n>` no component of the project has.
pub fn next_refdes(p: &Project, prefix: &str) -> String {
    let taken: std::collections::BTreeSet<u64> = p
        .conn
        .blocks
        .values()
        .flat_map(|b| b.components.values())
        .filter_map(|c| c.item.as_ref().ok()?.refdes.as_deref())
        .filter_map(|r| {
            let (pre, n, rest) = refdes_parts(r);
            (pre == prefix && rest.is_empty() && n > 0).then_some(n)
        })
        .collect();
    let n = (1u64..)
        .find(|n| !taken.contains(n))
        .expect("some n is free");
    format!("{prefix}{n}")
}

fn check_refdes(r: &str) -> Result<()> {
    if refdes_ok(r) {
        Ok(())
    } else {
        Err(Error::new(
            Code::InvalidParam,
            format!("{r:?} isn't a reference designator: a letter, then letters, digits and _"),
        )
        .with("param", "refdes"))
    }
}

/// Merges `params` over `into`: a string sets, `null` removes.
fn merge_params(into: &mut BTreeMap<String, String>, params: &Map<String, Value>) -> Result<()> {
    for (k, v) in params {
        match v {
            Value::Null => {
                into.remove(k);
            }
            Value::String(s) => {
                into.insert(k.clone(), s.clone());
            }
            v => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("parameter {k} is text (\"50 V\", \"X7R\"), not {v}"),
                )
                .with("param", format!("parameters/{k}")));
            }
        }
    }
    Ok(())
}

/// A component's answer: its file, contents, pins as references, faults
/// and its block's revision.
pub fn component_json(b: &BlockFiles, id: &Id) -> Result<Value> {
    let at = format!("{}/{id}", b.id);
    let l = b.components.get(id).ok_or_else(|| {
        Error::new(
            Code::NotFound,
            format!("block {} has no component {id}", b.id),
        )
        .with("reference", &at)
    })?;
    let c = l
        .item
        .as_ref()
        .map_err(|e| e.clone().with("reference", &at))?;
    let pins: Map<String, Value> = c
        .pins
        .iter()
        .map(|(pin, n)| (pin.clone(), json!(node_text(&b.id, n))))
        .collect();
    Ok(json!({
        "reference": at,
        "path": part_path(&b.id, Part::Component, id),
        "component": c,
        "connections": pins,
        "faults": fault::list(&l.faults),
        "revision": b.revision,
    }))
}

/// What a request gives of a component; an edit keeps what it leaves
/// out, and an empty text clears a field.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentFields {
    /// `<id>` or `lib/<alias>/libcomp/<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refdes: Option<String>,
    /// One of the library component's footprints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Merged into the parameters; `null` removes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Map<String, Value>>,
}

/// Writes `f` over `c`, as placed in the project.
fn fill(p: &Project, c: &mut Component, f: &ComponentFields) -> Result<()> {
    if let Some(of) = &f.of {
        c.of = libcomp_ref(of).map_err(|e| e.with("param", "of"))?;
        p.libcomp(SELF, &c.of).map_err(|e| e.with("param", "of"))?;
        // A footprint of the old component isn't the new one's.
        c.footprint = None;
    }
    if let Some(fp) = &f.footprint {
        c.footprint = match fp.as_str() {
            "" => None,
            text => {
                let lc = p.libcomp(SELF, &c.of)?;
                Some(
                    footprint_of(lc, alias_of(&c.of), text)
                        .map_err(|e| e.with("param", "footprint"))?,
                )
            }
        };
    }
    if let Some(r) = &f.refdes {
        c.refdes = match r.as_str() {
            "" => None,
            r => {
                check_refdes(r)?;
                Some(r.to_owned())
            }
        };
    }
    write::set_opt(&mut c.value, &f.value);
    if let Some(params) = &f.parameters {
        merge_params(&mut c.parameters, params)?;
    }
    Ok(())
}

/// `component.add`: a library component placed in a block, its reference
/// designator its prefix and the project's next free number unless given.
pub fn component_add(
    session: &mut Session,
    block: &str,
    id: Option<&str>,
    fields: &ComponentFields,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let b = write::block(p, block)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let of_text = fields.of.as_deref().ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            "a component names its library component",
        )
        .with("param", "of")
    })?;
    let of = libcomp_ref(of_text).map_err(|e| e.with("param", "of"))?;
    let lc = p.libcomp(SELF, &of).map_err(|e| e.with("param", "of"))?;
    let id = new_id(id, "component", |s| b.has(s))?;
    let mut c = Component {
        of,
        refdes: None,
        footprint: None,
        value: None,
        parameters: BTreeMap::new(),
        pins: BTreeMap::new(),
    };
    fill(
        p,
        &mut c,
        &ComponentFields {
            of: None,
            ..fields.clone()
        },
    )?;
    if c.refdes.is_none() && fields.refdes.is_none() {
        c.refdes = Some(next_refdes(p, &lc.prefix));
    }
    let bid = b.id.clone();
    let mut w = Writes::default();
    w.put(
        part_path(&bid, Part::Component, &id),
        &c,
        conn::COMPONENT_KIND,
    );
    commit(session, &format!("component {bid}/{id}"), w)?;
    component_json(session.project().conn.block(&bid)?, &id)
}

/// The component a request names, as read.
fn component<'a>(p: &'a Project, text: &str) -> Result<(&'a BlockFiles, Id, Component)> {
    let (b, id) = write::in_block(p, text, "component")?;
    let c = b
        .components
        .get(&id)
        .ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("block {} has no component {id}", b.id),
            )
            .with("reference", text)
        })?
        .item
        .clone()
        .map_err(|e| e.with("reference", text))?;
    Ok((b, id, c))
}

/// `component.edit`.
pub fn component_edit(
    session: &mut Session,
    target: &str,
    fields: &ComponentFields,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let (b, id, mut c) = component(p, target)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    fill(p, &mut c, fields)?;
    let bid = b.id.clone();
    let mut w = Writes::default();
    w.put(
        part_path(&bid, Part::Component, &id),
        &c,
        conn::COMPONENT_KIND,
    );
    commit(session, &format!("component {bid}/{id}"), w)?;
    component_json(session.project().conn.block(&bid)?, &id)
}

/// `component.delete`: its pins leave their nets.
pub fn component_delete(
    session: &mut Session,
    target: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let (b, id, _) = component(p, target)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let bid = b.id.clone();
    let mut w = Writes::default();
    w.delete(part_path(&bid, Part::Component, &id));
    commit(session, &format!("component {bid}/{id}"), w)?;
    Ok(json!({
        "deleted": format!("{bid}/{id}"),
        "revision": session.project().conn.block(&bid)?.revision,
    }))
}

pub fn component_show(p: &Project, target: &str) -> Result<Value> {
    let (b, id) = write::in_block(p, target, "component")?;
    component_json(b, &id)
}

/// What a connection request does to a pin.
pub enum Wire<'a> {
    /// To a net (by ID or name), `rail:<…>`, `port:<…>` or `nc`; with
    /// `new`, a new net of the block by that name.
    To {
        net: &'a str,
        new: bool,
    },
    Nc,
    Off,
}

/// `connect`, `nc` and `disconnect`: one pin's connection.
pub fn wire(session: &mut Session, pin: &str, how: Wire, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    let (b, cid, number) = write::pin(p, pin)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let mut c = b.components[&cid]
        .item
        .clone()
        .map_err(|e| e.with("reference", pin))?;
    let bid = b.id.clone();
    let mut w = Writes::default();
    if !matches!(how, Wire::Off) {
        let (_, symbol) = placed(p, &c.of)?;
        if !symbol.pins.contains_key(&number) {
            return Err(Error::new(
                Code::PinNotFound,
                format!("{bid}/{cid}'s symbol has no pin {number}"),
            )
            .with("pin", pin));
        }
    }
    let node = match how {
        Wire::Off => None,
        Wire::Nc => Some(Node::Nc),
        Wire::To { net, new: false } => Some(write::node(p, b, net)?),
        Wire::To { net, new: true } => {
            let nid = new_id(None, "net", |s| b.has(s))?;
            let n = conn::Net {
                name: net.to_owned(),
                class: None,
                voltage: None,
                current: None,
                impedance: None,
                pair: None,
            };
            w.put(part_path(&bid, Part::Net, &nid), &n, conn::NET_KIND);
            Some(Node::Net(nid))
        }
    };
    let before = c.pins.get(&number).cloned();
    match &node {
        Some(n) => c.pins.insert(number.clone(), n.clone()),
        None => c.pins.remove(&number),
    };
    if before != node {
        w.put(
            part_path(&bid, Part::Component, &cid),
            &c,
            conn::COMPONENT_KIND,
        );
        commit(session, &format!("pin {pin}"), w)?;
    }
    let b = session.project().conn.block(&bid)?;
    Ok(json!({
        "pin": pin,
        "net": node.as_ref().map(|n| node_text(&bid, n)),
        "changed": before != node,
        "revision": b.revision,
    }))
}

/// A net's answer: its file, contents, what names it, faults and its
/// block's revision.
pub fn net_json(p: &Project, b: &BlockFiles, id: &Id) -> Result<Value> {
    let at = format!("{}/{id}", b.id);
    let l = b.nets.get(id).ok_or_else(|| {
        Error::new(Code::NetNotFound, format!("block {} has no net {id}", b.id))
            .with("reference", &at)
    })?;
    let n = l
        .item
        .as_ref()
        .map_err(|e| e.clone().with("reference", &at))?;
    Ok(json!({
        "reference": at,
        "path": part_path(&b.id, Part::Net, id),
        "net": n,
        "pins": write::users(p, &Node::Net(id.clone()), Some(&b.id)),
        "faults": fault::list(&l.faults),
        "revision": b.revision,
    }))
}

/// The net a request names (`<block>/<net>`, the net by ID or name), as
/// read.
fn net<'a>(p: &'a Project, text: &str) -> Result<(&'a BlockFiles, Id, conn::Net)> {
    let Some((block, name)) = text.split_once('/') else {
        return Err(
            Error::new(Code::InvalidParam, "name a net as <block>/<net>").with("reference", text),
        );
    };
    let b = write::block(p, block)?;
    let id = b.net(name).cloned().ok_or_else(|| {
        Error::new(
            Code::NetNotFound,
            format!("block {block} has no net {name}"),
        )
        .with("reference", text)
    })?;
    let n = b.nets[&id]
        .item
        .clone()
        .map_err(|e| e.with("reference", text))?;
    Ok((b, id, n))
}

/// `net.add`: a net of a block, with its intent.
pub fn net_add(
    session: &mut Session,
    block: &str,
    name: &str,
    id: Option<&str>,
    intent: &crate::topcmd::IntentArgs,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let b = write::block(p, block)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let id = new_id(id, "net", |s| b.has(s))?;
    let mut n = conn::Net {
        name: name.to_owned(),
        class: None,
        voltage: None,
        current: None,
        impedance: None,
        pair: None,
    };
    crate::topcmd::fill_intent(
        p,
        intent,
        &mut n.class,
        [&mut n.voltage, &mut n.current, &mut n.impedance],
    )?;
    let bid = b.id.clone();
    let mut w = Writes::default();
    w.put(part_path(&bid, Part::Net, &id), &n, conn::NET_KIND);
    commit(session, &format!("net {bid}/{name}"), w)?;
    let p = session.project();
    net_json(p, p.conn.block(&bid)?, &id)
}

/// `net.edit`: a net's class and intent, and its pair: `pair_with` a net
/// of the block (both then name each other, the other the opposite
/// polarity), or empty for none.
pub fn net_edit(
    session: &mut Session,
    target: &str,
    intent: &crate::topcmd::IntentArgs,
    pair_with: Option<&str>,
    polarity: Option<conn::Polarity>,
    expected: Option<&str>,
) -> Result<Value> {
    use conn::{Pair, Polarity};
    let p = session.project();
    let (b, id, mut n) = net(p, target)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let bid = b.id.clone();
    crate::topcmd::fill_intent(
        p,
        intent,
        &mut n.class,
        [&mut n.voltage, &mut n.current, &mut n.impedance],
    )?;
    // Nets whose pair changes, besides this one.
    let mut others: BTreeMap<Id, conn::Net> = BTreeMap::new();
    let read = |x: &Id, others: &BTreeMap<Id, conn::Net>| -> Result<conn::Net> {
        match others.get(x) {
            Some(n) => Ok(n.clone()),
            None => b
                .nets
                .get(x)
                .ok_or_else(|| {
                    Error::new(Code::NetNotFound, format!("block {bid} has no net {x}"))
                })?
                .item
                .clone(),
        }
    };
    // `x` loses its pair if that pair is `with`.
    let clear = |x: &Id, with: &Id, others: &mut BTreeMap<Id, conn::Net>| -> Result<()> {
        let mut o = read(x, others)?;
        if o.pair.as_ref().is_some_and(|p| &p.with == with) {
            o.pair = None;
            others.insert(x.clone(), o);
        }
        Ok(())
    };
    if pair_with.is_some() || polarity.is_some() {
        let old = n.pair.take();
        let with = match pair_with {
            Some("") => None,
            Some(text) => Some(b.net(text).cloned().ok_or_else(|| {
                Error::new(Code::NetNotFound, format!("block {bid} has no net {text}"))
                    .with("param", "pair_with")
            })?),
            None => old.as_ref().map(|x| x.with.clone()),
        };
        let polarity = polarity
            .or(old.as_ref().map(|x| x.polarity))
            .unwrap_or(Polarity::P);
        if let Some(old) = &old
            && Some(&old.with) != with.as_ref()
        {
            clear(&old.with, &id, &mut others)?;
        }
        if let Some(with) = with {
            if with == id {
                return Err(
                    Error::new(Code::InvalidParam, "a net pairs with another net")
                        .with("param", "pair_with"),
                );
            }
            let mut o = read(&with, &others)?;
            if let Some(op) = &o.pair
                && op.with != id
            {
                clear(&op.with, &with, &mut others)?;
            }
            o.pair = Some(Pair {
                with: id.clone(),
                polarity: match polarity {
                    Polarity::P => Polarity::N,
                    Polarity::N => Polarity::P,
                },
            });
            n.pair = Some(Pair {
                with: with.clone(),
                polarity,
            });
            others.insert(with, o);
        }
    }
    let mut w = Writes::default();
    others.remove(&id);
    w.put(part_path(&bid, Part::Net, &id), &n, conn::NET_KIND);
    for (oid, o) in &others {
        w.put(part_path(&bid, Part::Net, oid), o, conn::NET_KIND);
    }
    commit(session, &format!("net {bid}/{id}"), w)?;
    let p = session.project();
    net_json(p, p.conn.block(&bid)?, &id)
}

/// `net.rename`: one file.
pub fn net_rename(
    session: &mut Session,
    target: &str,
    name: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let (b, id, mut n) = net(p, target)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let bid = b.id.clone();
    n.name = name.to_owned();
    let mut w = Writes::default();
    w.put(part_path(&bid, Part::Net, &id), &n, conn::NET_KIND);
    commit(session, &format!("net {bid}/{id}"), w)?;
    let p = session.project();
    net_json(p, p.conn.block(&bid)?, &id)
}

/// What else names net `id` of `b` besides pins and ports: its buses and
/// its pair's other net.
fn net_refs(b: &BlockFiles, id: &Id) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(block) = &b.block.item {
        for (bus, x) in &block.buses {
            if x.nets.contains(id) {
                out.push(format!("{}/bus:{bus}", b.id));
            }
        }
    }
    for (nid, n) in &b.nets {
        if n.item
            .as_ref()
            .is_ok_and(|n| n.pair.as_ref().is_some_and(|x| &x.with == id))
        {
            out.push(format!("{}/{nid}", b.id));
        }
    }
    out
}

/// `net.join`: every pin and port on `net` moves to `into` (a net of the
/// same block, or `rail:<name>`), its buses take `into` (or drop it, for a
/// rail), and `net` goes, in one change.
pub fn net_join(
    session: &mut Session,
    target: &str,
    into: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let (b, id, _) = net(p, target)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let bid = b.id.clone();
    let to = write::node(p, b, into)?;
    if to == Node::Net(id.clone()) || to == Node::Nc {
        return Err(Error::new(
            Code::InvalidParam,
            "join a net into another net of its block or a rail",
        )
        .with("param", "into"));
    }
    if let Node::Port(_) = to {
        return Err(Error::new(
            Code::InvalidParam,
            "a port isn't a net to join into: connect the pins to it instead",
        )
        .with("param", "into"));
    }
    let paired: Vec<String> = b
        .nets
        .iter()
        .filter(|(_, n)| {
            n.item
                .as_ref()
                .is_ok_and(|n| n.pair.as_ref().is_some_and(|x| x.with == id))
        })
        .map(|(nid, _)| format!("{bid}/{nid}"))
        .collect();
    if !paired.is_empty() {
        return Err(write::has_dependents(
            &format!("net {bid}/{id}, a pair's half,"),
            &paired,
        ));
    }
    let from = Node::Net(id.clone());
    let mut w = Writes::default();
    let mut moved = Vec::new();
    for (cid, c) in &b.components {
        let Ok(c) = &c.item else { continue };
        if c.pins.values().any(|n| *n == from) {
            let mut c = c.clone();
            for (pin, n) in c.pins.iter_mut() {
                if *n == from {
                    *n = to.clone();
                    moved.push(format!("{bid}/{cid}/{pin}"));
                }
            }
            w.put(
                part_path(&bid, Part::Component, cid),
                &c,
                conn::COMPONENT_KIND,
            );
        }
    }
    for (iid, i) in &b.instances {
        let Ok(i) = &i.item else { continue };
        if i.ports.values().any(|n| *n == from) {
            let mut i = i.clone();
            for (port, n) in i.ports.iter_mut() {
                if *n == from {
                    *n = to.clone();
                    moved.push(format!("{bid}/{iid}/{port}"));
                }
            }
            w.put(
                part_path(&bid, Part::Instance, iid),
                &i,
                conn::INSTANCE_KIND,
            );
        }
    }
    if let Ok(block) = &b.block.item
        && block.buses.values().any(|x| x.nets.contains(&id))
    {
        let mut block = block.clone();
        for x in block.buses.values_mut() {
            let mut nets = Vec::new();
            for n in &x.nets {
                let n = match (n == &id, &to) {
                    (true, Node::Net(t)) => t.clone(),
                    (true, _) => continue,
                    (false, _) => n.clone(),
                };
                if !nets.contains(&n) {
                    nets.push(n);
                }
            }
            x.nets = nets;
        }
        w.put(crate::design::block_path(&bid), &block, conn::BLOCK_KIND);
    }
    w.delete(part_path(&bid, Part::Net, &id));
    commit(session, &format!("net {bid}/{id}"), w)?;
    let b = session.project().conn.block(&bid)?;
    Ok(json!({
        "joined": format!("{bid}/{id}"),
        "into": node_text(&bid, &to),
        "moved": moved,
        "revision": b.revision,
    }))
}

/// `net.delete`: only a net nothing names.
pub fn net_delete(session: &mut Session, target: &str, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    let (b, id, _) = net(p, target)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let bid = b.id.clone();
    let pins = write::users(p, &Node::Net(id.clone()), Some(&bid));
    if !pins.is_empty() {
        return Err(Error::new(
            Code::NetHasPins,
            format!(
                "net {bid}/{id} still has {} pin(s) or port(s): disconnect them, or join it into another net",
                pins.len()
            ),
        )
        .with("pins", pins));
    }
    let refs = net_refs(b, &id);
    if !refs.is_empty() {
        return Err(write::has_dependents(&format!("net {bid}/{id}"), &refs));
    }
    let mut w = Writes::default();
    w.delete(part_path(&bid, Part::Net, &id));
    commit(session, &format!("net {bid}/{id}"), w)?;
    Ok(json!({
        "deleted": format!("{bid}/{id}"),
        "revision": session.project().conn.block(&bid)?.revision,
    }))
}

pub fn net_show(p: &Project, target: &str) -> Result<Value> {
    let (b, id, _) = net(p, target)?;
    net_json(p, b, &id)
}

/// An instance's answer: its file, contents, ports as references, faults
/// and its block's revision.
pub fn instance_json(b: &BlockFiles, id: &Id) -> Result<Value> {
    let at = format!("{}/{id}", b.id);
    let l = b.instances.get(id).ok_or_else(|| {
        Error::new(
            Code::NotFound,
            format!("block {} has no instance {id}", b.id),
        )
        .with("reference", &at)
    })?;
    let i = l
        .item
        .as_ref()
        .map_err(|e| e.clone().with("reference", &at))?;
    let ports: Map<String, Value> = i
        .ports
        .iter()
        .map(|(port, n)| (port.to_string(), json!(node_text(&b.id, n))))
        .collect();
    Ok(json!({
        "reference": at,
        "path": part_path(&b.id, Part::Instance, id),
        "instance": i,
        "connections": ports,
        "faults": fault::list(&l.faults),
        "revision": b.revision,
    }))
}

/// Sets an instance's ports from a request's: each port of the placed
/// block (by ID or name) to a node of `b` as text, or `null` to leave it
/// unconnected.
fn set_ports(
    p: &Project,
    b: &BlockFiles,
    child: &Id,
    ports: &Map<String, Value>,
    into: &mut BTreeMap<Id, Node>,
) -> Result<()> {
    let child = p.conn.block(child)?;
    let block = child
        .block
        .item
        .as_ref()
        .map_err(|e| e.clone().with("block", &child.id))?;
    for (name, v) in ports {
        let port = block.port(name).cloned().ok_or_else(|| {
            Error::new(
                Code::PortNotFound,
                format!("block {} has no port {name}", child.id),
            )
            .with("param", format!("ports/{name}"))
        })?;
        match v {
            Value::Null => {
                into.remove(&port);
            }
            Value::String(text) => {
                let n = write::node(p, b, text)
                    .map_err(|e| e.with("param", format!("ports/{name}")))?;
                into.insert(port, n);
            }
            v => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("port {name} connects to a net as text, or null, not {v}"),
                )
                .with("param", format!("ports/{name}")));
            }
        }
    }
    Ok(())
}

/// `instance.add`: a block placed in another.
pub fn instance_add(
    session: &mut Session,
    block: &str,
    of: &str,
    id: Option<&str>,
    ports: &Map<String, Value>,
    refdes: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let b = write::block(p, block)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let child = write::block(p, of)
        .map_err(|e| e.with("param", "of"))?
        .id
        .clone();
    let id = new_id(id, "instance", |s| b.has(s))?;
    let mut i = conn::Instance {
        of: child.clone(),
        ports: BTreeMap::new(),
        refdes: refdes.map(|r| conn::RefdesFormat(crate::props::preset(r).to_owned())),
        properties: BTreeMap::new(),
        overrides: BTreeMap::new(),
    };
    set_ports(p, b, &child, ports, &mut i.ports)?;
    let bid = b.id.clone();
    let mut w = Writes::default();
    w.put(
        part_path(&bid, Part::Instance, &id),
        &i,
        conn::INSTANCE_KIND,
    );
    commit(session, &format!("instance {bid}/{id}"), w)?;
    instance_json(session.project().conn.block(&bid)?, &id)
}

/// What `instance.edit` changes; what it leaves out stays.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceFields {
    /// By port: a node as text, or `null` to unconnect.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub ports: Map<String, Value>,
    /// A designator format (`"{refdes}_{stack_letter}"`) or a preset's
    /// name (`suffix_letter`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refdes: Option<String>,
    /// Drop the designator format.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_refdes: bool,
    /// By occurrence path below the instance: a reference designator, or
    /// `null` to drop the override.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub overrides: Map<String, Value>,
    /// This placement's properties: text sets, `null` removes.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub properties: Map<String, Value>,
}

/// The instance a request names, as read.
fn instance<'a>(p: &'a Project, text: &str) -> Result<(&'a BlockFiles, Id, conn::Instance)> {
    let (b, id) = write::in_block(p, text, "instance")?;
    let i = b
        .instances
        .get(&id)
        .ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("block {} has no instance {id}", b.id),
            )
            .with("reference", text)
        })?
        .item
        .clone()
        .map_err(|e| e.with("reference", text))?;
    Ok((b, id, i))
}

/// `instance.edit`.
pub fn instance_edit(
    session: &mut Session,
    target: &str,
    f: &InstanceFields,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let (b, id, mut i) = instance(p, target)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    set_ports(p, b, &i.of.clone(), &f.ports, &mut i.ports)?;
    if f.no_refdes {
        i.refdes = None;
    }
    if let Some(r) = &f.refdes {
        i.refdes = Some(conn::RefdesFormat(crate::props::preset(r).to_owned()));
    }
    crate::props::apply(&mut i.properties, &f.properties)?;
    for (path, v) in &f.overrides {
        match v {
            Value::Null => {
                i.overrides.remove(path);
            }
            Value::String(r) => {
                check_refdes(r)?;
                i.overrides.insert(path.clone(), r.clone());
            }
            v => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("an override is a reference designator or null, not {v}"),
                )
                .with("param", format!("overrides/{path}")));
            }
        }
    }
    let bid = b.id.clone();
    let mut w = Writes::default();
    w.put(
        part_path(&bid, Part::Instance, &id),
        &i,
        conn::INSTANCE_KIND,
    );
    commit(session, &format!("instance {bid}/{id}"), w)?;
    instance_json(session.project().conn.block(&bid)?, &id)
}

/// `instance.delete`.
pub fn instance_delete(
    session: &mut Session,
    target: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    let (b, id, _) = instance(p, target)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let bid = b.id.clone();
    let mut w = Writes::default();
    w.delete(part_path(&bid, Part::Instance, &id));
    commit(session, &format!("instance {bid}/{id}"), w)?;
    Ok(json!({
        "deleted": format!("{bid}/{id}"),
        "revision": session.project().conn.block(&bid)?.revision,
    }))
}

pub fn instance_show(p: &Project, target: &str) -> Result<Value> {
    let (b, id) = write::in_block(p, target, "instance")?;
    instance_json(b, &id)
}
