//! Cosmetic threads: a tapped hole's thread as data on its `wall` faces,
//! and a `cosmetic_thread` feature's on a shaft's or a bore's cylindrical
//! faces (ryucad 28.B), for the app's rings, renders and (28.D) drawings.
//! Never kernel geometry: a hole's is derived from the holes the rebuild
//! made, cached with them, never on disk; a feature's is its rebuild's
//! `Output::Thread`.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::holes::{Holes, Row};
use ryukei::geom::{Point3, Vector3};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    /// The hole's reference.
    pub hole: String,
    pub spec: String,
    pub designation: String,
    pub standard: String,
    /// Metres; a pipe thread's (no built-in major) is its drill's.
    pub major: f64,
    /// The minor diameter's limits, when the standard gives them.
    pub minor: Option<[f64; 2]>,
    pub pitch: f64,
    pub lead: f64,
    pub left: bool,
    pub starts: u32,
    pub class: Option<String>,
    /// Internal (a tapped hole, a bore) or external (a stud).
    pub internal: bool,
    /// `<part>/<feature>` for a cosmetic thread; none for a hole's.
    pub feature: Option<String>,
    /// The `wall` faces it lies on, as `<part>/<solid>/<name>`.
    pub faces: Vec<String>,
    pub axis: Vector3,
    /// Where it starts (the wall's top) and ends, on the axis.
    pub mouth: Point3,
    pub end: Point3,
}

impl Thread {
    pub fn to_json(&self) -> Value {
        let p = |x: Point3| json!([x.x, x.y, x.z]);
        let v = json!({
            "hole": self.hole, "spec": self.spec, "designation": self.designation,
            "standard": self.standard, "major": self.major, "minor": self.minor,
            "pitch": self.pitch, "lead": self.lead,
            "hand": if self.left { "left" } else { "right" }, "starts": self.starts,
            "class": self.class, "internal": self.internal, "faces": self.faces,
            "axis": [self.axis.x, self.axis.y, self.axis.z],
            "mouth": p(self.mouth), "end": p(self.end),
        });
        // A cosmetic thread is named by its feature; a hole's keeps every
        // key it had (the hole_specs golden hashes them).
        let mut v = v;
        if let Some(f) = &self.feature {
            v.as_object_mut().expect("an object").remove("hole");
            v["feature"] = json!(f);
        }
        v
    }

    /// Where its rings are drawn: an internal thread's major diameter, an
    /// external one's minor.
    pub fn ring_diameter(&self) -> f64 {
        if self.internal {
            self.major
        } else {
            self.minor.map_or(self.major, |m| m[0])
        }
    }
}

/// Every tapped hole's thread, in the holes' order.
pub fn of(holes: &Holes) -> Vec<Thread> {
    holes.rows.iter().filter_map(of_row).collect()
}

/// A row's thread, if it is tapped.
pub fn of_row(r: &Row) -> Option<Thread> {
    let t = r.sizes.thread.as_ref()?;
    let start = r.sizes.thread_start();
    let length = r.depth.2.unwrap_or(r.depth.0);
    Some(Thread {
        hole: r.reference.clone(),
        spec: r.spec.clone(),
        designation: t.designation.clone(),
        standard: t.standard.clone(),
        major: t.major.as_ref().map_or(r.sizes.drill.si, |m| m.si),
        minor: t.minor.as_ref().map(|[a, b]| [a.si, b.si]),
        pitch: t.pitch.si,
        lead: t.lead.si,
        left: r.left,
        starts: t.starts,
        class: t.class.clone(),
        internal: true,
        feature: None,
        faces: r
            .faces
            .get("wall")
            .into_iter()
            .flatten()
            .map(|n| format!("{}/{}/{n}", r.part, r.solid))
            .collect(),
        axis: r.axis,
        mouth: r.position + r.axis * start,
        end: r.position + r.axis * length,
    })
}

/// What a cosmetic_thread feature builds: its thread, and its warnings.
#[derive(Clone, Debug, PartialEq)]
pub struct Cosmetic {
    pub thread: Thread,
    pub warnings: Vec<Value>,
    pub hash: String,
}

