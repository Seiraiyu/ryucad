//! A fill pattern's lattice (design 533, Fill patterns): rows `spacing`
//! apart along a direction in a planar face's plane and `spacing2`
//! across, run both ways from the seed's places, each instance kept when
//! every seed place's widest mouth circle lies inside the face less the
//! margin. Inside is ryukei's certified point classification; the
//! distance to a straight or circular edge is exact in closed form, to
//! any other `measure`'s certified distance, and an instance it can't
//! decide is refused, never guessed.

use crate::doc::files::Feature;
use crate::doc::geometry::GeomRef;
use crate::doc::id::Id;
use crate::doc::pattern::{Instance, MAX_INSTANCES, Pattern};
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::geometry::{Want, one};
use crate::hole::place::{face_of, foot};
use crate::rebuild::Results;
use ryukei::geom::{Curve, Point3, Rigid, Surface};
use ryukei::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use ryukei::topo::{Body, FaceId};

/// The most lattice points a fill looks at before clipping.
const MAX_CANDIDATES: f64 = 10.0 * MAX_INSTANCES;

/// A fill's instances in order (`j` outer, `i` inner, each ascending),
/// the seed and its skips left out, each with its translation.
#[allow(clippy::too_many_arguments)]
pub fn lattice(
    part: &Part,
    id: &Id,
    p: &Pattern,
    face: &GeomRef,
    direction: Option<&GeomRef>,
    spacing: (f64, f64),
    margin: f64,
    before: &dyn Results,
) -> Result<Vec<(Instance, Rigid)>> {
    let at = super::at(part, id, before);
    let (b, f) = face_of(&at, "face", face)?;
    let body = &b.body;
    let r = face.reference().expect("face_of took a reference");
    let (o, n) =
        crate::reference::plane(part, id, "face", r, (b, ryukei::topo::EntityId::Face(f)))?;
    // The lattice's x: the direction given, in the face's plane, else the
    // face's own x.
    let x = match direction {
        Some(d) => {
            let v = one(&at, "direction", d, Want::Axis)?.axis().dir.get();
            let v = v - n * v.dot(n);
            if v.length() <= ANGULAR_RESOLUTION {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("pattern {id}: its direction runs along its face's normal"),
                )
                .with("feature", id)
                .with("pointer", "/direction"));
            }
            v * (1.0 / v.length())
        }
        None => {
            let face = body.face(f).expect("a live face");
            match body.surface(face.surface) {
                Some(Surface::Plane(pl)) => pl.frame().x().get(),
                _ => unreachable!("reference::plane took a planar face"),
            }
        }
    };
    let y = n.cross(x);
    // Each seed place's mouth on the face, with the radius it needs.
    let mut circles: Vec<(Point3, f64)> = Vec::new();
    for seed in &p.features {
        let Some(Ok(Feature::Hole(h))) = part.features.get(seed) else {
            unreachable!("check_seeds took holes only for a fill")
        };
        let sizes = crate::hole::derive::derive(h, part.hole_standards.as_deref())
            .map_err(crate::hole::derive::at_rebuild)?;
        let rad = crate::hole::mouth_radius(&sizes) + margin;
        for (k, m, down) in crate::hole::mouths(part, seed, h, before)? {
            let off = (m - o).dot(n).abs();
            let along = down.cross(n).length() <= 1e-9 && down.dot(n) < 0.0;
            if off > 10.0 * LINEAR_RESOLUTION || !along {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "pattern {id}: its seed {seed} at {k} isn't drilled into its face along its normal"
                    ),
                )
                .with("feature", id)
                .with("seed", seed)
                .with("place", &k)
                .with("reason", "seed_off_face")
                .with("pointer", "/face"));
            }
            circles.push((m, rad));
        }
    }
    // The lattice's extent: the face's box seen from each seed place.
    let bx = ryukei::intersect::face_box(body, f);
    let (lo, hi) = (bx.min(), bx.max());
    let corners = [
        Point3::new(lo.x, lo.y, lo.z),
        Point3::new(hi.x, lo.y, lo.z),
        Point3::new(lo.x, hi.y, lo.z),
        Point3::new(hi.x, hi.y, lo.z),
        Point3::new(lo.x, lo.y, hi.z),
        Point3::new(hi.x, lo.y, hi.z),
        Point3::new(lo.x, hi.y, hi.z),
        Point3::new(hi.x, hi.y, hi.z),
    ];
    let (s1, s2) = spacing;
    let (mut i0, mut i1, mut j0, mut j1) = (0i64, 0i64, 0i64, 0i64);
    for (c, _) in &circles {
        for k in corners {
            let (u, v) = ((k - *c).dot(x) / s1, (k - *c).dot(y) / s2);
            // Saturating casts: a lattice too wide is refused just below.
            i0 = i0.min(u.floor() as i64);
            i1 = i1.max(u.ceil() as i64);
            j0 = j0.min(v.floor() as i64);
            j1 = j1.max(v.ceil() as i64);
        }
    }
    let candidates = ((i1 - i0 + 1) as f64) * ((j1 - j0 + 1) as f64);
    if candidates > MAX_CANDIDATES
        || i0 < i64::from(i32::MIN)
        || i1 > i64::from(i32::MAX)
        || j0 < i64::from(i32::MIN)
        || j1 > i64::from(i32::MAX)
    {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "pattern {id}: its spacing lays {candidates} lattice points over its face; widen it"
            ),
        )
        .with("feature", id)
        .with("pointer", "/spacing"));
    }
    let edges = edges_of(body, f);
    let probe = Probe::new();
    let work = ryukei::work::Work::unlimited();
    let mut out = Vec::new();
    for j in j0..=j1 {
        for i in i0..=i1 {
            let ij = [i as i32, j as i32];
            if ij == [0, 0] || p.skip.contains(&ij) {
                continue;
            }
            let t = x * (i as f64 * s1) + y * (j as f64 * s2);
            if fits(body, f, &edges, &probe, &circles, t, &work).map_err(|what| {
                Error::new(
                    Code::RebuildFailed,
                    format!("pattern {id}: whether instance {ij:?} {what} is uncertain"),
                )
                .with("feature", id)
                .with("instance", ij.to_vec())
            })? {
                out.push((ij, Rigid::translation(t)));
            }
        }
    }
    if out.len() as f64 + 1.0 > MAX_INSTANCES {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "pattern {id}: it keeps {} instances, more than {MAX_INSTANCES}",
                out.len() + 1
            ),
        )
        .with("feature", id)
        .with("pointer", "/spacing"));
    }
    Ok(out)
}

