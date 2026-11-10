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
    let mut roots: Vec<VertexId> = vertices.into_iter().map(|v| root(&mut parent, v)).collect();
    roots.sort_unstable();
    roots.dedup();
    roots.len()
}

/// `wrong_body_kind`: `owner` was given body `solid` (by founder) in
/// `role` where it wants `want`, and the body is another kind; `hint`
/// says what to do instead (else a general hint for the kind).
pub fn wrong(
    owner: &Id,
    role: &str,
    solid: &Id,
    got: &str,
    want: Want,
    hint: Option<&str>,
) -> Error {
    let hint = hint.unwrap_or(match want {
        Want::Solid => "it acts only on solids; a sheet has no inside to join or hollow",
        Want::Sheet => "it takes a sheet; a solid's faces can be taken off as a sheet",
    });
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
    .with("hint", hint)
}

/// Refuses `body` (the state of `solid` that `owner` reads) where it
/// isn't `want`, saying `hint` (see [`wrong`]).
pub fn need(
    owner: &Id,
    role: &str,
    solid: &Id,
    body: &Body,
    want: Want,
    hint: Option<&str>,
) -> Result<()> {
    let ok = match (want, body.kind()) {
        (Want::Solid, BodyKind::Solid) | (Want::Sheet, BodyKind::Sheet) => true,
        // An empty body is refused where it is made (`body_emptied`).
        (_, BodyKind::Empty) => true,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(wrong(owner, role, solid, name(body), want, hint))
    }
}

/// One body a feature names: the role it names it in, the body by its
/// founder, what kind it wants (`None`: a solid or a sheet), whether the
/// feature changes it (acts on it or consumes it; a split's tool is only
/// read), and what to say when it is the wrong kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Read<'a> {
    pub role: &'static str,
    pub body: &'a Id,
    pub want: Option<Want>,
    pub changes: bool,
    pub hint: Option<&'static str>,
}

const JOIN: &str = "a sheet has no inside to join: sew sheets into a body, or thicken a sheet";
const TOOL: &str =
    "a sheet can't be a boolean's tool: split a body by the sheet instead (split --tool)";
const SHELL: &str = "a sheet has no inside to hollow: thicken it instead";
const COPY: &str = "a sheet's mirror image can't be joined, cut or intersected: make it new";
const SHEET_METAL: &str = "sheet metal is a solid: thicken a sheet into one first";