/// `invalid_param` for cosmetic thread `owner`.
fn refuse(owner: &Id, at: &str, reason: &str, msg: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("cosmetic thread {owner}: {msg}"),
    )
    .with("feature", owner)
    .with("pointer", at)
    .with("reason", reason)
}

/// Each of a cosmetic thread's faces, as `owner` would see it, is a
/// cylindrical face (`reference_wrong_kind` if not): what a write checks.
/// What the faces make together (one axis, radius and side) only rebuild
/// tells.
pub fn check_faces(
    part: &crate::doc::project::Part,
    order: &[Id],
    owner: &Id,
    c: &crate::doc::thread::CosmeticThread,
    results: &dyn crate::rebuild::Results,
) -> Result<()> {
    use ryukei::geom::Surface;
    use ryukei::topo::EntityId;
    for (i, r) in c.faces.iter().enumerate() {
        let role = format!("faces.{i}");
        let (b, ids) = crate::reference::resolve_all(part, order, owner, &role, r, results)?;
        for id in ids {
            let cylinder = match id {
                EntityId::Face(f) => matches!(
                    b.body.face(f).and_then(|x| b.body.surface(x.surface)),
                    Some(Surface::Cylinder(_))
                ),
                _ => false,
            };
            if !cylinder {
                let kind = match id {
                    EntityId::Face(_) => "face that isn't a cylinder",
                    EntityId::Edge(_) => "edge",
                    _ => "vertex",
                };
                return Err(crate::reference::wrong_kind(
                    part,
                    owner,
                    &role,
                    r,
                    kind,
                    "a cylindrical face",
                ));
            }
        }
    }
    Ok(())
}

