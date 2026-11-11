//! Resolving references against the solids of features built before. A
//! reference names a solid by the feature that founded it; its name
//! resolves to exactly the face, edge or vertex that carries it in the
//! solid as the features listed before the referring one left it (its
//! view, see [`crate::solid`]), or the feature holding the reference
//! fails. Never a nearest match.

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Results};
use crate::solid::Solids;
use ryukei::geom::{Curve, Point3, Surface, Vector3};
use ryukei::name::{Event, Name};
use ryukei::topo::{Body, EntityId, Sense};
use std::collections::BTreeMap;

/// What a role takes from a face or edge a later feature split or merged
/// (design 447): every role that reads one says which, where it reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    /// Only the surface or curve it lies on: a mate's connector, a sketch
    /// plane, an up-to target, a datum, an axis or a direction. Follows
    /// to the first piece when every piece lies on the old one, bit for
    /// bit.
    Carrier,
    /// The entity as a member of a set: a fillet's edges, a shell's or a
    /// tweak's faces. Follows to every piece.
    All,
    /// Its extent or boundary: a profile, a section, a path segment, a
    /// fixed face, a tool. Never follows: `reference_split`.
    One,
}

impl Follow {
    /// The class of `role` in `f`, or `None` for a role `f` never names
    /// (a test walks every feature's roles through this). Roles are
    /// matched with their indices as `#`; a sketch's projections by their
    /// `entities.` prefix.
    pub fn classify(f: &Feature, role: &str) -> Option<Follow> {
        use Follow::{All, Carrier, One};
        let shape: String = role
            .split('.')
            .map(|seg| {
                if !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()) {
                    "#"
                } else {
                    seg
                }
            })
            .collect::<Vec<_>>()
            .join(".");
        let r = shape.as_str();
        match f {
            Feature::Sketch(_) => match r {
                "plane" | "origin" | "x" => Some(Carrier),
                _ if r.starts_with("entities.") => Some(One),
                _ => None,
            },
            Feature::Extrude(_) => (r == "target").then_some(Carrier),
            Feature::Revolve(_) => (r == "axis").then_some(Carrier),
            Feature::Hole(_) => (r == "depth").then_some(Carrier),
            Feature::Fillet(_) => (r == "groups.#.edges.#").then_some(All),
            Feature::Chamfer(_) => match r {
                "groups.#.edges.#" => Some(All),
                // Which side the first distance is on: the kernel takes the
                // face itself, beside the edge.
                "groups.#.face" => Some(One),
                _ => None,
            },
            Feature::Pattern(_) => {
                matches!(r, "direction" | "direction2" | "axis" | "plane").then_some(Carrier)
            }
            Feature::Datum(_) => match r {
                "curve" => Some(One),
                "from" | "about" | "points.#" | "through" | "axis" | "point" | "at"
                | "between.#" | "origin" | "x" | "plane" | "in" | "of.#" | "onto" => Some(Carrier),
                _ => None,
            },
            Feature::Path(_) => match r {
                "segments.#" => Some(One),
                "axis" | "start" => Some(Carrier),
                _ => None,
            },
            Feature::Sweep(_) => match r {
                "profile" | "path" => Some(One),
                "direction" => Some(Carrier),
                _ => None,
            },
            Feature::Loft(_) => match r {
                "sections.#" => Some(One),
                "start_tangent" | "end_tangent" => Some(Carrier),
                _ => None,
            },
            Feature::Shell(_) => matches!(r, "open.#" | "faces.#.faces.#").then_some(All),
            Feature::Offset(_) => (r == "faces.#.faces.#").then_some(All),
            Feature::Mirror(_) => (r == "plane").then_some(Carrier),
            Feature::Split(_) => match r {
                "tool.faces.#" => Some(One),
                "plane" => Some(Carrier),
                _ => None,
            },
            Feature::Planar(_) => (r == "profile").then_some(One),
            Feature::Fill(_) => matches!(r, "sides.#.edge" | "sides.#.curve").then_some(One),
            Feature::Extract(_) => (r == "faces.#").then_some(All),
            Feature::Thicken(_) => match r {
                "from.faces.#" => Some(All),
                // Matched by name, never resolved: a piece isn't the face.
                "faces.#.faces.#" => Some(One),
                _ => None,
            },
            Feature::Flange(_) => matches!(
                r,
                "edges.#" | "edges.#.edge" | "edges.#.side" | "ends.#.vertex"
            )
            .then_some(One),
            Feature::Unfold(_) => matches!(r, "fixed" | "only.#" | "bends.#.bend").then_some(One),
            Feature::Refold(_) => (r == "only.#").then_some(One),
            Feature::MoveFace(_) => match r {
                "faces.#" | "groups.#.faces.#" => Some(All),
                "direction" | "axis" => Some(Carrier),
                _ => None,
            },
            Feature::ReplaceFace(_) => match r {
                "groups.#.faces.#" => Some(All),
                "groups.#.with" => Some(Carrier),
                _ => None,
            },
            Feature::Draft(_) => match r {
                "faces.#" | "groups.#.faces.#" | "parting.#" => Some(All),
                // Matched against the faces as written, never resolved.
                "flip.#" => Some(One),
                "neutral" | "pull" => Some(Carrier),
                _ => None,
            },
            Feature::Primitive(_)
            | Feature::Boolean(_)
            | Feature::Instance(_)
            | Feature::Mate(_)
            | Feature::InstancePattern(_)
            | Feature::Import(_)
            | Feature::Sew(_)
            | Feature::BaseFlange(_) => None,
        }
    }

    /// [`Follow::classify`], `One` for a role it doesn't know: refusing
    /// is never wrong.
    pub fn of(f: &Feature, role: &str) -> Follow {
        Follow::classify(f, role).unwrap_or(Follow::One)
    }
}

