//! Every hole of a part, from its rebuild: each place of each hole
//! feature in each solid it cuts, and each image of one through a pattern
//! or a mirror's copy, with its spec, sizes, position, axis, depths and
//! faces by segment, in a fixed order (solid founder, feature in list
//! order, place in natural order, instance as the pattern lists them).
//! Positions and axes come from the places and the patterns' motions,
//! never fitted from faces; faces come from the solid's tip, by origin.
//! Imported bodies list no holes: recognising them is a later stage's.

use crate::doc::files::Feature;
use crate::doc::hole::{Bottom, Depth, Hole};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::hole::derive::Sizes;
use crate::pattern::Motion;
use crate::rebuild::{Built, PartResult, Results};
use crate::session::Session;
use crate::solid::Solids;
use ryukei::geom::{Mirror, Plane, Point3, Vector3};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One hole.
#[derive(Clone, Debug)]
pub struct Row {
    /// `<part>/<feature>/<place>`, `<part>/<pattern>/<seed>/<place>/<i>[,<j>]`,
    /// or `<part>/<mirror>/<the source's ref without its part>`.
    pub reference: String,
    pub part: Id,
    /// The hole feature whose intent it carries.
    pub feature: Id,
    pub place: Id,
    /// The pattern and the instance, `[i]` or `[i, j]`.
    pub pattern: Option<(Id, Value)>,
    /// The mirror feature, for a mirror's copy.
    pub mirror: Option<Id>,
    pub solid: Id,
    pub spec: String,
    pub sizes: Arc<Sizes>,
    /// The mouth's centre, metres.
    pub position: Point3,
    /// Unit, into the material.
    pub axis: Vector3,
    /// Each segment's faces in the solid's tip, by name.
    pub faces: BTreeMap<String, Vec<String>>,
    /// The drill's depth to its shoulder and to its tip, and the thread's
    /// depth, in metres from the mouth.
    pub depth: (f64, f64, Option<f64>),
    /// Whether it is a left-hand thread (kept through mirrors).
    pub left: bool,
}

/// A hole that no face of its solid's tip is left of.
#[derive(Clone, Debug)]
pub struct Gone {
    pub reference: String,
    pub feature: Id,
    pub solid: Id,
    /// The first feature on its solid after which none of its faces is
    /// left.
    pub removed_by: Option<Id>,
}

#[derive(Clone, Debug, Default)]
pub struct Holes {
    pub rows: Vec<Row>,
    pub gone: Vec<Gone>,
}

/// What a face's origin says about the hole it belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    /// The feature that placed it: the hole, a pattern, or (prefixed) a
    /// mirror.
    by: String,
    hole: String,
    point: String,
    instance: String,
}

fn key_of(o: &Value) -> Option<(Key, String)> {
    let segment = o.get("segment")?.as_str()?.to_owned();
    let point = o.get("point")?.as_str()?.to_owned();
    let by = o.get("feature")?.as_str()?.to_owned();
    let (hole, instance) = match o.get("instance") {
        Some(i) => (o.get("seed")?.as_str()?.to_owned(), i.to_string()),
        None => (by.clone(), String::new()),
    };
    Some((
        Key {
            by,
            hole,
            point,
            instance,
        },
        segment,
    ))
}

type Faces = BTreeMap<Key, BTreeMap<String, Vec<String>>>;

/// Faces of `b` by the hole they belong to and their segment; a mirror's
/// copy's through `source`, the body it copied, by name.
fn faces_by_key(b: &Built, source: Option<(&Id, &Built)>) -> Faces {
    let mut out: Faces = BTreeMap::new();
    for f in b.body.faces() {
        let name = b.body.face(f).expect("a face of the body").name.to_string();
        let Some(o) = b.origins.get(&name) else {
            continue;
        };
        let found = match (o.get("mirror_of").and_then(Value::as_str), source) {
            (Some(n), Some((mirror, src))) => {
                src.origins.get(n).and_then(key_of).map(|(mut k, s)| {
                    k.by = format!("{mirror}/{}", k.by);
                    (k, s)
                })
            }
            _ => key_of(o),
        };
        if let Some((k, segment)) = found {
            out.entry(k)
                .or_default()
                .entry(segment)
                .or_default()
                .push(name);
        }
    }
    out
}

