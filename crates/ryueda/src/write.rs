//! How connectivity is written. A request works out the files it would
//! change; the project is read again over them in memory, and the write
//! goes to disk only if that adds no error to any file (by code, path and
//! pointer): a pin naming a net that isn't there, a duplicate name, a
//! cycle. Electrical judgement is ERC's, never a write's.

use crate::conn::{self, NC, Node};
use crate::design::BlockFiles;
use crate::doc;
use crate::error::{Code, Error, Result};
use crate::fault::{self, Fault};
use crate::id::Id;
use crate::project::{Project, resolve};
use crate::session::{Session, path_of};
use crate::source::write_atomic;
use serde::Serialize;
use std::sync::Arc;

/// The files a write changes: each path's new text, or `None` to delete
/// it.
#[derive(Default, Debug)]
pub struct Writes(pub Vec<(String, Option<String>)>);

impl Writes {
    /// Writes `t` as a `kind` at `path`.
    pub fn put<T: Serialize>(&mut self, path: String, t: &T, kind: &str) {
        let v = serde_json::to_value(t).expect("file types serialize");
        let format = conn::format_of(kind, &v);
        self.0.push((path, Some(doc::to_string(&v, kind, format))));
    }

    pub fn delete(&mut self, path: String) {
        self.0.push((path, None));
    }

    /// The paths written or deleted, in order.
    pub fn paths(&self) -> Vec<&str> {
        self.0.iter().map(|(p, _)| p.as_str()).collect()
    }
}

/// The errors in `after` that `before` doesn't have, by code, file and
/// place.
pub fn added_errors(before: &[Fault], after: &[Fault]) -> Vec<Fault> {
    let key = |f: &Fault| {
        (
            f.error.code,
            f.error.details.get("path").cloned(),
            f.error.details.get("pointer").cloned(),
        )
    };
    after
        .iter()
        .filter(|f| f.is_error() && !before.iter().any(|b| key(b) == key(f)))
        .cloned()
        .collect()
}

/// The refusal of a write that would add `errors`: the first's code, with
/// every one in the details.
pub fn refused(what: &str, errors: &[Fault]) -> Error {
    let first = &errors[0].error;
    Error::new(
        first.code,
        format!(
            "{what} would leave {} fault(s): {}",
            errors.len(),
            first.message
        ),
    )
    .with("faults", fault::list(errors))
}

/// The project as it would be after `writes`, read from the session's
/// files with them applied.
pub fn proposed(session: &Session, writes: &Writes) -> Result<Project> {
    let mut files = session.snapshot();
    for (path, text) in &writes.0 {
        match text {
            Some(t) => {
                files.insert(path.clone(), Arc::from(t.as_bytes()));
            }
            None => {
                files.remove(path);
            }
        }
    }
    let aliases = resolve(session.root(), &session.project().manifest);
    Project::load_from(session.root(), &files, &aliases)
}

/// Puts `writes` on disk if they add no error to the project, and takes
/// them in; refused with the first new fault's code otherwise.
pub fn commit(session: &mut Session, what: &str, writes: Writes) -> Result<()> {
    let after = proposed(session, &writes)?;
    let added = added_errors(&session.project().faults(), &after.faults());
    if !added.is_empty() {
        return Err(refused(what, &added));
    }
    let root = session.root().to_path_buf();
    let written: Vec<String> = writes.paths().into_iter().map(str::to_owned).collect();
    for (path, text) in writes.0 {
        let at = path_of(&root, &path);
        match text {
            Some(t) => write_atomic(&at, t)?,
            None if at.exists() => crate::activity::remove_file(&root, &at)?,
            None => {}
        }
    }
    // The project just judged is the project on disk now.
    session.adopt(after, &written)
}

/// A block a request names, or `not_found`.
pub fn block<'a>(p: &'a Project, text: &str) -> Result<&'a BlockFiles> {
    let id = Id::parse(text, "block")?;
    p.conn.block(&id)
}

/// A block and an object in it, from `<block>/<id>`.
pub fn in_block<'a>(p: &'a Project, text: &str, what: &str) -> Result<(&'a BlockFiles, Id)> {
    let Some((b, id)) = text.split_once('/') else {
        return Err(Error::new(
            Code::InvalidParam,
            format!("name a {what} as <block>/<{what}>"),
        )
        .with("reference", text));
    };
    Ok((block(p, b)?, Id::parse(id, what)?))
}

/// A pin, from `<block>/<component>/<pin>`.
pub fn pin<'a>(p: &'a Project, text: &str) -> Result<(&'a BlockFiles, Id, String)> {
    match text.splitn(3, '/').collect::<Vec<_>>().as_slice() {
        [b, c, pin] => {
            let block = block(p, b)?;
            let comp = Id::parse(c, "component")?;
            if !block.components.contains_key(&comp) {
                return Err(
                    Error::new(Code::NotFound, format!("block {b} has no component {c}"))
                        .with("reference", text),
                );
            }
            Ok((block, comp, (*pin).to_owned()))
        }
        _ => Err(Error::new(
            Code::InvalidParam,
            "name a pin as <block>/<component>/<pin>",
        )
        .with("reference", text)),
    }
}