/// Whether every seed circle moved by `t` lies inside the face and clear
/// of its edges; `Err` names what couldn't be told.
fn fits(
    body: &Body,
    f: FaceId,
    edges: &[EdgeOf],
    probe: &Probe,
    circles: &[(Point3, f64)],
    t: ryukei::geom::Vector3,
    work: &ryukei::work::Work,
) -> core::result::Result<bool, &'static str> {
    for (c, rad) in circles {
        let q = *c + t;
        match foot(body, f, q).3 {
            Some(true) => {}
            Some(false) => return Ok(false),
            None => return Err("lies inside its face"),
        }
        for e in edges {
            match e.clear(body, probe, q, *rad, work) {
                Some(true) => {}
                Some(false) => return Ok(false),
                None => return Err("keeps its margin from its face's edge"),
            }
        }
    }
    Ok(true)
}

/// One edge of a face: its ID, curve and parameter interval.
struct EdgeOf {
    id: ryukei::topo::EdgeId,
    curve: Curve,
    interval: [f64; 2],
}

/// A body of one vertex at the origin, moved to a point to measure from
/// it (`measure` takes entities of bodies, each placed by a `Rigid`).
struct Probe {
    body: Body,
    vertex: ryukei::topo::VertexId,
}

impl Probe {
    fn new() -> Probe {
        let mut body = Body::new();
        let plane = body.add_surface(Surface::Plane(ryukei::geom::Plane::new(
            ryukei::geom::Frame::WORLD,
        )));
        let (m, _) = ryukei::euler::mvfs(&mut body, ryukei::name::OpKey(1), Point3::ORIGIN, plane)
            .expect("a vertex at the origin");
        Probe {
            body,
            vertex: m.vertex,
        }
    }
}