/// The part's holes at the working files (`rev` none) or a revision;
/// the working files' are cached with the part's rebuild.
pub fn list(session: &mut Session, part: &Id, rev: Option<&str>) -> Result<Arc<Holes>> {
    let (_, session) = session.at(rev)?;
    let hash = session.rebuilt(part)?.hash.clone();
    if let Some((h, holes)) = session.store().holes.get(part)
        && *h == hash
    {
        return Ok(Arc::clone(holes));
    }
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(part)?.clone();
    let result = session.rebuilt(part)?;
    let holes = Arc::new(build(&p, result, unit)?);
    session.put_holes(part.clone(), hash, Arc::clone(&holes));
    Ok(holes)
}

fn build(p: &Part, result: &PartResult, unit: crate::units::Unit) -> Result<Holes> {
    let solids = Solids::of(p);
    let mut rows: Vec<Row> = Vec::new();
    let mut gone = Vec::new();
    for (solid, tip) in solids.tips() {
        let Some(Ok(out)) = result.result(tip) else {
            continue;
        };
        let Some(b) = out.solid(solid) else {
            continue;
        };
        let chain = solids.chain(solid).unwrap_or_default();
        let at = At {
            p,
            result,
            solids: &solids,
            unit,
        };
        let (r, g) = rows_at(&at, solid, chain, b, 0)?;
        rows.extend(r);
        gone.extend(g);
    }
    Ok(Holes { rows, gone })
}

/// What [`rows_at`] reads.
struct At<'a> {
    p: &'a Part,
    result: &'a PartResult,
    solids: &'a Solids,
    unit: crate::units::Unit,
}

/// The holes of `solid` with `chain` the features it has had so far and
/// `b` its body after them, and those gone: its own features' places,
/// then, for a mirror's copy, its source's holes as they were when it
/// copied them (the source's chain up to the mirror's view), reflected.
fn rows_at(
    at: &At,
    solid: &Id,
    chain: &[Id],
    b: &Built,
    depth: usize,
) -> Result<(Vec<Row>, Vec<Gone>)> {
    let At {
        p,
        result,
        solids,
        unit,
    } = *at;
    let mut rows: Vec<Row> = Vec::new();
    let mut gone = Vec::new();
    // A mirror founding this solid copies its source as the source was
    // just before it: the view's body.
    let mirror = match p.features.get(solid) {
        Some(Ok(Feature::Mirror(m))) => solids.view(&m.solid, solid).and_then(|view| {
            let o = result.result(view)?.as_ref().ok()?;
            Some((m, view, o.solid(&m.solid)?))
        }),
        _ => None,
    };
    let source = mirror.map(|(_, _, src)| (solid, src));
    let faces = faces_by_key(b, source);
    for (k, fid) in chain.iter().enumerate() {
        let Some(Ok(f)) = p.features.get(fid) else {
            continue;
        };
        for mut row in places(p, fid, f, result, unit)? {
            row.solid = solid.clone();
            let key = row_key(&row);
            match faces.get(&key) {
                Some(segments) => {
                    row.faces = segments.clone();
                    measure(b, &mut row);
                    rows.push(row);
                }
                None => gone.push(Gone {
                    removed_by: removed_by(result, &chain[k + 1..], solid, &key, source),
                    reference: row.reference,
                    feature: row.feature,
                    solid: solid.clone(),
                }),
            }
        }
    }
    // A mirror's copy: every hole its source had at the view, reflected.
    // A chain of mirrors is followed as far as a chain of solids can go.
    if let Some((m, view, src)) = mirror
        && depth < 64
    {
        let plane = crate::pattern::mirror_plane(p, solid, &m.plane, result)?;
        let src_chain = solids.chain(&m.solid).unwrap_or_default();
        let upto = src_chain
            .iter()
            .position(|f| f == view)
            .map_or(&[][..], |i| &src_chain[..=i]);
        let (src_rows, _) = rows_at(at, &m.solid, upto, src, depth + 1)?;
        for r in src_rows {
            let mut row = r.clone();
            let tail = r
                .reference
                .split_once('/')
                .map_or(r.reference.as_str(), |x| x.1);
            row.reference = format!("{}/{solid}/{tail}", p.id);
            row.mirror = Some(solid.clone());
            row.solid = solid.clone();
            row.position = reflect_point(&plane, r.position);
            row.axis = reflect_vector(&plane, r.axis);
            let mut key = row_key(&r);
            key.by = format!("{solid}/{}", key.by);
            match faces.get(&key) {
                Some(segments) => {
                    row.faces = segments.clone();
                    rows.push(row);
                }
                None => gone.push(Gone {
                    removed_by: removed_by(result, &chain[1..], solid, &key, source),
                    reference: row.reference,
                    feature: row.feature,
                    solid: solid.clone(),
                }),
            }
        }
    }
    Ok((rows, gone))
}