/// A cosmetic thread from its faces as `owner` sees them.
pub fn build(
    part: &crate::doc::project::Part,
    owner: &Id,
    c: &crate::doc::thread::CosmeticThread,
    results: &dyn crate::rebuild::Results,
) -> Result<Cosmetic> {
    use crate::reference::{Follow, resolve, resolve_all};
    use ryukei::geom::Surface;
    use ryukei::topo::{EntityId, Sense};
    let order = part.order();
    // Every face cylindrical, of one body, on one axis, of one radius and
    // one side.
    let mut cyl: Option<(Point3, Vector3, f64, bool)> = None;
    let mut body: Option<&crate::rebuild::Built> = None;
    let mut names = Vec::new();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (i, r) in c.faces.iter().enumerate() {
        let role = format!("faces.{i}");
        let at = format!("/faces/{i}");
        let (b, ids) = resolve_all(part, order, owner, &role, r, results)?;
        if body.is_some_and(|x| !std::ptr::eq(x, b)) {
            return Err(refuse(
                owner,
                &at,
                "faces_not_coaxial",
                "its faces are of one body",
            ));
        }
        body = Some(b);
        for id in ids {
            let not_cylinder = || {
                crate::reference::wrong_kind(part, owner, &role, r, "face", "a cylindrical face")
            };
            let EntityId::Face(f) = id else {
                return Err(not_cylinder());
            };
            let face = b.body.face(f).expect("a face of the body");
            let surface = b.body.surface(face.surface);
            let (Some(Surface::Cylinder(k)), Some(frame)) =
                (surface, surface.and_then(Surface::frame))
            else {
                return Err(not_cylinder());
            };
            // A cylinder's own normal points away from its axis: a face of
            // the same sense is outside (a stud), else a bore.
            let external = face.sense == Sense::Forward;
            let (o, d, rad) = (frame.origin(), frame.z().get(), k.radius());
            match cyl {
                None => cyl = Some((o, d, rad, external)),
                Some((o0, d0, r0, e0)) => {
                    let off = (o - o0) - d0 * (o - o0).dot(d0);
                    if d.cross(d0).length() > 1e-9
                        || off.length() > ryukei::tol::LINEAR_RESOLUTION
                        || (rad - r0).abs() > ryukei::tol::LINEAR_RESOLUTION
                        || external != e0
                    {
                        return Err(refuse(
                            owner,
                            &at,
                            "faces_not_coaxial",
                            "its faces share one axis, one radius and one side",
                        ));
                    }
                }
            }
            let (o0, d0, ..) = cyl.expect("set");
            for l in b.body.face_loops(f) {
                for fin in b.body.loop_fins(l) {
                    if let Some(v) = b.body.fin_start(fin).and_then(|v| b.body.vertex(v)) {
                        let t = (v.point - o0).dot(d0);
                        lo = lo.min(t);
                        hi = hi.max(t);
                    }
                }
            }
            names.push(format!("{}/{}/{}", part.id, r.feature, face.name));
        }
    }
    let (o, d, radius, external) = cyl.expect("faces is never empty");
    // No vertices (lo, hi infinite) or no length: nothing to thread.
    if (hi - lo).partial_cmp(&ryukei::tol::LINEAR_RESOLUTION) != Some(std::cmp::Ordering::Greater) {
        return Err(refuse(
            owner,
            "/faces",
            "faces_not_coaxial",
            "its faces have no length along their axis",
        ));
    }
    // Which end it starts from: `from`'s, else the axis's far end.
    let start_hi = match &c.from {
        Some(r) => {
            let (b, e) = resolve(part, order, owner, "from", Follow::One, r, results)?;
            let point = match e {
                EntityId::Edge(e) => b
                    .body
                    .edge_fins(e)
                    .filter_map(|f| b.body.fin_start(f))
                    .find_map(|v| b.body.vertex(v))
                    .map(|v| v.point),
                EntityId::Vertex(v) => b.body.vertex(v).map(|v| v.point),
                _ => None,
            }
            .ok_or_else(|| {
                crate::reference::wrong_kind(part, owner, "from", r, "face", "an edge or a vertex")
            })?;
            let t = (point - o).dot(d);
            (t - hi).abs() <= (t - lo).abs()
        }
        None => true,
    };
    let length = c
        .depth
        .as_ref()
        .map_or(hi - lo, |q| q.meters().min(hi - lo));
    let (mouth_t, end_t, axis) = if start_hi {
        (hi, hi - length, -d)
    } else {
        (lo, lo + length, d)
    };
    // The thread, by its side's default class when none is written.
    let mut tt = c.thread.clone();
    if tt.class.is_none() {
        tt.class = default_class(&tt.standard, external);
    }
    let n = numbers(part, owner, &tt, external)?;
    let thread = Thread {
        hole: String::new(),
        feature: Some(format!("{}/{owner}", part.id)),
        spec: n.designation.clone(),
        designation: n.designation,
        standard: tt.standard.clone(),
        major: n.major,
        minor: n.minor,
        pitch: n.pitch,
        lead: n.lead,
        left: tt.left,
        starts: tt.starts.unwrap_or(1),
        class: tt.class.clone(),
        internal: !external,
        faces: names,
        axis,
        mouth: o + d * mouth_t,
        end: o + d * end_t,
    };
    let warnings = mismatch(part, owner, &thread, radius * 2.0, n.range);
    let hash = blake3::hash(crate::doc::canonical::to_string(&thread.to_json()).as_bytes())
        .to_hex()[..32]
        .to_owned();
    Ok(Cosmetic {
        thread,
        warnings,
        hash,
    })
}

/// A cosmetic thread's numbers, in metres.
struct Numbers {
    designation: String,
    major: f64,
    minor: Option<[f64; 2]>,
    pitch: f64,
    lead: f64,
    /// The face's diameter the class takes (an external thread's major
    /// limits, an internal one's minor limits and tap drill), for
    /// `thread_size_mismatch`; none where nothing built in gives it.
    range: Option<[f64; 2]>,
}