impl EdgeOf {
    /// Whether `q` is at least `need` from the edge (not its whole
    /// carrier): `Some(true)` or `Some(false)` when that is certain,
    /// `None` when it can't be told. A line or a circle projects in
    /// closed form (the parameter clamped to the interval, else the
    /// nearer end); any other curve is `measure`'s certified distance
    /// from the edge to the probe moved to `q`, whose bound must clear
    /// `need` one way or the other.
    fn clear(
        &self,
        body: &Body,
        probe: &Probe,
        q: Point3,
        need: f64,
        work: &ryukei::work::Work,
    ) -> Option<bool> {
        let [t0, t1] = self.interval;
        match self.curve {
            Curve::Line(_) | Curve::Circle(_) => {
                let pr = self.curve.project(q);
                let mut t = pr.param;
                if let Curve::Circle(_) = self.curve {
                    // An angle: bring it into [t0, t0 + 2π).
                    let tau = core::f64::consts::TAU;
                    while t < t0 {
                        t += tau;
                    }
                    while t >= t0 + tau {
                        t -= tau;
                    }
                }
                let d = if t >= t0 && t <= t1 {
                    pr.distance
                } else {
                    (q - self.curve.point(t0))
                        .length()
                        .min((q - self.curve.point(t1)).length())
                };
                Some(d >= need - LINEAR_RESOLUTION)
            }
            _ => {
                use ryukei::measure::{DistanceOptions, Entity, distance_with};
                let d = distance_with(
                    (body, Entity::Edge(self.id), &Rigid::IDENTITY),
                    (
                        &probe.body,
                        Entity::Vertex(probe.vertex),
                        &Rigid::translation(q.to_vector()),
                    ),
                    &DistanceOptions::default(),
                    work,
                )
                .ok()?;
                let (v, bound) = (d.distance.value(), d.distance.bound());
                if v - bound >= need - LINEAR_RESOLUTION {
                    Some(true)
                } else if v + bound < need - LINEAR_RESOLUTION {
                    Some(false)
                } else {
                    None
                }
            }
        }
    }
}

