//! Holes as the app sees them: each part's `holes.list`, by face for
//! hover and the selection menu, by feature for the tree, and each
//! tapped hole's cosmetic thread as two dashed rings.

use serde_json::Value;
use std::collections::BTreeMap;

/// A tapped hole's rings: its axis, the ends' centres and the radius.
pub type RingAxis = ([f64; 3], [f64; 3], [f64; 3], f64);

#[derive(Clone, Debug, PartialEq)]
pub struct HoleRow {
    pub reference: String,
    pub spec: String,
    pub feature: String,
    pub faces: Vec<String>,
    pub ring: Option<RingAxis>,
}

/// A part's holes, at the revision they were listed at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoleTable {
    pub revision: String,
    pub rows: Vec<HoleRow>,
    by_face: BTreeMap<String, usize>,
    /// Each cosmetic thread's feature (`p/t`) and rings: at an internal
    /// thread's major diameter, an external one's minor.
    pub threads: Vec<(String, RingAxis)>,
}

fn point(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

impl HoleTable {
    /// From `holes.list`'s answer.
    pub fn from_list(v: &Value) -> HoleTable {
        let mut t = HoleTable::default();
        for r in v["holes"].as_array().into_iter().flatten() {
            let mut faces = Vec::new();
            for f in r["faces"].as_object().into_iter().flat_map(|o| o.values()) {
                match f {
                    Value::String(s) => faces.push(s.clone()),
                    Value::Array(a) => {
                        faces.extend(a.iter().filter_map(|x| x.as_str().map(str::to_owned)));
                    }
                    _ => {}
                }
            }
            let th = &r["thread"];
            let ring = (|| {
                Some((
                    point(&th["axis"])?,
                    point(&th["mouth"])?,
                    point(&th["end"])?,
                    th["major"].as_f64()? / 2.0,
                ))
            })();
            let i = t.rows.len();
            for f in &faces {
                t.by_face.insert(f.clone(), i);
            }
            t.rows.push(HoleRow {
                reference: r["ref"].as_str().unwrap_or_default().to_owned(),
                spec: r["spec"].as_str().unwrap_or_default().to_owned(),
                feature: r["feature"].as_str().unwrap_or_default().to_owned(),
                faces,
                ring,
            });
        }
        for c in v["threads"].as_array().into_iter().flatten() {
            let ring = (|| {
                let radius = if c["internal"].as_bool()? {
                    c["major"].as_f64()?
                } else {
                    c["minor"][0].as_f64()?
                } / 2.0;
                Some((
                    point(&c["axis"])?,
                    point(&c["mouth"])?,
                    point(&c["end"])?,
                    radius,
                ))
            })();
            if let Some(r) = ring {
                let feature = c["feature"].as_str().unwrap_or_default().to_owned();
                t.threads.push((feature, r));
            }
        }
        t
    }

    /// Every ring, labelled: the tapped holes', then the cosmetic threads'.
    fn labelled(&self) -> Vec<(String, RingAxis)> {
        self.rows
            .iter()
            .filter_map(|r| Some((r.reference.clone(), r.ring?)))
            .chain(self.threads.iter().cloned())
            .collect()
    }

    /// Every ring: the tapped holes', then the cosmetic threads'.
    pub fn rings(&self) -> Vec<RingAxis> {
        self.labelled().into_iter().map(|(_, r)| r).collect()
    }

    pub fn by_face(&self, face: &str) -> Option<&HoleRow> {
        self.by_face.get(face).map(|&i| &self.rows[i])
    }

    /// How many holes a hole feature makes, its patterns' and mirrors'
    /// too.
    pub fn count(&self, feature: &str) -> usize {
        self.rows.iter().filter(|r| r.feature == feature).count()
    }

    /// Every face of every hole of the feature a face's hole belongs to.
    pub fn siblings(&self, face: &str) -> Vec<String> {
        let Some(h) = self.by_face(face) else {
            return Vec::new();
        };
        self.rows
            .iter()
            .filter(|r| r.feature == h.feature)
            .flat_map(|r| r.faces.clone())
            .collect()
    }
}

/// The hole a face reference (`<part>/<solid>/<face>`) is a face of, from
/// its part's listed holes.
pub fn hole<'a>(model: &'a crate::core::model::Model, face: &str) -> Option<&'a HoleRow> {
    let part = face.split('/').next()?;
    model.holes.get(part)?.by_face(face)
}

/// Every face of every hole the hole with `face` shares a feature with;
/// nothing for a face no hole has.
pub fn siblings(model: &crate::core::model::Model, face: &str) -> Vec<String> {
    let Some(part) = face.split('/').next() else {
        return Vec::new();
    };
    model
        .holes
        .get(part)
        .map(|t| t.siblings(face))
        .unwrap_or_default()
}

/// What hover says of a hole's face: its spec and its reference.
pub fn line(model: &crate::core::model::Model, face: &str) -> Option<String> {
    hole(model, face).map(|h| format!("{} · {}", h.spec, h.reference))
}

/// One ring: a closed polyline, dashed.
#[derive(Clone, Debug, PartialEq)]
pub struct Ring {
    pub hole: String,
    pub points: Vec<[f64; 3]>,
    pub dashed: bool,
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    (l > 0.0 && l.is_finite()).then(|| a.map(|x| x / l))
}

