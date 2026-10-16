//! Features to ryukei bodies. Each phase 0 feature is one primitive with
//! its own body; they don't depend on each other, so they build in
//! parallel and are collected in feature order.

use crate::doc::files::{Feature, FeatureType, Placement};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use rayon::prelude::*;
use ryukei::geom::{Frame, Point3, Rigid, Vector3};
use ryukei::name::{Journal, OpKey};
use ryukei::primitive;
use ryukei::topo::Body;

/// A feature's rebuilt body.
#[derive(Clone, Debug)]
pub struct Built {
    pub body: Body,
    pub journal: Journal,
    /// BLAKE3 of the body's dump: equal hashes mean bit-identical bodies.
    pub hash: String,
}

/// A part's rebuild: each feature's body or error, in feature order.
#[derive(Clone, Debug)]
pub struct PartResult {
    pub features: Vec<(Id, Result<Built>)>,
    /// Combines the features' hashes (or error codes) in order.
    pub hash: String,
}

impl PartResult {
    pub fn failed(&self) -> usize {
        self.features.iter().filter(|(_, r)| r.is_err()).count()
    }
}

/// The ryukei key for a feature: names depend only on the part and feature
/// IDs, which never change, so editing parameters keeps every name.
pub fn op_key(part: &Id, feature: &Id) -> OpKey {
    let mut bytes = b"ryucad.feature\0".to_vec();
    bytes.extend_from_slice(part.as_str().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(feature.as_str().as_bytes());
    OpKey::from_bytes(&bytes)
}

/// The rigid placement of a feature's local frame. `x` is made
/// perpendicular to `z`; axes with no direction, or parallel ones, are
/// `invalid_placement`.
pub fn placement(p: &Placement) -> Result<Rigid> {
    let [ox, oy, oz] = p.origin.each_ref().map(|q| q.meters());
    let v = |a: [f64; 3]| Vector3::new(a[0], a[1], a[2]);
    let frame = Frame::new(Point3::new(ox, oy, oz), v(p.z), v(p.x)).map_err(|e| {
        let why = match e {
            ryukei::Error::Degenerate => "z and x must be non-zero and not parallel",
            ryukei::Error::OutsideSizeBox => "the origin is outside the ±500 m size box",
            _ => "the axes must be finite",
        };
        Error::new(Code::InvalidPlacement, format!("bad placement: {why}"))
            .with("kernel", format!("{e:?}"))
    })?;
    Ok(Rigid::from_frame(&frame))
}

/// Builds one feature and checks the result.
pub fn build_feature(part: &Id, id: &Id, f: &Feature) -> Result<Built> {
    let key = op_key(part, id);
    let at = placement(&f.placement)?;
    let m = |name| f.meters(name);
    let made = match f.ty {
        FeatureType::Box => primitive::block(key, m("x"), m("y"), m("z"), &at),
        FeatureType::Cylinder => primitive::cylinder(key, m("radius"), m("height"), &at),
        FeatureType::Cone if m("top_radius") > 0.0 => {
            primitive::frustum(key, m("radius"), m("top_radius"), m("height"), &at)
        }
        FeatureType::Cone => primitive::cone(key, m("radius"), m("height"), &at),
        FeatureType::Sphere => primitive::sphere(key, m("radius"), &at),
        FeatureType::Torus => primitive::torus(key, m("major_radius"), m("minor_radius"), &at),
    };
    let (body, journal) = made.map_err(|e| {
        Error::new(Code::RebuildFailed, format!("{} {id}: {e}", f.ty.as_str()))
            .with("feature", id)
            .with("kernel", format!("{e:?}"))
    })?;
    // A broken body is never reported as a success, in release builds too.
    let faults = ryukei::check::check(&body);
    if !faults.is_empty() {
        return Err(Error::new(
            Code::Internal,
            format!("{} {id}: the kernel built an invalid body", f.ty.as_str()),
        )
        .with("feature", id)
        .with(
            "faults",
            faults.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>(),
        ));
    }
    let hash = blake3::hash(body.dump().as_bytes()).to_hex()[..32].to_owned();
    Ok(Built {
        body,
        journal,
        hash,
    })
}

/// Rebuilds every listed feature of `part`, in parallel.
pub fn rebuild_part(part: &Part) -> PartResult {
    let features: Vec<(Id, Result<Built>)> = part
        .order()
        .par_iter()
        .map(|id| {
            let built = match &part.features[id] {
                Ok(f) => build_feature(&part.id, id, f),
                Err(e) => Err(e.clone()),
            };
            (id.clone(), built)
        })
        .collect();
    let mut h = blake3::Hasher::new();
    for (id, r) in &features {
        let piece = match r {
            Ok(b) => format!("{id} ok {}\n", b.hash),
            Err(e) => format!("{id} err {}\n", serde_json::json!(e.code)),
        };
        h.update(piece.as_bytes());
    }
    PartResult {
        features,
        hash: h.finalize().to_hex()[..32].to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::{Quantity, Unit};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    fn feature(ty: FeatureType, params: &[(&str, &str)]) -> Feature {
        Feature {
            ty,
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), Quantity::parse(v, Unit::Mm).unwrap()))
                .collect(),
            placement: Placement::identity(Unit::Mm),
        }
    }

    #[test]
    fn every_type_builds_a_valid_body() {
        for f in [
            feature(FeatureType::Box, &[("x", "10"), ("y", "20"), ("z", "30")]),
            feature(FeatureType::Cylinder, &[("radius", "5"), ("height", "10")]),
            feature(FeatureType::Cone, &[("radius", "5"), ("height", "10")]),
            feature(
                FeatureType::Cone,
                &[("radius", "5"), ("height", "10"), ("top_radius", "2")],
            ),
            feature(FeatureType::Sphere, &[("radius", "5")]),
            feature(
                FeatureType::Torus,
                &[("major_radius", "10"), ("minor_radius", "2")],
            ),
        ] {
            let b = build_feature(&id("p"), &id("f"), &f).unwrap();
            assert!(ryukei::check::check(&b.body).is_empty());
            assert_eq!(b.hash.len(), 32);
        }
    }

    #[test]
    fn placement_is_exact_for_axis_aligned_frames() {
        let mut p = Placement::identity(Unit::Mm);
        p.origin[2] = Quantity::parse("10 mm", Unit::Mm).unwrap();
        p.z = [0.0, 0.0, 2.0];
        p.x = [3.0, 0.0, 5.0];
        let r = placement(&p).unwrap();
        assert_eq!(
            r.apply_point(Point3::new(1.0, 0.0, 0.0)),
            Point3::new(1.0, 0.0, 0.01)
        );
        assert_eq!(
            r.apply_vector(Vector3::new(0.0, 1.0, 0.0)),
            Vector3::new(0.0, 1.0, 0.0)
        );
    }

    #[test]
    fn bad_placements() {
        for (z, x) in [
            ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            ([0.0, 0.0, 1.0], [0.0, 0.0, -3.0]),
        ] {
            let mut p = Placement::identity(Unit::Mm);
            p.z = z;
            p.x = x;
            assert_eq!(placement(&p).unwrap_err().code, Code::InvalidPlacement);
        }
        let mut p = Placement::identity(Unit::Mm);
        p.origin[0] = Quantity::parse("600 m", Unit::Mm).unwrap();
        let e = placement(&p).unwrap_err();
        assert_eq!(
            (e.code, e.details["kernel"].as_str()),
            (Code::InvalidPlacement, Some("OutsideSizeBox"))
        );
    }

    #[test]
    fn kernel_refusals_are_rebuild_failures() {
        // Equal radii make no cone; a torus whose tube reaches the axis is degenerate.
        for f in [
            feature(
                FeatureType::Cone,
                &[("radius", "5"), ("height", "10"), ("top_radius", "5")],
            ),
            feature(
                FeatureType::Torus,
                &[("major_radius", "2"), ("minor_radius", "2")],
            ),
            feature(FeatureType::Box, &[("x", "2000 m"), ("y", "1"), ("z", "1")]),
        ] {
            let e = build_feature(&id("p"), &id("f"), &f).unwrap_err();
            assert_eq!(e.code, Code::RebuildFailed, "{e}");
            assert_eq!(e.details["feature"], "f");
        }
    }

    #[test]
    fn keys_depend_on_both_ids() {
        assert_eq!(
            op_key(&id("p"), &id("f")),
            OpKey::from_bytes(b"ryucad.feature\0p\0f")
        );
        assert_ne!(op_key(&id("p"), &id("f")), op_key(&id("q"), &id("f")));
        assert_ne!(op_key(&id("p"), &id("f")), op_key(&id("p"), &id("g")));
    }
}