fn edges_of(body: &Body, f: FaceId) -> Vec<EdgeOf> {
    let mut out = Vec::new();
    for l in body.face_loops(f) {
        for fin in body.loop_fins(l) {
            let id = body.fin(fin).expect("live fin").edge;
            let e = body.edge(id).expect("live edge");
            let curve = body.curve(e.curve).expect("live curve").clone();
            out.push(EdgeOf {
                id,
                curve,
                interval: e.interval,
            });
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::doc::id::Id;
    use crate::error::Code;
    use crate::session::Session;
    use crate::testing::call;
    use serde_json::{Value, json};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// The name of box `b`'s planar face whose outward normal is `z`
    /// along z.
    pub(crate) fn face_z(s: &mut Session, z: f64) -> String {
        let body =
            call(s, "inspect", json!({"part": "p", "feature": "b"})).unwrap()["bodies"][0].clone();
        body["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
                f["surface"]["type"] == "plane"
                    && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == z
            })
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .unwrap()
    }

    /// Box `b` 100 × 60 × 10 mm, a sketch `s` on its top with a point
    /// `p1` at (10, 10) mm, a hole `h` there of `dia` mm through all, and
    /// the box's top face's name.
    pub(crate) fn plate(dia: f64) -> (tempfile::TempDir, Session, String) {
        let (dir, mut s) = crate::testing::fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b",
                   "params": {"x": 100, "y": 60, "z": 10}}),
        )
        .unwrap();
        let top = face_z(&mut s, 1.0);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s",
                   "placement": {"origin": [0, 0, 10]}}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "type": "point", "at": [10, 10]},
                {"op": "add", "type": "fix", "a": "p1"}]}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "hole", "id": "h", "sketch": "s",
                   "diameter": dia, "depth": "through_all"}),
        )
        .unwrap();
        (dir, s, top)
    }

    fn motions(s: &mut Session, fill: Value) -> crate::error::Result<Vec<([i32; 2], [i64; 2])>> {
        let pid = Id::parse("p", "part").unwrap();
        let r = s.rebuilt(&pid).unwrap().clone();
        let part = s.project().part(&pid).unwrap().clone();
        let mut v = fill;
        v["form"] = json!("fill");
        v["features"] = json!(["h"]);
        v["targets"] = json!(["b"]);
        let mut o = crate::doc::files::Obj::new(&v, "").unwrap();
        let p = crate::doc::pattern::Pattern::from_fields(
            &mut o,
            crate::units::Unit::Mm,
            &crate::expr::Env::empty(),
        )
        .unwrap();
        // As writing the fill last would leave the part, so its face
        // resolves as the fill sees it.
        let f = crate::doc::files::Feature::Pattern(p.clone());
        let mut order = part.order().to_vec();
        order.push(id("f"));
        let part = crate::part::proposed(&part, &order, &[(&id("f"), &f)]);
        let m = crate::pattern::motions(&part, &id("f"), &p, &r)?;
        Ok(m.into_iter()
            .map(|(i, m)| match m {
                crate::pattern::Motion::Rigid(r) => {
                    let t = r.translation_part();
                    (i, [(t.x * 1e3).round() as i64, (t.y * 1e3).round() as i64])
                }
                crate::pattern::Motion::Mirror(_) => unreachable!(),
            })
            .collect())
    }

    #[test]
    fn a_fill_lays_a_lattice_both_ways_clipped_to_the_face() {
        let (_dir, mut s, top) = plate(5.0);
        // 20 mm along x, 20 across: the seed at (10, 10) has 4 columns to
        // its right (30, 50, 70, 90) and 2 rows above (30, 50); none fits
        // left of it or below (−10 is off the face).
        let m = motions(
            &mut s,
            json!({"face": {"feature": "b", "name": top}, "direction": "x",
                   "spacing": "20 mm"}),
        )
        .unwrap();
        assert_eq!(m.len(), 5 * 3 - 1, "{m:?}");
        assert_eq!(m[0], ([1, 0], [20, 0]));
        assert_eq!(m.last().unwrap(), &([4, 2], [80, 40]));
        // A margin of 8 mm needs 10.5 mm from every edge: the column at
        // x = 90 (10 mm from the far edge) goes.
        let m8 = motions(
            &mut s,
            json!({"face": {"feature": "b", "name": top}, "direction": "x",
                   "spacing": "20 mm", "margin": "8 mm"}),
        )
        .unwrap();
        assert!(m8.iter().all(|(i, _)| i[0] < 4), "{m8:?}");
        // The seed itself is never an instance; a skip takes one out.
        let sk = motions(
            &mut s,
            json!({"face": {"feature": "b", "name": top}, "direction": "x",
                   "spacing": "20 mm", "skip": [[1, 0]]}),
        )
        .unwrap();
        assert_eq!(sk.len(), m.len() - 1);
    }

    #[test]
    fn a_lattice_running_back_from_the_seed_takes_negative_indices() {
        let (_dir, mut s, top) = plate(2.0);
        // Spacing 4.6 mm from the Ø2 seed at (10, 10): columns at 5.4 and
        // 0.8 lie left of it (−1, −2); 0.8 < 1 from the edge drops −2.
        // (Each also keeps clear of the seed's own bore, an inner loop of
        // the face.)
        let m = motions(
            &mut s,
            json!({"face": {"feature": "b", "name": top}, "direction": "x",
                   "spacing": "4.6 mm", "spacing2": "50 mm"}),
        )
        .unwrap();
        let row0: Vec<i32> = m
            .iter()
            .filter(|(i, _)| i[1] == 0)
            .map(|(i, _)| i[0])
            .collect();
        assert_eq!(row0.first(), Some(&-1), "{row0:?}");
        assert!(!row0.contains(&-2));
    }

    #[test]
    fn a_fill_of_too_many_instances_is_refused() {
        let (_dir, mut s, top) = plate(0.01);
        // 2,000 × 1,200 lattice points: refused before any is classified.
        let e = motions(
            &mut s,
            json!({"face": {"feature": "b", "name": top}, "spacing": "0.05 mm"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["pointer"], "/spacing");
    }

    /// The plate cut through by a cylinder of radius 10 mm tilted 45 deg
    /// about y through (70, 30, 10) mm: the top face gains an elliptical
    /// edge (semi-axes 10√2 along x and 10 along y, centred at (70, 30)).
    fn with_ellipse(s: &mut Session) {
        let d = core::f64::consts::FRAC_1_SQRT_2;
        let origin = [70.0 - 30.0 * d, 30.0, 10.0 - 30.0 * d];
        call(
            s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "slant",
                   "operation": "cut", "targets": ["b"], "params": {"radius": 10, "height": 60},
                   "placement": {"origin": origin, "z": [d, 0.0, d], "x": [d, 0.0, -d]}}),
        )
        .unwrap();
    }

    #[test]
    fn an_elliptical_edge_is_kept_clear_by_the_certified_distance() {
        let (_dir, mut s, top) = plate(5.0);
        with_ellipse(&mut s);
        // Instance [2, 1] sits at (50, 30) mm on the ellipse's major axis,
        // 70 − 10√2 − 50 = 5.858 mm from its near vertex; [4, 1] at
        // (90, 30) the same from the far one. With the hole's 2.5 mm
        // radius, a margin of 3.3 mm needs 5.8 (kept), 3.4 needs 5.9 (gone).
        let fill = |m: &str| {
            json!({"face": {"feature": "b", "name": top}, "direction": "x",
                   "spacing": "20 mm", "margin": m})
        };
        let kept = motions(&mut s, fill("3.3 mm")).unwrap();
        let has = |m: &Vec<([i32; 2], [i64; 2])>, i: [i32; 2]| m.iter().any(|(x, _)| *x == i);
        assert!(has(&kept, [2, 1]) && has(&kept, [4, 1]), "{kept:?}");
        // [3, 1] at (70, 30) is inside the cut: never kept.
        assert!(!has(&kept, [3, 1]));
        let gone = motions(&mut s, fill("3.4 mm")).unwrap();
        assert!(!has(&gone, [2, 1]) && !has(&gone, [4, 1]), "{gone:?}");
        // The edge really is no line or circle: the certified path ran.
        let pid = Id::parse("p", "part").unwrap();
        let r = s.rebuilt(&pid).unwrap().clone();
        let b = r
            .features
            .last()
            .unwrap()
            .1
            .as_ref()
            .unwrap()
            .solid(&id("b"))
            .unwrap();
        assert!(b.body.edges().any(|e| !matches!(
            b.body.curve(b.body.edge(e).unwrap().curve),
            Some(ryukei::geom::Curve::Line(_) | ryukei::geom::Curve::Circle(_))
        )));
    }

    #[test]
    fn a_seed_off_the_face_is_refused() {
        let (_dir, mut s, _) = plate(5.0);
        // The bottom face: the seed's mouth isn't on it.
        let bottom = face_z(&mut s, -1.0);
        let e = motions(
            &mut s,
            json!({"face": {"feature": "b", "name": bottom}, "spacing": "20 mm"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["reason"].as_str()),
            (Code::InvalidParam, Some("seed_off_face"))
        );
    }
}