/// What an entity lies on, as the last body that had it left it.
#[derive(Clone, Debug, PartialEq)]
enum Carrier {
    Face(Surface, Sense),
    Edge(Curve),
    Vertex(Point3),
}

/// `e`'s carrier in `body`.
fn carrier(body: &Body, e: EntityId) -> Option<Carrier> {
    match e {
        EntityId::Face(f) => {
            let face = body.face(f)?;
            Some(Carrier::Face(
                body.surface(face.surface)?.clone(),
                face.sense,
            ))
        }
        EntityId::Edge(x) => Some(Carrier::Edge(body.curve(body.edge(x)?.curve)?.clone())),
        EntityId::Vertex(v) => Some(Carrier::Vertex(body.vertex(v)?.point)),
        _ => None,
    }
}

/// Whether `a` and `b` are the same kind of entity.
fn same_kind(a: &Carrier, b: EntityId) -> bool {
    matches!(
        (a, b),
        (Carrier::Face(..), EntityId::Face(_))
            | (Carrier::Edge(_), EntityId::Edge(_))
            | (Carrier::Vertex(_), EntityId::Vertex(_))
    )
}

/// Where a retired name went: the pieces of its kind the reader's body
/// has, in name order, and the feature whose journal first retired it.
#[derive(Clone, Debug, PartialEq)]
pub struct Trail {
    pub pieces: Vec<Name>,
    pub by: Option<Id>,
}

/// What a name a reader's body lacks became.
#[derive(Clone, Debug, PartialEq)]
pub enum Followed {
    /// Nothing of its kind: `reference_unresolved`, as before.
    Gone,
    /// Followed: the entities the role reads (the first piece for
    /// `Carrier`, every piece for `All`).
    To(Vec<EntityId>, Trail),
    /// Split or merged, and the role can't take it: `one_needed` or
    /// `carriers_differ`.
    Refused(&'static str, Trail),
}

/// What a solid's chain did to the names it retired, from the journals
/// of the rebuild that is running and nothing else: each split, merge,
/// replacement, placement and deletion with the feature whose journal
/// has it, in order, and what each retired name lay on in the last body
/// that had it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lineage {
    events: Vec<(Id, Event)>,
    carriers: BTreeMap<Name, Carrier>,
}