/// The thread's numbers: the built-in's (its side checked against the
/// faces'), else the project's entry. A unified external class (1A, 2A,
/// 3A) isn't built in, as H28/2B prints no external limits: the
/// project's table gives it, as it gives a house standard's.
fn numbers(
    part: &crate::doc::project::Part,
    owner: &Id,
    t: &crate::doc::hole::Thread,
    external: bool,
) -> Result<Numbers> {
    use crate::hole::derive::{Source, needs_table, pitch_size, table};
    use ryu_standards::thread::{Pitch, Standard};
    let nm = |v: i64| v as f64 / 1e9;
    let unified = Standard::parse(&t.standard).is_some_and(|s| {
        matches!(
            s,
            Standard::Unc | Standard::Unf | Standard::Unef | Standard::Un
        )
    });
    let unified_external = unified
        && t.class
            .as_deref()
            .is_some_and(|c| c.ends_with('A') || c.ends_with('a'));
    let built = if unified_external {
        None
    } else {
        crate::doc::hole::resolve_any(t)?
    };
    let starts = f64::from(t.starts.unwrap_or(1));
    if let Some(b) = &built {
        if b.class.is_some() && b.internal == external {
            return Err(refuse(
                owner,
                "/thread/class",
                "thread_side",
                &format!(
                    "{} is an {} class, and its faces are {}",
                    t.class.as_deref().unwrap_or(""),
                    if b.internal { "internal" } else { "external" },
                    if external { "a shaft's" } else { "a bore's" }
                ),
            ));
        }
        let pitch = pitch_size(b.pitch.value, Source::Rule).si;
        let major = b.major.map_or(0.0, |m| nm(m.value));
        let minor = if b.internal {
            b.minor.map(|(lo, hi)| [nm(lo.value), nm(hi.value)])
        } else {
            b.basic_minor.map(|m| [nm(m.value), nm(m.value)])
        };
        let range = if b.internal {
            b.minor.map(|(lo, hi)| {
                let tap = ryu_standards::tap::rule(b).map_or(nm(hi.value), |d| nm(d.nm));
                [nm(lo.value), nm(hi.value).max(tap)]
            })
        } else {
            b.major_min
                .zip(b.major_max)
                .map(|(lo, hi)| [nm(lo.value), nm(hi.value)])
        };
        let designation = crate::hole::derive::designation(
            &t.standard,
            &t.size,
            Some(b.pitch.value),
            b.class.as_deref(),
            t.left,
        );
        return Ok(Numbers {
            designation,
            major,
            minor,
            pitch,
            lead: pitch * starts,
            range,
        });
    }
    // The project's entry.
    let entry = table(part.hole_standards.as_deref())?.and_then(|p| p.thread(&t.standard, &t.size));
    let missing = || {
        needs_table(
            crate::doc::hole_standards::Kind::Thread,
            &t.standard,
            &t.size,
            vec!["pitch".into(), "major".into()],
        )
        .with("feature", owner)
    };
    let e = entry.ok_or_else(missing)?;
    let pitch = match (&e.pitch, &e.tpi) {
        (Some(p), _) => Pitch::Mm((p.meters() * 1e9).round() as i64),
        (None, Some(n)) => Pitch::Tpi10((n.number() * 10.0).round() as u32),
        (None, None) => return Err(missing()),
    };
    use crate::doc::hole_standards::Diameters;
    let class_limits = |d: &Option<Diameters>| match d {
        Some(Diameters::Classes(m)) => t
            .class
            .as_ref()
            .and_then(|c| m.get(c))
            .map(|[a, b]| [a.meters(), b.meters()]),
        _ => None,
    };
    let major = match &e.major {
        Some(Diameters::Basic(q)) => q.meters(),
        _ => match class_limits(&e.major) {
            Some([_, hi]) => hi,
            None => return Err(missing()),
        },
    };
    let pitch_si = pitch_size(pitch, Source::Project).si;
    let range = if external {
        class_limits(&e.major)
    } else {
        class_limits(&e.minor)
    };
    Ok(Numbers {
        designation: crate::hole::derive::designation(
            &t.standard,
            &t.size,
            Some(pitch),
            t.class.as_deref(),
            t.left,
        ),
        major,
        minor: class_limits(&e.minor),
        pitch: pitch_si,
        lead: pitch_si * starts,
        range,
    })
}