impl Row {
    /// `tapped`, `clearance`, `fitted` or `plain`.
    pub fn kind(&self) -> &'static str {
        if self.sizes.thread.is_some() {
            "tapped"
        } else if ["CBORE ", "CSK ", "CLEAR "]
            .iter()
            .any(|w| self.spec.starts_with(w))
        {
            "clearance"
        } else if self.sizes.fit.is_some() {
            "fitted"
        } else {
            "plain"
        }
    }

    /// A face's reference, `<part>/<solid>/<name>`.
    fn face_ref(&self, name: &str) -> String {
        format!("{}/{}/{name}", self.part, self.solid)
    }

    pub fn to_json(&self, unit: crate::units::Unit) -> Value {
        let face = |names: &Vec<String>| -> Value {
            let refs: Vec<String> = names.iter().map(|n| self.face_ref(n)).collect();
            match refs.as_slice() {
                [one] => json!(one),
                _ => json!(refs),
            }
        };
        let faces: serde_json::Map<String, Value> = self
            .faces
            .iter()
            .map(|(k, n)| (k.clone(), face(n)))
            .collect();
        let mut v = json!({
            "ref": self.reference,
            "spec": self.spec,
            "type": self.kind(),
            "position": crate::measure::point(self.position, unit),
            "axis": [self.axis.x, self.axis.y, self.axis.z],
            "sizes": self.sizes.to_json(),
            "depth": {"drill": self.depth.0, "tip": self.depth.1, "thread": self.depth.2},
            "faces": faces,
            "feature": self.feature,
            "solid": self.solid,
        });
        if let Some((p, i)) = &self.pattern {
            v["pattern"] = json!({"feature": p, "seed": self.feature, "instance": i});
        }
        if let Some(m) = &self.mirror {
            v["mirror_of"] = json!(m);
        }
        if let Some(t) = crate::thread::of_row(self) {
            v["thread"] = t.to_json();
        }
        v
    }

    /// Its line of `holes --csv`.
    fn csv(&self) -> String {
        let n = crate::measure::number;
        let field = |s: &str| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        };
        let (pattern, instance) = match &self.pattern {
            Some((p, i)) => (
                p.to_string(),
                i.as_array()
                    .map(|a| a.iter().map(Value::to_string).collect::<Vec<_>>().join(";"))
                    .unwrap_or_default(),
            ),
            None => (String::new(), String::new()),
        };
        [
            field(&self.reference),
            field(&self.spec),
            self.kind().to_owned(),
            n(self.position.x),
            n(self.position.y),
            n(self.position.z),
            n(self.axis.x),
            n(self.axis.y),
            n(self.axis.z),
            n(self.sizes.drill.si),
            n(self.depth.0),
            self.depth.2.map(n).unwrap_or_default(),
            self.feature.to_string(),
            pattern,
            instance,
            self.mirror.as_ref().map(Id::to_string).unwrap_or_default(),
            self.solid.to_string(),
        ]
        .join(",")
    }
}