impl Lineage {
    /// The lineage of a chain's results in order, each with the feature
    /// that made it.
    pub fn of_chain<'a>(chain: impl IntoIterator<Item = (&'a Id, &'a Built)>) -> Lineage {
        let mut out = Lineage::default();
        let mut before: Option<&Body> = None;
        for (f, b) in chain {
            for ev in b.journal.events() {
                let retired: Vec<Name> = match ev {
                    Event::Split { parent, .. } => vec![*parent],
                    Event::Merged { parents, .. } => parents.clone(),
                    Event::Replaced { old, .. } => vec![*old],
                    Event::Deleted(n) => vec![*n],
                    Event::Placed { .. } => Vec::new(),
                    Event::Created(_) | Event::Modified(_) => continue,
                };
                out.events.push((f.clone(), ev.clone()));
                if let Some(body) = before {
                    for n in retired {
                        if let Some(c) = body.by_name(n).and_then(|e| carrier(body, e)) {
                            out.carriers.entry(n).or_insert(c);
                        }
                    }
                }
            }
            before = Some(&b.body);
        }
        out
    }

    /// The lineage of `solid` in `results`, its chain as `solids` has it,
    /// up to and including `last`.
    pub fn of(solids: &Solids, solid: &Id, last: &Id, results: &dyn Results) -> Lineage {
        let Some(chain) = solids.chain(solid) else {
            return Lineage::default();
        };
        let end = chain
            .iter()
            .position(|f| f == last)
            .map_or(chain.len(), |i| i + 1);
        Lineage::of_chain(
            chain[..end]
                .iter()
                .filter_map(|f| match results.result(f)? {
                    Ok(o) => Some((f, o.solid(solid)?)),
                    Err(_) => None,
                }),
        )
    }

    /// Whether it records nothing: no name of its chain was retired.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// What `name`, which `body` (the reader's) lacks, became for a role
    /// of class `follow`.
    pub fn follow(&self, name: Name, follow: Follow, body: &Body) -> Followed {
        let Some(old) = self.carriers.get(&name) else {
            return Followed::Gone;
        };
        let by = self
            .events
            .iter()
            .find(|(_, ev)| retires(ev, name))
            .map(|(f, _)| f.clone());
        let pieces: Vec<(Name, EntityId)> =
            crate::rebuild::successors_of(self.events.iter().map(|(_, e)| e), name, |n| {
                body.by_name(n).is_some()
            })
            .into_iter()
            .filter_map(|n| Some((n, body.by_name(n)?)))
            .filter(|(_, e)| same_kind(old, *e))
            .collect();
        if pieces.is_empty() {
            return Followed::Gone;
        }
        let trail = Trail {
            pieces: pieces.iter().map(|(n, _)| *n).collect(),
            by,
        };
        match follow {
            Follow::All => Followed::To(pieces.iter().map(|(_, e)| *e).collect(), trail),
            Follow::One => Followed::Refused("one_needed", trail),
            Follow::Carrier => {
                if pieces
                    .iter()
                    .all(|(_, e)| carrier(body, *e).as_ref() == Some(old))
                {
                    Followed::To(vec![pieces[0].1], trail)
                } else {
                    Followed::Refused("carriers_differ", trail)
                }
            }
        }
    }
}

/// Whether `ev` retires `name`.
fn retires(ev: &Event, name: Name) -> bool {
    match ev {
        Event::Split { parent, .. } => *parent == name,
        Event::Merged { parents, .. } => parents.contains(&name),
        Event::Replaced { old, .. } => *old == name,
        Event::Deleted(n) => *n == name,
        _ => false,
    }
}

/// The solid `r` names, as `owner` sees it, and the entity its name
/// carries. `owner` holds the reference in its `role`, of class `follow`
/// (never `All`: a set's role reads [`resolve_all`]); `order` is the
/// part's list as it is, or as a write would leave it.
pub fn resolve<'a>(
    part: &Part,
    order: &[Id],
    owner: &Id,
    role: &str,
    follow: Follow,
    r: &Reference,
    results: &'a dyn Results,
) -> Result<(&'a Built, EntityId)> {
    debug_assert!(
        follow != Follow::All,
        "{role}: a set's role reads resolve_all"
    );
    let (b, mut found) = lookup(part, order, owner, role, follow, r, results)?;
    Ok((b, found.remove(0)))
}

/// [`resolve`] for a role that reads a set (`Follow::All`): every entity
/// the name is now, in name order.
pub fn resolve_all<'a>(
    part: &Part,
    order: &[Id],
    owner: &Id,
    role: &str,
    r: &Reference,
    results: &'a dyn Results,
) -> Result<(&'a Built, Vec<EntityId>)> {
    lookup(part, order, owner, role, Follow::All, r, results)
}