/// A ring at each end of each tapped hole's thread, 64 segments each.
pub fn marks(t: &HoleTable) -> Vec<Ring> {
    let mut out = Vec::new();
    for (label, (axis, mouth, end, radius)) in t.labelled() {
        let Some(n) = unit(axis) else {
            continue;
        };
        let seed = if n[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let Some(u) = unit(cross(n, seed)) else {
            continue;
        };
        let v = cross(n, u);
        for c in [mouth, end] {
            let points = (0..=64)
                .map(|i| {
                    let a = core::f64::consts::TAU * f64::from(i) / 64.0;
                    let (s, k) = ryu_math::sin_cos(a);
                    [0, 1, 2].map(|j| c[j] + radius * (k * u[j] + s * v[j]))
                })
                .collect();
            out.push(Ring {
                hole: label.clone(),
                points,
                dashed: true,
            });
        }
    }
    out
}

/// Every shown part's thread rings as bands, in the colour renders draw
/// them.
pub fn overlay(
    model: &crate::core::model::Model,
    pr: &crate::core::nav::Projector,
    size: [f64; 2],
    scale: f64,
) -> Vec<crate::core::sketch::Corner> {
    let mut out = Vec::new();
    for t in model.holes.values() {
        let rings = marks(t);
        crate::core::sketch::bands(
            rings
                .iter()
                .map(|r| (r.points.as_slice(), ryucad::render::THREAD, r.dashed)),
            pr,
            size,
            scale,
            crate::core::sketch::CURVE_WIDTH,
            &mut out,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table() -> HoleTable {
        HoleTable::from_list(&json!({"holes": [
            {"ref": "p/h/p1", "spec": "M6x1.0-6H THRU", "feature": "h", "solid": "b",
             "faces": {"wall": "p/b/00aa"},
             "thread": {"designation": "M6x1.0-6H", "major": 0.006, "internal": true,
                        "axis": [0.0, 0.0, -1.0], "mouth": [0.0, 0.0, 0.012], "end": [0.0, 0.0, 0.0]}},
            {"ref": "p/row/h/p1/1", "spec": "M6x1.0-6H THRU", "feature": "h", "solid": "b",
             "pattern": {"feature": "row", "seed": "h", "instance": [1]},
             "faces": {"wall": ["p/b/00bb", "p/b/00cc"]}},
            {"ref": "p/c/p1", "spec": "DIA 6 THRU", "feature": "c", "solid": "b",
             "faces": {"wall": "p/b/00dd"}}
        ], "gone": []}))
    }

    #[test]
    fn holes_by_face_and_feature() {
        let t = table();
        assert_eq!(
            t.by_face("p/b/00cc").map(|r| r.reference.as_str()),
            Some("p/row/h/p1/1")
        );
        assert_eq!(t.count("h"), 2);
        assert_eq!(t.siblings("p/b/00aa"), ["p/b/00aa", "p/b/00bb", "p/b/00cc"]);
        assert!(t.by_face("p/b/00ee").is_none());
    }

    #[test]
    fn a_holes_face_describes_its_spec_and_ref() {
        let mut m = crate::core::model::Model::default();
        m.holes.insert("p".into(), std::sync::Arc::new(table()));
        assert_eq!(
            line(&m, "p/b/00cc").as_deref(),
            Some("M6x1.0-6H THRU · p/row/h/p1/1")
        );
        assert_eq!(line(&m, "p/b/00ee"), None, "a face no hole has");
        assert_eq!(line(&m, "q/b/00aa"), None, "a part with no holes listed");
        assert_eq!(
            hole(&m, "p/b/00aa").map(|h| h.faces.clone()),
            Some(vec!["p/b/00aa".to_owned()])
        );
        assert_eq!(siblings(&m, "p/b/00dd"), ["p/b/00dd"]);
        assert_eq!(siblings(&m, "p/b/00bb").len(), 3);
        assert!(siblings(&m, "p/b/00ee").is_empty());
    }

    #[test]
    fn a_ring_at_each_end_of_each_thread() {
        let marks = marks(&table());
        assert_eq!(marks.len(), 2, "one thread, a ring at each end");
        for m in &marks {
            assert!(m.dashed);
            assert_eq!(m.points.len(), 65);
            for p in &m.points {
                let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
                assert!((r - 0.003).abs() < 1e-12, "a ring at the major diameter");
            }
        }
        assert_eq!(marks[0].points[0][2], 0.012);
        assert_eq!(marks[1].points[0][2], 0.0);
    }

    #[test]
    fn cosmetic_threads_give_rings_at_their_minor_diameter() {
        let t = HoleTable::from_list(&serde_json::json!({"holes": [], "threads": [{
            "feature": "p/t", "designation": "M10x1.5-6g", "internal": false,
            "major": 0.010, "minor": [0.008376, 0.008376],
            "axis": [0.0, 0.0, -1.0], "mouth": [0.04, 0.0, 0.02], "end": [0.04, 0.0, 0.008],
            "faces": ["p/shaft/00000000000000000000000000000001"]}]}));
        let rings = t.rings();
        assert_eq!(rings.len(), 1);
        assert!((rings[0].3 - 0.004188).abs() < 1e-12);
        let m = marks(&t);
        assert_eq!((m.len(), m[0].hole.as_str()), (2, "p/t"));
    }
}