impl Holes {
    /// The hole a face (by name, in `solid`) belongs to, and its segment.
    pub fn of_face(&self, solid: &str, name: &str) -> Option<(&Row, &str)> {
        self.rows
            .iter()
            .filter(|r| r.solid.as_str() == solid)
            .find_map(|r| {
                r.faces
                    .iter()
                    .find(|(_, n)| n.iter().any(|x| x == name))
                    .map(|(s, _)| (r, s.as_str()))
            })
    }

    pub fn by_ref(&self, reference: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.reference == reference)
    }
}

/// The part `text` names when it reads as a hole reference rather than
/// an entity's: its last part is no entity name (32 hex digits), and its
/// feature is a hole, a pattern or a mirror of a part (not an assembly).
pub fn is_ref(session: &Session, text: &str) -> Option<Id> {
    let parts: Vec<&str> = text.split('/').collect();
    let [part, feature, .., last] = parts.as_slice() else {
        return None;
    };
    if parts.len() < 3 || (last.len() == 32 && last.bytes().all(|b| b.is_ascii_hexdigit())) {
        return None;
    }
    let pid = Id::parse(part, "part").ok()?;
    let fid = Id::parse(feature, "feature").ok()?;
    let p = session.project().part(&pid).ok()?;
    match p.features.get(&fid)? {
        Ok(Feature::Hole(_) | Feature::Pattern(_) | Feature::Mirror(_)) => Some(pid),
        _ => None,
    }
}

/// The face references of the hole `text` names, if it names one: what
/// `select` and `highlight` take for a hole.
pub fn face_refs(session: &mut Session, text: &str) -> Result<Option<Vec<String>>> {
    let Some(pid) = is_ref(session, text) else {
        return Ok(None);
    };
    let holes = list(session, &pid, None)?;
    let row = holes.by_ref(text).ok_or_else(|| {
        Error::new(Code::NotFound, format!("{pid} has no hole {text}")).with("ref", text)
    })?;
    Ok(Some(
        row.faces
            .values()
            .flatten()
            .map(|n| row.face_ref(n))
            .collect(),
    ))
}

/// What `inspect` and `measure` add for a face of a hole: `hole` (its
/// ref, segment and spec) and, on its wall, `thread`. Nothing when the
/// part's holes can't be listed.
pub fn face_note(session: &mut Session, part: &Id, solid: &str, name: &str) -> Option<Value> {
    let holes = list(session, part, None).ok()?;
    let (row, segment) = holes.of_face(solid, name)?;
    let mut v = json!({"hole": {"ref": row.reference, "segment": segment, "spec": row.spec}});
    if segment == "wall"
        && let Some(t) = crate::thread::of_row(row)
    {
        v["thread"] = t.to_json();
    }
    Some(v)
}

/// `holes --csv`'s header.
const CSV_HEADER: &str = "ref,spec,type,x_m,y_m,z_m,axis_x,axis_y,axis_z,drill_m,depth_m,thread_depth_m,feature,pattern,instance,mirror_of,solid";

impl Gone {
    pub fn to_json(&self) -> Value {
        json!({"ref": self.reference, "feature": self.feature, "solid": self.solid,
               "removed_by": self.removed_by})
    }
}