fn lookup<'a>(
    part: &Part,
    order: &[Id],
    owner: &Id,
    role: &str,
    follow: Follow,
    r: &Reference,
    results: &'a dyn Results,
) -> Result<(&'a Built, Vec<EntityId>)> {
    let text = r.text(&part.id);
    let fail = |code, msg: String| {
        Error::new(code, format!("{owner}: its {role} {text} {msg}"))
            .with("feature", owner)
            .with("role", role)
            .with("reference", &text)
    };
    let at = |x: &Id| order.iter().position(|o| o == x);
    match (at(&r.feature), at(owner)) {
        (None, _) => {
            return Err(fail(
                Code::NotFound,
                format!("names {}, which the part doesn't list", r.feature),
            ));
        }
        (Some(i), Some(me)) if i >= me => {
            return Err(fail(
                Code::ReferenceForward,
                format!("names {}, which doesn't come before it", r.feature),
            ));
        }
        _ => {}
    }
    let solids = Solids::of_order(part, order);
    if let Some(by) = solids.consumed_by(&r.feature, owner) {
        return Err(fail(
            Code::BodyConsumed,
            format!(
                "names {}, which {by} consumed: refer to what {by} made instead",
                r.feature
            ),
        )
        .with("solid", &r.feature)
        .with("consumed_by", by));
    }
    // The feature whose result holds the solid as `owner` sees it.
    let read = solids.view(&r.feature, owner).unwrap_or(&r.feature);
    match results.result(read) {
        None => Err(fail(
            Code::NotFound,
            format!("names {}, which hasn't been built", r.feature),
        )),
        Some(Err(e)) => Err(fail(
            Code::DependencyFailed,
            format!("names {}, which failed to rebuild", r.feature),
        )
        .with("dependency", read)
        .with("cause", e.code)),
        Some(Ok(o)) if o.sketch().is_some() => Err(fail(
            Code::ReferenceWrongKind,
            format!(
                "names {}, a sketch, which has no faces, edges or vertices",
                r.feature
            ),
        )),
        Some(Ok(o)) => {
            let Some(b) = o.solid(&r.feature) else {
                return Err(
                    crate::rebuild::not_a_solid(part, &solids, owner, role, &r.feature)
                        .with("reference", &text),
                );
            };
            if let Some(e) = b.body.by_name(r.name) {
                return Ok((b, vec![e]));
            }
            // Gone: what the chain's journals made of it (design 447).
            let lineage = Lineage::of(&solids, &r.feature, read, results);
            let prefix = text.rsplit_once('/').map_or("", |(p, _)| p);
            match lineage.follow(r.name, follow, &b.body) {
                Followed::To(found, _) => Ok((b, found)),
                Followed::Gone => Err(fail(
                    Code::ReferenceUnresolved,
                    format!("is gone from solid {}", r.feature),
                )
                .with("dependency", &r.feature)),
                Followed::Refused(reason, t) => {
                    let by = t.by.as_ref().map_or(String::new(), |b| format!(" by {b}"));
                    let msg = match reason {
                        "one_needed" => format!(
                            "was split{by} into {} pieces, and its {role} needs the one entity: refer to a piece",
                            t.pieces.len()
                        ),
                        _ => format!(
                            "was split{by} into {} pieces that no longer lie on its surface or curve: refer to a piece",
                            t.pieces.len()
                        ),
                    };
                    let mut e = fail(Code::ReferenceSplit, msg)
                        .with("dependency", &r.feature)
                        .with("reason", reason)
                        .with(
                            "candidates",
                            t.pieces
                                .iter()
                                .map(|n| format!("{prefix}/{n}"))
                                .collect::<Vec<_>>(),
                        );
                    if let Some(by) = &t.by {
                        e = e.with("removed_by", by);
                    }
                    Err(e)
                }
            }
        }
    }
}

/// What a resolved reference was, for messages: `face`, `edge`, `vertex`.
pub fn kind(e: EntityId) -> &'static str {
    match e {
        EntityId::Face(_) => "face",
        EntityId::Edge(_) => "edge",
        EntityId::Vertex(_) => "vertex",
        _ => "entity",
    }
}

