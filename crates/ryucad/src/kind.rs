//! What a body is: a solid or a sheet (ryucad 18, design 388). A body's
//! kind is the kernel's (`Body::kind()`), read from each state of its
//! chain, never stored. A feature that needs a solid and is given a sheet,
//! or the reverse, is `wrong_body_kind`, when written and when rebuilt.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use ryukei::topo::{Body, BodyKind, VertexId};
use std::collections::BTreeMap;

/// What a feature needs a body to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Solid,
    Sheet,
}

impl Want {
    fn as_str(self) -> &'static str {
        match self {
            Want::Solid => "solid",
            Want::Sheet => "sheet",
        }
    }
}

/// A body's kind as reports write it: `solid`, `sheet`, `empty` or `mixed`.
pub fn name(b: &Body) -> &'static str {
    match b.kind() {
        BodyKind::Solid => "solid",
        BodyKind::Sheet => "sheet",
        BodyKind::Empty => "empty",
        BodyKind::Mixed => "mixed",
    }
}

/// How many rings of laminar edges a sheet has: its boundaries. A sheet's
/// vertices are each one fan of faces (`check`), so two boundaries never
/// share a vertex, and the rings are the laminar edges joined at their
/// vertices. Zero for a closed sheet and for any solid.
pub fn boundaries(b: &Body) -> usize {
    fn root(parent: &mut BTreeMap<VertexId, VertexId>, v: VertexId) -> VertexId {
        let mut r = v;
        loop {
            let p = *parent.entry(r).or_insert(r);
            if p == r {
                break;
            }
            r = p;
        }
        // Halve the path behind us.
        let mut x = v;
        while x != r {
            let next = parent[&x];
            parent.insert(x, r);
            x = next;
        }
        r
    }
    let mut parent: BTreeMap<VertexId, VertexId> = BTreeMap::new();
    for e in b.edges() {
        if !b.is_laminar(e) {
            continue;
        }
        let edge = b.edge(e).expect("a live edge");
        let (rs, rt) = (root(&mut parent, edge.start), root(&mut parent, edge.end));
        if rs != rt {
            parent.insert(rs, rt);
        }
    }
    let vertices: Vec<VertexId> = parent.keys().copied().collect();
    let mut roots: Vec<VertexId> = vertices
        .into_iter()
        .map(|v| root(&mut parent, v))
        .collect();
    roots.sort_unstable();
    roots.dedup();
    roots.len()
}

/// `wrong_body_kind`: `owner` was given body `solid` (by founder) in
/// `role` where it wants `want`, and the body is another kind.
pub fn wrong(owner: &Id, role: &str, solid: &Id, got: &str, want: Want) -> Error {
    let hint = match want {
        Want::Solid => "it acts only on solids; a sheet has no inside to join, cut or round",
        Want::Sheet => "it takes a sheet; a solid's faces can be taken off as a sheet",
    };
    Error::new(
        Code::WrongBodyKind,
        format!(
            "{owner} needs a {} as its {role}, and {solid} is a {got}: {hint}",
            want.as_str()
        ),
    )
    .with("feature", owner)
    .with("role", role)
    .with("solid", solid)
    .with("wanted", want.as_str())
    .with("got", got)
}

/// Refuses `body` (the state of `solid` that `owner` reads) where it
/// isn't `want`.
pub fn need(owner: &Id, role: &str, solid: &Id, body: &Body, want: Want) -> Result<()> {
    let ok = match (want, body.kind()) {
        (Want::Solid, BodyKind::Solid) | (Want::Sheet, BodyKind::Sheet) => true,
        // An empty body is refused where it is made (`body_emptied`).
        (_, BodyKind::Empty) => true,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(wrong(owner, role, solid, name(body), want))
    }
}

/// The bodies feature `f` reads where a solid goes, each with its role:
/// a boolean's target and tools, a mirror's source, every other
/// feature's targets (`target` for a shell, offset and split).
pub fn bodies_read(f: &crate::doc::files::Feature) -> Vec<(&'static str, &Id)> {
    use crate::doc::files::Feature as F;
    match f {
        F::Boolean(b) => std::iter::once(("target", &b.target))
            .chain(b.tools.iter().map(|t| ("tools", t)))
            .collect(),
        F::Mirror(m) => std::iter::once(("solid", &m.solid))
            .chain(f.targets().into_iter().map(|t| ("targets", t)))
            .collect(),
        F::Shell(_) | F::Offset(_) | F::Split(_) => {
            f.targets().into_iter().map(|t| ("target", t)).collect()
        }
        _ => f.targets().into_iter().map(|t| ("targets", t)).collect(),
    }
}

/// Refuses feature `id` (listed in `order` of `part`, as it would be
/// written) that reads a sheet where it needs a solid, in the part's
/// current rebuild. Anything else wrong with what it reads is left to
/// rebuild, as before: only `wrong_body_kind` is raised here.
pub fn check(
    part: &crate::doc::project::Part,
    order: &[Id],
    id: &Id,
    f: &crate::doc::files::Feature,
    results: &dyn crate::rebuild::Results,
) -> Result<()> {
    let solids = crate::solid::Solids::of_order(part, order);
    for (role, t) in bodies_read(f) {
        match crate::rebuild::target_as(part, &solids, id, role, t, results) {
            Err(e) if e.code == Code::WrongBodyKind => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::geom::Rigid;
    use ryukei::name::OpKey;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    fn block() -> Body {
        ryukei::primitive::block(OpKey::from_bytes(b"k"), 1.0, 1.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0
    }

    fn top(b: &Body) -> Body {
        let f = b.faces().next().unwrap();
        let name = b.face(f).unwrap().name;
        ryukei::sheet::extract(OpKey::from_bytes(b"x"), b, &[name])
            .unwrap()
            .0
    }

    #[test]
    fn a_box_is_a_solid_and_its_top_a_sheet_with_one_boundary() {
        let b = block();
        assert_eq!(name(&b), "solid");
        assert_eq!(boundaries(&b), 0);
        let sheet = top(&b);
        assert_eq!(name(&sheet), "sheet");
        assert_eq!(boundaries(&sheet), 1);
        // The whole box as a closed sheet has no boundary.
        let (closed, _) = ryukei::sheet::remove(OpKey::from_bytes(b"y"), &b, &[]).unwrap();
        assert_eq!((name(&closed), boundaries(&closed)), ("sheet", 0));
    }

    #[test]
    fn a_sheet_where_a_solid_goes_is_wrong_body_kind() {
        let b = block();
        let sheet = top(&b);
        assert!(need(&id("c"), "targets", &id("w"), &b, Want::Solid).is_ok());
        let e = need(&id("c"), "targets", &id("w"), &sheet, Want::Solid).unwrap_err();
        assert_eq!(e.code, Code::WrongBodyKind);
        assert_eq!(e.details["wanted"], "solid");
        assert_eq!(e.details["got"], "sheet");
        assert_eq!(e.details["solid"], "w");
        assert_eq!(e.details["role"], "targets");
        assert_eq!(e.details["feature"], "c");
    }
}
