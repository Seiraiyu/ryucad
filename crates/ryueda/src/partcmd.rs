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