/// `reference_wrong_kind` for a reference in `role` that resolved to `e`
/// where `want` goes.
pub fn wrong_kind(
    part: &Part,
    owner: &Id,
    role: &str,
    r: &Reference,
    e: &str,
    want: &str,
) -> Error {
    let text = r.text(&part.id);
    Error::new(
        Code::ReferenceWrongKind,
        format!("{owner}: its {role} {text} is a {e}, not {want}"),
    )
    .with("feature", owner)
    .with("role", role)
    .with("reference", text)
    .with("kind", e)
}

/// A planar face's plane: a point on it and its outward normal.
pub fn plane(
    part: &Part,
    owner: &Id,
    role: &str,
    r: &Reference,
    (b, e): (&Built, EntityId),
) -> Result<(Point3, Vector3)> {
    let wrong = |what: &str| wrong_kind(part, owner, role, r, what, "a planar face");
    let EntityId::Face(f) = e else {
        return Err(wrong(kind(e)));
    };
    let face = b.body.face(f).expect("a live face");
    match b.body.surface(face.surface) {
        Some(s @ Surface::Plane(_)) => {
            let frame = s.frame().expect("a plane has a frame");
            let n = frame.z().get();
            let n = if face.sense == Sense::Reversed { -n } else { n };
            Ok((frame.origin(), n))
        }
        _ => Err(wrong("curved face")),
    }
}

/// A vertex's point.
pub fn point(
    part: &Part,
    owner: &Id,
    role: &str,
    r: &Reference,
    (b, e): (&Built, EntityId),
) -> Result<Point3> {
    match e {
        EntityId::Vertex(v) => Ok(b.body.vertex(v).expect("a live vertex").point),
        _ => Err(wrong_kind(part, owner, role, r, kind(e), "a vertex")),
    }
}

/// A straight edge's start and end points, in the edge's direction.
pub fn line(
    part: &Part,
    owner: &Id,
    role: &str,
    r: &Reference,
    (b, e): (&Built, EntityId),
) -> Result<(Point3, Point3)> {
    let wrong = |what: &str| wrong_kind(part, owner, role, r, what, "a straight edge");
    let EntityId::Edge(id) = e else {
        return Err(wrong(kind(e)));
    };
    let edge = b.body.edge(id).expect("a live edge");
    match b.body.curve(edge.curve) {
        Some(Curve::Line(_)) => {
            let at = |v| b.body.vertex(v).expect("a live vertex").point;
            Ok((at(edge.start), at(edge.end)))
        }
        _ => Err(wrong("curved edge")),
    }
}

#[cfg(test)]
mod tests {
    use crate::error::Code;
    use crate::server::tests::call;
    use crate::session::Session;
    use crate::sketch::tests::{edge_between, face_along, rectangle_ops, vertex_at, with_box};
    use serde_json::{Value, json};

