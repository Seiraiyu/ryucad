//! Rebuild properties: names survive edits, and results don't depend on
//! thread count.

use ryucad::doc::files::{Feature, PartFile, Placement, Primitive, PrimitiveType};
use ryucad::doc::id::Id;
use ryucad::doc::project::Part;
use ryucad::rebuild::{build_feature, rebuild_part};
use ryucad::units::{Quantity, Unit};
use ryukei::topo::Body;

fn id(s: &str) -> Id {
    Id::parse(s, "feature").unwrap()
}

fn feature(ty: PrimitiveType, params: &[(&str, &str)]) -> Feature {
    Feature::Primitive(Primitive {
        ty,
        params: params
            .iter()
            .map(|(k, v)| (k.to_string(), Quantity::parse(v, Unit::Mm).unwrap()))
            .collect(),
        placement: Placement::identity(Unit::Mm),
        combine: ryucad::doc::solid::Combine::NEW,
    })
}

fn part(features: Vec<(&str, Feature)>) -> Part {
    Part {
        id: id("p"),
        file: Ok(PartFile::new(features.iter().map(|(f, _)| id(f)).collect())),
        parameters: None,
        env: ryucad::expr::Env::empty(),
        uses: Default::default(),
        acts: Default::default(),
        imports: Default::default(),
        views: Default::default(),
        features: features.into_iter().map(|(f, x)| (id(f), Ok(x))).collect(),
        revision: String::new(),
    }
}

/// Every face, edge and vertex name, sorted.
fn names(b: &Body) -> Vec<String> {
    let mut n: Vec<String> = b
        .faces()
        .map(|f| b.face(f).unwrap().name.to_string())
        .chain(b.edges().map(|e| b.edge(e).unwrap().name.to_string()))
        .chain(b.vertices().map(|v| b.vertex(v).unwrap().name.to_string()))
        .collect();
    n.sort();
    n
}

#[test]
fn editing_params_and_placement_keeps_every_name() {
    let a = feature(PrimitiveType::Box, &[("x", "10"), ("y", "20"), ("z", "30")]);
    let mut b = feature(
        PrimitiveType::Box,
        &[("x", "11"), ("y", "2 in"), ("z", "3 m")],
    );
    let Feature::Primitive(p) = &mut b else {
        unreachable!()
    };
    p.placement.z = [1.0, 1.0, 0.0];
    let ba = build_feature(&id("p"), &id("f"), &a).unwrap();
    let bb = build_feature(&id("p"), &id("f"), &b).unwrap();
    assert_ne!(ba.hash, bb.hash);
    assert_eq!(names(&ba.body).len(), 6 + 12 + 8);
    assert_eq!(names(&ba.body), names(&bb.body));
}

#[test]
fn other_features_dont_rename_a_feature() {
    let cyl = || {
        feature(
            PrimitiveType::Cylinder,
            &[("radius", "5"), ("height", "10")],
        )
    };
    let ball = || feature(PrimitiveType::Sphere, &[("radius", "3")]);
    let one = rebuild_part(&part(vec![("c", cyl())]));
    let three = rebuild_part(&part(vec![("s", ball()), ("t", ball()), ("c", cyl())]));
    let get = |r: &ryucad::rebuild::PartResult, f: &str| {
        let (_, b) = r.features.iter().find(|(i, _)| i.as_str() == f).unwrap();
        b.as_ref().unwrap().body().unwrap().clone()
    };
    assert_eq!(names(&get(&one, "c").body), names(&get(&three, "c").body));
    assert_eq!(get(&one, "c").hash, get(&three, "c").hash);
    // The same type and parameters under another ID: same shape, new names.
    assert_ne!(names(&get(&three, "s").body), names(&get(&three, "t").body));
    assert_eq!(
        get(&three, "s").body.dump_unnamed(),
        get(&three, "t").body.dump_unnamed()
    );
}

#[test]
fn thread_count_doesnt_change_the_result() {
    let features: Vec<(String, Feature)> = (0..64)
        .map(|i| {
            let r = format!("{} mm", 1 + i);
            (
                format!("f{i}"),
                feature(
                    PrimitiveType::Torus,
                    &[
                        ("major_radius", &format!("{} mm", 100 + i)),
                        ("minor_radius", &r),
                    ],
                ),
            )
        })
        .collect();
    let p = part(
        features
            .iter()
            .map(|(f, x)| (f.as_str(), x.clone()))
            .collect(),
    );
    let run = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| rebuild_part(&p))
    };
    let (a, b) = (run(1), run(8));
    assert_eq!(a.hash, b.hash);
    let order = |r: &ryucad::rebuild::PartResult| {
        r.features
            .iter()
            .map(|(i, _)| i.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&a), order(&b));
    assert_eq!(order(&a), p.order());
}

#[test]
fn one_failure_doesnt_stop_the_others() {
    let mut p = part(vec![
        ("a", feature(PrimitiveType::Sphere, &[("radius", "1")])),
        (
            "b",
            feature(
                PrimitiveType::Torus,
                &[("major_radius", "1"), ("minor_radius", "1")],
            ),
        ),
        ("c", feature(PrimitiveType::Sphere, &[("radius", "2")])),
    ]);
    let r = rebuild_part(&p);
    assert_eq!(r.failed(), 1);
    assert!(r.features[0].1.is_ok() && r.features[2].1.is_ok());
    // A feature whose file is broken fails in its place, with its error.
    p.features.insert(
        id("a"),
        Err(ryucad::error::Error::new(
            ryucad::error::Code::InvalidJson,
            "bad",
        )),
    );
    let r2 = rebuild_part(&p);
    assert_eq!(r2.failed(), 2);
    assert_eq!(
        r2.features[0].1.as_ref().unwrap_err().code,
        ryucad::error::Code::InvalidJson
    );
    assert_ne!(r.hash, r2.hash);
}