/// The class a thread takes on that side when none is written: ISO
/// metric's 6g or 6H, unified's 2A or 2B (2A from a project's table);
/// pipe threads none.
fn default_class(standard: &str, external: bool) -> Option<String> {
    use ryu_standards::thread::Standard;
    match Standard::parse(standard)? {
        Standard::IsoMetric => Some(if external { "6g" } else { "6H" }.into()),
        Standard::Unc | Standard::Unf | Standard::Unef | Standard::Un => {
            Some(if external { "2A" } else { "2B" }.into())
        }
        _ => None,
    }
}

/// `thread_size_mismatch`, a warning: the face's diameter outside the
/// class's range widened by 0.1 mm each way.
fn mismatch(
    part: &crate::doc::project::Part,
    owner: &Id,
    t: &Thread,
    diameter: f64,
    range: Option<[f64; 2]>,
) -> Vec<Value> {
    let Some([lo, hi]) = range else {
        return Vec::new();
    };
    let (lo, hi) = (lo - 1e-4, hi + 1e-4);
    if (lo..=hi).contains(&diameter) {
        return Vec::new();
    }
    let mm = |x: f64| crate::measure::number(x * 1e3);
    vec![json!({
        "code": "thread_size_mismatch", "part": part.id, "feature": owner,
        "face": t.faces.first(), "diameter_m": diameter, "expected_m": [lo, hi],
        "message": format!(
            "cosmetic thread {owner}: its face is {} mm across; {} wants {} to {} mm",
            mm(diameter), t.designation, mm(lo), mm(hi)
        ),
    })]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::id::Id;
    use crate::hole::tests::{drilled, put};
    use serde_json::json;

    #[test]
    fn a_tapped_holes_thread() {
        let (_dir, mut s) = drilled();
        put(
            &mut s,
            "h",
            json!({"format": 23, "points": ["p1"], "form": "simple",
                   "thread": {"standard": "iso_metric", "size": "M3", "depth": "2.5 mm"},
                   "depth": {"blind": "4 mm"}}),
        );
        let holes = crate::holes::list(&mut s, &Id::parse("p", "part").unwrap(), None).unwrap();
        let t = of(&holes);
        assert_eq!(t.len(), 1);
        let t = &t[0];
        assert_eq!(t.hole, "p/h/p1");
        assert_eq!(t.designation, "M3x0.5-6H");
        assert!(t.internal && !t.left);
        assert_eq!((t.major, t.pitch), (0.003, 0.0005));
        // From the mouth (the box's top, z = 5 mm) down 2.5 mm.
        assert!((t.mouth - Point3::new(0.010, 0.005, 0.005)).length() < 1e-12);
        assert!((t.end - Point3::new(0.010, 0.005, 0.0025)).length() < 1e-12);
        assert_eq!(t.faces.len(), 1);
        assert!(t.faces[0].starts_with("p/b/"), "{:?}", t.faces);
        let v = t.to_json();
        assert_eq!(v["designation"], "M3x0.5-6H");
        // ISO 965-1's formula (T_D1(6) at 0.5 mm, 132 µm): metric limits
        // are the formula's, never printed values (28.A's Task 6).
        assert_eq!(v["minor"], json!([0.002459, 0.002591]));
    }

    /// Part p's 10 mm shaft (`shaft`, radius 4.95 mm, height 20 mm, at x
    /// 40 mm): its side face's and its top edge's references.
    pub(crate) fn shaft(s: &mut crate::session::Session) -> (Value, Value) {
        crate::testing::call(
            s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "shaft",
                   "params": {"radius": "4.95 mm", "height": "20 mm"},
                   "placement": {"origin": [40, 0, 0]}}),
        )
        .unwrap();
        let r = s.rebuilt(&Id::parse("p", "part").unwrap()).unwrap();
        let shaft = Id::parse("shaft", "feature").unwrap();
        let b = r.output(&shaft).unwrap().solid(&shaft).unwrap().clone();
        let side = b
            .body
            .faces()
            .find(|f| {
                matches!(
                    b.body.surface(b.body.face(*f).unwrap().surface),
                    Some(ryukei::geom::Surface::Cylinder(_))
                )
            })
            .unwrap();
        let top = b
            .body
            .edges()
            .find(|e| {
                b.body.edge_fins(*e).any(|f| {
                    b.body
                        .fin_start(f)
                        .and_then(|v| b.body.vertex(v))
                        .is_some_and(|v| (v.point.z - 0.020).abs() < 1e-9)
                })
            })
            .unwrap();
        (
            json!({"feature": "shaft", "name": b.body.face(side).unwrap().name.to_string()}),
            json!({"feature": "shaft", "name": b.body.edge(top).unwrap().name.to_string()}),
        )
    }

    /// Writes cosmetic thread `t` (feature.add takes it with Task 22).
    fn cosmetic(s: &mut crate::session::Session, v: Value) {
        let mut v = v;
        v["format"] = json!(24);
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!("cosmetic_thread");
        let f = crate::doc::files::Feature::from_json(
            &v,
            crate::units::Unit::Mm,
            &crate::expr::Env::empty(),
        )
        .unwrap();
        let p = Id::parse("p", "part").unwrap();
        let t = Id::parse("t", "feature").unwrap();
        let mut list = crate::part::order(s.project().part(&p).unwrap()).unwrap();
        if !list.contains(&t) {
            list.push(t.clone());
        }
        crate::save::save(s, &p, &[(&t, &f)], list).unwrap();
    }

    #[test]
    fn a_cosmetic_thread_on_a_shaft() {
        let (_dir, mut s) = drilled();
        let (side, top) = shaft(&mut s);
        let p = Id::parse("p", "part").unwrap();
        let t = Id::parse("t", "feature").unwrap();
        cosmetic(
            &mut s,
            json!({"faces": [side], "thread": {"standard": "iso_metric", "size": "M10"},
                   "from": top, "depth": "12 mm"}),
        );
        let r = s.rebuilt(&p).unwrap();
        let out = r.output(&t).unwrap();
        let crate::rebuild::Output::Thread(c) = out else {
            panic!("{out:?}")
        };
        let th = &c.thread;
        assert!(!th.internal);
        assert_eq!(th.class.as_deref(), Some("6g"));
        assert_eq!(th.designation, "M10x1.5-6g");
        // From the top (z = 20 mm) down 12 mm, on the shaft's axis.
        assert!(
            (th.mouth - Point3::new(0.040, 0.0, 0.020)).length() < 1e-12,
            "{th:?}"
        );
        assert!(
            (th.end - Point3::new(0.040, 0.0, 0.008)).length() < 1e-12,
            "{th:?}"
        );
        // Drawn at the minor diameter.
        assert!((th.ring_diameter() - 0.008376).abs() < 1e-12);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(th.to_json()["feature"], "p/t");
        // A 9.5 mm shaft is no M10 6g (9.732 to 9.968, widened by 0.1 mm).
        crate::testing::call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "shaft", "params": {"radius": "4.75 mm"}}),
        )
        .unwrap();
        let w = s.rebuilt(&p).unwrap().warnings(&p);
        let m = w
            .iter()
            .find(|x| x["code"] == "thread_size_mismatch")
            .unwrap();
        assert_eq!(m["feature"], "t");
        // 6H on a shaft is the wrong side: the feature fails at rebuild.
        cosmetic(
            &mut s,
            json!({"faces": [side], "thread": {"standard": "iso_metric", "size": "M10",
                   "class": "6H"}}),
        );
        let r = s.rebuilt(&p).unwrap();
        let (_, out) = r.features.iter().find(|(f, _)| f.as_str() == "t").unwrap();
        assert_eq!(out.as_ref().unwrap_err().details["reason"], "thread_side");
        // A unified stud's 2A isn't built in: the project's table gives it.
        cosmetic(
            &mut s,
            json!({"faces": [side], "thread": {"standard": "unc", "size": "3/8"}}),
        );
        let r = s.rebuilt(&p).unwrap();
        let (_, out) = r.features.iter().find(|(f, _)| f.as_str() == "t").unwrap();
        assert_eq!(
            out.as_ref().unwrap_err().details["reason"],
            "needs_project_table"
        );
    }
}