/// `holes.list`: a part's holes as rows, CSV or groups by spec, or one
/// by its reference; `solid` keeps one solid's.
pub fn request(
    session: &mut Session,
    part: &str,
    solid: Option<&str>,
    csv: bool,
    group: bool,
    reference: Option<&str>,
    rev: Option<&str>,
) -> Result<Value> {
    let pid = Id::parse(part, "part")?;
    let unit = session.project().manifest.default_unit;
    let holes = list(session, &pid, rev)?;
    let mut rows: Vec<&Row> = holes
        .rows
        .iter()
        .filter(|r| solid.is_none_or(|s| r.solid.as_str() == s))
        .collect();
    if let Some(want) = reference {
        rows.retain(|r| r.reference == want);
        if rows.is_empty() {
            let e =
                Error::new(Code::NotFound, format!("{part} has no hole {want}")).with("ref", want);
            return Err(match holes.gone.iter().find(|g| g.reference == want) {
                Some(g) => Error::new(
                    Code::NotFound,
                    match &g.removed_by {
                        Some(by) => format!("hole {want} is gone: {by} took it away"),
                        None => format!("hole {want} is gone"),
                    },
                )
                .with("ref", want)
                .with("removed_by", &g.removed_by),
                None => e,
            });
        }
    }
    if group {
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for r in &rows {
            match groups.iter_mut().find(|(s, _)| *s == r.spec) {
                Some((_, refs)) => refs.push(r.reference.clone()),
                None => groups.push((r.spec.clone(), vec![r.reference.clone()])),
            }
        }
        let groups: Vec<Value> = groups
            .into_iter()
            .map(|(spec, refs)| json!({"spec": spec, "count": refs.len(), "refs": refs}))
            .collect();
        return Ok(json!({"groups": groups}));
    }
    if csv {
        let mut text = String::from(CSV_HEADER);
        text.push('\n');
        for r in &rows {
            text.push_str(&r.csv());
            text.push('\n');
        }
        return Ok(json!({"csv": text}));
    }
    let gone: Vec<Value> = holes
        .gone
        .iter()
        .filter(|g| solid.is_none_or(|s| g.solid.as_str() == s))
        .filter(|g| reference.is_none_or(|r| g.reference == r))
        .map(Gone::to_json)
        .collect();
    Ok(json!({
        "holes": rows.iter().map(|r| r.to_json(unit)).collect::<Vec<_>>(),
        "gone": gone,
    }))
}

fn row_key(r: &Row) -> Key {
    match &r.pattern {
        Some((pid, inst)) => Key {
            by: pid.to_string(),
            hole: r.feature.to_string(),
            point: r.place.to_string(),
            instance: inst.to_string(),
        },
        None => Key {
            by: r.feature.to_string(),
            hole: r.feature.to_string(),
            point: r.place.to_string(),
            instance: String::new(),
        },
    }
}

/// The first of `later` (features on `solid` after the hole's) whose
/// result has none of the hole's faces.
/// The first of `later` whose body of `solid` no longer has the hole
/// `key`; a mirror's copies are found through `source`, the body it
/// copied, as [`faces_by_key`] finds them.
fn removed_by(
    result: &PartResult,
    later: &[Id],
    solid: &Id,
    key: &Key,
    source: Option<(&Id, &Built)>,
) -> Option<Id> {
    later
        .iter()
        .find(|f| {
            let Some(Ok(out)) = result.result(f) else {
                return false;
            };
            out.solid(solid)
                .is_some_and(|b| !faces_by_key(b, source).contains_key(key))
        })
        .cloned()
}

fn reflect_point(plane: &Plane, p: Point3) -> Point3 {
    Mirror::in_plane_of(plane.frame()).apply_point(p)
}

fn reflect_vector(plane: &Plane, v: Vector3) -> Vector3 {
    Mirror::in_plane_of(plane.frame()).apply_vector(v)
}

/// A through or up-to hole's depths, from its faces: the deepest of its
/// walls' vertices along its axis.
fn measure(b: &Built, row: &mut Row) {
    if row.depth.0.is_finite() {
        return;
    }
    let mut deepest = f64::NEG_INFINITY;
    let walls = ["wall", "drill_wall"]
        .iter()
        .filter_map(|s| row.faces.get(*s))
        .flatten();
    let walls: Vec<&String> = walls.collect();
    for f in b.body.faces() {
        let name = b.body.face(f).expect("a face of the body").name.to_string();
        if !walls.contains(&&name) {
            continue;
        }
        for l in b.body.face_loops(f) {
            for fin in b.body.loop_fins(l) {
                if let Some(v) = b.body.fin_start(fin).and_then(|v| b.body.vertex(v)) {
                    deepest = deepest.max((v.point - row.position).dot(row.axis));
                }
            }
        }
    }
    if deepest.is_finite() {
        let full = row.depth.2.is_none_or(|t| !t.is_finite());
        row.depth.0 = deepest;
        row.depth.1 = deepest;
        if row.sizes.thread.is_some() && full {
            row.depth.2 = Some(deepest);
        }
    }
}