    /// The box `b` with the rectangle sketch `s` on xy beside it, and the
    /// box's body in `inspect`.
    fn with_box_and_sketch() -> (tempfile::TempDir, Session, Value) {
        let (dir, mut s, body) = with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": rectangle_ops()}),
        )
        .unwrap();
        (dir, s, body)
    }

    /// The top of extrude `e`'s box.
    fn top(s: &mut Session) -> f64 {
        call(s, "inspect", json!({"part": "p", "feature": "e"})).unwrap()["bodies"][0]["bounds"]
            ["max"][2]
            .as_f64()
            .unwrap()
    }

    #[test]
    fn up_to_a_face_or_a_vertex() {
        let (_dir, mut s, body) = with_box_and_sketch();
        let face = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"type": "up_to", "target": face}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.005).abs() < 1e-15);
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "e"})).unwrap();
        assert_eq!(show["extent"], json!({"type": "up_to", "target": face}));
        // An offset alone keeps the target.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "e", "extent": {"offset": "-1 mm"}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.004).abs() < 1e-15);
        // The target moves; the extrude follows.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"z": 8}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.007).abs() < 1e-15);
        // Up to a vertex, keeping the offset.
        let corner = format!("p/b/{}", vertex_at(&body, [0.0, 0.0, 0.005]));
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "e", "extent": {"type": "up_to", "target": corner}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.007).abs() < 1e-15);
        // A new type starts afresh.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "e", "extent": {"type": "blind", "length": 2}}),
        )
        .unwrap();
        assert!((top(&mut s) - 0.002).abs() < 1e-15);
    }

    #[test]
    fn revolve_about_an_edge() {
        let (_dir, mut s, body) = with_box();
        // On zx the sketch's x is the part's z and its y the part's x, so
        // the box's bottom edge along x is the sketch's y axis.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "zx"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": rectangle_ops()}),
        )
        .unwrap();
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.0]),
            vertex_at(&body, [0.02, 0.0, 0.0]),
        );
        let along_x = edge_between(&body, &a, &b);
        // Which way the edge runs decides which way the turn goes.
        let starts_at_origin = {
            let e = body["edges"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["name"] == along_x.as_str())
                .unwrap();
            e["start"] == a.as_str()
        };
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s",
                   "axis": format!("p/b/{along_x}"), "angle": 90}),
        )
        .unwrap();
        let v = call(&mut s, "inspect", json!({"part": "p", "feature": "r"})).unwrap();
        // A quarter turn about x takes the sketch's far corner, at z = 20 mm,
        // to y = -20 mm (or +20 mm, turning the other way), z = 0.
        let want = if starts_at_origin { -0.02 } else { 0.02 };
        let swung = v["bodies"][0]["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| {
                let p = &x["point"];
                (p[1].as_f64().unwrap() - want).abs() < 1e-9 && p[2].as_f64().unwrap().abs() < 1e-9
            });
        assert!(swung, "{}", v["bodies"][0]["vertices"]);
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "r"})).unwrap();
        assert_eq!(show["axis"], format!("p/b/{along_x}"));
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
    }

    #[test]
    fn axes_that_wont_do() {
        let (_dir, mut s, body) = with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "zx"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": rectangle_ops()}),
        )
        .unwrap();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "sketch": "s", "axis": top, "angle": 90}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind);
        // The back bottom edge is 10 mm behind the sketch's plane.
        let (a, b) = (
            vertex_at(&body, [0.0, 0.01, 0.0]),
            vertex_at(&body, [0.02, 0.01, 0.0]),
        );
        let back = format!("p/b/{}", edge_between(&body, &a, &b));
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s", "axis": back,
                   "angle": 90}),
        )
        .unwrap();
        let e = call(&mut s, "rebuild", json!({})).unwrap_err();
        let f = &e.details["parts"][0]["features"][2];
        assert_eq!(f["error"]["code"], "invalid_extent", "{f}");
        assert_eq!(f["error"]["details"]["role"], "axis");
    }

    #[test]
    fn an_unresolved_reference_says_what_it_was() {
        let (_dir, mut s) = crate::sketch::tests::with_sketch();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": rectangle_ops()}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"length": 5}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "p", "feature": "e"})).unwrap()["bodies"]
            [0]
        .clone();
        let from_l2 = json!({"feature": "e", "sketch": "s", "entity": "l2"});
        let side = body["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["origin"] == from_l2)
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": format!("p/e/{side}")}),
        )
        .unwrap();
        // l2 gives way to l5: the face it made goes, and t with it.
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "remove", "id": "l2"},
                {"op": "add", "id": "l5", "type": "line", "start": "p2", "end": "p3"}
            ]}),
        )
        .unwrap();
        let e = call(&mut s, "rebuild", json!({})).unwrap_err();
        let t = &e.details["parts"][0]["features"][2]["error"];
        assert_eq!(t["code"], "reference_unresolved", "{t}");
        assert_eq!(t["details"]["role"], "plane");
        assert_eq!(
            t["details"]["was"],
            json!({"kind": "face", "from": from_l2})
        );
        // The extrude made it and makes it no more.
        assert_eq!(t["details"]["removed_by"], "e");
        // An edit that names it again is refused the same way.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "face": format!("p/e/{side}")}),
        )
        .unwrap_err();
        assert_eq!(e.details["was"]["from"], from_l2);
    }

    #[test]
    fn a_name_a_later_feature_took_says_which() {
        let (_dir, mut s, body) = crate::sketch::tests::with_box();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        // A cut taking the box's top millimetre.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "c", "operation": "cut",
                   "params": {"x": 30, "y": 30, "z": 2},
                   "placement": {"origin": [-5, -5, 4]}}),
        )
        .unwrap();
        // A sketch on the top, after the cut: the name is gone, and the
        // cut took it.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": top}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceUnresolved, "{e}");
        assert_eq!(e.details["removed_by"], "c");
        assert_eq!(
            e.details["was"],
            json!({"kind": "face", "from": {"feature": "b"}})
        );
        // Before the cut it is still there.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": top, "after": "b"}),
        )
        .unwrap();
    }

    #[test]
    fn targets_that_wont_do() {
        let (_dir, mut s, body) = with_box_and_sketch();
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.005]),
            vertex_at(&body, [0.02, 0.0, 0.005]),
        );
        let edge = format!("p/b/{}", edge_between(&body, &a, &b));
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "sketch": "s",
                   "extent": {"type": "up_to", "target": edge}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind);
        // A side, the sketch's own plane, and a face behind it: only
        // rebuild can tell.
        let mut id = 0;
        for (target, reverse) in [
            (face_along(&body, [0.0, -1.0, 0.0]), false),
            (face_along(&body, [0.0, 0.0, -1.0]), false),
            (face_along(&body, [0.0, 0.0, 1.0]), true),
        ] {
            id += 1;
            call(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "extrude", "id": format!("e{id}"), "sketch": "s",
                       "reverse": reverse,
                       "extent": {"type": "up_to", "target": format!("p/b/{target}")}}),
            )
            .unwrap();
        }
        let e = call(&mut s, "rebuild", json!({})).unwrap_err();
        for f in &e.details["parts"][0]["features"].as_array().unwrap()[2..] {
            assert_eq!(f["error"]["code"], "invalid_extent", "{f}");
            assert_eq!(f["error"]["details"]["role"], "target", "{f}");
        }
    }

    #[test]
    fn a_draft_s_roles_have_their_classes() {
        use super::Follow;
        use crate::units::Unit;
        let r = |k: u8| json!({"feature": "b", "name": format!("{k:032x}")});
        let v = json!({"format": crate::doc::tweak::DRAFT_FORMAT, "kind": "ryucad.feature",
            "type": "draft", "target": "b", "angle": "2 deg", "parting": [r(3)], "pull": "z",
            "faces": [r(1)], "groups": [{"angle": "5 deg", "faces": [r(2)]}], "flip": [r(2)]});
        let f = crate::doc::files::Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty())
            .unwrap();
        let classes: Vec<(String, Option<Follow>)> = f
            .references()
            .into_iter()
            .map(|(role, _)| {
                let c = Follow::classify(&f, &role);
                (role, c)
            })
            .collect();
        assert_eq!(
            classes,
            [
                ("faces.0".to_owned(), Some(Follow::All)),
                ("groups.0.faces.0".to_owned(), Some(Follow::All)),
                ("parting.0".to_owned(), Some(Follow::All)),
                ("flip.0".to_owned(), Some(Follow::One)),
            ]
        );
        // A role the feature never names has none, and refuses.
        assert_eq!(Follow::classify(&f, "plane"), None);
        assert_eq!(Follow::of(&f, "plane"), Follow::One);
    }

    /// The 1 mm slot `c` cut across the box `b` (20 x 10 x 5 mm): short,
    /// from y = 7 mm out through the back, it notches the top; long, from
    /// y = -5 mm, it cuts the top and its front edge in two.
    fn slot(s: &mut Session, long: bool) {
        let y = if long { -5 } else { 7 };
        let placement = json!({"origin": [9.5, y, -5], "z": [0, 0, 1], "x": [1, 0, 0]});
        let exists = call(s, "feature.show", json!({"part": "p", "id": "c"})).is_ok();
        if exists {
            call(
                s,
                "feature.edit",
                json!({"part": "p", "id": "c", "placement": placement}),
            )
            .unwrap();
        } else {
            call(
                s,
                "feature.add",
                json!({"part": "p", "type": "box", "id": "c", "operation": "cut",
                       "params": {"x": 1, "y": 20, "z": 20}, "placement": placement}),
            )
            .unwrap();
        }
    }

    /// Feature `id`'s entry in the rebuild's report, whether it built or
    /// not.
    fn reported(s: &mut Session, id: &str) -> Value {
        let report = match call(s, "rebuild", json!({})) {
            Ok(v) => v,
            Err(e) => e.details,
        };
        report["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == id)
            .unwrap()
            .clone()
    }

    /// The names of `feature`'s faces in `b` whose outward normal is `n`.
    fn faces_along(s: &mut Session, feature: &str, n: [f64; 3]) -> Vec<String> {
        let body = call(s, "inspect", json!({"part": "p", "feature": feature})).unwrap()["bodies"]
            [0]
        .clone();
        let mut out: Vec<String> = body["faces"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| {
                let z = &x["surface"]["frame"]["z"];
                let sign = if x["reversed"] == true { -1.0 } else { 1.0 };
                x["surface"]["type"] == "plane"
                    && (0..3).all(|k| (z[k].as_f64().unwrap() * sign - n[k]).abs() < 1e-12)
            })
            .map(|x| x["name"].as_str().unwrap().to_owned())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn a_sketch_on_a_split_face_follows_its_first_piece_and_says_so() {
        let (_dir, mut s, body) = with_box();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        slot(&mut s, false);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": top}),
        )
        .unwrap();
        assert!(reported(&mut s, "t")["warnings"].is_null());
        slot(&mut s, true);
        let pieces: Vec<String> = faces_along(&mut s, "c", [0.0, 0.0, 1.0])
            .iter()
            .map(|n| format!("p/b/{n}"))
            .collect();
        assert_eq!(pieces.len(), 2);
        let t = reported(&mut s, "t");
        assert_eq!(t["ok"], true, "{t}");
        assert_eq!(
            t["warnings"],
            json!([{"code": "reference_followed", "part": "p", "feature": "t",
                    "role": "plane", "reference": top, "pieces": pieces, "by": "c",
                    "message": t["warnings"][0]["message"]}]),
            "{t}"
        );
        // Shortened again, the top is whole and its name resolves as
        // written: nothing followed.
        slot(&mut s, false);
        let t = reported(&mut s, "t");
        assert_eq!(t["ok"], true, "{t}");
        assert!(t["warnings"].is_null(), "{t}");
    }

    #[test]
    fn a_set_takes_every_piece_of_a_split_face() {
        let (_dir, mut s, body) = with_box();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        slot(&mut s, false);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extract", "id": "x", "faces": [top]}),
        )
        .unwrap();
        slot(&mut s, true);
        let x = reported(&mut s, "x");
        assert_eq!(x["ok"], true, "{x}");
        assert_eq!(x["warnings"][0]["role"], "faces.0", "{x}");
        let v = call(&mut s, "inspect", json!({"part": "p", "feature": "x"})).unwrap();
        assert_eq!(v["bodies"][0]["faces"].as_array().unwrap().len(), 2, "{v}");
    }

    #[test]
    fn a_projection_of_a_split_edge_is_refused_with_its_pieces() {
        let (_dir, mut s, body) = with_box();
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.005]),
            vertex_at(&body, [0.02, 0.0, 0.005]),
        );
        let front = format!("p/b/{}", edge_between(&body, &a, &b));
        slot(&mut s, false);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "u"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "u",
                   "ops": [{"op": "add", "type": "line", "projected": front}]}),
        )
        .unwrap();
        slot(&mut s, true);
        let u = reported(&mut s, "u");
        let e = &u["error"];
        assert_eq!(e["code"], "reference_split", "{u}");
        assert_eq!(e["details"]["reason"], "one_needed");
        assert_eq!(e["details"]["removed_by"], "c");
        assert_eq!(e["details"]["candidates"].as_array().unwrap().len(), 2);
        assert_eq!(e["details"]["was"]["kind"], "edge", "{u}");
    }

    #[test]
    fn pieces_off_the_old_surface_are_refused() {
        let (_dir, mut s, body) = with_box();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        slot(&mut s, true);
        let pieces = faces_along(&mut s, "c", [0.0, 0.0, 1.0]);
        // One piece lifted a millimetre: the two no longer share a plane.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "move_face", "id": "m", "by": "offset",
                   "distance": 1, "faces": [format!("p/b/{}", pieces[0])]}),
        )
        .unwrap();
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "face": top}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceSplit, "{e}");
        assert_eq!(e.details["reason"], "carriers_differ");
    }
}