/// What a request's text names in `b`: `nc`, `rail:<id or name>`,
/// `port:<id or name>`, or a net of the block by ID or name.
pub fn node(p: &Project, b: &BlockFiles, text: &str) -> Result<Node> {
    if text == NC {
        return Ok(Node::Nc);
    }
    if let Some(r) = text.strip_prefix("rail:") {
        return p.conn.rail(r).cloned().map(Node::Rail).ok_or_else(|| {
            Error::new(Code::NotFound, format!("there is no rail {r}")).with("rail", r)
        });
    }
    if let Some(port) = text.strip_prefix("port:") {
        let found = b.block.item.as_ref().ok().and_then(|x| x.port(port));
        return found.cloned().map(Node::Port).ok_or_else(|| {
            Error::new(
                Code::PortNotFound,
                format!("block {} has no port {port}", b.id),
            )
            .with("port", port)
        });
    }
    b.net(text).cloned().map(Node::Net).ok_or_else(|| {
        Error::new(
            Code::NetNotFound,
            format!("block {} has no net {text}; give --new to make one", b.id),
        )
        .with("net", text)
    })
}

/// How output writes a node of block `b`.
pub fn node_text(b: &Id, n: &Node) -> String {
    match n {
        Node::Net(id) => format!("{b}/{id}"),
        Node::Rail(id) => format!("rail/{id}"),
        Node::Port(id) => format!("{b}/port:{id}"),
        Node::Nc => NC.to_owned(),
    }
}

/// An ID for a new object: the one asked for, if free, or a random one.
pub fn new_id(asked: Option<&str>, what: &str, taken: impl Fn(&str) -> bool) -> Result<Id> {
    match asked {
        Some(t) => {
            let id = Id::parse(t, what)?;
            if t == NC && what == "net" {
                return Err(Error::new(
                    Code::InvalidId,
                    format!("no net may take the ID {NC}: it marks a pin not connected"),
                )
                .with("id", t));
            }
            if taken(t) {
                return Err(Error::new(
                    Code::DuplicateId,
                    format!("there is already a {what} {t}"),
                )
                .with("id", t));
            }
            Ok(id)
        }
        None => crate::name::random(taken),
    }
}

/// An empty text clears an optional field; any other sets it.
pub fn set_opt(field: &mut Option<String>, value: &Option<String>) {
    if let Some(v) = value {
        *field = (!v.is_empty()).then(|| v.clone());
    }
}

/// What names `node` in the project: pins (`<block>/<component>/<pin>`)
/// and instances' ports (`<block>/<instance>/<port>`), the first in
/// `within` only when given.
pub fn users(p: &Project, node: &Node, within: Option<&Id>) -> Vec<String> {
    let mut out = Vec::new();
    for (bid, b) in &p.conn.blocks {
        if within.is_some_and(|w| w != bid) {
            continue;
        }
        for (cid, c) in &b.components {
            let Ok(c) = &c.item else { continue };
            for (pin, n) in &c.pins {
                if n == node {
                    out.push(format!("{bid}/{cid}/{pin}"));
                }
            }
        }
        for (iid, i) in &b.instances {
            let Ok(i) = &i.item else { continue };
            for (port, n) in &i.ports {
                if n == node {
                    out.push(format!("{bid}/{iid}/{port}"));
                }
            }
        }
    }
    out
}

/// The refusal of a change to what others still name.
pub fn has_dependents(what: &str, dependents: &[String]) -> Error {
    Error::new(
        Code::HasDependents,
        format!(
            "{} still name {what}; change them first",
            match dependents.len() {
                1 => dependents[0].clone(),
                n => format!("{} and {} more", dependents[0], n - 1),
            }
        ),
    )
    .with("dependents", dependents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_new_by_code_file_and_place() {
        let f = |code, path: &str, ptr: &str| {
            Fault::error(code, "x")
                .with("path", path)
                .with("pointer", ptr)
        };
        let before = [f(Code::NotFound, "a.json", "/of")];
        let after = [
            f(Code::NotFound, "a.json", "/of"),
            f(Code::NotFound, "b.json", "/of"),
            f(Code::NetNotFound, "a.json", "/of"),
            Fault::warning(Code::NetSinglePin, "w"),
        ];
        let added = added_errors(&before, &after);
        assert_eq!(added.len(), 2);
        let e = refused("net n", &added);
        assert_eq!(e.code, Code::NotFound);
        assert_eq!(e.details["faults"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn asked_ids_must_be_free_and_never_nc() {
        assert_eq!(
            new_id(Some("x"), "net", |s| s == "x").unwrap_err().code,
            Code::DuplicateId
        );
        assert_eq!(
            new_id(Some("nc"), "net", |_| false).unwrap_err().code,
            Code::InvalidId
        );
        assert_eq!(
            new_id(Some("nc"), "rail", |_| false).unwrap().as_str(),
            "nc"
        );
        assert_eq!(new_id(None, "net", |_| false).unwrap().as_str().len(), 10);
    }
}