/// The holes a feature places, before their faces are found: a hole's
/// places, a pattern's instances of its hole seeds.
fn places(
    p: &Part,
    fid: &Id,
    f: &Feature,
    result: &PartResult,
    unit: crate::units::Unit,
) -> Result<Vec<Row>> {
    match f {
        Feature::Hole(h) => own(p, fid, h, result, unit),
        Feature::Pattern(x) => {
            let mut out = Vec::new();
            let motions = crate::pattern::motions(p, fid, x, result)?;
            for seed in &x.features {
                let Some(Ok(Feature::Hole(h))) = p.features.get(seed) else {
                    continue;
                };
                let seeds = own(p, seed, h, result, unit)?;
                for (ij, m) in &motions {
                    let instance = crate::rebuild::instance_json(x, *ij);
                    let tail = instance
                        .as_array()
                        .expect("an array")
                        .iter()
                        .map(Value::to_string)
                        .collect::<Vec<_>>()
                        .join(",");
                    for s in &seeds {
                        let mut r = s.clone();
                        r.reference = format!("{}/{fid}/{seed}/{}/{tail}", p.id, s.place);
                        r.pattern = Some((fid.clone(), instance.clone()));
                        match m {
                            Motion::Rigid(t) => {
                                r.position = t.apply_point(s.position);
                                r.axis = t.apply_vector(s.axis);
                            }
                            Motion::Mirror(plane) => {
                                r.position = reflect_point(plane, s.position);
                                r.axis = reflect_vector(plane, s.axis);
                            }
                        }
                        out.push(r);
                    }
                }
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

/// A hole feature's own places.
fn own(
    p: &Part,
    fid: &Id,
    h: &Hole,
    result: &PartResult,
    unit: crate::units::Unit,
) -> Result<Vec<Row>> {
    let sizes = Arc::new(
        crate::hole::derive::derive(h, p.hole_standards.as_deref())
            .map_err(crate::hole::derive::at_rebuild)?,
    );
    let solved = crate::rebuild::sketch_input(p, fid, &h.sketch, result)?;
    let axis = -solved.frame.apply_vector(Vector3::new(0.0, 0.0, 1.0));
    let spec = crate::hole::spec::canonical(h, unit);
    let shoulder = match &h.depth {
        Depth::Blind(q) => q.meters(),
        // Through and up-to: measured from the faces.
        _ => f64::NAN,
    };
    let tip = match (&h.bottom, shoulder.is_finite()) {
        (Bottom::Drill(a), true) => {
            shoulder + sizes.drill.si / 2.0 / ryukei::math::tan(a.radians() / 2.0)
        }
        _ => shoulder,
    };
    let thread = sizes
        .thread
        .as_ref()
        .map(|t| t.depth.as_ref().map_or(shoulder, |d| d.si));
    Ok(crate::hole::points(fid, h, solved)?
        .into_iter()
        .map(|(point, at)| Row {
            reference: format!("{}/{fid}/{point}", p.id),
            part: p.id.clone(),
            feature: fid.clone(),
            place: point,
            pattern: None,
            mirror: None,
            solid: fid.clone(),
            spec: spec.clone(),
            sizes: Arc::clone(&sizes),
            position: at,
            axis,
            faces: BTreeMap::new(),
            depth: (shoulder, tip, thread),
            left: h.thread.as_ref().is_some_and(|t| t.left),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hole::tests::{drilled, put};
    use crate::testing::call;
    use serde_json::json;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    fn tapped(s: &mut Session) {
        put(
            s,
            "h",
            json!({"format": 23, "points": ["p1", "p2"], "form": "simple",
                   "thread": {"standard": "iso_metric", "size": "M2", "depth": "2 mm"},
                   "depth": {"blind": "3 mm"}, "bottom": {"drill": "118 deg"}}),
        );
    }

    #[test]
    fn every_hole_its_patterns_make() {
        let (_dir, mut s) = drilled();
        tapped(&mut s);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "pattern", "id": "row",
                   "features": ["h"], "direction": "y", "spacing": 3, "count": 2}),
        )
        .unwrap();
        let holes = list(&mut s, &id("p"), None).unwrap();
        let refs: Vec<&str> = holes.rows.iter().map(|r| r.reference.as_str()).collect();
        assert_eq!(refs, ["p/h/p1", "p/h/p2", "p/row/h/p1/1", "p/row/h/p2/1"]);
        let r = &holes.rows[2];
        assert_eq!(r.spec, "M2x0.4-6H 2 DEEP, DRILL 3 DEEP 118DEG");
        assert!((r.position - Point3::new(0.010, 0.008, 0.005)).length() < 1e-12);
        assert!((r.axis - Vector3::new(0.0, 0.0, -1.0)).length() < 1e-15);
        assert_eq!(
            r.pattern.as_ref().map(|(p, i)| (p.as_str(), i.clone())),
            Some(("row", json!([1])))
        );
        assert_eq!(
            r.faces.keys().collect::<Vec<_>>(),
            ["drill_wall", "tip", "wall"]
        );
        assert_eq!(r.depth.0, 0.003);
        assert_eq!(r.depth.2, Some(0.002));
        assert!(holes.gone.is_empty());
        // Cached until the part rebuilds differently.
        let again = list(&mut s, &id("p"), None).unwrap();
        assert!(Arc::ptr_eq(&holes, &again));
        // A later cut through a hole takes it away: it is listed as gone.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "cut",
                   "operation": "cut", "targets": ["b"], "params": {"x": 6, "y": 4, "z": 10},
                   "placement": {"origin": [8, 3, 0]}}),
        )
        .unwrap();
        let holes = list(&mut s, &id("p"), None).unwrap();
        let gone = holes.gone.iter().find(|g| g.reference == "p/h/p1").unwrap();
        assert_eq!(gone.removed_by.as_ref().map(Id::as_str), Some("cut"));
        assert_eq!(holes.rows.len(), 3);
    }

    #[test]
    fn faces_say_which_hole_and_holes_are_references() {
        let (_dir, mut s) = drilled();
        put(
            &mut s,
            "h",
            json!({"format": 23, "points": ["p1"], "form": "simple",
                   "thread": {"standard": "iso_metric", "size": "M2"}, "depth": "through_all"}),
        );
        let holes = list(&mut s, &id("p"), None).unwrap();
        let name = holes.rows[0].faces["wall"][0].clone();
        let wall = format!("p/b/{name}");
        let i = call(&mut s, "inspect", json!({"part": "p", "entity": name})).unwrap();
        assert_eq!(
            i["entity"]["hole"],
            json!({"ref": "p/h/p1", "segment": "wall", "spec": "M2x0.4-6H THRU"}),
            "{i}"
        );
        assert_eq!(i["entity"]["thread"]["designation"], "M2x0.4-6H");
        let m = call(&mut s, "measure", json!({"entities": [wall]})).unwrap();
        assert_eq!(m["hole"]["ref"], "p/h/p1", "{m}");
        let m = call(&mut s, "measure", json!({"entities": ["p/h/p1"]})).unwrap();
        assert_eq!(m["hole"]["spec"], "M2x0.4-6H THRU", "{m}");
        // Selecting or highlighting a hole takes its faces.
        let refs = crate::entity::parse_all(&mut s, &["p/h/p1".to_owned()]).unwrap();
        let faces: usize = holes.rows[0].faces.values().map(Vec::len).sum();
        assert_eq!(refs.len(), faces);
        let e = crate::entity::parse_all(&mut s, &["p/h/p9".to_owned()]).unwrap_err();
        assert_eq!(e.code, crate::error::Code::NotFound);
    }

    #[test]
    fn a_through_hole_measures_its_depth_and_a_mirror_copies_its_holes() {
        let (_dir, mut s) = drilled();
        put(
            &mut s,
            "h",
            json!({"format": 23, "points": ["p1"], "form": "simple",
                   "thread": {"standard": "iso_metric", "size": "M3", "hand": "left"},
                   "depth": "through_all"}),
        );
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "mirror", "id": "m", "solid": "b", "plane": "yz",
                   "operation": "new"}),
        )
        .unwrap();
        let holes = list(&mut s, &id("p"), None).unwrap();
        let refs: Vec<&str> = holes.rows.iter().map(|r| r.reference.as_str()).collect();
        assert_eq!(refs, ["p/h/p1", "p/m/h/p1"], "{:?}", holes.gone);
        let (a, b) = (&holes.rows[0], &holes.rows[1]);
        // Through the 5 mm box: its depth is measured from its wall.
        assert!((a.depth.0 - 0.005).abs() < 1e-12, "{:?}", a.depth);
        assert_eq!(a.depth.2, Some(a.depth.0));
        assert_eq!(b.mirror.as_ref().map(Id::as_str), Some("m"));
        assert_eq!(b.solid.as_str(), "m");
        assert!((b.position - Point3::new(-0.010, 0.005, 0.005)).length() < 1e-12);
        assert!(
            (b.axis - a.axis).length() < 1e-15,
            "yz keeps a vertical axis"
        );
        assert!(b.left, "a mirror's copy keeps its hand");
        assert_eq!(b.faces.keys().collect::<Vec<_>>(), ["wall"]);
    }
    #[test]
    fn a_mirror_copies_its_sources_holes_as_they_were_when_it_copied() {
        let (_dir, mut s) = drilled();
        put(
            &mut s,
            "h",
            json!({"format": 23, "points": ["p1"], "form": "simple",
                   "thread": {"standard": "iso_metric", "size": "M3"},
                   "depth": "through_all"}),
        );
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "mirror", "id": "m", "solid": "b", "plane": "yz",
                   "operation": "new"}),
        )
        .unwrap();
        let of_m = |h: &Holes| -> (Vec<String>, Vec<String>) {
            (
                h.rows
                    .iter()
                    .filter(|r| r.solid.as_str() == "m")
                    .map(|r| r.reference.clone())
                    .collect(),
                h.gone
                    .iter()
                    .filter(|g| g.solid.as_str() == "m")
                    .map(|g| g.reference.clone())
                    .collect(),
            )
        };
        let before = of_m(&list(&mut s, &id("p"), None).unwrap());
        assert_eq!(before, (vec!["p/m/h/p1".to_owned()], vec![]));
        // A hole drilled into the source after the mirror isn't the
        // mirror's, nor gone from it.
        put(
            &mut s,
            "h2",
            json!({"points": ["p2"], "form": "simple", "diameter": 2,
                   "depth": "through_all"}),
        );
        let holes = list(&mut s, &id("p"), None).unwrap();
        assert!(holes.rows.iter().any(|r| r.reference == "p/h2/p2"));
        assert_eq!(of_m(&holes), before, "{:?}", holes.gone);
        // A cut taking the source's hole after the mirror leaves the copy.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "c", "operation": "cut", "targets": ["b"],
                   "params": {"x": 4, "y": 4, "z": 5}, "placement": {"origin": [8, 3, 0]}}),
        )
        .unwrap();
        let holes = list(&mut s, &id("p"), None).unwrap();
        let g = holes.gone.iter().find(|g| g.reference == "p/h/p1").unwrap();
        assert_eq!(g.removed_by.as_ref().map(Id::as_str), Some("c"));
        assert_eq!(of_m(&holes), before);
        // A cut in the mirror's own solid takes the copy, and is named.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "mc", "operation": "cut", "targets": ["m"],
                   "params": {"x": 4, "y": 4, "z": 5}, "placement": {"origin": [-12, 3, 0]}}),
        )
        .unwrap();
        let holes = list(&mut s, &id("p"), None).unwrap();
        assert_eq!(
            of_m(&holes),
            (vec![], vec!["p/m/h/p1".to_owned()]),
            "{:?}",
            holes.rows
        );
        let g = holes
            .gone
            .iter()
            .find(|g| g.reference == "p/m/h/p1")
            .unwrap();
        assert_eq!(g.removed_by.as_ref().map(Id::as_str), Some("mc"));
    }
}