/// The bodies feature `f` names, each with what it wants of it: the one
/// table of design 388's "Sheets through other features". A join's
/// targets and a unite's target are solids, a cut's, intersect's,
/// hole's, pattern's, subtract's, fillet's, chamfer's, offset's, split's
/// and tweak's either; a boolean's tools and a shell's target solids; a
/// mirror's source either when its copy is new, else a solid; sheet
/// metal solids; a sew's sheets and a thicken's sheet sheets.
pub fn bodies_read(f: &crate::doc::files::Feature) -> Vec<Read<'_>> {
    use crate::doc::files::Feature as F;
    use crate::doc::solid::{BooleanOp, Combine, Operation};
    let read = |role, body, want, hint| Read {
        role,
        body,
        want,
        changes: true,
        hint,
    };
    // A combine's targets: solids to join, either to cut or intersect.
    let combined = |c: &Combine| -> (Option<Want>, Option<&'static str>) {
        match c.operation {
            Operation::Join => (Some(Want::Solid), Some(JOIN)),
            _ => (None, None),
        }
    };
    match f {
        F::Boolean(b) => {
            let unite = matches!(b.op, BooleanOp::Unite);
            let target = read(
                "target",
                &b.target,
                unite.then_some(Want::Solid),
                unite.then_some(JOIN),
            );
            std::iter::once(target)
                .chain(
                    b.tools
                        .iter()
                        .map(|t| read("tools", t, Some(Want::Solid), Some(TOOL))),
                )
                .collect()
        }
        F::Mirror(m) => {
            let new = m.combine.is_new();
            let (want, hint) = combined(&m.combine);
            std::iter::once(read(
                "solid",
                &m.solid,
                (!new).then_some(Want::Solid),
                (!new).then_some(COPY),
            ))
            .chain(
                m.combine
                    .targets
                    .iter()
                    .map(|t| read("targets", t, want, hint)),
            )
            .collect()
        }
        F::Shell(s) => vec![read("target", &s.target, Some(Want::Solid), Some(SHELL))],
        F::Split(s) => {
            let mut out = vec![read("target", &s.target, None, None)];
            if let Some(t) = s.tool_body() {
                out.push(Read {
                    changes: false,
                    ..read("tool", t, None, None)
                });
            }
            out
        }
        F::Offset(_) | F::MoveFace(_) | F::ReplaceFace(_) => f
            .targets()
            .into_iter()
            .map(|t| read("target", t, None, None))
            .collect(),
        F::Fillet(_) | F::Chamfer(_) | F::Hole(_) | F::Pattern(_) => f
            .targets()
            .into_iter()
            .map(|t| read("targets", t, None, None))
            .collect(),
        F::Flange(_) | F::Unfold(_) | F::Refold(_) => f
            .targets()
            .into_iter()
            .map(|t| read("targets", t, Some(Want::Solid), Some(SHEET_METAL)))
            .collect(),
        F::Sew(s) => s
            .sheets
            .iter()
            .map(|t| read("sheets", t, Some(Want::Sheet), None))
            .collect(),
        F::Thicken(t) => {
            let (want, hint) = combined(&t.combine);
            t.combine
                .targets
                .iter()
                .map(|x| read("targets", x, want, hint))
                .chain(t.sheet().map(|s| read("from", s, Some(Want::Sheet), None)))
                .collect()
        }
        f => match f.combine() {
            Some(c) => {
                let (want, hint) = combined(c);
                c.targets
                    .iter()
                    .map(|t| read("targets", t, want, hint))
                    .collect()
            }
            None => Vec::new(),
        },
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
    for r in bodies_read(f) {
        let need = crate::rebuild::Need {
            role: r.role,
            want: r.want,
            changes: r.changes,
            hint: r.hint,
        };
        match crate::rebuild::body_as(part, &solids, id, r.body, need, results) {
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

    fn feature(mut v: serde_json::Value) -> crate::doc::files::Feature {
        v["kind"] = serde_json::json!("ryucad.feature");
        v["format"] = serde_json::json!(crate::doc::files::FEATURE_FORMAT);
        crate::doc::files::Feature::from_json(
            &v,
            crate::units::Unit::Mm,
            &crate::expr::Env::empty(),
        )
        .unwrap()
    }

    /// What `f` wants of body `body` in `role`: `Some(None)` either kind.
    fn wants(f: &serde_json::Value, role: &str, body: &str) -> Option<Option<Want>> {
        let f = feature(f.clone());
        bodies_read(&f)
            .into_iter()
            .find(|r| r.role == role && r.body.as_str() == body)
            .map(|r| r.want)
    }

    #[test]
    fn the_table_of_what_each_feature_wants() {
        use serde_json::json;
        let edge = json!({"feature": "w", "name": "0123456789abcdef0123456789abcdef"});
        let boxed = |op: &str| {
            json!({"type": "box", "params": {"x": 1, "y": 1, "z": 1},
                                      "operation": op, "targets": ["w"],
                   "placement": {"origin": [0, 0, 0], "x": [1, 0, 0], "z": [0, 0, 1]}})
        };
        let s = Some(Want::Solid);
        for (f, role, body, want) in [
            (boxed("join"), "targets", "w", Some(s)),
            (boxed("cut"), "targets", "w", Some(None)),
            (boxed("intersect"), "targets", "w", Some(None)),
            (
                json!({"type": "boolean", "op": "unite", "target": "w", "tools": ["t"]}),
                "target",
                "w",
                Some(s),
            ),
            (
                json!({"type": "boolean", "op": "subtract", "target": "w", "tools": ["t"]}),
                "target",
                "w",
                Some(None),
            ),
            (
                json!({"type": "boolean", "op": "subtract", "target": "w", "tools": ["t"]}),
                "tools",
                "t",
                Some(s),
            ),
            (
                json!({"type": "shell", "target": "w", "thickness": 1}),
                "target",
                "w",
                Some(s),
            ),
            (
                json!({"type": "offset", "target": "w", "distance": 1}),
                "target",
                "w",
                Some(None),
            ),
            (
                json!({"type": "split", "target": "w", "plane": "xy"}),
                "target",
                "w",
                Some(None),
            ),
            (
                json!({"type": "split", "target": "w", "tool": {"body": "t"}}),
                "tool",
                "t",
                Some(None),
            ),
            (
                json!({"type": "mirror", "solid": "w", "plane": "yz", "operation": "new"}),
                "solid",
                "w",
                Some(None),
            ),
            (
                json!({"type": "mirror", "solid": "w", "plane": "yz", "operation": "join", "targets": ["b"]}),
                "solid",
                "w",
                Some(s),
            ),
            (
                json!({"type": "mirror", "solid": "w", "plane": "yz", "operation": "join", "targets": ["b"]}),
                "targets",
                "b",
                Some(s),
            ),
            (
                json!({"type": "mirror", "solid": "b", "plane": "yz", "operation": "cut", "targets": ["w"]}),
                "targets",
                "w",
                Some(None),
            ),
            (
                json!({"type": "fillet", "groups": [{"radius": 1, "edges": [edge.clone()]}]}),
                "targets",
                "w",
                Some(None),
            ),
            (
                json!({"type": "sew", "sheets": ["w", "v"], "tolerance": "0.01 mm", "solid": true}),
                "sheets",
                "w",
                Some(Some(Want::Sheet)),
            ),
        ] {
            assert_eq!(wants(&f, role, body), want, "{f} {role} {body}");
        }
        // A split's tool is read, never changed; its target is changed.
        let f = feature(json!({"type": "split", "target": "w", "tool": {"body": "t"}}));
        let read = bodies_read(&f);
        assert!(read.iter().any(|r| r.role == "tool" && !r.changes));
        assert!(read.iter().any(|r| r.role == "target" && r.changes));
        // A shell's refusal says to thicken; a boolean's tool, to split.
        let shell = feature(json!({"type": "shell", "target": "w", "thickness": 1}));
        assert!(bodies_read(&shell)[0].hint.unwrap().contains("thicken"));
        let b =
            feature(json!({"type": "boolean", "op": "subtract", "target": "w", "tools": ["t"]}));
        assert!(bodies_read(&b)[1].hint.unwrap().contains("split"));
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
        assert!(need(&id("c"), "targets", &id("w"), &b, Want::Solid, None).is_ok());
        let e = need(&id("c"), "targets", &id("w"), &sheet, Want::Solid, None).unwrap_err();
        assert_eq!(e.code, Code::WrongBodyKind);
        assert_eq!(e.details["wanted"], "solid");
        assert_eq!(e.details["got"], "sheet");
        assert_eq!(e.details["solid"], "w");
        assert_eq!(e.details["role"], "targets");
        assert_eq!(e.details["feature"], "c");
    }
}
